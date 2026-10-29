use crate::{Launcher, backend::Backend};
use gpui::{Context, RenderImage};
use raesir_lib::model::LibraryGame;
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
            if let Ok((poster, title)) = result
                && let Some(game) = this
                    .games
                    .iter_mut()
                    .find(|g| g.source.key == key && g.source.cover == cover)
            {
                game.poster = Some(poster);
                if game.title == game.source.id
                    && let Some(title) = title
                {
                    game.title = title;
                }
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
) -> Result<(Arc<RenderImage>, Option<String>), String> {
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
    let mut pixels = image.to_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok((
        Arc::new(RenderImage::new(vec![image_codec::Frame::new(pixels)])),
        art.title,
    ))
}
