use crate::{Launcher, backend::Backend};
use gpui::{Context, RenderImage};
use raesir_backend::model::LibraryGame;
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

pub fn request(this: &mut Launcher, index: usize, cx: &mut Context<Launcher>) {
    let game = &this.games[index];
    if this.closing || game.poster.is_some() || this.artwork_pending.len() >= 4 {
        return;
    }
    let key = &game.source.key;
    if this.artwork_pending.contains(key) || this.artwork_attempted.contains(key) {
        return;
    }
    let key = key.clone();
    this.artwork_attempted.insert(key.clone());
    this.artwork_pending.insert(key.clone());
    let source = game.source.clone();
    let cover = source.cover.clone();
    let backend = this.backend.clone();
    cx.spawn(async move |this, cx| {
        let result = cx
            .background_executor()
            .spawn(async move { load(&backend, &source) })
            .await;
        this.update(cx, |this, cx| {
            this.artwork_pending.remove(&key);
            if let Ok((poster, title, background, card_poster)) = result
                && let Some(game) = this
                    .games
                    .iter_mut()
                    .find(|g| g.source.key == key && g.source.cover == cover)
            {
                game.poster = Some(poster.clone());
                game.card_poster = Some(card_poster);
                if let Some(page) = &this.details {
                    page.update(cx, |page, cx| {
                        page.update_cover(&key, poster, background.clone(), cx)
                    });
                }
                if !game.background_checked || game.background.is_none() {
                    game.background = Some(background);
                }
                if game.title == game.source.id
                    && let Some(title) = title
                {
                    game.title = title;
                }
            }
            if let Some(i) = this.hovered
                && this.games[i].source.key == key
            {
                this.hover(i, true, cx);
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}

pub(crate) fn load(
    backend: &Backend,
    game: &LibraryGame,
) -> Result<
    (
        Arc<RenderImage>,
        Option<String>,
        Arc<RenderImage>,
        Arc<RenderImage>,
    ),
    String,
> {
    let art = backend.artwork(game)?;
    let mut hash = DefaultHasher::new();
    art.path.hash(&mut hash);
    fs::metadata(&art.path)
        .and_then(|m| m.modified())
        .ok()
        .hash(&mut hash);
    let path = backend
        .cache
        .join("thumbnails")
        .join(format!("{:016x}.jpg", hash.finish()));
    let cached = fs::read(&path)
        .ok()
        .and_then(|b| image_codec::load_from_memory(&b).ok());
    let image = if let Some(image) = cached {
        image
    } else {
        let image = image_codec::ImageReader::open(&art.path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?
            .thumbnail(384, 576);
        let mut jpeg = Vec::new();
        if image_codec::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&image)
            .is_ok()
            && let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_ok()
        {
            let pending = path.with_extension(format!("{}.tmp", std::process::id()));
            if fs::write(&pending, jpeg).is_ok() {
                let _ = fs::rename(&pending, &path);
            }
            let _ = fs::remove_file(pending);
        }
        image
    };
    let background =
        crate::backdrop::from_cover(&image, std::path::Path::new(&art.path), &backend.cache);
    let card = card_poster(&image.to_rgba8());
    let mut pixels = image.to_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok((
        Arc::new(RenderImage::new(vec![image_codec::Frame::new(pixels)])),
        art.title,
        background,
        card,
    ))
}

fn card_poster(source: &image_codec::RgbaImage) -> Arc<RenderImage> {
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
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image_codec::Frame::new(image)]))
}
