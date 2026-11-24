use crate::{CARD_TITLE_FONT_SIZE, CARD_TITLE_HEIGHT, CARD_TITLE_OVERLAP};
use gpui::{prelude::*, *};
use std::sync::Arc;

const HOVER_GROUP: &str = "artwork-card";

pub(crate) struct CardGrid {
    pub columns: usize,
    pub width: f32,
    pub row_height: f32,
}

impl CardGrid {
    pub fn new(
        viewport: f32,
        zoom: f32,
        target_width: f32,
        min_columns: usize,
        ratio: f32,
    ) -> Self {
        let columns =
            (((viewport - zoom * 3.) / (zoom * target_width)).floor() as usize).max(min_columns);
        let width = (viewport - zoom * (3. + 1.25 * (columns - 1) as f32)) / columns as f32;
        Self {
            columns,
            width,
            row_height: (card_height(width, ratio, zoom) + zoom * 1.15).ceil(),
        }
    }
}

fn card_height(width: f32, ratio: f32, zoom: f32) -> f32 {
    width * ratio * (1. - CARD_TITLE_OVERLAP) + zoom * CARD_TITLE_HEIGHT
}

fn frame(width: f32, has_cover: bool) -> Div {
    div()
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
}

fn artwork(height: f32) -> Div {
    div()
        .relative()
        .w_full()
        .h(px(height))
        .flex_none()
        .rounded_t(rems(0.8))
        .overflow_hidden()
}

fn title_band(overlap: f32) -> Div {
    div()
        .w_full()
        .h(rems(CARD_TITLE_HEIGHT))
        .mt(px(-overlap))
        // Keep the title below the lighter part of the artwork fade.
        .pt(px(overlap / 2.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
}

pub(crate) struct ArtworkCard {
    pub width: f32,
    /// Artwork height divided by its width.
    pub ratio: f32,
    pub title: String,
    pub cover: Option<Arc<RenderImage>>,
    pub dimmed: bool,
    pub installed: bool,
}

impl ArtworkCard {
    fn overlap(&self) -> f32 {
        self.width * self.ratio * CARD_TITLE_OVERLAP
    }

    pub fn render(
        self,
        poster_id: impl Into<ElementId>,
        badges: Option<Div>,
        actions: Option<Div>,
        zoom: f32,
    ) -> Div {
        let overlap = self.overlap();
        let has_cover = self.cover.is_some();
        let art = artwork(self.width * self.ratio)
            .id(poster_id)
            .when_some(self.cover, |d, poster| {
                let cover = |poster| {
                    img(poster)
                        .size_full()
                        .rounded_t(rems(0.8))
                        .object_fit(ObjectFit::Cover)
                };
                d.child(cover(poster.clone()).when(self.dimmed, |image| {
                    image
                        .grayscale(true)
                        .group_hover(HOVER_GROUP, |s| s.invisible())
                }))
                .when(self.dimmed, |d| {
                    d.child(
                        cover(poster)
                            .absolute()
                            .inset_0()
                            .invisible()
                            .group_hover(HOVER_GROUP, |s| s.visible()),
                    )
                })
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_t(rems(0.8))
                    .when(self.dimmed, |d| {
                        d.bg(linear_gradient(
                            180.,
                            linear_color_stop(rgba(0x020b0755), 0.75),
                            linear_color_stop(rgba(0x020b0700), 1.),
                        ))
                    }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_t(rems(0.8))
                    .bg(rgba(0xffffff00))
                    .group_hover(HOVER_GROUP, |s| {
                        s.bg(linear_gradient(
                            180.,
                            linear_color_stop(rgba(0xffffff16), 0.75),
                            linear_color_stop(rgba(0xffffff00), 1.),
                        ))
                    }),
            )
            .children(badges)
            .when(!has_cover, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            crate::icons::icon("mods")
                                .size(rems(3.))
                                .text_color(rgb(0x777b7a)),
                        ),
                )
            })
            .when(self.installed, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(zoom * 1.1 + overlap))
                        .left(rems(0.8))
                        .size(rems(0.3))
                        .rounded_full()
                        .bg(rgb(0x9ff4bc)),
                )
            })
            .when_some(actions, |d, actions| {
                d.child(
                    actions
                        .absolute()
                        .bottom(px(zoom * 0.55 + overlap))
                        .right(rems(0.55))
                        .flex()
                        .items_center()
                        .gap(rems(0.4)),
                )
            });
        frame(self.width, has_cover)
            .group(HOVER_GROUP)
            .cursor_pointer()
            .child(art)
            .child(
                title_band(overlap).px(rems(0.65)).child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_center()
                        .whitespace_normal()
                        .line_clamp(2)
                        .text_ellipsis()
                        .line_height(rems(1.45))
                        .text_size(rems(CARD_TITLE_FONT_SIZE))
                        .text_color(if self.dimmed {
                            rgb(0xa9b5b0)
                        } else {
                            rgb(0xe0eae4)
                        })
                        .child(title_breaks(&self.title)),
                ),
            )
    }

    pub fn skeleton(width: f32, ratio: f32) -> Div {
        let height = width * ratio;
        frame(width, true)
            .items_center()
            .child(artwork(height).bg(rgba(0xffffff1a)))
            .child(
                title_band(height * CARD_TITLE_OVERLAP).child(
                    div()
                        .w(relative(0.7))
                        .h(rems(1.1))
                        .rounded_full()
                        .bg(rgba(0xffffff1a)),
                ),
            )
    }

    pub fn shortcut(width: f32, ratio: f32, zoom: f32) -> Div {
        frame(width, true)
            .h(px(card_height(width, ratio, zoom)))
            .items_center()
            .justify_center()
            .gap(rems(1.))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0x303030b3)))
    }
}

// CamelCase and acronym boundaries are legal break points without adding spaces.
fn title_breaks(title: &str) -> String {
    let mut result = String::with_capacity(title.len());
    let mut chars = title.chars().peekable();
    let mut previous = None::<char>;
    while let Some(ch) = chars.next() {
        if ch.is_uppercase()
            && previous.is_some_and(|prev| {
                prev.is_lowercase()
                    || prev.is_numeric()
                    || prev.is_uppercase() && chars.peek().is_some_and(|next| next.is_lowercase())
            })
        {
            result.push('\u{200B}');
        }
        result.push(ch);
        previous = Some(ch);
    }
    result
}
