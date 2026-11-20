use crate::{
    Launcher,
    backend::{PartitionChoice, rememberable_partition},
    header::pill,
    library::FerestreSetup,
};
use exposed_games_backend::model::Os;
use gpui::{prelude::*, *};

pub(crate) fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(rems(1.))
        .py(rems(0.6))
        .rounded(rems(0.7))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff18)))
        .child(label.into())
}

impl Launcher {
    pub fn skeleton(&self, width: f32) -> impl IntoElement + use<> {
        div()
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .rounded(rems(0.8))
            .overflow_hidden()
            .bg(rgba(0x101010a6))
            .child(
                div()
                    .w_full()
                    .h(px(width * crate::CARD_POSTER_RATIO))
                    .flex_none()
                    .rounded_t(rems(0.8))
                    .bg(rgba(0xffffff1a)),
            )
            .child(
                div()
                    .w_full()
                    .h(rems(crate::CARD_TITLE_HEIGHT))
                    .mt(px(-width
                        * crate::CARD_POSTER_RATIO
                        * crate::CARD_TITLE_OVERLAP))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(relative(0.7))
                            .h(rems(1.1))
                            .rounded_full()
                            .bg(rgba(0xffffff1a)),
                    ),
            )
    }

    pub fn banners(&self, cx: &mut Context<Self>) -> Div {
        let scope = self.library_scope();
        let store = self.store.map(|s| crate::STORES[s]);
        let runtime_progress = self
            .download_operations
            .get(exposed_games_backend::ferestre::RUNTIME_JOB);
        let mut ferestre_setup = self
            .report
            .as_ref()
            .and_then(|report| crate::library::ferestre_setup(report, scope, store));
        if self.host == Os::Linux && store == Some("xbox") && scope != 2
            && (self.ferestre_task.as_ref().is_some_and(|(action, _)| *action == FerestreSetup::MaintainRuntime)
                || runtime_progress.is_some_and(|state| {
                    state.operation == exposed_games_backend::steam_control::Operation::Update
                        && (state.active() || state.phase == exposed_games_backend::steam_control::Phase::Failed)
                }))
        {
            ferestre_setup = Some(FerestreSetup::MaintainRuntime);
        }
        let xbox_link_needed = self.report.as_ref().is_some_and(|report| {
            report.linux_systems.iter().any(|system| {
                system.root == "/" && (system.managed_ferestre || system.ferestre_client) && !system.ferestre_library
            })
        });
        let xbox_notice = ferestre_setup != Some(FerestreSetup::Install)
            && self
                .report
                .as_ref()
                .is_some_and(|report| crate::library::xbox_runtime_notice(report, scope, store));
        let partition_choices = self.backend.partition_choices();
        let mount_failures = self.backend.partition_mount_failures();
        let volumes: Vec<_> = self
            .report
            .as_ref()
            .map(|r| {
                r.volumes
                    .iter()
                    .filter(|v| {
                        v.mountpoints.is_empty()
                            && ["ntfs", "ext4", "btrfs", "linux"]
                                .contains(&v.filesystem.to_lowercase().as_str())
                            && !self.ignored_partitions.contains(&v.id)
                            && match partition_choices.get(&v.id) {
                                Some(PartitionChoice::Ignore) => false,
                                Some(PartitionChoice::Mount) => {
                                    !self.syncing && mount_failures.contains_key(&v.id)
                                }
                                None => true,
                            }
                    })
                    .collect()
            })
            .unwrap_or_default();
        div()
            .w_full()
            .pt(rems(5.))
            .px(rems(1.5))
            .pb(rems(if volumes.is_empty() && !xbox_notice && ferestre_setup.is_none() { 0. } else { 1. }))
            .flex()
            .flex_col()
            .gap(rems(0.5))
            .when_some(ferestre_setup, |d, setup| {
                let action = self.ferestre_task.as_ref().map(|(action, _)| *action);
                let busy = action == Some(setup);
                let runtime_setup = matches!(setup, FerestreSetup::Runtime | FerestreSetup::MaintainRuntime);
                let runtime_active = runtime_setup
                    && runtime_progress.is_some_and(|state| state.active());
                let (message, label) = match (setup, busy) {
                    (FerestreSetup::Install, false) => ("Install support for compatible Xbox PC games on Linux.", "Set up Xbox"),
                    (FerestreSetup::Install, true) => ("Downloading and verifying Xbox support…", "Installing…"),
                    (FerestreSetup::SignIn, false) => ("Sign in with the Microsoft account you use for Xbox PC games.", "Sign in to Xbox"),
                    (FerestreSetup::SignIn, true) => ("Complete Microsoft sign-in in the account window.", "Signing in…"),
                    (FerestreSetup::Runtime, false) => ("Install the runtime to play compatible Xbox PC games.", "Install Xbox runtime"),
                    (FerestreSetup::Runtime, true) => ("Starting the Xbox runtime install…", "Starting…"),
                    (FerestreSetup::MaintainRuntime, false) => ("Update or repair the Xbox runtime.", "Update or repair"),
                    (FerestreSetup::MaintainRuntime, true) => ("Checking the Xbox runtime release…", "Checking…"),
                    (FerestreSetup::MaintainCli, _) => ("Checking the Xbox CLI release…", "Checking…"),
                };
                let (message, label) = if let Some(state) = runtime_progress
                    .filter(|state| runtime_setup && !busy
                        && (state.active() || state.phase == exposed_games_backend::steam_control::Phase::Failed))
                {
                    (state.label(), if state.active() { "Installing…" } else { "Retry runtime install" })
                } else {
                    (message.to_owned(), label)
                };
                d.child(
                    pill().w_full().rounded(rems(1.4)).px(rems(1.)).py(rems(0.7)).gap(rems(0.8))
                        .child(div().flex_1().min_w_0().whitespace_normal().child(message))
                        .child(text_button("configure-ferestre", label).rounded_full().bg(rgba(0xffffff12)).flex_none()
                            .when(action.is_none() && !runtime_active, |d| d.on_click(cx.listener(move |this, _, _, cx| this.configure_ferestre(setup, cx)))))
                        .when(runtime_setup && xbox_link_needed, |d| d.child(
                            text_button("xbox-sign-in-during-runtime",
                                if action == Some(FerestreSetup::SignIn) { "Signing in…" } else { "Sign in to Xbox" })
                                .rounded_full().bg(rgba(0xffffff12)).flex_none()
                                .when(action.is_none(), |d| d.on_click(cx.listener(|this, _, _, cx| {
                                    this.configure_ferestre(FerestreSetup::SignIn, cx);
                                })))))
                        .when(setup == FerestreSetup::Install && !busy, |d| d.child(text_button("nexus-alternative", "Xodus Nexus").rounded_full().bg(rgba(0xffffff12)).flex_none()
                            .on_click(cx.listener(|this, _, _, cx| this.support_project("nexus", cx)))))
                )
            })
            .when(xbox_notice, |d| d.child(
                pill().w_full().rounded(rems(1.4)).px(rems(1.)).py(rems(0.7)).gap(rems(0.8))
                    .child(crate::icons::icon("warning").flex_none().text_color(rgb(0xffcf70)))
                    .child(div().flex_1().min_w_0().whitespace_normal().child("One of your Linux installations is missing Ferestre or Xodus Nexus to run Xbox games."))
                    .child(div().flex().gap(rems(0.5)).flex_none().children(
                        [("ferestre", "Ferestre"), ("nexus", "Xodus Nexus")].into_iter().map(|(project, label)| {
                            text_button(project, label).rounded_full().bg(rgba(0xffffff12))
                                .on_click(cx.listener(move |this, _, _, cx| this.support_project(project, cx)))
                        })
                    ))
            ))
            .children(volumes.into_iter().map(|volume| {
                let id = volume.id.clone();
                let ntfs = volume.filesystem.eq_ignore_ascii_case("ntfs");
                let os = if volume.os == Os::Windows || ntfs {
                    "windows"
                } else {
                    "linux"
                };
                let title = volume.label.as_deref().unwrap_or(match volume.os {
                    Os::Windows => "Windows partition",
                    Os::Linux => "Linux partition",
                    Os::Unknown if ntfs => "NTFS partition",
                    Os::Unknown => "Linux filesystem",
                });
                let failure = mount_failures.get(&id).cloned()
                    .filter(|_| partition_choices.get(&id) == Some(&PartitionChoice::Mount));
                let mut metadata = volume.device.clone();
                if let Some(bytes) = volume.size_bytes {
                    let (size, unit) = if bytes >= 1 << 30 {
                        (bytes as f64 / (1_u64 << 30) as f64, "GiB")
                    } else {
                        (bytes as f64 / (1_u64 << 20) as f64, "MiB")
                    };
                    metadata.push_str(&format!(" · {size:.1} {unit}"));
                }
                pill()
                    .rounded(rems(1.))
                    .px(rems(1.))
                    .py(rems(0.4))
                    .gap(rems(0.8))
                    .child(crate::icons::system_icon(os).size(rems(1.55)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(rems(0.15)).py(rems(0.35))
                        .child(div().text_size(rems(1.125)).child(title.to_owned()))
                        .child(div().text_size(rems(0.95)).text_color(rgb(0xb2c7bc)).whitespace_normal().child(metadata))
                        .when_some(failure, |d, error| d.child(
                            div().id(format!("mount-failure-{}", volume.device))
                                .text_size(rems(0.95))
                                .text_color(rgb(0xffcf70))
                                .aria_description(error.clone())
                                .child("Automatic mount failed")
                                .tooltip(move |_, cx| {
                                    cx.new(|_| crate::library::ActionTooltip(error.clone())).into()
                                })
                        )))
                    .when(self.host == Os::Linux, |d| {
                        d.child(self.partition_actions(&id, &volume.device, cx))
                    })
                    .when(self.host != Os::Linux, |d| {
                        d.child(div().text_size(rems(1.125)).child(volume.note.clone()))
                    })
            }))
    }

    fn partition_actions(&self, id: &str, device: &str, cx: &mut Context<Self>) -> Div {
        let checked = self.remember_partition(id);
        let busy = self.syncing || self.closing;
        let can_remember = rememberable_partition(id);
        let remember_id = id.to_owned();
        let ignore_id = id.to_owned();
        let mount_id = id.to_owned();
        let remember = div()
            .id(format!("remember-{device}"))
            .role(Role::CheckBox)
            .aria_toggled(Toggled::from(checked))
            .aria_disabled(busy || !can_remember)
            .flex()
            .items_center()
            .gap(rems(0.5))
            .px(rems(0.65))
            .py(rems(0.6))
            .rounded(rems(0.7))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(rems(1.1))
                    .rounded(rems(0.25))
                    .border_1()
                    .border_color(rgba(0xffffff80))
                    .when(checked, |d| {
                        d.bg(rgb(0x9ff4bc)).border_color(rgb(0x9ff4bc)).child(
                            crate::icons::icon("check")
                                .size(rems(0.9))
                                .text_color(rgb(0x20342a)),
                        )
                    }),
            )
            .child("Remember")
            .when(!can_remember, |d| {
                d.opacity(0.55).tooltip(|_, cx| {
                    cx.new(|_| {
                        crate::library::ActionTooltip(
                            "This partition has no filesystem UUID".into(),
                        )
                    })
                    .into()
                })
            })
            .when(!busy && can_remember, |d| {
                d.tab_index(0)
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(0xffffff18)))
                    .focus_visible(|s| s.bg(rgba(0xffffff28)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let checked = this.remember_partition(&remember_id);
                        this.remember_partitions
                            .insert(remember_id.clone(), !checked);
                        cx.notify();
                    }))
            });
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(rems(0.45))
            .when(busy, |d| d.opacity(0.55))
            .child(remember)
            .child(
                text_button(format!("ignore-{device}"), "Ignore")
                    .role(Role::Button)
                    .aria_disabled(busy)
                    .when(busy, |d| d.cursor_default())
                    .when(!busy, |d| {
                        d.tab_index(0)
                            .focus_visible(|s| s.bg(rgba(0xffffff28)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ignore_partition(ignore_id.clone(), cx);
                            }))
                    }),
            )
            .child(
                text_button(format!("mount-{device}"), "Mount read-only")
                    .role(Role::Button)
                    .aria_disabled(busy)
                    .when(busy, |d| d.cursor_default())
                    .when(!busy, |d| {
                        d.tab_index(0)
                            .focus_visible(|s| s.bg(rgba(0xffffff28)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mount(mount_id.clone(), cx);
                            }))
                    }),
            )
    }

    pub fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let warnings = self
            .report
            .as_ref()
            .map(|r| r.warnings.clone())
            .unwrap_or_default();
        let automatic = self.backend.automatic_game_updates();
        let xbox = self.report.as_ref().filter(|report| report.host_os == Os::Linux)
            .and_then(|report| report.linux_systems.iter().find(|system| system.root == "/"))
            .filter(|system| system.xbox_runtime && exposed_games_backend::ferestre::setup_supported());
        let xbox_action = self.ferestre_task.as_ref().map(|(action, _)| *action);
        let xbox_runtime_busy = self.download_operations.get(exposed_games_backend::ferestre::RUNTIME_JOB)
            .is_some_and(|progress| progress.active());
        div().absolute().inset_0()
            .child(div().absolute().inset_0().on_mouse_down(MouseButton::Left, cx.listener(|this,_,_,cx| {
                this.menu=false; cx.notify();
            })))
            .child(pill()
            .id("settings-panel")
            .absolute()
            .top(rems(4.))
            .right(rems(5.))
            .rounded(rems(1.))
            .p(rems(1.2))
            .w(rems(27.))
            .max_h(rems(28.))
            .overflow_y_scroll()
            .shadow_lg()
            .flex_col()
            .items_stretch()
            .gap(rems(0.75))
            .child(div().flex().items_center().justify_between()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                .child(crate::header::button("close-settings","close",false).on_click(cx.listener(|this,_,_,cx| { this.menu=false; cx.notify(); }))))
            .child(div().id("automatic-game-updates-setting").flex().w_full().items_center().justify_between().gap(rems(1.))
                .py(rems(0.6)).cursor_pointer()
                .child(div().flex().flex_col().gap(rems(0.3)).flex_1().min_w_0()
                    .child("Automatic game updates")
                    .child(div().text_size(rems(0.95)).text_color(rgb(0xb2c7bc)).whitespace_normal()
                        .child("Start available game updates automatically in Exposed Games.")))
                .child(div().w(rems(2.8)).h(rems(1.6)).flex_none().rounded_full().flex().items_center().p(rems(0.2))
                    .bg(if automatic { rgba(0x9ff4bcff) } else { rgba(0xffffff29) })
                    .when(automatic,|d|d.justify_end())
                    .child(div().size(rems(1.2)).rounded_full().bg(if automatic { rgb(0x20342a) } else { rgb(0xf0f0f0) })))
                .on_click(cx.listener(move |this,_,_,cx| {
                    this.backend.set_automatic_game_updates(!automatic);
                    let backend=this.backend.clone();
                    cx.background_executor().spawn(async move { backend.save_ui(); }).detach();
                    this.check_game_updates(false,cx);
                    cx.notify();
                })))
            .when(xbox.is_some_and(|system| system.managed_ferestre || system.ferestre_client), |d| d.child(
                div().flex().items_center().justify_between()
                    .child("Xbox account")
                    .child(text_button("xbox-sign-in-setting", "Sign in")
                        .when(self.ferestre_task.is_none(), |d| d.on_click(cx.listener(|this, _, _, cx| {
                            this.menu = false;
                            this.configure_ferestre(FerestreSetup::SignIn, cx);
                        }))))
            ))
            .when_some(xbox, |d, system| {
                let (label, action) = if system.managed_ferestre {
                    ("Update or repair", FerestreSetup::MaintainCli)
                } else {
                    ("Install managed CLI", FerestreSetup::Install)
                };
                d.child(
                    div().flex().items_center().justify_between()
                        .child("Xbox CLI")
                        .child(text_button("xbox-cli-setting", if xbox_action == Some(action) { "Working…" } else { label })
                            .when(xbox_action.is_none() && !xbox_runtime_busy, |d| d.on_click(cx.listener(move |this, _, _, cx| {
                                this.configure_ferestre(action, cx);
                            }))))
                )
                .when(system.managed_ferestre, |d| d.child(
                    div().flex().items_center().justify_between()
                        .child("Xbox runtime")
                        .child(text_button("xbox-runtime-setting", if xbox_runtime_busy || xbox_action == Some(FerestreSetup::MaintainRuntime) {
                            "Working…"
                        } else {
                            "Update or repair"
                        })
                            .when(xbox_action.is_none() && !xbox_runtime_busy, |d| d.on_click(cx.listener(|this, _, _, cx| {
                                this.configure_ferestre(FerestreSetup::MaintainRuntime, cx);
                            }))))
                ))
            })
            .child(
                div()
                    .text_size(rems(1.125))
                    .text_color(rgb(0xb2c7bc))
                    .child("Ctrl F to search · Ctrl + / − / 0 for text size"),
            )
            .children(warnings.into_iter().map(|warning| {
                div()
                    .text_size(rems(1.125))
                    .whitespace_normal()
                    .child(warning)
            })))
    }

    pub fn game_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, position)) = &self.game_menu else {
            return div().into_any_element();
        };
        let Some(game) = self.games.iter().find(|g| &g.source.key == key) else {
            return div().into_any_element();
        };
        let busy = self
            .download_operations
            .get(key)
            .is_some_and(|p| p.phase != exposed_games_backend::steam_control::Phase::Failed);
        let play = self.play_button(key);
        let local = !busy && play.is_none() && game.installed(self.library_scope(), self.host) == 1;
        let items = [
            ("launch", "Launch", local && game.source.launch.is_some()),
            ("quit", "Quit game", play == Some(crate::game_activity::PlayButton::Quit) && self.library_scope() != 2),
            (
                "install",
                "Install",
                !busy
                    && self.library_scope() != 2
                    && exposed_games_backend::actions::can_install(&game.source),
            ),
            ("store", "View in store", game.source.store_page.is_some()),
            (
                "uninstall",
                "Uninstall",
                local && game.source.uninstall.is_some(),
            ),
        ];
        let x = f32::from(position.x)
            .min(self.header_state.viewport - self.zoom * 17.)
            .max(0.);
        let y = f32::from(position.y)
            .min(self.viewport_height - self.zoom * 12.)
            .max(self.zoom * 4.);
        div()
            .absolute()
            .inset_0()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.game_menu = None;
                    cx.notify();
                }),
            ))
            .child(
                pill()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(rems(17.))
                    .rounded(rems(1.))
                    .p(rems(0.5))
                    .flex_col()
                    .items_stretch()
                    .children(items.into_iter().filter(|(_, _, enabled)| *enabled).map(
                        |(kind, label, _)| {
                            let key = key.clone();
                            text_button(kind, label).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.action(key.clone(), kind, cx)
                                }),
                            )
                        },
                    )),
            )
            .into_any_element()
    }

    pub fn confirm_uninstall(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = &self.confirm else {
            return div().into_any_element();
        };
        let Some(game) = self.games.iter().find(|g| &g.source.key == key) else {
            return div().into_any_element();
        };
        let key = key.clone();
        let note = game
            .source
            .uninstall
            .as_ref()
            .and_then(|a| a.note.clone())
            .unwrap_or_else(|| {
                if game.source.store == "steam" {
                    "Removes the local game files. The game stays in your Steam library.".into()
                } else {
                    format!(
                        "{} will handle removing this installation.",
                        game.source.client
                    )
                }
            });
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000055))
            .flex()
            .items_center()
            .justify_center()
            .child(
                pill()
                    .flex_col()
                    .items_stretch()
                    .gap(rems(1.))
                    .w(rems(30.))
                    .rounded(rems(1.8))
                    .p(rems(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Uninstall {}?", game.title)),
                    )
                    .child(div().text_size(rems(1.125)).whitespace_normal().child(note))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(rems(0.5))
                            .child(
                                text_button("cancel-uninstall", "Cancel").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                text_button("confirm-uninstall", "Uninstall")
                                    .bg(rgba(0xff8f8f22))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.confirm = None;
                                        this.dispatch(key.clone(), "uninstall", cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
