use crate::{
    Launcher,
    backend::Backend,
    header::{action_button, button, pill, spinner},
    icons::{icon, rating_image, system_icon},
    smooth_scroll::SmoothWheel,
};
use exposed_games_backend::{details::Details, model::LibraryGame};
use gpui::{prelude::*, *};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Instant,
};
mod description;
mod player_stats;
mod reviews;
use unicode_segmentation::UnicodeSegmentation;

pub struct DetailsPage {
    game: LibraryGame,
    mod_context: Option<ModContext>,
    _mod_subscription: Option<Subscription>,
    download_progress: Option<exposed_games_backend::steam_control::Progress>,
    poster: Option<Arc<RenderImage>>,
    backdrop: Entity<crate::backdrop::Backdrop>,
    backdrop_source: String,
    backdrop_ready: bool,
    backdrop_task: Option<Task<()>>,
    backend: Arc<Backend>,
    owner: WeakEntity<Launcher>,
    data: Option<Arc<Details>>,
    loading: bool,
    error: Option<String>,
    task: Option<Task<()>>,
    stats: Option<Arc<exposed_games_backend::player_stats::PlayerStats>>,
    stats_task: Option<Task<()>>,
    stats_loading: bool,
    achievements_open: bool,
    hidden_achievements_expanded: bool,
    achievement_rows: ListState,
    achievement_order: Vec<usize>,
    achievement_preview_slots: usize,
    achievement_focus: FocusHandle,
    achievement_wheel: SmoothWheel,
    achievement_frame_pending: bool,
    images: HashMap<String, Arc<RenderImage>>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    animations: HashMap<usize, description::AnimationSlot>,
    selected: usize,
    reviews_visible: usize,
    player: Option<Entity<crate::video::Player>>,
    player_events: Option<Subscription>,
    fullscreen: bool,
    scroll: ScrollHandle,
    media_scroll: ScrollHandle,
    strip_bounds: Rc<Cell<Bounds<Pixels>>>,
    about_expanded: bool,
    review_state: reviews::State,
    filter_menu: Option<(usize, usize)>,
    filter_bounds: [Rc<Cell<Bounds<Pixels>>>; 6],
    filter_focus: FocusHandle,
    return_focus: FocusHandle,
    wheel: SmoothWheel,
    frame_pending: bool,
    cleanup_registered: bool,
}

#[derive(Clone)]
pub(crate) struct ModContext {
    pub browser: WeakEntity<crate::mods::ModBrowser>,
    pub id: String,
    pub version: String,
}

impl DetailsPage {
    fn new(
        source: LibraryGame,
        poster: Option<Arc<RenderImage>>,
        backdrop: Entity<crate::backdrop::Backdrop>,
        backend: Arc<Backend>,
        owner: WeakEntity<Launcher>,
        return_focus: FocusHandle,
        download_progress: Option<exposed_games_backend::steam_control::Progress>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            game: source,
            download_progress,
            poster,
            backdrop,
            backdrop_source: String::new(),
            backdrop_ready: false,
            backdrop_task: None,
            backend,
            owner,
            data: None,
            loading: true,
            error: None,
            task: None,
            stats: None,
            stats_task: None,
            stats_loading: true,
            achievements_open: false,
            hidden_achievements_expanded: false,
            achievement_rows: ListState::new(0, ListAlignment::Top, px(200.)),
            achievement_order: Vec::new(),
            achievement_preview_slots: 1,
            achievement_focus: cx.focus_handle(),
            achievement_wheel: SmoothWheel::default(),
            achievement_frame_pending: false,
            images: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            animations: HashMap::new(),
            selected: 0,
            reviews_visible: 4,
            player: None,
            player_events: None,
            fullscreen: false,
            scroll: ScrollHandle::new(),
            media_scroll: ScrollHandle::new(),
            strip_bounds: Rc::new(Cell::new(Bounds::default())),
            about_expanded: false,
            review_state: reviews::State::new(cx),
            filter_menu: None,
            filter_bounds: std::array::from_fn(|_| Rc::new(Cell::new(Bounds::default()))),
            filter_focus: cx.focus_handle(),
            return_focus,
            wheel: SmoothWheel::default(),
            frame_pending: false,
            cleanup_registered: false,
            mod_context: None,
            _mod_subscription: None,
        }
    }

    pub(crate) fn for_mod(
        source: LibraryGame,
        data: Details,
        poster: Option<Arc<RenderImage>>,
        backdrop: Entity<crate::backdrop::Backdrop>,
        backend: Arc<Backend>,
        owner: WeakEntity<Launcher>,
        return_focus: FocusHandle,
        context: ModContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let browser = context.browser.upgrade();
        let mut page = Self::new(
            source,
            poster,
            backdrop,
            backend,
            owner,
            return_focus,
            None,
            cx,
        );
        page.mod_context = Some(context);
        page.data = Some(Arc::new(data));
        page.loading = false;
        page.stats_loading = false;
        page._mod_subscription = browser.map(|browser| {
            cx.observe(&browser, |this, browser, cx| {
                if let Some(context) = &mut this.mod_context {
                    this.poster = browser.read(cx).mod_poster(&context.id);
                    if let Some(version) = browser.read(cx).mod_version(&context.id) {
                        context.version = version;
                    }
                }
                cx.notify();
            })
        });
        page
    }

    pub(crate) fn load_mod(&mut self, game: LibraryGame, id: String, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        self.loading = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.mod_details(&game, &id) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(data) => this.data = Some(Arc::new(data)),
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn mod_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(context) = &self.mod_context else {
            return div().into_any_element();
        };
        let Some(browser) = context.browser.upgrade() else {
            return div().into_any_element();
        };
        let (installed, busy, error) = browser.read(cx).mod_status(&context.id);
        let id = context.id.clone();
        div()
            .flex()
            .items_center()
            .gap(rems(0.65))
            .child(bubble().child(format!("Version {}", context.version)))
            .child(if busy {
                crate::header::button_shell("details-mod-busy", false)
                    .size(rems(3.3))
                    .bg(rgba(0x101010b8))
                    .child(spinner("mod-page-install", 1.25))
                    .into_any_element()
            } else {
                action_button(
                    "details-mod-action",
                    if installed { "trash" } else { "install" },
                    if installed {
                        "Uninstall"
                    } else {
                        "Install mod and dependencies"
                    },
                )
                .size(rems(3.3))
                .on_click(move |_, _, cx| {
                    browser.update(cx, |browser, cx| {
                        browser.request_mod_change(id.clone(), installed, cx)
                    });
                })
                .into_any_element()
            })
            .when_some(error, |d, error| {
                d.child(
                    div()
                        .text_size(rems(0.95))
                        .text_color(rgb(0xe6c58d))
                        .child(error),
                )
            })
            .into_any_element()
    }
}

pub(crate) fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .rounded_full()
        .px(rems(1.))
        .py(rems(0.6))
        .flex()
        .items_center()
        .gap(rems(0.5))
        .cursor_pointer()
        .bg(rgba(0x101010b8))
        .hover(|s| s.bg(rgba(0x303030b3)))
        .child(label.into())
}
fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(1.))
        .rounded(rems(1.4))
        .p(rems(1.5))
        .bg(rgba(0x101010b3))
}
fn heading(label: &'static str) -> Div {
    div()
        .text_size(rems(1.4))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(rems(1.125))
        .text_color(rgb(0xb5c9bf))
        .whitespace_normal()
        .child(text.into())
}
fn paragraph(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(rems(1.125))
        .line_height(rems(1.75))
        .whitespace_normal()
        .child(text.into())
}
fn review_paragraph(text: String) -> Div {
    let mut wrapped = String::with_capacity(text.len());
    for word in text.split_inclusive(char::is_whitespace) {
        if word.graphemes(true).count() > 40 {
            for grapheme in word.graphemes(true) {
                wrapped.push_str(grapheme);
                if !grapheme.chars().all(char::is_whitespace) {
                    wrapped.push('\u{200b}');
                }
            }
        } else {
            wrapped.push_str(word);
        }
    }
    paragraph(wrapped).w_full().min_w_0()
}
fn bubble() -> Div {
    pill()
        .min_h(rems(3.3))
        .px(rems(1.))
        .py(rems(0.75))
        .gap(rems(0.6))
}
fn client_icon(client: &str, store: &str) -> &'static str {
    match client {
        "Xbox" => return "xbox",
        "Steam" => return "steam",
        "EA app" | "Origin" => return "ea",
        "Ubisoft Connect" => return "ubisoft",
        "Epic" => return "epic",
        _ => {}
    }
    match store {
        "steam" => "steam",
        "epic" => "epic",
        "ea" => "ea",
        "ubisoft" => "ubisoft",
        "wargaming" => "wargaming",
        _ => "xbox",
    }
}
fn age_badge(rating: &str) -> Stateful<Div> {
    let label = rating.to_owned();
    div()
        .id("details-age")
        .relative()
        .w(rems(6.))
        .flex_none()
        .rounded(rems(1.2))
        .bg(rgba(0x101010b3))
        .child(
            div()
                .absolute()
                .inset(rems(0.6))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .when_some(rating_image(rating), |d, image| {
                    d.child(img(image).size_full().object_fit(ObjectFit::Contain))
                })
                .when(rating_image(rating).is_none(), |d| {
                    d.gap(rems(0.5))
                        .child(icon("age"))
                        .child(muted(rating.to_owned()).text_center())
                }),
        )
        .tooltip(move |_, cx| cx.new(|_| DetailTooltip(label.clone())).into())
}
fn rating_star(fill: f32) -> Div {
    div()
        .relative()
        .size(rems(1.25))
        .flex_none()
        .child(
            icon("star-empty")
                .absolute()
                .inset_0()
                .size(rems(1.25))
                .text_color(rgb(0x70867a)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(relative(fill))
                .overflow_hidden()
                .child(
                    icon("star")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size(rems(1.25))
                        .text_color(rgb(0xf3ce68)),
                ),
        )
}
fn skeleton_line(width: f32) -> Div {
    div()
        .w(relative(width))
        .h(rems(1.125))
        .my(rems(0.3125))
        .rounded(rems(0.35))
        .bg(rgba(0xffffff20))
}
fn skeleton_block(width: f32, height: f32) -> Div {
    div()
        .w(rems(width))
        .h(rems(height))
        .rounded(rems(0.8))
        .bg(rgba(0xffffff20))
}
fn skeleton_lines(widths: &[f32]) -> Div {
    div()
        .flex()
        .flex_col()
        .children(widths.iter().copied().map(skeleton_line))
}
fn content_skeleton() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(1.25))
        .child(
            panel()
                .child(heading("About"))
                .child(skeleton_lines(&[1., 0.98, 0.72]))
                .child(skeleton_block(6.4, 3.)),
        )
        .child(
            div()
                .w_full()
                .aspect_ratio(16. / 9.)
                .rounded(rems(1.2))
                .bg(rgba(0x101010b3)),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .gap(rems(0.6))
                .children((0..6).map(|_| skeleton_block(7., 4.))),
        )
        .child(
            panel().child(heading("System requirements")).child(
                div()
                    .flex()
                    .gap(rems(1.5))
                    .children(["Minimum", "Recommended"].into_iter().map(|label| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rems(0.75))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rems(0.5))
                                    .child(skeleton_block(1.55, 1.55))
                                    .child(label),
                            )
                            .child(skeleton_lines(&[0.7, 0.95, 0.55, 0.9, 0.6, 0.5]))
                    })),
            ),
        )
        .child(
            div()
                .flex()
                .gap(rems(0.65))
                .child(skeleton_block(12., 3.))
                .child(skeleton_block(14., 3.)),
        )
        .children((0..3).map(|_| {
            panel()
                .p(rems(1.25))
                .gap(rems(0.65))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.65))
                        .child(skeleton_block(1.25, 1.25))
                        .child(skeleton_block(12., 1.125)),
                )
                .child(skeleton_lines(&[0.98, 0.93, 0.65]))
                .child(skeleton_block(18., 1.125))
        }))
}
struct DetailTooltip(String);
impl Render for DetailTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        bubble().text_size(rems(1.125)).child(self.0.clone())
    }
}

impl Launcher {
    pub fn dismiss_details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.details.clone() else {
            return false;
        };
        if !page.update(cx, |page, cx| {
            if page.achievements_open {
                page.close_achievements(window, cx);
                true
            } else if page.filter_menu.is_some() {
                page.close_filter(window, cx);
                true
            } else {
                page.exit_fullscreen(window, cx)
            }
        }) {
            self.details = None;
        }
        cx.notify();
        true
    }
    pub fn open_details(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(game) = self.games.iter().find(|g| g.source.key == key) else {
            return;
        };
        let source = game.source.clone();
        let poster = game.poster.clone();
        let background = game.background.clone();
        let backdrop = self.backdrop.clone();
        let backend = self.backend.clone();
        let owner = cx.weak_entity();
        self.hover_task = None;
        self.hovered = None;
        self.game_menu = None;
        self.menu = false;
        if let Some(image) = background {
            backdrop.update(cx, |backdrop, cx| backdrop.retarget(image, cx));
        }
        let return_focus = self.focus.clone();
        let download_progress = self.display_progress(key);
        let page = cx.new(|cx| {
            DetailsPage::new(
                source,
                poster,
                backdrop,
                backend,
                owner,
                return_focus,
                download_progress,
                cx,
            )
        });
        page.update(cx, |page, cx| {
            page.load(cx);
            page.load_stats(cx);
        });
        self.details = Some(page);
        cx.notify();
    }
}
impl DetailsPage {
    pub fn library_changed(
        &mut self,
        games: &[crate::library::Game],
        progress: &HashMap<String, exposed_games_backend::steam_control::Progress>,
        cx: &mut Context<Self>,
    ) {
        if let Some(game) = games.iter().find(|g| g.source.key == self.game.key) {
            self.game = game.source.clone();
        }
        self.download_progress = progress.get(&self.game.key).cloned();
        cx.notify();
    }

    fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_menu = None;
        window.focus(&self.return_focus, cx);
        cx.notify();
    }
    pub fn update_cover(
        &mut self,
        key: &str,
        poster: Arc<RenderImage>,
        background: Arc<RenderImage>,
        cx: &mut Context<Self>,
    ) {
        if self.game.key == key {
            self.poster = Some(poster);
            if !self.backdrop_ready {
                self.backdrop
                    .update(cx, |backdrop, cx| backdrop.retarget(background, cx));
            }
            cx.notify();
        }
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let cached = cx
                .background_executor()
                .spawn({
                    let backend = backend.clone();
                    let game = game.clone();
                    async move { backend.cached_details(&game) }
                })
                .await;
            let fresh = cached.as_ref().is_some_and(|v| {
                exposed_games_backend::details::now().saturating_sub(v.updated) < 21600
                    && (v.description.is_empty() || v.description_html.is_some())
                    && (!matches!(game.store.as_str(), "epic" | "ea" | "ubisoft")
                        || v.store_url.is_some())
            });
            this.update(cx, |this, cx| {
                if let Some(data) = cached {
                    this.load_background(&data, cx);
                    this.data = Some(Arc::new(data));
                }
                this.loading = !fresh;
                cx.notify();
            })
            .ok();
            if fresh {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move { backend.details(&game) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(data) => {
                        this.load_background(&data, cx);
                        let selected = this
                            .data
                            .as_ref()
                            .and_then(|d| d.media.get(this.selected))
                            .and_then(|media| data.media.iter().position(|m| m.url == media.url));
                        this.selected = selected.unwrap_or(0);
                        if selected.is_none() {
                            this.player = None;
                            this.player_events = None;
                            this.fullscreen = false;
                        }
                        this.animations.clear();
                        this.data = Some(Arc::new(data));
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
    fn load_background(&mut self, data: &Details, cx: &mut Context<Self>) {
        let Some(source) = crate::backdrop::media_source(data) else {
            return;
        };
        if self.backdrop_source == source {
            return;
        }
        self.backdrop_source = source.into();
        let source = source.to_owned();
        let backend = self.backend.clone();
        self.backdrop_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::backdrop::from_media(&backend, &source) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(image) = result {
                    this.backdrop_ready = true;
                    this.backdrop
                        .update(cx, |backdrop, cx| backdrop.retarget(image.clone(), cx));
                    this.owner
                        .update(cx, |owner, _| {
                            if let Some(game) = owner
                                .games
                                .iter_mut()
                                .find(|g| g.source.key == this.game.key)
                            {
                                game.background = Some(image);
                                game.background_checked = true;
                            }
                        })
                        .ok();
                }
            })
            .ok();
        }));
    }
    fn image(&mut self, url: &str, cx: &mut Context<Self>) {
        if url.is_empty() || self.images.contains_key(url) {
            return;
        }
        if let Some(image) = self.backend.cached_achievement_icon(url) {
            self.images.insert(url.to_owned(), image);
            return;
        }
        if self.pending.contains(url) || self.failed.contains(url) || self.pending.len() >= 3 {
            return;
        }
        let achievement = self
            .stats
            .as_ref()
            .is_some_and(|stats| stats.achievements.iter().any(|a| a.icon == url));
        let url = url.to_owned();
        self.pending.insert(url.clone());
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let request = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if achievement {
                        return backend.achievement_icon(&request, true);
                    }
                    let path = backend.detail_media(&request)?;
                    let image = image_codec::ImageReader::open(path)
                        .map_err(|e| e.to_string())?
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?
                        .decode()
                        .map_err(|e| e.to_string())?
                        .thumbnail(1920, 1080);
                    let mut pixels = image.into_rgba8();
                    if pixels.pixels().all(|p| p[3] == 0) {
                        return Err("Empty media placeholder".into());
                    }
                    for p in pixels.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    Ok::<_, String>(Arc::new(RenderImage::new(vec![image_codec::Frame::new(
                        pixels,
                    )])))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pending.remove(&url);
                match result {
                    Ok(image) => {
                        this.images.insert(url, image);
                    }
                    Err(_) => {
                        this.failed.insert(url);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        self.player = None;
        self.player_events = None;
        cx.notify();
    }
    fn play(&mut self, cx: &mut Context<Self>) {
        let Some(media) = self
            .data
            .as_ref()
            .and_then(|d| d.media.get(self.selected))
            .cloned()
        else {
            return;
        };
        if !media.video {
            return;
        }
        let player =
            cx.new(|cx| crate::video::Player::new(media.url, self.backend.cache.clone(), cx));
        self.player_events = Some(cx.subscribe(
            &player,
            |this, _, event: &crate::video::Fullscreen, cx| {
                this.fullscreen = event.0;
                cx.notify();
            },
        ));
        self.player = Some(player);
        cx.notify();
    }
    pub fn exit_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.fullscreen {
            return false;
        }
        if let Some(player) = &self.player {
            player.update(cx, |player, cx| {
                player.leave_fullscreen(window);
                cx.notify();
            });
        }
        self.fullscreen = false;
        cx.notify();
        true
    }
    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.achievements_open {
            self.scroll_achievements(event, window, cx);
            return;
        }
        if self.fullscreen {
            return;
        }
        if self.filter_menu.is_some() {
            cx.stop_propagation();
            return;
        }
        let delta = event.delta.pixel_delta(window.rem_size() * 2.5);
        if self.strip_bounds.get().contains(&event.position)
            && (event.modifiers.shift || delta.x != px(0.))
        {
            let x = if delta.x != px(0.) { delta.x } else { delta.y };
            let offset = self.media_scroll.offset().x + x;
            self.media_scroll.set_offset(point(
                offset.clamp(-self.media_scroll.max_offset().x, px(0.)),
                px(0.),
            ));
            self.wheel.reset(-f32::from(self.scroll.offset().y) as f64);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.modifiers.shift || delta.x.abs() > delta.y.abs() {
            cx.stop_propagation();
            return;
        }
        let ScrollDelta::Lines(lines) = event.delta else {
            self.wheel.reset(-f32::from(self.scroll.offset().y) as f64);
            return;
        };
        if lines.y == 0. || event.modifiers.control {
            return;
        }
        cx.stop_propagation();
        if !self.wheel.active() {
            self.wheel.reset(-f32::from(self.scroll.offset().y) as f64);
        }
        self.wheel.push(
            -lines.y as f64 * f32::from(window.rem_size()) as f64 * 2.5,
            Instant::now(),
            f32::from(self.scroll.max_offset().y) as f64,
        );
        self.schedule(window, cx);
    }
    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frame_pending || !self.wheel.active() {
            return;
        }
        self.frame_pending = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.frame_pending = false;
            let offset = window.pixel_snap(px(-this.wheel.sample(Instant::now()) as f32));
            this.scroll.set_offset(point(px(0.), offset));
            cx.notify();
            this.schedule(window, cx);
        });
    }
    fn gallery(
        &mut self,
        data: &Details,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(media) = data.media.get(self.selected) else {
            return panel()
                .self_start()
                .child(muted(
                    "No trailers or screenshots supplied for this edition.",
                ))
                .into_any_element();
        };
        let url = media.poster_url();
        let thumbnail = if media.video {
            &media.preview
        } else {
            &media.url
        };
        self.image(&url, cx);
        let first = (-f32::from(self.media_scroll.offset().x)
            / (f32::from(window.rem_size()) * 7.6)) as usize;
        for item in data.media.iter().skip(first).take(12) {
            self.image(if item.video { &item.preview } else { &item.url }, cx);
        }
        let image = self
            .images
            .get(&url)
            .or_else(|| {
                self.failed
                    .contains(&url)
                    .then(|| self.images.get(thumbnail))
                    .flatten()
            })
            .cloned();
        let waiting = image.is_none()
            && [&url, thumbnail]
                .iter()
                .any(|url| !url.is_empty() && !self.failed.contains(url.as_str()));
        let count = data.media.len();
        if count == 1 {
            self.strip_bounds.set(Bounds::default());
        }
        let previous = (self.selected + count - 1) % count;
        let next = (self.selected + 1) % count;
        let viewer = div()
            .relative()
            .w_full()
            .aspect_ratio(16. / 9.)
            .rounded(rems(1.2))
            .overflow_hidden()
            .shadow_md()
            .bg(rgba(0x101010b3))
            .when_some(image, |d, image| {
                let size = image.size(0);
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(image)
                                .size_full()
                                .max_w(px(size.width.0 as f32))
                                .max_h(px(size.height.0 as f32))
                                .rounded(rems(1.2))
                                .object_fit(ObjectFit::Contain),
                        ),
                )
            })
            .when(waiting, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(pill().p(rems(1.)).child(spinner("preview-loading", 2.))),
                )
            })
            .when(media.video && self.player.is_none() && !waiting, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            action_button("play-trailer", "play", "Play trailer")
                                .size(rems(3.5))
                                .on_click(cx.listener(|this, _, _, cx| this.play(cx))),
                        ),
                )
            })
            .when_some(
                self.player.clone().filter(|_| !self.fullscreen),
                |d, player| d.child(player),
            );
        let strip_bounds = self.strip_bounds.clone();
        div()
            .flex()
            .flex_col()
            .gap(rems(0.9))
            .child(
                div()
                    .relative()
                    .child(viewer)
                    .child(
                        action_button("media-previous", "back", "Previous media")
                            .absolute()
                            .left(rems(-3.25))
                            .top(relative(0.5))
                            .mt(rems(-1.3))
                            .opacity(if count > 1 { 1. } else { 0.4 })
                            .on_click(cx.listener(move |this, _, _, cx| this.select(previous, cx))),
                    )
                    .child(
                        action_button("media-next", "next", "Next media")
                            .absolute()
                            .right(rems(-3.25))
                            .top(relative(0.5))
                            .mt(rems(-1.3))
                            .opacity(if count > 1 { 1. } else { 0.4 })
                            .on_click(cx.listener(move |this, _, _, cx| this.select(next, cx))),
                    ),
            )
            .when(count > 1, |d| {
                d.child(
                    div()
                        .id("media-strip")
                        .flex()
                        .gap(rems(0.6))
                        .w_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.media_scroll)
                        .on_prepaint(move |paint, _, _| strip_bounds.set(paint.bounds))
                        .child(div().flex().flex_none().mx_auto().gap(rems(0.6)).children(
                            data.media.iter().enumerate().map(|(i, item)| {
                                let url = if item.video { &item.preview } else { &item.url };
                                div()
                                    .id(("media-thumb", i))
                                    .w(rems(7.))
                                    .h(rems(4.))
                                    .flex_none()
                                    .relative()
                                    .rounded(rems(0.7))
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .opacity(if i == self.selected { 1. } else { 0.65 })
                                    .hover(|s| s.opacity(1.))
                                    .when_some(self.images.get(url).cloned(), |d, image| {
                                        d.child(
                                            img(image)
                                                .size_full()
                                                .rounded(rems(0.7))
                                                .object_fit(ObjectFit::Cover),
                                        )
                                    })
                                    .when(item.video, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .rounded_full()
                                                        .p(rems(0.3))
                                                        .bg(rgba(0xe6f3eaf0))
                                                        .child(icon("play").size(rems(1.))),
                                                ),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
                            }),
                        )),
                )
            })
            .into_any_element()
    }
}
impl Render for DetailsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.cleanup_registered {
            cx.on_release_in(window, |this, window, _| {
                for (_, image) in this.images.drain() {
                    let _ = window.drop_image(image);
                }
            })
            .detach();
            self.cleanup_registered = true;
        }
        let owner = self.owner.clone();
        let owner_close = self.owner.clone();
        let action_owner = self.owner.clone();
        let store_owner = self.owner.clone();
        let library_owner = self.owner.clone();
        let key = self.game.key.clone();
        let store_key = key.clone();
        let library_key = key.clone();
        let mod_back = self
            .mod_context
            .as_ref()
            .map(|context| context.browser.clone());
        let back = pill()
            .id("details-back")
            .gap(rems(0.4))
            .px(rems(0.65))
            .cursor_pointer()
            .child(icon("back"))
            .child(if mod_back.is_some() {
                "Mods"
            } else {
                "Library"
            })
            .on_click(move |_, window, cx| {
                if let Some(browser) = &mod_back {
                    browser
                        .update(cx, |browser, cx| browser.close_mod_details(window, cx))
                        .ok();
                    return;
                }
                owner
                    .update(cx, |this, cx| {
                        this.details = None;
                        cx.notify();
                    })
                    .ok();
            });
        let controls = pill()
            .child(
                button("details-min", "minimize", false)
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .child(
                button("details-max", "maximize", false)
                    .on_click(|_, window, _| window.zoom_window()),
            )
            .child(
                button("details-close", "close", false).on_click(move |_, window, cx| {
                    owner_close
                        .update(cx, |this, cx| this.close(window, cx))
                        .ok();
                }),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rems(1.25))
            .w_full()
            .max_w(rems(83.))
            .mx_auto()
            .pt(rems(5.3))
            .pb(rems(3.))
            .px(rems(3.5));
        let info = self.data.clone();
        let mut hero = div().flex().items_start().gap(rems(1.5));
        hero = hero.child(
            div()
                .relative()
                .w(rems(12.))
                .h(rems(if self.mod_context.is_some() { 12. } else { 18. }))
                .flex_none()
                .rounded(rems(1.4))
                .bg(rgba(0x101010b3))
                .shadow_lg()
                .when_some(self.poster.clone(), |d, poster| {
                    d.child(
                        img(poster)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .rounded(rems(1.4))
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .when(self.poster.is_none(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("installed").size(rems(3.))),
                    )
                }),
        );
        let mut meta = div()
            .id("details-meta")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rems(0.9));
        let mut title = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rems(0.65))
            .child(
                bubble()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.game.title.clone()),
            );
        if self.mod_context.is_none() {
            let platform = bubble()
                .id("details-client")
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x303030b3)))
                .child(icon(client_icon(&self.game.client, &self.game.store)))
                .child(self.game.client.clone());
            let client_label = format!("Open in {} library", self.game.client);
            title = title.child(
                if self
                    .game
                    .client_os
                    .contains(&exposed_games_backend::model::Os::host())
                {
                    platform
                        .tooltip(move |_, cx| {
                            cx.new(|_| DetailTooltip(client_label.clone())).into()
                        })
                        .on_click(move |_, _, cx| {
                            library_owner
                                .update(cx, |this, cx| {
                                    this.action(library_key.clone(), "library", cx)
                                })
                                .ok();
                        })
                } else {
                    platform.opacity(0.6).cursor_default().tooltip(|_, cx| {
                        cx.new(|_| DetailTooltip("Client library is on another OS".into()))
                            .into()
                    })
                },
            );
        } else {
            title = title.child(bubble().child(icon("mods")).child(self.game.client.clone()));
        }
        if self.game.mods.is_some() {
            let owner = self.owner.clone();
            let key = self.game.key.clone();
            title = title.child(
                bubble()
                    .id("details-mods")
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(0x303030b3)))
                    .child(icon("mods"))
                    .child("Mods")
                    .on_click(move |_, window, cx| {
                        owner
                            .update(cx, |owner, cx| owner.open_mods(&key, window, cx))
                            .ok();
                    }),
            );
        }
        let store_url = info.as_ref().and_then(|v| v.store_url.clone());
        if self.game.store_page.is_some() || store_url.is_some() {
            title = title.child(
                action_button("details-store", "store", "Open store page")
                    .size(rems(3.3))
                    .on_click(move |_, _, cx| {
                        if let Some(url) = &store_url {
                            if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_none() {
                                cx.open_url(url);
                            }
                        } else {
                            store_owner
                                .update(cx, |this, cx| this.action(store_key.clone(), "store", cx))
                                .ok();
                        }
                    }),
            );
        }
        if self.mod_context.is_none() {
            let installable = exposed_games_backend::actions::can_install(&self.game);
            let working = self.download_progress.as_ref().filter(|p| p.blocks_play());
            let primary_action = if installable { "install" } else { "launch" };
            let primary_tip = working
                .map(crate::downloads::button_tip)
                .unwrap_or_else(|| {
                    if installable {
                        self.game
                            .install
                            .as_ref()
                            .and_then(|a| a.note.clone())
                            .unwrap_or_else(|| "Install".into())
                    } else {
                        "Play".into()
                    }
                });
            let uninstall_owner = self.owner.clone();
            let uninstall_key = self.game.key.clone();
            if exposed_games_backend::catalog::local(&self.game.installations)
                && self.game.uninstall.is_some()
                && working.is_none()
            {
                title = title.child(
                    action_button("details-uninstall", "trash", "Uninstall")
                        .size(rems(3.3))
                        .on_click(move |_, _, cx| {
                            uninstall_owner
                                .update(cx, |this, cx| {
                                    this.action(uninstall_key.clone(), "uninstall", cx)
                                })
                                .ok();
                        }),
                );
            }
            title = title.when(
                installable || self.game.launch.is_some() || working.is_some(),
                |d| {
                    d.child(
                        crate::header::button_shell("details-primary-action", false)
                            .size(rems(3.3))
                            .bg(rgba(0x101010b8))
                            .hover(|s| s.bg(rgba(0x303030b3)))
                            .child(if let Some(progress) = working {
                                icon(crate::downloads::button_icon(progress))
                                    .text_color(rgb(0xf0f8f3))
                                    .into_any_element()
                            } else {
                                icon(if installable { "install" } else { "play" })
                                    .text_color(rgb(0xf0f8f3))
                                    .into_any_element()
                            })
                            .tooltip(move |_, cx| {
                                cx.new(|_| DetailTooltip(primary_tip.clone())).into()
                            })
                            .when_some(
                                working
                                    .map(crate::downloads::button_label)
                                    .filter(|s| !s.is_empty()),
                                |d, label| d.w_auto().px(rems(1.)).gap(rems(0.5)).child(label),
                            )
                            .on_click(move |_, _, cx| {
                                action_owner
                                    .update(cx, |this, cx| {
                                        if !this.primary_download_action(&key, cx) {
                                            this.action(key.clone(), primary_action, cx);
                                        }
                                    })
                                    .ok();
                            }),
                    )
                },
            );
            if working.is_none() && self.game.mods.as_ref().is_some_and(|m| m.launchable) {
                let owner = self.owner.clone();
                let key = self.game.key.clone();
                title = title.child(
                    action_button("details-launch-modded", "mods", "Launch modded")
                        .size(rems(3.3))
                        .on_click(move |_, _, cx| {
                            owner
                                .update(cx, |this, cx| {
                                    this.action(key.clone(), "launch-modded", cx)
                                })
                                .ok();
                        }),
                );
            }
        } else {
            title = title.child(self.mod_actions(cx));
        }
        meta = meta.child(title);
        if let Some(error) = self.download_progress.as_ref().and_then(|p| {
            p.error.map(|e| e.message().to_owned()).or_else(|| {
                (p.phase == exposed_games_backend::steam_control::Phase::Failed).then(|| p.label())
            })
        }) {
            meta = meta.child(
                div()
                    .text_size(rems(0.95))
                    .text_color(rgb(0xe6c58d))
                    .whitespace_normal()
                    .child(error),
            );
        }
        let mut facts = Vec::new();
        if let Some(data) = &info {
            let mut badges = div().flex().flex_wrap().items_center().gap(rems(0.65));
            for (label, values) in [
                ("Developer", &data.developers),
                ("Publisher", &data.publishers),
            ] {
                if !values.is_empty() {
                    facts.push(format!("{label}: {}", values.join(", ")));
                }
            }
            if !data.released.is_empty() {
                facts.push(format!("Released: {}", data.released));
            }
            if let Some(score) = data.score() {
                let label = data.scores.join(" · ");
                badges = badges.child(
                    bubble()
                        .id("details-score")
                        .children(
                            (0..5).map(|i| rating_star((score - i as f64).clamp(0., 1.) as f32)),
                        )
                        .tooltip(move |_, cx| cx.new(|_| DetailTooltip(label.clone())).into()),
                );
            }
            if !data.genres.is_empty() {
                badges = badges.child(bubble().child(data.genres.join(" · ")));
            }
            if data.score().is_some() || !data.genres.is_empty() {
                meta = meta.child(badges);
            }
        }
        let rating = info
            .as_ref()
            .map(|data| data.age_rating.as_str())
            .filter(|r| !r.is_empty());
        let stats_block = self.stats_block(cx);
        if !facts.is_empty() || rating.is_some() || stats_block.is_some() {
            let mut row = div()
                .flex()
                .flex_wrap()
                .items_stretch()
                .self_start()
                .w_full()
                .max_w_full()
                .gap(rems(0.9));
            if facts.is_empty() && rating.is_some() {
                row = row.min_h(rems(8.7));
            }
            if !facts.is_empty() {
                row = row.child(
                    panel()
                        .min_w_0()
                        .max_w(rems(27.))
                        .p(rems(1.15))
                        .gap(rems(0.5))
                        .children(facts.into_iter().map(paragraph)),
                );
            }
            if let Some(rating) = rating {
                row = row.child(age_badge(rating));
            }
            if let Some(stats) = stats_block {
                row = row.child(stats);
            }
            meta = meta.child(row);
        }
        if info.is_none() && self.loading {
            meta = meta
                .child(
                    div()
                        .flex()
                        .gap(rems(0.65))
                        .child(skeleton_block(10.65, 3.3))
                        .child(skeleton_block(15., 3.3)),
                )
                .child(
                    div()
                        .flex()
                        .items_stretch()
                        .gap(rems(0.9))
                        .child(
                            panel()
                                .w(rems(24.))
                                .p(rems(1.15))
                                .gap(rems(0.5))
                                .children([0.75, 0.95, 0.6].into_iter().map(skeleton_line)),
                        )
                        .child(skeleton_block(6., 8.55)),
                );
        }
        body = body.child(hero.child(meta));
        if let Some(error) = &self.error {
            body = body.child(
                panel()
                    .child(heading(if self.data.is_some() {
                        "Showing saved information"
                    } else {
                        "Could not load game details"
                    }))
                    .child(muted(error.clone())),
            );
        }
        if let Some(data) = &info {
            let description = if data.description.is_empty() {
                "No description supplied for this edition.".into()
            } else {
                data.description.clone()
            };
            let about = if self.about_expanded {
                self.description_content(data, cx).into_any_element()
            } else {
                paragraph(description)
                    .line_clamp(3)
                    .max_h(rems(5.25))
                    .overflow_hidden()
                    .into_any_element()
            };
            body = body.child(
                panel().child(heading("About")).child(about).child(
                    text_button(
                        "about-more",
                        if self.about_expanded { "Less" } else { "More" },
                    )
                    .self_start()
                    .child(icon(if self.about_expanded {
                        "collapse"
                    } else {
                        "expand"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.about_expanded = !this.about_expanded;
                        if !this.about_expanded {
                            this.animations.clear();
                        }
                        cx.notify();
                    })),
                ),
            );
            if self.mod_context.is_none() || !data.media.is_empty() {
                body = body.child(self.gallery(data, window, cx));
            }
            if self.mod_context.is_none() || !data.requirements.is_empty() {
                body =
                body.child(
                    panel()
                        .child(heading("System requirements"))
                        .when(data.requirements.is_empty(), |d| {
                            d.child(muted("No system requirements supplied for this edition."))
                        })
                        .children(
                            data.requirements
                                .iter()
                                .filter(|r| !r.minimum.is_empty() || !r.recommended.is_empty())
                                .map(|r| {
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(rems(0.75))
                                        .when_some(
                                            r.platform
                                                .split_once(" - ")
                                                .map(|(_, label)| label.trim().to_owned()),
                                            |d, label| {
                                                d.child(
                                                    div()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(label),
                                                )
                                            },
                                        )
                                        .child(
                                            div().flex().gap(rems(1.5)).children(
                                                [
                                                    ("Minimum", &r.minimum),
                                                    ("Recommended", &r.recommended),
                                                ]
                                                .into_iter()
                                                .filter(|(_, text)| !text.is_empty())
                                                .map(|(label, text)| {
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(rems(0.5))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap(rems(0.5))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .child(system_icon(&r.platform))
                                                                .child(label),
                                                        )
                                                        .child(paragraph(
                                                            exposed_games_backend::details::requirement_text(text),
                                                        ))
                                                }),
                                            ),
                                        )
                                }),
                        ),
                );
            }
            if self.mod_context.is_none() || !data.reviews.is_empty() {
                body = body.child(self.reviews_content(data, cx));
            }
        } else if self.loading {
            body = body.child(content_skeleton());
        }
        let dropdown = self.review_menu(window, cx);
        let achievements = self.achievements_popup(cx);
        let owner = cx.weak_entity();
        let scroll_input = canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                        owner
                            .update(cx, |this, cx| this.scroll(event, window, cx))
                            .ok();
                    }
                })
            },
        )
        .absolute()
        .inset_0();
        div()
            .absolute()
            .inset_0()
            .occlude()
            .child(scroll_input)
            .child(
                div()
                    .id("details-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .when(!self.fullscreen, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(rems(0.7))
                        .left(rems(1.1))
                        .right(rems(1.1))
                        .flex()
                        .justify_between()
                        .child(back)
                        .child(controls),
                )
            })
            .child(dropdown)
            .child(achievements)
            .when_some(
                self.player.clone().filter(|_| self.fullscreen),
                |d, player| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .occlude()
                            .bg(rgb(0x030907))
                            .child(player),
                    )
                },
            )
    }
}
