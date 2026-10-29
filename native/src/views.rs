use crate::{Launcher, header::pill};
use gpui::{prelude::*, *};
use raesir_lib::model::Os;

fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
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
            .gap(rems(0.7))
            .child(
                div()
                    .w_full()
                    .h(px(width * 1.5))
                    .rounded(rems(0.8))
                    .bg(rgba(0xc5dfd51a)),
            )
            .child(
                div()
                    .w(relative(0.7))
                    .h(rems(1.1))
                    .rounded_full()
                    .bg(rgba(0xc5dfd51a)),
            )
    }

    pub fn banners(&self, cx: &mut Context<Self>) -> Div {
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
                    })
                    .collect()
            })
            .unwrap_or_default();
        div()
            .pt(rems(5.))
            .px(rems(1.5))
            .pb(rems(if volumes.is_empty() { 0. } else { 1. }))
            .flex()
            .flex_col()
            .gap(rems(0.5))
            .children(volumes.into_iter().enumerate().map(|(i, volume)| {
                let id = volume.id.clone();
                let os = if volume.os == Os::Windows || volume.filesystem == "ntfs" {
                    "windows"
                } else {
                    "linux"
                };
                pill()
                    .rounded(rems(1.))
                    .px(rems(1.))
                    .py(rems(0.4))
                    .gap(rems(0.8))
                    .child(img(self.icons[os].clone()).size(rems(1.55)))
                    .child(div().flex_1().text_size(rems(1.)).child(format!(
                        "{}{} detected",
                        if volume.os == Os::Unknown {
                            "Possible "
                        } else {
                            ""
                        },
                        if os == "windows" { "Windows" } else { "Linux" }
                    )))
                    .when(self.host == Os::Linux, |d| {
                        d.child(text_button(("mount", i), "Mount").on_click(
                            cx.listener(move |this, _, _, cx| this.mount(id.clone(), cx)),
                        ))
                    })
                    .when(self.host != Os::Linux, |d| {
                        d.child(div().text_size(rems(0.95)).child(volume.note.clone()))
                    })
            }))
    }

    pub fn settings(&self) -> impl IntoElement + use<> {
        let warnings = self
            .report
            .as_ref()
            .map(|r| r.warnings.clone())
            .unwrap_or_default();
        pill()
            .id("settings-panel")
            .absolute()
            .top(rems(4.))
            .right(rems(5.))
            .rounded(rems(1.))
            .p(rems(1.2))
            .w(rems(27.))
            .max_h(rems(28.))
            .overflow_y_scroll()
            .flex_col()
            .items_start()
            .gap(rems(0.75))
            .child("Settings")
            .child(
                div()
                    .text_size(rems(1.))
                    .text_color(rgb(0xb2c7bc))
                    .child("Ctrl F to search · Ctrl + / − / 0 for text size"),
            )
            .children(
                warnings
                    .into_iter()
                    .map(|warning| div().text_size(rems(1.)).whitespace_normal().child(warning)),
            )
    }

    pub fn game_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, position)) = &self.game_menu else {
            return div().into_any_element();
        };
        let Some(game) = self.games.iter().find(|g| &g.source.key == key) else {
            return div().into_any_element();
        };
        let local = game.installed(self.os, self.host) == 1;
        let items = [
            ("launch", "Launch", local && game.source.launch.is_some()),
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
                format!(
                    "{} will handle removing this installation.",
                    game.source.client
                )
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
                    .child(div().text_size(rems(1.)).whitespace_normal().child(note))
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
