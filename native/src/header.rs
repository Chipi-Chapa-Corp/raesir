use crate::icons::icon;
use crate::library::CompatibilityBadge;
use crate::{Launcher, STORE_NAMES, STORES};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{EditableTextState, text_input};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, PartialEq)]
pub struct HeaderState {
    pub store: Option<usize>,
    pub status: u8,
    pub os: u8,
    pub access: u8,
    pub verification: Option<CompatibilityBadge>,
    pub show_verification: bool,
    pub query: String,
    pub menu: bool,
    pub syncing: bool,
    pub downloads: Option<String>,
    pub downloads_open: bool,
    pub zoom: f32,
    pub viewport: f32,
}

const BAR_TOP: f32 = 0.7;
const BAR_MARGIN: f32 = 1.1;
const BAR_HEIGHT: f32 = 2.9;
const PILL_PADDING: f32 = 0.25;
const BUTTON_GAP: f32 = 0.15;
pub(crate) const BUTTON_WIDTH: f32 = 2.25;
const PILL_GAP: f32 = 0.6;
const ROW_GAP: f32 = 0.5;
const MAX_SEARCH_WIDTH: f32 = 28.;

fn pill_width(buttons: usize) -> f32 {
    PILL_PADDING * 2. + buttons as f32 * BUTTON_WIDTH + (buttons - 1) as f32 * BUTTON_GAP
}

#[derive(Debug)]
struct HeaderLayout {
    rows: u8,
    search_width: f32,
    search_gap: f32,
    top_button_width: f32,
    top_gap: f32,
    queue_icon_only: bool,
}

impl HeaderState {
    fn layout(&self) -> HeaderLayout {
        let inner = self.viewport / self.zoom - 2. * BAR_MARGIN;
        let stores = pill_width(STORES.len() + 1);
        let access = pill_width(3);
        let status_and_os = 2. * pill_width(3) + PILL_GAP;
        let verification = if self.show_verification {
            pill_width(CompatibilityBadge::ALL.len() + 1) + PILL_GAP
        } else {
            0.
        };
        let filters = access + PILL_GAP + status_and_os + verification;
        let actions = pill_width(2) + PILL_GAP + pill_width(3);
        let queue_width = self.downloads.as_ref().map_or(0., |label| {
            PILL_GAP
                + if label.is_empty() {
                    2.95
                } else {
                    3.45 + label.len() as f32 * 0.73
                }
        });
        let search_gap = 0.8;
        let rows = if inner
            >= stores + filters + actions + queue_width + 2. * PILL_GAP + 2. * search_gap + 10.
        {
            1
        } else if inner >= filters {
            2
        } else {
            3
        };
        let controls = stores + actions + queue_width;
        let (search_width, top_button_width, top_gap, search_gap, queue_icon_only) = if rows == 1 {
            let width = inner - controls - filters - 2. * PILL_GAP - 2. * search_gap;
            (width, BUTTON_WIDTH, PILL_GAP, search_gap, false)
        } else if inner >= controls + 2. * search_gap + 7. {
            let width = inner - controls - 2. * search_gap;
            (width, BUTTON_WIDTH, PILL_GAP, search_gap, false)
        } else {
            // Keep search and window controls usable at the minimum width, including at high zoom.
            let gap = 0.35;
            let queue_icon = if self.downloads.is_some() {
                2.95 + gap
            } else {
                0.
            };
            let actions = actions - PILL_GAP + gap + queue_icon;
            let available = inner - stores - actions - 2. * gap - 7.;
            let delta = (available / (STORES.len() + 6) as f32).min(0.);
            (7. + available.max(0.), BUTTON_WIDTH + delta, gap, gap, true)
        };
        HeaderLayout {
            rows,
            search_width: search_width.min(MAX_SEARCH_WIDTH),
            search_gap,
            top_button_width,
            top_gap,
            queue_icon_only,
        }
    }

    pub fn bottom(&self) -> f32 {
        BAR_TOP + BAR_HEIGHT + f32::from(self.layout().rows - 1) * (pill_width(1) + ROW_GAP)
    }
}

pub struct Header {
    pub state: HeaderState,
    pub owner: WeakEntity<Launcher>,
    pub downloads_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub settings_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub search: Entity<EditableTextState>,
    pub _focus_subscriptions: [Subscription; 2],
}

pub fn pill() -> Div {
    div()
        .occlude()
        .flex()
        .items_center()
        .gap(rems(BUTTON_GAP))
        .p(rems(PILL_PADDING))
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
        .top(rems(BAR_TOP))
        .left(rems(BAR_MARGIN))
        .right(rems(BAR_MARGIN))
        .h(rems(BAR_HEIGHT))
        .flex()
        .items_start()
        .justify_between()
}

pub(crate) fn back_button(id: &'static str) -> Stateful<Div> {
    pill()
        .id(id)
        .h(rems(BAR_HEIGHT))
        .gap(rems(0.4))
        .px(rems(0.65))
        .cursor_pointer()
        .child(icon("back"))
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

fn sync_icon(syncing: bool, width: f32) -> AnyElement {
    let icon = svg()
        .path("sync")
        .size(rems(1.25_f32.min(width - 0.1)))
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

pub(crate) fn refresh_button(
    id: &'static str,
    label: &'static str,
    width: f32,
    syncing: bool,
) -> Stateful<Div> {
    button_tooltip(
        button_shell(id, syncing)
            .w(rems(width))
            .hover(|s| s.bg(rgba(0xffffff29)))
            .child(sync_icon(syncing, width)),
        label,
    )
}

pub(crate) fn settings_button(
    owner: WeakEntity<Launcher>,
    width: f32,
    open: bool,
    bounds: Option<Rc<Cell<Bounds<Pixels>>>>,
) -> Stateful<Div> {
    sized_button("menu", "menu", open, width)
        .aria_label("Settings")
        .on_prepaint(move |paint, _, _| {
            if let Some(bounds) = &bounds {
                bounds.set(paint.bounds);
            }
        })
        .on_click(move |_, _, cx| {
            owner
                .update(cx, |this, cx| {
                    this.menu = !this.menu;
                    cx.notify();
                })
                .ok();
        })
}

pub(crate) fn window_controls(owner: WeakEntity<Launcher>, width: f32) -> Div {
    pill()
        .child(
            sized_button("min", "minimize", false, width)
                .on_click(|_, window, _| window.minimize_window()),
        )
        .child(
            sized_button("max", "maximize", false, width)
                .on_click(|_, window, _| window.zoom_window()),
        )
        .child(
            sized_button("close", "close", false, width).on_click(move |_, window, cx| {
                owner.update(cx, |this, cx| this.close(window, cx)).ok();
            }),
        )
}

pub fn button(id: impl Into<ElementId>, name: &'static str, selected: bool) -> Stateful<Div> {
    sized_button(id, name, selected, BUTTON_WIDTH)
}

fn sized_button(
    id: impl Into<ElementId>,
    name: &'static str,
    selected: bool,
    width: f32,
) -> Stateful<Div> {
    button_shell(id, selected)
        .w(rems(width))
        .hover(|s| s.bg(rgba(0xffffff29)))
        .child(icon(name).max_w(rems(width - 0.1)).max_h(rems(width - 0.1)))
}

pub fn action_button(
    id: impl Into<ElementId>,
    name: &'static str,
    label: &'static str,
) -> Stateful<Div> {
    button_tooltip(
        item_action(id, false)
            .size(rems(2.6))
            .child(icon(name).text_color(rgb(0xf0f0f0))),
        label,
    )
}

pub(crate) fn item_action(id: impl Into<ElementId>, primary: bool) -> Stateful<Div> {
    button_shell(id, false)
        .role(Role::Button)
        .when(primary, |d| {
            d.text_color(rgb(0x202020))
                .text_size(rems(crate::CARD_TITLE_FONT_SIZE))
                .bg(rgba(0xdcece4f2))
                .hover(|s| s.bg(rgba(0xf0f8f3ff)))
        })
        .when(!primary, |d| {
            d.text_color(rgb(0xf0f0f0))
                .bg(rgba(0x101010b8))
                .hover(|s| s.bg(rgba(0x303030e6)))
        })
}

pub(crate) struct Tooltip(pub String);

impl Render for Tooltip {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let rem = window.rem_size();
        let max_width = (rem * 26.)
            .min(window.viewport_size().width - rem)
            .max(Pixels::ZERO);
        pill()
            .block()
            .rounded(rems(0.8))
            .px(rems(0.85))
            .py(rems(0.55))
            .max_w(max_width)
            .whitespace_normal()
            .text_size(rems(crate::CARD_TITLE_FONT_SIZE))
            .line_height(rems(1.45))
            .child(div().min_w_0().child(self.0.clone()))
    }
}

pub(crate) fn filter_button(
    id: impl Into<ElementId>,
    name: &'static str,
    selected: bool,
    label: &'static str,
) -> Stateful<Div> {
    button_tooltip(button(id, name, selected), label)
}

pub(crate) fn button_tooltip(button: Stateful<Div>, label: impl Into<String>) -> Stateful<Div> {
    let label = label.into();
    button
        .aria_label(label.clone())
        .tooltip_show_delay(millis(400))
        .tooltip(move |_, cx| cx.new(|_| Tooltip(label.clone())).into())
}

pub fn button_shell(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(rems(BUTTON_WIDTH))
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

impl Header {
    fn stores(&self, width: f32) -> Div {
        let all = button_tooltip(
            sized_button("all-stores", "all", self.state.store.is_none(), width),
            "All platforms",
        )
        .on_click(self.listener(|this, _, _, cx| {
            this.store = None;
            this.filter(cx);
        }));
        pill()
            .child(all)
            .children(STORES.iter().enumerate().map(|(i, name)| {
                button_tooltip(
                    sized_button(("store", i), name, self.state.store == Some(i), width),
                    STORE_NAMES[i],
                )
                .on_click(self.listener(move |this, _, _, cx| {
                    this.store = Some(i);
                    this.filter(cx);
                }))
            }))
    }

    fn access(&self) -> Div {
        pill().children(
            [
                ("all", "All games"),
                ("wallet", "Purchased"),
                ("game-pass", "Game Pass"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (icon, label))| {
                filter_button(("access", i), icon, self.state.access == i as u8, label).on_click(
                    self.listener(move |this, _, _, cx| {
                        this.access = i as u8;
                        if i == 2 {
                            this.status = 0;
                            this.os = 0;
                        } else if i == 1 && this.store.is_some_and(|store| STORES[store] == "xbox")
                        {
                            this.status = 0;
                        }
                        this.filter(cx);
                    }),
                )
            }),
        )
    }

    fn installation(&self) -> Div {
        pill().children(
            [
                ("all", "All games"),
                ("installed", "Installed"),
                ("not-installed", "Not installed"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (icon, label))| {
                filter_button(("status", i), icon, self.state.status == i as u8, label).on_click(
                    self.listener(move |this, _, _, cx| {
                        this.status = i as u8;
                        this.filter(cx);
                    }),
                )
            }),
        )
    }

    fn verification(&self) -> Div {
        pill()
            .child(
                filter_button(
                    "all-verification",
                    "all",
                    self.state.verification.is_none(),
                    "All compatibility ratings",
                )
                .on_click(self.listener(|this, _, _, cx| {
                    this.verification = None;
                    this.filter(cx);
                })),
            )
            .children(CompatibilityBadge::ALL.into_iter().map(|badge| {
                button_tooltip(
                    button_shell(badge.key(), self.state.verification == Some(badge))
                        .hover(|s| s.bg(rgba(0xffffff29)))
                        .child(badge.icon()),
                    badge.label(),
                )
                .on_click(self.listener(move |this, _, _, cx| {
                    this.verification = Some(badge);
                    this.filter(cx);
                }))
            }))
    }

    fn os(&self) -> Div {
        pill().children(
            [
                ("all", "All operating systems"),
                ("this", "This OS"),
                ("other", "Other OS"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (icon, label))| {
                filter_button(("os", i), icon, self.state.os == i as u8, label).on_click(
                    self.listener(move |this, _, _, cx| {
                        this.os = i as u8;
                        this.filter(cx);
                    }),
                )
            }),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSize {
    Compact,
    Short,
    Full,
}

impl SearchSize {
    pub fn for_width(width: f32) -> Self {
        if width < 9. {
            Self::Compact
        } else if width < 12. {
            Self::Short
        } else {
            Self::Full
        }
    }
}

pub(crate) fn search_field(
    id: &'static str,
    input_id: &'static str,
    search: &Entity<EditableTextState>,
    size: SearchSize,
    placeholder: &'static str,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let search_focus = search.focus_handle(cx);
    let search_active = search_focus.is_focused(window);
    pill()
        .id(id)
        .flex_1()
        .min_w_0()
        .max_w(rems(MAX_SEARCH_WIDTH))
        .h(rems(BAR_HEIGHT))
        .px(rems(0.8))
        .gap(rems(0.65))
        .when(size == SearchSize::Compact, |d| {
            d.px(rems(0.5)).gap(rems(0.45))
        })
        .when(search_active, |d| d.bg(rgba(0x242424b3)))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&search_focus, cx);
        })
        .child(icon("search"))
        .child(
            text_input(input_id)
                .state(search.downgrade())
                .placeholder(if size != SearchSize::Full {
                    "Search"
                } else {
                    placeholder
                })
                .placeholder_color(rgb(0xacbdb5))
                .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                .caret_blink_interval_500ms()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_x_scroll()
                .text_size(rems(1.125))
                .line_height(rems(1.6))
                .text_color(rgb(0xf0f8f3))
                .px(rems(0.3))
                .py(rems(0.25))
                .rounded_full(),
        )
}

impl Render for Header {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_controls = crate::client_titlebar(window);
        let layout = self.state.layout();
        let top_button_width = layout.top_button_width;
        let wide = layout.rows == 1;
        let show_verification = self.state.show_verification;
        let lower_row = || div().flex().w_full().gap(rems(PILL_GAP)).justify_between();
        bar()
            .h_auto()
            .flex_col()
            .gap(rems(ROW_GAP))
            .child(
                div()
                    .w_full()
                    .h(rems(BAR_HEIGHT))
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(rems(layout.search_gap))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(rems(layout.top_gap))
                            .child(self.stores(top_button_width))
                            .when(wide, |d| d.child(self.access())),
                    )
                    .child(search_field(
                        "search-pill",
                        "search-games",
                        &self.search,
                        SearchSize::for_width(layout.search_width),
                        "Search games",
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(rems(layout.top_gap))
                            .when_some(self.state.downloads.clone(), |d, label| {
                                d.child(
                                    crate::downloads::queue_pill(
                                        if layout.queue_icon_only {
                                            String::new()
                                        } else {
                                            label
                                        },
                                        self.state.downloads_open,
                                        self.downloads_bounds.clone(),
                                    )
                                    .on_click(
                                        self.listener(|this, _, _, cx| this.toggle_downloads(cx)),
                                    ),
                                )
                            })
                            .when(wide, |d| {
                                d.child(self.installation())
                                    .when(show_verification, |d| d.child(self.verification()))
                                    .child(self.os())
                            })
                            .child(
                                pill()
                                    .child(
                                        refresh_button(
                                            "sync",
                                            "Refresh games",
                                            top_button_width,
                                            self.state.syncing,
                                        )
                                        .on_click(self.listener(|this, _, _, cx| this.sync(cx))),
                                    )
                                    .child(settings_button(
                                        self.owner.clone(),
                                        top_button_width,
                                        self.state.menu,
                                        Some(self.settings_bounds.clone()),
                                    )),
                            )
                            .when(show_controls, |d| {
                                d.child(window_controls(self.owner.clone(), top_button_width))
                            }),
                    ),
            )
            .when(layout.rows == 2, |d| {
                d.child(
                    lower_row()
                        .child(self.access())
                        .child(self.installation())
                        .when(show_verification, |d| d.child(self.verification()))
                        .child(self.os()),
                )
            })
            .when(layout.rows == 3, |d| {
                d.child(
                    lower_row()
                        .child(self.access())
                        .when(show_verification, |d| d.child(self.verification())),
                )
                .child(lower_row().child(self.installation()).child(self.os()))
            })
    }
}
