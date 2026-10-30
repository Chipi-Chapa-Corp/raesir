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
            if let Ok((poster, title, colors)) = result
                && let Some(game) = this
                    .games
                    .iter_mut()
                    .find(|g| g.source.key == key && g.source.cover == cover)
            {
                game.poster = Some(poster);
                game.colors = Some(colors);
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
) -> Result<(Arc<RenderImage>, Option<String>, [u32; 2]), String> {
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
    let colors = edge_colors(&image);
    let mut pixels = image.to_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok((
        Arc::new(RenderImage::new(vec![image_codec::Frame::new(pixels)])),
        art.title,
        colors,
    ))
}

fn edge_colors(image: &image_codec::DynamicImage) -> [u32; 2] {
    let image = image.thumbnail(48, 72).to_rgb8();
    let strip = (image.width() / 5).max(1);
    [0, image.width() - strip].map(|start| {
        let mut sum = [0.; 3];
        for y in 0..image.height() {
            for x in start..start + strip {
                for (sum, value) in sum.iter_mut().zip(image.get_pixel(x, y).0) {
                    *sum += value as f32;
                }
            }
        }
        let mean = sum.map(|v| v / (strip * image.height()) as f32);
        let [r, g, b] = mean.map(|v| v.round() as u32);
        let mut color = gpui::rgb_to_hsla(gpui::rgb((r << 16) | (g << 8) | b));
        color.saturation = (color.saturation * 1.5).min(0.85);
        color.lightness = (color.lightness * 1.15).clamp(0.16, 0.38);
        let color = gpui::hsla_to_rgba(color);
        let [r, g, b] = [color.red, color.green, color.blue].map(|v| (v * 255.).round() as u32);
        (r << 16) | (g << 8) | b
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_uses_opposite_edges_without_the_center_logo() {
        let image = image_codec::RgbImage::from_fn(100, 150, |x, _| {
            image_codec::Rgb(if x < 20 {
                [180, 30, 20]
            } else if x >= 80 {
                [20, 30, 180]
            } else {
                [255, 255, 255]
            })
        });
        let [left, right] = edge_colors(&image.into());
        assert!((left >> 16) > (left & 255) + 100);
        assert!((right & 255) > (right >> 16) + 100);
        let gray = image_codec::RgbImage::from_pixel(1, 1, image_codec::Rgb([40, 40, 40]));
        let [left, right] = edge_colors(&gray.into());
        assert_eq!(left, right);
        assert_eq!(left >> 16, left & 255);
        assert_eq!((left >> 8) & 255, left & 255);
        let white = image_codec::RgbImage::from_pixel(2, 2, image_codec::Rgb([255, 255, 255]));
        let [left, _] = edge_colors(&white.into());
        assert!(gpui::rgb_to_hsla(gpui::rgb(left)).lightness <= 0.39);
    }
}
