use super::*;
use std::io::Write;

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
    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some() {
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
        if progress.phase != Phase::Paused || ferestre_game {
            if let Ok(mut file) = fs::File::open(dir.join("output.log")) {
                let length = file.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = file.seek(SeekFrom::Start(length.saturating_sub(65536)));
                let mut bytes = Vec::new();
                if file.take(65536).read_to_end(&mut bytes).is_ok() {
                    let text = String::from_utf8_lossy(&bytes);
                    if let Completion::FerestreRuntime { cache, .. } = &request.completion {
                        runtime_progress(cache, &text, &mut progress);
                    } else if ferestre_game {
                        update_ferestre_progress(&text, &mut progress);
                        ferestre_error = ferestre_failure(&text).or(ferestre_error);
                    } else if let Some(percent) = text.lines().rev().find_map(parse_progress) {
                        progress.percent = Some(percent);
                    }
                }
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
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
            progress.detail = (progress.phase == Phase::Failed).then(|| {
                if request.completion.is_runtime() {
                    if extracting {
                        "Could not extract or install the Xbox runtime".into()
                    } else {
                        "Could not download or verify the Xbox runtime".into()
                    }
                } else if ferestre_game {
                    ferestre_error
                        .unwrap_or("Ferestre did not finish installing this Xbox game")
                        .into()
                } else {
                    "The client could not finish this download. Sign in to the client and retry."
                        .into()
                }
            });
            if ferestre_game && progress.phase == Phase::Failed {
                progress.controllable = true;
            }
            progress.percent = (progress.phase == Phase::Complete).then_some(100);
            storage::write_json(&dir.join("progress.json"), &progress)?;
            break;
        }
        storage::write_json(&dir.join("progress.json"), &progress)?;
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(())
}

fn update_ferestre_progress(log: &str, progress: &mut Progress) {
    let Some(event) = log.lines().rev().find_map(|line| {
        let event: serde_json::Value =
            serde_json::from_str(line.strip_prefix("FERESTRE_PROGRESS ")?).ok()?;
        (event["schema"].as_u64() == Some(1)).then_some(event)
    }) else {
        // Older clients may emit only simple transfer counters.
        if let Some(percent) = log.lines().rev().find_map(parse_progress) {
            progress.phase = Phase::Downloading;
            progress.percent = Some(percent);
            progress.detail = None;
        }
        return;
    };
    progress.controllable = event["canPause"] == true;
    progress.percent = None;
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
            progress.percent = total.map(|total| (done.saturating_mul(100) / total).min(99) as u8);
            let mib = 1024. * 1024.;
            let label = if event["paused"] == true {
                "Paused".into()
            } else if let Some(total) = total {
                format!(
                    "Downloading · {:.1} of {:.1} MiB",
                    done as f64 / mib,
                    total as f64 / mib
                )
            } else {
                format!("Downloading · {:.1} MiB received", done as f64 / mib)
            };
            (
                if event["paused"] == true {
                    Phase::Paused
                } else {
                    Phase::Downloading
                },
                label,
            )
        }
        Some("finalizing" | "complete") => {
            (Phase::Installing, "Finishing the Xbox installation…".into())
        }
        _ => return,
    };
    progress.phase = phase;
    progress.detail = Some(label);
}

fn ferestre_failure(log: &str) -> Option<&'static str> {
    log.lines().rev().find_map(|line| {
        if line == "Selection failed" || line.starts_with("no PC package found") {
            return Some("Ferestre could not select a compatible PC package for this Xbox edition");
        }
        if line.starts_with("No .msixvc file found") {
            if log.contains("the package list came back empty") {
                return Some("The Xbox Store returned no downloadable PC package. Check again before retrying.");
            }
            return Some("This Xbox game uses a package format that Ferestre cannot install");
        }
        let stage = line
            .strip_prefix("FERESTRE_TRANSFER_DIAGNOSTIC_V1 ")?
            .split_whitespace()
            .next()?;
        match stage {
            "authentication" => Some("Xbox sign-in failed. Sign in again and retry"),
            "license" => Some("The Store account could not obtain this Xbox game's license"),
            "metadata-invalid" | "transfer-integrity" => {
                Some("The Xbox game package failed verification")
            }
            "metadata-network" | "transfer-connect" | "transfer-timeout" | "transfer-idle"
            | "transfer-range" | "transfer-eof" | "transfer-status" => {
                Some("The Xbox download connection failed. Retry to continue")
            }
            "disk-space" | "transfer-io" => {
                Some("Ferestre could not write the game data. Check free space and permissions")
            }
            "installation-busy" => Some("The game is running or its files are in use"),
            _ => None,
        }
    })
}

/// Ferestre reports runtime stages on stderr. Its current CLI does not emit a
/// byte total, so use the size of its partial archive without guessing a percent.
fn runtime_progress(cache: &Path, log: &str, progress: &mut Progress) {
    let (phase, detail) = log
        .lines()
        .rev()
        .find_map(|line| {
            if line.starts_with("-- unpacking runtime ") {
                Some((Phase::Installing, "Extracting the Xbox runtime…".into()))
            } else if line.starts_with("-- verified the signature of ferestre-runtime-") {
                Some((
                    Phase::Installing,
                    "Preparing to extract the Xbox runtime…".into(),
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
                } else if bytes < 1024 * 1024 {
                    format!("Downloading Xbox runtime · {} KiB received", bytes / 1024)
                } else {
                    format!(
                        "Downloading Xbox runtime · {:.1} MiB received",
                        bytes as f64 / (1024. * 1024.)
                    )
                };
                Some((Phase::Downloading, detail))
            }
        })
        .unwrap_or_else(|| (Phase::Starting, "Checking the Xbox runtime release…".into()));
    progress.phase = phase;
    progress.detail = Some(detail);
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
