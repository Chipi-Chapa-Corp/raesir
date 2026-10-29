use crate::icons::Icons;
use crate::{Mock, STORES};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{EditableTextState, text_input};

#[derive(Clone, PartialEq)]
pub struct HeaderState {
    pub store: Option<usize>,
    pub status: u8,
    pub os: u8,
    pub query: String,
    pub menu: bool,
    pub syncing: bool,
    pub zoom: f32,
    pub viewport: f32,
}

pub struct Header {
    pub state: HeaderState,
    pub icons: Icons,
    pub owner: WeakEntity<Mock>,
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
        .bg(rgba(0x102522c9))
        .text_color(rgb(0xf0f8f3))
        .when(std::env::var_os("RAESIR_MOCK_NO_BLUR").is_none(), |d| {
            d.backdrop_blur(px(14.))
        })
}

pub fn icon(icons: &Icons, name: &'static str) -> impl IntoElement + use<> {
    img(icons[name].clone())
        .size(rems(
            if matches!(
                name,
                "steam" | "epic" | "ea" | "ubisoft" | "xbox" | "windows" | "linux"
            ) {
                1.55
            } else {
                1.25
            },
        ))
        .object_fit(ObjectFit::Contain)
}

fn sync_icon(syncing: bool) -> AnyElement {
    let icon = svg()
        .path("icons/sync.svg")
        .size(rems(1.25))
        .text_color(rgb(0xbccfc7));
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

pub fn button(
    icons: &Icons,
    id: impl Into<ElementId>,
    name: &'static str,
    selected: bool,
) -> Stateful<Div> {
    button_shell(id, selected).child(icon(icons, name))
}

fn button_shell(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
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
            rgba(0xa2c7ba29)
        } else {
            rgba(0x00000000)
        })
        .hover(|s| s.bg(rgba(0xc0e1d52b)))
        .transitions(|t| t.bg(millis(130)))
}

impl Header {
    fn listener<F>(&self, callback: F) -> impl Fn(&ClickEvent, &mut Window, &mut App) + use<F>
    where
        F: Fn(&mut Mock, &ClickEvent, &mut Window, &mut Context<Mock>) + 'static,
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
        let search_width = (viewport * 0.24).clamp(self.state.zoom * 14., self.state.zoom * 28.);
        let search_focus = self.search.focus_handle(cx);
        let search_active = search_focus.is_focused(window);
        div()
            .absolute()
            .top(rems(0.7))
            .left(rems(1.1))
            .right(rems(1.1))
            .h(rems(2.9))
            .flex()
            .items_start()
            .justify_between()
            .child(
                pill()
                    .child(
                        button(&self.icons, "all-stores", "all", self.state.store.is_none())
                            .on_click(self.listener(|this, _, _, cx| {
                                this.store = None;
                                this.filter(cx);
                            })),
                    )
                    .children(STORES.iter().enumerate().map(|(i, name)| {
                        button(&self.icons, ("store", i), name, self.state.store == Some(i))
                            .on_click(self.listener(move |this, _, _, cx| {
                                this.store = Some(i);
                                this.filter(cx);
                            }))
                    })),
            )
            .child(
                pill()
                    .id("search-pill")
                    .absolute()
                    .left(px(viewport / 2.
                        - search_width / 2.
                        - self.state.zoom * 1.1))
                    .w(px(search_width))
                    .h(rems(2.9))
                    .px(rems(0.8))
                    .gap(rems(0.65))
                    .when(search_active, |d| d.bg(rgba(0x25473ee8)))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&search_focus, cx);
                    })
                    .child(icon(&self.icons, "search"))
                    .child(
                        text_input("search-games")
                            .state(self.search.downgrade())
                            .placeholder("Search games")
                            .placeholder_color(rgb(0xacbdb5))
                            .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                            .selection_color(rgb_to_hsla(rgba(0x79b7a966)))
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
                    .child(
                        pill().children(["all", "installed", "cloud"].into_iter().enumerate().map(
                            |(i, icon)| {
                                button(
                                    &self.icons,
                                    ("status", i),
                                    icon,
                                    self.state.status == i as u8,
                                )
                                .on_click(self.listener(
                                    move |this, _, _, cx| {
                                        this.status = i as u8;
                                        this.filter(cx);
                                    },
                                ))
                            },
                        )),
                    )
                    .child(
                        pill().children(["all", "this", "other"].into_iter().enumerate().map(
                            |(i, icon)| {
                                button(&self.icons, ("os", i), icon, self.state.os == i as u8)
                                    .on_click(self.listener(move |this, _, _, cx| {
                                        this.os = i as u8;
                                        this.filter(cx);
                                    }))
                            },
                        )),
                    )
                    .child(
                        pill()
                            .child(
                                button_shell("sync", self.state.syncing)
                                    .child(sync_icon(self.state.syncing))
                                    .on_click(self.listener(|this, _, _, cx| this.sync(cx))),
                            )
                            .child(
                                button(&self.icons, "menu", "menu", self.state.menu).on_click(
                                    self.listener(|this, _, _, cx| {
                                        this.menu = !this.menu;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .child(
                        pill()
                            .child(
                                button(&self.icons, "min", "minimize", false)
                                    .on_click(|_, window, _| window.minimize_window()),
                            )
                            .child(
                                button(&self.icons, "max", "maximize", false)
                                    .on_click(|_, window, _| window.zoom_window()),
                            )
                            .child(
                                button(&self.icons, "close", "close", false)
                                    .on_click(|_, window, _| window.remove_window()),
                            ),
                    ),
            )
    }
}
