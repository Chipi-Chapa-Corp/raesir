use super::*;
use crate::steam_control::Error;
use std::io::Write;

fn log_tail(dir: &Path) -> Option<String> {
    let mut file = fs::File::open(dir.join("output.log")).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(65536)))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(65536).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn recover_transfer_counters(
    dir: &Path,
    completion: &Completion,
    progress: &mut Progress,
) {
    let Some(log) = log_tail(dir) else {
        return;
    };
    let mut latest = progress.clone();
    match completion {
        Completion::Ferestre { .. } => update_ferestre_progress(&log, &mut latest),
        Completion::FerestreRuntime { cache, .. } => runtime_progress(cache, &log, &mut latest),
        Completion::Legendary { .. } => {
            update_simple_progress(&log, &mut latest);
        }
    }
    if latest.phase == Phase::Downloading {
        progress.downloaded_bytes = latest.downloaded_bytes;
        progress.detail = latest.detail;
    }
}

pub(super) fn busy(dir: &Path) -> bool {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("worker.lock"))
        .is_ok_and(|f| f.try_lock().is_err())
}

pub fn run(dir: &Path) {
    let Ok(lease) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("worker.lock"))
    else {
        return;
    };
    if lease.try_lock().is_err() {
        return;
    }
    if let Err(message) = run_inner(dir) {
        if let Some(request) = read_json(&dir.join("request.json"))
            .and_then(|v| serde_json::from_value::<Request>(v).ok())
        {
            let mut progress = Progress::starting(request.operation);
            progress.phase = Phase::Failed;
            progress.detail = Some(message);
            let _ = storage::write_json(&dir.join("progress.json"), &progress);
        }
    }
}

fn run_inner(dir: &Path) -> Result<(), String> {
    let request: Request = serde_json::from_value(
        read_json(&dir.join("request.json")).ok_or("Missing download request")?,
    )
    .map_err(|_| "Invalid download request")?;
    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
        return Err("Downloads are disabled in diagnostics".into());
    }
    let mut progress = Progress::starting(request.operation);
    let ferestre_game = request.completion.is_ferestre_game();
    progress.controllable = !request.completion.is_runtime() && !ferestre_game;
    if ferestre_game {
        progress.detail = Some("Checking the Xbox license and package…".into());
    }
    let _game_lease = if let Some(path) = &request.game_lock {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| e.to_string())?;
        if file.try_lock().is_err() {
            progress.phase = Phase::Available;
            progress.detail = Some("This game is busy · Update later".into());
            return storage::write_json(&dir.join("progress.json"), &progress);
        }
        Some(file)
    } else {
        None
    };
    if request
        .game_dir
        .as_ref()
        .is_some_and(|p| crate::mods::download_game_running(p))
    {
        progress.phase = Phase::Available;
        progress.detail = Some("Close the game to update".into());
        return storage::write_json(&dir.join("progress.json"), &progress);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(dir.join("output.log"))
        .map_err(|e| e.to_string())?;
    let mut log_position = log.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .envs(&request.env)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    if ferestre_game {
        command
            .env("XODUS_PROGRESS", "json")
            .env("FERESTRE_GUI_CANCELLATION", "1")
            .stdin(Stdio::piped());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A downloader may try to open /dev/tty even when its standard input
        // is redirected. An unattended job must never inherit that terminal.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let previous_revision = request
        .previous_version
        .clone()
        .or_else(|| request.completion.revision());
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the installed game client helper")?;
    if !ferestre_game && !request.completion.is_runtime() {
        progress.phase = Phase::Downloading;
    }
    let mut generation = serde_json::Value::Null;
    let mut suspended = vec![];
    let mut ferestre_error = None;
    loop {
        if log_position
            .metadata()
            .is_ok_and(|m| m.len() > 8 * 1024 * 1024)
        {
            let _ = log_position.set_len(0);
            let _ = log_position.seek(SeekFrom::Start(0));
        }
        if let Some(control) = read_json(&dir.join("control.json"))
            && control["generation"] != generation
        {
            generation = control["generation"].clone();
            let resume = control["resume"].as_bool().unwrap_or(true);
            if ferestre_game {
                if let Some(input) = child.stdin.as_mut() {
                    let _ = input.write_all(if resume { b"r" } else { b"p" });
                }
            } else {
                match suspend(child.id(), !resume, &mut suspended) {
                    Ok(()) => {
                        progress.phase = if resume {
                            Phase::Downloading
                        } else {
                            Phase::Paused
                        };
                        progress.detail = None;
                    }
                    Err(_) => progress.detail = Some("Could not change the download state".into()),
                }
            }
        }
        // Once the child exits, its final diagnostic is available to read.
        let status = child.try_wait().map_err(|e| e.to_string())?;
        if (progress.phase != Phase::Paused || ferestre_game)
            && let Some(text) = log_tail(dir)
        {
            if let Completion::FerestreRuntime { cache, .. } = &request.completion {
                runtime_progress(cache, &text, &mut progress);
            } else if ferestre_game {
                update_ferestre_progress(&text, &mut progress);
                ferestre_error = ferestre_failure(&text).or(ferestre_error);
            } else {
                update_simple_progress(&text, &mut progress);
            }
        }
        if let Some(status) = status {
            let extracting = progress.phase == Phase::Installing;
            progress.phase = if status.success()
                && request
                    .completion
                    .committed(&request.version, previous_revision.as_deref())
            {
                Phase::Complete
            } else {
                Phase::Failed
            };
            progress.detail = if progress.phase != Phase::Failed {
                None
            } else if ferestre_game {
                progress.controllable = true;
                if status.success() {
                    Some("Could not verify the installed Xbox game files or version.".into())
                } else {
                    match ferestre_error {
                        Some(FerestreFailure::SignInRequired) => {
                            progress.error = Some(Error::XboxSignInRequired);
                            None
                        }
                        Some(FerestreFailure::Message(message)) => Some(message.into()),
                        None => {
                            let reason = status.code().map_or_else(
                                || status.to_string(),
                                |code| format!("exit code {code}"),
                            );
                            Some(format!(
                                "Xbox download stopped ({reason}). See output.log for details."
                            ))
                        }
                    }
                }
            } else if request.completion.is_runtime() {
                Some(if extracting {
                    "Could not extract or install the Xbox runtime".into()
                } else {
                    "Could not download or verify the Xbox runtime".into()
                })
            } else {
                Some(
                    "The client could not finish this download. Sign in to the client and retry."
                        .into(),
                )
            };
            progress.percent = (progress.phase == Phase::Complete).then_some(100);
            progress.downloaded_bytes = None;
            storage::write_json(&dir.join("progress.json"), &progress)?;
            break;
        }
        if progress.phase != Phase::Downloading {
            progress.downloaded_bytes = None;
        }
        storage::write_json(&dir.join("progress.json"), &progress)?;
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(())
}

fn update_simple_progress(log: &str, progress: &mut Progress) -> bool {
    if let Some((percent, bytes)) = log.lines().rev().find_map(parse_transfer_progress) {
        progress.percent = Some(percent);
        progress.downloaded_bytes = bytes;
        true
    } else {
        false
    }
}

fn update_ferestre_progress(log: &str, progress: &mut Progress) {
    let Some(event) = log.lines().rev().find_map(|line| {
        let event: serde_json::Value =
            serde_json::from_str(line.strip_prefix("FERESTRE_PROGRESS ")?).ok()?;
        (event["schema"].as_u64() == Some(1)).then_some(event)
    }) else {
        // Older clients may emit only simple transfer counters.
        if update_simple_progress(log, progress) {
            progress.phase = Phase::Downloading;
            progress.detail = None;
        }
        return;
    };
    progress.controllable = event["canPause"] == true;
    progress.percent = None;
    progress.downloaded_bytes = None;
    let (phase, label) = match event["phase"].as_str() {
        Some("metadata") => (
            Phase::Starting,
            "Checking the Xbox license and package…".into(),
        ),
        Some("downloading") => {
            let done = event["completedBytes"].as_u64().unwrap_or_default();
            let total = event["totalBytes"]
                .as_u64()
                .filter(|&total| total > 0 && done <= total);
            progress.percent =
                total.map(|total| (u128::from(done) * 100 / u128::from(total)).min(99) as u8);
            if event["paused"] == true {
                (Phase::Paused, "Paused".into())
            } else {
                let reused = event["reusedBytes"].as_u64().unwrap_or_default();
                progress.downloaded_bytes = event["networkBytes"]
                    .as_u64()
                    .or_else(|| event["completedBytes"].as_u64()?.checked_sub(reused));
                let received = format_bytes(done);
                let label = if let Some(total) = total {
                    format!("Downloading · {received} of {}", format_bytes(total))
                } else {
                    format!("Downloading · {received} received")
                };
                (Phase::Downloading, label)
            }
        }
        Some("finalizing" | "complete") => {
            (Phase::Installing, "Finishing the Xbox installation…".into())
        }
        _ => return,
    };
    progress.phase = phase;
    progress.detail = Some(label);
}

enum FerestreFailure {
    SignInRequired,
    Message(&'static str),
}

fn ferestre_failure(log: &str) -> Option<FerestreFailure> {
    use FerestreFailure::{Message, SignInRequired};
    log.lines().rev().find_map(|line| {
        let line = line.trim();
        if line == "no user token (entry not found); sign in again" {
            return Some(SignInRequired);
        }
        if line == "Selection failed" || line.starts_with("no PC package found") {
            return Some(Message("Could not find a compatible PC download for this Xbox edition"));
        }
        if line.starts_with("No .msixvc file found") {
            if log.contains("the package list came back empty") {
                return Some(Message("The Xbox Store returned no downloadable PC package. Check again before retrying."));
            }
            return Some(Message("This Xbox edition can't be installed on Linux"));
        }
        let stage = line
            .strip_prefix("FERESTRE_TRANSFER_DIAGNOSTIC_V1 ")?
            .split_whitespace()
            .next()?;
        match stage {
            "authentication" => Some(SignInRequired),
            "license" => Some(Message("The Store account could not obtain this Xbox game's license")),
            "metadata-invalid" | "transfer-integrity" => {
                Some(Message("The Xbox game package failed verification"))
            }
            "metadata-network" | "transfer-connect" | "transfer-timeout" | "transfer-idle"
            | "transfer-range" | "transfer-eof" | "transfer-status" => {
                Some(Message("The Xbox download connection failed. Retry to continue"))
            }
            "disk-space" | "transfer-io" => {
                Some(Message("Could not write the Xbox game files. Check free space and permissions"))
            }
            "installation-busy" => Some(Message("The game is running or its files are in use")),
            _ => None,
        }
    })
}

/// Ferestre reports runtime stages on stderr. Its current CLI does not emit a
/// byte total, so use the size of its partial archive without guessing a percent.
fn runtime_progress(cache: &Path, log: &str, progress: &mut Progress) {
    let (phase, detail, downloaded_bytes) = log
        .lines()
        .rev()
        .find_map(|line| {
            if line.starts_with("-- unpacking runtime ") {
                Some((
                    Phase::Installing,
                    "Extracting the Xbox runtime…".into(),
                    None,
                ))
            } else if line.starts_with("-- verified the signature of ferestre-runtime-") {
                Some((
                    Phase::Installing,
                    "Preparing to extract the Xbox runtime…".into(),
                    None,
                ))
            } else {
                let version = line
                    .strip_prefix("-- downloading runtime ")?
                    .split_whitespace()
                    .next()?;
                let prefix = format!(".ferestre-runtime-{version}.tar.xz.");
                let bytes = fs::read_dir(cache)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        name.starts_with(&prefix) && name.ends_with(".part")
                    })
                    .filter_map(|entry| entry.metadata().ok().filter(|metadata| metadata.is_file()))
                    .map(|metadata| metadata.len())
                    .max()
                    .unwrap_or_default();
                let detail = if bytes == 0 {
                    "Waiting for the Xbox runtime download…".into()
                } else {
                    format!(
                        "Downloading Xbox runtime · {} received",
                        format_bytes(bytes)
                    )
                };
                Some((Phase::Downloading, detail, Some(bytes)))
            }
        })
        .unwrap_or_else(|| {
            (
                Phase::Starting,
                "Checking the Xbox runtime release…".into(),
                None,
            )
        });
    progress.phase = phase;
    progress.detail = Some(detail);
    progress.downloaded_bytes = downloaded_bytes;
}

#[cfg(unix)]
fn suspend(pid: u32, pause: bool, _: &mut Vec<(u32, u64)>) -> Result<(), String> {
    let signal = if pause { libc::SIGSTOP } else { libc::SIGCONT };
    if unsafe { libc::kill(-(pid as i32), signal) } == 0 {
        Ok(())
    } else {
        Err("Could not control download group".into())
    }
}

#[cfg(windows)]
fn suspend(pid: u32, pause: bool, suspended: &mut Vec<(u32, u64)>) -> Result<(), String> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSuspendProcess(handle: *mut c_void) -> i32;
        fn NtResumeProcess(handle: *mut c_void) -> i32;
    }
    fn change(pid: u32, pause: bool) -> Result<(), String> {
        unsafe {
            let handle = OpenProcess(0x0800, 0, pid);
            if handle.is_null() {
                return Err("Cannot access the download process".into());
            }
            let status = if pause {
                NtSuspendProcess(handle)
            } else {
                NtResumeProcess(handle)
            };
            CloseHandle(handle);
            if status >= 0 {
                Ok(())
            } else {
                Err("Cannot control the download process".into())
            }
        }
    }
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    if !pause {
        for (id, stamp) in suspended.drain(..).rev() {
            if system
                .process(sysinfo::Pid::from_u32(id))
                .is_some_and(|p| p.start_time() == stamp)
            {
                let _ = change(id, false);
            }
        }
        return Ok(());
    }
    let mut owned = std::collections::BTreeSet::from([sysinfo::Pid::from_u32(pid)]);
    loop {
        let before = owned.len();
        for (id, p) in system.processes() {
            if p.parent().is_some_and(|p| owned.contains(&p)) {
                owned.insert(*id);
            }
        }
        if before == owned.len() {
            break;
        }
    }
    for id in owned {
        if let Some(p) = system.process(id) {
            if let Err(error) = change(id.as_u32(), true) {
                for (id, _) in suspended.drain(..).rev() {
                    let _ = change(id, false);
                }
                return Err(error);
            }
            suspended.push((id.as_u32(), p.start_time()));
        }
    }
    Ok(())
}
