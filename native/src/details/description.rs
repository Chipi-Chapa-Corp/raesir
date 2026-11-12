use super::*;
use exposed_games_backend::details::rich::{Block, Span};

#[derive(Default)]
pub(super) struct AnimationSlot {
    visible: Rc<Cell<bool>>,
    loading: bool,
    error: Option<String>,
    player: Option<Entity<crate::video::Player>>,
}

fn text(id: usize, spans: &[Span]) -> Div {
    let mut content = String::new();
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    let mut urls = Vec::new();
    for span in spans {
        let start = content.len();
        content.push_str(&span.text);
        let range = start..content.len();
        highlights.push((
            range.clone(),
            HighlightStyle {
                font_weight: span.style.bold.then_some(FontWeight::BOLD),
                font_style: span.style.italic.then_some(FontStyle::Italic),
                strikethrough: span.style.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                }),
                color: span.style.link.as_ref().map(|_| rgb_to_hsla(rgb(0x9fd8ff))),
                underline: span.style.link.as_ref().map(|_| UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ));
        if let Some(url) = &span.style.link {
            links.push(range);
            urls.push(url.clone());
        }
    }
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .line_height(rems(1.75))
        .child(
            InteractiveText::new(
                ("description-text", id),
                StyledText::new(content).with_highlights(highlights),
            )
            .on_click(links, move |index, _, cx| {
                if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_none() {
                    cx.open_url(&urls[index]);
                }
            }),
        )
}

impl DetailsPage {
    fn animation_visibility(
        &mut self,
        index: usize,
        url: String,
        dimensions: [u32; 2],
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.animations.get_mut(&index) else {
            return;
        };
        if !slot.visible.get() {
            slot.player = None;
            cx.notify();
            return;
        }
        if slot.loading || slot.player.is_some() || slot.error.is_some() {
            return;
        }
        slot.loading = true;
        let visible = slot.visible.clone();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let cache = backend.cache.clone();
            let result = cx
                .background_executor()
                .spawn(async move { backend.detail_animation(&url) })
                .await;
            this.update(cx, |this, cx| {
                let Some(slot) = this
                    .animations
                    .get_mut(&index)
                    .filter(|s| Rc::ptr_eq(&s.visible, &visible))
                else {
                    return;
                };
                slot.loading = false;
                match result {
                    Ok(path) if slot.visible.get() => {
                        slot.player =
                            Some(cx.new(|cx| {
                                crate::video::Player::animation(path, cache, dimensions, cx)
                            }))
                    }
                    Ok(_) => {}
                    Err(error) => slot.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn description_content(&mut self, data: &Details, cx: &mut Context<Self>) -> Div {
        if data.description_blocks.is_empty() {
            return paragraph(data.description.clone());
        }
        let mut body = div().flex().flex_col().gap(rems(0.85)).w_full().min_w_0();
        for (index, block) in data.description_blocks.iter().enumerate() {
            match block {
                Block::Text(block) => {
                    let content = text(index, &block.spans)
                        .flex_1()
                        .when(block.heading > 0, |d| {
                            d.text_size(rems(1.4))
                                .font_weight(FontWeight::SEMIBOLD)
                                .mt(rems(0.4))
                        });
                    body = body.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(rems(0.6))
                            .pl(rems(block.indent as f32 * 1.25))
                            .when(block.quote, |d| {
                                d.border_l_2()
                                    .border_color(rgba(0xffffff55))
                                    .pl(rems(1.))
                                    .italic()
                            })
                            .when_some(block.marker.clone(), |d, marker| {
                                d.child(
                                    div()
                                        .min_w(rems(1.1))
                                        .flex_none()
                                        .line_height(rems(1.75))
                                        .child(marker),
                                )
                            })
                            .child(content),
                    );
                }
                Block::Image { url, alt, link } => {
                    if self.failed.contains(url) {
                        if !alt.is_empty() {
                            body = body.child(muted(alt.clone()));
                        }
                        continue;
                    }
                    self.image(url, cx);
                    let image = self.images.get(url).cloned();
                    let ratio = image
                        .as_ref()
                        .map(|i| {
                            let size = i.size(0);
                            size.width.0 as f32 / size.height.0.max(1) as f32
                        })
                        .unwrap_or(16. / 9.);
                    body = body.child(
                        div()
                            .id(("description-image", index))
                            .self_center()
                            .w_full()
                            .aspect_ratio(ratio)
                            .max_h(rems(36.))
                            .rounded(rems(0.8))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(link.clone(), |d, link| {
                                d.cursor_pointer().on_click(move |_, _, cx| {
                                    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_none() {
                                        cx.open_url(&link);
                                    }
                                })
                            })
                            .when_some(image, |d, image| {
                                d.child(
                                    img(image)
                                        .size_full()
                                        .object_fit(ObjectFit::Contain)
                                        .rounded(rems(0.8)),
                                )
                            })
                            .when(self.pending.contains(url), |d| {
                                d.bg(rgba(0xffffff0a))
                                    .child(spinner("description-image", 2.))
                            }),
                    );
                }
                Block::Video {
                    url,
                    poster,
                    dimensions,
                } => {
                    let slot = self.animations.entry(index).or_default();
                    let visible = slot.visible.clone();
                    let player = slot.player.clone();
                    let error = slot.error.clone();
                    let loading = slot.loading;
                    if visible.get()
                        && let Some(poster) = poster
                    {
                        self.image(poster, cx);
                    }
                    let poster = poster
                        .as_ref()
                        .and_then(|url| self.images.get(url))
                        .cloned();
                    let owner = cx.weak_entity();
                    let url = url.clone();
                    let dimensions = *dimensions;
                    let fullscreen = self.fullscreen;
                    body = body.child(
                        div()
                            .id(("description-video", index))
                            .relative()
                            .self_center()
                            .w_full()
                            .aspect_ratio(dimensions[0] as f32 / dimensions[1] as f32)
                            .max_h(rems(36.))
                            .rounded(rems(0.8))
                            .overflow_hidden()
                            .on_prepaint(move |paint, window, cx| {
                                let shown = !fullscreen
                                    && paint.bounds.intersects(&window.content_mask().bounds)
                                    && paint.bounds.intersects(&Bounds::new(
                                        Point::default(),
                                        window.viewport_size(),
                                    ));
                                if visible.replace(shown) != shown {
                                    let owner = owner.clone();
                                    let url = url.clone();
                                    cx.defer(move |cx| {
                                        owner
                                            .update(cx, |this, cx| {
                                                this.animation_visibility(
                                                    index, url, dimensions, cx,
                                                )
                                            })
                                            .ok();
                                    });
                                }
                            })
                            .when_some(poster, |d, image| {
                                d.child(
                                    img(image)
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .when_some(player, |d, player| d.child(player))
                            .when(loading, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(spinner("animation-download", 2.)),
                                )
                            })
                            .when_some(error, |d, error| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(pill().p(rems(1.)).child(error)),
                                )
                            }),
                    );
                }
            }
        }
        body
    }
}
