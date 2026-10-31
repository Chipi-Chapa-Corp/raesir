//! Experimental read-only client IPC. Called only in a disposable helper process.
//! The client ABI is versioned but not a general-purpose launcher API contract.
use crate::model::{OwnedGame, OwnedLibrary};
use std::{fs, io::Read, path::Path};

pub fn catalog(path: &Path) -> Result<Vec<OwnedGame>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "Steam's appinfo cache is not available yet")?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Steam's appinfo cache")?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Steam appinfo cache is too large".into());
    }
    // The parser expects an entry-sized lookahead even at the four-byte EOF
    // marker. Padding our private snapshot handles short catalogs/v40 safely.
    bytes.resize(bytes.len() + 68, 0);
    let parsed = steam_vdf_parser::parse_appinfo(&bytes).map_err(|_| "Cannot parse Steam's appinfo cache; wait for Steam to finish loading its library and retry")?;
    let apps = parsed
        .as_obj()
        .ok_or("Steam appinfo cache has no app records")?;
    let mut games = Vec::new();
    for (id, app) in apps.iter() {
        let Ok(appid) = id.parse::<u32>() else {
            continue;
        };
        let app = app.get("appinfo").unwrap_or(app);
        let Some(title) = app.get_str(&["common", "name"]) else {
            continue;
        };
        let kind = app.get_str(&["common", "type"]).unwrap_or("");
        if !["game", "demo", "mod"]
            .iter()
            .any(|k| kind.eq_ignore_ascii_case(k))
        {
            continue;
        }
        games.push(OwnedGame {
            appid,
            name: title.into(),
        });
    }
    if games.is_empty() {
        return Err(
            "Steam's catalog has no game metadata yet. Open the Steam library and retry.".into(),
        );
    }
    Ok(games)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn probe(root: &Path) -> Result<OwnedLibrary, String> {
    use std::ffi::{c_char, c_void};
    type Obj = *mut c_void;
    type Create = unsafe extern "C" fn(*const c_char, *mut i32) -> Obj;
    type NewPipe = unsafe extern "C" fn(Obj) -> i32;
    type Connect = unsafe extern "C" fn(Obj, i32) -> i32;
    type GetInterface = unsafe extern "C" fn(Obj, i32, i32, *const c_char) -> Obj;
    type ReleaseUser = unsafe extern "C" fn(Obj, i32, i32);
    type ReleasePipe = unsafe extern "C" fn(Obj, i32) -> bool;
    type LoggedOn = unsafe extern "C" fn(Obj) -> bool;
    type GetId = unsafe extern "C" fn(Obj) -> u64;
    type Subscribed = unsafe extern "C" fn(Obj, u32) -> bool;
    // Exact vtable slots of SteamClient017, SteamUser019 and STEAMAPPS_INTERFACE_VERSION008.
    // Safety: only pointers from the matching versioned CreateInterface/GetISteam*
    // factories may be passed here, and the Library stays loaded for their lifetime.
    unsafe fn method<T: Copy>(object: Obj, slot: usize) -> T {
        unsafe {
            let table = *(object as *const *const *const c_void);
            std::mem::transmute_copy(&*table.add(slot))
        }
    }
    struct Session {
        client: Obj,
        pipe: i32,
        user: i32,
    }
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                if self.user != 0 {
                    method::<ReleaseUser>(self.client, 4)(self.client, self.pipe, self.user);
                }
                if self.pipe != 0 {
                    method::<ReleasePipe>(self.client, 1)(self.client, self.pipe);
                }
            }
        }
    }
    let candidates = catalog(&root.join("appcache/appinfo.vdf"))?;
    // No credentials/tokens are read, copied, or passed to the helper.
    unsafe {
        let library = libloading::Library::new(root.join("linux64/steamclient.so"))
            .map_err(|_| "Cannot load the installed Steam client library in this runtime")?;
        let create: libloading::Symbol<Create> = library
            .get(b"CreateInterface\0")
            .map_err(|_| "Steam client factory is unavailable")?;
        let client = create(c"SteamClient017".as_ptr(), std::ptr::null_mut());
        if client.is_null() {
            return Err("SteamClient017 is not supported by this client build".into());
        }
        let mut session = Session {
            client,
            pipe: method::<NewPipe>(client, 0)(client),
            user: 0,
        };
        if session.pipe == 0 {
            return Err("Steam is not ready for client IPC. Finish signing in and retry.".into());
        }
        session.user = method::<Connect>(client, 2)(client, session.pipe);
        if session.user == 0 {
            return Err("Steam has no logged-in global user. Finish signing in and retry.".into());
        }
        let user = method::<GetInterface>(client, 5)(
            client,
            session.user,
            session.pipe,
            c"SteamUser019".as_ptr(),
        );
        let apps = method::<GetInterface>(client, 15)(
            client,
            session.user,
            session.pipe,
            c"STEAMAPPS_INTERFACE_VERSION008".as_ptr(),
        );
        if user.is_null() || apps.is_null() {
            return Err("Steam's user/apps interface is unavailable".into());
        }
        if !method::<LoggedOn>(user, 1)(user) {
            return Err(
                "Steam is not signed in and online yet. Finish login in Steam, then retry.".into(),
            );
        }
        let steamid = method::<GetId>(user, 2)(user);
        if steamid == 0 {
            return Err("Steam did not identify an active account".into());
        }
        let subscribed = method::<Subscribed>(apps, 6);
        let games: Vec<_> = candidates
            .into_iter()
            .filter(|g| subscribed(apps, g.appid))
            .collect();
        if method::<GetId>(user, 2)(user) != steamid || !method::<LoggedOn>(user, 1)(user) {
            return Err("The Steam account/session changed during lookup. Retry after signing in; the previous list was kept.".into());
        }
        if games.is_empty() {
            return Err("Steam returned no available game licenses yet. Wait for its library to load and retry; the previous list was kept.".into());
        }
        // Session is dropped before the native library, releasing only our pipe.
        drop(session);
        Ok(OwnedLibrary {
            steamid: steamid.to_string(),
            games,
            source: "steam_client".into(),
        })
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn probe(_root: &Path) -> Result<OwnedLibrary, String> {
    Err("The experimental Steam client-auth bridge currently supports Linux x86_64. Use the Steam Web API lookup on this platform.".into())
}

/// Entrypoint used by the desktop executable before starting the UI.
pub fn helper(root: &Path, result: &Path) {
    let output = std::panic::catch_unwind(|| probe(root))
        .unwrap_or_else(|_| Err("Steam cache parsing failed".into()));
    if let Ok(json) = serde_json::to_vec(&output) {
        let _ = fs::write(result, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_cache_is_an_error_not_an_empty_owned_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appinfo.vdf");
        fs::write(&path, b"not an appinfo file").unwrap();
        assert!(catalog(&path).is_err());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn native_abi_filters_catalog_and_releases_its_own_pipe() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("linux64")).unwrap();
        fs::create_dir_all(dir.path().join("appcache")).unwrap();
        let status = Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(dir.path().join("linux64/steamclient.so"))
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/steamclient.c"))
            .status()
            .unwrap();
        assert!(status.success());
        let mut entries = Vec::new();
        for (id, name) in [(42u32, "Licensed fixture"), (43u32, "Unlicensed fixture")] {
            let mut kv = vec![0];
            kv.extend(0u32.to_le_bytes()); // appinfo
            kv.push(0);
            kv.extend(1u32.to_le_bytes()); // common
            kv.push(1);
            kv.extend(2u32.to_le_bytes());
            kv.extend(name.as_bytes());
            kv.push(0);
            kv.push(1);
            kv.extend(3u32.to_le_bytes());
            kv.extend(b"game\0");
            kv.extend([8, 8, 8]);
            entries.extend(id.to_le_bytes());
            entries.extend((60u32 + kv.len() as u32).to_le_bytes());
            entries.extend([0u8; 60]);
            entries.extend(kv);
        }
        entries.extend(0u32.to_le_bytes());
        let mut bytes = 0x07564429u32.to_le_bytes().to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend((16u64 + entries.len() as u64).to_le_bytes());
        bytes.extend(entries);
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(b"appinfo\0common\0name\0type\0");
        fs::write(dir.path().join("appcache/appinfo.vdf"), bytes).unwrap();
        let owned = probe(dir.path()).unwrap();
        assert_eq!(owned.steamid, "76561198000000000");
        assert_eq!(owned.source, "steam_client");
        assert_eq!(owned.games.len(), 1);
        assert_eq!(owned.games[0].appid, 42);
        // Fixture refuses a new pipe if the previous pipe/user were not released.
        assert!(probe(dir.path()).is_ok());
    }
}
