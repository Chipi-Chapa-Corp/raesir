//! A disposable IPC helper for Steam-owned launches and temporary launch options.
//! Signatures: OpenSteamClient/OpenSteamworks Generated/IClientConfigStore.cs and
//! IClientAppManager.cs. The proxy markers below were checked against the local
//! Linux Steam client before enabling mutations; an unknown ABI fails closed.
use super::*;
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Read {
        appid: u32,
    },
    Set {
        appid: u32,
        expected: Option<String>,
        value: Option<String>,
    },
    Launch {
        appid: u32,
        args: String,
    },
}

pub(super) fn call(root: &Path, request: Request) -> Result<Option<String>, String> {
    let input = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    crate::storage::write_json(input.path(), &request)?;
    let output = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--steam-mods")
        .arg(root)
        .arg(input.path())
        .arg(output.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the Steam launch helper: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err("Steam's launch helper exited unexpectedly".into());
            }
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Steam's launch helper timed out".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    serde_json::from_slice::<Result<Option<String>, String>>(
        &fs::read(output.path()).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?
}

/// Executable entrypoint, handled before creating any UI or backend state.
pub fn helper(root: &Path, input: &Path, output: &Path) {
    let result = (|| {
        let request: Request = serde_json::from_slice(&fs::read(input).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if !matches!(&request, Request::Read { .. })
            && std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
            && !super::diagnostic_launch_enabled()
        {
            return Err("Steam launch changes are disabled in this diagnostic profile".into());
        }
        request_impl(root, request)
    })();
    let _ = crate::storage::write_json(output, &result);
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn request_impl(_: &Path, _: Request) -> Result<Option<String>, String> {
    Err("Steam's mod launch helper is not available on this platform".into())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn request_impl(root: &Path, request: Request) -> Result<Option<String>, String> {
    use std::ffi::{CStr, CString, c_char, c_void};
    type Object = *mut c_void;
    unsafe fn method<T: Copy>(object: Object, slot: usize) -> T {
        unsafe { std::mem::transmute_copy(&*(*(object as *const *const *const c_void)).add(slot)) }
    }
    unsafe fn identity(object: Object, name: &[u8]) -> bool {
        if object.is_null() {
            return false;
        }
        unsafe {
            let table = *(object as *const *const *const c_void);
            let info = *table.sub(1) as *const *const c_char;
            CStr::from_ptr(*info.add(1)).to_bytes() == name
        }
    }
    unsafe fn signature(object: Object, slot: usize, ids: [u32; 2]) -> bool {
        unsafe {
            let ptr: *const u8 = method(object, slot);
            let bytes = std::slice::from_raw_parts(ptr, 512);
            ids.iter()
                .all(|id| bytes.windows(4).any(|b| b == id.to_le_bytes()))
        }
    }
    unsafe {
        let library = libloading::Library::new(root.join("linux64/steamclient.so"))
            .map_err(|e| e.to_string())?;
        let create: libloading::Symbol<unsafe extern "C" fn(*const c_char, *mut i32) -> Object> =
            library
                .get(b"CreateInterface\0")
                .map_err(|e| e.to_string())?;
        let client = create(c"SteamClient017".as_ptr(), std::ptr::null_mut());
        let engine = create(
            c"CLIENTENGINE_INTERFACE_VERSION005".as_ptr(),
            std::ptr::null_mut(),
        );
        if client.is_null() || engine.is_null() {
            return Err("Steam's client interface is unavailable".into());
        }
        struct Session {
            client: Object,
            pipe: i32,
            user: i32,
        }
        impl Drop for Session {
            fn drop(&mut self) {
                unsafe {
                    if self.user != 0 {
                        method::<unsafe extern "C" fn(Object, i32, i32)>(self.client, 4)(
                            self.client,
                            self.pipe,
                            self.user,
                        );
                    }
                    if self.pipe != 0 {
                        method::<unsafe extern "C" fn(Object, i32) -> bool>(self.client, 1)(
                            self.client,
                            self.pipe,
                        );
                    }
                }
            }
        }
        let mut session = Session {
            client,
            pipe: method::<unsafe extern "C" fn(Object) -> i32>(client, 0)(client),
            user: 0,
        };
        if session.pipe == 0 {
            return Err("Open Steam and sign in before launching modded".into());
        }
        session.user =
            method::<unsafe extern "C" fn(Object, i32) -> i32>(client, 2)(client, session.pipe);
        if session.user == 0 {
            return Err("Open Steam and sign in before launching modded".into());
        }
        type Get = unsafe extern "C" fn(Object, i32, i32) -> Object;
        let config = method::<Get>(engine, 44)(engine, session.user, session.pipe);
        if !identity(config, b"21IClientConfigStoreMap")
            || !signature(config, 5, [0x51d86418, 0x5544d1ef])
            || !signature(config, 13, [0xb94bf8da, 0xbcc085d2])
            || !signature(config, 16, [0x92bb8526, 0x96348030])
            || !signature(config, 18, [0x49640c54, 0x4cdfbc69])
        {
            return Err(
                "Steam's launch-options interface changed; no settings were modified".into(),
            );
        }
        let appid = match &request {
            Request::Read { appid }
            | Request::Set { appid, .. }
            | Request::Launch { appid, .. } => *appid,
        };
        if appid == 0 {
            return Err("Invalid Steam app ID".into());
        }
        let key = CString::new(format!("Software/Valve/Steam/apps/{appid}/LaunchOptions")).unwrap();
        type Read =
            unsafe extern "C" fn(Object, i32, *const c_char, *const c_char) -> *const c_char;
        let read = || -> Result<Option<String>, String> {
            let value = method::<Read>(config, 5)(
                config,
                3,
                key.as_ptr(),
                c"__RAESIR_UNSET_LAUNCH_OPTIONS__".as_ptr(),
            );
            if value.is_null() {
                return Err("Steam did not return launch options".into());
            }
            let value = CStr::from_ptr(value).to_str().map_err(|e| e.to_string())?;
            if value.len() > 32768 {
                return Err("Steam launch options are too long".into());
            }
            Ok((value != "__RAESIR_UNSET_LAUNCH_OPTIONS__").then(|| value.to_owned()))
        };
        match request {
            Request::Read { .. } => read(),
            Request::Set {
                expected, value, ..
            } => {
                if read()? != expected {
                    return Err(
                        "Steam launch options changed; your changes were kept. Retry the launch."
                            .into(),
                    );
                }
                let ok = if let Some(value) = &value {
                    let value =
                        CString::new(value.as_str()).map_err(|_| "Invalid launch options")?;
                    method::<unsafe extern "C" fn(Object, i32, *const c_char, *const c_char) -> bool>(
                        config, 13,
                    )(config, 3, key.as_ptr(), value.as_ptr())
                } else {
                    method::<unsafe extern "C" fn(Object, i32, *const c_char) -> bool>(config, 16)(
                        config,
                        3,
                        key.as_ptr(),
                    )
                };
                if !ok || read()? != value {
                    return Err("Steam did not accept the launch-options change".into());
                }
                if !method::<unsafe extern "C" fn(Object, bool) -> bool>(config, 18)(config, false)
                {
                    return Err("Steam could not save its launch options".into());
                }
                Ok(value)
            }
            Request::Launch { args, .. } => {
                let manager = method::<Get>(engine, 43)(engine, session.user, session.pipe);
                if !identity(manager, b"20IClientAppManagerMap")
                    || !signature(manager, 2, [0x0f248f48, 0x11440057])
                {
                    return Err("Steam's launch interface changed; the game was not started".into());
                }
                let game_id = appid as u64;
                let args = CString::new(args).map_err(|_| "Invalid game launch arguments")?;
                type Launch =
                    unsafe extern "C" fn(Object, *const u64, u32, u32, *const c_char) -> u64;
                // Launch option 0 is the client's default; 300 denotes its
                // command-line launcher. Steam owns the runtime and process.
                if method::<Launch>(manager, 2)(manager, &game_id, 0, 300, args.as_ptr()) == 0 {
                    return Err("Steam rejected the modded launch".into());
                }
                Ok(None)
            }
        }
    }
}
