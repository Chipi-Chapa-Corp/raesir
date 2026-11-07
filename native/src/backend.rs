use raesir_backend::{
    actions, artwork, client_control, epic_online,
    model::*,
    mount,
    steam_control::{self, Operation, Phase, Progress},
    storage,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Backend {
    pub data: PathBuf,
    pub cache: PathBuf,
    mod_manager: raesir_backend::mods::Manager,
    mod_launches: Mutex<Vec<raesir_backend::mods::LaunchTask>>,
    fixture: Option<PathBuf>,
    report: RwLock<Option<Arc<Report>>>,
    achievement_icons: Mutex<HashMap<String, Arc<gpui::RenderImage>>>,
    operation: Mutex<()>,
    cleanup: Mutex<()>,
    ui: OnceLock<Mutex<serde_json::Map<String, serde_json::Value>>>,
    ui_save: Mutex<()>,
    closing: AtomicBool,
    closed: AtomicBool,
    steam: client_control::Manager,
    steam_jobs: Mutex<HashMap<String, SteamJob>>,
    steam_bridge: Mutex<Option<(String, std::time::SystemTime, bool)>>,
    epic: epic_online::Manager,
    downloads: raesir_backend::downloads::Manager,
    client_downloads: raesir_backend::client_downloads::Manager,
    handoffs: Mutex<HashMap<String, Progress>>,
    handoffs_loaded: OnceLock<()>,
    handoff_scan: Mutex<std::time::Instant>,
    game_checking: AtomicBool,
    mounts: mount::Manager,
}

#[derive(Clone)]
struct SteamJob {
    root: String,
    library: PathBuf,
    appid: u32,
    progress: Progress,
    started: std::time::Instant,
}

fn merge_download_progress(live: Option<Progress>, disk: Progress) -> Progress {
    let mut progress = live.unwrap_or_else(|| disk.clone());
    // Completion also requires Steam's committed manifest and payload.
    if progress.phase == Phase::Complete && disk.phase != Phase::Complete {
        progress.phase = Phase::Installing;
        progress.percent = Some(100);
    }
    if disk.error == Some(steam_control::Error::Disk) {
        return disk;
    }
    progress
}

impl Backend {
    pub fn new() -> Arc<Self> {
        let data = std::env::var_os("RAESIR_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .expect("user data directory")
                    .join("dev.raesir.launcher")
            });
        let cache = std::env::var_os("RAESIR_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::cache_dir()
                    .expect("user cache directory")
                    .join("dev.raesir.launcher")
            });
        let mod_manager = raesir_backend::mods::Manager::new(&data, &cache);
        let downloads = raesir_backend::downloads::Manager::new(&data);
        let client_downloads = raesir_backend::client_downloads::Manager::new(&data);
        Arc::new(Self {
            data,
            cache,
            mod_manager,
            mod_launches: Mutex::new(Vec::new()),
            fixture: std::env::var_os("RAESIR_LIBRARY_FIXTURE").map(PathBuf::from),
            report: RwLock::new(None),
            achievement_icons: Mutex::new(HashMap::new()),
            operation: Mutex::new(()),
            cleanup: Mutex::new(()),
            ui: OnceLock::new(),
            ui_save: Mutex::new(()),
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            steam: Default::default(),
            steam_jobs: Default::default(),
            steam_bridge: Default::default(),
            epic: Default::default(),
            downloads,
            client_downloads,
            handoffs: Default::default(),
            handoffs_loaded: OnceLock::new(),
            handoff_scan: Mutex::new(std::time::Instant::now()),
            game_checking: AtomicBool::new(false),
            mounts: Default::default(),
        })
    }

    fn ready(&self) -> Result<(), String> {
        if self.closing.load(Ordering::SeqCst) {
            Err("Raesir is closing".into())
        } else {
            Ok(())
        }
    }

    fn set(&self, mut report: Report) -> Arc<Report> {
        if self.fixture.is_none() {
            report.warnings.extend(self.mod_manager.recover_staging());
            self.steam_actions(&mut report);
            self.observe_handoffs(&report);
        }
        for game in &mut report.catalog {
            game.mods = self.mod_manager.summary(game);
        }
        let mut current = self.report.write().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = current.as_ref() {
            for game in &mut report.catalog {
                if game.title == game.id
                    && let Some(old) = previous
                        .catalog
                        .iter()
                        .find(|g| g.key == game.key && g.title != g.id)
                {
                    game.title = old.title.clone();
                }
            }
        }
        let report = Arc::new(report);
        *current = Some(report.clone());
        report
    }

    pub fn cached(&self) -> Result<Option<Arc<Report>>, String> {
        let report = if let Some(path) = &self.fixture {
            Some(
                serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            storage::cached(&self.data)
        };
        Ok(report.map(|r| self.set(r)))
    }

    pub fn scan(&self) -> Result<Arc<Report>, String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.ready()?;
        if self.fixture.is_some() {
            return self.cached()?.ok_or("No fixture library".into());
        }
        Ok(self.set(storage::scan_and_save(&self.data, &ScanOptions::default())))
    }

    pub fn refresh_game_pass(&self, force: bool) -> Result<Option<Arc<Report>>, String> {
        if self.fixture.is_some() {
            return Ok(None);
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.ready()?;
        let mut report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|r| (**r).clone())
            .ok_or("Library is not ready")?;
        if !raesir_backend::game_pass::refresh(&self.data, &mut report, force)? {
            return Ok(None);
        }
        self.ready()?;
        storage::write_json(&self.data.join("inventory.json"), &report)?;
        Ok(Some(self.set(report)))
    }

    pub fn ownership(&self) -> Result<Option<Arc<Report>>, String> {
        if self.fixture.is_some() {
            return Ok(None);
        }
        self.ready()?;
        let status = self.steam.status();
        let root = status
            .managed_root
            .or_else(|| status.clients.first().map(|c| c.root.clone()));
        let Some(root) = root.filter(|_| status.supported) else {
            return Ok(None);
        };
        let owned = self.steam.read_library(&root)?;
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.ready()?;
        storage::write_json(&self.data.join("owned.json"), &owned)?;
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Library is still loading")?;
        Ok(Some(self.set(storage::apply_owned(
            &self.data,
            &owned,
            (*report).clone(),
        ))))
    }

    pub fn epic_profiles(&self) -> Vec<String> {
        if self.fixture.is_some() {
            return vec![];
        }
        self.report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|r| r.epic.legendary_available)
            .map(|r| {
                r.epic
                    .sources
                    .iter()
                    .filter(|s| s.local && s.kind == "legendary")
                    .map(|s| s.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn refresh_epic(&self, profile: &str) -> Result<Arc<Report>, String> {
        self.ready()?;
        let library = self.epic.refresh(profile)?;
        self.ready()?;
        epic_online::save(&self.data, library)?;
        self.scan()
    }

    pub fn mount(&self, id: &str) -> Result<Arc<Report>, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return Err("Mounting is disabled in the fixture profile".into());
        }
        if std::env::var_os("RAESIR_DISABLE_MOUNTS").is_some() {
            return Err("Mounting is disabled in this diagnostic profile".into());
        }
        self.mounts.mount(id)?;
        self.scan()
    }

    pub fn action(&self, key: &str, kind: &str) -> Result<String, String> {
        self.ready()?;
        if self.fixture.is_some()
            && !(matches!(kind, "launch" | "launch-modded")
                && raesir_backend::mods::diagnostic_launch_enabled())
        {
            return Err("Client actions are disabled in the fixture profile".into());
        }
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Library is still loading")?;
        if matches!(kind, "launch" | "launch-modded") {
            let game = report
                .catalog
                .iter()
                .find(|g| g.key == key)
                .ok_or("Game is no longer in the library")?;
            let test_args =
                if self.fixture.is_some() && raesir_backend::mods::diagnostic_launch_enabled() {
                    mod_test_args()?
                } else {
                    Vec::new()
                };
            if kind == "launch-modded"
                || self.fixture.is_some() && raesir_backend::mods::diagnostic_launch_enabled()
            {
                let launch = if kind == "launch-modded" {
                    self.mod_manager.launch_modded(&report, game, &test_args)?
                } else {
                    self.mod_manager.launch_vanilla(&report, game, &test_args)?
                };
                self.mod_launches
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(launch);
                return Ok("Sent to modded game".into());
            }
            self.mod_manager.restore_for_game(game)?;
            if game.store == "steam" {
                let args = self.mod_manager.vanilla_args(game)?;
                if !args.is_empty() {
                    return actions::launch_with_args(&report, key, &args);
                }
            }
        }
        if kind == "update" {
            let progress = self
                .current_download_progress()
                .remove(key)
                .filter(|p| p.operation == Operation::Update && !matches!(p.phase, Phase::Complete))
                .ok_or("This game has no available update")?;
            let _ = progress;
            self.control_download(key, true)?;
            return Ok("Update started".into());
        }
        let action = actions::resolve(&report, key, kind)?;
        if kind == "install"
            && self.fixture.is_none()
            && self.client_downloads.install(&report, key)?
        {
            return Ok("Installation sent to the game client".into());
        }
        if kind == "install"
            && let Some(game) = report.catalog.iter().find(|g| g.key == key)
            && let Some(request) = raesir_backend::downloads::install_request(&report, game)
        {
            self.downloads.start(request)?;
            return Ok("Download started".into());
        }
        if let ActionTarget::Steam {
            root,
            appid,
            operation,
        } = action.target
        {
            let result = self.steam.control(&root, appid, operation.as_str());
            let library = result
                .as_ref()
                .ok()
                .and_then(|status| {
                    steam_control::libraries(Path::new(&root))
                        .into_iter()
                        .find(|(index, _)| *index == status.library)
                        .map(|(_, path)| path)
                })
                .unwrap_or_else(|| Path::new(&root).join("unavailable-library"));
            let mut progress = Progress::starting(operation);
            if let Err(error) = result {
                progress.phase = Phase::Failed;
                progress.error = Some(error);
            }
            self.steam_jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(
                    key.into(),
                    SteamJob {
                        root,
                        library,
                        appid,
                        progress,
                        started: std::time::Instant::now(),
                    },
                );
            return result
                .map(|_| "Sent to Steam".into())
                .map_err(|error| error.message().into());
        }
        if kind == "install"
            && let Some(game) = report
                .catalog
                .iter()
                .find(|g| g.key == key && g.store == "steam")
        {
            if let Some(client) = self.steam.status().clients.first() {
                self.steam.preserve(&client.root);
            }
            let result = actions::perform(&report, key, kind)?;
            if let Some(client) = self.steam.status().clients.first() {
                let mut progress = Progress::starting(Operation::Install);
                progress.controllable = false;
                self.steam_jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        key.into(),
                        SteamJob {
                            root: client.root.clone(),
                            library: PathBuf::from(&client.root),
                            appid: game.id.parse().unwrap_or(0),
                            progress,
                            started: std::time::Instant::now(),
                        },
                    );
            }
            return Ok(result);
        }
        let result = actions::perform(&report, key, kind)?;
        if kind == "install"
            && let Some(game) = report.catalog.iter().find(|g| g.key == key)
        {
            let mut progress = Progress::starting(Operation::Install);
            progress.phase = Phase::Queued;
            progress.controllable = false;
            progress.detail = Some(format!("Continue installing in {}", game.client));
            self.handoffs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.into(), progress);
            self.save_handoffs();
        }
        Ok(result)
    }

    fn save_handoffs(&self) {
        let handoffs = self.handoffs.lock().unwrap_or_else(|e| e.into_inner());
        let _ = storage::write_json(&self.data.join("client-downloads.json"), &*handoffs);
    }

    fn observe_handoffs(&self, report: &Report) {
        self.handoffs_loaded.get_or_init(|| {
            if let Ok(bytes) = std::fs::read(self.data.join("client-downloads.json"))
                && let Ok(saved) = serde_json::from_slice::<HashMap<String, Progress>>(&bytes)
            {
                *self.handoffs.lock().unwrap_or_else(|e| e.into_inner()) = saved;
            }
        });
        {
            let mut handoffs = self.handoffs.lock().unwrap_or_else(|e| e.into_inner());
            for (key, progress) in handoffs.iter_mut().filter(|(_, p)| p.active()) {
                if let Some(game) = report.catalog.iter().find(|g| &g.key == key) {
                    if raesir_backend::catalog::local(&game.installations) {
                        progress.phase = Phase::Complete;
                        progress.percent = Some(100);
                        progress.detail = None;
                    } else if game
                        .installations
                        .iter()
                        .any(|i| i.os == Os::host() && i.state == "incomplete")
                    {
                        progress.phase = Phase::Installing;
                        progress.detail = Some(format!("Installation managed by {}", game.client));
                    }
                } else {
                    progress.phase = Phase::Failed;
                    progress.detail = Some("Game is no longer in this library".into());
                }
            }
        }
        self.save_handoffs();
    }

    fn steam_actions(&self, report: &mut Report) {
        let client = client_control::clients().into_iter().next();
        let mut bridge = false;
        if let Some(client) = &client
            && cfg!(all(target_os = "linux", target_arch = "x86_64"))
            && let Ok(modified) =
                std::fs::metadata(Path::new(&client.root).join("linux64/steamclient.so"))
                    .and_then(|m| m.modified())
        {
            let mut cached = self.steam_bridge.lock().unwrap_or_else(|p| p.into_inner());
            bridge = if let Some((root, stamp, available)) = cached.as_ref()
                && root == &client.root
                && *stamp == modified
            {
                *available
            } else {
                let result = self.steam.control(&client.root, 0, "probe");
                if result.is_ok() || matches!(result, Err(steam_control::Error::Unavailable)) {
                    *cached = Some((client.root.clone(), modified, result.is_ok()));
                }
                result.is_ok()
            };
        }
        let mut jobs = self.steam_jobs.lock().unwrap_or_else(|p| p.into_inner());
        for game in report.catalog.iter_mut().filter(|g| g.store == "steam") {
            let Some(client) = &client else {
                game.install = None;
                game.uninstall = None;
                continue;
            };
            let Ok(appid) = game.id.parse::<u32>() else {
                game.install = None;
                continue;
            };
            let local = raesir_backend::catalog::local(&game.installations);
            if bridge {
                let operation = if local {
                    Operation::Uninstall
                } else {
                    Operation::Install
                };
                let action = GameAction {
                    client: "Steam".into(),
                    target: ActionTarget::Steam {
                        root: client.root.clone(),
                        appid,
                        operation,
                    },
                    note: (operation == Operation::Install)
                        .then(|| "Install in Steam's main local library.".into()),
                };
                if local {
                    game.uninstall = Some(action);
                } else if game.in_library && game.client_os.contains(&Os::host()) {
                    game.install = Some(action);
                } else {
                    game.install = None;
                }
            } else {
                // Explicit documented handoff on unsupported builds/platforms;
                // never reuse an old cached native action from another client.
                if local {
                    game.uninstall = Some(raesir_backend::catalog::url(
                        "Steam",
                        format!("steam://uninstall/{appid}"),
                    ));
                } else if game.in_library && game.client_os.contains(&Os::host()) {
                    let mut action =
                        raesir_backend::catalog::url("Steam", format!("steam://install/{appid}"));
                    action.note = Some("Steam will open its installation dialog.".into());
                    game.install = Some(action);
                } else {
                    game.install = None;
                }
            }
            // Adopt downloads already in progress, including after restarting
            // Raesir. A missing payload by itself is not a download operation.
            if !jobs.contains_key(&game.key)
                || jobs
                    .get(&game.key)
                    .is_some_and(|job| job.progress.phase == Phase::Available)
            {
                let operation = if steam_control::installed_payload(Path::new(&client.root), appid)
                {
                    Operation::Update
                } else {
                    Operation::Install
                };
                let progress = steam_control::progress(Path::new(&client.root), appid, operation);
                if matches!(
                    progress.phase,
                    Phase::Available
                        | Phase::Queued
                        | Phase::Downloading
                        | Phase::Installing
                        | Phase::Paused
                ) {
                    let mut progress = progress;
                    progress.controllable = bridge;
                    jobs.insert(
                        game.key.clone(),
                        SteamJob {
                            root: client.root.clone(),
                            library: steam_control::app_library(Path::new(&client.root), appid)
                                .unwrap_or_else(|| PathBuf::from(&client.root)),
                            appid,
                            progress,
                            started: std::time::Instant::now(),
                        },
                    );
                } else if progress.phase == Phase::Complete {
                    jobs.remove(&game.key);
                }
            }
        }
    }

    pub fn watch_client_downloads(&self) -> bool {
        self.fixture.is_none() && self.client_downloads.observing()
    }

    pub fn download_progress(&self) -> HashMap<String, Progress> {
        if self.fixture.is_none() {
            self.downloads.advance_queue();
            if let Some(report) = self
                .report
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                self.client_downloads.refresh(&report, false, false);
                if self.automatic_game_updates() {
                    self.client_downloads.automatic(&report);
                }
            }
        }
        let observing = self
            .handoffs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(Progress::active)
            || self
                .client_downloads
                .snapshot()
                .values()
                .any(Progress::active);
        let due = {
            let mut last = self.handoff_scan.lock().unwrap_or_else(|e| e.into_inner());
            let due = observing && last.elapsed().as_secs() >= 5;
            if due {
                *last = std::time::Instant::now();
            }
            due
        };
        if due {
            let _ = self.scan();
        }
        // Never hold the job lock across client IPC: the UI reads this cache.
        let pending: Vec<_> = self
            .steam_jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(_, job)| job.progress.active())
            .map(|(key, job)| (key.clone(), job.clone()))
            .collect();
        for (key, snapshot) in pending {
            let library = if snapshot.progress.phase == Phase::Starting {
                steam_control::app_library(Path::new(&snapshot.root), snapshot.appid)
                    .unwrap_or_else(|| snapshot.library.clone())
            } else {
                snapshot.library.clone()
            };
            let disk = steam_control::progress_in_library(
                &library,
                snapshot.appid,
                snapshot.progress.operation,
            );
            let live = if snapshot.progress.operation.downloads() && snapshot.progress.controllable
            {
                self.steam
                    .control(&snapshot.root, snapshot.appid, "status")
                    .ok()
                    .map(|status| status.progress(snapshot.progress.operation))
            } else {
                None
            };
            let mut progress = merge_download_progress(live, disk);
            progress.controllable = snapshot.progress.controllable;
            if progress.phase == Phase::Available && snapshot.progress.phase == Phase::Starting {
                progress.phase = Phase::Starting;
            }
            if progress.phase == Phase::Starting && snapshot.started.elapsed().as_secs() > 60 {
                progress.phase = Phase::Failed;
                progress.error = Some(steam_control::Error::Rejected);
            }
            let mut jobs = self.steam_jobs.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(job) = jobs.get_mut(&key)
                && job.started == snapshot.started
            {
                progress.error = progress.error.or(job.progress.error);
                job.progress = progress;
                job.library = library;
            }
        }
        self.current_download_progress()
    }

    pub fn control_download(&self, key: &str, resume: bool) -> Result<(), String> {
        use steam_control::Error;
        if self.ready().is_err() || self.fixture.is_some() {
            return Err(Error::Unavailable.message().into());
        }
        if let Some(report) = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            && self.client_downloads.control(&report, key, resume)?
        {
            return Ok(());
        }
        if self.downloads.request(key).is_some() {
            return self.downloads.control(key, resume);
        }
        let snapshot = self
            .steam_jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .filter(|job| {
                job.progress.operation.downloads() && job.progress.phase != Phase::Complete
            })
            .cloned()
            .ok_or_else(|| Error::Busy.message().to_string())?;
        if !snapshot.progress.controllable {
            if !resume {
                return Err("Pause this download in the game client".into());
            }
            self.steam.preserve(&snapshot.root);
            actions::dispatch(&ActionTarget::Url {
                url: format!("steam://install/{}", snapshot.appid),
            })?;
            if let Some(job) = self
                .steam_jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_mut(key)
            {
                job.progress.phase = Phase::Starting;
                job.started = std::time::Instant::now();
            }
            return Ok(());
        }
        let result = self.steam.control(
            &snapshot.root,
            snapshot.appid,
            if resume { "resume" } else { "pause" },
        );
        if let Some(job) = self
            .steam_jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(key)
        {
            job.started = std::time::Instant::now();
            match result {
                Ok(_) => {
                    job.progress.phase = if resume {
                        Phase::Starting
                    } else {
                        Phase::Paused
                    };
                    job.progress.error = None;
                }
                Err(error) => job.progress.error = Some(error),
            }
        }
        result.map(|_| ()).map_err(|e| e.message().to_string())
    }

    pub fn current_download_progress(&self) -> HashMap<String, Progress> {
        let mut progress: HashMap<_, _> = self
            .steam_jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(key, job)| (key.clone(), job.progress.clone()))
            .collect();
        progress.extend(self.downloads.snapshot());
        progress.extend(
            self.handoffs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        );
        progress.extend(self.client_downloads.snapshot());
        if self.fixture.is_some()
            && let Ok(bytes) = std::fs::read(self.data.join("download-progress.json"))
            && let Ok(fixture) = serde_json::from_slice::<HashMap<String, Progress>>(&bytes)
        {
            progress.extend(fixture);
        }
        progress
    }

    pub fn finish_download_jobs(&self) {
        self.downloads.finish();
        self.client_downloads.finish();
        self.handoffs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, p| p.phase != Phase::Complete);
        if self.fixture.is_none() {
            self.save_handoffs();
        }
        self.steam_jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, job| job.progress.phase != Phase::Complete);
    }

    pub fn automatic_game_updates(&self) -> bool {
        self.ui()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("automatic_game_updates")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }

    pub fn set_automatic_game_updates(&self, enabled: bool) {
        self.ui()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("automatic_game_updates".into(), serde_json::json!(enabled));
    }

    pub fn check_game_updates(&self, force: bool) -> (HashMap<String, Progress>, Vec<String>) {
        if self.fixture.is_some()
            || self.closing.load(Ordering::SeqCst)
            || std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
        {
            return (self.current_download_progress(), vec![]);
        }
        if self.game_checking.swap(true, Ordering::SeqCst) {
            return (self.current_download_progress(), vec![]);
        }
        struct CheckGuard<'a>(&'a AtomicBool);
        impl Drop for CheckGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = CheckGuard(&self.game_checking);
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(report) = report else {
            return (HashMap::new(), vec![]);
        };
        self.steam_actions(&mut (*report).clone());
        let mut errors = self.downloads.check_updates(&report, force);
        errors.extend(
            self.client_downloads
                .refresh(&report, true, self.automatic_game_updates()),
        );
        if self.automatic_game_updates() {
            for (key, progress) in self.current_download_progress() {
                if progress.operation == Operation::Update && progress.phase == Phase::Available {
                    if let Err(error) = self.control_download(&key, true) {
                        errors.push(error);
                    }
                }
            }
        }
        (self.download_progress(), errors)
    }

    pub fn mod_catalog(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> Result<raesir_backend::mods::Snapshot, String> {
        self.ready()?;
        self.mod_manager.snapshot(game, refresh)
    }

    pub fn mod_auto_updates_enabled(&self) -> bool {
        let test = cfg!(debug_assertions)
            && std::env::var_os("RAESIR_MODS_TEST_UPDATES").is_some_and(|v| v == "1");
        (self.fixture.is_none() || test)
            && (std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() || test)
    }

    pub fn auto_update_mods(
        &self,
        startup: bool,
    ) -> Result<Option<(raesir_backend::mods::UpdateReport, Option<Arc<Report>>)>, String> {
        self.ready()?;
        if !self.mod_auto_updates_enabled() {
            return Ok(None);
        }
        let Some(report) = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Ok(None);
        };
        let result = self
            .mod_manager
            .auto_update(&report.catalog, startup, &self.closing)?;
        if self.closing.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let report = if result.updated.is_empty() {
            None
        } else {
            Some(self.refresh_mods()?)
        };
        Ok(Some((result, report)))
    }

    pub fn mod_launch_progress(&self) -> (bool, Vec<String>) {
        let mut jobs = self.mod_launches.lock().unwrap_or_else(|e| e.into_inner());
        let mut errors = Vec::new();
        jobs.retain(|job| match job.poll() {
            None => true,
            Some(result) => {
                if let Err(error) = result {
                    errors.push(error);
                }
                false
            }
        });
        (!jobs.is_empty(), errors)
    }

    pub fn install_mod(
        &self,
        game: &LibraryGame,
        package: &str,
    ) -> Result<(raesir_backend::mods::Snapshot, Arc<Report>), String> {
        self.ready()?;
        if self.fixture.is_some() && std::env::var_os("RAESIR_MODS_TEST_INSTALL").is_none() {
            return Err("Mod installation is disabled in this fixture profile".into());
        }
        let snapshot = self.mod_manager.install(game, package)?;
        Ok((snapshot, self.refresh_mods()?))
    }

    pub fn choose_mod_profile(
        &self,
        game: &LibraryGame,
        profile: &str,
    ) -> Result<Arc<Report>, String> {
        self.ready()?;
        self.mod_manager.select(game, profile)?;
        self.refresh_mods()
    }

    pub fn mod_details(
        &self,
        game: &LibraryGame,
        package: &str,
    ) -> Result<raesir_backend::details::Details, String> {
        self.ready()?;
        self.mod_manager.details(game, package)
    }

    pub fn uninstall_mod(
        &self,
        game: &LibraryGame,
        package: &str,
    ) -> Result<(raesir_backend::mods::Snapshot, Arc<Report>), String> {
        self.ready()?;
        if self.fixture.is_some() && std::env::var_os("RAESIR_MODS_TEST_INSTALL").is_none() {
            return Err("Mod changes are disabled in this fixture profile".into());
        }
        let snapshot = self.mod_manager.uninstall(game, package)?;
        Ok((snapshot, self.refresh_mods()?))
    }

    pub fn import_nexus_mod(
        &self,
        game: &LibraryGame,
        url: &str,
        archive: &Path,
        folder: &str,
    ) -> Result<(raesir_backend::mods::Snapshot, Arc<Report>), String> {
        self.ready()?;
        if self.fixture.is_some() && std::env::var_os("RAESIR_MODS_TEST_INSTALL").is_none() {
            return Err("Mod changes are disabled in this fixture profile".into());
        }
        let snapshot = self.mod_manager.import_nexus(game, url, archive, folder)?;
        Ok((snapshot, self.refresh_mods()?))
    }

    pub fn mod_files(
        &self,
        game: &LibraryGame,
        id: &str,
    ) -> Result<Vec<raesir_backend::mods::ModFile>, String> {
        self.ready()?;
        self.mod_manager.mod_files(game, id)
    }

    pub fn edit_mod_profile(
        &self,
        game: &LibraryGame,
        operation: &str,
        id: &str,
        name: &str,
    ) -> Result<(raesir_backend::mods::Snapshot, Arc<Report>), String> {
        self.ready()?;
        match operation {
            "create" => self.mod_manager.create_profile(game, name)?,
            "rename" => self.mod_manager.rename_profile(game, id, name)?,
            "delete" => self.mod_manager.delete_profile(game, id)?,
            _ => return Err("Unknown profile action".into()),
        }
        Ok((
            self.mod_manager.snapshot(game, false)?,
            self.refresh_mods()?,
        ))
    }

    pub fn refresh_mods(&self) -> Result<Arc<Report>, String> {
        self.ready()?;
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Library is not ready")?;
        Ok(self.set((*report).clone()))
    }

    pub fn artwork_cancelable(
        &self,
        game: &LibraryGame,
        cancel: &AtomicBool,
    ) -> Result<artwork::Artwork, String> {
        self.ready()?;
        if self.fixture.is_some() && std::env::var_os("RAESIR_ARTWORK_NETWORK_FIXTURE").is_none() {
            return game
                .cover
                .as_ref()
                .filter(|p| Path::new(p).is_file())
                .map(|path| artwork::Artwork {
                    path: path.clone(),
                    title: None,
                })
                .ok_or("No fixture cover".into());
        }
        artwork::fetch_cancelable(game, &self.cache.join("artwork"), cancel)
    }

    pub fn support(&self, project: &str) -> Result<(), String> {
        self.ready()?;
        if self.fixture.is_some() {
            return Err("Project links are disabled in the fixture profile".into());
        }
        actions::support(project)
    }

    pub fn cached_details(&self, game: &LibraryGame) -> Option<raesir_backend::details::Details> {
        raesir_backend::details::cached(game, &self.cache.join("details"))
    }

    pub fn player_stats(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> raesir_backend::player_stats::PlayerStats {
        if self.fixture.is_some() {
            return std::fs::read(self.data.join("player-stats.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| serde_json::from_value(v.get(&game.key)?.clone()).ok())
                .unwrap_or_default();
        }
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(report) = report else {
            return Default::default();
        };
        if refresh {
            raesir_backend::player_stats::refresh(game, &report, &self.cache.join("player-stats"))
        } else {
            raesir_backend::player_stats::cached(game, &report, &self.cache.join("player-stats"))
        }
    }

    pub fn details(&self, game: &LibraryGame) -> Result<raesir_backend::details::Details, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return self
                .cached_details(game)
                .ok_or("No details in this diagnostic profile".into());
        }
        raesir_backend::details::fetch(game, &self.cache.join("details"))
    }

    pub fn detail_media(&self, url: &str) -> Result<String, String> {
        self.ready()?;
        let mut game = raesir_backend::catalog::entry("media", url, "", vec![], "media");
        game.cover = Some(url.into());
        if self.fixture.is_some() {
            if let Some(art) = artwork::cached(&game, &self.cache.join("artwork")) {
                return Ok(art.path);
            }
            return Path::new(url)
                .is_file()
                .then(|| url.to_owned())
                .ok_or("No local diagnostic image".into());
        }
        artwork::fetch(&game, &self.cache.join("artwork")).map(|art| art.path)
    }

    pub fn cached_achievement_icon(&self, url: &str) -> Option<Arc<gpui::RenderImage>> {
        self.achievement_icons
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(url)
            .cloned()
    }

    pub fn warm_achievement_icons(&self, stats: &raesir_backend::player_stats::PlayerStats) {
        let order = stats.highlights().into_iter().chain(
            stats
                .by_progress()
                .into_iter()
                .filter(|&i| stats.achievements[i].concealed()),
        );
        for index in order.take(512) {
            let _ = self.achievement_icon(&stats.achievements[index].icon, false);
        }
    }

    pub fn achievement_icon(
        &self,
        url: &str,
        download: bool,
    ) -> Result<Arc<gpui::RenderImage>, String> {
        if let Some(image) = self.cached_achievement_icon(url) {
            return Ok(image);
        }
        if url.is_empty() {
            return Err("No achievement icon".into());
        }
        let mut game = raesir_backend::catalog::entry("media", url, "", vec![], "media");
        game.cover = Some(url.into());
        let path = if Path::new(url).is_file() {
            url.to_owned()
        } else if let Some(art) = artwork::cached(&game, &self.cache.join("artwork")) {
            art.path
        } else if download {
            self.detail_media(url)?
        } else {
            return Err("Icon is not cached".into());
        };
        let mut pixels = image_codec::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?
            .thumbnail(128, 128)
            .into_rgba8();
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let image = Arc::new(gpui::RenderImage::new(vec![image_codec::Frame::new(
            pixels,
        )]));
        let mut icons = self
            .achievement_icons
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if icons.len() >= 512 {
            if let Some(key) = icons.keys().next().cloned() {
                icons.remove(&key);
            }
        }
        icons.insert(url.to_owned(), image.clone());
        Ok(image)
    }

    pub fn detail_animation(&self, url: &str) -> Result<String, String> {
        self.ready()?;
        raesir_backend::details::media::animation(
            url,
            &self.cache.join("animations"),
            self.fixture.is_none(),
        )
        .map(|p| p.to_string_lossy().into_owned())
    }

    fn ui(&self) -> &Mutex<serde_json::Map<String, serde_json::Value>> {
        self.ui.get_or_init(|| {
            Mutex::new(
                std::fs::read(self.data.join("native-ui.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default(),
            )
        })
    }

    pub fn zoom(&self) -> f32 {
        self.ui()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("zoom")
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .map(|v| (v as f32).clamp(12., 22.))
            .unwrap_or(16.)
    }

    pub fn set_zoom(&self, zoom: f32) {
        self.ui()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("zoom".into(), serde_json::json!(zoom));
    }

    pub fn filters(&self) -> (u8, u8, Option<usize>, u8) {
        let ui = self.ui().lock().unwrap_or_else(|e| e.into_inner());
        let filters = &ui.get("filters").cloned().unwrap_or_default();
        let selection = |key: &str, default| {
            filters[key]
                .as_u64()
                .filter(|v| *v <= 2)
                .map(|v| v as u8)
                .unwrap_or(default)
        };
        let store = filters["store"]
            .as_str()
            .and_then(|name| crate::STORES.iter().position(|store| *store == name));
        (
            selection("status", 1),
            selection("os", 0),
            store,
            selection("access", 1),
        )
    }

    pub fn set_filters(&self, status: u8, os: u8, store: Option<usize>, access: u8) {
        self.ui().lock().unwrap_or_else(|e| e.into_inner()).insert(
            "filters".into(),
            serde_json::json!({"status": status, "os": os, "store": store.and_then(|i| crate::STORES.get(i)), "access": access}),
        );
    }

    pub fn save_ui(&self) {
        let _save = self.ui_save.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ui) = self.ui.get() else { return };
        let snapshot = ui.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = storage::write_json(&self.data.join("native-ui.json"), &snapshot);
    }

    pub fn begin_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.steam.closing.store(true, Ordering::SeqCst);
        self.mounts.begin_closing();
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let _guard = self.cleanup.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.begin_closing();
        self.save_ui();
        self.epic.shutdown();
        let steam = self.steam.stop();
        let mounts = self.mounts.shutdown();
        let errors: Vec<_> = [steam, mounts]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        if errors.is_empty() {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            self.closing.store(false, Ordering::SeqCst);
            self.steam.closing.store(false, Ordering::SeqCst);
            self.epic.resume();
            self.mounts.resume();
            Err(errors.join("\n"))
        }
    }
}

fn mod_test_args() -> Result<Vec<String>, String> {
    // The fixture passes a JSON argv array, avoiding another shell parser.
    std::env::var("RAESIR_MODS_TEST_ARGS").map_or_else(
        |_| Ok(Vec::new()),
        |s| serde_json::from_str(&s).map_err(|e| e.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_download_progress_wins_over_a_stale_queued_manifest_and_waits_for_commit() {
        let queued = Progress {
            phase: Phase::Queued,
            ..Progress::starting(Operation::Install)
        };
        let live = Progress {
            phase: Phase::Downloading,
            percent: Some(45),
            ..queued.clone()
        };
        assert_eq!(
            merge_download_progress(Some(live.clone()), queued.clone()),
            live
        );
        let complete = Progress {
            phase: Phase::Complete,
            percent: None,
            ..queued.clone()
        };
        let committing = merge_download_progress(Some(complete.clone()), queued.clone());
        assert_eq!(committing.phase, Phase::Installing);
        assert_eq!(committing.percent, Some(100));
        assert_eq!(
            merge_download_progress(Some(complete.clone()), complete.clone()).phase,
            Phase::Complete
        );
        let lost_drive = Progress {
            phase: Phase::Failed,
            error: Some(steam_control::Error::Disk),
            ..queued
        };
        assert_eq!(
            merge_download_progress(Some(complete), lost_drive.clone()),
            lost_drive
        );
    }

    fn fixture(dir: &Path) -> Backend {
        let mut backend = Arc::try_unwrap(Backend::new()).ok().unwrap();
        backend.data = dir.join("data");
        backend.cache = dir.join("cache");
        backend.mod_manager = raesir_backend::mods::Manager::new(&backend.data, &backend.cache);
        backend.downloads = raesir_backend::downloads::Manager::new(&backend.data);
        backend.client_downloads = raesir_backend::client_downloads::Manager::new(&backend.data);
        let path = dir.join("library.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "host_os": "linux", "games": [], "libraries": [], "volumes": [],
                "warnings": [], "owned_account": null, "catalog": []
            }))
            .unwrap(),
        )
        .unwrap();
        backend.fixture = Some(path);
        backend
    }

    #[test]
    fn fixture_refresh_is_read_only_and_cannot_launch_clients_or_mount() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        assert!(backend.cached().unwrap().is_some());
        assert!(backend.scan().unwrap().catalog.is_empty());
        assert!(backend.ownership().unwrap().is_none());
        assert!(backend.epic_profiles().is_empty());
        assert!(
            backend
                .action("steam:1", "launch")
                .unwrap_err()
                .contains("fixture")
        );
        assert!(backend.mount("fixture").unwrap_err().contains("fixture"));
        backend.shutdown().unwrap();
        backend.shutdown().unwrap();
        assert!(backend.scan().is_err());
        assert!(!backend.data.exists());
    }

    #[test]
    fn text_zoom_round_trips_and_invalid_settings_keep_a_readable_default() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        backend.set_zoom(19.);
        backend.save_ui();
        assert_eq!(fixture(dir.path()).zoom(), 19.);
        backend.set_zoom(999.);
        backend.save_ui();
        assert_eq!(fixture(dir.path()).zoom(), 22.);
        std::fs::write(backend.data.join("native-ui.json"), b"bad json").unwrap();
        assert_eq!(fixture(dir.path()).zoom(), 16.);
    }

    #[test]
    fn automatic_game_updates_persist_without_replacing_other_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        assert!(backend.automatic_game_updates());
        backend.set_zoom(19.);
        backend.set_automatic_game_updates(false);
        backend.save_ui();
        let reopened = fixture(dir.path());
        assert!(!reopened.automatic_game_updates());
        assert_eq!(reopened.zoom(), 19.);
        reopened.set_automatic_game_updates(true);
        reopened.save_ui();
        assert!(fixture(dir.path()).automatic_game_updates());
    }

    #[test]
    fn filters_restore_by_store_name_and_share_settings_with_zoom() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        assert_eq!(backend.filters(), (1, 0, None, 1));
        backend.set_zoom(19.);
        backend.set_filters(2, 1, Some(3), 2);
        backend.save_ui();
        let restored = fixture(dir.path());
        assert_eq!(restored.filters(), (2, 1, Some(3), 2));
        assert_eq!(restored.zoom(), 19.);
        restored.set_zoom(20.);
        restored.save_ui();
        assert_eq!(fixture(dir.path()).filters(), (2, 1, Some(3), 2));
        restored.set_filters(0, 2, None, 0);
        restored.save_ui();
        assert_eq!(fixture(dir.path()).filters(), (0, 2, None, 0));
        assert_eq!(fixture(dir.path()).zoom(), 20.);
        std::fs::write(
            backend.data.join("native-ui.json"),
            br#"{"filters":{"status":99,"os":-1,"store":"unknown"}}"#,
        )
        .unwrap();
        assert_eq!(fixture(dir.path()).filters(), (1, 0, None, 1));
        std::fs::write(
            backend.data.join("native-ui.json"),
            br#"{"filters":{"status":0,"os":2,"store":"ubisoft"}}"#,
        )
        .unwrap();
        assert_eq!(fixture(dir.path()).filters(), (0, 2, Some(3), 1));
    }

    #[test]
    fn cached_achievement_icons_are_ready_before_publishing_stats_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        let path = dir.path().join("achievement.png");
        image_codec::RgbaImage::from_pixel(64, 64, image_codec::Rgba([40, 80, 120, 255]))
            .save(&path)
            .unwrap();
        let url = path.to_string_lossy().into_owned();
        let stats = raesir_backend::player_stats::PlayerStats {
            achievements: vec![raesir_backend::player_stats::Achievement {
                icon: url.clone(),
                ..Default::default()
            }],
            ..Default::default()
        };
        backend.warm_achievement_icons(&stats);
        let first = backend.cached_achievement_icon(&url).unwrap();
        std::fs::remove_file(path).unwrap();
        let second = backend.achievement_icon(&url, false).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
    #[test]
    fn existing_artwork_cache_with_img_extension_decodes_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        let path = dir.path().join("existing-cache.img");
        image_codec::RgbImage::from_pixel(8, 12, image_codec::Rgb([40, 80, 120]))
            .save_with_format(&path, image_codec::ImageFormat::Jpeg)
            .unwrap();
        let mut game =
            raesir_backend::catalog::entry("steam", "1", "Cached game", vec![], "fixture");
        game.cover = Some(path.to_string_lossy().into_owned());
        let ready = crate::artwork::load(&backend, &game).unwrap();
        let image = &ready.poster;
        assert_eq!(image.frame_count(), 1);
        assert!(!image.as_bytes(0).unwrap().is_empty());
        let again = crate::artwork::load(&backend, &game).unwrap();
        assert_eq!(ready.card.as_bytes(0), again.card.as_bytes(0));
    }
}
