use crate::{
    Launcher,
    backend::{PartitionChoice, rememberable_partition},
    header::pill,
    library::FerestreSetup,
};
use raesir_backend::model::Os;
use gpui::{prelude::*, *};

mod card;
pub(crate) use card::{ArtworkCard, CardGrid};

const BANNER_GAP: f32 = 1.;

pub(crate) fn empty_state(
    title: &'static str,
    hint: Option<&'static str>,
    indicator: Option<AnyElement>,
) -> Div {
    div().flex().items_center().justify_center().child(
        pill()
            .flex_col()
            .rounded(rems(1.8))
            .p(rems(2.5))
            .gap(rems(1.))
            .child(indicator.unwrap_or_else(|| {
                crate::icons::icon("empty-search")
                    .size(rems(2.))
                    .into_any_element()
            }))
            .child(title)
            .when_some(hint, |d, hint| {
                d.child(
                    div()
                        .text_size(rems(1.125))
                        .text_color(rgb(0xaabdb4))
                        .child(hint),
                )
            }),
    )
}

pub(crate) fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    text_button_base(id, label)
        .rounded(rems(0.7))
        .hover(|s| s.bg(rgba(0xffffff18)))
}

fn text_button_base(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(rems(0.5))
        .px(rems(1.))
        .py(rems(0.6))
        .cursor_pointer()
        .child(label.into())
}

pub(crate) fn pill_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    text_button_base(id, label)
        .rounded_full()
        .bg(rgba(0x101010b8))
        .hover(|s| s.bg(rgba(0x303030b3)))
}

pub(crate) fn action_banner(message: impl Into<SharedString>) -> Div {
    pill()
        .w_full()
        .rounded(rems(1.4))
        .px(rems(1.))
        .py(rems(0.7))
        .gap(rems(0.8))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(message.into()),
        )
}

pub(crate) fn banner_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    text_button(id, label)
        .rounded_full()
        .bg(rgba(0xffffff12))
        .flex_none()
}

pub(crate) fn modal(width: f32, content: impl FnOnce(Div) -> Div) -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(rgba(0x00000055))
        .flex()
        .items_center()
        .justify_center()
        .child(content(
            pill()
                .flex_col()
                .items_stretch()
                .gap(rems(1.))
                .w(rems(width))
                .max_w(relative(0.95))
                .rounded(rems(1.8))
                .p(rems(2.)),
        ))
}

pub(crate) fn dialog_actions() -> Div {
    div().flex().justify_end().gap(rems(0.5))
}

fn setting_switch(id: &'static str, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(rems(1.))
        .py(rems(0.6))
        .cursor_pointer()
        .child(label)
        .child(
            div()
                .w(rems(2.8))
                .h(rems(1.6))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .p(rems(0.2))
                .bg(if enabled { rgba(0x9ff4bcff) } else { rgba(0xffffff29) })
                .when(enabled, |d| d.justify_end())
                .child(div().size(rems(1.2)).rounded_full().bg(if enabled {
                    rgb(0x20342a)
                } else {
                    rgb(0xf0f0f0)
                })),
        )
}

impl Launcher {
    pub fn banners(&self, cx: &mut Context<Self>) -> Div {
        let scope = self.library_scope();
        let store = self.store.map(|s| crate::STORES[s]);
        let runtime_progress = self
            .download_operations
            .get(raesir_backend::ferestre::RUNTIME_JOB);
        let mut ferestre_setup = self
            .report
            .as_ref()
            .and_then(|report| crate::library::ferestre_setup(report, scope, store));
        if self.host == Os::Linux && store == Some("xbox") && scope != 2
            && (self.ferestre_task.as_ref().is_some_and(|(action, _)| *action == FerestreSetup::MaintainRuntime)
                || runtime_progress.is_some_and(|state| {
                    state.operation == raesir_backend::steam_control::Operation::Update
                        && (state.active() || state.phase == raesir_backend::steam_control::Phase::Failed)
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
        let compact_mount_banners = self.header_state.viewport < self.zoom * 40.;
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
            .pt(rems(self.header_state.bottom() + BANNER_GAP))
            .px(rems(1.5))
            .pb(rems(if volumes.is_empty() && !xbox_notice && ferestre_setup.is_none() { 0. } else { BANNER_GAP }))
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
                        && (state.active() || state.phase == raesir_backend::steam_control::Phase::Failed))
                {
                    (state.label(), if state.active() { "Installing…" } else { "Retry runtime install" })
                } else {
                    (message.to_owned(), label)
                };
                d.child(
                    action_banner(message)
                        .child(banner_button("configure-ferestre", label)
                            .when(action.is_none() && !runtime_active, |d| d.on_click(cx.listener(move |this, _, _, cx| this.configure_ferestre(setup, cx)))))
                        .when(runtime_setup && xbox_link_needed, |d| d.child(
                            banner_button("xbox-sign-in-during-runtime",
                                if action == Some(FerestreSetup::SignIn) { "Signing in…" } else { "Sign in to Xbox" })
                                .when(action.is_none(), |d| d.on_click(cx.listener(|this, _, _, cx| {
                                    this.configure_ferestre(FerestreSetup::SignIn, cx);
                                })))))
                        .when(setup == FerestreSetup::Install && !busy, |d| d.child(banner_button("nexus-alternative", "Xodus Nexus").flex_none()
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
                let pool = !volume.additional_devices.is_empty();
                let os = if volume.os == Os::Windows || ntfs {
                    "windows"
                } else {
                    "linux"
                };
                let title = volume.label.as_deref().unwrap_or(match volume.os {
                    _ if pool => "Btrfs pool",
                    Os::Windows => "Windows partition",
                    Os::Linux => "Linux partition",
                    Os::Unknown if ntfs => "NTFS partition",
                    Os::Unknown => "Linux filesystem",
                });
                let can_mount = self.host == Os::Linux
                    && raesir_backend::mount::mount_options(&volume.filesystem).is_some();
                let failure = mount_failures.get(&id).cloned()
                    .filter(|_| partition_choices.get(&id) == Some(&PartitionChoice::Mount));
                let mut metadata = if pool {
                    Some(format!("{} devices", volume.additional_devices.len() + 1))
                } else if can_mount {
                    volume.label.as_ref().map(|_| volume.device.clone())
                } else {
                    Some(volume.note.clone())
                };
                if let Some(text) = metadata.as_mut()
                    && let Some(bytes) = volume.size_bytes
                {
                    let (size, unit) = if bytes >= 1 << 30 {
                        (bytes as f64 / (1_u64 << 30) as f64, "GiB")
                    } else {
                        (bytes as f64 / (1_u64 << 20) as f64, "MiB")
                    };
                    text.push_str(&format!(" · {size:.1} {unit}"));
                    if pool {
                        text.push_str(" raw");
                    }
                }
                pill()
                    .rounded(rems(1.))
                    .px(rems(1.))
                    .py(rems(0.4))
                    .gap(rems(0.8))
                    .when(compact_mount_banners, |d| d.flex_col().items_stretch().gap(rems(0.15)))
                    .child(
                        div().flex().flex_1().min_w_0().items_center().gap(rems(0.8))
                            .child(crate::icons::system_icon(os).size(rems(1.55)))
                            .child(div().min_w_0().flex().flex_col().gap(rems(0.15)).py(rems(0.35))
                                .child(div().text_size(rems(1.125)).child(title.to_owned()))
                                .when_some(metadata, |d, text| d.child(
                                    div().text_size(rems(0.95)).text_color(rgb(0xb2c7bc)).whitespace_normal().child(text)
                                ))
                                .when_some(failure, |d, error| d.child(
                                    div().id(format!("mount-failure-{}", volume.device))
                                        .text_size(rems(0.95))
                                        .text_color(rgb(0xffcf70))
                                        .aria_description(error.clone())
                                        .child("Automatic mount failed")
                                        .tooltip(move |_, cx| {
                                            cx.new(|_| crate::header::Tooltip(error.clone())).into()
                                        })
                                ))),
                    )
                    .when(self.host == Os::Linux, |d| {
                        d.child(self.partition_actions(&id, &volume.device, can_mount, cx).when(compact_mount_banners, |actions| {
                            actions.flex_wrap().justify_end()
                        }))
                    })
            }))
    }

    fn partition_actions(
        &self,
        id: &str,
        device: &str,
        can_mount: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let checked = self.remember_partition(id);
        let busy = (self.host == Os::Linux && self.syncing) || self.closing;
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
                        crate::header::Tooltip("This partition has no persistent ID".into())
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
                text_button(format!("ignore-{device}"), "Dismiss")
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
            .when(can_mount, |d| {
                d.child(
                    text_button(format!("mount-{device}"), "Mount")
                        .role(Role::Button)
                        .aria_label("Mount read-only")
                        .tooltip(|_, cx| {
                            cx.new(|_| crate::header::Tooltip("Mount read-only".into()))
                                .into()
                        })
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
            })
    }

    pub fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let warnings = self
            .report
            .as_ref()
            .map(|r| r.warnings.clone())
            .unwrap_or_default();
        let xbox = self.report.as_ref().filter(|report| report.host_os == Os::Linux)
            .and_then(|report| report.linux_systems.iter().find(|system| system.root == "/"))
            .filter(|system| system.xbox_runtime && raesir_backend::ferestre::setup_supported());
        let xbox_action = self.ferestre_task.as_ref().map(|(action, _)| *action);
        let xbox_runtime_busy = self.download_operations.get(raesir_backend::ferestre::RUNTIME_JOB)
            .is_some_and(|progress| progress.active());
        div().absolute().inset_0()
            .child(pill()
            .id("settings-panel")
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left
                    && !this.settings_bounds.get().contains(&event.position)
                {
                    this.menu = false;
                    cx.notify();
                }
            }))
            .absolute()
            .top(rems(4.))
            .right(rems(5.))
            .rounded(rems(1.))
            .p(rems(1.2))
            .w(rems(27.))
            .max_w(px(self.header_state.viewport - self.zoom * 6.1))
            .max_h(rems(28.))
            .overflow_y_scroll()
            .shadow_lg()
            .flex_col()
            .items_stretch()
            .gap(rems(0.75))
            .child(div().flex().items_center().justify_between()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                .child(crate::header::button("close-settings","close",false).on_click(cx.listener(|this,_,_,cx| { this.menu=false; cx.notify(); }))))
            .child(setting_switch("automatic-game-updates-setting", "Automatic game updates", self.backend.automatic_game_updates())
                .on_click(cx.listener(|this, _, _, cx| {
                    let backend = this.backend.clone();
                    backend.set_automatic_game_updates(!backend.automatic_game_updates());
                    cx.background_executor().spawn(async move { backend.save_ui(); }).detach();
                    this.check_game_updates(false,cx);
                    cx.notify();
                })))
            .child(setting_switch("exit-with-game-setting", "Exit with game", self.backend.exit_with_game())
                .on_click(cx.listener(|this, _, _, cx| {
                    let backend = this.backend.clone();
                    backend.set_exit_with_game(!backend.exit_with_game());
                    cx.background_executor().spawn(async move { backend.save_ui(); }).detach();
                    cx.notify();
                })))
            .when(raesir_backend::mods::nexus_account::configured(), |d| {
                d.child(self.nexus_account_settings(cx))
            })
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
            .is_some_and(|p| p.phase != raesir_backend::steam_control::Phase::Failed);
        let transferring = self
            .download_operations
            .get(key)
            .is_some_and(|p| p.active());
        let play = self.play_button(key);
        let local = play.is_none() && game.installed(self.library_scope(), self.host) == 1;
        let items = [
            (
                "launch",
                "Launch",
                local && !busy && game.source.launch.is_some(),
            ),
            (
                "quit",
                "Quit game",
                play == Some(crate::game_activity::PlayButton::Quit) && self.library_scope() != 2,
            ),
            (
                "install",
                "Install",
                !busy
                    && self.library_scope() != 2
                    && raesir_backend::actions::can_install(&game.source),
            ),
            ("store", "View in store", game.source.store_page.is_some()),
            (
                "uninstall",
                "Uninstall",
                local && !transferring && game.source.uninstall.is_some(),
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
                        raesir_backend::catalog::client_label(&game.source.client)
                    )
                }
            });
        modal(30., |panel| {
            panel
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Uninstall {}?", game.title)),
                )
                .child(div().text_size(rems(1.125)).whitespace_normal().child(note))
                .child(
                    dialog_actions()
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
                )
        })
        .into_any_element()
    }
}
