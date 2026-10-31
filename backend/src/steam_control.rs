//! Steam-owned installs: a small, isolated bridge to the running client's app manager.
//!
//! This is a private client ABI, not the Steamworks game SDK. Never call it in the UI
//! process. The Linux bridge checks the interface identity and verified proxy markers
//! before using the current InstallApp(app, library, false) / UninstallApp(app) ABI.
//! Reference types/state flags: SteamRE/open-steamworks, AppsCommon.h. The current
//! signatures were checked against Steam's Linux client and tested with SpiderHeck.
use crate::{steam, vdf};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Install,
    Uninstall,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    Unavailable,
    SignedOut,
    NotOwned,
    Running,
    Disk,
    Network,
    Busy,
    Rejected,
}

impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "Steam's background installation controls are unavailable. Open Steam to manage this game."
            }
            Self::SignedOut => "Sign in to the local Steam client, then try again.",
            Self::NotOwned => "This game is not available to the current Steam account.",
            Self::Running => "Close the game before uninstalling it.",
            Self::Disk => {
                "Steam could not write the game files. Check the library's free space and permissions."
            }
            Self::Network => "Steam could not connect. Check its connection, then try again.",
            Self::Busy => "Steam is busy with this game. Wait for its current operation to finish.",
            Self::Rejected => {
                "Steam could not complete this request. Check this game in Steam before retrying."
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub state: u32,
    pub library: u32,
    pub download_total: Option<u64>,
    pub downloaded: Option<u64>,
}

impl Status {
    pub fn progress(&self, operation: Operation) -> Progress {
        let done = self.downloaded.unwrap_or(0);
        let total = self.download_total.unwrap_or(0);
        let mut progress = state_progress(self.state as u64, 0, done, total, operation);
        // GetAppInstallState exposes aggregate Running/Queued bits, without the
        // manifest's detailed Downloading/Staging bits. Use the live stage counters
        // while the client is running this update, rather than calling it queued.
        if operation == Operation::Install
            && self.state & 256 != 0
            && self.state & 512 == 0
            && total > 0
        {
            progress.phase = if done < total {
                Phase::Downloading
            } else {
                Phase::Installing
            };
            progress.percent = Some((done.saturating_mul(100) / total).min(100) as u8);
        }
        progress
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Queued,
    Downloading,
    Installing,
    Paused,
    Removing,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub operation: Operation,
    pub phase: Phase,
    pub percent: Option<u8>,
    pub error: Option<Error>,
}

impl Progress {
    pub fn starting(operation: Operation) -> Self {
        Self {
            operation,
            phase: if operation == Operation::Install {
                Phase::Starting
            } else {
                Phase::Removing
            },
            percent: None,
            error: None,
        }
    }
    pub fn active(&self) -> bool {
        !matches!(self.phase, Phase::Complete | Phase::Failed)
    }
    pub fn label(&self) -> String {
        match self.phase {
            Phase::Starting => "Starting…".into(),
            Phase::Queued => "Queued".into(),
            Phase::Downloading => self
                .percent
                .map(|p| format!("Downloading {p}%"))
                .unwrap_or_else(|| "Downloading…".into()),
            Phase::Installing => "Installing…".into(),
            Phase::Paused => "Paused in Steam".into(),
            Phase::Removing => "Uninstalling…".into(),
            Phase::Complete => match self.operation {
                Operation::Install => "Installed",
                Operation::Uninstall => "Uninstalled",
            }
            .into(),
            Phase::Failed => match self.operation {
                Operation::Install => "Installation stopped",
                Operation::Uninstall => "Uninstall stopped",
            }
            .into(),
        }
    }
}

/// Only inspect local libraries belonging to the selected client. Foreign drives
/// and unrelated clients' copies never participate in an installation operation.
pub fn libraries(root: &Path) -> Vec<(u32, PathBuf)> {
    let mut result = vec![(0, root.into())];
    if let Ok(text) = steam::read_text(&root.join("steamapps/libraryfolders.vdf"))
        && let Ok(value) = vdf::parse(&text)
        && let Some(entries) = value.get("libraryfolders").and_then(|v| v.entries())
    {
        for (index, value) in entries {
            if let (Ok(index), Some(path)) = (index.parse(), value.field("path")) {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    result.retain(|(i, _)| *i != index);
                    result.push((index, path));
                }
            }
        }
    }
    result.sort_by_key(|(index, _)| *index);
    result
}

/// Locate an existing install, retaining disconnected library paths so a lost
/// drive cannot be mistaken for a successful removal.
pub fn app_library(root: &Path, appid: u32) -> Option<PathBuf> {
    libraries(root)
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| {
            path.join(format!("steamapps/appmanifest_{appid}.acf"))
                .is_file()
        })
}

pub fn progress(root: &Path, appid: u32, operation: Operation) -> Progress {
    let library = app_library(root, appid).unwrap_or_else(|| root.into());
    progress_in_library(&library, appid, operation)
}

pub fn progress_in_library(library: &Path, appid: u32, operation: Operation) -> Progress {
    let failed = || Progress {
        phase: Phase::Failed,
        error: Some(Error::Disk),
        ..Progress::starting(operation)
    };
    if !library.join("steamapps").is_dir() {
        return failed();
    }
    let path = library.join(format!("steamapps/appmanifest_{appid}.acf"));
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut state = Progress::starting(operation);
            if operation == Operation::Uninstall {
                state.phase = Phase::Complete;
            }
            return state;
        }
        Err(_) => return failed(),
    };
    if let Ok(value) = vdf::parse(&text)
        && let Some(app) = value.get("AppState")
        && app.field("appid").and_then(|s| s.parse::<u32>().ok()) == Some(appid)
    {
        let progress = manifest_progress(app, operation);
        if progress.phase == Phase::Complete {
            let present = app
                .field("installdir")
                .filter(|dir| {
                    !dir.is_empty()
                        && !dir.contains(['/', '\\', ':'])
                        && *dir != "."
                        && *dir != ".."
                })
                .is_some_and(|dir| library.join("steamapps/common").join(dir).is_dir());
            if !present {
                return failed();
            }
        }
        return progress;
    }
    // Steam rewrites these files; a partial snapshot is not a completed removal.
    Progress::starting(operation)
}

fn manifest_progress(app: &vdf::Value, operation: Operation) -> Progress {
    let number = |key| {
        app.field(key)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    state_progress(
        number("StateFlags"),
        number("UpdateResult") as u32,
        number("BytesDownloaded"),
        number("BytesToDownload"),
        operation,
    )
}

fn state_progress(
    flags: u64,
    result: u32,
    downloaded: u64,
    total: u64,
    operation: Operation,
) -> Progress {
    let mut state = Progress::starting(operation);
    if operation == Operation::Uninstall {
        return state;
    }
    // Steam may report a suspended shader pass while scheduling the actual game.
    // Pauses and suspension are normal client state, not an error notification.
    state.phase = if flags & 4 != 0 && flags & !(2 | 4 | 64) == 0 {
        Phase::Complete
    } else if flags & 512 != 0 {
        Phase::Paused
    } else if flags & 1048576 != 0 {
        Phase::Downloading
    } else if flags & (2097152 | 4194304 | 131072 | 524288 | 65536 | 262144) != 0 {
        Phase::Installing
    } else if flags & (256 | 1024) != 0 {
        Phase::Queued
    } else {
        Phase::Starting
    };
    if matches!(
        state.phase,
        Phase::Downloading | Phase::Installing | Phase::Paused
    ) && total > 0
    {
        state.percent = Some((downloaded.saturating_mul(100) / total).min(100) as u8);
    }
    if !matches!(
        state.phase,
        Phase::Complete | Phase::Downloading | Phase::Installing
    ) && !matches!(result, 0..=4 | 14 | 20)
    {
        state.phase = Phase::Failed;
        state.error = Some(update_error(result));
    }
    state
}

fn update_error(code: u32) -> Error {
    match code {
        5 => Error::NotOwned,
        6 | 7 | 21 => Error::Network,
        10 | 11 | 15 | 30 => Error::Disk,
        16 => Error::Running,
        14 | 20 => Error::Busy,
        _ => Error::Rejected,
    }
}

/// Called by the executable before any UI initialization, in a disposable child.
pub fn helper(root: &Path, appid: u32, operation: &str, output: &Path) {
    let result = std::panic::catch_unwind(|| request(root, appid, operation))
        .unwrap_or(Err(Error::Unavailable));
    if let Ok(bytes) = serde_json::to_vec(&result) {
        let _ = fs::write(output, bytes);
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn request(_root: &Path, _appid: u32, _operation: &str) -> Result<Status, Error> {
    Err(Error::Unavailable)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn request(root: &Path, appid: u32, operation: &str) -> Result<Status, Error> {
    use std::ffi::{CStr, c_char, c_void};
    type Object = *mut c_void;
    unsafe fn method<T: Copy>(object: Object, slot: usize) -> T {
        unsafe { std::mem::transmute_copy(&*(*(object as *const *const *const c_void)).add(slot)) }
    }
    unsafe fn identity(object: Object) -> bool {
        if object.is_null() {
            return false;
        }
        unsafe {
            let table = *(object as *const *const *const c_void);
            let info = *table.sub(1) as *const *const c_char;
            CStr::from_ptr(*info.add(1)).to_bytes() == b"20IClientAppManagerMap"
        }
    }
    // Markers from the verified client proxies supplement the RTTI/slot checks.
    // Do not guess slots or invoke another signature after a client update.
    unsafe fn signature(object: Object, slot: usize, ids: [u32; 2]) -> bool {
        unsafe {
            let address: *const u8 = method(object, slot);
            let code = std::slice::from_raw_parts(address, 512);
            ids.iter()
                .all(|id| code.windows(4).any(|s| s == id.to_le_bytes()))
        }
    }
    if !matches!(
        operation,
        "probe" | "status" | "install" | "uninstall" | "pause" | "resume"
    ) {
        return Err(Error::Rejected);
    }
    if matches!(operation, "install" | "uninstall" | "pause" | "resume")
        && std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
    {
        return Err(Error::Unavailable);
    }
    unsafe {
        let library = libloading::Library::new(root.join("linux64/steamclient.so"))
            .map_err(|_| Error::Unavailable)?;
        let create: libloading::Symbol<unsafe extern "C" fn(*const c_char, *mut i32) -> Object> =
            library
                .get(b"CreateInterface\0")
                .map_err(|_| Error::Unavailable)?;
        let client = create(c"SteamClient017".as_ptr(), std::ptr::null_mut());
        let engine = create(
            c"CLIENTENGINE_INTERFACE_VERSION005".as_ptr(),
            std::ptr::null_mut(),
        );
        if client.is_null() || engine.is_null() {
            return Err(Error::Unavailable);
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
            return Err(Error::SignedOut);
        }
        session.user =
            method::<unsafe extern "C" fn(Object, i32) -> i32>(client, 2)(client, session.pipe);
        if session.user == 0 {
            return Err(Error::SignedOut);
        }
        type Get = unsafe extern "C" fn(Object, i32, i32, *const c_char) -> Object;
        let user =
            method::<Get>(client, 5)(client, session.user, session.pipe, c"SteamUser019".as_ptr());
        let apps = method::<Get>(client, 15)(
            client,
            session.user,
            session.pipe,
            c"STEAMAPPS_INTERFACE_VERSION008".as_ptr(),
        );
        if user.is_null() || apps.is_null() {
            return Err(Error::Unavailable);
        }
        if method::<unsafe extern "C" fn(Object) -> u64>(user, 2)(user) == 0 {
            return Err(Error::SignedOut);
        }
        let manager = method::<Get>(engine, 43)(
            engine,
            session.user,
            session.pipe,
            c"CLIENTAPPMANAGER_INTERFACE_VERSION001".as_ptr(),
        );
        if !identity(manager)
            || !signature(manager, 0, [0xdd9af16a, 0xdfc1c497])
            || !signature(manager, 1, [0x4505e2cd, 0x47325f11])
            || !signature(manager, 4, [0xeaf261b3, 0xed1250c4])
            || !signature(manager, 82, [0x166ebab8, 0x18c97bb8])
            || !signature(manager, 20, [0x27b7a56e, 0x29ecbed5])
            || !signature(manager, 56, [0x9bd943b1, 0x9e250174])
        {
            return Err(Error::Unavailable);
        }
        // The current proxy copies a 248-byte update-info record. Refuse another
        // output size before passing a buffer, even if the method IDs still match.
        let update_code: *const u8 = method(manager, 20);
        if !std::slice::from_raw_parts(update_code, 512)
            .windows(5)
            .any(|bytes| bytes == [0xba, 0xf8, 0, 0, 0])
        {
            return Err(Error::Unavailable);
        }
        if operation == "probe" {
            return Ok(Status {
                state: 0,
                library: 0,
                download_total: None,
                downloaded: None,
            });
        }
        if appid == 0 {
            return Err(Error::Rejected);
        }
        let state = method::<unsafe extern "C" fn(Object, u32) -> u32>(manager, 4)(manager, appid);
        let folder =
            method::<unsafe extern "C" fn(Object, u32) -> u32>(manager, 82)(manager, appid);
        let folder = if folder == u32::MAX { 0 } else { folder };
        if operation == "status" {
            // AppUpdateInfo's current network stage counters (done/total) are at 0x80/0x88, verified
            // against the client proxy and a live download. Manifests are snapshots,
            // so they cannot supply live transfer progress.
            let mut update = [0u64; 31];
            let valid = method::<unsafe extern "C" fn(Object, u32, *mut u64) -> bool>(manager, 20)(
                manager,
                appid,
                update.as_mut_ptr(),
            );
            let valid = valid && update[16] <= update[17];
            return Ok(Status {
                state,
                library: folder,
                download_total: valid.then_some(update[17]),
                downloaded: valid.then_some(update[16]),
            });
        }
        if state & 8192 != 0 {
            return Err(Error::Running);
        }
        if matches!(operation, "install" | "resume")
            && !method::<unsafe extern "C" fn(Object, u32) -> bool>(apps, 6)(apps, appid)
        {
            return Err(Error::NotOwned);
        }
        if operation == "install" && !method::<unsafe extern "C" fn(Object) -> bool>(user, 1)(user)
        {
            return Err(Error::Network);
        }
        if matches!(operation, "pause" | "resume") {
            // These are the placements used by Steam's own Downloads.PauseAppUpdate
            // (6) and ResumeAppUpdate (1) handlers. They affect this app only.
            if !method::<unsafe extern "C" fn(Object, u32, u32) -> bool>(manager, 56)(
                manager,
                appid,
                if operation == "pause" { 6 } else { 1 },
            ) {
                return Err(Error::Rejected);
            }
            return Ok(Status {
                state,
                library: folder,
                download_total: None,
                downloaded: None,
            });
        }
        let result = if operation == "install" {
            if state & 4 != 0 && state & !(2 | 4 | 64) == 0 {
                return Ok(Status {
                    state,
                    library: folder,
                    download_total: None,
                    downloaded: None,
                });
            }
            // Keep partial installs in their existing library. New installs use the
            // main local library (0). Steam owns depot/Proton selection and scheduling.
            method::<unsafe extern "C" fn(Object, u32, u32, bool) -> u32>(manager, 0)(
                manager, appid, folder, false,
            )
        } else {
            if state & 1 != 0 {
                return Ok(Status {
                    state,
                    library: folder,
                    download_total: None,
                    downloaded: None,
                });
            }
            method::<unsafe extern "C" fn(Object, u32) -> u32>(manager, 1)(manager, appid)
        };
        if result != 0 {
            return Err(update_error(result));
        }
        Ok(Status {
            state: method::<unsafe extern "C" fn(Object, u32) -> u32>(manager, 4)(manager, appid),
            library: folder,
            download_total: None,
            downloaded: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(flags: u32, result: u32, bytes: u64, total: u64) -> vdf::Value {
        vdf::parse(&format!(r#""AppState" {{ "appid" "42" "installdir" "Game" "StateFlags" "{flags}" "UpdateResult" "{result}" "BytesDownloaded" "{bytes}" "BytesToDownload" "{total}" }}"#)).unwrap()
    }

    #[test]
    fn download_completion_waits_for_commit_and_pauses_are_not_errors() {
        for (flags, result, expected) in [
            (2 | 1048576, 0, Phase::Downloading),
            (2 | 1048576 | 2097152, 0, Phase::Downloading),
            (2 | 4194304, 0, Phase::Installing),
            (4 | 4194304, 0, Phase::Installing),
            (4, 0, Phase::Complete),
            (2 | 512, 2, Phase::Paused),
            (2 | 1024, 4, Phase::Queued),
        ] {
            let value = manifest(flags, result, 100, 100);
            let progress = manifest_progress(value.get("AppState").unwrap(), Operation::Install);
            assert_eq!(progress.phase, expected);
            assert!(progress.error.is_none());
            assert_eq!(
                progress.percent,
                matches!(
                    expected,
                    Phase::Downloading | Phase::Installing | Phase::Paused
                )
                .then_some(100)
            );
        }
        let value = manifest(2 | 1048576, 0, 25, 100);
        assert_eq!(
            manifest_progress(value.get("AppState").unwrap(), Operation::Install).percent,
            Some(25)
        );
        for (result, error) in [(5, Error::NotOwned), (6, Error::Network), (11, Error::Disk)] {
            let value = manifest(2, result, 0, 100);
            let progress = manifest_progress(value.get("AppState").unwrap(), Operation::Install);
            assert_eq!(progress.phase, Phase::Failed);
            assert_eq!(progress.error, Some(error));
        }
    }

    #[test]
    fn missing_or_rewritten_manifests_never_claim_a_completed_install() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("steamapps")).unwrap();
        assert_eq!(
            progress(dir.path(), 42, Operation::Install).phase,
            Phase::Starting
        );
        assert_eq!(
            progress(dir.path(), 42, Operation::Uninstall).phase,
            Phase::Complete
        );
        let path = dir.path().join("steamapps/appmanifest_42.acf");
        fs::write(&path, "\"AppState\" {").unwrap();
        assert_eq!(
            progress(dir.path(), 42, Operation::Uninstall).phase,
            Phase::Removing
        );
        fs::write(
            &path,
            r#""AppState" { "appid" "42" "installdir" "Game" "StateFlags" "4" }"#,
        )
        .unwrap();
        assert_ne!(
            progress(dir.path(), 42, Operation::Install).phase,
            Phase::Complete
        );
        fs::create_dir_all(dir.path().join("steamapps/common/Game")).unwrap();
        assert_eq!(
            progress(dir.path(), 42, Operation::Install).phase,
            Phase::Complete
        );
        assert_eq!(
            progress(dir.path(), 42, Operation::Uninstall).phase,
            Phase::Removing
        );
        fs::remove_dir_all(dir.path().join("steamapps")).unwrap();
        assert_eq!(
            progress(dir.path(), 42, Operation::Uninstall).phase,
            Phase::Failed
        );
    }

    #[test]
    fn losing_the_selected_library_never_completes_a_removal() {
        let root = tempfile::tempdir().unwrap();
        let drive = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("steamapps")).unwrap();
        fs::create_dir(drive.path().join("steamapps")).unwrap();
        fs::write(
            root.path().join("steamapps/libraryfolders.vdf"),
            format!(
                r#""libraryfolders" {{ "1" {{ "path" "{}" }} }}"#,
                drive.path().display()
            ),
        )
        .unwrap();
        assert_eq!(libraries(root.path())[1].1, drive.path());
        fs::remove_dir(drive.path().join("steamapps")).unwrap();
        assert_eq!(libraries(root.path())[1].1, drive.path());
        assert_eq!(
            progress_in_library(drive.path(), 42, Operation::Uninstall).phase,
            Phase::Failed
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn native_control_checks_ownership_identity_running_state_and_releases_its_pipe() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("linux64")).unwrap();
        let path = dir.path().join("linux64/steamclient.so");
        assert!(
            Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&path)
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/steamcontrol.c"))
                .status()
                .unwrap()
                .success()
        );
        unsafe {
            let library = libloading::Library::new(path).unwrap();
            let set_state: libloading::Symbol<unsafe extern "C" fn(u32)> =
                library.get(b"fixture_state\0").unwrap();
            let incompatible: libloading::Symbol<unsafe extern "C" fn()> =
                library.get(b"fixture_incompatible\0").unwrap();
            let mutations: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                library.get(b"fixture_mutations\0").unwrap();
            assert!(request(dir.path(), 0, "probe").is_ok());
            assert_eq!(request(dir.path(), 42, "status").unwrap().state, 4);
            assert_eq!(mutations(), 0);
            assert_eq!(request(dir.path(), 42, "uninstall").unwrap().state, 1);
            assert_eq!(
                request(dir.path(), 43, "install").unwrap_err(),
                Error::NotOwned
            );
            assert_eq!(mutations(), 1);
            assert_eq!(request(dir.path(), 42, "install").unwrap().state, 4);
            assert_eq!(mutations(), 2);
            set_state(2 | 256 | 1024);
            let live = request(dir.path(), 42, "status").unwrap();
            assert_eq!(live.downloaded, Some(450));
            assert_eq!(live.download_total, Some(1000));
            assert_eq!(live.progress(Operation::Install).percent, Some(45));
            assert_eq!(live.progress(Operation::Install).phase, Phase::Downloading);
            request(dir.path(), 42, "pause").unwrap();
            assert_eq!(
                request(dir.path(), 42, "status")
                    .unwrap()
                    .progress(Operation::Install)
                    .phase,
                Phase::Paused
            );
            request(dir.path(), 42, "resume").unwrap();
            assert_eq!(
                request(dir.path(), 42, "status")
                    .unwrap()
                    .progress(Operation::Install)
                    .phase,
                Phase::Downloading
            );
            assert_eq!(
                request(dir.path(), 43, "resume").unwrap_err(),
                Error::NotOwned
            );
            assert_eq!(mutations(), 4);
            set_state(4 | 8192);
            assert_eq!(
                request(dir.path(), 42, "uninstall").unwrap_err(),
                Error::Running
            );
            assert_eq!(mutations(), 4);
            incompatible();
            assert_eq!(
                request(dir.path(), 42, "uninstall").unwrap_err(),
                Error::Unavailable
            );
            assert_eq!(mutations(), 4);
            assert_eq!(
                request(dir.path(), 42, "unexpected").unwrap_err(),
                Error::Rejected
            );
        }
    }
}
