use crate::icons::icon;
use crate::{Launcher, STORES};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{EditableTextState, text_input};

#[derive(Clone, PartialEq)]
pub struct HeaderState {
    pub store: Option<usize>,
    pub status: u8,
    pub os: u8,
    pub access: u8,
    pub query: String,
    pub menu: bool,
    pub syncing: bool,
    pub downloads: Option<String>,
    pub downloads_open: bool,
    pub zoom: f32,
    pub viewport: f32,
}

pub struct Header {
    pub state: HeaderState,
    pub owner: WeakEntity<Launcher>,
    pub downloads_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub search: Entity<EditableTextState>,
    pub _focus_subscriptions: [Subscription; 2],
}

pub fn pill() -> Div {
    div()
        .occlude()
        .flex()
        .items_center()
        .gap(rems(0.15))
        .p(rems(0.25))
        .rounded_full()
        .bg(rgba(0x101010a6))
        .text_color(rgb(0xf0f8f3))
        .when(std::env::var_os("RAESIR_NO_BLUR").is_none(), |d| {
            d.backdrop_blur(px(14.))
        })
}

pub fn bar() -> Div {
    div()
        .absolute()
        .top(rems(0.7))
        .left(rems(1.1))
        .right(rems(1.1))
        .h(rems(2.9))
        .flex()
        .items_start()
        .justify_between()
}

pub fn spinner(id: &'static str, size: f32) -> AnyElement {
    spinner_colored(id, size, 0xf0f0f0)
}

pub fn spinner_colored(id: &'static str, size: f32, color: u32) -> AnyElement {
    icon("spinner")
        .size(rems(size))
        .text_color(rgb(color))
        .with_animation(
            id,
            Animation::new(millis(900)).repeat(),
            |icon, progress| {
                icon.with_transformation(Transformation::rotate(radians(
                    progress * std::f32::consts::TAU,
                )))
            },
        )
        .into_any_element()
}

fn sync_icon(syncing: bool) -> AnyElement {
    let icon = svg()
        .path("sync")
        .size(rems(1.25))
        .text_color(rgb(0xd8d8d8));
    if syncing {
        icon.with_animation(
            "sync-spin",
            Animation::new(millis(900)).repeat(),
            |icon, progress| {
                icon.with_transformation(Transformation::rotate(radians(
                    progress * std::f32::consts::TAU,
                )))
            },
        )
        .into_any_element()
    } else {
        icon.into_any_element()
    }
}

pub fn button(id: impl Into<ElementId>, name: &'static str, selected: bool) -> Stateful<Div> {
    button_shell(id, selected)
        .hover(|s| s.bg(rgba(0xffffff29)))
        .child(icon(name))
}

pub fn action_button(
    id: impl Into<ElementId>,
    name: &'static str,
    label: &'static str,
) -> Stateful<Div> {
    button_shell(id, false)
        .size(rems(2.6))
        .bg(rgba(0x101010b8))
        .hover(|s| s.bg(rgba(0x303030b3)))
        .child(icon(name).text_color(rgb(0xf0f0f0)))
        .tooltip_show_delay(millis(400))
        .tooltip(move |_, cx| cx.new(|_| FilterTooltip(label)).into())
}

struct FilterTooltip(&'static str);

impl Render for FilterTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        pill()
            .px(rems(0.85))
            .py(rems(0.45))
            .bg(rgba(0x101010e6))
            .text_size(rems(1.125))
            .whitespace_nowrap()
            .shadow_sm()
            .child(self.0)
    }
}

fn filter_button(
    id: impl Into<ElementId>,
    name: &'static str,
    selected: bool,
    label: &'static str,
) -> Stateful<Div> {
    button(id, name, selected)
        .tooltip_show_delay(millis(400))
        .tooltip(move |_, cx| cx.new(|_| FilterTooltip(label)).into())
}

pub fn button_shell(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(rems(2.25))
        .flex_none()
        .rounded_full()
        .cursor_pointer()
        .bg(if selected {
            rgba(0xffffff24)
        } else {
            rgba(0x00000000)
        })
        .transitions(|t| t.bg(millis(130)))
}

impl Header {
    fn listener<F>(&self, callback: F) -> impl Fn(&ClickEvent, &mut Window, &mut App) + use<F>
    where
        F: Fn(&mut Launcher, &ClickEvent, &mut Window, &mut Context<Launcher>) + 'static,
    {
        let owner = self.owner.clone();
        move |event, window, cx| {
            owner
                .update(cx, |this, cx| callback(this, event, window, cx))
                .ok();
        }
    }
}

impl Render for Header {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.state.viewport;
        let zoom = self.state.zoom;
        let stores_width = 0.5 + (STORES.len() + 1) as f32 * 2.25 + STORES.len() as f32 * 0.15;
        let access_width = 0.5 + 3. * 2.25 + 2. * 0.15;
        let left = (1.1 + stores_width + 0.6 + access_width + 0.8) * zoom;
        let right_width = 29.75
            + self.state.downloads.as_ref().map_or(0., |label| {
                0.6 + if label.is_empty() {
                    2.95
                } else {
                    3.45 + label.len() as f32 * 0.73
                }
            });
        let right = viewport - (1.1 + right_width + 0.8) * zoom;
        let search_width = (viewport * 0.24)
            .clamp(zoom * 10., zoom * 28.)
            .min((right - left).max(zoom * 6.));
        let search_left = (viewport / 2. - search_width / 2.)
            .max(left)
            .min((right - search_width).max(left));
        let search_focus = self.search.focus_handle(cx);
        let search_active = search_focus.is_focused(window);
        bar()
            .child(
                div()
                    .flex()
                    .gap(rems(0.6))
                    .flex_none()
                    .child(
                        pill()
                            .child(
                                filter_button(
                                    "all-stores",
                                    "all",
                                    self.state.store.is_none(),
                                    "All platforms",
                                )
                                .on_click(self.listener(
                                    |this, _, _, cx| {
                                        this.store = None;
                                        this.filter(cx);
                                    },
                                )),
                            )
                            .children(STORES.iter().enumerate().map(|(i, name)| {
                                filter_button(
                                    ("store", i),
                                    name,
                                    self.state.store == Some(i),
                                    [
                                        "Steam",
                                        "Epic Games",
                                        "EA",
                                        "Ubisoft Connect",
                                        "Xbox",
                                        "Wargaming",
                                    ][i],
                                )
                                .on_click(self.listener(
                                    move |this, _, _, cx| {
                                        this.store = Some(i);
                                        this.filter(cx);
                                    },
                                ))
                            })),
                    )
                    .child(
                        pill().children(
                            ["all", "wallet", "game-pass"].into_iter().enumerate().map(
                                |(i, icon)| {
                                    filter_button(
                                        ("access", i),
                                        icon,
                                        self.state.access == i as u8,
                                        ["All games", "Purchased", "Game Pass"][i],
                                    )
                                    .on_click(self.listener(
                                        move |this, _, _, cx| {
                                            this.access = i as u8;
                                            if i == 2 {
                                                this.status = 0;
                                                this.os = 0;
                                            }
                                            this.filter(cx);
                                        },
                                    ))
                                },
                            ),
                        ),
                    ),
            )
            .child(
                pill()
                    .id("search-pill")
                    .absolute()
                    .left(px(search_left - zoom * 1.1))
                    .w(px(search_width))
                    .h(rems(2.9))
                    .px(rems(0.8))
                    .gap(rems(0.65))
                    .when(search_active, |d| d.bg(rgba(0x242424b3)))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&search_focus, cx);
                    })
                    .child(icon("search"))
                    .child(
                        text_input("search-games")
                            .state(self.search.downgrade())
                            .placeholder("Search games")
                            .placeholder_color(rgb(0xacbdb5))
                            .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                            .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                            .caret_blink_interval_500ms()
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(1.125))
                            .line_height(rems(1.6))
                            .text_color(rgb(0xf0f8f3))
                            .px(rems(0.3))
                            .py(rems(0.25))
                            .rounded_full(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(rems(0.6))
                    .when_some(self.state.downloads.clone(), |d, label| {
                        d.child(
                            crate::downloads::queue_pill(
                                label,
                                self.state.downloads_open,
                                self.downloads_bounds.clone(),
                            )
                            .on_click(self.listener(|this, _, _, cx| this.toggle_downloads(cx))),
                        )
                    })
                    .child(
                        pill().children(
                            ["all", "installed", "not-installed"]
                                .into_iter()
                                .enumerate()
                                .map(|(i, icon)| {
                                    filter_button(
                                        ("status", i),
                                        icon,
                                        self.state.status == i as u8,
                                        ["All games", "Installed", "Not installed"][i],
                                    )
                                    .on_click(self.listener(
                                        move |this, _, _, cx| {
                                            this.status = i as u8;
                                            this.filter(cx);
                                        },
                                    ))
                                }),
                        ),
                    )
                    .child(
                        pill().children(["all", "this", "other"].into_iter().enumerate().map(
                            |(i, icon)| {
                                filter_button(
                                    ("os", i),
                                    icon,
                                    self.state.os == i as u8,
                                    ["All operating systems", "This OS", "Other OS"][i],
                                )
                                .on_click(self.listener(
                                    move |this, _, _, cx| {
                                        this.os = i as u8;
                                        this.filter(cx);
                                    },
                                ))
                            },
                        )),
                    )
                    .child(
                        pill()
                            .child(
                                button_shell("sync", self.state.syncing)
                                    .hover(|s| s.bg(rgba(0xffffff29)))
                                    .child(sync_icon(self.state.syncing))
                                    .on_click(self.listener(|this, _, _, cx| this.sync(cx))),
                            )
                            .child(button("menu", "menu", self.state.menu).on_click(
                                self.listener(|this, _, _, cx| {
                                    this.menu = !this.menu;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        pill()
                            .child(
                                button("min", "minimize", false)
                                    .on_click(|_, window, _| window.minimize_window()),
                            )
                            .child(
                                button("max", "maximize", false)
                                    .on_click(|_, window, _| window.zoom_window()),
                            )
                            .child(button("close", "close", false).on_click(
                                self.listener(|this, _, window, cx| this.close(window, cx)),
                            )),
                    ),
            )
    }
}
