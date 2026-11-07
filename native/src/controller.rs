use crate::{Launcher, library::Game};
use gpui::*;
use raesir_backend::{catalog, model::Report};
use std::{collections::HashMap, sync::Arc};

impl Launcher {
    pub fn start(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let cached = cx
                .background_executor()
                .spawn(async move { backend.cached() })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(Some(report)) = cached {
                    this.apply_report(report, cx);
                }
                this.start_refresh(false, cx);
            })
            .ok();
        }));
    }

    pub(crate) fn apply_report(&mut self, report: Arc<Report>, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let top = self.rows.logical_scroll_top();
        let anchor = self
            .visible
            .get(top.item_ix.saturating_sub(1) * self.columns)
            .and_then(|i| self.games.get(*i))
            .map(|g| g.source.key.clone());
        let old: HashMap<_, _> = std::mem::take(&mut self.games)
            .into_iter()
            .map(|g| (g.source.key.clone(), g))
            .collect();
        self.host = report.host_os;
        self.games = report
            .catalog
            .iter()
            .filter(|g| catalog::visible(g))
            .cloned()
            .filter_map(Game::new)
            .map(|mut game| {
                if let Some(previous) = old.get(&game.source.key) {
                    if previous.source.cover == game.source.cover {
                        game.poster = previous.poster.clone();
                        game.card_poster = previous.card_poster.clone();
                        game.background = previous.background.clone();
                        game.background_checked = previous.background_checked;
                    }
                    if game.title == game.source.id && previous.title != previous.source.id {
                        game.title = previous.title.clone();
                    }
                }
                game
            })
            .collect();
        self.report = Some(report);
        self.start_mod_updates(cx);
        self.start_game_updates(cx);
        self.download_operations = self.backend.current_download_progress();
        self.update_download_details(cx);
        self.watch_downloads(cx);
        self.starting = false;
        self.load_error = None;
        self.game_menu = None;
        self.artwork_attempted.clear();
        self.filter(cx);
        if top.item_ix > 0
            && let Some(anchor) = anchor
            && let Some(index) = self
                .visible
                .iter()
                .position(|i| self.games[*i].source.key == anchor)
        {
            self.rows.scroll_to(ListOffset {
                item_ix: 1 + index / self.columns,
                offset_in_item: top.offset_in_item,
            });
            self.wheel_needs_sync = true;
        }
    }

    pub fn sync(&mut self, cx: &mut Context<Self>) {
        self.start_refresh(true, cx);
        self.check_game_updates(true, cx);
    }

    pub fn support_project(&mut self, project: &'static str, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.support(project) })
                .await;
            if let Err(error) = result {
                this.update(cx, |this, cx| this.notice(error, cx)).ok();
            }
        })
        .detach();
    }

    pub fn refresh_on_focus(&mut self, cx: &mut Context<Self>) {
        if self.last_refresh.elapsed().as_secs() > 90 {
            self.start_refresh(false, cx);
        }
    }

    fn start_refresh(&mut self, full: bool, cx: &mut Context<Self>) {
        if self.syncing || self.closing {
            return;
        }
        self.syncing = true;
        let backend = self.backend.clone();
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            let scan = backend.clone();
            let result = cx
                .background_executor()
                .spawn(async move { scan.scan() })
                .await;
            let mut error = None;
            match result {
                Ok(report) => {
                    if this
                        .update(cx, |this, cx| this.apply_report(report, cx))
                        .is_err()
                    {
                        return;
                    }
                    let pass = backend.clone();
                    match cx
                        .background_executor()
                        .spawn(async move { pass.refresh_game_pass(full) })
                        .await
                    {
                        Ok(Some(report)) => {
                            this.update(cx, |this, cx| this.apply_report(report, cx))
                                .ok();
                        }
                        Err(e) => {
                            this.update(cx, |this, cx| this.notice(e, cx)).ok();
                        }
                        _ => {}
                    }
                    let owned = backend.clone();
                    match cx
                        .background_executor()
                        .spawn(async move { owned.ownership() })
                        .await
                    {
                        Ok(Some(report)) => {
                            this.update(cx, |this, cx| this.apply_report(report, cx))
                                .ok();
                        }
                        Err(e) if full => error = Some(e),
                        _ => {}
                    }
                    if full {
                        for profile in backend.epic_profiles() {
                            let epic = backend.clone();
                            match cx
                                .background_executor()
                                .spawn(async move { epic.refresh_epic(&profile) })
                                .await
                            {
                                Ok(report) => {
                                    this.update(cx, |this, cx| this.apply_report(report, cx))
                                        .ok();
                                }
                                Err(e) => error = Some(e),
                            }
                        }
                    }
                }
                Err(e) => error = Some(e),
            }
            this.update(cx, |this, cx| {
                this.syncing = false;
                this.starting = false;
                this.last_refresh = std::time::Instant::now();
                if this.closing {
                    return;
                }
                if let Some(error) = error {
                    if this.report.is_none() {
                        this.load_error = Some(error.clone());
                    }
                    this.notice(error, cx);
                } else if full {
                    this.notice("Library refreshed".into(), cx);
                }
                this.reset_rows_if_empty();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn reset_rows_if_empty(&mut self) {
        if self.games.is_empty() {
            self.reset_rows();
        }
    }

    pub fn mount(&mut self, id: String, cx: &mut Context<Self>) {
        if self.syncing || self.closing {
            return;
        }
        self.syncing = true;
        let backend = self.backend.clone();
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.mount(&id) })
                .await;
            this.update(cx, |this, cx| {
                this.syncing = false;
                match result {
                    Ok(report) => this.apply_report(report, cx),
                    Err(e) => this.notice(e, cx),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn action(&mut self, key: String, kind: &'static str, cx: &mut Context<Self>) {
        if self.closing || self.pending_action.is_some() {
            return;
        }
        if matches!(kind, "launch" | "launch-modded" | "install" | "uninstall")
            && self.download_operations.get(&key).is_some_and(|p| {
                p.blocks_play()
                    && !(kind == "install"
                        && p.phase == raesir_backend::steam_control::Phase::Available)
            })
        {
            return;
        }
        self.game_menu = None;
        if kind == "uninstall" {
            self.confirm = Some(key);
            cx.notify();
            return;
        }
        self.dispatch(key, kind, cx);
    }

    pub fn dispatch(&mut self, key: String, kind: &'static str, cx: &mut Context<Self>) {
        if self.closing || self.pending_action.is_some() {
            return;
        }
        self.pending_action = Some(key.clone());
        let steam_operation = if kind == "update" {
            Some(raesir_backend::steam_control::Operation::Update)
        } else if kind == "install" {
            Some(raesir_backend::steam_control::Operation::Install)
        } else {
            self.games
                .iter()
                .find(|g| g.source.key == key)
                .and_then(|game| {
                    let action = match kind {
                        "install" => game.source.install.as_ref(),
                        "uninstall" => game.source.uninstall.as_ref(),
                        _ => None,
                    }?;
                    if let raesir_backend::model::ActionTarget::Steam { operation, .. } =
                        &action.target
                    {
                        Some(*operation)
                    } else {
                        None
                    }
                })
        };
        if let Some(operation) = steam_operation {
            self.download_operations.insert(
                key.clone(),
                raesir_backend::steam_control::Progress::starting(operation),
            );
            self.update_download_details(cx);
        }
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.action(&key, kind) })
                .await;
            let refresh = result.is_ok() && kind == "uninstall" && steam_operation.is_none();
            let modded = result.is_ok() && kind == "launch-modded";
            this.update(cx, |this, cx| {
                this.pending_action = None;
                if steam_operation.is_some() {
                    this.download_operations = this.backend.current_download_progress();
                    this.update_download_details(cx);
                    this.watch_downloads(cx);
                    if let Err(error) = result {
                        this.notice(error, cx);
                    }
                } else {
                    match result {
                        Err(error) => this.notice(error, cx),
                        Ok(message) if !message.starts_with("Sent to ") => this.notice(message, cx),
                        _ => {}
                    }
                }
                if modded {
                    this.watch_modded(cx);
                }
                cx.notify();
            })
            .ok();
            if refresh {
                cx.background_executor().timer(millis(3500)).await;
                this.update(cx, |this, cx| this.start_refresh(false, cx))
                    .ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn update_download_details(&self, cx: &mut Context<Self>) {
        let progress = &self.download_operations;
        if let Some(page) = &self.details {
            page.update(cx, |page, cx| {
                page.library_changed(&self.games, progress, cx)
            });
        }
    }

    pub(crate) fn check_game_updates(&mut self, force: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let (progress, errors) = cx
                .background_executor()
                .spawn(async move { backend.check_game_updates(force) })
                .await;
            this.update(cx, |this, cx| {
                this.download_operations = progress;
                this.update_download_details(cx);
                this.watch_downloads(cx);
                if let Some(error) = errors.first() {
                    if force || this.game_update_notice.as_ref() != Some(error) {
                        this.notice(error.clone(), cx);
                    }
                    this.game_update_notice = Some(error.clone());
                } else {
                    this.game_update_notice = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_game_updates(&mut self, cx: &mut Context<Self>) {
        if self.game_update_task.is_some() || self.closing {
            return;
        }
        self.game_update_task = Some(cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| {
                        if this.closing {
                            return false;
                        }
                        this.check_game_updates(false, cx);
                        true
                    })
                    .unwrap_or(false)
                    == false
                {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
            }
        }));
    }

    fn start_mod_updates(&mut self, cx: &mut Context<Self>) {
        if self.mod_update_task.is_some()
            || self.closing
            || !self.backend.mod_auto_updates_enabled()
        {
            return;
        }
        let backend = self.backend.clone();
        self.mod_update_task = Some(cx.spawn(async move |this, cx| {
            let mut startup = true;
            loop {
                let worker = backend.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { worker.auto_update_mods(startup) })
                    .await;
                startup = false;
                let keep = this
                    .update(cx, |this, cx| {
                        if this.closing {
                            return false;
                        }
                        match result {
                            Ok(Some((result, report))) => {
                                let changed = !result.updated.is_empty();
                                if let Some(report) = report {
                                    this.apply_report(report, cx);
                                }
                                let issue = result
                                    .issues
                                    .iter()
                                    .find(|i| i.kind != "deferred")
                                    .map(|i| i.message.clone());
                                if changed {
                                    this.notice(
                                        format!("{} mod updates installed", result.updated.len()),
                                        cx,
                                    );
                                } else if let Some(issue) = issue
                                    .as_ref()
                                    .filter(|i| this.mod_update_notice.as_ref() != Some(i))
                                {
                                    this.notice(issue.clone(), cx);
                                }
                                if issue.is_some() {
                                    this.mod_update_notice = issue;
                                }
                                if (changed || !result.issues.is_empty())
                                    && let Some(page) = &this.mods
                                {
                                    page.update(cx, |page, cx| page.refresh_after_updates(cx));
                                }
                            }
                            Err(error) => {
                                if this.mod_update_notice.as_ref() != Some(&error) {
                                    this.notice(format!("Mod updates: {error}"), cx);
                                }
                                this.mod_update_notice = Some(error);
                            }
                            _ => {}
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                // Per-profile timestamps enforce hourly checks. A light timer
                // retries busy profiles soon after the game/proxy exits.
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
            }
        }));
    }

    fn watch_modded(&mut self, cx: &mut Context<Self>) {
        if self.mods_watching || self.closing {
            return;
        }
        self.mods_watching = true;
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let refresh = backend.clone();
            if let Ok(report) = cx
                .background_executor()
                .spawn(async move { refresh.refresh_mods() })
                .await
            {
                this.update(cx, |this, cx| this.apply_report(report, cx))
                    .ok();
            }
            loop {
                cx.background_executor().timer(millis(1000)).await;
                let (active, errors) = backend.mod_launch_progress();
                let keep = this
                    .update(cx, |this, cx| {
                        for error in errors {
                            this.notice(error, cx);
                        }
                        this.mods_watching = active && !this.closing;
                        this.mods_watching
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn watch_downloads(&mut self, cx: &mut Context<Self>) {
        if self.downloads_watching
            || self.closing
            || !self.download_operations.values().any(|p| p.active())
        {
            return;
        }
        self.downloads_watching = true;
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(millis(1000)).await;
                let poll = backend.clone();
                let progress = cx
                    .background_executor()
                    .spawn(async move { poll.download_progress() })
                    .await;
                let complete = progress
                    .values()
                    .any(|p| p.phase == raesir_backend::steam_control::Phase::Complete);
                let stop = this
                    .update(cx, |this, cx| {
                        if this.closing {
                            this.downloads_watching = false;
                            return true;
                        }
                        if this.download_operations != progress {
                            this.download_operations = progress;
                            this.update_download_details(cx);
                            cx.notify();
                        }
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    return;
                }
                if complete {
                    let scan = backend.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { scan.scan() })
                        .await;
                    if let Ok(report) = result {
                        backend.finish_download_jobs();
                        this.update(cx, |this, cx| {
                            this.apply_report(report, cx);
                            cx.notify();
                        })
                        .ok();
                    }
                }
                // A new job can arrive while the completion scan awaits. Decide
                // whether to stop from current UI state, in the same update that
                // clears the flag, so a newly queued job always has a watcher.
                let active = this
                    .update(cx, |this, _| {
                        let active =
                            !this.closing && this.download_operations.values().any(|p| p.active());
                        if !active {
                            this.downloads_watching = false;
                        }
                        active
                    })
                    .unwrap_or(false);
                if !active {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.details = None;
        self.backend.begin_closing();
        self.hover_task = None;
        self.wheel.reset(0.);
        let backend = self.backend.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.shutdown() })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => window.remove_window(),
                    Err(e) => {
                        this.closing = false;
                        this.syncing = false;
                        this.notice(e, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn save_zoom(&self, cx: &mut Context<Self>) {
        self.backend.set_zoom(self.zoom);
        let backend = self.backend.clone();
        cx.background_executor()
            .spawn(async move { backend.save_ui() })
            .detach();
    }
}
