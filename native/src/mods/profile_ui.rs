use super::*;
use crate::details::text_button;

#[derive(Clone)]
pub(super) enum Dialog {
    Create,
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
        name: String,
    },
    Uninstall {
        id: String,
        name: String,
        dependencies: Vec<String>,
    },
}

impl ModBrowser {
    pub(super) fn restore_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.details.is_some() {
            if let Some(owner) = self.owner.upgrade() {
                let focus = owner.read(cx).focus.clone();
                window.focus(&focus, cx);
            }
        } else {
            window.focus(&self.search.focus_handle(cx), cx);
        }
    }

    pub(super) fn open_profile_dialog(
        &mut self,
        mode: Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.installing.is_some() {
            return;
        }
        let name = match &mode {
            Dialog::Rename { name, .. } => name.as_str(),
            _ => "",
        };
        self.profile_name
            .update(cx, |field, cx| field.emplace(name, cx));
        let editing = matches!(mode, Dialog::Create | Dialog::Rename { .. });
        self.dialog = Some(mode);
        self.profiles_open = false;
        self.error = None;
        if editing {
            window.focus(&self.profile_name.focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn profile_menu(
        &self,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().into_any_element();
        };
        let can_delete = snapshot.profiles.len() > 1;
        pill()
            .id("mods-profile-menu")
            .absolute()
            .top(bounds.bottom() + px(8.))
            .left(bounds.left())
            .w(rems(18.))
            .p(rems(0.5))
            .rounded(rems(1.))
            .flex_col()
            .items_stretch()
            .gap_0()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this.profile_bounds.get().contains(&event.position) {
                    this.profiles_open = false;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("profile-choices")
                    .max_h(rems(18.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .children(snapshot.profiles.iter().enumerate().map(|(i, p)| {
                        let id = p.id.clone();
                        let rename = Dialog::Rename {
                            id: p.id.clone(),
                            name: p.name.clone(),
                        };
                        let delete = Dialog::Delete {
                            id: p.id.clone(),
                            name: p.name.clone(),
                        };
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .child(
                                crate::views::text_button(("mod-profile", i), p.name.clone())
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .when(p.id == snapshot.selected, |d| {
                                        d.font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_profile(id.clone(), cx)
                                    })),
                            )
                            .child(
                                button(("rename-mod-profile", i), "edit", false)
                                    .tooltip(|_, cx| {
                                        cx.new(|_| {
                                            crate::library::ActionTooltip("Rename profile".into())
                                        })
                                        .into()
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_profile_dialog(rename.clone(), window, cx)
                                    })),
                            )
                            .child(
                                button(("delete-mod-profile", i), "trash", false)
                                    .opacity(if can_delete { 1. } else { 0.4 })
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| {
                                            crate::library::ActionTooltip(
                                                if can_delete {
                                                    "Delete profile"
                                                } else {
                                                    "Keep at least one profile"
                                                }
                                                .into(),
                                            )
                                        })
                                        .into()
                                    })
                                    .when(can_delete, |d| {
                                        d.on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_profile_dialog(delete.clone(), window, cx)
                                        }))
                                    }),
                            )
                    })),
            )
            .into_any_element()
    }

    fn submit_dialog(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.installing.is_some() {
            return;
        }
        let Some(mode) = self.dialog.clone() else {
            return;
        };
        let (operation, id) = match mode {
            Dialog::Uninstall { id, .. } => {
                self.dialog = None;
                self.change_mod(id, true, cx);
                return;
            }
            Dialog::Create => ("create", String::new()),
            Dialog::Rename { id, .. } => ("rename", id),
            Dialog::Delete { id, .. } => ("delete", id),
        };
        let name = self.profile_name.read(cx).as_str().to_owned();
        let game = self.game.clone();
        let backend = self.backend.clone();
        let owner = self.owner.clone();
        self.loading = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.edit_mod_profile(&game, operation, &id, &name) })
                .await;
            if let Ok((_, report)) = &result {
                owner
                    .update(cx, |owner, cx| owner.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((snapshot, _)) => {
                        this.snapshot = Some(snapshot);
                        this.dialog = None;
                        this.rebuild_rows(cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn profile_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(mode) = &self.dialog else {
            return div().into_any_element();
        };
        let (heading, note, action, editing, destructive) = match mode {
            Dialog::Create => (
                "New profile".to_owned(),
                String::new(),
                "Create",
                true,
                false,
            ),
            Dialog::Rename { name, .. } => {
                (format!("Rename {name}"), String::new(), "Save", true, false)
            }
            Dialog::Delete { name, .. } => (
                format!("Delete {name}?"),
                "Removes this profile and its installed mods.".into(),
                "Delete",
                false,
                true,
            ),
            Dialog::Uninstall {
                name, dependencies, ..
            } => (
                format!("Uninstall {name}?"),
                if dependencies.is_empty() {
                    "Removes this mod from the selected profile.".into()
                } else {
                    format!(
                        "Also removes unused dependencies: {}.",
                        dependencies.join(", ")
                    )
                },
                "Uninstall",
                false,
                true,
            ),
        };
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
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "enter" {
                            this.submit_dialog(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_normal()
                            .child(heading),
                    )
                    .when(editing, |d| {
                        d.child(
                            pill().rounded(rems(0.8)).px(rems(0.8)).py(rems(0.3)).child(
                                text_input("profile-name")
                                    .state(self.profile_name.downgrade())
                                    .placeholder("Profile name")
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(rems(1.125))
                                    .line_height(rems(1.6))
                                    .py(rems(0.4))
                                    .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                                    .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                                    .caret_blink_interval_500ms(),
                            ),
                        )
                    })
                    .when(!note.is_empty(), |d| {
                        d.child(
                            div()
                                .id("profile-action-note")
                                .max_h(rems(15.))
                                .overflow_y_scroll()
                                .text_size(rems(1.125))
                                .whitespace_normal()
                                .child(note),
                        )
                    })
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .text_color(rgb(0xffc4ab))
                                .whitespace_normal()
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(rems(0.5))
                            .child(text_button("cancel-profile-action", "Cancel").when(
                                !self.loading,
                                |d| {
                                    d.on_click(cx.listener(|this, _, window, cx| {
                                        this.dialog = None;
                                        this.error = None;
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    }))
                                },
                            ))
                            .child(
                                text_button("save-profile-action", action)
                                    .when(destructive, |d| d.bg(rgba(0xff8f8f22)))
                                    .when(self.loading, |d| {
                                        d.child(spinner("profile-action-spinner", 1.2))
                                    })
                                    .when(!self.loading, |d| {
                                        d.on_click(
                                            cx.listener(|this, _, _, cx| this.submit_dialog(cx)),
                                        )
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}
