use super::*;
use raesir_backend::details::ModPost;

impl DetailsPage {
    pub(super) fn mod_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let context = self.mod_context.as_ref().expect("mod details context");
        let info = self.data.as_deref();
        let meta = info.and_then(|data| data.mod_metadata.as_ref());
        let browser = context.browser.upgrade();
        let (installed, busy, error) = browser
            .as_ref()
            .map(|browser| browser.read(cx).mod_status(&context.id))
            .unwrap_or_default();
        let action = browser
            .as_ref()
            .and_then(|browser| browser.read(cx).mod_action(&context.id, installed, cx));
        let version = if installed && !context.version.is_empty() {
            context.version.as_str()
        } else {
            meta.map(|metadata| metadata.version.as_str())
                .filter(|version| !version.is_empty())
                .unwrap_or(&context.version)
        };
        let source = self.game.client.clone();
        let poster = self.header_image();
        let ratio = poster.as_ref().map(|(image, _)| {
            let size = image.size(0);
            size.width.0 as f32 / size.height.0.max(1) as f32
        });
        let horizontal = ratio.map_or(source == "Nexus Mods", |ratio| ratio > 1.15);
        let title = info
            .map(|data| data.title.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.game.title)
            .to_owned();
        let heading = mod_row()
            .child(bubble().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(
                bubble()
                    .child(crate::icons::mod_source_icon(&source))
                    .child(source.clone()),
            )
            .when_some(info.and_then(|data| data.store_url.clone()), |d, url| {
                d.child(
                    action_button("mod-source-page", "store", "View on source site")
                        .size(rems(3.3))
                        .on_click(move |_, _, cx| {
                            if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                                cx.open_url(&url);
                            }
                        }),
                )
            });
        let mut controls = mod_row();
        if let Some(meta) = meta {
            let likes = match source.as_str() {
                "Nexus Mods" => "Endorsements",
                "Thunderstore" => "Rating score",
                _ => "Likes",
            };
            for (glyph, label, count) in [
                ("thumb-up", likes, meta.likes),
                ("install", "Downloads", meta.downloads),
                ("eye", "Views", meta.views),
            ] {
                if let Some(count) = count {
                    let hint = format!("{count} {label}");
                    controls = controls.child(
                        bubble()
                            .id(format!("mod-metric-{glyph}"))
                            .child(icon(glyph))
                            .child(reviews::compact_count(count))
                            .aria_label(hint.clone())
                            .tooltip(move |_, cx| cx.new(|_| Tooltip(hint.clone())).into()),
                    );
                }
            }
        }
        if let Some(action) = action.or_else(|| busy.then_some(crate::mods::ModAction::Install)) {
            let id = context.id.clone();
            controls = controls.child(
                action
                    .button(
                        if installed {
                            "mod-uninstall"
                        } else {
                            "mod-install"
                        },
                        busy,
                        false,
                    )
                    .h(rems(3.3))
                    .on_click(move |_, window, cx| {
                        if !busy && let Some(browser) = &browser {
                            browser.update(cx, |browser, cx| {
                                browser.request_mod_change(id.clone(), installed, window, cx)
                            });
                        }
                    }),
            );
        } else {
            let unavailable = if context.id.starts_with("workshop:") {
                "Installing this Workshop item requires a Steam subscription."
            } else {
                "In-app installation is unavailable for this game."
            };
            controls = controls.child(header::button_tooltip(
                header::item_action("mod-install-disabled", false)
                    .h(rems(3.3))
                    .w_auto()
                    .px(rems(0.7))
                    .gap(rems(0.35))
                    .child(icon("install"))
                    .child("Install")
                    .aria_disabled(true)
                    .opacity(0.55)
                    .cursor_default(),
                format!("{unavailable} Follow the mod's manual installation instructions."),
            ));
        }
        let mut facts = Vec::new();
        if !version.is_empty() {
            facts.push(format!("Version {version}"));
        }
        if let Some(info) = info {
            if !info.developers.is_empty() {
                facts.push(format!("By {}", info.developers.join(", ")));
            }
            if !info.released.is_empty() {
                facts.push(format!("Released {}", info.released));
            }
            if let Some(meta) = meta.filter(|meta| !meta.last_updated.is_empty()) {
                facts.push(format!("Updated {}", meta.last_updated));
            }
        }
        let mut metadata = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rems(0.8))
            .child(heading)
            .child(controls);
        if horizontal {
            metadata = metadata.child(HeaderArt {
                poster: poster.clone(),
                metadata: (!facts.is_empty()).then(|| {
                    facts_panel()
                        .children(facts.into_iter().map(paragraph))
                        .into_any_element()
                }),
                format: ArtFormat::Landscape(ratio.unwrap_or(16. / 9.)),
                fixed_cover: self.loading,
            });
        } else if !facts.is_empty() {
            metadata = metadata
                .child(mod_row().children(facts.into_iter().map(|fact| bubble().child(fact))));
        }
        let metadata = metadata.when_some(error, |d, error| {
            d.child(
                div()
                    .text_color(rgb(0xe6c58d))
                    .whitespace_normal()
                    .child(error),
            )
        });
        if horizontal {
            metadata.into_any_element()
        } else {
            HeaderArt {
                poster,
                metadata: Some(metadata.into_any_element()),
                format: ArtFormat::Portrait(ratio.unwrap_or(2. / 3.).clamp(2. / 3., 1.)),
                fixed_cover: self.loading,
            }
            .into_any_element()
        }
    }

    pub(super) fn mod_comments(&self, posts: &[ModPost], cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(rems(0.8))
            .child(heading("Comments"))
            .children(posts.iter().enumerate().map(|(i, post)| {
                self.review_card(
                    &raesir_backend::details::Review {
                        author: post.author.clone(),
                        date: post.date.clone(),
                        title: post.title.clone(),
                        text: post.text.clone(),
                        ..Default::default()
                    },
                    format!("mod-comment-{i}"),
                    false,
                    cx,
                )
            }))
    }

    pub(super) fn mod_posts(&self, label: &'static str, posts: &[ModPost]) -> impl IntoElement {
        panel()
            .child(heading(label))
            .children(posts.iter().map(|post| {
                let byline = [&post.author, &post.date]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ");
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.3))
                    .py(rems(0.55))
                    .when(!post.title.is_empty(), |d| {
                        d.child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(post.title.clone()),
                        )
                    })
                    .when(!byline.is_empty(), |d| d.child(muted(byline)))
                    .when(!post.text.is_empty(), |d| {
                        d.child(
                            div()
                                .whitespace_normal()
                                .children(post.text.lines().map(|line| paragraph(line.to_owned()))),
                        )
                    })
            }))
    }
}

fn mod_row() -> Div {
    div().flex().flex_wrap().items_center().gap(rems(0.65))
}
