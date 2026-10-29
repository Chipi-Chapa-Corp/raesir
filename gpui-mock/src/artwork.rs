use gpui::{Context, RenderImage};
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::{Mock, TITLES};

pub fn start(cx: &mut Context<Mock>) {
    cx.spawn(async |this, cx| {
        for (batch, titles) in TITLES.chunks(4).enumerate() {
            let tasks: Vec<_> = titles
                .iter()
                .enumerate()
                .map(|(i, (_, source))| {
                    cx.background_executor()
                        .spawn(async move { (batch * 4 + i, load(source)) })
                })
                .collect();
            let mut loaded = Vec::new();
            for task in tasks {
                let (i, poster) = task.await;
                if let Some(poster) = poster {
                    loaded.push((i, poster));
                }
            }
            if this
                .update(cx, |this, cx| {
                    for (i, poster) in loaded {
                        for game in this.games.iter_mut().skip(i).step_by(TITLES.len()) {
                            game.poster = Some(poster.clone());
                        }
                    }
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn cache_path(source: &str) -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))?;
    let mut hash = DefaultHasher::new();
    source.hash(&mut hash);
    Some(
        root.join("raesir-gpui-mock/covers")
            .join(format!("{:016x}.jpg", hash.finish())),
    )
}

fn load(source: &str) -> Option<Arc<RenderImage>> {
    let path = cache_path(source);
    let cached = path
        .as_ref()
        .and_then(|p| fs::read(p).ok())
        .and_then(|bytes| image_codec::load_from_memory(&bytes).ok());
    let decoded = cached.or_else(|| {
        if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
            return None;
        }
        static HTTP: LazyLock<Option<reqwest::blocking::Client>> = LazyLock::new(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(8))
                .connect_timeout(Duration::from_secs(3))
                .user_agent("Raesir mock/0.1")
                .build()
                .ok()
        });
        let url = if source.starts_with("https://") {
            source.to_owned()
        } else {
            format!("https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{source}")
        };
        let mut bytes = Vec::new();
        HTTP.as_ref()?
            .get(url)
            .send()
            .ok()?
            .error_for_status()
            .ok()?
            .take(6 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .ok()?;
        let decoded = image_codec::load_from_memory(&bytes)
            .ok()?
            .thumbnail(384, 576);
        if let Some(path) = path.as_ref() {
            let mut jpeg = Vec::new();
            if image_codec::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .encode_image(&decoded)
                .is_ok()
                && fs::create_dir_all(path.parent()?).is_ok()
            {
                let pending = path.with_extension(format!("{}.tmp", std::process::id()));
                if fs::write(&pending, jpeg).is_ok() {
                    let _ = fs::rename(&pending, path);
                }
                let _ = fs::remove_file(pending);
            }
        }
        Some(decoded)
    })?;
    let mut pixels = decoded.to_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image_codec::Frame::new(
        pixels,
    )])))
}
