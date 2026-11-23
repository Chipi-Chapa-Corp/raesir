use super::{cover_contents, header_frame};
use gpui::{prelude::*, *};
use std::sync::Arc;

pub(super) type Poster = (Arc<RenderImage>, Option<Arc<RenderImage>>);

pub(super) enum Format {
    Game,
    Portrait(f32),
    Landscape(f32),
}

/// Measure the artwork and its adjacent content before either is painted.
pub(super) struct HeaderArt {
    pub poster: Option<Poster>,
    pub metadata: Option<AnyElement>,
    pub format: Format,
    pub loading: bool,
}

pub(super) struct HeaderArtLayout {
    cover: AnyElement,
    metadata_offset: Pixels,
}

impl IntoElement for HeaderArt {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HeaderArt {
    type RequestLayoutState = HeaderArtLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, HeaderArtLayout) {
        let rem = window.rem_size();
        let width = (window.viewport_size().width.min(rem * 83.) - rem * 7.).max(px(1.));
        let (minimum, ratio, share, gap) = match self.format {
            Format::Game => (18., 2. / 3., 0.35, 1.5),
            Format::Portrait(ratio) => (7., ratio, 0.35, 0.65),
            Format::Landscape(ratio) => (9., ratio, 0.55, 0.65),
        };
        let gap = if self.metadata.is_some() {
            rem * gap
        } else {
            px(0.)
        };
        let width_limit = width * share;
        let minimum_height = rem * minimum;
        let cover_width = |height: Pixels| (height.max(minimum_height) * ratio).min(width_limit);
        let mut art_width = cover_width(minimum_height);
        let mut metadata_height = px(0.);
        if let Some(metadata) = &mut self.metadata {
            // A wider cover can wrap another line of metadata. Resolve that within
            // layout and keep the amount of text measurement bounded.
            for pass in 0..3 {
                let available = (width - art_width - gap).max(px(1.));
                metadata_height = metadata
                    .layout_as_root(
                        size(
                            AvailableSpace::Definite(available),
                            AvailableSpace::MaxContent,
                        ),
                        window,
                        cx,
                    )
                    .height;
                if self.loading || pass == 2 {
                    break;
                }
                let next_width = cover_width(metadata_height);
                if (next_width - art_width).abs() < px(1.) {
                    break;
                }
                art_width = next_width;
            }
        }
        let mut art_height = art_width / ratio;
        if !self.loading && matches!(self.format, Format::Landscape(_)) {
            art_height = art_height.max(metadata_height);
        }
        let mut cover = cover_contents(
            header_frame().w(art_width).h(art_height),
            self.poster.take(),
        )
        .into_any_element();
        cover.layout_as_root(
            size(
                AvailableSpace::Definite(art_width),
                AvailableSpace::Definite(art_height),
            ),
            window,
            cx,
        );
        let style = Style {
            size: size(width.into(), art_height.max(metadata_height).into()),
            flex_shrink: 0.,
            ..Style::default()
        };
        (
            window.request_layout(style, [], cx),
            HeaderArtLayout {
                cover,
                metadata_offset: art_width + gap,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut HeaderArtLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        layout.cover.prepaint_at(bounds.origin, window, cx);
        if let Some(metadata) = &mut self.metadata {
            let origin = bounds.origin + point(layout.metadata_offset, px(0.));
            metadata.prepaint_at(origin, window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut HeaderArtLayout,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        layout.cover.paint(window, cx);
        if let Some(metadata) = &mut self.metadata {
            metadata.paint(window, cx);
        }
    }
}
