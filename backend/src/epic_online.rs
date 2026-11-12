//! User-triggered Epic account refresh delegated to an installed Legendary CLI.
use crate::{epic, model::*};
use std::{
    env, fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn binary() -> Option<PathBuf> {
    if let Some(path) = env::var_os("EXPOSED_GAMES_LEGENDARY_BINARY") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    if let Some(path) = crate::discovery::program("legendary") {
        return Some(path.into());
    }
    let name = if cfg!(windows) {
        "legendary.exe"
    } else {
        "legendary"
    };
    let home = crate::platform::home()?;
    for root in [
        home.join(".config/heroic/tools/legendary"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/tools/legendary"),
        home.join("AppData/Roaming/heroic/tools/legendary"),
    ] {
        let binary = root.join(name);
        if binary.is_file() {
            return Some(binary);
        }
    }
    None
}

#[derive(Default)]
pub struct Manager {
    process: Mutex<Option<Child>>,
    operation: Mutex<()>,
    closing: AtomicBool,
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        // Each lookup gets a private group. Include subprocesses created by
        // packaged Legendary bootstraps; the unreaped Child reserves this PID.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Manager {
    pub fn resume(&self) {
        self.closing.store(false, Ordering::SeqCst);
    }
    pub fn shutdown(&self) {
        let mut process = self.process.lock().unwrap_or_else(|e| e.into_inner());
        self.closing.store(true, Ordering::SeqCst);
        if let Some(mut child) = process.take() {
            terminate(&mut child);
        }
    }

    pub fn refresh(&self, profile: &str) -> Result<EpicOnlineLibrary, String> {
        let profile = epic::local_sources()
        .into_iter()
        .find(|s| s.kind == "legendary" && s.path == profile)
        .ok_or(
            "Choose a detected local Legendary/Heroic profile. Other-OS profiles are read-only.",
        )?;
        let program = binary().ok_or("Legendary is not available. Install its CLI or set EXPOSED_GAMES_LEGENDARY_BINARY; cached libraries still work. Sign in with Heroic or Legendary first.")?;
        self.run(program, &profile.path, Duration::from_secs(120))
    }

    fn run(
        &self,
        program: PathBuf,
        profile: &str,
        timeout: Duration,
    ) -> Result<EpicOnlineLibrary, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Epic refresh is unavailable")?;
        // Temporary files avoid pipe deadlocks; output is bounded before parsing.
        // stderr may contain account data, so it is never echoed into the UI/logs.
        let output = tempfile::tempfile().map_err(|e| e.to_string())?;
        let errors = tempfile::tempfile().map_err(|e| e.to_string())?;
        let reader = output.try_clone().map_err(|e| e.to_string())?;
        let error_reader = errors.try_clone().map_err(|e| e.to_string())?;
        let mut command = Command::new(program);
        command
            .args(["list", "--json", "--third-party", "--force-refresh"])
            .env("LEGENDARY_CONFIG_PATH", profile)
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors);
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
        {
            let mut process = self.process.lock().unwrap_or_else(|e| e.into_inner());
            if self.closing.load(Ordering::SeqCst) {
                return Err("Epic lookup cancelled because Exposed Games is closing".into());
            }
            *process = Some(command.spawn().map_err(|_| "Could not run Legendary")?);
        }
        let until = Instant::now() + timeout;
        let status = loop {
            let mut process = self.process.lock().unwrap_or_else(|e| e.into_inner());
            let child = process
                .as_mut()
                .ok_or("Epic lookup cancelled because Exposed Games is closing")?;
            let too_large = [reader.metadata(), error_reader.metadata()]
                .into_iter()
                .any(|m| m.map(|m| m.len() > 32 * 1024 * 1024).unwrap_or(true));
            match child.try_wait() {
                Ok(Some(status)) => {
                    process.take();
                    break status;
                }
                Ok(None) if Instant::now() < until && !too_large => {
                    drop(process);
                    thread::sleep(Duration::from_millis(100));
                }
                _ => {
                    terminate(child);
                    process.take();
                    return Err(
                    "Epic refresh timed out or could not finish. The previous library was kept."
                        .into(),
                );
                }
            }
        };
        if !status.success() {
            return Err("Legendary could not read the Epic account. Sign in using Heroic or `legendary auth`, then retry. The previous Exposed Games library was kept.".into());
        }
        if reader.metadata().map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
            return Err(
                "Legendary returned an unexpectedly large library. Previous results were kept."
                    .into(),
            );
        }
        use std::io::{Read, Seek};
        let mut reader = reader;
        reader.rewind().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        reader
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| "Legendary returned invalid JSON; previous results were kept")?;
        let games = epic::parse_library(&value)?;
        Ok(EpicOnlineLibrary {
            profile: profile.into(),
            checked_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            games,
        })
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn save(dir: &std::path::Path, library: EpicOnlineLibrary) -> Result<(), String> {
    let path = dir.join("epic-owned.json");
    let mut libraries: Vec<EpicOnlineLibrary> = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Cannot read the previous Epic cache; it was kept")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(_) => return Err("Cannot read the previous Epic cache; it was kept".into()),
    };
    libraries.retain(|l| l.profile != library.profile);
    libraries.push(library);
    crate::storage::write_json(&path, &libraries)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn script(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("legendary-fixture");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[test]
    fn refresh_uses_selected_profile_and_expected_json_command_and_redacts_failures() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            r#"
test "$*" = 'list --json --third-party --force-refresh' || exit 2
test -d "$LEGENDARY_CONFIG_PATH" || exit 3
printf '%s' fixture > "$LEGENDARY_CONFIG_PATH/observed"
printf '%s' '[{"app_name":"opaque-id","app_title":"Example"}]'
"#,
        );
        let manager = Manager::default();
        let result = manager
            .run(
                program.clone(),
                dir.path().to_str().unwrap(),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(result.games[0].app_name, "opaque-id");
        assert!(dir.path().join("observed").is_file());
        script(dir.path(), "echo secret-fixture >&2\nexit 1");
        let error = manager
            .run(
                program,
                dir.path().to_str().unwrap(),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(!error.contains("secret-fixture"));
        assert!(error.contains("previous"));
    }
    #[test]
    fn timeout_and_app_shutdown_reap_only_the_lookup_child() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "exec /bin/sleep 20");
        let manager = Manager::default();
        assert!(
            manager
                .run(
                    program.clone(),
                    dir.path().to_str().unwrap(),
                    Duration::from_millis(100)
                )
                .is_err()
        );
        assert!(manager.process.lock().unwrap().is_none());
        let manager = std::sync::Arc::new(manager);
        let worker = manager.clone();
        let profile = dir.path().to_str().unwrap().to_owned();
        let task = thread::spawn(move || worker.run(program, &profile, Duration::from_secs(20)));
        let until = Instant::now() + Duration::from_secs(5);
        while manager.process.lock().unwrap().is_none() && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(manager.process.lock().unwrap().is_some());
        manager.shutdown();
        assert!(task.join().unwrap().is_err());
        assert!(manager.process.lock().unwrap().is_none());
    }
    #[test]
    fn snapshots_replace_only_the_selected_profile_and_keep_corrupt_cache_intact() {
        let dir = tempfile::tempdir().unwrap();
        for (profile, title) in [("one", "Old"), ("two", "Other"), ("one", "New")] {
            save(
                dir.path(),
                EpicOnlineLibrary {
                    profile: profile.into(),
                    checked_at: 1,
                    games: vec![EpicTitle {
                        app_name: "id".into(),
                        title: title.into(),
                    }],
                },
            )
            .unwrap();
        }
        let path = dir.path().join("epic-owned.json");
        let result: Vec<EpicOnlineLibrary> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(
            result.iter().find(|l| l.profile == "one").unwrap().games[0].title,
            "New"
        );
        assert_eq!(
            result.iter().find(|l| l.profile == "two").unwrap().games[0].title,
            "Other"
        );
        fs::write(&path, "broken").unwrap();
        assert!(save(dir.path(), result[0].clone()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
}
