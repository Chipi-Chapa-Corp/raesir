//! Identifies local game processes by their executable and installation, not by
//! their display name or by the lifetime of a launcher handoff.
use crate::model::{LibraryGame, Os};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessId {
    pid: u32,
    started: u64,
}

#[derive(Serialize)]
struct Target<'a> {
    key: &'a str,
    roots: Vec<&'a Path>,
}

impl<'a> Target<'a> {
    fn new(game: &'a LibraryGame) -> Self {
        Self {
            key: &game.key,
            roots: game
                .installations
                .iter()
                .filter(|install| {
                    install.os == Os::host()
                        && matches!(install.state.as_str(), "installed" | "external")
                })
                .map(|install| Path::new(&install.path))
                .filter(|path| path.is_absolute() && path.parent().is_some())
                .collect(),
        }
    }
}

pub fn running<'a>(
    games: impl Iterator<Item = &'a LibraryGame>,
) -> Result<HashMap<String, Vec<ProcessId>>, String> {
    let targets: Vec<_> = games
        .map(Target::new)
        .filter(|target| !target.roots.is_empty())
        .collect();
    if targets.is_empty() {
        return Ok(HashMap::new());
    }
    platform::running(&targets)
}

pub fn quit(game: &LibraryGame, processes: &[ProcessId]) -> Result<(), String> {
    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some() {
        return Err("Game actions are disabled in this diagnostic profile".into());
    }
    let target = Target::new(game);
    if target.roots.is_empty() || processes.is_empty() {
        return Err("There is no confirmed local game process to quit".into());
    }
    platform::quit(&target, processes)
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn quit_ignores_stale_process_ids_and_keeps_other_games_running() {
    use crate::model::Installation;
    use std::{
        fs,
        process::Child,
        time::{Duration, Instant},
    };

    struct Fixture(Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game");
    let other = dir.path().join("game backup");
    fs::create_dir(&path).unwrap();
    fs::create_dir(&other).unwrap();
    let mut child = Fixture(crate::test_process::spawn(&path.join("game")));
    let mut unrelated = Fixture(crate::test_process::spawn(&other.join("game")));
    let game = crate::catalog::entry(
        "steam",
        "1",
        "Fixture",
        vec![Installation {
            library: "fixture".into(),
            path: path.to_str().unwrap().into(),
            os: Os::Linux,
            state: "installed".into(),
            last_seen: 0,
        }],
        "fixture",
    );
    let processes = &running(std::iter::once(&game)).unwrap()[&game.key];
    assert_eq!(processes.len(), 1);
    let id = processes[0];
    assert_eq!(id.pid, child.0.id());
    quit(
        &game,
        &[ProcessId {
            started: id.started + 1,
            ..id
        }],
    )
    .unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    quit(&game, &[id]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child.0.try_wait().unwrap().is_some());
    assert!(unrelated.0.try_wait().unwrap().is_none());
    assert!(running(std::iter::once(&game)).unwrap().is_empty());

    // Wine uses the runtime's Unix executable but reports the game's DOS path
    // as argv[0]. Exercise that path without requiring a Wine installation.
    use std::os::unix::process::CommandExt;
    let exe = path.join("game.exe");
    let loader = dir.path().join("wine64-preloader");
    fs::hard_link(path.join("game"), &exe).unwrap();
    fs::hard_link(path.join("game"), &loader).unwrap();
    let wine = Fixture(
        std::process::Command::new(&loader)
            .arg0(format!("Z:{}", exe.to_string_lossy().replace('/', "\\")))
            .spawn()
            .unwrap(),
    );
    let running = running(std::iter::once(&game)).unwrap();
    assert_eq!(running[&game.key][0].pid, wine.0.id());
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    fn query<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, String> {
        let input = serde_json::to_vec(&input).map_err(|e| e.to_string())?;
        let mut command = if Path::new("/.flatpak-info").is_file() {
            let mut command = Command::new("flatpak-spawn");
            command.args(["--host", "python3"]);
            command
        } else {
            Command::new("python3")
        };
        let mut child = command
            .args(["-I", "-c", include_str!("game_sessions/linux.py")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start game process tracking: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&input);
        }
        let response = child.wait_with_output().map_err(|e| e.to_string())?;
        let mut value: serde_json::Value = serde_json::from_slice(&response.stdout)
            .map_err(|_| "Game process tracking could not read the host process list")?;
        if let Some(error) = value["error"].as_str() {
            return Err(error.into());
        }
        serde_json::from_value(value["data"].take())
            .map_err(|_| "Game process tracking returned an invalid response".into())
    }

    pub(super) fn running(
        targets: &[Target<'_>],
    ) -> Result<HashMap<String, Vec<ProcessId>>, String> {
        query(serde_json::json!({"targets": targets}))
    }

    pub(super) fn quit(target: &Target<'_>, processes: &[ProcessId]) -> Result<(), String> {
        query(serde_json::json!({"targets": [target], "quit": processes}))
    }
}

#[cfg(target_os = "windows")]
#[path = "game_sessions/windows.rs"]
mod platform;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::*;

    pub(super) fn running(_: &[Target<'_>]) -> Result<HashMap<String, Vec<ProcessId>>, String> {
        Err("Game process tracking is unavailable on this OS".into())
    }

    pub(super) fn quit(_: &Target<'_>, _: &[ProcessId]) -> Result<(), String> {
        Err("Game process tracking is unavailable on this OS".into())
    }
}
