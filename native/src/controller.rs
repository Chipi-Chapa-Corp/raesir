use crate::{Launcher, library::Game};
use gpui::*;
use raesir_lib::{catalog, model::Report};
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

    fn apply_report(&mut self, report: Arc<Report>, cx: &mut Context<Self>) {
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
        self.available = self.games.iter().fold(0, |bits, g| bits | (1 << g.store));
        if self
            .store
            .is_some_and(|store| !self.games.iter().any(|g| g.store == store))
        {
            self.store = None;
        }
        self.report = Some(report);
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
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.action(&key, kind) })
                .await;
            let refresh = result.is_ok() && kind == "uninstall";
            this.update(cx, |this, cx| {
                this.pending_action = None;
                match result {
                    Err(error) => this.notice(error, cx),
                    Ok(message) if !message.starts_with("Sent to ") => this.notice(message, cx),
                    _ => {}
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
