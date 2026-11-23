//! Presentation and controls for game downloads and updates.
use crate::{
    Launcher,
    header::{action_button, pill},
    icons::icon,
};
use raesir_backend::{
    downloads::format_bytes,
    steam_control::{Error, Operation, Phase, Progress},
};
use gpui::{prelude::*, *};
use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

const SIGN_IN: &str = "Sign in";

#[derive(Default)]
pub(crate) struct DownloadRates {
    samples: HashMap<String, VecDeque<(Instant, u64)>>,
}

impl DownloadRates {
    pub(crate) fn record(&mut self, progress: &HashMap<String, Progress>, now: Instant) {
        self.samples.retain(|key, _| {
            progress.get(key).is_some_and(|state| {
                state.phase == Phase::Downloading && state.downloaded_bytes.is_some()
            })
        });
        for (key, state) in progress {
            if state.phase != Phase::Downloading {
                continue;
            }
            let Some(bytes) = state.downloaded_bytes else {
                continue;
            };
            let samples = self.samples.entry(key.clone()).or_default();
            if samples
                .back()
                .is_some_and(|&(_, previous)| bytes < previous)
            {
                samples.clear();
            }
            samples.push_back((now, bytes));
            // Smooth the rate over recent polls so bursts of completed chunks
            // don't cause large swings every time the popup redraws.
            while samples.len() > 2 && now.duration_since(samples[0].0) > Duration::from_secs(5) {
                samples.pop_front();
            }
        }
    }

    pub(crate) fn label(&self, progress: &HashMap<String, Progress>) -> Option<String> {
        let total = self
            .samples
            .iter()
            .filter_map(|(key, samples)| {
                let state = progress.get(key)?;
                let &(first_at, first) = samples.front()?;
                let &(last_at, last) = samples.back()?;
                let seconds = last_at.duration_since(first_at).as_secs_f64();
                (state.phase == Phase::Downloading
                    && state.downloaded_bytes == Some(last)
                    && seconds > 0.)
                    .then(|| (last - first) as f64 / seconds)
            })
            .reduce(|sum, rate| sum + rate)?;
        Some(format!("{}/s", format_bytes(total.round() as u64)))
    }
}

fn needs_xbox_sign_in(progress: &Progress) -> bool {
    progress.phase == Phase::Failed && progress.error == Some(Error::XboxSignInRequired)
}

pub fn has_primary_action(progress: &Progress) -> bool {
    progress.blocks_play() || needs_xbox_sign_in(progress)
}

pub fn button_label(progress: &Progress) -> String {
    match progress.phase {
        Phase::Available => if progress.operation == Operation::Update {
            "Update"
        } else {
            "Install"
        }
        .into(),
        Phase::Queued => "Queued".into(),
        Phase::Starting => "Starting…".into(),
        Phase::Paused => "Resume".into(),
        Phase::Failed if needs_xbox_sign_in(progress) => SIGN_IN.into(),
        Phase::Failed if progress.operation == Operation::Update => "Retry update".into(),
        Phase::Removing | Phase::Complete | Phase::Failed => String::new(),
        Phase::Downloading | Phase::Installing => progress
            .percent
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| {
                if progress.phase == Phase::Installing {
                    "Installing…"
                } else {
                    "Downloading…"
                }
                .into()
            }),
    }
}

pub fn button_icon(progress: &Progress) -> &'static str {
    if needs_xbox_sign_in(progress) {
        "xbox"
    } else if matches!(
        progress.phase,
        Phase::Available | Phase::Paused | Phase::Failed
    ) {
        "install"
    } else if progress.controllable && progress.operation.downloads() {
        "pause"
    } else {
        "install"
    }
}

pub fn button_tip(progress: &Progress) -> String {
    match progress.phase {
        Phase::Available => progress.label(),
        Phase::Paused => "Resume download".into(),
        Phase::Failed if needs_xbox_sign_in(progress) => SIGN_IN.into(),
        Phase::Failed => "Retry update".into(),
        _ if progress.controllable && progress.operation.downloads() => "Pause download".into(),
        _ => progress.label(),
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

fn download_entry(game: &crate::library::Game, progress: &Progress) -> bool {
    !game.source.xbox_console_only
        && progress.operation.downloads()
        && progress.phase != Phase::Complete
}

fn queue_order(progress: &Progress) -> (u8, u64) {
    let priority = match progress.phase {
        Phase::Starting | Phase::Downloading | Phase::Installing => 0,
        Phase::Queued => 1,
        Phase::Paused => 2,
        Phase::Failed => 3,
        Phase::Available if progress.scheduled_at.is_none() => 4,
        _ => 5,
    };
    (priority, progress.scheduled_at.unwrap_or_default())
}

impl Launcher {
    pub(crate) fn control_download(&mut self, key: String, resume: bool, cx: &mut Context<Self>) {
        if self.closing || self.pending_action.is_some() {
            return;
        }
        if resume {
            self.download_failures_dismissed.remove(&key);
        }
        self.pending_action = Some(key.clone());
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.control_download(&key, resume) })
                .await;
            this.update(cx, |this, cx| {
                this.pending_action = None;
                if let Err(error) = result {
                    this.notice(error, cx);
                }
                this.set_download_progress(this.backend.current_download_progress(), cx);
                this.watch_downloads(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn display_progress(&self, key: &str) -> Option<Progress> {
        self.download_operations.get(key).cloned()
    }

    pub(crate) fn primary_download_action(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(progress) = self.display_progress(key).filter(has_primary_action) else {
            return false;
        };
        if needs_xbox_sign_in(&progress) {
            self.retry_ferestre_sign_in(key.into(), progress.operation, cx);
        } else if matches!(progress.phase, Phase::Available | Phase::Failed) {
            self.action(
                key.into(),
                if progress.operation == Operation::Update {
                    "update"
                } else {
                    "install"
                },
                cx,
            );
        } else if progress.controllable {
            self.control_download(key.into(), progress.phase == Phase::Paused, cx);
        } else {
            self.toggle_downloads(cx);
        }
        true
    }

    pub(crate) fn download_summary(&self) -> Option<String> {
        if self.download_operations.is_empty() {
            return None;
        }
        let mut installing: Vec<_> = self
            .games
            .iter()
            .filter_map(|game| {
                let key = &game.source.key;
                let progress = self.download_operations.get(key)?;
                download_entry(game, progress).then_some((key, progress))
            })
            .collect();
        if let Some((key, progress)) = self
            .download_operations
            .get_key_value(raesir_backend::ferestre::RUNTIME_JOB)
            .filter(|(_, progress)| progress.active())
        {
            installing.push((key, progress));
        }
        installing.sort_by_key(|(key, _)| *key);
        let (_, state) = installing
            .iter()
            .find(|(_, p)| p.percent.is_some())
            .or_else(|| installing.first())?;
        Some(if state.phase == Phase::Available {
            installing.len().to_string()
        } else {
            state.percent.map(|p| format!("{p}%")).unwrap_or_default()
        })
    }

    pub(crate) fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        if self.downloads_open {
            self.close_downloads(cx);
            return;
        }
        self.downloads_open = true;
        self.menu = false;
        self.game_menu = None;
        self.watch_downloads(cx);
        self.wheel
            .reset(-f32::from(self.rows.scroll_px_offset_for_scrollbar().y) as f64);
        self.wheel_needs_sync = true;
        cx.notify();
    }

    pub(crate) fn close_downloads(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.downloads_open, false) {
            return;
        }
        let seen = std::mem::take(&mut self.download_failures_seen);
        if !seen.is_empty() {
            self.download_failures_dismissed.extend(seen.clone());
            self.set_download_progress(self.download_operations.clone(), cx);
            let backend = self.backend.clone();
            cx.background_executor()
                .spawn(async move { backend.dismiss_download_failures(&seen) })
                .detach();
        }
        cx.notify();
    }

    pub(crate) fn set_download_progress(
        &mut self,
        mut progress: HashMap<String, Progress>,
        cx: &mut Context<Self>,
    ) -> bool {
        // A poll started before the popup closed can still return its old failure.
        self.download_failures_dismissed
            .retain(|key, seen| progress.get(key).is_none_or(|current| current == seen));
        progress.retain(|key, state| self.download_failures_dismissed.get(key) != Some(state));
        if self.download_operations == progress {
            return false;
        }
        self.download_operations = progress;
        self.update_download_details(cx);
        true
    }

    pub(crate) fn download_popup(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let progress = &self.download_operations;
        let speed = self.download_rates.label(progress);
        let runtime = progress
            .get(raesir_backend::ferestre::RUNTIME_JOB)
            .filter(|state| state.active())
            .cloned();
        let mut entries: Vec<_> = self
            .games
            .iter()
            .enumerate()
            .filter_map(|(index, game)| {
                let state = progress.get(&game.source.key)?;
                download_entry(game, state).then_some((index, state.clone()))
            })
            .collect();
        entries.sort_by_key(|(index, p)| (queue_order(p), self.games[*index].title.clone()));
        let count = entries.len();
        self.downloads_visible.clear();
        if self.download_rows.item_count() != count {
            self.download_rows.reset(count);
        }
        let bounds = self.downloads_bounds.get();
        let margin = self.zoom * 1.1;
        let width = (self.zoom * 38.).min((self.header_state.viewport - margin * 2.).max(0.));
        let left = (f32::from(bounds.origin.x + bounds.size.width / 2.) - width / 2.).clamp(
            margin,
            (self.header_state.viewport - width - margin).max(margin),
        );
        let top = f32::from(bounds.bottom()) + self.zoom * 0.45;
        let max_list_height = (self.viewport_height - top - self.zoom * 8.).max(self.zoom * 4.);
        let rows = if count <= 8 {
            // Let short queues size to their wrapped text. Large queues retain
            // virtualization and scroll inside the available window height.
            for (index, state) in &entries {
                let key = self.games[*index].source.key.clone();
                self.downloads_visible.insert(key.clone());
                if state.phase == Phase::Failed {
                    self.download_failures_seen.insert(key, state.clone());
                }
                crate::artwork::request(self, *index, cx);
            }
            div()
                .id("download-rows")
                .max_h(px(max_list_height))
                .w_full()
                .overflow_y_scroll()
                .children(
                    entries
                        .iter()
                        .map(|(index, state)| self.download_row(&self.games[*index], state, cx)),
                )
                .into_any_element()
        } else {
            let owner = cx.weak_entity();
            list(self.download_rows.clone(), move |row, _, cx| {
                owner
                    .update(cx, |this, cx| {
                        let Some((index, state)) = entries.get(row) else {
                            return div().into_any_element();
                        };
                        crate::artwork::request(this, *index, cx);
                        let Some(game) = this.games.get(*index) else {
                            return div().into_any_element();
                        };
                        this.downloads_visible.insert(game.source.key.clone());
                        if state.phase == Phase::Failed {
                            this.download_failures_seen
                                .insert(game.source.key.clone(), state.clone());
                        }
                        this.download_row(game, state, cx)
                    })
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .h(px(max_list_height))
            .w_full()
            .flex_none()
            .into_any_element()
        };
        div()
            .absolute()
            .inset_0()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_downloads(cx);
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
                            .when_some(speed, |d, speed| {
                                d.child(div().text_color(rgb(0xaabbb2)).child(speed))
                            }),
                    )
                    .when_some(runtime.as_ref(), |d, state| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(rems(0.25))
                                .child("Xbox runtime")
                                .child(
                                    div()
                                        .text_color(rgb(0xaabbb2))
                                        .whitespace_normal()
                                        .child(state.label()),
                                ),
                        )
                    })
                    .when(count == 0 && runtime.is_none(), |d| {
                        d.child(
                            div()
                                .py(rems(1.))
                                .text_color(rgb(0xaabbb2))
                                .child("No downloads"),
                        )
                    })
                    .when(count > 0, |d| d.child(rows)),
            )
            .into_any_element()
    }
    fn download_row(
        &self,
        game: &crate::library::Game,
        state: &Progress,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = game.source.key.clone();
        let ferestre = !raesir_backend::actions::library_handoff_allowed(&game.source);
        let steam_update = game.source.store == "steam"
            && state.operation == Operation::Update
            && !state.controllable;
        let update_now = state.phase == Phase::Available && state.operation == Operation::Update;
        let blocked = raesir_backend::actions::install_limitation(&game.source);
        let sign_in = needs_xbox_sign_in(state);
        let operation = state.operation;
        let label = if let Some(reason) = blocked {
            reason.into()
        } else if state.detail.is_some() || state.phase == Phase::Available {
            state.label()
        } else if let Some(error) = state.error {
            error.message().into()
        } else if matches!(state.phase, Phase::Downloading | Phase::Installing)
            && let Some(percent) = state.percent
        {
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
            .w_full()
            .min_h(rems(5.1))
            .py(rems(0.45))
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
                        d.child(img(poster).size_full().object_fit(ObjectFit::Cover))
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
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(rems(0.55))
                            .child(
                                div()
                                    .whitespace_normal()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(game.title.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rems(0.25))
                                    .px(rems(0.4))
                                    .py(rems(0.15))
                                    .rounded_full()
                                    .bg(rgba(0xffffff15))
                                    .text_size(rems(0.85))
                                    .child(icon(crate::STORES[game.store]).size(rems(1.)))
                                    .child(crate::STORE_NAMES[game.store]),
                            ),
                    )
                    .child(
                        div()
                            .whitespace_normal()
                            .text_color(rgb(0xaabbb2))
                            .child(label),
                    )
                    .when_some(state.percent, |d, percent| {
                        d.child(
                            div()
                                .w_full()
                                .h(rems(0.3))
                                .rounded_full()
                                .bg(rgba(0xffffff1a))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(rgb(0x9ff4bc))
                                        .w(relative(percent as f32 / 100.)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(rems(0.15))
                    .when(
                        blocked.is_none()
                            && !steam_update
                            && matches!(
                                state.phase,
                                Phase::Available | Phase::Paused | Phase::Failed
                            )
                            && state.controllable,
                        |d| {
                            d.child(
                                action_button(
                                    SharedString::from(format!("start-{key}")),
                                    if sign_in { "xbox" } else { "install" },
                                    if sign_in {
                                        SIGN_IN
                                    } else if state.phase == Phase::Failed {
                                        "Retry download"
                                    } else if state.phase == Phase::Available {
                                        if state.operation == Operation::Update {
                                            "Update now"
                                        } else {
                                            "Install"
                                        }
                                    } else {
                                        "Resume download"
                                    },
                                )
                                .when(update_now || sign_in, |button| {
                                    button
                                        .w_auto()
                                        .px(rems(0.65))
                                        .gap(rems(0.35))
                                        .text_size(rems(0.9))
                                        .child(if sign_in { SIGN_IN } else { "Update now" })
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if sign_in {
                                            this.retry_ferestre_sign_in(
                                                start_key.clone(),
                                                operation,
                                                cx,
                                            );
                                        } else {
                                            this.control_download(start_key.clone(), true, cx);
                                        }
                                    },
                                )),
                            )
                        },
                    )
                    .when(
                        blocked.is_none()
                            && !matches!(
                                state.phase,
                                Phase::Available | Phase::Paused | Phase::Failed
                            )
                            && state.controllable,
                        |d| {
                            d.child(
                                action_button(
                                    SharedString::from(format!("stop-{key}")),
                                    "pause",
                                    "Pause download",
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.control_download(stop_key.clone(), false, cx);
                                    },
                                )),
                            )
                        },
                    )
                    .when(
                        blocked.is_none() && (steam_update || (!ferestre && !state.controllable)),
                        |d| {
                            let key = key.clone();
                            d.child(
                                action_button(
                                    SharedString::from(format!("client-{key}")),
                                    "external",
                                    if steam_update {
                                        "Open Steam Downloads"
                                    } else {
                                        "Open in game client"
                                    },
                                )
                                .when(steam_update, |button| {
                                    button
                                        .w_auto()
                                        .px(rems(0.65))
                                        .gap(rems(0.35))
                                        .text_size(rems(0.9))
                                        .child("Steam Downloads")
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if steam_update {
                                            this.action(key.clone(), "steam-downloads", cx);
                                        } else {
                                            this.action(key.clone(), "library", cx);
                                        }
                                    },
                                )),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadRates, Duration, HashMap, Instant, Operation, Phase, Progress};

    #[test]
    fn download_rates_sum_live_transfers_and_reset_on_pause_or_restart() {
        let mut rates = DownloadRates::default();
        let active = |bytes| Progress {
            phase: Phase::Downloading,
            downloaded_bytes: Some(bytes),
            ..Progress::starting(Operation::Install)
        };
        let mut transfers = HashMap::from([
            ("xbox".into(), active(1_000_000_000)),
            ("steam".into(), active(0)),
        ]);
        let start = Instant::now();
        rates.record(&transfers, start);
        assert_eq!(rates.label(&transfers), None);

        transfers.insert("xbox".into(), active(1_020_000_000));
        transfers.insert("steam".into(), active(2_000_000));
        rates.record(&transfers, start + Duration::from_secs(2));
        assert_eq!(rates.label(&transfers).as_deref(), Some("11.0 Mb/s"));

        transfers.get_mut("steam").unwrap().phase = Phase::Paused;
        transfers.insert("xbox".into(), active(0));
        rates.record(&transfers, start + Duration::from_secs(3));
        assert_eq!(rates.label(&transfers), None);

        transfers.insert("xbox".into(), active(2_000_000));
        rates.record(&transfers, start + Duration::from_secs(4));
        assert_eq!(rates.label(&transfers).as_deref(), Some("2.0 Mb/s"));
    }
}
