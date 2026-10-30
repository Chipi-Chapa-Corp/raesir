use super::*;
use raesir_lib::details::rich::{Block, Span};

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
                if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
                    cx.open_url(&urls[index]);
                }
            }),
        )
}

impl DetailsPage {
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
                                    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none() {
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
            }
        }
        body
    }
}
