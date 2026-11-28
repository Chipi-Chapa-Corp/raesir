use super::*;
use raesir_backend::mods::{CatalogPage, Package};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Pages {
    nexus: Option<u32>,
    gamebanana: Option<u32>,
    modworkshop: Option<u32>,
    workshop: Option<u32>,
}

#[derive(Default)]
pub(super) struct Discovery {
    pub(super) nexus: Option<Option<Snapshot>>,
    gamebanana: Option<Option<u64>>,
    modworkshop: Option<Option<u64>>,
    pub(super) packages: Vec<Package>,
    pub(super) query: String,
    pub(super) warnings: Vec<String>,
    next: Pages,
}

impl Discovery {
    pub(super) fn nexus(&self) -> Option<&Snapshot> {
        self.nexus.as_ref().and_then(Option::as_ref)
    }

    pub(super) fn has_more(&self) -> bool {
        self.next != Pages::default()
    }
}

impl ModBrowser {
    pub(super) fn load_discovery(
        &mut self,
        continuation: Option<Pages>,
        refresh: bool,
        debounce: bool,
        cx: &mut Context<Self>,
    ) {
        let installed_only = self.filter == 1;
        if !raesir_backend::mods::can_discover_mods(&self.game) || installed_only && !refresh {
            return;
        }
        let reset = continuation.is_none();
        let pages = continuation.unwrap_or(Pages {
            nexus: Some(0),
            gamebanana: (!installed_only).then_some(1),
            modworkshop: (!installed_only).then_some(1),
            workshop: (!installed_only && self.game.store == "steam").then_some(1),
        });
        if reset {
            self.discovery.next = Pages::default();
            if self.query != self.discovery.query {
                self.filter_rows(cx);
            }
        }
        let backend = self.backend.clone();
        let game = self.game.clone();
        let nexus = self.discovery.nexus.clone();
        let nexus_revision = self.nexus_revision;
        let gamebanana = self.discovery.gamebanana;
        let modworkshop = self.discovery.modworkshop;
        let query = self.query.clone();
        let request_query = query.clone();
        let order = self.order;
        let executor = cx.background_executor().clone();
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                executor.timer(Duration::from_millis(320)).await;
            }
            let nexus_job = executor.spawn({
                let (backend, game, query) = (backend.clone(), game.clone(), query.clone());
                async move {
                    fetch(
                        nexus,
                        pages.nexus,
                        refresh,
                        || backend.nexus_catalog(&game, refresh),
                        |snapshot, offset| {
                            if installed_only {
                                Ok(CatalogPage::default())
                            } else {
                                backend.nexus_page(
                                    &snapshot.community,
                                    &query,
                                    offset,
                                    order,
                                    refresh,
                                )
                            }
                        },
                    )
                }
            });
            let gamebanana_job = executor.spawn({
                let (backend, game, query) = (backend.clone(), game.clone(), query.clone());
                async move {
                    fetch(
                        gamebanana,
                        pages.gamebanana,
                        refresh,
                        || backend.gamebanana_game(&game, refresh),
                        |id, page| backend.gamebanana_page(*id, &query, page, order, refresh),
                    )
                }
            });
            let modworkshop_job = executor.spawn({
                let (backend, game, query) = (backend.clone(), game.clone(), query.clone());
                async move {
                    fetch(
                        modworkshop,
                        pages.modworkshop,
                        refresh,
                        || backend.modworkshop_game(&game, refresh),
                        |id, page| backend.modworkshop_page(*id, &query, page, order, refresh),
                    )
                }
            });
            let workshop_job = executor.spawn(async move {
                pages.workshop.map_or_else(
                    || Ok(CatalogPage::default()),
                    |page| backend.workshop_page(&game, &query, page, order, refresh),
                )
            });
            let (nexus, gamebanana, modworkshop, workshop) =
                futures::join!(nexus_job, gamebanana_job, modworkshop_job, workshop_job);
            this.update(cx, |this, cx| {
                this.catalog_task = None;
                let old_scroll = this.scroll.0.borrow().base_handle.offset();
                let discovery = &mut this.discovery;
                if reset {
                    discovery.packages.clear();
                    discovery.query = request_query;
                    discovery.warnings.clear();
                }
                let warnings = &mut discovery.warnings;
                // Install and import operations can replace the installed Nexus profile
                // while this background request is running.
                let nexus = if this.nexus_revision == nexus_revision {
                    nexus.apply(&mut discovery.nexus, "Nexus Mods", warnings)
                } else {
                    nexus.apply(&mut None, "Nexus Mods", warnings)
                };
                let gamebanana =
                    gamebanana.apply(&mut discovery.gamebanana, "GameBanana", warnings);
                let modworkshop =
                    modworkshop.apply(&mut discovery.modworkshop, "ModWorkshop", warnings);
                let workshop = page_or_warn(workshop, "Steam Workshop", warnings);
                discovery.next = Pages {
                    nexus: next_page(pages.nexus, nexus.next, discovery.next.nexus),
                    gamebanana: next_page(
                        pages.gamebanana,
                        gamebanana.next,
                        discovery.next.gamebanana,
                    ),
                    modworkshop: next_page(
                        pages.modworkshop,
                        modworkshop.next,
                        discovery.next.modworkshop,
                    ),
                    workshop: next_page(pages.workshop, workshop.next, discovery.next.workshop),
                };
                warnings.sort();
                warnings.dedup();
                let mut incoming = [
                    nexus.packages.into_iter(),
                    gamebanana.packages.into_iter(),
                    modworkshop.packages.into_iter(),
                    workshop.packages.into_iter(),
                ];
                let mut listed: HashSet<_> =
                    discovery.packages.iter().map(|p| p.id.clone()).collect();
                while incoming.iter().any(|batch| batch.len() > 0) {
                    discovery.packages.extend(
                        incoming
                            .iter_mut()
                            .filter_map(Iterator::next)
                            .filter(|p| listed.insert(p.id.clone())),
                    );
                }
                if this.can_install_nexus() {
                    this.owner
                        .update(cx, |owner, cx| owner.check_nexus_account(cx))
                        .ok();
                }
                this.rebuild_rows(cx);
                if !reset {
                    let mut scroll = this.scroll.0.borrow_mut();
                    scroll.deferred_scroll_to_item = None;
                    scroll.base_handle.set_offset(old_scroll);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn load_visible_discovery(
        &self,
        last_visible: usize,
        rows: usize,
        cx: &mut Context<Self>,
    ) {
        if last_visible.saturating_add(1) < rows || !self.can_load_more() {
            return;
        }
        let next = self.visible_pages();
        let browser = cx.weak_entity();
        cx.defer(move |cx| {
            browser
                .update(cx, |this, cx| {
                    if this.visible_pages() == next && this.can_load_more() {
                        this.load_discovery(Some(next), false, false, cx);
                    }
                })
                .ok();
        });
    }

    fn visible_pages(&self) -> Pages {
        let next = self.discovery.next;
        let Some(source) = self.source_filter else {
            return next;
        };
        Pages {
            nexus: next.nexus.filter(|_| source == "Nexus Mods"),
            gamebanana: next.gamebanana.filter(|_| source == "GameBanana"),
            modworkshop: next.modworkshop.filter(|_| source == "ModWorkshop"),
            workshop: next.workshop.filter(|_| source == "Steam Workshop"),
        }
    }

    fn can_load_more(&self) -> bool {
        self.visible_pages() != Pages::default()
            && self.filter != 1
            && self.catalog_task.is_none()
            && self.installing.is_none()
            && self.dialog.is_none()
            && self.details.is_none()
            && !self.loading
            && !self.profiles_open
    }
}

fn next_page(requested: Option<u32>, next: Option<u32>, current: Option<u32>) -> Option<u32> {
    if requested.is_some() { next } else { current }
}

struct SearchResult<T> {
    resolved: Option<Option<T>>,
    match_error: Option<String>,
    page: Result<CatalogPage, String>,
}

impl<T> SearchResult<T> {
    fn apply(
        self,
        current: &mut Option<Option<T>>,
        source: &str,
        warnings: &mut Vec<String>,
    ) -> CatalogPage {
        if let Some(resolved) = self.resolved {
            *current = Some(resolved);
        }
        if let Some(error) = self.match_error {
            warnings.push(format!("{source}: {error}"));
        }
        page_or_warn(self.page, source, warnings)
    }
}

fn fetch<T>(
    cached: Option<Option<T>>,
    offset: Option<u32>,
    refresh: bool,
    resolve: impl FnOnce() -> Result<Option<T>, String>,
    load: impl FnOnce(&T, u32) -> Result<CatalogPage, String>,
) -> SearchResult<T> {
    let (resolved, match_error) = if offset.is_some() && (refresh || cached.is_none()) {
        match resolve() {
            Ok(found) => (Some(found), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let page = match (
        resolved
            .as_ref()
            .or(cached.as_ref())
            .and_then(Option::as_ref),
        offset,
    ) {
        (Some(catalog), Some(offset)) => load(catalog, offset),
        _ => Ok(CatalogPage::default()),
    };
    SearchResult {
        resolved,
        match_error,
        page,
    }
}

fn page_or_warn(
    result: Result<CatalogPage, String>,
    source: &str,
    warnings: &mut Vec<String>,
) -> CatalogPage {
    match result {
        Ok(mut page) => {
            warnings.append(&mut page.warnings);
            page
        }
        Err(error) => {
            warnings.push(format!("{source}: {error}"));
            CatalogPage::default()
        }
    }
}
