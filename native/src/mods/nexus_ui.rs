use super::*;
use crate::details::text_button;

impl ModBrowser {
    pub(super) fn nexus_sign_in_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let owner = self.owner.upgrade()?;
        let account = &owner.read(cx).nexus_account;
        let configured = raesir_backend::mods::nexus_account::configured();
        let (message, label) = if configured || raesir_backend::mods::nexus_account::testing() {
            let (message, label) = account.banner()?;
            (message, configured.then_some(label))
        } else {
            (
                "Direct Nexus downloads need sign-in. Raesir registration is pending.",
                None,
            )
        };
        Some(
            pill()
                .id("nexus-sign-in-banner")
                .mx(rems(1.5))
                .rounded(rems(1.4))
                .px(rems(1.))
                .py(rems(0.7))
                .gap(rems(0.8))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(message.to_owned()),
                )
                .when_some(label, |d, label| {
                    d.child(
                        text_button("nexus-sign-in", label.to_owned())
                            .rounded_full()
                            .bg(rgba(0xffffff12))
                            .flex_none()
                            .when(!account.busy(), |d| {
                                d.on_click(move |_, _, cx| {
                                    owner.update(cx, |launcher, cx| launcher.sign_in_nexus(cx));
                                })
                            }),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn perform_nexus_install(
        &mut self,
        id: String,
        file: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.installing.is_some() {
            return;
        }
        self.catalog_task = None;
        let row = self.all_rows.iter().find(|row| row.id == id);
        let page = row.and_then(|row| row.page.clone());
        let name = row.map(|row| row.title.clone());
        self.installing = Some(id.clone());
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        let owner = self.owner.clone();
        cx.spawn(async move |this, cx| {
            let package = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    backend.install_nexus_mod(&game, &package, name.as_deref(), file)
                })
                .await;
            if let Ok((_, Some(report))) = &result {
                owner
                    .update(cx, |o, cx| o.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                use raesir_backend::mods::NexusInstall;
                this.installing = None;
                match result {
                    Ok((NexusInstall::Installed(snapshot), _)) => this.apply_snapshot(snapshot, cx),
                    Ok((NexusInstall::ChooseFile(files), _)) => {
                        this.dialog = Some(profile_ui::Dialog::Files { id, files });
                    }
                    Ok((NexusInstall::BrowserDownload, _)) => {
                        this.prepare_nexus_import(page, cx);
                        this.error =
                            Some("Nexus requires a browser download for this file.".into());
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn nexus_filters(&self, cx: &mut Context<Self>) -> Div {
        pill().children(
            [
                ("all", "All mods", self.filter == 0 && !self.trending),
                ("trending", "Trending", self.trending),
                ("installed", "Installed", self.filter == 1),
            ]
            .into_iter()
            .enumerate()
            .take(if self.can_install_nexus() { 3 } else { 2 })
            .map(|(mode, (icon, label, selected))| {
                crate::header::filter_button(("nexus-filter", mode), icon, selected, label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if selected
                            || this.installing.is_some()
                            || this.dialog.is_some()
                            || this.loading && this.catalog_task.is_none()
                        {
                            return;
                        }
                        this.filter = u8::from(mode == 2);
                        this.trending = mode == 1;
                        if mode == 2 {
                            this.catalog_task = None;
                            this.loading = false;
                            this.error = None;
                            this.rebuild_rows(cx);
                        } else {
                            this.load_nexus(None, false, false, cx);
                        }
                    }))
            }),
        )
    }

    pub(super) fn nexus_import_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        text_button("import-nexus-archive", "Import archive").on_click(cx.listener(
            |this, _, window, cx| {
                if !this.loading && this.installing.is_none() {
                    this.open_nexus_import(None, window, cx);
                }
            },
        ))
    }

    pub(super) fn load_nexus(
        &mut self,
        offset: Option<u32>,
        refresh: bool,
        debounce: bool,
        cx: &mut Context<Self>,
    ) {
        if self.installing.is_some()
            || self.dialog.is_some()
            || self.loading && self.catalog_task.is_none() && self.snapshot.is_some()
        {
            return;
        }
        if self.filter == 1 && !refresh {
            self.filter_rows(cx);
            return;
        }
        self.loading = offset.is_none();
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        let snapshot = self.snapshot.clone().filter(|_| !refresh);
        let query = self.query.clone();
        let trending = self.trending;
        let installed_only = self.filter == 1;
        let executor = cx.background_executor().clone();
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                executor.timer(Duration::from_millis(320)).await;
            }
            let result = executor
                .spawn(async move {
                    let snapshot = match snapshot {
                        Some(snapshot) => Some(snapshot),
                        None => backend.nexus_catalog(&game, refresh)?,
                    };
                    let Some(mut snapshot) = snapshot else {
                        return Ok(None);
                    };
                    let page = if installed_only {
                        Ok(raesir_backend::mods::NexusPage {
                            packages: Vec::new(),
                            next: None,
                        })
                    } else {
                        backend.nexus_page(
                            &snapshot.community,
                            &query,
                            offset.unwrap_or(0),
                            trending,
                            refresh,
                        )
                    };
                    if offset.is_none() {
                        snapshot.packages = Arc::default();
                    }
                    Ok::<_, String>(Some((snapshot, page)))
                })
                .await;
            this.update(cx, |this, cx| {
                this.catalog_task = None;
                if offset.is_none() {
                    this.loading = false;
                }
                let old_scroll = this.scroll.0.borrow().base_handle.offset();
                match result {
                    Ok(Some((mut snapshot, Ok(page)))) => {
                        this.next_page = page.next;
                        let packages = Arc::make_mut(&mut snapshot.packages);
                        let listed: HashSet<_> = packages.iter().map(|p| p.id.clone()).collect();
                        packages.extend(
                            page.packages
                                .into_iter()
                                .filter(|p| !listed.contains(&p.id)),
                        );
                        this.snapshot = Some(snapshot);
                    }
                    Ok(Some((snapshot, Err(error)))) => {
                        this.next_page = None;
                        this.snapshot = Some(snapshot);
                        this.error = Some(error);
                    }
                    Ok(None) => {
                        this.next_page = None;
                        this.snapshot = None;
                    }
                    Err(error) => {
                        this.next_page = None;
                        this.snapshot = None;
                        this.error = Some(error);
                    }
                }
                this.rebuild_rows(cx);
                if offset.is_some() {
                    let mut scroll = this.scroll.0.borrow_mut();
                    scroll.deferred_scroll_to_item = None;
                    scroll.base_handle.set_offset(old_scroll);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn next_nexus_offset(&self) -> Option<u32> {
        self.next_page.filter(|_| {
            self.browse_nexus
                && self.filter == 0
                && !self.trending
                && !self.loading
                && self.catalog_task.is_none()
                && self.installing.is_none()
                && self.error.is_none()
                && self.dialog.is_none()
                && self.details.is_none()
                && !self.profiles_open
        })
    }

    pub(super) fn load_visible_nexus_page(
        &self,
        last_visible: usize,
        rows: usize,
        cx: &mut Context<Self>,
    ) {
        if last_visible.saturating_add(1) < rows {
            return;
        }
        let Some(offset) = self.next_nexus_offset() else {
            return;
        };
        let browser = cx.weak_entity();
        cx.defer(move |cx| {
            browser
                .update(cx, |this, cx| {
                    if this.next_nexus_offset() == Some(offset) {
                        this.load_nexus(Some(offset), false, false, cx);
                    }
                })
                .ok();
        });
    }

    pub(super) fn choose_modworkshop_file(&mut self, id: String, cx: &mut Context<Self>) {
        self.installing = Some(id.clone());
        self.error = None;
        let backend = self.backend.clone();
        let game = self.game.clone();
        cx.spawn(async move |this, cx| {
            let query = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { backend.mod_files(&game, &query) })
                .await;
            this.update(cx, |this, cx| {
                this.installing = None;
                match result {
                    Ok(files) if files.len() == 1 => {
                        this.change_mod(format!("{id}:{}", files[0].id), false, cx)
                    }
                    Ok(files) if !files.is_empty() => {
                        this.dialog = Some(profile_ui::Dialog::Files { id, files })
                    }
                    Ok(_) => {
                        this.error =
                            Some("This mod uses an external download; open its mod page".into())
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn modworkshop_files_dialog(
        &self,
        id: &str,
        files: &[raesir_backend::mods::ModFile],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000055))
            .flex()
            .items_center()
            .justify_center()
            .child(
                pill()
                    .w(rems(34.))
                    .p(rems(2.))
                    .rounded(rems(1.8))
                    .flex_col()
                    .items_stretch()
                    .gap(rems(1.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Choose mod file"),
                    )
                    .child(
                        div()
                            .id("mod-file-options")
                            .max_h(rems(20.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .items_stretch()
                            .children(files.iter().enumerate().map(|(i, file)| {
                                let id = id.to_owned();
                                let file_id = file.id;
                                crate::views::text_button(("mod-file", i), file.label.clone())
                                    .whitespace_normal()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dialog = None;
                                        if id.starts_with("nexus:") {
                                            this.perform_nexus_install(
                                                id.clone(),
                                                Some(file_id),
                                                cx,
                                            );
                                        } else {
                                            this.change_mod(format!("{id}:{file_id}"), false, cx);
                                        }
                                    }))
                            })),
                    )
                    .child(
                        text_button("cancel-mod-files", "Cancel").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.dialog = None;
                                this.restore_focus(window, cx);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn install_card(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("add-nexus-mod")
            .w(px(width))
            .h(px(
                width * (1. - crate::CARD_TITLE_OVERLAP) + self.zoom * crate::CARD_TITLE_HEIGHT
            ))
            .flex_none()
            .rounded(rems(0.8))
            .bg(rgba(0x101010a6))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(rems(1.))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0x303030b3)))
            .child(icon("more").size(rems(4.)))
            .child("Install Mod")
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_nexus_import(None, window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn open_nexus_import(
        &mut self,
        page: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_nexus_import(page, cx);
        window.focus(&self.nexus_url.focus_handle(cx), cx);
        cx.notify();
    }

    fn prepare_nexus_import(&mut self, page: Option<String>, cx: &mut Context<Self>) {
        self.catalog_task = None;
        self.nexus_archive = None;
        self.nexus_url
            .update(cx, |f, cx| f.emplace(page.as_deref().unwrap_or(""), cx));
        self.nexus_folder.update(cx, |f, cx| f.emplace("", cx));
        self.details = None;
        self.profiles_open = false;
        self.dialog = Some(profile_ui::Dialog::Nexus { page });
        self.error = None;
    }

    pub(super) fn smapi_controls(&self, snapshot: &Snapshot) -> AnyElement {
        let path = snapshot
            .profiles
            .iter()
            .find(|profile| profile.id == snapshot.selected && !profile.installed.is_empty())
            .map(|profile| profile.path.join("mods"));
        let xbox = self.game.store == "xbox";
        let setup = match (xbox, cfg!(target_os = "windows")) {
            (true, true) => {
                Some("https://stardewvalleywiki.com/Modding:Installing_SMAPI_on_Windows#Xbox_app")
            }
            (true, false) => None,
            (false, _) => Some("https://smapi.io/"),
        };
        let help = if xbox {
            "Xbox PC profiles need a separate SMAPI setup to play these mods."
        } else {
            "Install SMAPI for your native Steam copy to use Launch modded."
        };
        div()
            .mx(rems(1.5))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rems(0.8))
            .child(div().whitespace_normal().child(help))
            .when_some(setup, |d, url| {
                d.child(
                    text_button("smapi-setup", "SMAPI setup").on_click(move |_, _, cx| {
                        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                            cx.open_url(url);
                        }
                    }),
                )
            })
            .when_some(path, |d, path| {
                d.child(
                    text_button("smapi-profile-folder", "Show profile mods").on_click(
                        move |_, _, cx| {
                            if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                                cx.reveal_path(&path);
                            }
                        },
                    ),
                )
            })
            .into_any_element()
    }

    fn choose_nexus_archive(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose the archive downloaded from Nexus".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(Some(paths))) => this.nexus_archive = paths.into_iter().next(),
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn submit_nexus(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.installing.is_some() {
            return;
        }
        let Some(archive) = self.nexus_archive.clone() else {
            self.error = Some("Choose the archive downloaded from Nexus".into());
            cx.notify();
            return;
        };
        let url = self.nexus_url.read(cx).as_str().trim().to_owned();
        let folder = self.nexus_folder.read(cx).as_str().trim().to_owned();
        let game = self.game.clone();
        let backend = self.backend.clone();
        let owner = self.owner.clone();
        self.loading = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.import_nexus_mod(&game, &url, &archive, &folder) })
                .await;
            if let Ok((_, report)) = &result {
                owner
                    .update(cx, |o, cx| o.apply_report(report.clone(), cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((snapshot, _)) => {
                        this.apply_snapshot(snapshot, cx);
                        this.dialog = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn nexus_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = match &self.dialog {
            Some(profile_ui::Dialog::Nexus { page }) => page.as_deref(),
            _ => None,
        };
        let url = page
            .map(|page| format!("{page}?tab=files"))
            .or_else(|| self.snapshot.as_ref().and_then(|s| s.nexus_url.clone()))
            .unwrap_or_default();
        let browse = if page.is_some() {
            "Download from Nexus"
        } else {
            "Browse Nexus Mods"
        };
        let managed_profile = raesir_backend::mods::has_managed_nexus(&self.game);
        let description = if managed_profile {
            "Download the mod from Nexus, then choose its archive to install it in the selected profile."
        } else {
            "Download the mod from Nexus, then choose its archive to install it into this game."
        };
        let archive = self
            .nexus_archive
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose downloaded archive…".into());
        div().absolute().inset_0().occlude().bg(rgba(0x00000055)).flex().items_center().justify_center()
            .child(pill().w(rems(34.)).p(rems(2.)).rounded(rems(1.8)).flex_col().items_stretch().gap(rems(1.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Install Mod"))
                .child(div().whitespace_normal().child(description))
                .child(text_button("browse-nexus",browse).on_click(move|_,_,cx|{
                    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none(){cx.open_url(&url);}
                }))
                .child(pill().rounded(rems(0.8)).px(rems(0.8)).child(
                    text_input("nexus-page-url").state(self.nexus_url.downgrade()).placeholder("Nexus mod page URL")
                        .flex_1().min_w_0().py(rems(0.6)).caret_color(rgb_to_hsla(rgb(0xf0f8f3))).selection_color(rgb_to_hsla(rgba(0xffffff40))).caret_blink_interval_500ms()
                ))
                .child(text_button("nexus-archive",archive).when(!self.loading,|d|d.on_click(cx.listener(|this,_,_,cx|this.choose_nexus_archive(cx)))))
                .when(managed_profile,|d|d.child(pill().rounded(rems(0.8)).px(rems(0.8)).child(
                    text_input("nexus-folder").state(self.nexus_folder.downgrade()).placeholder("Mod folder name (only for a flat archive)")
                        .flex_1().min_w_0().py(rems(0.6)).caret_color(rgb_to_hsla(rgb(0xf0f8f3))).selection_color(rgb_to_hsla(rgba(0xffffff40))).caret_blink_interval_500ms()
                )))
                .when_some(self.error.clone(),|d,error|d.child(div().text_color(rgb(0xffc4ab)).whitespace_normal().child(error)))
                .child(div().flex().justify_end().gap(rems(0.5))
                    .child(text_button("cancel-nexus","Cancel").when(!self.loading,|d|d.on_click(cx.listener(|this,_,window,cx|{this.dialog=None;this.error=None;this.restore_focus(window,cx);cx.notify();}))))
                    .child(text_button("install-nexus","Install").when(self.loading,|d|d.child(spinner("nexus-install",1.2)))
                        .when(!self.loading,|d|d.on_click(cx.listener(|this,_,_,cx|this.submit_nexus(cx)))))
                )
            ).into_any_element()
    }
}
