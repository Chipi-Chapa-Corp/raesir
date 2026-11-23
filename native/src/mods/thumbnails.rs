use super::ModBrowser;
use crate::{artwork, backend::Backend};
use gpui::{Context, Window, millis};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Thumbnails {
    pub cache: artwork::Cache,
    pending: HashMap<String, Arc<AtomicBool>>,
    failed: HashSet<String>,
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        for cancel in self.pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl ModBrowser {
    pub(super) fn hover_art(&mut self, url: &str, entered: bool, cx: &mut Context<Self>) {
        if !entered {
            if self.hovered_art.as_deref() == Some(url) {
                self.hovered_art = None;
                self.hover_task = None;
            }
            return;
        }
        self.hover_task = None;
        self.hovered_art = (!url.is_empty()).then(|| url.to_owned());
        let Some(url) = self.hovered_art.clone() else {
            return;
        };
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(millis(180)).await;
            this.update(cx, |this, cx| {
                if this.hovered_art.as_deref() != Some(&url) || this.details.is_some() {
                    return;
                }
                let Some(ready) = this.thumbnails.cache.peek(&url) else {
                    return;
                };
                let Some(owner) = this.owner.upgrade() else {
                    return;
                };
                let backdrop = owner.read(cx).backdrop.clone();
                backdrop.update(cx, |backdrop, cx| backdrop.retarget(ready.background, cx));
            })
            .ok();
        }));
    }

    pub(super) fn maintain_thumbnails(
        &mut self,
        first: usize,
        after: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.thumbnails.cache.release_textures(window);
        let shape = if self.landscape_cards() {
            artwork::CardShape::Landscape
        } else {
            artwork::CardShape::Portrait
        };
        let wanted = self.rows[first.min(self.rows.len())..after.min(self.rows.len())]
            .iter()
            .map(|&i| &self.all_rows[i])
            .filter(|row| !row.icon.is_empty() || row.remote || row.id.starts_with("modworkshop:"))
            .map(|row| {
                (
                    row.artwork_key().to_owned(),
                    row.icon.clone(),
                    row.id.clone(),
                )
            })
            .collect::<Vec<_>>();
        for (key, cancel) in &self.thumbnails.pending {
            if !wanted.iter().any(|(wanted, _, _)| wanted == key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        for (url, icon, id) in wanted {
            if self.thumbnails.cache.get(&url).is_some()
                || self.thumbnails.pending.contains_key(&url)
                || self.thumbnails.failed.contains(&url)
            {
                continue;
            }
            if self.thumbnails.pending.len() >= 24
                || self
                    .thumbnails
                    .pending
                    .values()
                    .filter(|c| !c.load(Ordering::Relaxed))
                    .count()
                    >= 12
            {
                break;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            self.thumbnails.pending.insert(url.clone(), cancel.clone());
            let backend = self.backend.clone();
            let game = self.game.clone();
            let domain = id
                .starts_with("nexus:")
                .then(|| {
                    self.discovery
                        .nexus()
                        .map(|catalog| catalog.community.clone())
                })
                .flatten();
            let worker_cancel = cancel.clone();
            let (send, receive) = futures::channel::oneshot::channel();
            if std::thread::Builder::new()
                .name("raesir-mod-icon".into())
                .spawn(move || {
                    let mut result = if icon.is_empty() {
                        Err("No mod cover".into())
                    } else {
                        load(&backend, &icon, shape, &worker_cancel)
                    };
                    if result.is_err()
                        && !worker_cancel.load(Ordering::Relaxed)
                        && let Ok(details) = backend.mod_details(&game, &id, domain.as_deref())
                        && let Some(media) = details.first_screenshot()
                        && media.url != icon
                    {
                        result = load(&backend, &media.url, shape, &worker_cancel);
                    }
                    let _ = send.send(result);
                })
                .is_err()
            {
                self.thumbnails.pending.remove(&url);
                self.thumbnails.failed.insert(url);
                continue;
            }
            cx.spawn(async move |this, cx| {
                let result = receive.await;
                this.update(cx, |this, cx| {
                    this.thumbnails.pending.remove(&url);
                    if !cancel.load(Ordering::Relaxed) {
                        match result {
                            Ok(Ok(ready)) => {
                                if this.hovered_art.as_deref() == Some(&url) {
                                    this.hover_art(&url, true, cx);
                                }
                                this.thumbnails.cache.insert(url, ready);
                                this.thumbnails.cache.trim_to(64 * 1024 * 1024);
                            }
                            _ => {
                                this.thumbnails.failed.insert(url);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }
}

fn load(
    backend: &Backend,
    url: &str,
    shape: artwork::CardShape,
    cancel: &AtomicBool,
) -> Result<artwork::Ready, String> {
    let mut game = raesir_backend::catalog::entry("media", url, "", vec![], "thunderstore");
    game.cover = Some(url.into());
    artwork::load_cancelable(backend, &game, shape, cancel)
}
