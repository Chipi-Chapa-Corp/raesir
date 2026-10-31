//! Queue presentation. Preview entries never send commands to a game client.
use crate::{
    Launcher,
    header::{action_button, pill},
    icons::icon,
    library::Game,
};
use gpui::{prelude::*, *};
use raesir_backend::steam_control::{Operation, Phase, Progress};
use std::{
    cell::Cell,
    collections::{HashMap, hash_map::RandomState},
    hash::BuildHasher,
    rc::Rc,
};

#[derive(Default)]
pub struct PreviewQueue {
    seeded: bool,
    items: Vec<(String, Option<u8>)>,
}

impl PreviewQueue {
    pub fn clear(&mut self) {
        self.seeded = true;
        self.items.clear();
    }
    pub fn seed(&mut self, games: &[Game]) {
        if self.seeded || games.is_empty() {
            return;
        }
        self.seeded = true;
        // Temporary preview requested for UI review. Opt out for live-only use.
        if std::env::var_os("RAESIR_MOCK_DOWNLOADS").is_some_and(|v| v == "0") {
            return;
        }
        let random = RandomState::new();
        let mut candidates: Vec<_> = games
            .iter()
            .filter(|g| {
                g.source.store == "steam" && raesir_backend::actions::can_install(&g.source)
            })
            .collect();
        candidates.sort_by_key(|g| {
            (
                !g.source
                    .cover
                    .as_ref()
                    .is_some_and(|p| std::path::Path::new(p).is_file()),
                random.hash_one(&g.source.key),
            )
        });
        self.items = candidates
            .into_iter()
            .take(3)
            .enumerate()
            .map(|(i, g)| (g.source.key.clone(), (i == 0).then_some(45)))
            .collect();
    }

    fn progress(&self, key: &str) -> Option<Progress> {
        let (_, percent) = self.items.iter().find(|(k, _)| k == key)?;
        Some(Progress {
            operation: Operation::Install,
            phase: if percent.is_some() {
                Phase::Downloading
            } else {
                Phase::Queued
            },
            percent: *percent,
            error: None,
        })
    }

    fn stop(&mut self, key: &str) {
        self.items.retain(|(k, _)| k != key);
        self.start_next();
    }

    fn start_next(&mut self) {
        if !self.items.iter().any(|(_, percent)| percent.is_some())
            && let Some((_, percent)) = self.items.first_mut()
        {
            *percent = Some(0);
        }
    }

    fn advance(&mut self) {
        self.items.retain(|(_, percent)| *percent != Some(100));
        self.start_next();
        if let Some((_, Some(percent))) = self.items.iter_mut().find(|(_, p)| p.is_some()) {
            *percent = percent.saturating_add(1).min(100);
        }
    }

    fn start_now(&mut self, key: &str) {
        if !self.items.iter().any(|(k, _)| k == key) {
            return;
        }
        // One active preview download; starting another moves the previous
        // game back into the queue.
        for (k, percent) in &mut self.items {
            *percent = (k == key).then_some(0);
        }
        self.items.sort_by_key(|(_, percent)| percent.is_none());
    }
}

pub fn button_label(progress: &Progress) -> String {
    match progress.phase {
        Phase::Queued | Phase::Starting => "Queued".into(),
        Phase::Paused => "Paused".into(),
        Phase::Removing | Phase::Complete | Phase::Failed => String::new(),
        _ => progress
            .percent
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "Queued".into()),
    }
}

pub fn queue_pill(
    label: String,
    selected: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
) -> Stateful<Div> {
    pill()
        .id("downloads-button")
        .on_prepaint(move |paint, _, _| bounds.set(paint.bounds))
        .h(rems(2.9))
        .px(rems(0.85))
        .gap(rems(if label.is_empty() { 0. } else { 0.5 }))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0x303030b3)))
        .when(selected, |d| d.bg(rgba(0x303030e6)))
        .child(icon("install"))
        .text_size(rems(1.125))
        .child(label)
        .tooltip(|_, cx| {
            cx.new(|_| crate::library::ActionTooltip("Downloads".into()))
                .into()
        })
}

impl Launcher {
    fn control_download(&mut self, key: String, resume: bool, cx: &mut Context<Self>) {
        if self.closing || self.pending_action.is_some() {
            return;
        }
        if self.download_preview.progress(&key).is_some()
            && !self.steam_operations.contains_key(&key)
        {
            if resume {
                self.download_preview.start_now(&key);
            } else {
                self.download_preview.stop(&key);
            }
            self.update_steam_details(cx);
            cx.notify();
            return;
        }
        self.pending_action = Some(key.clone());
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move { backend.steam_download(&key, resume) })
                .await;
            this.update(cx, |this, cx| {
                this.pending_action = None;
                this.steam_operations = this.backend.current_steam_progress();
                this.update_steam_details(cx);
                this.watch_steam(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn watch_download_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview_watching || self.closing || self.download_preview.items.is_empty() {
            return;
        }
        self.preview_watching = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(millis(1000)).await;
                let stop = this
                    .update(cx, |this, cx| {
                        if this.closing || this.download_preview.items.is_empty() {
                            this.preview_watching = false;
                            return true;
                        }
                        this.download_preview.advance();
                        this.update_steam_details(cx);
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    return;
                }
            }
        })
        .detach();
    }

    pub(crate) fn display_progress(&self, key: &str) -> Option<Progress> {
        self.steam_operations
            .get(key)
            .cloned()
            .or_else(|| self.download_preview.progress(key))
    }

    pub(crate) fn display_operations(&self) -> HashMap<String, Progress> {
        let mut progress = self.steam_operations.clone();
        for (key, _) in &self.download_preview.items {
            if let Some(value) = self.download_preview.progress(key) {
                progress.entry(key.clone()).or_insert(value);
            }
        }
        progress
    }

    pub(crate) fn download_summary(&self) -> Option<String> {
        let progress = self.display_operations();
        let mut installing: Vec<_> = progress
            .iter()
            .filter(|(_, p)| p.operation == Operation::Install && p.active())
            .collect();
        installing.sort_by_key(|(key, _)| (!self.steam_operations.contains_key(*key), *key));
        let (_, state) = installing
            .iter()
            .find(|(_, p)| p.percent.is_some())
            .or_else(|| installing.first())?;
        Some(state.percent.map(|p| format!("{p}%")).unwrap_or_default())
    }

    pub(crate) fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads_open = !self.downloads_open;
        if self.downloads_open {
            self.menu = false;
            self.game_menu = None;
        }
        cx.notify();
    }

    pub(crate) fn request_download_artwork(&mut self, cx: &mut Context<Self>) {
        if self.downloads_open {
            let keys = self.display_operations();
            let indices: Vec<_> = self
                .games
                .iter()
                .enumerate()
                .filter(|(_, g)| keys.get(&g.source.key).is_some_and(|p| p.active()))
                .map(|(i, _)| i)
                .collect();
            for i in indices {
                crate::artwork::request(self, i, cx);
            }
        }
    }

    pub(crate) fn download_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.display_operations();
        let mut entries: Vec<_> = self
            .games
            .iter()
            .filter_map(|game| {
                let state = progress.get(&game.source.key)?;
                (state.operation == Operation::Install && state.active()).then_some((game, state))
            })
            .collect();
        entries.sort_by_key(|(g, p)| (p.phase == Phase::Queued, g.title.clone()));
        let count = entries.len();
        let bounds = self.downloads_bounds.get();
        let margin = self.zoom * 1.1;
        let width = (self.zoom * 26.).min((self.header_state.viewport - margin * 2.).max(0.));
        let left = (f32::from(bounds.origin.x + bounds.size.width / 2.) - width / 2.).clamp(
            margin,
            (self.header_state.viewport - width - margin).max(margin),
        );
        let top = f32::from(bounds.bottom()) + self.zoom * 0.45;
        div()
            .absolute()
            .inset_0()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.downloads_open = false;
                    cx.notify();
                }),
            ))
            .child(
                pill()
                    .id("downloads-popup")
                    .absolute()
                    .top(px(top))
                    .left(px(left))
                    .w(px(width))
                    .text_size(rems(1.125))
                    .max_h(px((self.viewport_height - self.zoom * 5.).max(120.)))
                    .overflow_y_scroll()
                    .rounded(rems(1.2))
                    .shadow_lg()
                    .p(rems(1.))
                    .flex_col()
                    .items_stretch()
                    .gap(rems(0.9))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Downloads"))
                            .child(div().text_color(rgb(0xaabbb2)).child(format!(
                                "{count} {}",
                                if count == 1 { "game" } else { "games" }
                            ))),
                    )
                    .when(count == 0, |d| {
                        d.child(
                            div()
                                .py(rems(1.))
                                .text_color(rgb(0xaabbb2))
                                .child("No downloads"),
                        )
                    })
                    .children(entries.into_iter().map(|(game, state)| {
                        let key = game.source.key.clone();
                        let queued =
                            matches!(state.phase, Phase::Starting | Phase::Queued | Phase::Paused);
                        let label = if let Some(error) = state.error {
                            error.message().into()
                        } else if state.phase == Phase::Paused {
                            "Paused".into()
                        } else if queued {
                            "Queued".into()
                        } else if let Some(percent) = state.percent {
                            format!(
                                "{} · {percent}%",
                                if state.phase == Phase::Installing {
                                    "Installing"
                                } else {
                                    "Downloading"
                                }
                            )
                        } else {
                            state.label()
                        };
                        let stop_key = key.clone();
                        let start_key = key.clone();
                        div()
                            .id(SharedString::from(format!("download-{key}")))
                            .flex()
                            .items_center()
                            .gap(rems(0.8))
                            .child(
                                div()
                                    .w(rems(3.))
                                    .h(rems(4.2))
                                    .flex_none()
                                    .rounded(rems(0.45))
                                    .overflow_hidden()
                                    .bg(rgba(0xffffff14))
                                    .when_some(game.poster.clone(), |d, poster| {
                                        d.child(
                                            img(poster).size_full().object_fit(ObjectFit::Cover),
                                        )
                                    })
                                    .when(game.poster.is_none(), |d| {
                                        d.flex()
                                            .items_center()
                                            .justify_center()
                                            .child(icon("install"))
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(rems(0.4))
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(game.title.clone()),
                                    )
                                    .child(div().text_color(rgb(0xaabbb2)).child(label))
                                    .child(
                                        div()
                                            .w_full()
                                            .h(rems(0.3))
                                            .rounded_full()
                                            .bg(rgba(0xffffff1a))
                                            .overflow_hidden()
                                            .child(
                                                div().h_full().rounded_full().bg(rgb(0x9ff4bc)).w(
                                                    relative(
                                                        state.percent.unwrap_or(0) as f32 / 100.,
                                                    ),
                                                ),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap(rems(0.15))
                                    .when(queued, |d| {
                                        d.child(
                                            action_button(
                                                SharedString::from(format!("start-{key}")),
                                                "install",
                                                "Download now",
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.control_download(
                                                        start_key.clone(),
                                                        true,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        )
                                    })
                                    .child(
                                        action_button(
                                            SharedString::from(format!("stop-{key}")),
                                            "stop",
                                            "Stop downloading",
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.control_download(stop_key.clone(), false, cx);
                                            }),
                                        ),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn starting_a_queued_preview_switches_the_active_game_and_stop_removes_it() {
        let mut queue = PreviewQueue {
            seeded: true,
            items: vec![
                ("a".into(), Some(45)),
                ("b".into(), None),
                ("c".into(), None),
            ],
        };
        queue.start_now("missing");
        assert_eq!(queue.progress("a").unwrap().percent, Some(45));
        queue.start_now("b");
        assert_eq!(queue.progress("b").unwrap().percent, Some(0));
        assert_eq!(queue.progress("a").unwrap().phase, Phase::Queued);
        assert_eq!(queue.items.iter().filter(|(_, p)| p.is_some()).count(), 1);
        queue.stop("b");
        assert!(queue.progress("b").is_none());
        assert_eq!(queue.items.len(), 2);
        assert_eq!(queue.progress("a").unwrap().percent, Some(0));
    }

    #[::core::prelude::v1::test]
    fn preview_progress_advances_completes_and_starts_the_next_download() {
        let mut queue = PreviewQueue {
            seeded: true,
            items: vec![("a".into(), Some(45)), ("b".into(), None)],
        };
        queue.advance();
        assert_eq!(queue.progress("a").unwrap().percent, Some(46));
        for _ in 0..54 {
            queue.advance();
        }
        assert_eq!(queue.progress("a").unwrap().percent, Some(100));
        queue.advance();
        assert!(queue.progress("a").is_none());
        assert_eq!(queue.progress("b").unwrap().percent, Some(1));
        queue.stop("b");
        queue.advance();
        assert!(queue.items.is_empty());
    }
}
