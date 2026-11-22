use super::ModBrowser;
use crate::{artwork, backend::Backend};
use gpui::{Context, Window};
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
    pub(super) fn maintain_thumbnails(
        &mut self,
        first: usize,
        after: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.thumbnails.cache.release_textures(window);
        let wanted = self.rows[first.min(self.rows.len())..after.min(self.rows.len())]
            .iter()
            .map(|&i| self.all_rows[i].icon.clone())
            .filter(|url| !url.is_empty())
            .collect::<Vec<_>>();
        for (url, cancel) in &self.thumbnails.pending {
            if !wanted.contains(url) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        for url in wanted {
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
            let request = url.clone();
            let worker_cancel = cancel.clone();
            let (send, receive) = futures::channel::oneshot::channel();
            if std::thread::Builder::new()
                .name("raesir-mod-icon".into())
                .spawn(move || {
                    let _ = send.send(load(&backend, &request, &worker_cancel));
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

fn load(backend: &Backend, url: &str, cancel: &AtomicBool) -> Result<artwork::Ready, String> {
    let mut game = raesir_backend::catalog::entry("media", url, "", vec![], "thunderstore");
    game.cover = Some(url.into());
    artwork::load_cancelable(backend, &game, cancel)
}
