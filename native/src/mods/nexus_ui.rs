use super::*;
use crate::views::{action_banner, banner_button, dialog_actions, modal, text_button};

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
            div()
                .px(rems(1.5))
                .child(action_banner(message).id("nexus-sign-in-banner").when_some(
                    label,
                    |d, label| {
                        d.child(
                            banner_button("nexus-sign-in", label).when(!account.busy(), |d| {
                                d.on_click(move |_, _, cx| {
                                    owner.update(cx, |launcher, cx| launcher.sign_in_nexus(cx));
                                })
                            }),
                        )
                    },
                ))
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
                    Ok((NexusInstall::Installed(snapshot), _)) => this.apply_nexus_snapshot(snapshot, cx),
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

    pub(super) fn nexus_import_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        crate::views::pill_button("import-nexus-archive", "Import archive").on_click(cx.listener(
            |this, _, window, cx| {
                if !this.loading && this.installing.is_none() {
                    this.open_nexus_import(None, window, cx);
                }
            },
        ))
    }

    pub(super) fn choose_provider_file(&mut self, id: String, cx: &mut Context<Self>) {
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
                            Some("No supported archive is available for this mod. See its installation instructions.".into())
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

    pub(super) fn provider_files_dialog(
        &self,
        id: &str,
        files: &[raesir_backend::mods::ModFile],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        modal(34., |panel| {
            panel
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
                                        this.perform_nexus_install(id.clone(), Some(file_id), cx);
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
                )
        })
        .into_any_element()
    }

    pub(super) fn install_card(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        ArtworkCard::shortcut(width, self.card_ratio(), self.zoom)
            .id("add-nexus-mod")
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
                        this.apply_nexus_snapshot(snapshot, cx);
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
            .or_else(|| self.nexus_url().cloned())
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
        modal(34., |panel| {
            panel
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Install Mod"))
                .child(div().whitespace_normal().child(description))
                .child(
                    text_button("browse-nexus", browse).on_click(move |_, _, cx| {
                        if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                            cx.open_url(&url);
                        }
                    }),
                )
                .child(
                    pill().rounded(rems(0.8)).px(rems(0.8)).child(
                        text_input("nexus-page-url")
                            .state(self.nexus_url.downgrade())
                            .placeholder("Nexus mod page URL")
                            .flex_1()
                            .min_w_0()
                            .py(rems(0.6))
                            .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                            .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                            .caret_blink_interval_500ms(),
                    ),
                )
                .child(
                    text_button("nexus-archive", archive).when(!self.loading, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.choose_nexus_archive(cx)))
                    }),
                )
                .when(managed_profile, |d| {
                    d.child(
                        pill().rounded(rems(0.8)).px(rems(0.8)).child(
                            text_input("nexus-folder")
                                .state(self.nexus_folder.downgrade())
                                .placeholder("Mod folder name (only for a flat archive)")
                                .flex_1()
                                .min_w_0()
                                .py(rems(0.6))
                                .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                                .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                                .caret_blink_interval_500ms(),
                        ),
                    )
                })
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .text_color(rgb(0xffc4ab))
                            .whitespace_normal()
                            .child(error),
                    )
                })
                .child(
                    dialog_actions()
                        .child(
                            text_button("cancel-nexus", "Cancel").when(!self.loading, |d| {
                                d.on_click(cx.listener(|this, _, window, cx| {
                                    this.dialog = None;
                                    this.error = None;
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                }))
                            }),
                        )
                        .child(
                            text_button("install-nexus", "Install")
                                .when(self.loading, |d| d.child(spinner("nexus-install", 1.2)))
                                .when(!self.loading, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| this.submit_nexus(cx)))
                                }),
                        ),
                )
        })
        .into_any_element()
    }
}
