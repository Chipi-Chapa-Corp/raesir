use crate::{Launcher, backend::Backend};
use gpui::{Context, RenderImage, Window};
use raesir_backend::model::LibraryGame;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const CARD_CACHE_VERSION: &str = "finished-card-v7-footer-256";
const DOWNLOADS: usize = 12;
const PREFETCH_DOWNLOADS: usize = 6;
const MAX_DOWNLOADS: usize = 24;
const DISK_LOADS: usize = 8;
const HOT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Disk,
    Download,
}

pub struct Job {
    lane: Lane,
    cancel: Arc<AtomicBool>,
    foreground: bool,
}

#[derive(Clone)]
pub(crate) struct Ready {
    pub poster: Arc<RenderImage>,
    pub title: Option<String>,
    pub background: Arc<RenderImage>,
    pub card: Arc<RenderImage>,
}

impl Ready {
    fn bytes(&self) -> usize {
        [&self.poster, &self.background, &self.card]
            .into_iter()
            .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len))
            .sum()
    }
}

#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Ready>,
    recent: VecDeque<String>,
    bytes: usize,
    retired: Vec<Arc<RenderImage>>,
    pub memory_hits: usize,
    pub disk_hits: usize,
    pub downloads: usize,
    pub peak_downloads: usize,
    pub peak_disk_loads: usize,
    pub cancelled_jobs: usize,
    viewport: Option<(usize, usize)>,
    up: bool,
}

impl Cache {
    pub(crate) fn peek(&self, key: &str) -> Option<Ready> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn get(&mut self, key: &str) -> Option<Ready> {
        let ready = self.entries.get(key)?.clone();
        self.recent.retain(|item| item != key);
        self.recent.push_back(key.to_owned());
        self.memory_hits += 1;
        Some(ready)
    }
    pub(crate) fn insert(&mut self, key: String, ready: Ready) {
        if let Some(old) = self.entries.remove(&key) {
            self.retire(old);
        }
        self.recent.retain(|item| item != &key);
        self.bytes += ready.bytes();
        self.entries.insert(key.clone(), ready);
        self.recent.push_back(key);
        self.trim_to(HOT_BYTES);
    }
    pub(crate) fn trim_to(&mut self, budget: usize) {
        while self.bytes > budget {
            let Some(key) = self.recent.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&key) {
                self.retire(old);
            }
        }
    }
    fn retire(&mut self, ready: Ready) {
        self.bytes -= ready.bytes();
        self.retired
            .extend([ready.poster, ready.card, ready.background]);
    }
    pub(crate) fn release_textures(&mut self, window: &mut Window) {
        self.retired.retain(|image| {
            if Arc::strong_count(image) > 1 {
                return true;
            }
            let _ = window.drop_image(image.clone());
            false
        });
    }
}

pub(crate) fn load_cancelable(
    backend: &Backend,
    game: &LibraryGame,
    cancel: &AtomicBool,
) -> Result<Ready, String> {
    let (ready, persist) = prepare(backend, game, cancel)?;
    if let Some(persist) = persist {
        queue_persist(persist);
    }
    Ok(ready)
}

impl Cache {
    pub(crate) fn clear(&mut self, window: &mut Window) {
        self.trim_to(0);
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
    }
}

/// Retain recent decoded cards and load the viewport before neighboring rows.
pub fn maintain(this: &mut Launcher, window: &mut Window, cx: &mut Context<Launcher>) {
    if this.details.is_some() {
        return;
    }
    let row = this.rows.logical_scroll_top().item_ix.saturating_sub(1);
    let rows = (this.viewport_height / this.row_height.max(1.)).ceil() as usize + 1;
    let first = (row * this.columns).min(this.visible.len());
    let after = ((row + rows) * this.columns).min(this.visible.len());
    if let Some((previous, _)) = this.artwork_cache.viewport
        && first != previous
    {
        this.artwork_cache.up = first < previous;
    }
    let up = this.artwork_cache.up;
    let start = row.saturating_sub(if up { 8 } else { 2 }) * this.columns;
    let end = ((row + rows + if up { 2 } else { 8 }) * this.columns).min(this.visible.len());
    this.artwork_cache.viewport = Some((first, after));
    let mut foreground: HashSet<_> = this.visible[first..after]
        .iter()
        .map(|i| this.games[*i].key.clone())
        .collect();
    foreground.extend(
        this.downloads_visible
            .iter()
            .map(|key| Arc::<str>::from(key.as_str())),
    );
    let mut keep: HashSet<_> = this
        .visible
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .copied()
        .collect();
    for (index, game) in this.games.iter().enumerate() {
        if this.downloads_visible.contains(&game.source.key) {
            keep.insert(index);
        }
    }
    let mut wanted: HashMap<_, _> = this
        .visible
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(offset, i)| (this.games[*i].key.clone(), start + offset))
        .collect();
    for key in &this.downloads_visible {
        wanted
            .entry(Arc::<str>::from(key.as_str()))
            .or_insert(first);
    }
    for (index, game) in this.games.iter_mut().enumerate() {
        if !keep.contains(&index) && game.poster.is_some() {
            game.poster = None;
            game.card_poster = None;
            game.background = None;
            game.background_checked = false;
            this.artwork_attempted.remove(&game.source.key);
        }
    }
    for (key, job) in &mut this.artwork_pending {
        job.foreground = foreground.contains(key.as_str());
        if job.lane == Lane::Download
            && !wanted.contains_key(key.as_str())
            && !job.cancel.swap(true, Ordering::Relaxed)
        {
            this.artwork_cache.cancelled_jobs += 1;
        }
    }
    // Nearby prefetch may finish, but cannot crowd out the new viewport.
    let mut background: Vec<_> = this
        .artwork_pending
        .iter()
        .filter(|(_, job)| {
            job.lane == Lane::Download && !job.foreground && !job.cancel.load(Ordering::Relaxed)
        })
        .map(|(key, _)| {
            (
                key.clone(),
                wanted.get(key.as_str()).copied().unwrap_or(usize::MAX),
            )
        })
        .collect();
    background.sort_by_key(|(_, position)| {
        (
            if up {
                *position >= after
            } else {
                *position < first
            },
            position.abs_diff(first),
        )
    });
    for (key, _) in background.into_iter().skip(PREFETCH_DOWNLOADS) {
        if !this.artwork_pending[&key]
            .cancel
            .swap(true, Ordering::Relaxed)
        {
            this.artwork_cache.cancelled_jobs += 1;
        }
    }
    this.artwork_cache.release_textures(window);
    let mut indices: Vec<_> = this.visible[first..after].to_vec();
    if up {
        indices.extend(
            this.visible
                .get(start..first)
                .unwrap_or_default()
                .iter()
                .rev()
                .copied(),
        );
        indices.extend(
            this.visible
                .get(after..end)
                .unwrap_or_default()
                .iter()
                .copied(),
        );
    } else {
        indices.extend(
            this.visible
                .get(after..end)
                .unwrap_or_default()
                .iter()
                .copied(),
        );
        indices.extend(
            this.visible
                .get(start..first)
                .unwrap_or_default()
                .iter()
                .rev()
                .copied(),
        );
    }
    for index in indices {
        let foreground = this.visible[first..after].contains(&index);
        request_prioritized(this, index, foreground, cx);
    }
}

pub fn request(this: &mut Launcher, index: usize, cx: &mut Context<Launcher>) {
    request_prioritized(this, index, true, cx);
}

fn request_prioritized(
    this: &mut Launcher,
    index: usize,
    foreground: bool,
    cx: &mut Context<Launcher>,
) {
    let game = &this.games[index];
    if this.closing || game.poster.is_some() || this.artwork_pending.contains_key(&game.source.key)
    {
        return;
    }
    let cache_key = key(&game.source);
    if let Some(ready) = this.artwork_cache.get(&cache_key) {
        apply(this, index, ready, cx);
        return;
    }
    if this.artwork_attempted.contains(&game.source.key) {
        return;
    }
    let lane = if prepared(
        &this.backend.cache,
        &cache_key,
        raesir_backend::artwork::fallback_applies(&game.source),
    ) {
        Lane::Disk
    } else {
        Lane::Download
    };
    let limit = if lane == Lane::Disk {
        DISK_LOADS
    } else if !foreground {
        PREFETCH_DOWNLOADS
    } else {
        DOWNLOADS
    };
    if this
        .artwork_pending
        .values()
        .filter(|job| {
            job.lane == lane
                && !job.cancel.load(Ordering::Relaxed)
                && (lane == Lane::Disk || job.foreground == foreground)
        })
        .count()
        >= limit
    {
        return;
    }
    if lane == Lane::Download
        && this
            .artwork_pending
            .values()
            .filter(|job| job.lane == Lane::Download)
            .count()
            >= MAX_DOWNLOADS
    {
        return;
    }
    let source = game.source.clone();
    let game_key = source.key.clone();
    this.artwork_attempted.insert(game_key.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    this.artwork_pending.insert(
        game_key.clone(),
        Job {
            lane,
            cancel: cancel.clone(),
            foreground,
        },
    );
    let active = this
        .artwork_pending
        .values()
        .filter(|job| job.lane == lane && !job.cancel.load(Ordering::Relaxed))
        .count();
    if lane == Lane::Disk {
        this.artwork_cache.peak_disk_loads = this.artwork_cache.peak_disk_loads.max(active);
    } else {
        this.artwork_cache.peak_downloads = this.artwork_cache.peak_downloads.max(active);
    }
    let backend = this.backend.clone();
    let (send, receive) = futures::channel::oneshot::channel();
    // Blocking HTTP gets dedicated bounded workers, independent of the UI and
    // GPUI's shared executor. Cached cards have a separate worker lane.
    let worker_source = source.clone();
    let worker_cancel = cancel.clone();
    if std::thread::Builder::new()
        .name("raesir-artwork".into())
        .spawn(move || {
            let result = if lane == Lane::Disk {
                load_cached(&backend.cache, &worker_source).map(|ready| (ready, None))
            } else {
                prepare(&backend, &worker_source, &worker_cancel)
            };
            match result {
                Ok((ready, persist)) => {
                    let _ = send.send(Ok(ready));
                    if let Some(persist) = persist {
                        queue_persist(persist);
                    }
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                }
            }
        })
        .is_err()
    {
        this.artwork_pending.remove(&game_key);
        this.artwork_attempted.remove(&game_key);
        return;
    }
    cx.spawn(async move |this, cx| {
        let result = receive.await;
        this.update(cx, |this, cx| {
            this.artwork_pending.remove(&game_key);
            if cancel.load(Ordering::Relaxed) {
                this.artwork_attempted.remove(&game_key);
            } else if let Ok(Ok(ready)) = result {
                if lane == Lane::Disk {
                    this.artwork_cache.disk_hits += 1;
                } else {
                    this.artwork_cache.downloads += 1;
                }
                this.artwork_cache.insert(cache_key.clone(), ready.clone());
                if let Some(index) = this.games.iter().position(|game| {
                    game.source.key == game_key && game.source.cover == source.cover
                }) {
                    apply(this, index, ready, cx);
                    if this.hovered == Some(index) {
                        this.hover(index, true, cx);
                    }
                }
            } else if lane == Lane::Disk {
                // A damaged processed cache can be rebuilt without a retry loop.
                let _ = fs::remove_file(paths(&this.backend.cache, &cache_key).1);
                this.artwork_attempted.remove(&game_key);
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}

fn apply(this: &mut Launcher, index: usize, ready: Ready, cx: &mut Context<Launcher>) {
    let game = &mut this.games[index];
    game.poster = Some(ready.poster.clone());
    game.card_poster = Some(ready.card);
    if !game.background_checked || game.background.is_none() {
        game.background = Some(ready.background.clone());
    }
    if game.title == game.source.id
        && let Some(title) = ready.title
    {
        game.title = title;
    }
    if let Some(page) = &this.details {
        page.update(cx, |page, cx| {
            page.update_cover(&game.source.key, ready.poster, ready.background, cx)
        });
    }
}

fn key(game: &LibraryGame) -> String {
    let mut hash = DefaultHasher::new();
    (CARD_CACHE_VERSION, &game.key, &game.cover).hash(&mut hash);
    if let Some(tag) = raesir_backend::artwork::cache_tag(game) {
        tag.hash(&mut hash);
    }
    if let Some(path) = &game.cover
        && let Ok(metadata) = fs::metadata(path)
    {
        metadata.modified().ok().hash(&mut hash);
        metadata.len().hash(&mut hash);
    }
    format!("{:016x}", hash.finish())
}
fn paths(cache: &Path, key: &str) -> (PathBuf, PathBuf) {
    let root = cache.join("thumbnails");
    (
        root.join(format!("{key}-v7.jpg")),
        root.join(format!("{key}-v7-card.png")),
    )
}
fn prepared(cache: &Path, key: &str, community: bool) -> bool {
    let (poster, card) = paths(cache, key);
    poster.is_file()
        && card.is_file()
        && (!community
            || fs::metadata(&poster)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < std::time::Duration::from_secs(86_400)))
}
fn render(mut pixels: image_codec::RgbaImage) -> Arc<RenderImage> {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image_codec::Frame::new(pixels)]))
}
fn ready(
    image: image_codec::DynamicImage,
    card: image_codec::RgbaImage,
    title: Option<String>,
) -> Ready {
    let background = crate::backdrop::preview(&image);
    Ready {
        poster: render(image.to_rgba8()),
        card: render(card),
        background,
        title,
    }
}
fn load_cached(cache: &Path, game: &LibraryGame) -> Result<Ready, String> {
    let key = key(game);
    if !prepared(cache, &key, raesir_backend::artwork::fallback_applies(game)) {
        return Err("Thumbnail needs refreshing".into());
    }
    let (poster, card) = paths(cache, &key);
    let image = image_codec::open(&poster).map_err(|e| e.to_string())?;
    let card = image_codec::open(card)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let title = fs::read(poster.with_extension("json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Option<String>>(&b).ok())
        .flatten();
    Ok(ready(image, card, title))
}
fn atomic_image(
    path: &Path,
    image: &image_codec::DynamicImage,
    format: image_codec::ImageFormat,
) -> Result<(), String> {
    let parent = path.parent().ok_or("No thumbnail directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, format)
        .map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(bytes.get_ref()).map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

struct Persist {
    poster: PathBuf,
    card_path: PathBuf,
    image: image_codec::DynamicImage,
    card: image_codec::RgbaImage,
    title: Option<String>,
}
impl Persist {
    fn write(self) {
        let _ = atomic_image(
            &self.poster,
            &image_codec::DynamicImage::ImageRgb8(self.image.to_rgb8()),
            image_codec::ImageFormat::Jpeg,
        );
        let _ = atomic_image(
            &self.card_path,
            &image_codec::DynamicImage::ImageRgba8(self.card),
            image_codec::ImageFormat::Png,
        );
        let _ =
            raesir_backend::storage::write_json(&self.poster.with_extension("json"), &self.title);
    }
}
fn queue_persist(job: Persist) {
    static WRITER: LazyLock<mpsc::SyncSender<Persist>> = LazyLock::new(|| {
        let (send, receive) = mpsc::sync_channel::<Persist>(32);
        let receive = Arc::new(Mutex::new(receive));
        for _ in 0..2 {
            let receive = receive.clone();
            std::thread::Builder::new()
                .name("raesir-thumb-cache".into())
                .spawn(move || {
                    loop {
                        let next = receive.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = next else { break };
                        job.write();
                    }
                })
                .expect("thumbnail cache writer");
        }
        send
    });
    // A full writer queue must never hold up first paint or scrolling.
    let _ = WRITER.try_send(job);
}
fn prepare(
    backend: &Backend,
    game: &LibraryGame,
    cancel: &AtomicBool,
) -> Result<(Ready, Option<Persist>), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    if let Ok(ready) = load_cached(&backend.cache, game) {
        return Ok((ready, None));
    }
    let art = backend.artwork_cancelable(game, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    let image = image_codec::ImageReader::open(&art.path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?
        .thumbnail(256, 384);
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    let card = card_pixels(&image.to_rgba8());
    let (poster, card_path) = paths(&backend.cache, &key(game));
    let ready = ready(image.clone(), card.clone(), art.title.clone());
    Ok((
        ready,
        Some(Persist {
            poster,
            card_path,
            image,
            card,
            title: art.title,
        }),
    ))
}
#[cfg(any(test, feature = "benchmark"))]
pub(crate) fn load(backend: &Backend, game: &LibraryGame) -> Result<Ready, String> {
    let (ready, persist) = prepare(backend, game, &AtomicBool::new(false))?;
    if let Some(persist) = persist {
        persist.write();
    }
    Ok(ready)
}

fn card_pixels(source: &image_codec::RgbaImage) -> image_codec::RgbaImage {
    let mut image = source.clone();
    let start = image.height() * 3 / 4;
    let strip =
        image_codec::imageops::crop_imm(source, 0, start, image.width(), image.height() - start)
            .to_image();
    let blurred = image_codec::imageops::fast_blur(&strip, image.width() as f32 / 48.);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        if y >= start {
            let t = (y - start) as f32 / (source.height() - start - 1).max(1) as f32;
            let blur = t * t * (3. - 2. * t);
            let fade = ((t - 0.35) / 0.65).clamp(0., 1.);
            let alpha = 1. - fade * fade * (3. - 2. * fade);
            for channel in 0..3 {
                pixel[channel] = (pixel[channel] as f32 * (1. - blur)
                    + blurred.get_pixel(x, y - start)[channel] as f32 * blur)
                    .round() as u8;
            }
            pixel[3] = (pixel[3] as f32 * alpha).round() as u8;
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ready {
        let image = render(image_codec::RgbaImage::from_pixel(
            4,
            6,
            image_codec::Rgba([10, 30, 90, 255]),
        ));
        Ready {
            poster: image.clone(),
            card: image.clone(),
            background: image,
            title: None,
        }
    }

    #[test]
    fn warm_cache_reuses_render_ids_and_evicts_least_recently_used_within_budget() {
        let mut cache = Cache::default();
        let a = sample();
        let id = a.card.id;
        let bytes = a.bytes();
        cache.insert("a".into(), a);
        cache.insert("b".into(), sample());
        assert_eq!(cache.get("a").unwrap().card.id, id);
        cache.insert("c".into(), sample());
        cache.trim_to(bytes * 2);
        assert!(cache.get("b").is_none());
        assert_eq!(cache.get("a").unwrap().card.id, id);
        assert!(cache.bytes <= bytes * 2);
    }

    #[test]
    fn finished_card_disk_cache_preserves_faded_blur_and_loads_without_original_or_network() {
        let dir = tempfile::tempdir().unwrap();
        let mut game = raesir_backend::catalog::entry("steam", "42", "Fixture", vec![], "fixture");
        game.cover = Some("https://unreachable.invalid/portrait.jpg".into());
        let pixels =
            image_codec::RgbaImage::from_pixel(64, 96, image_codec::Rgba([15, 60, 130, 255]));
        let card = card_pixels(&pixels);
        assert_eq!(card.get_pixel(0, 0)[3], 255);
        assert_eq!(card.get_pixel(0, card.height() - 1)[3], 0);
        let (poster, card_path) = paths(dir.path(), &key(&game));
        atomic_image(
            &poster,
            &image_codec::DynamicImage::ImageRgb8(
                image_codec::DynamicImage::ImageRgba8(pixels).to_rgb8(),
            ),
            image_codec::ImageFormat::Jpeg,
        )
        .unwrap();
        atomic_image(
            &card_path,
            &image_codec::DynamicImage::ImageRgba8(card.clone()),
            image_codec::ImageFormat::Png,
        )
        .unwrap();
        let modified = fs::metadata(&card_path).unwrap().modified().unwrap();
        let cached = load_cached(dir.path(), &game).unwrap();
        assert_eq!(cached.card.as_bytes(0), render(card).as_bytes(0));
        assert_eq!(
            fs::metadata(card_path).unwrap().modified().unwrap(),
            modified
        );
        assert!(prepared(dir.path(), &key(&game), false));
    }

    #[test]
    fn community_art_invalidates_old_thumbnails_and_retries_fallbacks_after_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let game = raesir_backend::catalog::entry("steam", "319510", "FNAF", vec![], "fixture");
        let mut hash = DefaultHasher::new();
        (CARD_CACHE_VERSION, &game.key, &game.cover).hash(&mut hash);
        let old_key = format!("{:016x}", hash.finish());
        let new_key = key(&game);
        assert_ne!(old_key, new_key);
        let (poster, card) = paths(dir.path(), &new_key);
        fs::create_dir_all(poster.parent().unwrap()).unwrap();
        fs::write(&poster, b"prepared poster").unwrap();
        fs::write(&card, b"prepared card").unwrap();
        assert!(prepared(dir.path(), &new_key, true));
        assert!(!prepared(dir.path(), &old_key, false));
        let yesterday = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
        fs::File::options()
            .write(true)
            .open(poster)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(yesterday))
            .unwrap();
        assert!(!prepared(dir.path(), &new_key, true));
        assert!(prepared(dir.path(), &new_key, false));
    }
}
