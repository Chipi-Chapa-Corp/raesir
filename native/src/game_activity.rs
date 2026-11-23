use crate::{Launcher, downloads, header, header::spinner_colored, icons::icon};
use gpui::{prelude::*, *};
use raesir_backend::{
    game_sessions::{self, ProcessId},
    model::LibraryGame,
    steam_control::Progress,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const START_TIMEOUT: Duration = Duration::from_secs(90);
const QUIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Activity {
    Starting(Instant),
    Running(Vec<ProcessId>),
    Quitting(Instant, Vec<ProcessId>),
}

#[derive(Clone)]
pub(crate) struct GameSession {
    pub game: Arc<LibraryGame>,
    pub activity: Activity,
    // Set after dispatch. A failed process lookup or a game that was already
    // running cannot trigger an automatic exit.
    pub started_by_raesir: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayButton {
    Starting,
    Quit,
    Quitting,
}

impl PlayButton {
    pub(crate) fn element_id(self) -> &'static str {
        match self {
            Self::Starting => "game-starting",
            Self::Quit => "quit-game",
            Self::Quitting => "game-quitting",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting…",
            Self::Quit => "Quit game",
            Self::Quitting => "Quitting…",
        }
    }

    pub(crate) fn glyph(self, color: u32) -> AnyElement {
        match self {
            Self::Quit => icon("pause").text_color(rgb(color)).into_any_element(),
            _ => spinner_colored("game-transition", 1.25, color),
        }
    }
}

pub(crate) fn game_action_button(
    id: impl Into<ElementId>,
    play: Option<PlayButton>,
    progress: Option<&Progress>,
    idle_icon: &'static str,
    tooltip: Option<String>,
) -> Stateful<Div> {
    let glyph = match play {
        Some(button) => button.glyph(0x202020),
        None => icon(progress.map_or(idle_icon, downloads::button_icon))
            .text_color(rgb(0x202020))
            .into_any_element(),
    };
    let label = play
        .filter(|button| *button != PlayButton::Quit)
        .map(|button| button.label().to_owned())
        .or_else(|| progress.map(downloads::button_label))
        .filter(|label| !label.is_empty());
    header::item_action(id, true)
        .aria_disabled(play.is_some_and(|button| button != PlayButton::Quit))
        .child(glyph)
        .when_some(label, |d, label| {
            d.w_auto().px(rems(0.7)).gap(rems(0.35)).child(label)
        })
        .when_some(tooltip, header::button_tooltip)
}

impl Launcher {
    pub(crate) fn play_button(&self, key: &str) -> Option<PlayButton> {
        self.game_activity
            .get(key)
            .map(|session| match &session.activity {
                Activity::Starting(_) => PlayButton::Starting,
                Activity::Running(_) => PlayButton::Quit,
                Activity::Quitting(_, _) => PlayButton::Quitting,
            })
    }

    pub(crate) fn refresh_game_activity(&self, cx: &mut Context<Self>) {
        if let Some(details) = &self.details {
            details.update(cx, |page, cx| page.update_play_button(self, cx));
        }
        cx.notify();
    }

    pub(crate) fn watch_game_activity(&mut self, cx: &mut Context<Self>) {
        if self.game_activity_task.is_some() {
            return;
        }
        let window = self.window;
        self.game_activity_task = Some(cx.spawn(async move |this, cx| {
            let mut exit_after_game = false;
            loop {
                cx.background_executor().timer(millis(750)).await;
                let Ok(tracked) = this.update(cx, |this, _| {
                    (!this.closing).then(|| this.game_activity.values()
                        .filter(|session| session.started_by_raesir.is_some())
                        .cloned()
                        .collect::<Vec<_>>())
                }) else { break };
                let Some(tracked) = tracked else { break };
                if tracked.is_empty() {
                    if exit_after_game && window.update(cx, |_, window, app| {
                        this.update(app, |this, cx| {
                            if !this.game_activity.is_empty() || !this.backend.exit_with_game() {
                                return false;
                            }
                            if this.mods_watching {
                                return true;
                            }
                            this.close(window, cx);
                            false
                        }).unwrap_or(false)
                    }).unwrap_or(false) {
                        continue;
                    }
                    break;
                }
                let games: Vec<_> = tracked.iter().map(|session| session.game.clone()).collect();
                let observed = cx.background_executor()
                    .spawn(async move { game_sessions::running(games.iter().map(Arc::as_ref)) })
                    .await;
                let Ok(should_exit) = this.update(cx, |this, cx| {
                    let mut changed = false;
                    let mut game_stopped = false;
                    for previous in tracked {
                        let key = &previous.game.key;
                        let Some(current) = this.game_activity.get_mut(key) else { continue };
                        if !Arc::ptr_eq(&current.game, &previous.game) || current.activity != previous.activity {
                            continue;
                        }
                        let activity = &previous.activity;
                        let (next, message) = match (activity, &observed) {
                            (Activity::Starting(_), Ok(games)) if games.contains_key(key) => {
                                (Some(Activity::Running(games[key].clone())), None)
                            }
                            (Activity::Starting(since), _) if since.elapsed() >= START_TIMEOUT => {
                                (None, Some("Could not detect the game starting. Check the game client."))
                            }
                            (Activity::Quitting(since, _), Ok(games)) if games.contains_key(key) && since.elapsed() >= QUIT_TIMEOUT => {
                                (Some(Activity::Running(games[key].clone())), Some("The game is still running. Check for a quit or save confirmation."))
                            }
                            (Activity::Running(_), Ok(games)) => {
                                (games.get(key).cloned().map(Activity::Running), None)
                            }
                            (Activity::Quitting(..), Ok(games)) if !games.contains_key(key) => (None, None),
                            (Activity::Quitting(since, processes), Err(_)) if since.elapsed() >= QUIT_TIMEOUT => {
                                (Some(Activity::Running(processes.clone())), Some("Could not confirm that the game quit."))
                            }
                            _ => continue,
                        };
                        if next.as_ref() != Some(activity) {
                            changed = true;
                            if let Some(next) = next {
                                current.activity = next;
                            } else {
                                game_stopped |= previous.started_by_raesir == Some(true)
                                    && matches!(activity, Activity::Running(_) | Activity::Quitting(..));
                                this.game_activity.remove(key);
                            }
                        }
                        if let Some(message) = message {
                            this.notice(message.into(), cx);
                        }
                    }
                    if changed {
                        this.refresh_game_activity(cx);
                    }
                    game_stopped && this.game_activity.is_empty() && this.backend.exit_with_game()
                }) else { break };
                exit_after_game = should_exit;
            }
            let _ = this.update(cx, |this, _| this.game_activity_task = None);
        }));
    }

    pub(crate) fn quit_running_game(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(session) = self.game_activity.get_mut(&key) else {
            return;
        };
        let Activity::Running(processes) = &session.activity else {
            return;
        };
        let processes = processes.clone();
        let request = Activity::Quitting(Instant::now(), processes.clone());
        session.activity = request.clone();
        self.refresh_game_activity(cx);
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let game = key.clone();
            let result = cx
                .background_executor()
                .spawn(async move { backend.quit_game(&game, &processes) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    if let Some(session) = this.game_activity.get_mut(&key)
                        && session.activity == request
                    {
                        let Activity::Quitting(_, processes) = request else {
                            unreachable!()
                        };
                        session.activity = Activity::Running(processes);
                        this.notice(error, cx);
                        this.refresh_game_activity(cx);
                    }
                });
            }
        })
        .detach();
    }
}
