//! Managed transfers through installed clients. No account credentials are copied.
use crate::{
    model::*,
    steam_control::{Operation, Phase, Progress},
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

mod worker;
pub use worker::run as run_worker;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub key: String,
    pub operation: Operation,
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub completion: Completion,
    pub version: String,
    #[serde(default)]
    pub previous_version: Option<String>,
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
    #[serde(default)]
    pub game_lock: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completion {
    Legendary { profile: PathBuf, app: String },
    Ferestre { record: PathBuf },
}

impl Completion {
    fn revision(&self) -> Option<String> {
        match self {
            Self::Legendary { profile, app } => read_json(&profile.join("installed.json"))?
                .get(app)?["version"]
                .as_str()
                .map(str::to_owned),
            Self::Ferestre { record } => read_json(record)?["package_version"]
                .as_str()
                .map(str::to_owned),
        }
    }

    fn committed(&self, expected: &str, before: Option<&str>) -> bool {
        if self.installed(expected) {
            return true;
        }
        // Legendary can refresh its manifest after the queued request was
        // created. Accept that revision only if it changed and its own refreshed
        // asset catalog confirms it; an unchanged old install is not success.
        let Self::Legendary { profile, app } = self else {
            return false;
        };
        if before.is_none() {
            return false;
        }
        let Some(actual) = self
            .revision()
            .filter(|v| !v.is_empty() && Some(v.as_str()) != before)
        else {
            return false;
        };
        let Some(current) =
            read_json(&profile.join("installed.json")).and_then(|v| v.get(app).cloned())
        else {
            return false;
        };
        let platform = current["platform"].as_str().unwrap_or("Windows");
        let latest = read_json(&profile.join("assets.json")).is_some_and(|assets| {
            assets[platform].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["app_name"].as_str().or_else(|| row["appName"].as_str())
                        == Some(app.as_str())
                        && row["build_version"]
                            .as_str()
                            .or_else(|| row["buildVersion"].as_str())
                            == Some(actual.as_str())
                })
            })
        });
        latest && self.installed(&actual)
    }

    pub fn directory(&self) -> Option<PathBuf> {
        match self {
            Self::Legendary { profile, app } => read_json(&profile.join("installed.json"))
                .and_then(|v| v.get(app).cloned())
                .and_then(|v| v["install_path"].as_str().map(PathBuf::from)),
            Self::Ferestre { record } => {
                read_json(record).and_then(|v| v["dir"].as_str().map(PathBuf::from))
            }
        }
    }
    pub fn installed(&self, version: &str) -> bool {
        match self {
            Self::Legendary { profile, app } => read_json(&profile.join("installed.json"))
                .and_then(|v| v.get(app).cloned())
                .filter(|v| version.is_empty() || v["version"].as_str() == Some(version))
                .and_then(|v| v["install_path"].as_str().map(PathBuf::from))
                .is_some_and(|p| p.is_dir()),
            Self::Ferestre { record } => read_json(record)
                .filter(|v| version.is_empty() || v["package_version"].as_str() == Some(version))
                .and_then(|v| v["dir"].as_str().map(PathBuf::from))
                .is_some_and(|p| p.join(".xodus-streaming.msixvc").is_file()),
        }
    }
}

pub struct Manager {
    root: PathBuf,
    checking: Mutex<Option<Instant>>,
    starting: Mutex<()>,
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 8 * 1024 * 1024)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}

impl Manager {
    pub fn new(data: &Path) -> Self {
        Self {
            root: data.join("downloads"),
            checking: Mutex::new(None),
            starting: Mutex::new(()),
        }
    }

    fn directory(&self, key: &str) -> PathBuf {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hash);
        self.root.join(format!("{:016x}", hash.finish()))
    }

    pub fn request(&self, key: &str) -> Option<Request> {
        serde_json::from_value(read_json(&self.directory(key).join("request.json"))?).ok()
    }

    pub fn snapshot(&self) -> HashMap<String, Progress> {
        let mut result = HashMap::new();
        for row in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            let dir = row.path();
            let Some(mut progress) = read_json(&dir.join("progress.json"))
                .and_then(|v| serde_json::from_value::<Progress>(v).ok())
            else {
                continue;
            };
            let Some(request) = read_json(&dir.join("request.json"))
                .and_then(|v| serde_json::from_value::<Request>(v).ok())
            else {
                continue;
            };
            // The owning client can finish an available update outside Raesir.
            if progress.phase == Phase::Available
                && request
                    .completion
                    .committed(&request.version, request.previous_version.as_deref())
            {
                progress.phase = Phase::Complete;
                progress.percent = Some(100);
                progress.detail = None;
            }
            // A worker owns this lock for the complete transfer. An interrupted
            // worker leaves a resumable job, never an endlessly spinning card.
            if progress.active()
                && !matches!(progress.phase, Phase::Queued | Phase::Paused)
                && fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(dir.join("worker.lock"))
                    .is_ok_and(|f| f.try_lock().is_ok())
            {
                let fresh = fs::metadata(dir.join("progress.json"))
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d < Duration::from_secs(15));
                if !fresh {
                    progress.phase = Phase::Paused;
                    progress.detail = Some("Download interrupted · Resume".into());
                }
            }
            result.insert(request.key, progress);
        }
        result
    }

    pub fn available(&self, mut request: Request) -> Result<(), String> {
        let dir = self.directory(&request.key);
        let current = read_json(&dir.join("progress.json"))
            .and_then(|v| serde_json::from_value::<Progress>(v).ok());
        if current.as_ref().is_some_and(|p| p.active()) {
            return Ok(());
        }
        if self.request(&request.key).is_some_and(|old| {
            old.version == request.version && old.completion == request.completion
        }) && current.is_some()
        {
            return Ok(());
        }
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        request.previous_version = request.completion.revision();
        storage::write_json(&dir.join("request.json"), &request)?;
        let mut progress = Progress::starting(request.operation);
        progress.phase = Phase::Available;
        storage::write_json(&dir.join("progress.json"), &progress)
    }

    pub fn start(&self, mut request: Request) -> Result<(), String> {
        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
            return Err("Downloads are disabled in this diagnostic profile".into());
        }
        let dir = self.directory(&request.key);
        let _starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if worker::busy(&dir) {
            drop(_starting);
            return self.control(&request.key, true);
        }
        let occupied = self.snapshot().into_iter().any(|(key, p)| {
            key != request.key
                && p.active()
                && p.phase != Phase::Queued
                && self
                    .request(&key)
                    .is_some_and(|other| queue_group(&other) == queue_group(&request))
        });
        if occupied || client_busy(&request) {
            storage::write_json(&dir.join("request.json"), &request)?;
            let mut progress = Progress::starting(request.operation);
            progress.phase = Phase::Queued;
            progress.detail = Some("Waiting for another client download".into());
            return storage::write_json(&dir.join("progress.json"), &progress);
        }
        request.game_dir = request.completion.directory();
        if request.previous_version.is_none() {
            request.previous_version = request.completion.revision();
        }
        request.game_lock = request
            .game_dir
            .as_ref()
            .map(|dir| crate::mods::download_lock_path(dir));
        if Path::new("/.flatpak-info").is_file() && request.program.starts_with("/app/bin/") {
            request.program = Path::new(&request.program)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
        }
        storage::write_json(&dir.join("request.json"), &request)?;
        storage::write_json(
            &dir.join("progress.json"),
            &Progress::starting(request.operation),
        )?;
        let _ = fs::remove_file(dir.join("control.json"));
        let mut command = if cfg!(target_os = "linux") && Path::new("/.flatpak-info").is_file() {
            let script = self.root.join("host-worker.py");
            fs::write(&script, include_str!("downloads/host_worker.py"))
                .map_err(|e| e.to_string())?;
            let mut c = Command::new("flatpak-spawn");
            c.args(["--host", "python3"]).arg(script).arg(&dir);
            c
        } else {
            let mut c = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
            c.arg("--download-worker").arg(&dir);
            c
        };
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
        let mut child = match command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                let mut progress = Progress::starting(request.operation);
                progress.phase = Phase::Failed;
                progress.detail = Some("Could not start the game download worker".into());
                storage::write_json(&dir.join("progress.json"), &progress)?;
                return Err("Could not start the game download worker".into());
            }
        };
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    pub fn control(&self, key: &str, resume: bool) -> Result<(), String> {
        let request = self.request(key).ok_or("Download is no longer available")?;
        let dir = self.directory(key);
        if !worker::busy(&dir) {
            if resume {
                return self.start(request);
            }
            let current = self.snapshot().remove(key);
            if current.is_some_and(|p| p.phase == Phase::Queued) {
                let mut progress = Progress::starting(request.operation);
                progress.phase = Phase::Paused;
                return storage::write_json(&dir.join("progress.json"), &progress);
            }
        }
        storage::write_json(
            &dir.join("control.json"),
            &serde_json::json!({
                "resume": resume, "generation": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
            }),
        )
    }

    pub fn finish(&self) {
        for (key, progress) in self.snapshot() {
            if progress.phase == Phase::Complete {
                let _ = fs::remove_file(self.directory(&key).join("progress.json"));
                let _ = fs::remove_file(self.directory(&key).join("output.log"));
            }
        }
    }

    pub fn advance_queue(&self) {
        let mut groups = std::collections::HashSet::new();
        for (key, progress) in self.snapshot() {
            if progress.phase == Phase::Queued
                && !worker::busy(&self.directory(&key))
                && let Some(request) = self.request(&key)
                && groups.insert(queue_group(&request))
            {
                let _ = self.start(request);
            }
        }
    }

    pub fn check_updates(&self, report: &Report, force: bool) -> Vec<String> {
        let mut checking = self.checking.lock().unwrap_or_else(|e| e.into_inner());
        if !force && checking.is_some_and(|t| t.elapsed() < Duration::from_secs(15 * 60)) {
            return vec![];
        }
        *checking = Some(Instant::now());
        drop(checking);
        let mut errors = vec![];
        if let Some(binary) = crate::epic_online::binary() {
            for source in report
                .epic
                .sources
                .iter()
                .filter(|s| s.local && s.kind == "legendary")
            {
                let mut command = client_command(&binary.to_string_lossy());
                command
                    .args(["list-installed", "--check-updates", "--json"])
                    .env("LEGENDARY_CONFIG_PATH", &source.path);
                if let Err(e) = query(command) {
                    errors.push(format!("Epic update check: {e}"));
                    continue;
                }
                for game in report.catalog.iter().filter(|g| g.store == "epic") {
                    if let Some(request) = legendary_request(report, game, Operation::Update)
                        && request.env.get("LEGENDARY_CONFIG_PATH") == Some(&source.path)
                    {
                        let _ = self.available(request);
                    }
                }
            }
        }
        if report
            .catalog
            .iter()
            .any(|game| game.client == "Ferestre" && crate::catalog::local(&game.installations))
            && let Some(binary) = crate::discovery::program("ferestre")
        {
            let mut command = client_command(&binary);
            command.args(["--json", "updates"]);
            match query(command) {
                Ok(value) => {
                    for row in value["updates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|r| r["update_available"].as_bool() == Some(true))
                    {
                        if let Some(game) = report.catalog.iter().find(|g| {
                            g.store == "xbox" && row["product_id"].as_str() == Some(g.id.as_str())
                        }) && let Some(mut request) = ferestre_request(game, Operation::Update)
                        {
                            request.version =
                                row["available_version"].as_str().unwrap_or("").into();
                            let _ = self.available(request);
                        }
                    }
                }
                Err(e) => errors.push(format!("Xbox update check: {e}")),
            }
        }
        errors
    }
}

fn queue_group(request: &Request) -> String {
    match &request.completion {
        Completion::Legendary { profile, .. } => format!("legendary:{}", profile.display()),
        Completion::Ferestre { .. } => "ferestre".into(),
    }
}

fn client_busy(request: &Request) -> bool {
    match &request.completion {
        Completion::Legendary { profile, .. } => fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(profile.join("installed.json.lock"))
            .is_ok_and(|file| file.try_lock().is_err()),
        Completion::Ferestre { .. } => false,
    }
}

fn client_command(program: &str) -> Command {
    if cfg!(target_os = "linux") && Path::new("/.flatpak-info").is_file() {
        let program = program.strip_prefix("/app/bin/").unwrap_or(program);
        let mut c = Command::new("flatpak-spawn");
        c.args(["--host", program]);
        c
    } else {
        Command::new(program)
    }
}

fn query(mut command: Command) -> Result<serde_json::Value, String> {
    // Files avoid blocked pipes; no auth output is exposed to the UI.
    let out = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut reader = out.try_clone().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "The installed client helper could not start")?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Client update check timed out".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        return Err("Sign in to the game client and retry".into());
    }
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    serde_json::from_reader(reader.take(8 * 1024 * 1024))
        .map_err(|_| "Client returned invalid update metadata".into())
}

pub fn install_request(report: &Report, game: &LibraryGame) -> Option<Request> {
    if !game.in_library || crate::catalog::local(&game.installations) {
        return None;
    }
    legendary_request(report, game, Operation::Install)
        .or_else(|| ferestre_request(game, Operation::Install))
}

fn legendary_request(report: &Report, game: &LibraryGame, operation: Operation) -> Option<Request> {
    if game.store != "epic" {
        return None;
    }
    let program = crate::epic_online::binary()?.to_string_lossy().into_owned();
    legendary_plan(report, game, operation, program)
}

fn legendary_plan(
    report: &Report,
    game: &LibraryGame,
    operation: Operation,
    program: String,
) -> Option<Request> {
    if game.store != "epic" || !game.in_library {
        return None;
    }
    let profile = report.epic.sources.iter().find(|s| {
        s.local
            && s.kind == "legendary"
            && (game
                .installations
                .iter()
                .any(|i| i.os == Os::host() && i.library == s.path)
                || operation == Operation::Install
                    && report
                        .epic
                        .games
                        .iter()
                        .any(|g| g.app_name == game.id && g.library_sources.contains(&s.path)))
    })?;
    let path = Path::new(&profile.path);
    let mut version = String::new();
    if operation == Operation::Update {
        let installed = read_json(&path.join("installed.json"))?;
        let current = installed.get(&game.id)?;
        let platform = current["platform"].as_str().unwrap_or("Windows");
        let assets = read_json(&path.join("assets.json"))?;
        let latest = assets[platform]
            .as_array()?
            .iter()
            .find(|a| a["app_name"].as_str().or_else(|| a["appName"].as_str()) == Some(&game.id))?;
        version = latest["build_version"]
            .as_str()
            .or_else(|| latest["buildVersion"].as_str())?
            .into();
        if version.is_empty()
            || current["version"].as_str() == Some(&version)
            || !crate::catalog::local(&game.installations)
        {
            return None;
        }
    }
    Some(Request {
        key: game.key.clone(),
        operation,
        program,
        args: vec![
            "-y".into(),
            "install".into(),
            "--skip-dlcs".into(),
            "--".into(),
            game.id.clone(),
        ],
        env: BTreeMap::from([("LEGENDARY_CONFIG_PATH".into(), profile.path.clone())]),
        completion: Completion::Legendary {
            profile: path.into(),
            app: game.id.clone(),
        },
        version,
        previous_version: None,
        game_dir: None,
        game_lock: None,
    })
}

fn ferestre_request(game: &LibraryGame, operation: Operation) -> Option<Request> {
    if game.store != "xbox" || game.client != "Ferestre" || game.id.len() != 12 {
        return None;
    }
    let program = crate::discovery::program("ferestre")?;
    let home = crate::platform::home()?;
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("ferestre");
    Some(Request {
        key: game.key.clone(),
        operation,
        program,
        args: vec!["install".into(), game.id.clone()],
        env: BTreeMap::new(),
        completion: Completion::Ferestre {
            record: state.join("installed").join(format!("{}.json", game.id)),
        },
        version: String::new(),
        previous_version: None,
        game_dir: None,
        game_lock: None,
    })
}

pub fn parse_progress(line: &str) -> Option<u8> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        let progress = &value["progress"];
        let done = progress["done"].as_u64()?;
        let total = progress["total"].as_u64()?;
        return (total > 0 && done <= total)
            .then(|| (done.saturating_mul(100) / total).min(99) as u8);
    }
    let text = line.split("Progress:").nth(1)?;
    let number = text.split('%').next()?.trim().parse::<f64>().ok()?;
    (number.is_finite() && (0.0..=100.0).contains(&number)).then_some(number.min(99.0) as u8)
}

#[cfg(test)]
mod tests;
