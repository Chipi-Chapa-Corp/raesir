use crate::{
    Launcher,
    backend::Backend,
    game_activity::{PlayButton, game_action_button},
    header::{self, Tooltip, action_button, pill, spinner},
    icons::{icon, rating_image, system_icon},
    smooth_scroll::SmoothWheel,
    views::pill_button as text_button,
};
use gpui::{prelude::*, *};
use raesir_backend::{details::Details, model::LibraryGame};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Instant,
};
mod description;
mod header_art;
use header_art::{Format as ArtFormat, HeaderArt, Poster};
mod mod_view;
mod player_stats;
mod reviews;
use unicode_segmentation::UnicodeSegmentation;

pub struct DetailsPage {
    game: LibraryGame,
    mod_context: Option<ModContext>,
    _mod_subscription: Option<Subscription>,
    download_progress: Option<raesir_backend::steam_control::Progress>,
    play_button: Option<PlayButton>,
    compatibility: Option<crate::library::CompatibilityStatus>,
    poster: Option<Arc<RenderImage>>,
    poster_blur: Option<Arc<RenderImage>>,
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
    stats: Option<Arc<raesir_backend::player_stats::PlayerStats>>,
    stats_task: Option<Task<()>>,
    stats_loading: bool,
    achievements_open: bool,
    hidden_achievements_expanded: bool,
    achievement_rows: ListState,
    achievement_order: Vec<usize>,
    achievement_focus: FocusHandle,
    achievement_wheel: SmoothWheel,
    achievement_frame_pending: bool,
    images: HashMap<String, Arc<RenderImage>>,
    image_blurs: HashMap<String, Arc<RenderImage>>,
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
        download_progress: Option<raesir_backend::steam_control::Progress>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            game: source,
            download_progress,
            play_button: None,
            compatibility: None,
            poster_blur: poster.as_deref().and_then(crate::artwork::blurred_render),
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
            achievement_focus: cx.focus_handle(),
            achievement_wheel: SmoothWheel::default(),
            achievement_frame_pending: false,
            images: HashMap::new(),
            image_blurs: HashMap::new(),
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
                    let poster = browser.read(cx).mod_poster(&context.id);
                    if let Some(version) = browser.read(cx).mod_version(&context.id) {
                        context.version = version;
                    }
                    this.set_poster(poster);
                }
                cx.notify();
            })
        });
        page
    }

    pub(crate) fn load_mod(
        &mut self,
        game: LibraryGame,
        id: String,
        nexus_domain: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        self.loading = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.mod_details(&game, &id, nexus_domain.as_deref()) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut data) => {
                        if data.description.is_empty()
                            && let Some(previous) = &this.data
                        {
                            data.description.clone_from(&previous.description);
                        }
                        this.data = Some(Arc::new(data));
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// Viewer images may resolve to a larger file than the gallery image for one URL.
fn media_key(url: &str, full_size: bool) -> String {
    if full_size {
        format!("viewer\n{url}")
    } else {
        url.to_owned()
    }
}

/// Cached details younger than six hours with complete store metadata skip the refresh.
fn details_fresh(data: &Details, game: &LibraryGame) -> bool {
    raesir_backend::util::unix_now().saturating_sub(data.updated) < 21600
        && (data.description.is_empty() || data.description_html.is_some())
        && (!matches!(game.store.as_str(), "epic" | "ea" | "ubisoft") || data.store_url.is_some())
}

fn header_frame() -> Div {
    div()
        .relative()
        .flex_none()
        .rounded(rems(1.4))
        .overflow_hidden()
        .bg(rgba(0x101010b3))
        .shadow_lg()
}

fn cover_contents(frame: Div, poster: Option<Poster>) -> Div {
    match poster {
        Some((image, blurred)) => image_layers(frame, image, blurred, 1.4),
        None => frame
            .flex()
            .items_center()
            .justify_center()
            .child(icon("mods").size(rems(3.)).text_color(rgb(0x777b7a))),
    }
}

fn image_layers(
    frame: Div,
    image: Arc<RenderImage>,
    blurred: Option<Arc<RenderImage>>,
    radius: f32,
) -> Div {
    frame
        .when_some(blurred, |d, blurred| {
            d.child(
                img(blurred)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .aspect_ratio(1.)
                    .rounded(rems(radius))
                    .object_fit(ObjectFit::Fill),
            )
        })
        .child(
            img(image)
                .absolute()
                .inset_0()
                .size_full()
                .aspect_ratio(1.)
                .rounded(rems(radius))
                .object_fit(ObjectFit::Contain),
        )
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
        .tooltip(move |_, cx| cx.new(|_| Tooltip(label.clone())).into())
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
fn facts_panel() -> Div {
    panel()
        .min_w_0()
        .max_w(rems(27.))
        .p(rems(1.15))
        .gap(rems(0.5))
}

fn about_skeleton() -> Div {
    panel()
        .child(heading("About"))
        .child(skeleton_lines(&[1., 0.98, 0.72]))
        .child(skeleton_block(6.55, 3.).rounded_full())
}

fn gallery_frame() -> Div {
    div()
        .relative()
        .w_full()
        .aspect_ratio(16. / 9.)
        .rounded(rems(1.2))
        .overflow_hidden()
        .shadow_md()
        .bg(rgba(0x101010b3))
}

fn content_skeleton() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(1.25))
        .child(about_skeleton())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rems(0.9))
                .child(gallery_frame())
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .gap(rems(0.6))
                        .h(rems(4.))
                        .overflow_hidden()
                        .children(
                            (0..6).map(|_| skeleton_block(7., 4.).flex_none().rounded(rems(0.7))),
                        ),
                ),
        )
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
        let mut source = game.source.clone();
        source.title.clone_from(&game.title);
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
            page.update_play_button(self, cx);
            page.update_compatibility(self, cx);
            page.load(cx);
            page.load_stats(cx);
            page.check_proton_reviews(cx);
        });
        self.details = Some(page);
        cx.notify();
    }
}
impl DetailsPage {
    pub(crate) fn game(&self) -> &LibraryGame {
        &self.game
    }

    pub(crate) fn update_compatibility(&mut self, launcher: &Launcher, cx: &mut Context<Self>) {
        let status = launcher
            .compatibility
            .status(&self.game, &self.game.title, launcher.host);
        if status != self.compatibility {
            self.compatibility = status;
            cx.notify();
        }
    }

    pub(crate) fn update_play_button(&mut self, launcher: &Launcher, cx: &mut Context<Self>) {
        let play_button = launcher.play_button(&self.game.key);
        if self.play_button != play_button {
            self.play_button = play_button;
            cx.notify();
        }
    }

    pub fn library_changed(&mut self, launcher: &Launcher, cx: &mut Context<Self>) {
        if let Some(game) = launcher
            .games
            .iter()
            .find(|g| g.source.key == self.game.key)
        {
            self.game = game.source.clone();
            self.game.title.clone_from(&game.title);
        }
        self.download_progress = launcher.download_operations.get(&self.game.key).cloned();
        self.update_compatibility(launcher, cx);
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
            self.set_poster(Some(poster));
            if !self.backdrop_ready {
                self.backdrop
                    .update(cx, |backdrop, cx| backdrop.retarget(background, cx));
            }
            cx.notify();
        }
    }
    fn set_poster(&mut self, poster: Option<Arc<RenderImage>>) {
        if self.poster.as_ref().map(|image| image.id) == poster.as_ref().map(|image| image.id) {
            return;
        }
        self.poster_blur = poster.as_deref().and_then(crate::artwork::blurred_render);
        self.poster = poster;
    }

    fn header_image(&self) -> Option<Poster> {
        if let Some(poster) = &self.poster {
            return Some((poster.clone(), self.poster_blur.clone()));
        }
        let first = self.data.as_ref()?.first_screenshot()?;
        let original = first.poster_url();
        [&original, first.thumbnail_url()]
            .into_iter()
            .find_map(|url| {
                let image = self.images.get(url)?;
                Some((image.clone(), self.image_blurs.get(url).cloned()))
            })
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        // A page opened earlier this session renders on its first frame.
        let remembered = backend.remembered_details(&game);
        if let Some(data) = &remembered {
            self.load_background(data, cx);
            self.data = Some(data.clone());
        }
        self.loading = remembered
            .as_ref()
            .is_none_or(|data| !details_fresh(data, &game));
        self.task = Some(cx.spawn(async move |this, cx| {
            let cached = match remembered {
                Some(data) => Some(data),
                None => {
                    let cached = cx
                        .background_executor()
                        .spawn({
                            let backend = backend.clone();
                            let game = game.clone();
                            async move {
                                let data = backend.cached_details(&game)?;
                                Some(backend.remember_details(&game, data))
                            }
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        if let Some(data) = &cached {
                            this.load_background(data, cx);
                            this.data = Some(data.clone());
                        }
                        this.loading = cached
                            .as_ref()
                            .is_none_or(|data| !details_fresh(data, &this.game));
                        cx.notify();
                    })
                    .ok();
                    cached
                }
            };
            let fresh = cached
                .as_ref()
                .is_some_and(|data| details_fresh(data, &game));
            let needs_author_names = cached
                .as_ref()
                .and_then(|data| data.steam_reviews.as_ref())
                .is_some_and(|pages| pages.needs_author_names());
            if fresh {
                if needs_author_names {
                    let details = cx
                        .background_executor()
                        .spawn(async move {
                            let details = backend.refresh_cached_review_authors(&game)?;
                            Some(backend.remember_details(&game, details))
                        })
                        .await;
                    if let Some(details) = details {
                        this.update(cx, |this, cx| {
                            this.apply_review_author_names(&details);
                            this.data = Some(details);
                            cx.notify();
                        })
                        .ok();
                    }
                }
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let data = backend.details(&game)?;
                    Ok::<_, String>(backend.remember_details(&game, data))
                })
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
                        this.data = Some(data);
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
        self.request_image(url, false, cx);
    }

    fn viewer_image(&mut self, url: &str, cx: &mut Context<Self>) {
        self.request_image(url, true, cx);
    }

    fn request_image(&mut self, url: &str, full_size: bool, cx: &mut Context<Self>) {
        if url.is_empty() || self.images.contains_key(url) {
            return;
        }
        if let Some(image) = self.backend.cached_achievement_icon(url) {
            self.images.insert(url.to_owned(), image);
            return;
        }
        if let Some((image, blurred)) = self.backend.remembered_media(&media_key(url, full_size)) {
            if let Some(blurred) = blurred {
                self.image_blurs.insert(url.to_owned(), blurred);
            }
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
                        return backend
                            .achievement_icon(&request, true)
                            .map(|image| (image, None));
                    }
                    let path = if full_size {
                        backend.detail_viewer_media(&request)?
                    } else {
                        backend.detail_media(&request)?
                    };
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
                    for p in pixels.as_chunks_mut::<4>().0 {
                        p.swap(0, 2);
                    }
                    let image = Arc::new(RenderImage::new(vec![image_codec::Frame::new(pixels)]));
                    let blurred = crate::artwork::blurred_render(&image);
                    backend.remember_media(
                        media_key(&request, full_size),
                        (image.clone(), blurred.clone()),
                    );
                    Ok::<_, String>((image, blurred))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pending.remove(&url);
                match result {
                    Ok((image, blurred)) => {
                        if let Some(blurred) = blurred {
                            this.image_blurs.insert(url.clone(), blurred);
                        }
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
        let thumbnail = media.thumbnail_url();
        self.viewer_image(&url, cx);
        let first = (-f32::from(self.media_scroll.offset().x)
            / (f32::from(window.rem_size()) * 7.6)) as usize;
        for item in data.media.iter().skip(first).take(12) {
            self.image(item.thumbnail_url(), cx);
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
            && [url.as_str(), thumbnail]
                .iter()
                .any(|&url| !url.is_empty() && !self.failed.contains(url));
        let blurred = self
            .image_blurs
            .get(&url)
            .or_else(|| self.image_blurs.get(thumbnail))
            .cloned();
        let count = data.media.len();
        if count == 1 {
            self.strip_bounds.set(Bounds::default());
        }
        let previous = (self.selected + count - 1) % count;
        let next = (self.selected + 1) % count;
        let viewer = gallery_frame()
            .when_some(image, |d, image| image_layers(d, image, blurred, 1.2))
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
            .child(div().relative().child(viewer).when(count > 1, |d| {
                d.child(
                    action_button("media-previous", "back", "Previous media")
                        .absolute()
                        .left(rems(-3.25))
                        .top(relative(0.5))
                        .mt(rems(-1.3))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(previous, cx))),
                )
                .child(
                    action_button("media-next", "next", "Next media")
                        .absolute()
                        .right(rems(-3.25))
                        .top(relative(0.5))
                        .mt(rems(-1.3))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(next, cx))),
                )
            }))
            .when(count > 1, |d| {
                d.child(
                    div()
                        .id("media-strip")
                        .flex()
                        .gap(rems(0.6))
                        .w_full()
                        .h(rems(4.))
                        .flex_none()
                        .overflow_x_scroll()
                        .track_scroll(&self.media_scroll)
                        .on_prepaint(move |paint, _, _| strip_bounds.set(paint.bounds))
                        .child(div().flex().flex_none().mx_auto().gap(rems(0.6)).children(
                            data.media.iter().enumerate().map(|(i, item)| {
                                let url = item.thumbnail_url();
                                div()
                                    .id(("media-thumb", i))
                                    .w(rems(7.))
                                    .h(rems(4.))
                                    .flex_none()
                                    .relative()
                                    .rounded(rems(0.7))
                                    .bg(rgba(0x101010b3))
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .opacity(if i == self.selected { 1. } else { 0.65 })
                                    .hover(|s| s.opacity(1.))
                                    .when_some(self.images.get(url).cloned(), |d, image| {
                                        let blurred = self.image_blurs.get(url).cloned();
                                        d.when_some(blurred, |d, blurred| {
                                            d.child(
                                                img(blurred)
                                                    .absolute()
                                                    .top_0()
                                                    .left_0()
                                                    .w(rems(7.))
                                                    .h(rems(4.))
                                                    .rounded(rems(0.7))
                                                    .object_fit(ObjectFit::Fill),
                                            )
                                        })
                                        .child(
                                            img(image)
                                                .absolute()
                                                .top_0()
                                                .left_0()
                                                .w(rems(7.))
                                                .h(rems(4.))
                                                .rounded(rems(0.7))
                                                .object_fit(ObjectFit::Contain),
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
impl DetailsPage {
    fn game_header(&mut self, info: Option<&Details>, cx: &mut Context<Self>) -> HeaderArt {
        let action_owner = self.owner.clone();
        let store_owner = self.owner.clone();
        let library_owner = self.owner.clone();
        let key = self.game.key.clone();
        let store_key = key.clone();
        let library_key = key.clone();
        let mut meta = div().flex_1().min_w_0().flex().flex_col().gap(rems(0.9));
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
        let client = raesir_backend::catalog::client_label(&self.game.client);
        let handoff = raesir_backend::actions::library_handoff_allowed(&self.game);
        let platform = bubble()
            .id("details-client")
            .when(handoff, |platform| {
                platform.cursor_pointer().hover(|s| s.bg(rgba(0x303030b3)))
            })
            .child(icon(client_icon(client, &self.game.store)))
            .child(client.to_owned());
        let client_label = format!("Open in {client} library");
        let badge = self
            .compatibility
            .and_then(|status| Some((status.badge?, status.label)));
        title = title
            .when_some(badge, |title, (badge, label)| {
                title.child(
                    pill()
                        .id("details-compatibility")
                        .size(rems(3.3))
                        .flex_none()
                        .justify_center()
                        .aria_label(label)
                        .child(badge.icon().size(rems(1.55)))
                        .tooltip(move |_, cx| {
                            cx.new(|_| crate::header::Tooltip(label.into())).into()
                        }),
                )
            })
            .child(if !handoff {
                platform
            } else if self
                .game
                .client_os
                .contains(&raesir_backend::model::Os::host())
            {
                platform
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(client_label.clone())).into())
                    .on_click(move |_, _, cx| {
                        library_owner
                            .update(cx, |this, cx| {
                                this.action(library_key.clone(), "library", cx)
                            })
                            .ok();
                    })
            } else {
                platform.opacity(0.6).cursor_default().tooltip(|_, cx| {
                    cx.new(|_| Tooltip("Client library is on another OS".into()))
                        .into()
                })
            })
            .when_some(
                self.compatibility.filter(|status| status.badge.is_none()),
                |title, status| {
                    title.child(
                        bubble()
                            .id("details-xbox-compatibility")
                            .child(status.label),
                    )
                },
            );
        let mut actions = div().flex().flex_wrap().items_center().gap(rems(0.65));
        let show_mods = raesir_backend::mods::can_browse_mods(&self.game);
        if show_mods {
            let owner = self.owner.clone();
            let key = self.game.key.clone();
            actions = actions.child(
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
        let store_url = info.and_then(|v| v.store_url.clone());
        if self.game.store_page.is_some() || store_url.is_some() {
            title = title.child(
                action_button("details-store", "store", "Open store page")
                    .size(rems(3.3))
                    .on_click(move |_, _, cx| {
                        if let Some(url) = &store_url {
                            if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
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
        let installable = raesir_backend::actions::can_install(&self.game);
        let play_button = self.play_button;
        let working = self.download_progress.as_ref().filter(|p| {
            play_button.is_none()
                && raesir_backend::actions::install_limitation(&self.game).is_none()
                && crate::downloads::has_primary_action(p)
        });
        let primary_action = if installable { "install" } else { "launch" };
        let primary_tip = play_button
            .map(|button| button.label().to_owned())
            .or_else(|| working.map(crate::downloads::button_tip))
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
        if raesir_backend::catalog::local(&self.game.installations)
            && self.game.uninstall.is_some()
            && !self.download_progress.as_ref().is_some_and(|p| p.active())
            && play_button.is_none()
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
        let show_primary =
            installable || self.game.launch.is_some() || working.is_some() || play_button.is_some();
        actions = actions.when(show_primary, |d| {
            d.child(
                game_action_button(
                    play_button.map_or("details-primary-action", PlayButton::element_id),
                    play_button,
                    working,
                    if installable { "install" } else { "play" },
                    Some(primary_tip),
                )
                .h(rems(3.3))
                .min_w(rems(3.3))
                .on_click(move |_, _, cx| {
                    action_owner
                        .update(cx, |this, cx| match play_button {
                            Some(PlayButton::Quit) => this.action(key.clone(), "quit", cx),
                            None if !this.primary_download_action(&key, cx) => {
                                this.action(key.clone(), primary_action, cx);
                            }
                            _ => {}
                        })
                        .ok();
                }),
            )
        });
        let game_folder = raesir_backend::mods::local_game_folder(&self.game);
        let show_folder = game_folder.is_some();
        if let Some(path) = game_folder {
            actions = actions.child(
                action_button(
                    "details-game-folder",
                    "document-open-symbolic",
                    "Open game folder",
                )
                .size(rems(3.3))
                .on_click(move |_, _, cx| {
                    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                        #[cfg(windows)]
                        cx.open_url(&path.to_string_lossy());
                        #[cfg(not(windows))]
                        cx.reveal_path(&path);
                    }
                }),
            );
        }
        if play_button.is_none()
            && working.is_none()
            && self.game.mods.as_ref().is_some_and(|m| m.launchable)
        {
            let owner = self.owner.clone();
            let key = self.game.key.clone();
            title = title.child(
                action_button("details-launch-modded", "mods", "Launch modded")
                    .size(rems(3.3))
                    .on_click(move |_, _, cx| {
                        owner
                            .update(cx, |this, cx| this.action(key.clone(), "launch-modded", cx))
                            .ok();
                    }),
            );
        }
        let action_error = self.download_progress.as_ref().and_then(|p| {
            p.error.map(|e| e.message().to_owned()).or_else(|| {
                (p.phase == raesir_backend::steam_control::Phase::Failed).then(|| p.label())
            })
        });
        let mut facts = Vec::new();
        let mut detail_badges = None;
        if let Some(data) = info {
            let mut badges = div().flex().flex_wrap().items_center().gap(rems(0.65));
            let creator = "Developer";
            for (label, values) in [(creator, &data.developers), ("Publisher", &data.publishers)] {
                if !values.is_empty() {
                    facts.push(format!("{label}: {}", values.join(", ")));
                }
            }
            if !data.released.is_empty() {
                facts.push(format!("Released: {}", data.released));
            }
            if let Some(metadata) = data
                .mod_metadata
                .as_ref()
                .filter(|m| !m.last_updated.is_empty())
            {
                facts.push(format!("Updated: {}", metadata.last_updated));
            }
            if let Some(score) = data.score() {
                let label = data.scores.join(" · ");
                badges = badges.child(
                    bubble()
                        .id("details-score")
                        .children(
                            (0..5).map(|i| rating_star((score - i as f64).clamp(0., 1.) as f32)),
                        )
                        .tooltip(move |_, cx| cx.new(|_| Tooltip(label.clone())).into()),
                );
            }
            if !data.genres.is_empty() {
                badges = badges.child(bubble().child(data.genres.join(" · ")));
            }
            if data.score().is_some() || !data.genres.is_empty() {
                detail_badges = Some(badges);
            }
        }
        meta = meta
            .child(title)
            .when(show_mods || show_primary || show_folder, |d| {
                d.child(actions)
            })
            .when_some(action_error, |d, error| {
                d.child(
                    div()
                        .text_size(rems(0.95))
                        .text_color(rgb(0xe6c58d))
                        .whitespace_normal()
                        .child(error),
                )
            })
            .children(detail_badges);
        let metadata_loading = info.is_none() && self.loading;
        if metadata_loading {
            meta = meta.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(rems(0.65))
                    .child(skeleton_block(10.65, 3.3).rounded_full())
                    .child(skeleton_block(15., 3.3).rounded_full()),
            );
        }
        let rating = info
            .map(|data| data.age_rating.as_str())
            .filter(|r| !r.is_empty());
        let stats_block = self.stats_block(cx);
        if metadata_loading || !facts.is_empty() || rating.is_some() || stats_block.is_some() {
            let facts = if metadata_loading {
                Some(
                    facts_panel()
                        .w(rems(15.))
                        .children([0.95, 0.86, 0.71].map(skeleton_line)),
                )
            } else {
                (!facts.is_empty())
                    .then(|| facts_panel().children(facts.into_iter().map(paragraph)))
            };
            let age_only = rating.is_some() && facts.is_none() && stats_block.is_none();
            meta = meta.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_stretch()
                    .self_start()
                    .w_full()
                    .max_w_full()
                    .gap(rems(0.9))
                    .when(age_only, |d| d.min_h(rems(8.7)))
                    .children(facts)
                    .when_some(rating, |d, rating| d.child(age_badge(rating)))
                    .children(stats_block),
            );
        }
        HeaderArt {
            poster: self.header_image(),
            metadata: Some(meta.w_full().into_any_element()),
            format: ArtFormat::Game,
            // Placeholders match the loaded header height, so the cover sizes from them.
            fixed_cover: false,
        }
    }
}

impl Render for DetailsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.cleanup_registered {
            cx.on_release_in(window, |this, window, _| {
                for image in this
                    .images
                    .drain()
                    .chain(this.image_blurs.drain())
                    .map(|(_, image)| image)
                    .chain(this.poster_blur.take())
                {
                    let _ = window.drop_image(image);
                }
            })
            .detach();
            self.cleanup_registered = true;
        }
        let owner = self.owner.clone();
        let mod_back = self
            .mod_context
            .as_ref()
            .map(|context| context.browser.clone());
        let back = header::back_button("details-back")
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
        if self.poster.is_none()
            && let Some(media) = info.as_ref().and_then(|info| info.first_screenshot())
        {
            let url = media.poster_url();
            self.image(&url, cx);
        }
        body = if self.mod_context.is_some() {
            body.child(self.mod_header(cx))
        } else {
            body.child(self.game_header(info.as_deref(), cx))
        };
        if let Some(error) = &self.error {
            body = body.child(
                panel()
                    .child(heading(if self.data.is_some() {
                        "Showing saved information"
                    } else if self.mod_context.is_some() {
                        "Could not load mod details"
                    } else {
                        "Could not load game details"
                    }))
                    .child(muted(error.clone())),
            );
        }
        if let Some(data) = &info {
            let description = if data.description.is_empty() {
                if self.mod_context.is_some() {
                    "No description supplied for this mod.".into()
                } else {
                    "No description supplied for this edition.".into()
                }
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
            if self.loading && data.description.is_empty() {
                body = body.child(about_skeleton());
            } else {
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
            }
            if let Some(metadata) = data
                .mod_metadata
                .as_ref()
                .filter(|m| !m.requirements.is_empty())
            {
                body = body.child(
                    panel().child(heading("Mod requirements")).children(
                        metadata
                            .requirements
                            .iter()
                            .enumerate()
                            .map(|(i, requirement)| {
                                let item = div().flex().flex_col().items_start().gap(rems(0.35));
                                let item = if let Some(url) = requirement.url.clone() {
                                    item.child(
                                        text_button(
                                            ("mod-requirement", i),
                                            requirement.name.clone(),
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                if std::env::var_os("RAESIR_DISABLE_ACTIONS")
                                                    .is_none()
                                                {
                                                    cx.open_url(&url);
                                                }
                                            },
                                        ),
                                    )
                                } else {
                                    item.child(paragraph(requirement.name.clone()))
                                };
                                item.when(!requirement.notes.is_empty(), |item| {
                                    item.child(muted(requirement.notes.clone()))
                                })
                            }),
                    ),
                );
            }
            if self.mod_context.is_none() || !data.media.is_empty() {
                body = body.child(self.gallery(data, window, cx));
            }
            if let Some(metadata) = &data.mod_metadata {
                if !metadata.changelog.is_empty() {
                    body = body.child(self.mod_posts("Changelog", &metadata.changelog));
                }
                if !metadata.comments.is_empty() {
                    body = body.child(self.mod_comments(&metadata.comments, cx));
                }
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
                                                            raesir_backend::details::requirement_text(text),
                                                        ))
                                                }),
                                            ),
                                        )
                                }),
                        ),
                );
            }
            if self.mod_context.is_none() || !data.reviews.is_empty() {
                body = body.child(self.reviews_content(Some(data), window, cx));
            }
        } else if self.loading {
            body = body.child(content_skeleton());
        } else if self.has_proton_reviews() {
            body = body.child(self.reviews_content(None, window, cx));
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
                        .when(crate::client_titlebar(window), |d| {
                            d.child(header::window_controls(
                                self.owner.clone(),
                                header::BUTTON_WIDTH,
                            ))
                        }),
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
