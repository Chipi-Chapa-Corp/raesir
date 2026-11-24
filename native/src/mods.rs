use crate::{
    Launcher,
    backend::Backend,
    header::{self, SearchSize, bar, button, filter_button, item_action, pill, spinner},
    icons::icon,
    views::{ArtworkCard, CardGrid},
};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};
use raesir_backend::{
    model::LibraryGame,
    mods::{CatalogOrder, Profile, Snapshot},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

mod discovery;
mod nexus_ui;
mod profile_ui;
mod thumbnails;

#[derive(Clone)]
struct Row {
    id: String,
    title: String,
    owner: String,
    description: String,
    version: String,
    page: Option<String>,
    installed: bool,
    search: String,
    remote: bool,
    icon: String,
    source: String,
}

impl Row {
    fn artwork_key(&self) -> &str {
        if self.icon.is_empty() {
            &self.id
        } else {
            &self.icon
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ModAction {
    Install,
    Uninstall,
    SignIn,
    Import,
}

impl ModAction {
    pub fn button(self, id: impl Into<ElementId>, busy: bool, compact: bool) -> Stateful<Div> {
        let (label, glyph) = match self {
            Self::Install => ("Install", "install"),
            Self::Uninstall => ("Uninstall", "trash"),
            Self::SignIn => ("Sign in to install", "external"),
            Self::Import => ("Import downloaded archive", "more"),
        };
        let primary = !matches!(self, Self::Uninstall);
        let color = if primary { 0x202020 } else { 0xf0f0f0 };
        header::button_tooltip(
            item_action(id, primary),
            if busy { "Working…" } else { label },
        )
        .aria_disabled(busy)
        .child(if busy {
            header::spinner_colored("mod-action-busy", 1.2, color)
        } else {
            icon(glyph).text_color(rgb(color)).into_any_element()
        })
        .when(!busy && !compact, |d| {
            d.w_auto().px(rems(0.7)).gap(rems(0.35)).child(label)
        })
    }
}

pub struct ModBrowser {
    owner: WeakEntity<Launcher>,
    backend: Arc<Backend>,
    game: LibraryGame,
    discovery: discovery::Discovery,
    nexus_revision: u64,
    order: CatalogOrder,
    source_filter: Option<&'static str>,
    hovered_art: Option<String>,
    hover_task: Option<Task<()>>,
    catalog_task: Option<Task<()>>,
    snapshot: Option<Snapshot>,
    search: Entity<EditableTextState>,
    _search_changed: Subscription,
    _search_focus_subscriptions: [Subscription; 2],
    search_size: SearchSize,
    query: String,
    filter: u8,
    all_rows: Vec<Row>,
    rows: Vec<usize>,
    scroll: UniformListScrollHandle,
    zoom: f32,
    loading: bool,
    installing: Option<String>,
    error: Option<String>,
    profiles_open: bool,
    profile_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    thumbnails: thumbnails::Thumbnails,
    cleanup_registered: bool,
    details: Option<Entity<crate::details::DetailsPage>>,
    dialog: Option<profile_ui::Dialog>,
    profile_name: Entity<EditableTextState>,
    nexus_url: Entity<EditableTextState>,
    nexus_folder: Entity<EditableTextState>,
    nexus_archive: Option<std::path::PathBuf>,
}

impl Launcher {
    pub(crate) fn open_mods(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.games.iter().find(|g| g.source.key == key) else {
            return;
        };
        let game = game.source.clone();
        if !raesir_backend::mods::can_browse_mods(&game) {
            return;
        }
        if raesir_backend::mods::has_managed_nexus(&game) {
            self.check_nexus_account(cx);
        }
        let backend = self.backend.clone();
        let owner = cx.weak_entity();
        let zoom = self.zoom;
        let page = cx.new(|cx| {
            let search = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
            let subscription = cx.subscribe(
                &search,
                |this: &mut ModBrowser, search, _: &TextChanged, cx| {
                    let query = search.read(cx).as_str().trim().to_lowercase();
                    if this.query != query {
                        this.query = query;
                        if this.filter == 1 || !raesir_backend::mods::can_discover_mods(&this.game)
                        {
                            this.filter_rows(cx);
                        } else {
                            this.load_discovery(None, false, true, cx);
                        }
                    }
                },
            );
            let search_focus = search.focus_handle(cx);
            let focus_subscriptions = [
                cx.on_focus(&search_focus, window, |_, _, cx| cx.notify()),
                cx.on_blur(&search_focus, window, |_, _, cx| cx.notify()),
            ];
            ModBrowser {
                owner,
                backend,
                game,
                discovery: Default::default(),
                nexus_revision: 0,
                order: CatalogOrder::All,
                source_filter: None,
                hovered_art: None,
                hover_task: None,
                catalog_task: None,
                snapshot: None,
                search,
                _search_changed: subscription,
                _search_focus_subscriptions: focus_subscriptions,
                search_size: SearchSize::Full,
                query: String::new(),
                filter: 0,
                all_rows: Vec::new(),
                rows: Vec::new(),
                scroll: UniformListScrollHandle::new(),
                zoom,
                loading: false,
                installing: None,
                error: None,
                profiles_open: false,
                profile_bounds: Default::default(),
                thumbnails: Default::default(),
                cleanup_registered: false,
                details: None,
                dialog: None,
                profile_name: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                nexus_url: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                nexus_folder: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                nexus_archive: None,
            }
        });
        page.update(cx, |page, cx| {
            page.load(false, cx);
            page.focus_search(window, cx);
        });
        self.mods = Some(page);
        self.menu = false;
        self.close_downloads(cx);
        self.game_menu = None;
        cx.notify();
    }
}

impl ModBrowser {
    pub(crate) fn game(&self) -> &LibraryGame {
        &self.game
    }

    pub(crate) fn dismiss_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let had_dialog = self.dialog.take().is_some();
        if had_dialog {
            self.error = None;
        }
        let dismissed = had_dialog
            || std::mem::replace(&mut self.profiles_open, false)
            || self.details.take().is_some();
        if dismissed {
            self.restore_focus(window, cx);
            cx.notify();
        }
        dismissed
    }
    pub(crate) fn close_mod_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.details = None;
        self.focus_search(window, cx);
        cx.notify();
    }
    pub(crate) fn mod_status(&self, id: &str) -> (bool, bool, Option<String>) {
        (
            self.all_rows
                .iter()
                .find(|r| r.id == id)
                .is_some_and(|r| r.installed),
            self.installing.is_some(),
            self.error.clone(),
        )
    }
    pub(crate) fn mod_poster(&self, id: &str) -> Option<Arc<RenderImage>> {
        self.all_rows
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| self.thumbnails.cache.peek(r.artwork_key()))
            .map(|r| r.poster)
    }

    pub(crate) fn mod_version(&self, id: &str) -> Option<String> {
        self.all_rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.version.clone())
    }

    pub(crate) fn mod_action(&self, id: &str, installed: bool, cx: &App) -> Option<ModAction> {
        if installed {
            Some(ModAction::Uninstall)
        } else if !self.can_manage_mod(id) {
            None
        } else if !id.starts_with("nexus:") || self.nexus_signed_in(cx) {
            Some(ModAction::Install)
        } else if raesir_backend::mods::nexus_account::configured() {
            Some(ModAction::SignIn)
        } else {
            Some(ModAction::Import)
        }
    }

    fn can_manage_mod(&self, id: &str) -> bool {
        match id.split_once(':') {
            Some(("gamebanana", _)) => raesir_backend::mods::can_install_gamebanana(&self.game),
            Some(("workshop", _)) => false,
            Some(("modworkshop", _)) => raesir_backend::mods::has_managed_nexus(&self.game),
            Some(("nexus", _)) => self.can_install_nexus(),
            _ => true,
        }
    }

    fn nexus_signed_in(&self, cx: &App) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|owner| owner.read(cx).nexus_account.signed_in())
    }
    pub(crate) fn request_mod_change(
        &mut self,
        id: String,
        installed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.installing.is_some() || self.loading {
            return;
        }
        if installed {
            let plan = self
                .installed_profile(&id)
                .ok_or_else(|| "Select a mod profile first".to_owned())
                .and_then(|p| p.removal_plan(&id));
            let removed = match plan {
                Ok(removed) => removed,
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            let dependencies = removed
                .into_iter()
                .skip(1)
                .map(|m| m.name.replace('_', " "))
                .collect();
            let name = self
                .all_rows
                .iter()
                .find(|r| r.id == id)
                .map_or_else(|| id.clone(), |r| r.title.clone());
            self.dialog = Some(profile_ui::Dialog::Uninstall {
                id,
                name,
                dependencies,
            });
            self.error = None;
            cx.notify();
        } else if id.starts_with("nexus:") {
            if !self.can_install_nexus() {
                return;
            }
            if self.nexus_signed_in(cx) {
                self.perform_nexus_install(id, None, cx);
            } else if raesir_backend::mods::nexus_account::configured() {
                self.owner
                    .update(cx, |owner, cx| owner.sign_in_nexus(cx))
                    .ok();
            } else {
                let page = self
                    .all_rows
                    .iter()
                    .find(|row| row.id == id)
                    .and_then(|row| row.page.clone());
                self.open_nexus_import(page, window, cx);
            }
        } else if id.starts_with("modworkshop:") || id.starts_with("gamebanana:") {
            self.choose_provider_file(id, cx);
        } else {
            self.change_mod(id, false, cx);
        }
    }

    fn open_mod(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.all_rows.iter().find(|row| row.id == id).cloned() else {
            return;
        };
        self.hovered_art = None;
        self.hover_task = None;
        let domain = row
            .id
            .starts_with("nexus:")
            .then(|| self.discovery.nexus().map(|s| s.community.clone()))
            .flatten();
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let backdrop = owner.read(cx).backdrop.clone();
        let return_focus = owner.read(cx).focus.clone();
        window.focus(&return_focus, cx);
        let mut source =
            raesir_backend::catalog::entry("thunderstore", &row.id, &row.title, vec![], "mods");
        source.client = row.source;
        let data = raesir_backend::details::Details {
            key: source.key.clone(),
            title: row.title,
            description: row.description,
            developers: (!row.owner.is_empty())
                .then_some(row.owner)
                .into_iter()
                .collect(),
            store_url: row.page,
            ..Default::default()
        };
        let poster = self.mod_poster(&row.id);
        let backend = self.backend.clone();
        let owner = self.owner.clone();
        let context = crate::details::ModContext {
            browser: cx.weak_entity(),
            id: row.id.clone(),
            version: row.version,
        };
        let page = cx.new(|cx| {
            crate::details::DetailsPage::for_mod(
                source,
                data,
                poster,
                backdrop,
                backend,
                owner,
                return_focus,
                context,
                cx,
            )
        });
        let details_available = match row.id.split_once(':') {
            Some(("deadlock-local", _)) => false,
            Some(("nexus", _)) => domain.is_some(),
            _ => true,
        };
        if details_available {
            page.update(cx, |page, cx| {
                page.load_mod(self.game.clone(), row.id, domain, cx)
            });
        }
        self.details = Some(page);
        self.profiles_open = false;
        cx.notify();
    }

    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            return;
        }
        if self.details.take().is_some() {
            cx.notify();
        }
        window.focus(&self.search.focus_handle(cx), cx);
    }

    fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.load_discovery(None, refresh, false, cx);
        if !self.manages_profiles() {
            return;
        }
        self.loading = true;
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.mod_catalog(&game, refresh) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => this.snapshot = Some(snapshot),
                    Err(e) => this.error = Some(e),
                }
                this.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn manages_profiles(&self) -> bool {
        self.game.mods.is_some()
    }

    fn can_install_nexus(&self) -> bool {
        raesir_backend::mods::has_managed_nexus(&self.game)
            || self
                .discovery
                .nexus()
                .is_some_and(|s| !s.selected.is_empty())
    }

    fn nexus_url(&self) -> Option<&String> {
        self.discovery
            .nexus()
            .and_then(|s| s.nexus_url.as_ref())
            .or_else(|| self.snapshot.as_ref().and_then(|s| s.nexus_url.as_ref()))
    }

    fn installed_profile(&self, id: &str) -> Option<&Profile> {
        let snapshot =
            if id.starts_with("nexus:") && !raesir_backend::mods::has_managed_nexus(&self.game) {
                self.discovery.nexus()
            } else {
                self.snapshot.as_ref()
            }?;
        snapshot.profiles.iter().find(|p| p.id == snapshot.selected)
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.snapshot = Some(snapshot);
        self.rebuild_rows(cx);
    }

    fn apply_nexus_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if raesir_backend::mods::has_managed_nexus(&self.game) {
            self.apply_snapshot(snapshot, cx);
        } else {
            self.nexus_revision += 1;
            self.discovery.nexus = Some(Some(snapshot));
            self.rebuild_rows(cx);
        }
    }

    pub(crate) fn refresh_after_updates(&mut self, cx: &mut Context<Self>) {
        if !self.manages_profiles()
            || self.loading
            || self.installing.is_some()
            || self.dialog.is_some()
        {
            return;
        }
        let backend = self.backend.clone();
        let game = self.game.clone();
        let selected = self.snapshot.as_ref().map(|s| s.selected.clone());
        cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { backend.mod_catalog(&game, false) })
                .await;
            this.update(cx, |this, cx| {
                if this.loading
                    || this.installing.is_some()
                    || this.dialog.is_some()
                    || this.snapshot.as_ref().map(|s| s.selected.clone()) != selected
                {
                    return;
                }
                if let Ok(snapshot) = snapshot {
                    let top = this.scroll.0.borrow().base_handle.offset();
                    this.snapshot = Some(snapshot);
                    this.rebuild_rows(cx);
                    let mut scroll = this.scroll.0.borrow_mut();
                    scroll.deferred_scroll_to_item = None;
                    scroll.base_handle.set_offset(top);
                }
            })
            .ok();
        })
        .detach();
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let direct_nexus = !raesir_backend::mods::has_managed_nexus(&self.game);
        let installed = [
            self.installed_profile(""),
            direct_nexus
                .then(|| self.installed_profile("nexus:"))
                .flatten(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|profile| &profile.installed)
        .collect::<Vec<_>>();
        let versions = installed
            .iter()
            .map(|m| (m.id.as_str(), m.version.as_str()))
            .collect::<HashMap<_, _>>();
        let mut local = self
            .snapshot
            .iter()
            .flat_map(|s| s.packages.iter())
            .collect::<Vec<_>>();
        if self.order == CatalogOrder::Downloads {
            local.sort_by_key(|package| std::cmp::Reverse(package.downloads));
        }
        let mut local = local.into_iter();
        let remote = &self.discovery.packages;
        let catalog = remote
            .iter()
            .flat_map(|package| {
                [(package, true)]
                    .into_iter()
                    .chain(local.next().map(|package| (package, false)))
            })
            .collect::<Vec<_>>();
        let remote_ids = remote
            .iter()
            .map(|package| package.id.as_str())
            .collect::<HashSet<_>>();
        let mut listed = HashSet::new();
        let mut rows = catalog
            .into_iter()
            .chain(local.map(|package| (package, false)))
            .filter(|(package, remote)| {
                (*remote || !remote_ids.contains(package.id.as_str()))
                    && listed.insert(package.id.as_str())
            })
            .map(|(p, remote)| Row {
                search: String::new(),
                remote,
                icon: p.icon.clone(),
                id: p.id.clone(),
                title: p.name.replace('_', " "),
                owner: p.owner.clone(),
                description: p.description.clone(),
                version: versions
                    .get(p.id.as_str())
                    .copied()
                    .unwrap_or(&p.version)
                    .to_owned(),
                page: Some(p.page.clone()).filter(|page| !page.is_empty()),
                installed: versions.contains_key(p.id.as_str()),
                source: p.source.clone(),
            })
            .collect::<Vec<_>>();
        let community = self.snapshot.as_ref().map_or("", |s| s.community.as_str());
        rows.extend(
            installed
                .into_iter()
                .filter(|m| !listed.contains(m.id.as_str()))
                .map(|m| {
                    let (source, page) = if let Some(id) = m.id.strip_prefix("nexus:") {
                        (
                            "Nexus Mods",
                            self.nexus_url().map(|url| format!("{url}{id}")),
                        )
                    } else if let Some(id) = m.id.strip_prefix("modworkshop:") {
                        (
                            "ModWorkshop",
                            Some(format!("https://modworkshop.net/mod/{id}")),
                        )
                    } else if let Some((kind, id)) =
                        m.id.strip_prefix("gamebanana:")
                            .and_then(|id| id.split_once(':'))
                    {
                        (
                            "GameBanana",
                            Some(format!("https://gamebanana.com/{kind}/{id}")),
                        )
                    } else if m.id.starts_with("recipe:") {
                        ("Raesir catalog", None)
                    } else if m.local {
                        ("Local", None)
                    } else {
                        (
                            "Thunderstore",
                            Some(format!(
                                "https://thunderstore.io/c/{community}/p/{}/",
                                m.id.replace('-', "/")
                            )),
                        )
                    };
                    Row {
                        search: String::new(),
                        remote: false,
                        icon: String::new(),
                        id: m.id.clone(),
                        title: m.name.replace('_', " "),
                        owner: m.owner.clone(),
                        description: if m.local {
                            "Local mod"
                        } else {
                            "Installed mod"
                        }
                        .into(),
                        version: m.version.clone(),
                        page,
                        installed: true,
                        source: source.into(),
                    }
                }),
        );
        for row in &mut rows {
            row.search = format!("{} {} {} {}", row.id, row.title, row.description, row.owner)
                .to_lowercase();
        }
        self.all_rows = rows;
        self.filter_rows(cx);
    }

    fn filter_rows(&mut self, cx: &mut Context<Self>) {
        self.rows = self
            .all_rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                self.source_filter.is_none_or(|source| r.source == source)
                    && (self.filter == 0
                        || self.filter == 1 && r.installed
                        || self.filter == 2 && !r.installed && self.can_manage_mod(&r.id))
                    && (r.remote && self.filter != 1 && self.discovery.query == self.query
                        || self.query.is_empty()
                        || r.search.contains(&self.query))
            })
            .map(|(i, _)| i)
            .collect();
        self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn choose_profile(&mut self, profile: String, cx: &mut Context<Self>) {
        if self.loading || self.installing.is_some() {
            return;
        }
        self.profiles_open = false;
        self.loading = true;
        let backend = self.backend.clone();
        let game = self.game.clone();
        let owner = self.owner.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let report = backend.choose_mod_profile(&game, &profile)?;
                    let snapshot = backend.mod_catalog(&game, false)?;
                    Ok::<_, String>((snapshot, report))
                })
                .await;
            if let Ok((_, report)) = &result {
                owner
                    .update(cx, |owner, cx| owner.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((snapshot, _)) => {
                        this.apply_snapshot(snapshot, cx);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn change_mod(&mut self, id: String, uninstall: bool, cx: &mut Context<Self>) {
        if self.installing.is_some() || self.loading {
            return;
        }
        let package =
            if !uninstall && (id.starts_with("gamebanana:") || id.starts_with("modworkshop:")) {
                id.rsplit_once(':').map_or(id.as_str(), |(id, _)| id)
            } else {
                &id
            };
        self.installing = Some(package.to_owned());
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        let owner = self.owner.clone();
        let nexus = id.starts_with("nexus:");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if uninstall {
                        backend.uninstall_mod(&game, &id)
                    } else {
                        backend.install_mod(&game, &id)
                    }
                })
                .await;
            if let Ok((_, report)) = &result {
                owner
                    .update(cx, |owner, cx| owner.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.installing = None;
                match result {
                    Ok((snapshot, _)) if nexus => this.apply_nexus_snapshot(snapshot, cx),
                    Ok((snapshot, _)) => this.apply_snapshot(snapshot, cx),
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_deadlock_archive(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.installing.is_some() {
            return;
        }
        self.loading = true;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a Deadlock VPK or mod archive".into()),
        });
        let backend = self.backend.clone();
        let game = self.game.clone();
        let owner = self.owner.clone();
        cx.spawn(async move |this, cx| {
            let result = match selection.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => cx
                        .background_executor()
                        .spawn(async move { backend.import_deadlock_mod(&game, &path) })
                        .await
                        .map(Some),
                    None => Ok(None),
                },
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            if let Ok(Some((_, report))) = &result {
                owner
                    .update(cx, |owner, cx| owner.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(Some((snapshot, _))) => {
                        this.error = None;
                        this.apply_snapshot(snapshot, cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn landscape_cards(&self) -> bool {
        !self.manages_profiles() || self.snapshot.as_ref().is_some_and(|s| s.uses_smapi())
    }

    fn card_ratio(&self) -> f32 {
        if self.landscape_cards() {
            let (width, height) = crate::artwork::LANDSCAPE_CARD_SIZE;
            height as f32 / width as f32
        } else {
            crate::CARD_POSTER_RATIO
        }
    }

    fn card(&mut self, i: usize, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(i).and_then(|&i| self.all_rows.get(i)) else {
            return div().into_any_element();
        };
        let cover = self
            .thumbnails
            .cache
            .get(row.artwork_key())
            .map(|ready| ready.card);
        let installed = row.installed;
        let busy = self.installing.as_deref() == Some(&row.id);
        let id = row.id.clone();
        let action_id = id.clone();
        let artwork = row.artwork_key().to_owned();
        let actions = self.mod_action(&id, installed, cx).map(|action| {
            div().child(
                action
                    .button(format!("mod-action-{action_id}"), busy, true)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.request_mod_change(action_id.clone(), installed, window, cx);
                    })),
            )
        });
        ArtworkCard {
            width,
            ratio: self.card_ratio(),
            title: row.title.clone(),
            cover,
            dimmed: !installed && self.mod_action(&row.id, installed, cx).is_some(),
            installed,
        }
        .render(format!("mod-poster-{id}"), None, actions, self.zoom)
        .id(format!("mod-card-{id}"))
        .on_hover(cx.listener(move |this, entered, _, cx| this.hover_art(&artwork, *entered, cx)))
        .on_click(cx.listener(move |this, _, window, cx| this.open_mod(&id, window, cx)))
        .into_any_element()
    }

    fn compact_profile_header(&self, viewport: f32) -> bool {
        viewport / self.zoom < 80.
    }

    fn profile_controls(&self, viewport: f32, cx: &mut Context<Self>) -> Div {
        let profile_bounds = self.profile_bounds.clone();
        let profile_label = self
            .snapshot
            .as_ref()
            .and_then(|s| s.profiles.iter().find(|p| p.id == s.selected))
            .map_or_else(|| "Default".to_owned(), |p| p.name.clone());
        div()
            .flex()
            .flex_none()
            .gap(rems(0.6))
            .child(
                pill()
                    .id("mods-profile")
                    .h(rems(2.9))
                    .px(rems(0.8))
                    .on_prepaint(move |paint, _, _| profile_bounds.set(paint.bounds))
                    .child(
                        div()
                            .max_w(rems(if viewport < 1100. { 6. } else { 10. }))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(profile_label.clone()),
                    )
                    .child(icon("expand"))
                    .cursor_pointer()
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::header::Tooltip(profile_label.clone()))
                            .into()
                    })
                    .when(self.installing.is_none() && !self.loading, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| {
                            this.profiles_open = !this.profiles_open;
                            cx.notify();
                        }))
                    }),
            )
            .child(
                pill().h(rems(2.9)).child(
                    button("new-mod-profile", "more", false)
                        .tooltip(|_, cx| {
                            cx.new(|_| crate::header::Tooltip("New profile".into()))
                                .into()
                        })
                        .when(self.installing.is_none() && !self.loading, |d| {
                            d.on_click(cx.listener(|this, _, window, cx| {
                                this.open_profile_dialog(profile_ui::Dialog::Create, window, cx)
                            }))
                        }),
                ),
            )
    }

    fn filters(&self, cx: &mut Context<Self>) -> Div {
        let installation = self.manages_profiles() || self.can_install_nexus();
        let online = raesir_backend::mods::can_discover_mods(&self.game);
        pill().children(
            [
                ("all", "All mods", 0, CatalogOrder::All, true),
                ("trending", "Trending", 0, CatalogOrder::Trending, online),
                (
                    "install",
                    "Most downloaded",
                    0,
                    CatalogOrder::Downloads,
                    true,
                ),
                ("installed", "Installed", 1, CatalogOrder::All, installation),
                (
                    "not-installed",
                    "Not installed",
                    2,
                    CatalogOrder::All,
                    installation,
                ),
            ]
            .into_iter()
            .enumerate()
            .filter(|(_, (_, _, _, _, show))| *show)
            .map(|(i, (name, label, filter, order, _))| {
                filter_button(
                    ("mod-order", i),
                    name,
                    self.filter == filter && self.order == order,
                    label,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.filter == filter && this.order == order {
                        return;
                    }
                    let reload = this.order != order || this.filter == 1;
                    this.filter = filter;
                    this.order = order;
                    if filter == 1 {
                        this.catalog_task = None;
                    }
                    if reload && filter != 1 {
                        this.load_discovery(None, false, false, cx);
                    }
                    this.filter_rows(cx);
                }))
            }),
        )
    }

    fn source_filters(&self, cx: &mut Context<Self>) -> Div {
        const SOURCES: [&str; 5] = [
            "Nexus Mods",
            "GameBanana",
            "Steam Workshop",
            "Thunderstore",
            "ModWorkshop",
        ];
        pill()
            .child(
                filter_button(
                    "all-mod-sources",
                    "all",
                    self.source_filter.is_none(),
                    "All mod sources",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.source_filter = None;
                    this.filter_rows(cx);
                })),
            )
            .children(SOURCES.into_iter().enumerate().map(|(i, source)| {
                header::button_tooltip(
                    header::button_shell(("mod-source", i), self.source_filter == Some(source)),
                    source,
                )
                .child(crate::icons::mod_source_icon(source))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.source_filter = (this.source_filter != Some(source)).then_some(source);
                    this.filter_rows(cx);
                }))
            }))
    }

    fn header(&self, viewport: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.owner.clone();
        let queue_owner = self.owner.clone();
        let (settings_open, settings_bounds) =
            self.owner.upgrade().map_or((false, None), |owner| {
                let owner = owner.read(cx);
                (owner.menu, Some(owner.settings_bounds.clone()))
            });
        let queue = self.owner.upgrade().and_then(|o| {
            let owner = o.read(cx);
            owner
                .download_summary()
                .map(|label| (label, owner.downloads_open, owner.downloads_bounds.clone()))
        });
        let game = self.game.title.clone();
        let heading = format!("{game} · Mods");
        let title_width = (viewport / self.zoom * 0.34).clamp(6., 33.);
        bar()
            .gap(rems(0.6))
            .child(
                div()
                    .flex()
                    .gap(rems(0.6))
                    .min_w_0()
                    .child(
                        header::back_button("mods-back")
                            .min_w_0()
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::header::Tooltip(heading.clone())).into()
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(rems(title_width))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .when(viewport / self.zoom >= 41., |d| {
                                        d.child(format!("{game} ·"))
                                    }),
                            )
                            .child(div().flex_none().whitespace_nowrap().child("Mods"))
                            .on_click(move |_, window, cx| {
                                owner
                                    .update(cx, |owner, cx| {
                                        owner.mods = None;
                                        window.focus(&owner.focus, cx);
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .when(
                        self.manages_profiles() && !self.compact_profile_header(viewport),
                        |d| d.child(self.profile_controls(viewport, cx)),
                    ),
            )
            .child(
                header::search_field(
                    "mods-search-pill",
                    "search-mods",
                    &self.search,
                    self.search_size,
                    "Search mods",
                    window,
                    cx,
                )
                .on_prepaint(cx.listener(
                    |this, paint: &InteractivityPrepaint, window, cx| {
                        let width = paint.bounds.size.width / window.rem_size();
                        let size = SearchSize::for_width(width);
                        if this.search_size != size {
                            this.search_size = size;
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                div()
                    .flex()
                    .gap(rems(0.6))
                    .flex_none()
                    .when_some(queue, |d, (label, open, bounds)| {
                        d.child(
                            crate::downloads::queue_pill(
                                if viewport / self.zoom < 54. {
                                    String::new()
                                } else {
                                    label
                                },
                                open,
                                bounds,
                            )
                            .on_click(move |_, _, cx| {
                                queue_owner
                                    .update(cx, |owner, cx| owner.toggle_downloads(cx))
                                    .ok();
                            }),
                        )
                    })
                    .when(!self.compact_profile_header(viewport), |d| {
                        d.child(self.filters(cx)).child(self.source_filters(cx))
                    })
                    .child(
                        pill()
                            .child(
                                header::refresh_button(
                                    "refresh-mods",
                                    "Refresh mods",
                                    header::BUTTON_WIDTH,
                                    self.loading,
                                )
                                .when(
                                    !self.loading && self.installing.is_none(),
                                    |d| {
                                        d.on_click(
                                            cx.listener(|this, _, _, cx| this.load(true, cx)),
                                        )
                                    },
                                ),
                            )
                            .child(header::settings_button(
                                self.owner.clone(),
                                header::BUTTON_WIDTH,
                                settings_open,
                                settings_bounds,
                            )),
                    )
                    .when(crate::client_titlebar(window), |d| {
                        d.child(header::window_controls(
                            self.owner.clone(),
                            header::BUTTON_WIDTH,
                        ))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ModBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.cleanup_registered {
            cx.on_release_in(window, |this, window, _| {
                this.thumbnails.cache.clear(window)
            })
            .detach();
            self.cleanup_registered = true;
        }
        if let Some(page) = self.details.clone() {
            return div()
                .absolute()
                .inset_0()
                .occlude()
                .child(page)
                .when(self.dialog.is_some(), |d| d.child(self.profile_dialog(cx)))
                .into_any_element();
        }
        self.zoom = self
            .owner
            .upgrade()
            .map_or(self.zoom, |owner| owner.read(cx).zoom);
        let viewport = f32::from(window.viewport_size().width);
        let landscape = self.landscape_cards();
        let CardGrid {
            columns,
            width,
            row_height,
        } = CardGrid::new(
            viewport,
            self.zoom,
            if landscape { 19. } else { 12.5 },
            if landscape { 1 } else { 2 },
            self.card_ratio(),
        );
        let managed_nexus = raesir_backend::mods::has_managed_nexus(&self.game);
        let add_card = managed_nexus
            && self.filter == 0
            && self.query.is_empty()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.nexus_url.is_some());
        let offset = usize::from(add_card);
        let first_row = {
            let scroll = self.scroll.0.borrow();
            scroll.deferred_scroll_to_item.map_or_else(
                || (-f32::from(scroll.base_handle.offset().y) / row_height).max(0.) as usize,
                |request| request.item_index,
            )
        };
        let visible_rows =
            (f32::from(window.viewport_size().height) / row_height).ceil() as usize + 1;
        let (first, after) = if self.loading {
            (0, 0)
        } else {
            (
                (first_row * columns).saturating_sub(offset),
                ((first_row + visible_rows) * columns).saturating_sub(offset),
            )
        };
        self.maintain_thumbnails(first, after, window, cx);
        let this = cx.weak_entity();
        let row_count = (self.rows.len() + offset).div_ceil(columns);
        let remote_rows = self
            .rows
            .iter()
            .rposition(|&i| self.all_rows[i].remote)
            .map_or(row_count, |last| (last + offset + 1).div_ceil(columns));
        let rows = uniform_list("mods-grid", row_count, move |range, _, cx| {
            this.update(cx, |this, cx| {
                this.load_visible_discovery(range.end, remote_rows, cx);
                range
                    .map(|row| {
                        div()
                            .w_full()
                            .h(px(row_height))
                            .flex()
                            .gap(rems(1.25))
                            .px(rems(1.5))
                            .pb(rems(1.15))
                            .children(
                                (row * columns
                                    ..((row + 1) * columns).min(this.rows.len() + offset))
                                    .map(|i| {
                                        if add_card && i == 0 {
                                            this.install_card(width, cx)
                                        } else {
                                            this.card(i - offset, width, cx)
                                        }
                                    }),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        })
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let profile_bounds = self.profile_bounds.get();
        let compact = self.compact_profile_header(viewport);
        let import_deadlock = raesir_backend::mods::can_install_gamebanana(&self.game);
        let import = self.can_install_nexus() && self.nexus_url().is_some() && !add_card;
        let warnings = self
            .snapshot
            .iter()
            .flat_map(|s| &s.warnings)
            .chain(&self.discovery.warnings)
            .cloned()
            .collect::<Vec<_>>();
        let workshop_warning = warnings
            .iter()
            .any(|warning| warning.starts_with("Steam Workshop:"));
        let empty = self.rows.is_empty() && !add_card;
        let waiting = empty && (self.loading || self.catalog_task.is_some());
        div()
            .absolute()
            .inset_0()
            .occlude()
            .pt(rems(4.9))
            .flex()
            .flex_col()
            .gap(rems(0.9))
            .when_some(
                self.error.clone().filter(|_| self.dialog.is_none()),
                |d, error| {
                    d.child(
                        pill()
                            .mx(rems(1.5))
                            .rounded(rems(1.))
                            .px(rems(1.))
                            .py(rems(0.7))
                            .text_color(rgb(0xffc4ab))
                            .child(error),
                    )
                },
            )
            .when(import || import_deadlock || compact, |d| {
                d.child(
                    div()
                        .mx(rems(1.5))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rems(0.6))
                        .when(compact && self.manages_profiles(), |d| {
                            d.child(self.profile_controls(viewport, cx))
                        })
                        .when(compact, |d| {
                            d.child(self.filters(cx)).child(self.source_filters(cx))
                        })
                        .when(import, |d| d.child(self.nexus_import_button(cx)))
                        .when(import_deadlock, |d| {
                            d.child(
                                crate::views::pill_button(
                                    "import-deadlock",
                                    "Import VPK or archive",
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.import_deadlock_archive(cx)),
                                ),
                            )
                        }),
                )
            })
            .when_some(
                self.snapshot.as_ref().filter(|s| s.uses_smapi()),
                |d, snapshot| d.child(self.smapi_controls(snapshot)),
            )
            .when_some(
                self.nexus_url()
                    .filter(|_| self.can_install_nexus())
                    .and_then(|_| self.nexus_sign_in_banner(cx)),
                |d, banner| d.child(banner),
            )
            .when(waiting, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(spinner("mods-loading", 1.8)),
                )
            })
            .when_some(
                (!warnings.is_empty()).then(|| warnings.join(" · ")),
                |d, warning| {
                    d.child(
                        div()
                            .px(rems(1.5))
                            .when(!workshop_warning, |d| d.text_size(rems(0.9)))
                            .whitespace_normal()
                            .text_color(rgb(0xffc4ab))
                            .child(warning),
                    )
                },
            )
            .when(!waiting && empty, |d| {
                let hint = if self.query.is_empty() && self.filter == 0 {
                    "The connected catalogs returned no mods for this game."
                } else {
                    "Try a different filter or search."
                };
                d.child(crate::views::empty_state("No matching mods", Some(hint), None).flex_1())
            })
            .when(!empty, |d| d.child(rows))
            .when(!waiting && self.catalog_task.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(rems(0.75))
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(pill().p(rems(0.6)).child(spinner("mods-loading-page", 1.2))),
                )
            })
            .child(self.header(viewport, window, cx))
            .when(self.profiles_open, |d| {
                d.child(self.profile_menu(profile_bounds, cx))
            })
            .when(self.dialog.is_some(), |d| d.child(self.profile_dialog(cx)))
            .into_any_element()
    }
}
