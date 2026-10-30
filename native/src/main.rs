#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
};
use library::Game;
use raesir_lib::model::{Os, Report};
use std::{collections::HashSet, sync::Arc, time::Instant};

actions!(raesir, [ClearSearch]);

mod artwork;
mod backdrop;
mod backend;
mod controller;
mod header;
mod icons;
mod library;
mod views;
use header::{Header, HeaderState, button, pill};
use icons::icon;

mod smooth_scroll;
use smooth_scroll::SmoothWheel;
#[cfg(feature = "benchmark")]
mod benchmark;

const STORES: [&str; 5] = ["steam", "epic", "ea", "ubisoft", "xbox"];

struct Launcher {
    backend: Arc<backend::Backend>,
    report: Option<Arc<Report>>,
    host: Os,
    viewport_height: f32,
    starting: bool,
    closing: bool,
    load_error: Option<String>,
    load_task: Option<Task<()>>,
    _activation: Subscription,
    last_refresh: Instant,
    pending_action: Option<String>,
    game_menu: Option<(String, Point<Pixels>)>,
    confirm: Option<String>,
    artwork_pending: HashSet<String>,
    artwork_attempted: HashSet<String>,
    games: Vec<Game>,
    available: u8,
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

impl Launcher {
    fn new(backend: Arc<backend::Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(feature = "benchmark")]
        benchmark::start(window, cx);
        let zoom = backend.zoom();
        window.set_rem_size(px(zoom));
        eprintln!("GPU: {:?}", window.gpu_specs());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let search = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let search_subscription = cx.subscribe(&search, |this, search, _: &TextChanged, cx| {
            this.query = search.read(cx).as_str().to_owned();
            this.filter(cx);
        });
        let header_state = HeaderState {
            store: None,
            status: 1,
            os: 0,
            query: String::new(),
            menu: false,
            syncing: false,
            available: 0,
            zoom,
            viewport: f32::from(window.viewport_size().width),
        };
        let owner = cx.weak_entity();
        let header = cx.new(|cx| {
            let search_focus = search.focus_handle(cx);
            Header {
                state: header_state.clone(),
                owner,
                search: search.clone(),
                _focus_subscriptions: [
                    cx.on_focus(&search_focus, window, |_, _, cx| cx.notify()),
                    cx.on_blur(&search_focus, window, |_, _, cx| cx.notify()),
                ],
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh_on_focus(cx);
            }
        });
        Self {
            backend,
            report: None,
            host: Os::host(),
            viewport_height: f32::from(window.viewport_size().height),
            starting: true,
            closing: false,
            load_error: None,
            load_task: None,
            _activation: activation,
            last_refresh: Instant::now(),
            pending_action: None,
            game_menu: None,
            confirm: None,
            artwork_pending: HashSet::new(),
            artwork_attempted: HashSet::new(),
            games: vec![],
            available: 0,
            search,
            _search_subscription: search_subscription,
            header,
            header_state,
            visible: vec![],
            rows: ListState::new(31, ListAlignment::Top, px(300.)),
            columns: 8,
            store: None,
            status: 1,
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
            zoom,
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
            .filter(|(_, g)| g.matches(&query, self.store, self.status, self.os, self.host))
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
            1 + (if self.starting && self.games.is_empty() {
                24
            } else {
                self.visible.len()
            })
            .div_ceil(self.columns),
            px(self.row_height),
        );
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu || self.game_menu.is_some() || self.confirm.is_some() || self.closing {
            return;
        }
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
            cx.background_executor().timer(millis(6500)).await;
            this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
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
                            if let Some(colors) = this.games[i].colors
                                && backdrop.retarget(colors)
                            {
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
        let colors = g.colors.unwrap_or(backdrop::DEFAULT_COLORS);
        let installed = g.installed(self.os, self.host);
        let installed_here = installed == 1;
        let playable = installed_here && g.source.launch.is_some();
        let key = g.key.clone();
        let menu_key = key.clone();
        let title = g.title.clone();

        div()
            .id(("card", i))
            .group("game-card")
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .gap(rems(0.5))
            .on_hover(cx.listener(move |this, entered, _, cx| this.hover(i, *entered, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.game_menu = Some((menu_key.to_string(), event.position));
                    cx.notify();
                }),
            )
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
                        linear_color_stop(rgb(colors[0]), 0.),
                        linear_color_stop(rgb(colors[1]), 1.),
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
                            .child(icon(STORES[g.store])),
                    )
                    .when(
                        !installed_here && self.os != 1 && g.other_os(self.host).is_some(),
                        |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        icon(if g.other_os(self.host) == Some(Os::Linux) {
                                            "linux"
                                        } else {
                                            "windows"
                                        })
                                        .size(rems(1.8)),
                                    ),
                            )
                        },
                    )
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
                            ("action", i),
                            if playable { "play" } else { "external" },
                            false,
                        )
                        .absolute()
                        .bottom(rems(0.55))
                        .right(rems(0.55))
                        .bg(if playable {
                            rgba(0xdcece4f2)
                        } else {
                            rgba(0x102522db)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.action(
                                key.to_string(),
                                if playable { "launch" } else { "store" },
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

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = f32::from(window.viewport_size().width);
        self.viewport_height = f32::from(window.viewport_size().height);
        let header_state = HeaderState {
            store: self.store,
            status: self.status,
            os: self.os,
            query: self.query.clone(),
            menu: self.menu,
            syncing: self.syncing,
            available: self.available,
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
                    return this.banners(cx).into_any_element();
                }
                let indices: Vec<_> = this
                    .visible
                    .iter()
                    .skip((row - 1) * columns)
                    .take(columns)
                    .copied()
                    .collect();
                for &i in &indices {
                    artwork::request(this, i, cx);
                }
                div()
                    .flex()
                    .w_full()
                    .h(px(row_height))
                    .gap(rems(1.25))
                    .px(rems(1.5))
                    .pb(rems(1.15))
                    .when(this.starting && this.games.is_empty(), |d| {
                        d.children((0..columns).map(|_| this.skeleton(width)))
                    })
                    .children(indices.into_iter().map(|i| this.card(i, width, cx)))
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
                this.game_menu = None;
                this.confirm = None;
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
                    this.save_zoom(cx);
                    cx.notify();
                    return;
                }
                if key.key == "escape" {
                    this.search.update(cx, |search, cx| search.emplace("", cx));
                    this.menu = false;
                    this.game_menu = None;
                    this.confirm = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .child(self.backdrop.clone())
            .child(scroll_input)
            .child(rows)
            .when(!self.starting && self.visible.is_empty(), |d| {
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
                                .child(icon("empty-search").size(rems(2.)))
                                .child(if self.load_error.is_some() {
                                    "Could not load library"
                                } else if self.games.is_empty() {
                                    "No games found"
                                } else {
                                    "No matching games"
                                })
                                .child(
                                    div().text_size(rems(1.05)).text_color(rgb(0xaabdb4)).child(
                                        if self.load_error.is_some() {
                                            "Refresh to try again."
                                        } else if self.games.is_empty() {
                                            "Refresh after signing in to a supported client."
                                        } else {
                                            "Try a different filter or search."
                                        },
                                    ),
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
            .when(self.menu, |d| d.child(self.settings()))
            .when(self.game_menu.is_some(), |d| d.child(self.game_actions(cx)))
            .when(self.confirm.is_some(), |d| {
                d.child(self.confirm_uninstall(cx))
            })
            .when(self.closing, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            pill()
                                .p(rems(2.))
                                .child("Closing clients and releasing mounts…"),
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
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 4 && args[1] == "--steam-library-probe" {
        raesir_lib::steam_ipc::helper(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
        );
        return;
    }
    let backend = backend::Backend::new();
    let app_backend = backend.clone();
    eprintln!(
        "Raesir: native GPUI; pill blur {}.",
        if std::env::var_os("RAESIR_MOCK_NO_BLUR").is_some() {
            "disabled"
        } else {
            "enabled"
        }
    );
    gpui_platform::application()
        .with_assets(icons::assets())
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
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
                        title: Some("Raesir".into()),
                        ..Default::default()
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    app_id: Some("dev.raesir.launcher".into()),
                    window_min_size: Some(size(px(1180.), px(700.))),
                    inactive_frame_interval: std::env::var_os("RAESIR_MOCK_LIMIT_BACKGROUND")
                        .map(|_| millis(33)),
                    ..Default::default()
                },
                |window, cx| {
                    let root = cx.new(|cx| Launcher::new(app_backend.clone(), window, cx));
                    let weak = root.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        weak.update(cx, |this, cx| this.close(window, cx)).ok();
                        false
                    });
                    root.update(cx, |this, cx| this.start(cx));
                    root
                },
            )
            .expect("open Raesir window");
            cx.activate(true);
        });
    if let Err(error) = backend.shutdown() {
        eprintln!("{error}");
    }
}
