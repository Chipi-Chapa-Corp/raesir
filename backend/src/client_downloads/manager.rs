use super::{
    Client,
    context::{self, Context},
    transport,
};
use crate::{
    model::{LibraryGame, Report},
    steam_control::{Operation, Phase, Progress},
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    root: String,
    id: String,
    progress: Progress,
}

#[derive(Default)]
struct State {
    contexts: Vec<Context>,
    discovered: Option<Instant>,
    polled: Option<Instant>,
    leases: HashMap<String, Vec<std::fs::File>>,
    waiting: HashMap<String, Instant>,
    checked: HashMap<String, Instant>,
    automatic: Option<Instant>,
}

pub struct Manager {
    file: std::path::PathBuf,
    operation: Mutex<State>,
    jobs: RwLock<HashMap<String, Job>>,
    observing: AtomicBool,
}

impl Manager {
    pub fn new(data: &Path) -> Self {
        let file = data.join("native-client-downloads.json");
        let mut jobs: HashMap<String, Job> = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        jobs.retain(|_, job| job.progress.phase != Phase::Failed);
        for job in jobs.values_mut() {
            job.progress.controllable = false;
            if job.progress.phase == Phase::Starting && job.progress.operation == Operation::Update
            {
                job.progress.phase = Phase::Available;
            }
        }
        Self {
            file,
            operation: Mutex::new(State::default()),
            jobs: RwLock::new(jobs),
            observing: AtomicBool::new(false),
        }
    }

    pub fn observing(&self) -> bool {
        self.observing.load(Ordering::Relaxed)
    }

    pub fn automatic(&self, report: &Report) {
        {
            let Ok(mut state) = self.operation.try_lock() else {
                return;
            };
            if state
                .automatic
                .is_some_and(|at| at.elapsed() < Duration::from_secs(15))
            {
                return;
            }
            state.automatic = Some(Instant::now());
        }
        for (key, progress) in self.snapshot() {
            if progress.operation == Operation::Update
                && progress.phase == Phase::Available
                && progress.controllable
            {
                let _ = self.control(report, &key, true);
            }
        }
    }

    pub fn snapshot(&self) -> HashMap<String, Progress> {
        self.jobs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, j)| (k.clone(), j.progress.clone()))
            .collect()
    }

    fn save(&self) {
        let jobs = self.jobs.read().unwrap_or_else(|e| e.into_inner());
        let active: HashMap<_, _> = jobs
            .iter()
            .filter(|(_, job)| job.progress.phase != Phase::Failed)
            .collect();
        let _ = storage::write_json(&self.file, &active);
    }

    pub fn dismiss_failed(&self, seen: &HashMap<String, Progress>) {
        self.jobs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key, job| {
                job.progress.phase != Phase::Failed || seen.get(key) != Some(&job.progress)
            });
    }

    fn discover(state: &mut State, report: &Report) {
        if state
            .discovered
            .is_none_or(|last| last.elapsed() > Duration::from_secs(60))
        {
            state.contexts = context::contexts(report);
            state.discovered = Some(Instant::now());
        }
    }

    /// Polls every live client, including downloads started outside Raesir.
    /// No filesystem scanning or IPC happens when the UI reads snapshot().
    pub fn refresh(&self, report: &Report, updates: bool) -> Vec<String> {
        let Ok(mut state) = self.operation.try_lock() else {
            return vec![];
        };
        if !updates
            && state
                .polled
                .is_some_and(|last| last.elapsed() < Duration::from_secs(2))
        {
            return vec![];
        }
        Self::discover(&mut state, report);
        self.observing
            .store(!state.contexts.is_empty(), Ordering::Relaxed);
        state.polled = Some(Instant::now());
        let mut errors = vec![];
        for context in state.contexts.clone() {
            let root = crate::epic::key(&context.root);
            if !context.verified(port(context.client)) {
                state.checked.remove(&root);
                for job in self
                    .jobs
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .values_mut()
                    .filter(|j| j.root == root)
                {
                    job.progress.controllable = false;
                }
                continue;
            }
            let check = updates || !state.checked.contains_key(&root);
            match self.request(&context, "snapshot", "", check) {
                Ok(value) => {
                    self.merge(report, &context, &value);
                    state.checked.insert(root.clone(), Instant::now());
                }
                Err(error) => {
                    for job in self
                        .jobs
                        .write()
                        .unwrap_or_else(|e| e.into_inner())
                        .values_mut()
                        .filter(|j| j.root == root)
                    {
                        job.progress.controllable = false;
                    }
                    if updates {
                        errors.push(format!("{}: {error}", context.client.name()));
                    }
                }
            }
        }
        for (key, job) in self.jobs.read().unwrap_or_else(|e| e.into_inner()).iter() {
            if job.progress.active()
                && !state.leases.contains_key(key)
                && let Ok(leases) = update_leases(report, key)
            {
                state.leases.insert(key.clone(), leases);
            }
        }
        // Retain game/mod exclusion while a client owns an active transfer.
        let jobs = self.jobs.read().unwrap_or_else(|e| e.into_inner());
        state
            .leases
            .retain(|key, _| jobs.get(key).is_some_and(|j| j.progress.active()));
        let expired: Vec<_> = state
            .waiting
            .iter()
            .filter(|(key, at)| {
                at.elapsed() >= Duration::from_secs(120)
                    && jobs
                        .get(*key)
                        .is_some_and(|j| j.progress.phase == Phase::Starting)
            })
            .map(|(key, _)| key.clone())
            .collect();
        state.waiting.retain(|key, _| {
            jobs.get(key)
                .is_some_and(|j| j.progress.phase == Phase::Starting)
                && !expired.contains(key)
        });
        drop(jobs);
        for key in expired {
            state.leases.remove(&key);
            if let Some(job) = self
                .jobs
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&key)
            {
                job.progress.phase = Phase::Failed;
                job.progress.detail=Some("The client did not queue this update. Check it in the game client, then retry.".into());
            }
        }
        self.save();
        errors
    }

    /// Open a client the user selected, enabling its download interface when needed.
    pub fn open_library(&self, report: &Report, game: &LibraryGame) -> Result<bool, String> {
        if !matches!(game.store.as_str(), "ea" | "ubisoft") {
            return Ok(false);
        }
        let mut state = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        Self::discover(&mut state, report);
        let mut candidates = state
            .contexts
            .iter()
            .filter(|context| context.owns(game, report));
        let Some(context) = candidates.next() else {
            return Ok(false);
        };
        if candidates.next().is_some() || context.verified(port(context.client)) {
            return Ok(false);
        }
        context.open_library(game)?;
        let root = crate::epic::key(&context.root);
        state.checked.remove(&root);
        state.polled = None;
        self.observing.store(true, Ordering::Relaxed);
        Ok(true)
    }

    pub fn control(&self, report: &Report, key: &str, resume: bool) -> Result<bool, String> {
        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
            return Err("Client actions are disabled".into());
        }
        let mut state = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = self
            .jobs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
        else {
            return Ok(false);
        };
        Self::discover(&mut state, report);
        let context = state
            .contexts
            .iter()
            .find(|c| {
                crate::epic::key(&c.root) == job.root
                    && context::game(report, c, &job.id).is_some_and(|g| g.key == key)
            })
            .ok_or("This download's client profile is unavailable")?;
        let action = if resume
            && job.progress.operation == Operation::Update
            && matches!(job.progress.phase, Phase::Available | Phase::Failed)
        {
            "update"
        } else if resume {
            "resume"
        } else {
            "pause"
        };
        let leases = if action == "update" && !state.leases.contains_key(key) {
            Some(update_leases(report, key)?)
        } else {
            None
        };
        let value = self.request(context, action, &job.id, false)?;
        self.merge(report, context, &value);
        if let Some(leases) = leases {
            state.leases.insert(key.into(), leases);
        }
        if action == "update" {
            state.waiting.insert(key.into(), Instant::now());
            let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = jobs.get_mut(key)
                && job.progress.phase == Phase::Available
            {
                job.progress.phase = Phase::Starting;
                job.progress.controllable = false;
            }
        }
        self.save();
        Ok(true)
    }

    fn merge(&self, report: &Report, context: &Context, value: &Value) {
        let root = crate::epic::key(&context.root);
        let mut jobs = self.jobs.write().unwrap_or_else(|e| e.into_inner());
        if let Some(patches) = value["patches"].as_array() {
            for id in patches.iter().filter_map(Value::as_str) {
                let Some(game) = context::game(report, context, id) else {
                    continue;
                };
                if !crate::catalog::local(&game.installations) {
                    continue;
                }
                let mut progress = Progress::starting(Operation::Update);
                progress.phase = Phase::Available;
                jobs.entry(game.key.clone())
                    .and_modify(|job| {
                        if !job.progress.active() {
                            job.progress = progress.clone();
                        }
                    })
                    .or_insert(Job {
                        root: root.clone(),
                        id: id.into(),
                        progress,
                    });
            }
        }
        for row in value["downloads"].as_array().into_iter().flatten() {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            let Some(game) = context::game(report, context, id) else {
                continue;
            };
            let operation = jobs
                .get(&game.key)
                .map(|j| j.progress.operation)
                .unwrap_or_else(|| {
                    if crate::catalog::local(&game.installations) {
                        Operation::Update
                    } else {
                        Operation::Install
                    }
                });
            let progress = progress(
                context.client,
                row,
                operation,
                value["paused"].as_bool().unwrap_or(false),
            )
            .unwrap_or_else(|| {
                let mut progress = Progress::starting(operation);
                progress.phase = Phase::Queued;
                progress.controllable = false;
                progress.detail = Some(format!(
                    "Open {} to manage this transfer",
                    context.client.name()
                ));
                progress
            });
            {
                jobs.insert(
                    game.key.clone(),
                    Job {
                        root: root.clone(),
                        id: id.into(),
                        progress,
                    },
                );
            }
        }
        for id in value["completed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(game) = context::game(report, context, id)
                && let Some(job) = jobs.get_mut(&game.key)
                && crate::catalog::local(&game.installations)
                && !value["downloads"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|row| row["id"].as_str() == Some(id) && row["state"].as_u64() != Some(7))
            {
                job.progress.phase = Phase::Complete;
                job.progress.percent = Some(100);
                job.progress.controllable = false;
                job.progress.detail = None;
            }
        }
    }

    fn request(
        &self,
        context: &Context,
        action: &str,
        id: &str,
        updates: bool,
    ) -> Result<Value, String> {
        let root = crate::epic::key(&context.root);
        let watching: Vec<_> = self
            .jobs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|j| j.root == root && j.progress.active())
            .map(|j| j.id.clone())
            .collect();
        request(context, action, id, updates, &watching)
    }

    pub fn finish(&self) {
        self.jobs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, j| j.progress.phase != Phase::Complete);
        self.save();
    }
}

fn port(client: Client) -> u16 {
    match client {
        Client::Ubisoft => 9222,
        Client::Ea => 9394,
    }
}

fn request(
    context: &Context,
    action: &str,
    id: &str,
    updates: bool,
    watching: &[String],
) -> Result<Value, String> {
    let port = port(context.client);
    if !context.verified(port) {
        return Err(format!(
            "{} download integration is unavailable. Quit the client once, then open it through Raesir.",
            context.client.name()
        ));
    }
    let (script, allowed): (String, fn(&str) -> bool) = match context.client {
        Client::Ubisoft => (
            format!(
                "{}\nraesirUbisoft({})",
                include_str!("ubisoft.js"),
                json!({"action":action,"id":id,"checkUpdates":updates,"watching":watching})
            ),
            |url| url == "uplay://localhost/ui/v2/view/popup/main.html",
        ),
        Client::Ea => (
            format!(
                "{}\nraesirEa({})",
                include_str!("ea.js"),
                json!({"action":action,"id":id,"checkUpdates":updates,"watching":watching})
            ),
            |url| {
                reqwest::Url::parse(url).is_ok_and(|u| {
                    u.scheme() == "https"
                        && u.host_str() == Some("pc.ea.com")
                        && u.port_or_known_default() == Some(443)
                })
            },
        ),
    };
    transport::cdp(port, allowed)?.evaluate(&script)
}

fn progress(client: Client, row: &Value, operation: Operation, paused: bool) -> Option<Progress> {
    let (phase, controllable) = match client {
        Client::Ubisoft => {
            let active = row["active"].as_bool()?;
            let state = row["state"].as_u64()?;
            let phase = match state {
                0 | 1 => {
                    if active {
                        Phase::Starting
                    } else {
                        Phase::Queued
                    }
                }
                2 => {
                    if !active {
                        Phase::Queued
                    } else if paused {
                        Phase::Paused
                    } else {
                        Phase::Downloading
                    }
                }
                3 => Phase::Paused,
                4..=6 => Phase::Failed,
                7 => Phase::Installing,
                8 => Phase::Installing,
                _ => return None,
            };
            (phase, active && matches!(state, 2 | 3))
        }
        Client::Ea => {
            let state = row["state"].as_str()?;
            let phase = match state {
                "QUEUED" => Phase::Queued,
                "PAUSED" => Phase::Paused,
                "ACTIVE" => match row["phase"].as_str() {
                    Some("DOWNLOAD") => Phase::Downloading,
                    Some("PREPARE" | "SCANNING") => Phase::Starting,
                    Some("VERIFY" | "FINALIZE") => Phase::Installing,
                    _ => Phase::Installing,
                },
                "INSTALLED" => Phase::Installing,
                "CANCELING" | "NOT_INSTALLED" => Phase::Failed,
                _ => return None,
            };
            (
                phase,
                row["pausable"].as_bool().unwrap_or(false) || phase == Phase::Paused,
            )
        }
    };
    let (percent, downloaded_bytes) = match (row["downloaded"].as_u64(), row["total"].as_u64()) {
        (Some(done), Some(total)) if total > 0 && done <= total => (
            Some(((u128::from(done) * 100) / u128::from(total)) as u8),
            (phase == Phase::Downloading).then_some(done),
        ),
        _ => (None, None),
    };
    Some(Progress {
        operation,
        phase,
        percent,
        error: None,
        controllable,
        detail: None,
        downloaded_bytes,
        scheduled_at: None,
    })
}

fn update_leases(report: &Report, key: &str) -> Result<Vec<std::fs::File>, String> {
    let game = report
        .catalog
        .iter()
        .find(|g| g.key == key)
        .ok_or("This game is no longer in the library")?;
    let mut leases = vec![];
    for install in game
        .installations
        .iter()
        .filter(|i| i.os == crate::model::Os::host() && Path::new(&i.path).is_dir())
    {
        let dir = Path::new(&install.path);
        if crate::mods::download_game_running(dir) {
            return Err("Close this game before updating it".into());
        }
        let path = crate::mods::download_lock_path(dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|_| "This game's mods or installation are busy; update later")?;
        leases.push(file);
    }
    Ok(leases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog, model::*};

    fn fixture(store: &str) -> (tempfile::TempDir, Manager, Report, Context) {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(dir.path());
        let mut report:Report=serde_json::from_value(json!({"host_os":Os::host(),"games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null})).unwrap();
        let payload = dir.path().join("game");
        std::fs::create_dir_all(&payload).unwrap();
        let mut game = catalog::entry(
            store,
            "42",
            "Same title",
            vec![catalog::install(
                "fixture",
                payload.to_string_lossy().into(),
                Os::host(),
                true,
            )],
            "fixture",
        );
        game.in_library = true;
        report.catalog.push(game);
        let context = Context {
            client: if store == "ea" {
                Client::Ea
            } else {
                Client::Ubisoft
            },
            root: dir.path().join("client"),
            prefix: None,
        };
        (dir, manager, report, context)
    }

    #[cfg(unix)]
    #[test]
    fn update_checks_and_requests_leave_a_closed_client_closed() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, manager, report, context) = fixture("ea");
        std::fs::create_dir(&context.root).unwrap();
        let exe = context.root.join(context.client.executable());
        let marker = context.root.join("started");
        let marker_path = shlex::try_quote(marker.to_str().unwrap()).unwrap();
        std::fs::write(&exe, format!("#!/bin/sh\n: > {marker_path}\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        manager.merge(&report, &context, &json!({"patches": ["42"]}));
        {
            let mut state = manager.operation.lock().unwrap();
            state.contexts.push(context);
            state.discovered = Some(Instant::now());
        }

        assert!(manager.refresh(&report, true).is_empty());
        let progress = &manager.snapshot()["ea:42"];
        assert_eq!(progress.phase, Phase::Available);
        assert!(!progress.controllable);
        manager.automatic(&report);
        assert!(manager.control(&report, "ea:42", true).is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn native_client_queue_never_matches_games_by_title_or_completes_from_missing_rows() {
        let (_dir, manager, report, context) = fixture("ubisoft");
        manager.merge(
            &report,
            &context,
            &json!({"patches":["42","other"],"downloads":[]}),
        );
        assert_eq!(manager.snapshot()["ubisoft:42"].phase, Phase::Available);
        manager.merge(
            &report,
            &context,
            &json!({"downloads":[{"id":"42","state":2,"active":true,"downloaded":1,"total":4}]}),
        );
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot["ubisoft:42"].phase, Phase::Downloading);
        assert_eq!(snapshot["ubisoft:42"].percent, Some(25));
        manager.merge(&report, &context, &json!({"downloads":[]}));
        assert_eq!(manager.snapshot()["ubisoft:42"].phase, Phase::Downloading);
        manager.merge(
            &report,
            &context,
            &json!({"downloads":[],"completed":["42"]}),
        );
        assert_eq!(manager.snapshot()["ubisoft:42"].phase, Phase::Complete);
    }

    #[test]
    fn stale_completion_and_missing_payload_cannot_finish_a_new_transfer() {
        let (_dir, manager, mut report, context) = fixture("ubisoft");
        let snapshot =
            json!({"downloads":[{"id":"42","state":2,"active":true}],"completed":["42"]});
        manager.merge(&report, &context, &snapshot);
        assert_eq!(manager.snapshot()["ubisoft:42"].phase, Phase::Downloading);
        report.catalog[0].installations.clear();
        manager.merge(
            &report,
            &context,
            &json!({"downloads":[],"completed":["42"]}),
        );
        assert_ne!(manager.snapshot()["ubisoft:42"].phase, Phase::Complete);
    }

    #[test]
    fn ea_offer_aliases_must_match_one_game_in_the_selected_profile() {
        let (_dir, manager, mut report, context) = fixture("ea");
        report.ea.games.push(EaGame {
            id: "42".into(),
            aliases: vec!["offer:42".into()],
            title: "Same title".into(),
            library_sources: vec![],
            installations: vec![],
        });
        manager.merge(&report, &context, &json!({"patches":["offer:42"]}));
        assert_eq!(manager.snapshot()["ea:42"].phase, Phase::Available);
        let mut other = report.catalog[0].clone();
        other.id = "43".into();
        other.key = "ea:43".into();
        report.catalog.push(other);
        report.ea.games.push(EaGame {
            id: "43".into(),
            aliases: vec!["offer:42".into()],
            title: "Same title".into(),
            library_sources: vec![],
            installations: vec![],
        });
        assert!(context::game(&report, &context, "offer:42").is_none());
        assert!(context::game(&report, &context, "Same title").is_none());
    }

    #[test]
    fn native_states_and_byte_counters_do_not_invent_pause_or_completion() {
        let active = json!({"state":2,"active":true,"downloaded":u64::MAX,"total":u64::MAX});
        let p = progress(Client::Ubisoft, &active, Operation::Update, false).unwrap();
        assert_eq!(p.percent, Some(100));
        assert_eq!(p.phase, Phase::Downloading);
        let queued = json!({"state":0,"active":false,"downloaded":5,"total":0});
        let p = progress(Client::Ubisoft, &queued, Operation::Install, false).unwrap();
        assert_eq!(p.phase, Phase::Queued);
        assert!(!p.controllable);
        assert_eq!(p.percent, None);
        let allocation = json!({"state":8,"active":true});
        assert!(
            !progress(Client::Ubisoft, &allocation, Operation::Install, false)
                .unwrap()
                .controllable
        );
        let ea = json!({"state":"ACTIVE","phase":"DOWNLOAD","pausable":true,"downloaded":99,"total":100});
        let p = progress(Client::Ea, &ea, Operation::Update, false).unwrap();
        assert_eq!(p.phase, Phase::Downloading);
        assert!(p.controllable);
        assert_eq!(p.percent, Some(99));
        assert!(
            progress(
                Client::Ea,
                &json!({"state":"future"}),
                Operation::Update,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn native_updates_share_exclusion_with_game_and_mod_operations() {
        let (_dir, _manager, report, _context) = fixture("ea");
        let first = update_leases(&report, "ea:42").unwrap();
        assert_eq!(first.len(), 1);
        assert!(update_leases(&report, "ea:42").is_err());
        drop(first);
        assert!(update_leases(&report, "ea:42").is_ok());
    }

    #[test]
    fn restarted_launcher_preserves_the_queue_but_requires_fresh_control_evidence() {
        let (dir, manager, report, context) = fixture("ubisoft");
        manager.merge(
            &report,
            &context,
            &json!({"downloads":[{"id":"42","state":2,"active":true}]}),
        );
        manager.save();
        assert!(manager.snapshot()["ubisoft:42"].controllable);
        let adopted = Manager::new(dir.path());
        assert_eq!(adopted.snapshot()["ubisoft:42"].phase, Phase::Downloading);
        assert!(!adopted.snapshot()["ubisoft:42"].controllable);
    }
}
