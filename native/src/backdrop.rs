use crate::backend::Backend;
use gpui::{prelude::*, *};
use raesir_lib::{details::Details, model::LibraryGame};
use std::{
    cell::Cell,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

const SIZE: (u32, u32) = (128, 72);

pub struct Backdrop {
    from: Arc<RenderImage>,
    target: Arc<RenderImage>,
    displayed: Rc<Cell<f32>>,
    revision: usize,
    retired: Vec<Arc<RenderImage>>,
    fade: Option<Task<()>>,
    cleanup_registered: bool,
}

impl Default for Backdrop {
    fn default() -> Self {
        let image = render_image(
            image_codec::load_from_memory(include_bytes!("../../static/default-background.png"))
                .expect("bundled default background")
                .into_rgba8(),
        );
        Self {
            from: image.clone(),
            target: image,
            displayed: Rc::new(Cell::new(1.)),
            revision: 0,
            retired: Vec::new(),
            fade: None,
            cleanup_registered: false,
        }
    }
}

impl Backdrop {
    pub fn retarget(&mut self, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        if image.id == self.target.id {
            return;
        }
        let progress = self.displayed.get();
        let from = if progress >= 1. {
            self.target.clone()
        } else {
            blend(&self.from, &self.target, progress)
        };
        self.retired
            .extend([self.from.clone(), self.target.clone()]);
        self.from = from;
        self.target = image;
        self.displayed.set(0.);
        self.revision += 1;
        self.fade = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(millis(500)).await;
            this.update(cx, |this, cx| {
                this.retired
                    .push(std::mem::replace(&mut this.from, this.target.clone()));
                this.displayed.set(1.);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for Backdrop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.cleanup_registered {
            cx.on_release_in(window, |this, window, _| {
                for image in this
                    .retired
                    .drain(..)
                    .chain([this.from.clone(), this.target.clone()])
                {
                    let _ = window.drop_image(image);
                }
            })
            .detach();
            self.cleanup_registered = true;
        }
        for image in self.retired.drain(..) {
            if image.id != self.from.id && image.id != self.target.id {
                let _ = window.drop_image(image);
            }
        }
        let background = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(rgb(0x0c1d1a));
        let texture = |image| {
            // Keep atlas filtering at the image boundary outside the visible window.
            div()
                .absolute()
                .inset(relative(-0.02))
                .child(img(image).size_full().object_fit(ObjectFit::Cover))
        };
        if self.from.id == self.target.id {
            return background
                .child(texture(self.target.clone()))
                .into_any_element();
        }
        let displayed = self.displayed.clone();
        background
            .child(texture(self.from.clone()))
            .child(texture(self.target.clone()).with_animation(
                ("backdrop", self.revision),
                Animation::new(millis(500)).with_easing(ease_in_out),
                move |image, progress| {
                    displayed.set(progress);
                    image.opacity(progress)
                },
            ))
            .into_any_element()
    }
}

pub fn media_source(details: &Details) -> Option<&str> {
    details.media.iter().find_map(|media| {
        let url = if media.video {
            &media.preview
        } else {
            &media.url
        };
        (!url.is_empty()).then_some(url.as_str())
    })
}

pub fn load(backend: &Backend, game: &LibraryGame) -> Result<Arc<RenderImage>, String> {
    let details = backend
        .cached_details(game)
        .map(Ok)
        .unwrap_or_else(|| backend.details(game))?;
    from_media(backend, media_source(&details).ok_or("No game media")?)
}

pub fn from_media(backend: &Backend, source: &str) -> Result<Arc<RenderImage>, String> {
    let path = backend.detail_media(source)?;
    let cache = cache_path(Path::new(&path), &backend.cache);
    if let Ok(image) = image_codec::open(&cache) {
        return Ok(render_image(image.into_rgba8()));
    }
    let image = image_codec::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    Ok(prepare(&image, &cache))
}

pub fn from_cover(
    image: &image_codec::DynamicImage,
    path: &Path,
    cache: &Path,
) -> Arc<RenderImage> {
    let path = cache_path(path, cache);
    if let Ok(image) = image_codec::open(&path) {
        return render_image(image.into_rgba8());
    }
    prepare(image, &path)
}

fn cache_path(source: &Path, cache: &Path) -> PathBuf {
    let mut hash = DefaultHasher::new();
    ("mesh-v2", source).hash(&mut hash);
    fs::metadata(source)
        .and_then(|m| m.modified())
        .ok()
        .hash(&mut hash);
    cache
        .join("backdrops")
        .join(format!("{:016x}.png", hash.finish()))
}

fn prepare(image: &image_codec::DynamicImage, path: &Path) -> Arc<RenderImage> {
    let image = color_mesh(image);
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_ok()
    {
        let temp = path.with_extension(format!(
            "{}.tmp",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        if image
            .save_with_format(&temp, image_codec::ImageFormat::Png)
            .is_ok()
        {
            let _ = fs::rename(&temp, path);
        }
        let _ = fs::remove_file(temp);
    }
    render_image(image)
}

fn color_mesh(image: &image_codec::DynamicImage) -> image_codec::RgbaImage {
    let image = image
        .resize_to_fill(SIZE.0, SIZE.1, image_codec::imageops::FilterType::Triangle)
        .into_rgb32f();
    let mut image = image_codec::imageops::fast_blur(&image, 18.);
    for pixel in image.pixels_mut() {
        let [red, green, blue] = pixel.0;
        let mut color = rgb_to_hsla(Rgba::new(red, green, blue, 1.));
        // Lifting nearly black pixels must not amplify their barely visible hue noise.
        color.saturation = (color.saturation * 1.2 * (color.lightness / 0.16).min(1.)).min(0.92);
        color.lightness = color.lightness.clamp(0.16, 0.52);
        let color = hsla_to_rgba(color);
        pixel.0 = [color.red, color.green, color.blue];
    }
    image_codec::DynamicImage::ImageRgb32F(image).into_rgba8()
}

fn render_image(mut image: image_codec::RgbaImage) -> Arc<RenderImage> {
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image_codec::Frame::new(image)]))
}

fn blend(a: &RenderImage, b: &RenderImage, progress: f32) -> Arc<RenderImage> {
    let pixels = a
        .as_bytes(0)
        .unwrap()
        .iter()
        .zip(b.as_bytes(0).unwrap())
        .map(|(&a, &b)| (a as f32 + (b as f32 - a as f32) * progress).round() as u8)
        .collect();
    Arc::new(RenderImage::new(vec![image_codec::Frame::new(
        image_codec::RgbaImage::from_raw(SIZE.0, SIZE.1, pixels).unwrap(),
    )]))
}

#[cfg(test)]
mod tests {
    use super::{Details, SIZE, color_mesh, media_source};
    #[test]
    fn default_background_is_bundled_and_matches_mesh_dimensions() {
        let backdrop = super::Backdrop::default();
        assert_eq!(
            backdrop.target.as_bytes(0).unwrap().len(),
            (SIZE.0 * SIZE.1 * 4) as usize
        );
        assert_eq!(backdrop.from.id, backdrop.target.id);
    }
    #[test]
    fn mesh_removes_fine_detail_and_has_bounded_size() {
        let image = image_codec::RgbImage::from_fn(1024, 576, |x, y| {
            image_codec::Rgb(if (x + y) % 2 == 0 {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            })
        });
        let mesh = color_mesh(&image.into());
        assert_eq!(mesh.dimensions(), SIZE);
        let values = mesh.pixels().map(|p| p[0]).collect::<Vec<_>>();
        assert!(values.iter().max().unwrap() - values.iter().min().unwrap() < 3);
        let tiny = color_mesh(&image_codec::DynamicImage::new_rgb8(1, 1));
        assert_eq!(tiny.dimensions(), SIZE);
    }
    #[test]
    fn shadow_lift_does_not_turn_near_black_noise_into_colored_ridges() {
        let image = image_codec::RgbImage::from_fn(512, 288, |x, _| {
            image_codec::Rgb(if x < 256 { [1, 0, 0] } else { [0, 0, 1] })
        });
        let mesh = color_mesh(&image.into());
        for pixel in mesh.pixels() {
            let channels = &pixel.0[..3];
            assert!(channels.iter().max().unwrap() - channels.iter().min().unwrap() <= 2);
        }
    }
    #[test]
    fn background_uses_first_available_content_instead_of_cover() {
        use raesir_lib::details::Media;
        let mut details = Details {
            media: vec![
                Media {
                    video: true,
                    preview: "trailer-thumb".into(),
                    ..Default::default()
                },
                Media {
                    url: "screenshot".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(media_source(&details), Some("trailer-thumb"));
        details.media[0].preview.clear();
        assert_eq!(media_source(&details), Some("screenshot"));
    }
}
