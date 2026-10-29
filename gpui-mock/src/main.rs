use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
};
use std::{sync::Arc, time::Instant};

actions!(raesir, [ClearSearch]);

mod artwork;
mod backdrop;
mod header;
mod icons;
use header::{Header, HeaderState, button, icon, pill};

mod smooth_scroll;
use smooth_scroll::SmoothWheel;
#[cfg(feature = "benchmark")]
mod benchmark;

const STORES: [&str; 5] = ["steam", "epic", "ea", "ubisoft", "xbox"];
const TITLES: &[(&str, &str)] = &[
    ("Among Us", "945360/library_600x900.jpg"),
    (
        "Ancestors: The Humankind Odyssey",
        "536270/library_600x900.jpg",
    ),
    ("ARK: Survival Evolved", "346110/library_600x900.jpg"),
    ("Baba Is You", "736260/library_600x900.jpg"),
    ("Balatro", "2379780/library_600x900.jpg"),
    ("Beat Saber", "620980/library_600x900.jpg"),
    ("Big Walk", "1478500/library_600x900.jpg"),
    ("Bloons TD 6", "960090/library_600x900.jpg"),
    ("Chained Together", "2567870/library_600x900.jpg"),
    ("Content Warning", "2881650/library_600x900.jpg"),
    ("Core Keeper", "1621690/library_600x900.jpg"),
    ("Counter-Strike 2", "730/library_600x900.jpg"),
    ("Crab Game", "1782210/library_600x900.jpg"),
    ("Dead by Daylight", "381210/library_600x900.jpg"),
    ("Deadlock", "1422450/library_600x900.jpg"),
    ("Deep Rock Galactic", "548430/library_600x900.jpg"),
    ("Despotism 3k", "699920/library_600x900.jpg"),
    ("Don't Starve Together", "322330/library_600x900.jpg"),
    ("DOOM Eternal", "782330/library_600x900.jpg"),
    ("Dota 2", "570/library_600x900.jpg"),
    ("Dying Light", "239140/library_600x900.jpg"),
    ("Fall Guys", "1097150/library_600x900.jpg"),
    ("Geometry Dash", "322170/library_600x900.jpg"),
    (
        "Grounded 2",
        "2661300/64f074d18d8f7f0a9a48b7cffb7967193c5ad389/library_capsule.jpg",
    ),
    ("Helltaker", "1289310/library_600x900.jpg"),
    ("Hollow Knight", "367520/library_600x900.jpg"),
    (
        "Minecraft",
        "https://store-images.s-microsoft.com/image/apps.808.14492077886571533.be42f4bd-887b-4430-8ed0-622341b4d2b0.c8274c53-105e-478b-9f4b-41b8088210a3",
    ),
    ("Noita", "881100/library_600x900.jpg"),
    (
        "PEAK",
        "3527290/480bd879ac737921bfa2529a6fea15961267ad21/library_600x900.jpg",
    ),
    ("Portal 2", "620/library_600x900.jpg"),
    ("R.E.P.O.", "3241660/library_600x900.jpg"),
    ("SpiderHeck", "1329500/library_600x900.jpg"),
    ("Stardew Valley", "413150/library_600x900.jpg"),
    ("Terraria", "105600/library_600x900.jpg"),
    ("Timberborn", "1062090/library_600x900.jpg"),
    ("Valheim", "892970/library_600x900.jpg"),
];
fn palette(i: usize) -> (u32, u32) {
    const COLORS: [(u32, u32); 8] = [
        (0x008b94, 0x113244),
        (0x2f8d59, 0x143f4a),
        (0x93612b, 0x652f48),
        (0x2e6eb8, 0x283348),
        (0xb13e53, 0x4f2344),
        (0x687e27, 0x213c4c),
        (0x3977a4, 0x553c76),
        (0xba5e28, 0x364a48),
    ];
    COLORS[i % COLORS.len()]
}

struct Game {
    title: String,
    store: usize,
    installed: u8,
    client_os: u8,
    poster: Option<Arc<RenderImage>>,
}

impl Game {
    fn new(i: usize) -> Self {
        let installed = if i % 5 == 0 {
            1
        } else if i % 4 == 0 {
            2
        } else {
            0
        };
        let client_os = if installed != 0 {
            installed
        } else if i % 7 == 0 {
            2
        } else {
            1
        };
        let title = format!(
            "{}{}",
            TITLES[i % TITLES.len()].0,
            if i < TITLES.len() {
                String::new()
            } else {
                format!(" · {}", i / TITLES.len() + 1)
            }
        );
        Self {
            title,
            store: (i * 7) % 5,
            installed,
            client_os,
            poster: None,
        }
    }
}

struct Mock {
    games: Vec<Game>,
    icons: icons::Icons,
    search: Entity<EditableTextState>,
    _search_subscription: Subscription,
    header: Entity<Header>,
    header_state: HeaderState,
    visible: Vec<usize>,
    rows: ListState,
    columns: usize,
    store: Option<usize>,
    status: u8,
    os: u8,
    query: String,
    focus: FocusHandle,
    backdrop: Entity<backdrop::Backdrop>,
    hovered: Option<usize>,
    hover_task: Option<Task<()>>,
    notice: Option<String>,
    notice_task: Option<Task<()>>,
    menu: bool,
    syncing: bool,
    sync_task: Option<Task<()>>,
    zoom: f32,
    wheel: SmoothWheel,
    wheel_needs_sync: bool,
    frame_pending: bool,
    row_height: f32,
}

impl Mock {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(feature = "benchmark")]
        benchmark::start(window, cx);
        artwork::start(cx);
        eprintln!("GPU: {:?}", window.gpu_specs());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let icons = icons::load(cx);
        let search = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let search_subscription = cx.subscribe(&search, |this, search, _: &TextChanged, cx| {
            this.query = search.read(cx).as_str().to_owned();
            this.filter(cx);
        });
        let header_state = HeaderState {
            store: None,
            status: 0,
            os: 0,
            query: String::new(),
            menu: false,
            syncing: false,
            zoom: 16.,
            viewport: f32::from(window.viewport_size().width),
        };
        let owner = cx.weak_entity();
        let header = cx.new(|cx| {
            let search_focus = search.focus_handle(cx);
            Header {
                state: header_state.clone(),
                icons: icons.clone(),
                owner,
                search: search.clone(),
                _focus_subscriptions: [
                    cx.on_focus(&search_focus, window, |_, _, cx| cx.notify()),
                    cx.on_blur(&search_focus, window, |_, _, cx| cx.notify()),
                ],
            }
        });
        Self {
            games: (0..240).map(Game::new).collect(),
            icons,
            search,
            _search_subscription: search_subscription,
            header,
            header_state,
            visible: (0..240).collect(),
            rows: ListState::new(31, ListAlignment::Top, px(300.)),
            columns: 8,
            store: None,
            status: 0,
            os: 0,
            query: String::new(),
            focus,
            backdrop: cx.new(|_| backdrop::Backdrop::default()),
            hovered: None,
            hover_task: None,
            notice: None,
            notice_task: None,
            menu: false,
            syncing: false,
            sync_task: None,
            zoom: 16.,
            wheel: SmoothWheel::default(),
            wheel_needs_sync: true,
            frame_pending: false,
            row_height: 0.,
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.to_lowercase();
        self.visible = self
            .games
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                self.store.is_none_or(|s| s == g.store)
                    && (self.status == 0 || (self.status == 1) == (g.installed != 0))
                    && (self.os == 0 || g.client_os == self.os)
                    && g.title.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();
        self.reset_rows();
        self.hover_task = None;
        self.hovered = None;
        cx.notify();
    }

    fn reset_rows(&mut self) {
        self.wheel.reset(0.);
        self.wheel_needs_sync = false;
        self.rows.reset_with_uniform_height(
            1 + self.visible.len().div_ceil(self.columns),
            px(self.row_height),
        );
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let position = -f32::from(self.rows.scroll_px_offset_for_scrollbar().y) as f64;
        let ScrollDelta::Lines(lines) = event.delta else {
            self.wheel.reset(position);
            self.wheel_needs_sync = true;
            return;
        };
        if lines.y == 0. || event.modifiers.control {
            return;
        }
        cx.stop_propagation();
        if self.wheel_needs_sync {
            self.wheel.reset(position);
            self.wheel_needs_sync = false;
        }
        let max = f32::from(self.rows.max_offset_for_scrollbar().y) as f64;
        self.wheel.push(
            -lines.y as f64 * self.zoom as f64 * 2.5,
            Instant::now(),
            max,
        );
        self.hover_task = None;
        self.hovered = None;
        self.schedule_scroll(window, cx);
    }

    fn schedule_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wheel.active() || self.frame_pending {
            return;
        }
        self.frame_pending = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.frame_pending = false;
            if !this.wheel.active() {
                return;
            }
            let position = this.wheel.sample(Instant::now());
            let offset = -window.pixel_snap(px(position as f32));
            this.rows.set_offset_from_scrollbar(point(px(0.), offset));
            cx.notify();
            this.schedule_scroll(window, cx);
        });
    }

    fn notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice = Some(text);
        self.notice_task = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(millis(2200)).await;
            this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        self.sync_task = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(millis(1100)).await;
            this.update(cx, |this, cx| {
                this.syncing = false;
                this.notice("Mock library refreshed".into(), cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn hover(&mut self, i: usize, entered: bool, cx: &mut Context<Self>) {
        if entered {
            self.hovered = Some(i);
            self.hover_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(millis(180)).await;
                this.update(cx, |this, cx| {
                    if this.hovered == Some(i) {
                        this.backdrop.update(cx, |backdrop, cx| {
                            if backdrop.retarget(i) {
                                cx.notify();
                            }
                        });
                    }
                })
                .ok();
            }));
        } else if self.hovered == Some(i) {
            self.hovered = None;
            self.hover_task = None;
        }
    }

    fn card(&self, i: usize, width: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let g = &self.games[i];
        let installed_here = g.installed == 1;
        let title = g.title.clone();
        let button_title = title.clone();
        div()
            .id(("card", i))
            .group("game-card")
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .gap(rems(0.5))
            .on_hover(cx.listener(move |this, entered, _, cx| this.hover(i, *entered, cx)))
            .child(
                div()
                    .id(("poster", i))
                    .relative()
                    .w_full()
                    .h(px(width * 1.5))
                    .rounded(rems(0.8))
                    .overflow_hidden()
                    .bg(linear_gradient(
                        130.,
                        linear_color_stop(rgb(palette(i).0), 0.),
                        linear_color_stop(rgb(palette(i).1), 1.),
                    ))
                    .when_some(g.poster.clone(), |d, poster| {
                        d.child(
                            img(poster)
                                .size_full()
                                .rounded(rems(0.8))
                                .object_fit(ObjectFit::Cover)
                                .grayscale(!installed_here),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(rems(0.8))
                            .bg(if installed_here {
                                rgba(0x00000000)
                            } else {
                                rgba(0x020b0755)
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(rems(0.8))
                            .bg(rgba(0xffffff00))
                            .group_hover("game-card", |s| s.bg(rgba(0xffffff16))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(rems(0.5))
                            .left(rems(0.5))
                            .size(rems(1.9))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(0x102522db))
                            .child(icon(&self.icons, STORES[g.store])),
                    )
                    .when(g.installed == 2, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(img(self.icons["windows"].clone()).size(rems(1.8))),
                        )
                    })
                    .when(g.poster.is_none(), |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(rems(2.8))
                                .left(rems(0.85))
                                .right(rems(0.75))
                                .text_size(rems(1.2))
                                .text_center()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xf2f8f3))
                                .child(title.to_uppercase()),
                        )
                    })
                    .when(installed_here, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(rems(1.1))
                                .left(rems(0.8))
                                .size(rems(0.3))
                                .rounded_full()
                                .bg(rgb(0x9ff4bc)),
                        )
                    })
                    .child(
                        button(
                            &self.icons,
                            ("action", i),
                            if installed_here { "play" } else { "external" },
                            false,
                        )
                        .absolute()
                        .bottom(rems(0.55))
                        .right(rems(0.55))
                        .bg(if installed_here {
                            rgba(0xdcece4f2)
                        } else {
                            rgba(0x102522db)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.notice(
                                format!(
                                    "Mock: {} {button_title}",
                                    if installed_here {
                                        "launch"
                                    } else {
                                        "store page for"
                                    }
                                ),
                                cx,
                            );
                        })),
                    ),
            )
            .child(
                div()
                    .h(rems(3.1))
                    .w_full()
                    .overflow_hidden()
                    .text_center()
                    .line_height(rems(1.45))
                    .text_size(rems(1.125))
                    .text_color(if installed_here {
                        rgb(0xe0eae4)
                    } else {
                        rgb(0xa9b5b0)
                    })
                    .child(title),
            )
    }
}

impl Render for Mock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = f32::from(window.viewport_size().width);
        let header_state = HeaderState {
            store: self.store,
            status: self.status,
            os: self.os,
            query: self.query.clone(),
            menu: self.menu,
            syncing: self.syncing,
            zoom: self.zoom,
            viewport,
        };
        if self.header_state != header_state {
            self.header_state = header_state.clone();
            self.header.update(cx, |header, cx| {
                header.state = header_state;
                cx.notify();
            });
        }
        let columns = ((viewport - self.zoom * 3.) / (self.zoom * 12.5))
            .floor()
            .max(2.) as usize;
        let width = (viewport - self.zoom * (3. + 1.25 * (columns - 1) as f32)) / columns as f32;
        let row_height = (width * 1.5 + self.zoom * 4.75).ceil();
        if columns != self.columns || (row_height - self.row_height).abs() > 0.01 {
            self.columns = columns;
            self.row_height = row_height;
            self.reset_rows();
        }
        let this = cx.weak_entity();
        let rows = list(self.rows.clone(), move |row, _, cx| {
            this.update(cx, |this, cx| {
                if row == 0 {
                    return div().h(rems(5.)).into_any_element();
                }
                div()
                    .flex()
                    .w_full()
                    .h(px(row_height))
                    .gap(rems(1.25))
                    .px(rems(1.5))
                    .pb(rems(1.15))
                    .children(
                        this.visible
                            .iter()
                            .skip((row - 1) * columns)
                            .take(columns)
                            .map(|i| this.card(*i, width, cx)),
                    )
                    .into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full();
        let this = cx.weak_entity();
        let scroll_input = canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                        this.update(cx, |this, cx| this.scroll(event, window, cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        div()
            .id("root")
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family("sans-serif")
            .text_size(rems(1.125))
            .text_color(rgb(0xf0f8f3))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClearSearch, window, cx| {
                this.search.update(cx, |search, cx| search.emplace("", cx));
                this.menu = false;
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                #[cfg(feature = "profile")]
                if key.key == "f12" {
                    eprintln!(
                        "Display refresh: {:?} Hz",
                        window
                            .refresh_interval(cx)
                            .map(|interval| 1. / interval.as_secs_f64())
                    );
                    window.cycle_debug_frame_overlay_mode();
                    return;
                }
                if key.modifiers.control {
                    if key.key == "f" {
                        window.focus(&this.search.focus_handle(cx), cx);
                        return;
                    }
                    match key.key.as_str() {
                        "=" | "+" => this.zoom = (this.zoom + 1.).min(22.),
                        "-" => this.zoom = (this.zoom - 1.).max(12.),
                        "0" => this.zoom = 16.,
                        _ => return,
                    }
                    window.set_rem_size(px(this.zoom));
                    this.rows.remeasure_items(0..this.rows.item_count());
                    cx.notify();
                    return;
                }
                if key.key == "escape" {
                    this.search.update(cx, |search, cx| search.emplace("", cx));
                    this.menu = false;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .child(self.backdrop.clone())
            .child(scroll_input)
            .child(rows)
            .when(self.visible.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            pill()
                                .flex_col()
                                .rounded(rems(1.8))
                                .p(rems(2.5))
                                .gap(rems(1.))
                                .child(img(self.icons["empty-search"].clone()).size(rems(2.)))
                                .child("No matching games")
                                .child(
                                    div()
                                        .text_size(rems(1.05))
                                        .text_color(rgb(0xaabdb4))
                                        .child("Try a different filter or search."),
                                ),
                        ),
                )
            })
            .child(self.header.clone())
            .child(
                div()
                    .absolute()
                    .top_0()
                    .w_full()
                    .h(rems(0.5))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
            )
            .when(self.menu, |d| {
                d.child(
                    pill()
                        .absolute()
                        .top(rems(4.))
                        .right(rems(5.))
                        .rounded(rems(1.))
                        .p(rems(1.2))
                        .flex_col()
                        .items_start()
                        .gap(rems(0.6))
                        .child("GPUI · Mock library")
                        .child(
                            div()
                                .text_size(rems(1.))
                                .text_color(rgb(0xb2c7bc))
                                .child(format!(
                                    "{} games · smooth wheel scrolling",
                                    self.visible.len()
                                )),
                        )
                        .child(
                            div()
                                .text_size(rems(1.))
                                .child("Ctrl F to search · Esc to clear · Ctrl + / − / 0"),
                        ),
                )
            })
            .when_some(self.notice.clone(), |d, text| {
                d.child(
                    pill()
                        .absolute()
                        .bottom(rems(1.5))
                        .left(px(viewport / 2. - 220.))
                        .w(px(440.))
                        .p(rems(1.))
                        .justify_center()
                        .child(text),
                )
            })
    }
}

fn main() {
    eprintln!(
        "Raesir GPUI mock: frame-driven smooth wheel scrolling; pill blur {}.",
        if std::env::var_os("RAESIR_MOCK_NO_BLUR").is_some() {
            "disabled"
        } else {
            "enabled"
        }
    );
    gpui_platform::application()
        .with_assets(icons::assets())
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)));
            cx.bind_keys([KeyBinding::new(
                "escape",
                ClearSearch,
                Some(DEFAULT_INPUT_CONTEXT),
            )]);
            let bounds = Bounds::centered(None, size(px(1440.), px(960.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Maximized(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Raesir · GPUI mock".into()),
                        ..Default::default()
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    app_id: Some("raesir.gpui.mock".into()),
                    window_min_size: Some(size(px(1180.), px(700.))),
                    inactive_frame_interval: std::env::var_os("RAESIR_MOCK_LIMIT_BACKGROUND")
                        .map(|_| millis(33)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Mock::new(window, cx)),
            )
            .expect("open mock window");
            cx.activate(true);
        });
}
