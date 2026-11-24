#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use raesir_backend::model::{Os, Report};
use game_activity::{PlayButton, game_action_button};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
};
use library::{CompatibilityBadge, Game};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

actions!(raesir, [ClearSearch]);

mod artwork;
mod backdrop;
mod backend;
mod compatibility;
mod controller;
mod details;
mod downloads;
mod game_activity;
mod header;
mod icons;
mod library;
mod nexus_account;
mod recent;
mod video;
mod views;
use header::{Header, HeaderState, action_button, pill};
use icons::{icon, system_icon};

mod mods;
mod smooth_scroll;
use smooth_scroll::SmoothWheel;
#[cfg(feature = "benchmark")]
mod benchmark;

const STORES: [&str; 6] = ["steam", "epic", "ea", "ubisoft", "xbox", "wargaming"];
const STORE_NAMES: [&str; 6] = [
    "Steam",
    "Epic Games",
    "EA",
    "Ubisoft Connect",
    "Xbox",
    "Wargaming",
];
const CARD_TITLE_HEIGHT: f32 = 4.;
const CARD_TITLE_FONT_SIZE: f32 = 1.125;
const CARD_TITLE_OVERLAP: f32 = 0.08;
const CARD_POSTER_RATIO: f32 = 1.5;

struct Launcher {
    backend: Arc<backend::Backend>,
    report: Option<Arc<Report>>,
    host: Os,
    viewport_height: f32,
    starting: bool,
    closing: bool,
    load_error: Option<String>,
    load_task: Option<Task<()>>,
    ferestre_task: Option<(library::FerestreSetup, Task<()>)>,
    nexus_account: nexus_account::NexusAccount,
    _activation: Subscription,
    last_refresh: Instant,
    pending_action: Option<String>,
    game_activity: HashMap<String, game_activity::GameSession>,
    game_activity_task: Option<Task<()>>,
    window: AnyWindowHandle,
    download_operations: HashMap<String, raesir_backend::steam_control::Progress>,
    download_rates: downloads::DownloadRates,
    downloads_watching: bool,
    mods_watching: bool,
    mod_update_task: Option<Task<()>>,
    game_update_task: Option<Task<()>>,
    game_update_notice: Option<String>,
    mod_update_notice: Option<String>,
    downloads_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    settings_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    download_rows: ListState,
    downloads_open: bool,
    downloads_visible: HashSet<String>,
    download_failures_seen: HashMap<String, raesir_backend::steam_control::Progress>,
    download_failures_dismissed: HashMap<String, raesir_backend::steam_control::Progress>,
    game_menu: Option<(String, Point<Pixels>)>,
    details: Option<Entity<details::DetailsPage>>,
    mods: Option<Entity<mods::ModBrowser>>,
    confirm: Option<String>,
    artwork_pending: HashMap<String, artwork::Job>,
    artwork_attempted: HashSet<String>,
    artwork_cache: artwork::Cache,
    compatibility: compatibility::Ratings,
    games: Vec<Game>,
    search: Entity<EditableTextState>,
    _search_subscription: Subscription,
    header: Entity<Header>,
    header_state: HeaderState,
    visible: Vec<usize>,
    matching_games: Vec<usize>,
    rows: ListState,
    columns: usize,
    store: Option<usize>,
    status: u8,
    os: u8,
    access: u8,
    verification: Option<CompatibilityBadge>,
    saved_filters: backend::LibraryFilters,
    query: String,
    focus: FocusHandle,
    backdrop: Entity<backdrop::Backdrop>,
    background_pending: bool,
    hovered: Option<usize>,
    hover_task: Option<Task<()>>,
    notice: Option<String>,
    notice_task: Option<Task<()>>,
    menu: bool,
    syncing: bool,
    sync_task: Option<Task<()>>,
    ignored_partitions: HashSet<String>,
    remember_partitions: HashMap<String, bool>,
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
        let saved_filters = backend.filters();
        let backend::LibraryFilters {
            status,
            os,
            store,
            access,
            verification,
        } = saved_filters;
        let host = Os::host();
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
            store,
            status,
            os,
            access,
            verification,
            show_verification: host == Os::Linux,
            query: String::new(),
            menu: false,
            syncing: false,
            downloads: None,
            downloads_open: false,
            zoom,
            viewport: f32::from(window.viewport_size().width),
        };
        let owner = cx.weak_entity();
        let downloads_bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
        let settings_bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
        let header = cx.new(|cx| {
            let search_focus = search.focus_handle(cx);
            Header {
                state: header_state.clone(),
                owner,
                downloads_bounds: downloads_bounds.clone(),
                settings_bounds: settings_bounds.clone(),
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
            host,
            viewport_height: f32::from(window.viewport_size().height),
            starting: true,
            closing: false,
            load_error: None,
            load_task: None,
            ferestre_task: None,
            nexus_account: Default::default(),
            _activation: activation,
            last_refresh: Instant::now(),
            pending_action: None,
            game_activity: HashMap::new(),
            game_activity_task: None,
            window: window.window_handle(),
            download_operations: HashMap::new(),
            download_rates: downloads::DownloadRates::default(),
            downloads_watching: false,
            mods_watching: false,
            mod_update_task: None,
            game_update_task: None,
            game_update_notice: None,
            mod_update_notice: None,
            downloads_bounds,
            settings_bounds,
            download_rows: ListState::new(0, ListAlignment::Top, px(600.)),
            downloads_open: false,
            downloads_visible: HashSet::new(),
            download_failures_seen: HashMap::new(),
            download_failures_dismissed: HashMap::new(),
            game_menu: None,
            details: None,
            mods: None,
            confirm: None,
            artwork_pending: HashMap::new(),
            artwork_attempted: HashSet::new(),
            artwork_cache: artwork::Cache::default(),
            compatibility: compatibility::Ratings::default(),
            games: vec![],
            search,
            _search_subscription: search_subscription,
            header,
            header_state,
            visible: vec![],
            matching_games: vec![],
            rows: ListState::new(31, ListAlignment::Top, px(300.)),
            columns: 8,
            store,
            status,
            os,
            access,
            verification,
            saved_filters,
            query: String::new(),
            focus,
            backdrop: cx.new(|_| backdrop::Backdrop::default()),
            background_pending: false,
            hovered: None,
            hover_task: None,
            notice: None,
            notice_task: None,
            menu: false,
            syncing: false,
            sync_task: None,
            ignored_partitions: HashSet::new(),
            remember_partitions: HashMap::new(),
            zoom,
            wheel: SmoothWheel::default(),
            wheel_needs_sync: true,
            frame_pending: false,
            row_height: 0.,
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        if self.store.is_some_and(|store| STORES[store] == "xbox")
            && self.status == 1
            && self.access == 1
        {
            self.access = 0;
        }
        let filters = backend::LibraryFilters {
            status: self.status,
            os: self.os,
            store: self.store,
            access: self.access,
            verification: self.verification,
        };
        if filters != self.saved_filters {
            self.saved_filters = filters;
            self.backend.set_filters(filters);
            let backend = self.backend.clone();
            cx.background_executor()
                .spawn(async move { backend.save_ui() })
                .detach();
        }
        self.matching_games = self.matching_games_for_filters();
        self.visible = self.sorted_matches();
        self.compatibility.scan_offset = 0;
        compatibility::request_filtered(self, cx);
        self.reset_rows();
        self.hover_task = None;
        self.hovered = None;
        cx.notify();
    }

    fn matching_games_for_filters(&self) -> Vec<usize> {
        library::matching_games(
            &self.games,
            &self.query.trim().to_lowercase(),
            self.saved_filters,
            self.host,
        )
    }

    fn refresh_title_matches(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let matching_games = self.matching_games_for_filters();
        if matching_games != self.matching_games {
            self.matching_games = matching_games;
            self.compatibility.scan_offset = 0;
            compatibility::request_filtered(self, cx);
        }
        self.update_visible_matches();
        cx.notify();
    }

    fn active_verification(&self) -> Option<CompatibilityBadge> {
        self.verification.filter(|_| self.host == Os::Linux)
    }

    fn sorted_matches(&self) -> Vec<usize> {
        let selected = self.active_verification();
        let mut visible: Vec<_> = self
            .matching_games
            .iter()
            .copied()
            .filter(|&i| {
                selected.is_none_or(|badge| {
                    self.compatibility.matches_badge(&self.games[i], self.host, badge)
                })
            })
            .collect();
        library::sort_matches(&mut visible, &self.games, self.host, |game| {
            self.compatibility.matches_badge(game, self.host, CompatibilityBadge::Tick)
        });
        visible
    }

    fn update_visible_matches(&mut self) {
        let visible = self.sorted_matches();
        if visible != self.visible {
            let mut scroll = self.rows.logical_scroll_top();
            if scroll.item_ix > 0
                && let Some(anchor) = self.visible.get((scroll.item_ix - 1) * self.columns)
                && let Some(index) = visible.iter().position(|index| index == anchor)
            {
                scroll.item_ix = 1 + index / self.columns;
            }
            self.visible = visible;
            self.reset_rows();
            self.rows.scroll_to(scroll);
            self.wheel_needs_sync = true;
            if self
                .hovered
                .is_some_and(|i| !self.visible.contains(&i))
            {
                self.hover_task = None;
                self.hovered = None;
            }
        }
    }

    fn library_scope(&self) -> u8 {
        if self.query.trim().is_empty() {
            self.os
        } else {
            0
        }
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
        if self.details.is_some()
            || self.mods.is_some()
            || self.menu
            || self.downloads_open
            || self.game_menu.is_some()
            || self.confirm.is_some()
            || self.closing
        {
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
                        if let Some(image) = this.games[i].background.clone() {
                            this.backdrop
                                .update(cx, |backdrop, cx| backdrop.retarget(image, cx));
                        }
                        this.load_background(i, cx);
                    }
                })
                .ok();
            }));
        } else if self.hovered == Some(i) {
            self.hovered = None;
            self.hover_task = None;
        }
    }

    fn load_background(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.background_pending || self.games[index].background_checked {
            return;
        }
        self.background_pending = true;
        let game = self.games[index].source.clone();
        let key = game.key.clone();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backdrop::load(&backend, &game) })
                .await;
            this.update(cx, |this, cx| {
                this.background_pending = false;
                if let Some(game) = this.games.iter_mut().find(|game| game.source.key == key) {
                    game.background_checked = true;
                    if let Ok(image) = result {
                        game.background = Some(image);
                    }
                }
                if let Some(index) = this.hovered {
                    this.hover(index, true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn card(&self, i: usize, width: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let g = &self.games[i];
        let scope = self.library_scope();
        let installed = g.installed(scope, self.host);
        let installed_here = installed == 1;
        let playable = installed_here && g.source.launch.is_some();
        let modded = playable && g.source.mods.as_ref().is_some_and(|m| m.launchable);
        let installable = scope != 2 && raesir_backend::actions::can_install(&g.source);
        let play_button = (scope != 2)
            .then(|| self.play_button(&g.source.key))
            .flatten();
        let unsupported = g.source.ferestre_unsupported_package;
        let action = if playable { "launch" } else { "install" };
        let progress = self.display_progress(&g.source.key);
        let working = progress
            .as_ref()
            .filter(|p| play_button.is_none() && !unsupported && downloads::has_primary_action(p));
        let can_uninstall = installed_here
            && g.source.uninstall.is_some()
            && !progress.as_ref().is_some_and(|p| p.active())
            && play_button.is_none();
        let error = progress.as_ref().and_then(|p| p.error);
        let action_tip = play_button
            .map(|state| state.label().into())
            .or_else(|| {
                error
                    .filter(|e| *e != raesir_backend::steam_control::Error::XboxSignInRequired)
                    .map(|e| e.message().to_owned())
            })
            .or_else(|| {
                if let Some(progress) = working {
                    Some(downloads::button_tip(progress))
                } else if installable {
                    Some(
                        g.source
                            .install
                            .as_ref()
                            .and_then(|a| a.note.clone())
                            .unwrap_or_else(|| "Install".into()),
                    )
                } else if playable {
                    Some("Play".into())
                } else {
                    None
                }
            });
        let duplicate_systems = g.duplicate_systems(self.host);
        let compatibility_icon = self.compatibility.card_badge(g, self.host);
        let key = g.key.clone();
        let uninstall_key = key.clone();
        let modded_key = key.clone();
        let menu_key = key.clone();
        let details_key = key.clone();
        let has_primary = playable || installable || working.is_some() || play_button.is_some();
        let show_modded = modded && working.is_none() && play_button.is_none();
        let badges = div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .top(rems(0.5))
                    .left(rems(0.5))
                    .flex()
                    .items_center()
                    .gap(rems(0.3))
                    .child(
                        div()
                            .size(rems(1.9))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(0x101010b3))
                            .child(icon(STORES[g.store])),
                    )
                    .when_some(compatibility_icon, |d, (badge, reason)| {
                        d.child(
                            div()
                                .id(("linux-compatibility", i))
                                .size(rems(1.9))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgba(0x101010b3))
                                .child(badge.icon().size(rems(1.4)))
                                .tooltip(move |_, cx| {
                                    cx.new(|_| crate::header::Tooltip(reason.into())).into()
                                }),
                        )
                    }),
            )
            .when(!duplicate_systems.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(rems(0.5))
                        .right(rems(0.5))
                        .h(rems(1.9))
                        .px(rems(0.5))
                        .gap(rems(0.4))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(0xbc303d))
                        .child(icon("warning").size(rems(1.1)).text_color(rgb(0xffffff)))
                        .children(duplicate_systems.into_iter().map(|os| {
                            system_icon(if os == Os::Linux { "linux" } else { "windows" })
                                .size(rems(1.3))
                        })),
                )
            })
            .when_some(
                (installed == 2)
                    .then(|| g.foreign_install_badge(self.host))
                    .flatten(),
                |d, os| {
                    let (icon, hint) = if os == Os::Linux {
                        ("linux", "Installed on Linux")
                    } else {
                        ("windows", "Installed on Windows")
                    };
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id(("foreign-install", i))
                                    .flex()
                                    .child(system_icon(icon).size(rems(1.8)))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| crate::header::Tooltip(hint.into())).into()
                                    }),
                            ),
                    )
                },
            );
        let actions = (can_uninstall || show_modded || has_primary).then(|| {
            div()
                .when(can_uninstall, |d| {
                    d.child(
                        action_button(("uninstall", i), "trash", "Uninstall")
                            .size(rems(header::BUTTON_WIDTH))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.action(uninstall_key.to_string(), "uninstall", cx);
                            })),
                    )
                })
                .when(show_modded, |d| {
                    d.child(
                        action_button(("launch-modded", i), "mods", "Launch modded")
                            .size(rems(header::BUTTON_WIDTH))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.action(modded_key.to_string(), "launch-modded", cx);
                            })),
                    )
                })
                .when(has_primary, |d| {
                    d.child(
                        game_action_button(
                            (play_button.map_or("action", PlayButton::element_id), i),
                            play_button,
                            working,
                            if playable { "play" } else { "install" },
                            action_tip,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            match play_button {
                                Some(PlayButton::Quit) => this.action(key.to_string(), "quit", cx),
                                None if !this.primary_download_action(&key, cx) => {
                                    this.action(key.to_string(), action, cx);
                                }
                                _ => {}
                            }
                        })),
                    )
                })
        });
        views::ArtworkCard {
            width,
            ratio: CARD_POSTER_RATIO,
            title: g.title.clone(),
            cover: g.card_poster.clone(),
            dimmed: !installed_here,
            installed: installed_here,
        }
        .render(("poster", i), Some(badges), actions, self.zoom)
        .id(("card", i))
        .on_click(cx.listener(move |this, _, window, cx| {
            window.focus(&this.focus, cx);
            this.open_details(&details_key, cx);
        }))
        .on_hover(cx.listener(move |this, entered, _, cx| this.hover(i, *entered, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.game_menu = Some((menu_key.to_string(), event.position));
                cx.notify();
            }),
        )
    }
}

#[cfg(windows)]
fn hide_windows_border(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DwmSetWindowAttribute,
    };

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        let color = DWMWA_COLOR_NONE;
        // Windows 11 accepts this attribute. Older versions retain their
        // system outline and native resize hit testing still works.
        unsafe {
            DwmSetWindowAttribute(
                handle.hwnd.get() as _,
                DWMWA_BORDER_COLOR as u32,
                (&raw const color).cast(),
                size_of::<u32>() as u32,
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn x11_window_id(window: &Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn hide_x11_frame(window: &Window) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::{protocol::xproto::*, wrapper::ConnectionExt as _};

    if matches!(window.window_decorations(), Decorations::Server)
        && let Some(id) = x11_window_id(window)
    {
        // GPUI requests a server frame if X11 compositing is unavailable.
        // An opaque client window can still ask the window manager to remove it.
        let (connection, _) = x11rb::connect(None)?;
        let hints = connection.intern_atom(false, b"_MOTIF_WM_HINTS")?.reply()?.atom;
        const HINT_DECORATIONS: u32 = 1 << 1;
        connection
            .change_property32(PropMode::REPLACE, id, hints, hints, &[HINT_DECORATIONS, 0, 0, 0, 0])?
            .check()?;
    }
    Ok(())
}

fn client_titlebar(window: &Window) -> bool {
    #[cfg(target_os = "linux")]
    if x11_window_id(window).is_some() {
        return true;
    }
    cfg!(windows) || matches!(window.window_decorations(), Decorations::Client { .. })
}

#[cfg(target_os = "linux")]
fn resize_border(window: &Window) -> Div {
    let mut border = div().absolute().inset_0();
    if client_titlebar(window) && !window.is_maximized() && !window.is_fullscreen() {
        let tiling = match window.window_decorations() {
            Decorations::Client { tiling } => tiling,
            Decorations::Server => Tiling::default(),
        };
        let grip = px(5.);
        let handle = |edge: ResizeEdge| {
            div().absolute().cursor(edge.into()).on_mouse_down(
                MouseButton::Left,
                move |_, window, _| {
                    window.start_window_resize(edge);
                },
            )
        };
        border = border
            .when(!tiling.top, |d| {
                d.child(handle(ResizeEdge::Top).top_0().w_full().h(grip))
            })
            .when(!tiling.bottom, |d| {
                d.child(handle(ResizeEdge::Bottom).bottom_0().w_full().h(grip))
            })
            .when(!tiling.left, |d| {
                d.child(handle(ResizeEdge::Left).left_0().h_full().w(grip))
            })
            .when(!tiling.right, |d| {
                d.child(handle(ResizeEdge::Right).right_0().h_full().w(grip))
            })
            .when(!tiling.top && !tiling.left, |d| {
                d.child(handle(ResizeEdge::TopLeft).top_0().left_0().size(grip))
            })
            .when(!tiling.top && !tiling.right, |d| {
                d.child(handle(ResizeEdge::TopRight).top_0().right_0().size(grip))
            })
            .when(!tiling.bottom && !tiling.left, |d| {
                d.child(
                    handle(ResizeEdge::BottomLeft)
                        .bottom_0()
                        .left_0()
                        .size(grip),
                )
            })
            .when(!tiling.bottom && !tiling.right, |d| {
                d.child(
                    handle(ResizeEdge::BottomRight)
                        .bottom_0()
                        .right_0()
                        .size(grip),
                )
            });
    }
    border
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        compatibility::request_details(self, cx);
        if !self.downloads_open {
            self.downloads_visible.clear();
        }
        let viewport = f32::from(window.viewport_size().width);
        self.viewport_height = f32::from(window.viewport_size().height);
        let header_state = HeaderState {
            store: self.store,
            status: self.status,
            os: self.os,
            access: self.access,
            verification: self.verification,
            show_verification: self.host == Os::Linux,
            query: self.query.clone(),
            menu: self.menu,
            syncing: self.syncing,
            downloads: self.download_summary(),
            downloads_open: self.downloads_open,
            zoom: self.zoom,
            viewport,
        };
        if self.header_state != header_state {
            if self.header_state.bottom() != header_state.bottom() {
                self.rows.remeasure_items(0..1);
            }
            self.header_state = header_state.clone();
            self.header.update(cx, |header, cx| {
                header.state = header_state;
                cx.notify();
            });
        }
        let views::CardGrid {
            columns,
            width,
            row_height,
        } = views::CardGrid::new(viewport, self.zoom, 12.5, 2, CARD_POSTER_RATIO);
        if self.details.is_none()
            && (columns != self.columns || (row_height - self.row_height).abs() > 0.01)
        {
            self.columns = columns;
            self.row_height = row_height;
            self.reset_rows();
        }
        #[cfg(target_os = "linux")]
        let border = resize_border(window);
        artwork::maintain(self, window, cx);
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
                    compatibility::request(this, i, cx);
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
                        d.children(
                            (0..columns)
                                .map(|_| views::ArtworkCard::skeleton(width, CARD_POSTER_RATIO)),
                        )
                    })
                    .children(indices.into_iter().map(|i| this.card(i, width, cx)))
                    .into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full();
        let checking_verification =
            self.active_verification().is_some() && self.compatibility.checking();
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
            .font_family(if cfg!(windows) {
                "Segoe UI"
            } else {
                "Adwaita Sans"
            })
            .text_size(rems(1.125))
            .text_color(rgb(0xf0f8f3))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClearSearch, window, cx| {
                if this.mods.is_some() && (this.menu || this.downloads_open) {
                    this.menu = false;
                    this.close_downloads(cx);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.mods.as_ref().is_some_and(|page| {
                    page.update(cx, |page, cx| page.dismiss_overlay(window, cx))
                }) {
                    cx.stop_propagation();
                    return;
                }
                if this.mods.take().is_some() {
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.dismiss_details(window, cx) {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
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
                        if let Some(mods) = &this.mods {
                            mods.update(cx, |mods, cx| mods.focus_search(window, cx));
                            return;
                        }
                        this.details = None;
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
                    if this.mods.is_some() && (this.menu || this.downloads_open) {
                        this.menu = false;
                        this.close_downloads(cx);
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if this.mods.as_ref().is_some_and(|page| {
                        page.update(cx, |page, cx| page.dismiss_overlay(window, cx))
                    }) {
                        cx.stop_propagation();
                        return;
                    }
                    if this.mods.take().is_some() {
                        window.focus(&this.focus, cx);
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if this.downloads_open {
                        this.close_downloads(cx);
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if this.dismiss_details(window, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    this.search.update(cx, |search, cx| search.emplace("", cx));
                    this.menu = false;
                    this.game_menu = None;
                    this.confirm = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .child(self.backdrop.clone())
            .when(self.details.is_none(), |d| {
                d.child(scroll_input).child(rows)
            })
            .when(
                self.details.is_none() && !self.starting && self.visible.is_empty(),
                |d| {
                    let xbox = self.store.is_some_and(|index| STORES[index] == "xbox");
                    let title = if checking_verification {
                        "Checking game compatibility…"
                    } else if self.load_error.is_some() {
                        "Could not load library"
                    } else if xbox && self.status == 1 {
                        "No installed Xbox games match these filters"
                    } else if self.games.is_empty() {
                        "No games found"
                    } else {
                        "No matching games"
                    };
                    let hint = if checking_verification {
                        None
                    } else if self.load_error.is_some() {
                        Some("Refresh to try again.")
                    } else if !self.games.is_empty() {
                        Some("Try a different filter or search.")
                    } else if !xbox {
                        Some("Refresh after signing in to a supported client.")
                    } else {
                        None
                    };
                    let indicator = checking_verification
                        .then(|| header::spinner("checking-verification", 2.));
                    d.child(
                        views::empty_state(title, hint, indicator)
                            .absolute()
                            .inset_0()
                            .top(rems(self.header_state.bottom())),
                    )
                },
            )
            .when(self.details.is_none(), |d| d.child(self.header.clone()))
            .when(self.mods.is_none(), |d| {
                d.when_some(self.details.clone(), |d, page| d.child(page))
            })
            .when_some(self.mods.clone(), |d, mods| d.child(mods))
            .when(client_titlebar(window) && !window.is_fullscreen(), |d| {
                let drag_area = div().absolute().top_0().w_full().h(rems(0.6));
                #[cfg(windows)]
                let drag_area = drag_area.window_control_area(WindowControlArea::Drag);
                #[cfg(not(windows))]
                let drag_area = drag_area.on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                });
                d.child(drag_area)
            })
            .map(|d| {
                #[cfg(target_os = "linux")]
                let d = d.child(border);
                d
            })
            .when(self.details.is_some() && self.mods.is_none(), |d| {
                d.when_some(self.download_summary(), |d, label| {
                    d.child(
                        downloads::queue_pill(
                            label,
                            self.downloads_open,
                            self.downloads_bounds.clone(),
                        )
                        .absolute()
                        .top(rems(0.7))
                        .right(rems(9.55))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_downloads(cx))),
                    )
                })
            })
            .when(self.menu, |d| d.child(self.settings(cx)))
            .when(self.downloads_open, |d| d.child(self.download_popup(cx)))
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
                    div()
                        .absolute()
                        .bottom(rems(1.5))
                        .left(rems(1.))
                        .right(rems(1.))
                        .flex()
                        .justify_center()
                        .child(
                            pill()
                                .max_w(rems(40.))
                                .min_w_0()
                                .px(rems(1.25))
                                .py(rems(0.75))
                                .rounded(rems(1.2))
                                .child(div().min_w_0().whitespace_normal().text_center().child(text)),
                        ),
                )
            })
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--download-worker" {
        raesir_backend::downloads::run_worker(std::path::Path::new(&args[2]));
        return;
    }
    if args.len() == 2 && matches!(args[1].to_str(), Some("--version" | "-V")) {
        println!("Raesir {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    #[cfg(feature = "benchmark")]
    if args.len() == 3 && args[1] == "--artwork-probe" {
        let backend = backend::Backend::new();
        let mut game = raesir_backend::catalog::entry(
            "media",
            &args[2].to_string_lossy(),
            "Probe",
            vec![],
            "diagnostic",
        );
        game.cover = Some(args[2].to_string_lossy().into_owned());
        for stage in ["cold", "cached"] {
            let start = Instant::now();
            let ready = artwork::load(&backend, &game).expect("artwork probe");
            println!(
                "{stage}: {:.2} ms, {:?}",
                start.elapsed().as_secs_f64() * 1000.,
                ready.poster.size(0)
            );
        }
        return;
    }
    if args.len() == 6 && args[1] == "--steam-control" {
        let appid = args[4].to_string_lossy().parse().unwrap_or(0);
        raesir_backend::steam_control::helper(
            std::path::Path::new(&args[2]),
            appid,
            &args[3].to_string_lossy(),
            std::path::Path::new(&args[5]),
        );
        return;
    }
    if args.len() == 5 && args[1] == "--steam-mods" {
        raesir_backend::mods::steam_helper(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
        );
        return;
    }
    if args.len() == 4 && args[1] == "--steam-library-probe" {
        raesir_backend::steam_ipc::helper(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
        );
        return;
    }
    let backend = backend::Backend::new();
    let app_backend = backend.clone();
    eprintln!(
        "Raesir: native GPUI; pill blur {}.",
        if std::env::var_os("RAESIR_NO_BLUR").is_some() {
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
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    app_id: Some(raesir_backend::storage::APP_ID.into()),
                    icon: Some(Arc::new(
                        image_codec::load_from_memory(include_bytes!("../../build/icon.png"))
                            .expect("bundled Raesir icon")
                            .into_rgba8(),
                    )),
                    window_min_size: Some(size(px(600.), px(700.))),
                    inactive_frame_interval: std::env::var_os("RAESIR_LIMIT_BACKGROUND")
                        .map(|_| millis(33)),
                    ..Default::default()
                },
                |window, cx| {
                    #[cfg(windows)]
                    hide_windows_border(window);
                    #[cfg(target_os = "linux")]
                    if let Err(error) = hide_x11_frame(window) {
                        eprintln!("Could not hide the native X11 frame: {error}");
                    }
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
