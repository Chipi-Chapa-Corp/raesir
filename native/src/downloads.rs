//! Presentation and controls for game downloads and updates.
use crate::{
    Launcher,
    header::{action_button, pill},
    icons::icon,
};
use gpui::{prelude::*, *};
use raesir_backend::steam_control::{Operation, Phase, Progress};
use std::{cell::Cell, rc::Rc};

pub fn button_label(progress: &Progress) -> String {
    match progress.phase {
        Phase::Available => if progress.operation == Operation::Update {
            "Update"
        } else {
            "Install"
        }
        .into(),
        Phase::Queued | Phase::Starting => "Queued".into(),
        Phase::Paused => "Resume".into(),
        Phase::Failed if progress.operation == Operation::Update => "Retry update".into(),
        Phase::Removing | Phase::Complete | Phase::Failed => String::new(),
        _ => progress
            .percent
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "Queued".into()),
    }
}

pub fn button_icon(progress: &Progress) -> &'static str {
    if matches!(
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
        Phase::Available => progress
            .detail
            .clone()
            .unwrap_or_else(|| button_label(progress)),
        Phase::Paused => "Resume download".into(),
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

impl Launcher {
    pub(crate) fn control_download(&mut self, key: String, resume: bool, cx: &mut Context<Self>) {
        if self.closing || self.pending_action.is_some() {
            return;
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
                this.download_operations = this.backend.current_download_progress();
                this.update_download_details(cx);
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
        let Some(progress) = self.display_progress(key).filter(Progress::blocks_play) else {
            return false;
        };
        if matches!(progress.phase, Phase::Available | Phase::Failed) {
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
        let progress = &self.download_operations;
        let mut installing: Vec<_> = progress
            .iter()
            .filter(|(_, p)| p.operation.downloads() && p.blocks_play())
            .collect();
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
        self.downloads_open = !self.downloads_open;
        if self.downloads_open {
            self.menu = false;
            self.game_menu = None;
            self.wheel
                .reset(-f32::from(self.rows.scroll_px_offset_for_scrollbar().y) as f64);
            self.wheel_needs_sync = true;
        }
        cx.notify();
    }

    pub(crate) fn request_download_artwork(&mut self, cx: &mut Context<Self>) {
        if self.downloads_open {
            let keys = &self.download_operations;
            let indices: Vec<_> = self
                .games
                .iter()
                .enumerate()
                .filter(|(_, g)| keys.get(&g.source.key).is_some_and(|p| p.active()))
                .filter(|(_, g)| self.downloads_visible.contains(&g.source.key))
                .map(|(i, _)| i)
                .collect();
            for i in indices {
                crate::artwork::request(self, i, cx);
            }
        }
    }

    pub(crate) fn download_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let progress = &self.download_operations;
        let mut entries: Vec<_> = self
            .games
            .iter()
            .enumerate()
            .filter_map(|(index, game)| {
                let state = progress.get(&game.source.key)?;
                (state.operation.downloads() && state.blocks_play())
                    .then_some((index, state.clone()))
            })
            .collect();
        entries.sort_by_key(|(index, p)| {
            (
                match p.phase {
                    Phase::Starting | Phase::Downloading | Phase::Installing => 0,
                    Phase::Queued => 1,
                    Phase::Paused => 2,
                    _ => 3,
                },
                self.games[*index].title.clone(),
            )
        });
        let count = entries.len();
        let bounds = self.downloads_bounds.get();
        let margin = self.zoom * 1.1;
        let width = (self.zoom * 26.).min((self.header_state.viewport - margin * 2.).max(0.));
        let left = (f32::from(bounds.origin.x + bounds.size.width / 2.) - width / 2.).clamp(
            margin,
            (self.header_state.viewport - width - margin).max(margin),
        );
        let top = f32::from(bounds.bottom()) + self.zoom * 0.45;
        let height = (count as f32 * self.zoom * 5.1)
            .min((self.viewport_height - top - self.zoom * 4.).max(0.));
        let list = uniform_list(
            "download-rows",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                this.downloads_visible.clear();
                for index in range.clone() {
                    if let Some((index, _)) = entries.get(index)
                        && let Some(game) = this.games.get(*index)
                    {
                        this.downloads_visible.insert(game.source.key.clone());
                    }
                }
                range
                    .filter_map(|row| {
                        let (index, state) = entries.get(row)?;
                        crate::artwork::request(this, *index, cx);
                        this.games
                            .get(*index)
                            .map(|game| this.download_row(game, state, cx))
                    })
                    .collect()
            }),
        )
        .h(px(height))
        .w_full()
        .flex_none();
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
                    .when(count > 0, |d| d.child(list)),
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
        let queued = matches!(
            state.phase,
            Phase::Available | Phase::Failed | Phase::Starting | Phase::Queued | Phase::Paused
        );
        let label = if state.detail.is_some() || state.phase == Phase::Available {
            state.label()
        } else if let Some(error) = state.error {
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
            .w_full()
            .h(rems(5.1))
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
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(game.title.clone()),
                    )
                    .child(div().truncate().text_color(rgb(0xaabbb2)).child(label))
                    .child(
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
                                    .w(relative(state.percent.unwrap_or(0) as f32 / 100.)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(rems(0.15))
                    .when(
                        matches!(
                            state.phase,
                            Phase::Available | Phase::Paused | Phase::Failed
                        ) && (state.controllable || state.phase == Phase::Available),
                        |d| {
                            d.child(
                                action_button(
                                    SharedString::from(format!("start-{key}")),
                                    "install",
                                    if state.phase == Phase::Available {
                                        if state.operation == Operation::Update {
                                            "Update"
                                        } else {
                                            "Install"
                                        }
                                    } else {
                                        "Resume download"
                                    },
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.control_download(start_key.clone(), true, cx);
                                    },
                                )),
                            )
                        },
                    )
                    .when(
                        !matches!(
                            state.phase,
                            Phase::Available | Phase::Paused | Phase::Failed
                        ) && state.controllable,
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
                        !state.controllable && state.phase != Phase::Available,
                        |d| {
                            let key = key.clone();
                            d.child(
                                action_button(
                                    SharedString::from(format!("client-{key}")),
                                    "external",
                                    "Open in game client",
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.action(key.clone(), "library", cx),
                                )),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}
