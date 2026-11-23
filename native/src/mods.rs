use crate::{
    Launcher,
    backend::Backend,
    header::{bar, button, button_shell, filter_button, pill, spinner},
    icons::icon,
};
use gpui::{prelude::*, *};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};
use raesir_backend::{model::LibraryGame, mods::Snapshot};
use std::{collections::HashSet, sync::Arc, time::Duration};

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
    icon: String,
    source: String,
}

pub struct ModBrowser {
    owner: WeakEntity<Launcher>,
    backend: Arc<Backend>,
    game: LibraryGame,
    browse_nexus: bool,
    trending: bool,
    next_page: Option<u32>,
    catalog_task: Option<Task<()>>,
    snapshot: Option<Snapshot>,
    search: Entity<EditableTextState>,
    _search_changed: Subscription,
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
        self.open_mod_browser(key, false, window, cx);
    }

    pub(crate) fn open_nexus_mods(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_mod_browser(key, true, window, cx);
    }

    fn open_mod_browser(
        &mut self,
        key: &str,
        browse_nexus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(game) = self.games.iter().find(|g| g.source.key == key) else {
            return;
        };
        let game = game.source.clone();
        let supported = if browse_nexus {
            raesir_backend::mods::can_browse_nexus(&game)
        } else {
            game.mods.is_some()
        };
        if !supported {
            return;
        }
        if browse_nexus || raesir_backend::mods::has_managed_nexus(&game) {
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
                        if this.browse_nexus {
                            this.load_nexus(None, false, true, cx);
                        } else {
                            this.filter_rows(cx);
                        }
                    }
                },
            );
            ModBrowser {
                owner,
                backend,
                game,
                browse_nexus,
                trending: false,
                next_page: None,
                catalog_task: None,
                snapshot: None,
                search,
                _search_changed: subscription,
                query: String::new(),
                filter: 0,
                all_rows: Vec::new(),
                rows: Vec::new(),
                scroll: UniformListScrollHandle::new(),
                zoom,
                loading: true,
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
            .and_then(|r| self.thumbnails.cache.peek(&r.icon))
            .map(|r| r.poster)
    }

    pub(crate) fn mod_version(&self, id: &str) -> Option<String> {
        self.all_rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.version.clone())
    }

    pub(crate) fn mod_action_label(
        &self,
        id: &str,
        installed: bool,
        cx: &App,
    ) -> Option<&'static str> {
        if installed {
            Some("Uninstall")
        } else if !id.starts_with("nexus:") {
            Some("Install")
        } else if !self.can_install_nexus() {
            None
        } else if self.nexus_signed_in(cx) {
            Some("Install")
        } else if raesir_backend::mods::nexus_account::configured() {
            Some("Sign in to install")
        } else {
            Some("Import downloaded archive")
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
        self.catalog_task = None;
        if installed {
            let plan = self
                .snapshot
                .as_ref()
                .and_then(|s| s.profiles.iter().find(|p| p.id == s.selected))
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
        } else if id.starts_with("modworkshop:") {
            self.choose_modworkshop_file(id, cx);
        } else {
            self.change_mod(id, false, cx);
        }
    }

    fn open_mod(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .rows
            .get(i)
            .and_then(|&i| self.all_rows.get(i))
            .cloned()
        else {
            return;
        };
        let domain = row
            .id
            .starts_with("nexus:")
            .then(|| self.snapshot.as_ref().map(|s| s.community.clone()))
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
            developers: vec![row.owner],
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
        if !row.installed || domain.is_none() || self.browse_nexus {
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
        if self.browse_nexus {
            self.load_nexus(None, refresh, false, cx);
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
                    Ok(snapshot) => {
                        this.error = snapshot.warnings.first().cloned();
                        this.snapshot = Some(snapshot);
                    }
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
        !self.browse_nexus || raesir_backend::mods::has_managed_nexus(&self.game)
    }

    fn can_install_nexus(&self) -> bool {
        if self.browse_nexus {
            self.snapshot.as_ref().is_some_and(|s| !s.selected.is_empty())
        } else {
            raesir_backend::mods::has_managed_nexus(&self.game)
        }
    }

    fn apply_snapshot(&mut self, mut snapshot: Snapshot, cx: &mut Context<Self>) {
        if self.browse_nexus
            && let Some(current) = &self.snapshot
        {
            snapshot.community.clone_from(&current.community);
            snapshot.nexus_url.clone_from(&current.nexus_url);
            snapshot.packages = current.packages.clone();
        }
        self.snapshot = Some(snapshot);
        self.rebuild_rows(cx);
    }

    pub(crate) fn refresh_after_updates(&mut self, cx: &mut Context<Self>) {
        if self.browse_nexus || self.loading || self.installing.is_some() || self.dialog.is_some() {
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
                    this.error = snapshot.warnings.first().cloned();
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
        let mut rows = Vec::new();
        if let Some(snapshot) = &self.snapshot {
            let profile = snapshot.profiles.iter().find(|p| p.id == snapshot.selected);
            let installed: HashSet<_> = profile
                .into_iter()
                .flat_map(|p| &p.installed)
                .map(|m| m.id.as_str())
                .collect();
            let listed: HashSet<_> = snapshot.packages.iter().map(|p| p.id.as_str()).collect();
            rows.extend(snapshot.packages.iter().map(|p| {
                Row {
                    search: String::new(),
                    icon: p.icon.clone(),
                    id: p.id.clone(),
                    title: p.name.replace('_', " "),
                    owner: p.owner.clone(),
                    description: p.description.clone(),
                    version: profile
                        .and_then(|profile| profile.installed.iter().find(|m| m.id == p.id))
                        .map_or_else(|| p.version.clone(), |m| m.version.clone()),
                    page: Some(p.page.clone()),
                    installed: installed.contains(p.id.as_str()),
                    source: p.source.clone(),
                }
            }));
            if let Some(profile) = profile.filter(|_| !self.browse_nexus || self.filter == 1) {
                rows.extend(
                    profile
                        .installed
                        .iter()
                        .filter(|m| {
                            !listed.contains(m.id.as_str())
                                && (!self.browse_nexus || m.id.starts_with("nexus:"))
                        })
                        .map(|m| Row {
                            search: String::new(),
                            icon: String::new(),
                            id: m.id.clone(),
                            title: m.name.replace('_', " "),
                            owner: m.owner.clone(),
                            description: if m.local {
                                "Local mod".into()
                            } else {
                                "Installed mod".into()
                            },
                            version: m.version.clone(),
                            page: if let Some(id) = m.id.strip_prefix("nexus:") {
                                snapshot.nexus_url.as_ref().map(|url| format!("{url}{id}"))
                            } else {
                                (!m.local).then(|| {
                                    format!(
                                        "https://thunderstore.io/c/{}/p/{}/",
                                        snapshot.community,
                                        m.id.replace('-', "/")
                                    )
                                })
                            },
                            installed: true,
                            source: if m.id.starts_with("nexus:") {
                                "Nexus Mods"
                            } else {
                                "Thunderstore"
                            }
                            .into(),
                        }),
                );
            }
        }
        for row in &mut rows {
            row.search = format!("{} {} {}", row.id, row.title, row.description).to_lowercase();
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
                (self.filter == 0
                    || self.filter == 1 && r.installed
                    || self.filter == 2 && !r.installed)
                    && (self.browse_nexus && self.filter != 1
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
        self.catalog_task = None;
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
        self.installing = Some(if id.starts_with("modworkshop:") {
            id.split(':').take(2).collect::<Vec<_>>().join(":")
        } else {
            id.clone()
        });
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        let owner = self.owner.clone();
        let browse_nexus = self.browse_nexus;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if uninstall {
                        backend.uninstall_mod(&game, &id, browse_nexus)
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

    fn landscape_cards(&self) -> bool {
        self.browse_nexus || self.snapshot.as_ref().is_some_and(|s| s.uses_smapi())
    }

    fn card_image_height(&self, width: f32) -> f32 {
        if self.landscape_cards() {
            width * 9. / 16.
        } else {
            width
        }
    }

    fn card(&mut self, i: usize, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(i).and_then(|&i| self.all_rows.get(i)) else {
            return div().into_any_element();
        };
        let landscape = self.landscape_cards();
        let poster_height = self.card_image_height(width);
        let picture = self.thumbnails.cache.get(&row.icon).map(|ready| ready.card);
        let title_overlap = poster_height * crate::CARD_TITLE_OVERLAP;
        let has_cover = picture.is_some();
        let busy = self.installing.as_deref() == Some(&row.id);
        let id = row.id.clone();
        let title = row.title.clone();
        let installed = row.installed;
        let action_tip = self.mod_action_label(&id, installed, cx);
        div()
            .id(("mod-card", i))
            .group("mod-card")
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.open_mod(i, window, cx)))
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(rems(0.8))
            .overflow_hidden()
            .bg(if has_cover {
                rgba(0x101010a6)
            } else {
                rgba(0x101010e6)
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(poster_height))
                    .flex_none()
                    .rounded_t(rems(0.8))
                    .overflow_hidden()
                    .when_some(picture, |d, picture| {
                        d.child(
                            img(picture)
                                .size_full()
                                .rounded_t(rems(0.8))
                                .object_fit(ObjectFit::Cover)
                                .grayscale(!landscape && !installed),
                        )
                    })
                    .when(!has_cover && !landscape, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .p(rems(1.))
                                .pb(rems(3.5))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_center()
                                        .whitespace_normal()
                                        .max_h(rems(4.5))
                                        .overflow_hidden()
                                        .child(title.clone()),
                                ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded_t(rems(0.8))
                            .bg(rgba(0xffffff00))
                            .group_hover("mod-card", |s| {
                                s.bg(linear_gradient(
                                    180.,
                                    linear_color_stop(rgba(0xffffff16), 0.75),
                                    linear_color_stop(rgba(0xffffff00), 1.),
                                ))
                            }),
                    )
                    .when_some(action_tip, |d, action_tip| {
                        d.child(
                            button_shell(("mod-action", i), false)
                                .absolute()
                                .bottom(px(self.zoom * 0.55 + title_overlap))
                                .right(rems(0.55))
                                .bg(if installed {
                                    rgba(0x101010b8)
                                } else {
                                    rgba(0xe6f3eaf0)
                                })
                                .hover(|s| {
                                    s.bg(if installed {
                                        rgba(0x303030e6)
                                    } else {
                                        rgba(0xfffffffa)
                                    })
                                })
                                .child(if busy {
                                    spinner("mod-install-spinner", 1.2)
                                } else {
                                    icon(if installed { "trash" } else { "install" })
                                        .text_color(rgb(if installed {
                                            0xf0f0f0
                                        } else {
                                            0x202020
                                        }))
                                        .into_any_element()
                                })
                                .tooltip(move |_, cx| {
                                    cx.new(|_| crate::library::ActionTooltip(action_tip.into()))
                                        .into()
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_mod_change(id.clone(), installed, window, cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .h(rems(crate::CARD_TITLE_HEIGHT))
                    .mt(px(-title_overlap))
                    .w_full()
                    .flex_none()
                    .px(rems(0.65))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_cover || landscape, |d| {
                        d.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_center()
                                .whitespace_normal()
                                .line_clamp(2)
                                .text_ellipsis()
                                .max_h(rems(2.9))
                                .overflow_hidden()
                                .line_height(rems(1.45))
                                .text_size(rems(1.125))
                                .text_color(if installed || landscape {
                                    rgb(0xe0eae4)
                                } else {
                                    rgb(0xa9b5b0)
                                })
                                .child(title),
                        )
                    }),
            )
            .into_any_element()
    }

    fn compact_profile_header(&self, viewport: f32) -> bool {
        viewport / self.zoom < 48.
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
                        cx.new(|_| crate::library::ActionTooltip(profile_label.clone()))
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
                            cx.new(|_| crate::library::ActionTooltip("New profile".into()))
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

    fn header(&self, viewport: f32, show_controls: bool, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.owner.clone();
        let close_owner = self.owner.clone();
        let settings_owner = self.owner.clone();
        let queue_owner = self.owner.clone();
        let settings_open = self.owner.upgrade().is_some_and(|o| o.read(cx).menu);
        let queue = self.owner.upgrade().and_then(|o| {
            let owner = o.read(cx);
            owner
                .download_summary()
                .map(|label| (label, owner.downloads_open, owner.downloads_bounds.clone()))
        });
        let search_focus = self.search.focus_handle(cx);
        let source = if self.browse_nexus {
            "Nexus Mods"
        } else {
            "Mods"
        };
        let game = self.game.title.clone();
        let heading = format!("{game} · {source}");
        let title_width = (viewport / self.zoom * 0.34).clamp(6., 33.);
        bar()
            .gap(rems(0.6))
            .child(
                div()
                    .flex()
                    .gap(rems(0.6))
                    .min_w_0()
                    .child(
                        pill()
                            .id("mods-back")
                            .h(rems(2.9))
                            .px(rems(0.65))
                            .cursor_pointer()
                            .min_w_0()
                            .child(icon("back").flex_none())
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::library::ActionTooltip(heading.clone()))
                                    .into()
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
                            .child(div().flex_none().whitespace_nowrap().child(source))
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
                pill()
                    .flex_1()
                    .min_w(rems(6.))
                    .max_w(rems(28.))
                    .h(rems(2.9))
                    .px(rems(0.8))
                    .gap(rems(0.65))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&search_focus, cx)
                    })
                    .child(icon("search"))
                    .child(
                        text_input("search-mods")
                            .state(self.search.downgrade())
                            .placeholder(if viewport / self.zoom < 42. {
                                "Search"
                            } else {
                                "Search mods"
                            })
                            .whitespace_nowrap()
                            .overflow_x_scroll()
                            .placeholder_color(rgb(0xacbdb5))
                            .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                            .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                            .caret_blink_interval_500ms()
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(1.125))
                            .line_height(rems(1.6))
                            .px(rems(0.3))
                            .py(rems(0.25))
                            .rounded_full(),
                    ),
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
                    .when(!self.browse_nexus, |d| {
                        d.child(
                            pill().children(
                                [
                                    ("all", "All mods"),
                                    ("installed", "Installed"),
                                    ("not-installed", "Not installed"),
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(i, (name, label))| {
                                    filter_button(
                                        ("mod-filter", i),
                                        name,
                                        self.filter == i as u8,
                                        label,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.filter = i as u8;
                                            this.filter_rows(cx);
                                        },
                                    ))
                                }),
                            ),
                        )
                    })
                    .when(
                        self.browse_nexus
                            && self.snapshot.is_some()
                            && !self.compact_profile_header(viewport),
                        |d| d.child(self.nexus_filters(cx)),
                    )
                    .child(
                        pill()
                            .child(
                                button("refresh-mods", "sync", self.loading)
                                    .tooltip(|_, cx| {
                                        cx.new(|_| {
                                            crate::library::ActionTooltip("Refresh mods".into())
                                        })
                                        .into()
                                    })
                                    .when(!self.loading && self.installing.is_none(), |d| {
                                        d.on_click(
                                            cx.listener(|this, _, _, cx| this.load(true, cx)),
                                        )
                                    }),
                            )
                            .child(button("mods-settings", "menu", settings_open).on_click(
                                move |_, _, cx| {
                                    settings_owner
                                        .update(cx, |owner, cx| {
                                            owner.menu = !owner.menu;
                                            cx.notify();
                                        })
                                        .ok();
                                },
                            )),
                    )
                    .when(show_controls, |d| {
                        d.child(
                            pill()
                                .child(
                                    button("mods-min", "minimize", false)
                                        .on_click(|_, window, _| window.minimize_window()),
                                )
                                .child(
                                    button("mods-max", "maximize", false)
                                        .on_click(|_, window, _| window.zoom_window()),
                                )
                                .child(button("mods-close", "close", false).on_click(
                                    move |_, window, cx| {
                                        close_owner
                                            .update(cx, |owner, cx| owner.close(window, cx))
                                            .ok();
                                    },
                                )),
                        )
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
        let target = if landscape { 19. } else { 12.5 };
        let columns = ((viewport - self.zoom * 3.) / (self.zoom * target))
            .floor()
            .max(if landscape { 1. } else { 2. }) as usize;
        let width = (viewport - self.zoom * (3. + 1.25 * (columns - 1) as f32)) / columns as f32;
        let row_height = (self.card_image_height(width) * (1. - crate::CARD_TITLE_OVERLAP)
            + self.zoom * (crate::CARD_TITLE_HEIGHT + 1.15))
            .ceil();
        let add_card = !self.browse_nexus
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
        let rows = uniform_list("mods-grid", row_count, move |range, _, cx| {
            this.update(cx, |this, cx| {
                this.load_visible_nexus_page(range.end, row_count, cx);
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
        let import = self.browse_nexus && self.can_install_nexus() && self.snapshot.is_some();
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
            .when(
                import || compact && (self.manages_profiles() || self.snapshot.is_some()),
                |d| {
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
                            .when(
                                compact && self.browse_nexus && self.snapshot.is_some(),
                                |d| d.child(self.nexus_filters(cx)),
                            )
                            .when(import, |d| d.child(self.nexus_import_button(cx))),
                    )
                },
            )
            .when_some(
                self.snapshot.as_ref().filter(|s| !self.browse_nexus && s.uses_smapi()),
                |d, snapshot| d.child(self.smapi_controls(snapshot)),
            )
            .when_some(
                self.snapshot
                    .as_ref()
                    .filter(|s| s.nexus_url.is_some())
                    .and_then(|_| self.nexus_sign_in_banner(cx)),
                |d, banner| d.child(banner),
            )
            .when(self.loading, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(spinner("mods-loading", 1.8)),
                )
            })
            .when(
                !self.loading
                    && self.rows.is_empty()
                    && !add_card
                    && (!self.browse_nexus || self.error.is_none()),
                |d| {
                    let hint = if self.browse_nexus && self.snapshot.is_none() {
                        "No Nexus Mods listing matched this game."
                    } else {
                        "Try a different filter or search."
                    };
                    d.child(crate::views::empty_state("No matching mods", Some(hint), None).flex_1())
                },
            )
            .when(!self.loading && (!self.rows.is_empty() || add_card), |d| {
                d.child(rows)
            })
            .when(
                self.browse_nexus && !self.loading && self.catalog_task.is_some(),
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom(rems(0.75))
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                pill()
                                    .p(rems(0.6))
                                    .child(spinner("nexus-loading-next-page", 1.2)),
                            ),
                    )
                },
            )
            .child(self.header(viewport, crate::client_titlebar(window), cx))
            .when(self.profiles_open, |d| {
                d.child(self.profile_menu(profile_bounds, cx))
            })
            .when(self.dialog.is_some(), |d| d.child(self.profile_dialog(cx)))
            .into_any_element()
    }
}
