use super::*;
use crate::details::text_button;

impl ModBrowser {
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
        files: &[exposed_games_backend::mods::ModFile],
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
                                let request = format!("{id}:{}", file.id);
                                crate::views::text_button(("mod-file", i), file.label.clone())
                                    .whitespace_normal()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dialog = None;
                                        this.change_mod(request.clone(), false, cx)
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
                this.nexus_archive = None;
                this.nexus_url.update(cx, |f, cx| f.emplace("", cx));
                this.nexus_folder.update(cx, |f, cx| f.emplace("", cx));
                this.dialog = Some(profile_ui::Dialog::Nexus);
                this.error = None;
                window.focus(&this.nexus_url.focus_handle(cx), cx);
                cx.notify();
            }))
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
                        this.snapshot = Some(snapshot);
                        this.dialog = None;
                        this.rebuild_rows(cx);
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
        let url = self
            .snapshot
            .as_ref()
            .and_then(|s| s.nexus_url.clone())
            .unwrap_or_default();
        let archive = self
            .nexus_archive
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose downloaded archive…".into());
        div().absolute().inset_0().occlude().bg(rgba(0x00000055)).flex().items_center().justify_center()
            .child(pill().w(rems(34.)).p(rems(2.)).rounded(rems(1.8)).flex_col().items_stretch().gap(rems(1.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Install Mod"))
                .child(div().whitespace_normal().child("Download the mod from Nexus, then choose its archive. Installs into the selected profile."))
                .child(text_button("browse-nexus","Browse Nexus Mods").on_click(move|_,_,cx|{
                    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_none(){cx.open_url(&url);}
                }))
                .child(pill().rounded(rems(0.8)).px(rems(0.8)).child(
                    text_input("nexus-page-url").state(self.nexus_url.downgrade()).placeholder("Nexus mod page URL")
                        .flex_1().min_w_0().py(rems(0.6)).caret_color(rgb_to_hsla(rgb(0xf0f8f3))).selection_color(rgb_to_hsla(rgba(0xffffff40))).caret_blink_interval_500ms()
                ))
                .child(text_button("nexus-archive",archive).when(!self.loading,|d|d.on_click(cx.listener(|this,_,_,cx|this.choose_nexus_archive(cx)))))
                .child(pill().rounded(rems(0.8)).px(rems(0.8)).child(
                    text_input("nexus-folder").state(self.nexus_folder.downgrade()).placeholder("Mod folder name (only for a flat archive)")
                        .flex_1().min_w_0().py(rems(0.6)).caret_color(rgb_to_hsla(rgb(0xf0f8f3))).selection_color(rgb_to_hsla(rgba(0xffffff40))).caret_blink_interval_500ms()
                ))
                .when_some(self.error.clone(),|d,error|d.child(div().text_color(rgb(0xffc4ab)).whitespace_normal().child(error)))
                .child(div().flex().justify_end().gap(rems(0.5))
                    .child(text_button("cancel-nexus","Cancel").when(!self.loading,|d|d.on_click(cx.listener(|this,_,window,cx|{this.dialog=None;this.error=None;this.restore_focus(window,cx);cx.notify();}))))
                    .child(text_button("install-nexus","Install").when(self.loading,|d|d.child(spinner("nexus-install",1.2)))
                        .when(!self.loading,|d|d.on_click(cx.listener(|this,_,_,cx|this.submit_nexus(cx)))))
                )
            ).into_any_element()
    }
}
