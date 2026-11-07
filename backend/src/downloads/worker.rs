use super::*;

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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
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
    progress.phase = Phase::Downloading;
    let mut generation = serde_json::Value::Null;
    let mut suspended = vec![];
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
        if progress.phase != Phase::Paused {
            if let Ok(mut file) = fs::File::open(dir.join("output.log")) {
                let length = file.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = file.seek(SeekFrom::Start(length.saturating_sub(65536)));
                let mut bytes = Vec::new();
                if file.take(65536).read_to_end(&mut bytes).is_ok() {
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some(percent) = text.lines().rev().find_map(parse_progress) {
                        progress.percent = Some(percent);
                    }
                }
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
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
                "The client could not finish this download. Sign in to the client and retry.".into()
            });
            progress.percent = (progress.phase == Phase::Complete).then_some(100);
            storage::write_json(&dir.join("progress.json"), &progress)?;
            break;
        }
        storage::write_json(&dir.join("progress.json"), &progress)?;
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(())
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
