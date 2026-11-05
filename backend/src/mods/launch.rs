//! Loader staging is reversible. Profiles remain separate from the game's files,
//! and a durable receipt allows recovery after an interrupted launcher session.
use super::*;
use crate::{discovery, model::*, runtime as wine_runtime};
use std::{
    io::Read,
    process::{Command, Stdio},
};
use thunderstore_engine::profile::launch::{
    self, HostOs, LaunchContext, LaunchMode, Runtime, StorePlatform,
};
pub(super) mod steam_launch;

pub struct LaunchTask(pub(super) std::sync::mpsc::Receiver<Result<(), String>>);

pub(super) struct GameLease(fs::File);

impl Drop for GameLease {
    fn drop(&mut self) {
        // A concurrent fork may briefly inherit this open-file description
        // before CLOEXEC runs. Explicit unlock releases our lease immediately.
        let _ = self.0.unlock();
    }
}

impl LaunchTask {
    pub fn poll(&self) -> Option<Result<(), String>> {
        match self.0.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Modded launch monitor stopped".into()))
            }
        }
    }
    /// Wait for the game and loader-file restoration, useful to headless callers.
    pub fn wait(self) -> Result<(), String> {
        self.0
            .recv()
            .map_err(|_| "Modded launch monitor stopped".to_owned())?
    }
}

#[derive(Serialize, Deserialize)]
struct StagedFile {
    relative: PathBuf,
    backup: Option<String>,
    staged: String,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    generation: u128,
    game_dir: PathBuf,
    profile: PathBuf,
    files: Vec<StagedFile>,
}

impl Manager {
    pub(super) fn game_lease(&self, dir: &Path) -> Result<GameLease, String> {
        fs::create_dir_all(&self.leases).map_err(|e| e.to_string())?;
        let key = hash(dir.to_string_lossy().as_bytes());
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.leases.join(format!("{key}.lock")))
            .map_err(|e| e.to_string())?;
        file.try_lock().map_err(|e| {
            format!(
                "Another Raesir session is using this game, or its mod lock is unavailable: {e}"
            )
        })?;
        Ok(GameLease(file))
    }

    /// Recover only our own receipts, leaving a running game and any externally
    /// changed files alone. Called by normal library hydration after a restart.
    pub fn recover_staging(&self) -> Vec<String> {
        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
            return Vec::new();
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let mut errors = Vec::new();
        for entry in fs::read_dir(self.data.join("staged"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let staged = entry.path();
            let dir = ["receipt.json", "steam.json", "noita.json"]
                .into_iter()
                .find_map(|file| {
                    let bytes = fs::read(staged.join(file)).ok()?;
                    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
                    value["game_dir"]
                        .as_str()
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from)
                });
            let Some(dir) = dir else {
                continue;
            };
            if game_running(&dir)
                || self
                    .active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&dir)
            {
                continue;
            }
            let Ok(_lease) = self.game_lease(&dir) else {
                continue;
            };
            if let Err(error) = steam_launch::restore_session(&staged) {
                errors.push(format!("Mod launch recovery: {error}"));
            }
        }
        errors
    }

    pub fn vanilla_args(&self, game: &LibraryGame) -> Result<Vec<String>, String> {
        if recipes::for_game(game).is_some() {
            return Ok(Vec::new());
        }
        let Some(slug) = community(game) else {
            return Ok(Vec::new());
        };
        let profiles = self.profiles(slug);
        if profiles.iter().all(|p| p.installed.is_empty()) {
            return Ok(Vec::new());
        }
        let installation = game
            .installations
            .iter()
            .find(|i| i.os == Os::host() && matches!(i.state.as_str(), "installed" | "external"))
            .ok_or("This game is not installed on this OS")?;
        let dir = Path::new(&installation.path);
        let schema = ECOSYSTEM
            .game(slug)
            .and_then(|g| g.profile())
            .ok_or("No mod loader profile")?;
        let exe = launch::resolve_game_exe(dir, &schema.exe_names).map_err(|e| e.to_string())?;
        let mut magic = [0; 2];
        fs::File::open(exe)
            .and_then(|mut f| f.read_exact(&mut magic))
            .map_err(|e| e.to_string())?;
        let context = LaunchContext::new(
            HostOs::detect().ok_or("Unsupported OS")?,
            StorePlatform::Steam,
            if cfg!(target_os = "linux") && magic == *b"MZ" {
                Runtime::Proton
            } else {
                Runtime::Native
            },
            LaunchMode::Vanilla,
        );
        let target = if dir.join(".doorstop_version").is_file() {
            dir
        } else {
            self.selected(slug, &profiles)
                .map_or(dir, |p| p.path.as_path())
        };
        launch::loader_args(target, schema, &context).map_err(|e| e.to_string())
    }

    /// Non-Steam games use the engine's executable/script plan and their existing
    /// Wine runner. Steam always owns its own runtime through `launch_steam`.
    fn direct_command(
        &self,
        report: &Report,
        game: &LibraryGame,
        extra_args: &[String],
    ) -> Result<(Command, PathBuf, PathBuf, LaunchContext), String> {
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let profiles = self.profiles(slug);
        let profile = self
            .selected(slug, &profiles)
            .ok_or("Install a mod first")?;
        if !profile.installed.iter().any(|m| m.enabled) {
            return Err("This profile has no enabled mods".into());
        }
        let install = game
            .installations
            .iter()
            .find(|i| i.os == Os::host() && matches!(i.state.as_str(), "installed" | "external"))
            .ok_or("This game is not installed on this OS")?;
        let game_dir = Path::new(&install.path)
            .canonicalize()
            .map_err(|e| format!("Cannot find game files: {e}"))?;
        let schema = ECOSYSTEM
            .game(slug)
            .and_then(|g| g.profile())
            .ok_or("No mod loader profile is available for this game")?;
        let exe =
            launch::resolve_game_exe(&game_dir, &schema.exe_names).map_err(|e| e.to_string())?;
        let windows_binary = {
            let mut magic = [0; 2];
            fs::File::open(&exe)
                .and_then(|mut f| f.read_exact(&mut magic))
                .map_err(|e| e.to_string())?;
            magic == *b"MZ"
        };
        let proton = cfg!(target_os = "linux") && windows_binary;
        let context = LaunchContext::new(
            HostOs::detect().ok_or("Modded launch is not supported on this OS")?,
            StorePlatform::Other,
            if proton {
                Runtime::Proton
            } else {
                Runtime::Native
            },
            LaunchMode::Modded,
        )
        .with_extra_args(extra_args.to_vec());
        let plan = launch::launch_plan_in(&profile.path, &ECOSYSTEM, slug, &context)
            .map_err(|e| e.to_string())?;
        let mut command = if proton {
            let prefix = discovery::prefixes(report)
                .into_iter()
                .find(|p| wine_runtime::matches(p, game, report))
                .ok_or("Could not find this game's Wine profile")?;
            let action = wine_runtime::wine(&prefix, &exe, plan.args.clone())
                .ok_or("Could not find this game's Wine or Proton runner")?;
            let ActionTarget::Command {
                program, args, env, ..
            } = action.target
            else {
                return Err("Invalid Wine launch command".into());
            };
            let mut command = Command::new(program);
            command.args(args).envs(env);
            command
        } else {
            plan.to_command(&game_dir).map_err(|e| e.to_string())?
        };
        command.envs(plan.env).current_dir(&game_dir);
        Ok((command, profile.path.clone(), game_dir, context))
    }

    pub fn launch_modded(
        &self,
        report: &Report,
        game: &LibraryGame,
        extra_args: &[String],
    ) -> Result<LaunchTask, String> {
        if recipes::for_game(game).is_some() {
            return self.launch_noita(report, game, extra_args);
        }
        self.launch_game(report, game, extra_args, LaunchMode::Modded)
    }

    pub fn launch_vanilla(
        &self,
        report: &Report,
        game: &LibraryGame,
        extra_args: &[String],
    ) -> Result<LaunchTask, String> {
        self.launch_game(report, game, extra_args, LaunchMode::Vanilla)
    }

    fn launch_game(
        &self,
        report: &Report,
        game: &LibraryGame,
        extra_args: &[String],
        mode: LaunchMode,
    ) -> Result<LaunchTask, String> {
        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
            && !super::diagnostic_launch_enabled()
        {
            return Err("Game launches are disabled in this diagnostic profile".into());
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let installation = game
            .installations
            .iter()
            .find(|i| i.os == Os::host() && matches!(i.state.as_str(), "installed" | "external"))
            .ok_or("This game is not installed on this OS")?;
        let game_dir = Path::new(&installation.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let lease = self.game_lease(&game_dir)?;
        for install in game.installations.iter().filter(|i| i.os == Os::host()) {
            if game_running(Path::new(&install.path))
                || Path::new(&install.path)
                    .canonicalize()
                    .ok()
                    .is_some_and(|p| {
                        self.active
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains(&p)
                    })
            {
                return Err("Close the game before changing its modded launch".into());
            }
        }
        if mode == LaunchMode::Modded {
            self.prepare_selected_for_launch(game)?;
        }
        if game.store == "steam" {
            return self.launch_steam(report, game, extra_args, mode, lease);
        }
        if mode == LaunchMode::Vanilla {
            return Err("Use this game's normal Play action for a vanilla launch".into());
        }
        let (mut command, profile, game_dir, context) =
            self.direct_command(report, game, extra_args)?;
        if game_running(&game_dir) {
            return Err("Close the game before changing its modded launch".into());
        }
        let receipt_dir = self.stage_dir(&game_dir);
        steam_launch::restore_session(&receipt_dir)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let log_dir = self.data.join("logs");
        fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
        let log_path = log_dir.join(format!(
            "{}.log",
            community(game).ok_or("Unknown mod community")?
        ));
        let log = Arc::new(Mutex::new((
            fs::File::create(&log_path).map_err(|e| e.to_string())?,
            0_usize,
        )));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        self.stage(game, &profile, &game_dir, &context, &receipt_dir)?;
        let receipt_identity = fs::read(receipt_dir.join("receipt.json")).ok();
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                restore(&receipt_dir)?;
                return Err(format!("Could not launch modded game: {e}"));
            }
        };
        if let Some(stdout) = child.stdout.take() {
            capture_log(stdout, log.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            capture_log(stderr, log);
        }
        let operation = self.operation.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _lease = lease;
            let exited = child
                .wait()
                .map_err(|e| format!("Could not wait for modded game: {e}"));
            // Some launch scripts hand off to another process. Leave the loader
            // in place until that process exits, too.
            while game_running(&game_dir) {
                std::thread::sleep(Duration::from_secs(2));
            }
            let _guard = operation.lock().unwrap_or_else(|e| e.into_inner());
            if fs::read(receipt_dir.join("receipt.json")).ok() != receipt_identity {
                let _ = tx.send(Ok(()));
                return;
            }
            let restored = steam_launch::restore_session(&receipt_dir);
            let result = restored.and_then(|_| match exited {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!(
                    "Modded game exited with {status}. Launch log: {}",
                    log_path.display()
                )),
                Err(error) => Err(error),
            });
            if let Err(error) = &result {
                eprintln!("Modded launch: {error}");
            }
            let _ = tx.send(result);
        });
        Ok(LaunchTask(rx))
    }

    /// Called before normal Play. A previous interrupted modded session must not
    /// silently turn the ordinary Play action into a modded launch.
    pub fn restore_for_game(&self, game: &LibraryGame) -> Result<(), String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let noita = recipes::for_game(game).is_some();
        for installation in game.installations.iter().filter(|i| i.os == Os::host()) {
            if let Ok(dir) = Path::new(&installation.path).canonicalize() {
                let staged = self.stage_dir(&dir);
                if noita
                    || staged.join("receipt.json").exists()
                    || staged.join("steam.json").exists()
                    || staged.join("noita.json").exists()
                {
                    let _lease = self.game_lease(&dir)?;
                    if game_running(&dir)
                        || self
                            .active
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains(&dir)
                    {
                        return Err("Close the modded game before starting another launch".into());
                    }
                    if let Err(error) = steam_launch::restore_session(&staged) {
                        if noita || staged.join("receipt.json").exists() {
                            return Err(error);
                        }
                        // A stopped Steam client can defer option cleanup. The
                        // pass-through wrapper is safe for normal Play, and the
                        // original game files have already been restored.
                        eprintln!("Steam launch-option cleanup deferred: {error}");
                    }
                    if noita {
                        super::noita::disable_for_vanilla(installation)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn stage_dir(&self, game_dir: &Path) -> PathBuf {
        self.data
            .join("staged")
            .join(hash(game_dir.to_string_lossy().as_bytes()))
    }

    fn stage(
        &self,
        game: &LibraryGame,
        profile: &Path,
        game_dir: &Path,
        context: &LaunchContext,
        receipt_dir: &Path,
    ) -> Result<(), String> {
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        // Ask the engine to stage into a temporary mirror first. Its loader-
        // specific routing is reused while game files are replaced atomically.
        let mirror = tempfile::tempdir().map_err(|e| e.to_string())?;
        let files = launch::stage_for_launch_in(profile, &ECOSYSTEM, slug, mirror.path(), context)
            .map_err(|e| e.to_string())?;
        launch::ensure_loader_scripts_executable(profile).map_err(|e| e.to_string())?;
        if files.is_empty() {
            return Ok(());
        }
        self.stage_mirror(profile, game_dir, mirror.path(), &files, receipt_dir)
    }

    pub(super) fn stage_mirror(
        &self,
        profile: &Path,
        game_dir: &Path,
        mirror: &Path,
        files: &[PathBuf],
        receipt_dir: &Path,
    ) -> Result<(), String> {
        fs::create_dir_all(receipt_dir).map_err(|e| e.to_string())?;
        let mut receipt = Receipt {
            generation: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            game_dir: game_dir.into(),
            profile: profile.into(),
            files: Vec::new(),
        };
        for source in files {
            let relative = source
                .strip_prefix(mirror)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let destination = safe_destination(game_dir, &relative)?;
            if relative == Path::new("doorstop_config.ini") {
                // External Steam launches remain vanilla if Raesir is interrupted.
                // The engine's explicit per-launch argument enables Doorstop.
                let text = fs::read_to_string(source).map_err(|e| e.to_string())?;
                fs::write(source, disable_doorstop(&text)).map_err(|e| e.to_string())?;
            }
            let backup = if destination.exists() {
                if !destination.is_file() {
                    return Err(format!(
                        "Loader destination is not a file: {}",
                        destination.display()
                    ));
                }
                let name = format!("backup-{}", receipt.files.len());
                fs::copy(&destination, receipt_dir.join(&name)).map_err(|e| e.to_string())?;
                Some(name)
            } else {
                None
            };
            receipt.files.push(StagedFile {
                relative,
                backup,
                staged: fingerprint(source)?,
            });
        }
        crate::storage::write_json(&receipt_dir.join("receipt.json"), &receipt)?;
        let result = files
            .iter()
            .zip(&receipt.files)
            .try_for_each(|(source, entry)| {
                let destination = safe_destination(game_dir, &entry.relative)?;
                atomic_copy(source, &destination)
            });
        if let Err(error) = result {
            restore(receipt_dir)?;
            return Err(error);
        }
        Ok(())
    }
}

pub(super) fn capture_log(
    mut stream: impl Read + Send + 'static,
    log: Arc<Mutex<(fs::File, usize)>>,
) {
    use std::io::Write;
    std::thread::spawn(move || {
        let mut bytes = [0_u8; 8192];
        while let Ok(count) = stream.read(&mut bytes) {
            if count == 0 {
                break;
            }
            let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
            let count = count.min((8 * 1024 * 1024_usize).saturating_sub(log.1));
            let _ = log.0.write_all(&bytes[..count]);
            log.1 += count;
        }
    });
}

pub(super) fn game_running(dir: &Path) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet)
            .without_tasks(),
    );
    let path = dir.to_string_lossy().replace('\\', "/").to_lowercase();
    system.processes().values().any(|p| {
        p.exe().is_some_and(|exe| exe.starts_with(dir))
            || p.cmd().iter().any(|arg| {
                arg.to_string_lossy()
                    .replace('\\', "/")
                    .to_lowercase()
                    .contains(&format!("{path}/"))
            })
            || (p.cwd().is_some_and(|cwd| cwd == dir)
                && p.name().to_string_lossy().to_lowercase().ends_with(".exe"))
    })
}

fn safe_destination(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Invalid loader staging path".into());
    }
    if fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("The game's staging directory was replaced by a symbolic link".into());
    }
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "Cannot replace a loader file through a symbolic link: {}",
                path.display()
            ));
        }
    }
    Ok(path)
}

pub(super) fn atomic_copy(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("Loader file has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    fs::copy(source, temp.path()).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(target).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn hash(bytes: &[u8]) -> String {
    let mut value = 0xcbf29ce484222325_u64;
    for b in bytes {
        value = (value ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{value:016x}")
}

fn fingerprint(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| hash(&bytes))
        .map_err(|e| e.to_string())
}

fn restore(root: &Path) -> Result<(), String> {
    let path = root.join("receipt.json");
    if !path.exists() {
        return Ok(());
    }
    let receipt: Receipt = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for entry in &receipt.files {
        let target = safe_destination(&receipt.game_dir, &entry.relative)?;
        if !target.exists() {
            continue;
        }
        // Preserve updates made by the game, Steam, or another mod manager.
        if fingerprint(&target)? != entry.staged {
            continue;
        }
        if let Some(name) = &entry.backup {
            if name.contains(['/', '\\']) || !name.starts_with("backup-") {
                return Err("Invalid loader backup path".into());
            }
            atomic_copy(&root.join(name), &target)?;
        } else {
            fs::remove_file(&target).map_err(|e| e.to_string())?;
        }
    }
    if root.join("steam.json").exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
        for entry in receipt.files {
            if let Some(name) = entry.backup {
                let _ = fs::remove_file(root.join(name));
            }
        }
        Ok(())
    } else {
        fs::remove_dir_all(root).map_err(|e| e.to_string())
    }
}

fn disable_doorstop(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("enabled"))
            {
                "enabled = false".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_fixture(root: &Path, leases: &Path) -> Manager {
        Manager {
            data: root.join("data"),
            cache: root.join("cache"),
            catalogs: Mutex::new(HashMap::new()),
            operation: Arc::new(Mutex::new(())),
            active: Default::default(),
            leases: leases.into(),
        }
    }

    #[test]
    fn game_lease_protects_the_same_installation_across_separate_raesir_profiles() {
        let root = tempfile::tempdir().unwrap();
        let first = manager_fixture(&root.path().join("first"), &root.path().join("locks"));
        let second = manager_fixture(&root.path().join("second"), &root.path().join("locks"));
        let game = root.path().join("game");
        let lease = first.game_lease(&game).unwrap();
        assert!(second.game_lease(&game).is_err());
        let inherited = lease.0.try_clone().unwrap();
        drop(lease);
        assert!(second.game_lease(&game).is_ok());
        drop(inherited);
    }

    #[test]
    fn normal_play_uses_the_installed_loaders_vanilla_switch() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager_fixture(root.path(), &root.path().join("locks"));
        let profile = manager.data.join("peak/revisions/profile-test");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join(".doorstop_version"), "3.0.0").unwrap();
        let version: thunderstore_engine::models::Version = serde_json::from_value(serde_json::json!({"name":"Test", "version_number":"1.0.0", "dependencies":[], "date_created":"1970-01-01T00:00:00Z"})).unwrap();
        let entry = thunderstore_engine::profile::mod_entry_from_version(
            "Owner-Test",
            "Owner",
            &version,
            0,
        );
        modlist::write(&profile, &[entry]).unwrap();
        crate::storage::write_json(
            &manager.data.join("peak/current.json"),
            &serde_json::json!({"revision":"profile-test"}),
        )
        .unwrap();
        let game_dir = root.path().join("game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("PEAK.exe"), b"MZfixture").unwrap();
        fs::write(game_dir.join(".doorstop_version"), "4.5.0").unwrap();
        let game = crate::catalog::entry(
            "steam",
            "3527290",
            "Fixture",
            vec![Installation {
                library: root.path().display().to_string(),
                path: game_dir.display().to_string(),
                os: Os::host(),
                state: "installed".into(),
                last_seen: 0,
            }],
            "test",
        );
        assert_eq!(
            manager.vanilla_args(&game).unwrap(),
            ["--doorstop-enabled", "false"]
        );
        assert_eq!(
            fs::read_to_string(profile.join(".doorstop_version")).unwrap(),
            "3.0.0"
        );
    }

    #[test]
    fn engine_staging_keeps_profiles_separate_and_can_restore_a_real_loader_layout() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager {
            data: dir.path().join("data"),
            cache: dir.path().join("cache"),
            catalogs: Mutex::new(HashMap::new()),
            operation: Arc::new(Mutex::new(())),
            active: Default::default(),
            leases: dir.path().join("leases"),
        };
        let game = crate::catalog::entry("steam", "3527290", "Fixture", vec![], "test");
        let profile = dir.path().join("profile");
        let game_dir = dir.path().join("game");
        fs::create_dir_all(profile.join("BepInEx/plugins")).unwrap();
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(profile.join("winhttp.dll"), b"new proxy").unwrap();
        fs::write(
            profile.join("doorstop_config.ini"),
            "[General]\nenabled = true\n",
        )
        .unwrap();
        fs::write(profile.join("BepInEx/plugins/test.dll"), b"plugin").unwrap();
        fs::write(game_dir.join("winhttp.dll"), b"user's original proxy").unwrap();
        let context = LaunchContext::new(
            HostOs::Linux,
            StorePlatform::SteamDirect,
            Runtime::Proton,
            LaunchMode::Modded,
        );
        let receipt = manager.stage_dir(&game_dir);
        manager
            .stage(&game, &profile, &game_dir, &context, &receipt)
            .unwrap();
        assert_eq!(
            fs::read(game_dir.join("winhttp.dll")).unwrap(),
            b"new proxy"
        );
        assert!(
            fs::read_to_string(game_dir.join("doorstop_config.ini"))
                .unwrap()
                .contains("enabled = false")
        );
        assert!(!game_dir.join("BepInEx").exists());
        assert_eq!(
            fs::read(profile.join("BepInEx/plugins/test.dll")).unwrap(),
            b"plugin"
        );
        #[cfg(unix)]
        {
            let sleeper = game_dir.join("fixture-game");
            std::os::unix::fs::symlink("/usr/bin/sleep", &sleeper).unwrap();
            let mut child = Command::new(sleeper).arg("30").spawn().unwrap();
            let errors = manager.recover_staging();
            let kept = game_dir.join("doorstop_config.ini").exists();
            let _ = child.kill();
            let _ = child.wait();
            assert!(errors.is_empty());
            assert!(kept, "a running game's files must not be restored");
        }
        assert!(manager.recover_staging().is_empty());
        assert_eq!(
            fs::read(game_dir.join("winhttp.dll")).unwrap(),
            b"user's original proxy"
        );
        assert!(!game_dir.join("doorstop_config.ini").exists());
        assert!(
            fs::read_to_string(profile.join("doorstop_config.ini"))
                .unwrap()
                .contains("enabled = true")
        );
    }

    #[test]
    fn restoration_recovers_overwritten_files_removes_new_loader_files_and_preserves_external_updates()
     {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        let receipt_dir = root.path().join("receipt");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&receipt_dir).unwrap();
        fs::write(receipt_dir.join("backup-0"), b"original").unwrap();
        let mut files = Vec::new();
        for (i, name) in ["winhttp.dll", "doorstop_config.ini", "game-update.txt"]
            .iter()
            .enumerate()
        {
            fs::write(game.join(name), b"staged").unwrap();
            files.push(StagedFile {
                relative: name.into(),
                backup: (i == 0).then(|| "backup-0".into()),
                staged: hash(b"staged"),
            });
        }
        fs::write(game.join("game-update.txt"), b"newer external data").unwrap();
        crate::storage::write_json(
            &receipt_dir.join("receipt.json"),
            &Receipt {
                generation: 1,
                game_dir: game.clone(),
                profile: root.path().join("profile"),
                files,
            },
        )
        .unwrap();
        restore(&receipt_dir).unwrap();
        assert_eq!(fs::read(game.join("winhttp.dll")).unwrap(), b"original");
        assert!(!game.join("doorstop_config.ini").exists());
        assert_eq!(
            fs::read(game.join("game-update.txt")).unwrap(),
            b"newer external data"
        );
        assert!(!receipt_dir.exists());
    }

    #[test]
    fn loader_destinations_reject_traversal_and_symlinked_parents() {
        let dir = tempfile::tempdir().unwrap();
        assert!(safe_destination(dir.path(), Path::new("../outside")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), dir.path().join("link")).unwrap();
            assert!(safe_destination(dir.path(), Path::new("link/winhttp.dll")).is_err());
        }
        assert_eq!(
            disable_doorstop("[UnityDoorstop]\nenabled=true\ntargetAssembly=x\n"),
            "[UnityDoorstop]\nenabled = false\ntargetAssembly=x\n"
        );
    }
}
