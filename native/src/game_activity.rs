use crate::{Launcher, header::spinner_colored, icons::icon};
use exposed_games_backend::game_sessions::ProcessId;
use gpui::{prelude::*, *};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(90);
const QUIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Activity {
    Starting(Instant),
    Running(Vec<ProcessId>),
    Quitting(Instant, Vec<ProcessId>),
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

impl Launcher {
    pub(crate) fn play_button(&self, key: &str) -> Option<PlayButton> {
        self.game_activity.get(key).map(|activity| match activity {
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
        let backend = self.backend.clone();
        self.game_activity_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(millis(750)).await;
                let Ok(tracked) = this.update(cx, |this, _| {
                    (!this.closing && !this.game_activity.is_empty())
                        .then(|| this.game_activity.clone())
                }) else { break };
                let Some(tracked) = tracked else { break };
                let keys: Vec<_> = tracked.keys().cloned().collect();
                let scan = backend.clone();
                let observed = cx.background_executor()
                    .spawn(async move { scan.running_games(keys.into_iter()) })
                    .await;
                if this.update(cx, |this, cx| {
                    let mut changed = false;
                    for (key, previous) in tracked {
                        if this.game_activity.get(&key) != Some(&previous) {
                            continue;
                        }
                        let (next, message) = match (&previous, &observed) {
                            (Activity::Starting(_), Ok(games)) if games.contains_key(&key) => {
                                (Some(Activity::Running(games[&key].clone())), None)
                            }
                            (Activity::Starting(since), _) if since.elapsed() >= START_TIMEOUT => {
                                (None, Some("Could not detect the game starting. Check the game client."))
                            }
                            (Activity::Quitting(since, _), Ok(games)) if games.contains_key(&key) && since.elapsed() >= QUIT_TIMEOUT => {
                                (Some(Activity::Running(games[&key].clone())), Some("The game is still running. Check for a quit or save confirmation."))
                            }
                            (Activity::Running(_), Ok(games)) => {
                                (games.get(&key).cloned().map(Activity::Running), None)
                            }
                            (Activity::Quitting(..), Ok(games)) if !games.contains_key(&key) => (None, None),
                            (Activity::Quitting(since, processes), Err(_)) if since.elapsed() >= QUIT_TIMEOUT => {
                                (Some(Activity::Running(processes.clone())), Some("Could not confirm that the game quit."))
                            }
                            _ => continue,
                        };
                        if next.as_ref() != Some(&previous) {
                            changed = true;
                            if let Some(next) = next {
                                this.game_activity.insert(key, next);
                            } else {
                                this.game_activity.remove(&key);
                            }
                        }
                        if let Some(message) = message {
                            this.notice(message.into(), cx);
                        }
                    }
                    if changed {
                        this.refresh_game_activity(cx);
                    }
                }).is_err() { break; }
            }
            let _ = this.update(cx, |this, _| this.game_activity_task = None);
        }));
    }

    pub(crate) fn quit_running_game(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(Activity::Running(processes)) = self.game_activity.get(&key).cloned() else {
            return;
        };
        let request = Activity::Quitting(Instant::now(), processes.clone());
        self.game_activity.insert(key.clone(), request.clone());
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
                    if this.game_activity.get(&key) == Some(&request) {
                        let Activity::Quitting(_, processes) = request else {
                            unreachable!()
                        };
                        this.game_activity.insert(key, Activity::Running(processes));
                        this.notice(error, cx);
                        this.refresh_game_activity(cx);
                    }
                });
            }
        })
        .detach();
    }
}
