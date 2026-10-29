use gpui::{App, AssetRegistry, RenderImage};
use std::{borrow::Cow, collections::HashMap, sync::Arc};

pub type Icons = Arc<HashMap<&'static str, Arc<RenderImage>>>;

pub fn assets() -> AssetRegistry {
    [(
        "icons/sync.svg",
        Cow::Borrowed(
            include_bytes!("../../static/icons/adwaita/arrows-circle-rotated-symbolic.svg")
                .as_slice(),
        ),
    )]
    .into_iter()
    .collect()
}

pub fn load(cx: &App) -> Icons {
    macro_rules! asset {
        ($name:literal, $file:literal) => {
            (
                $name,
                include_bytes!(concat!("../../static/icons/", $file)).as_slice(),
            )
        };
    }
    Arc::new(
        [
            asset!("all", "adwaita/view-grid-symbolic.svg"),
            asset!("installed", "adwaita/document-save-symbolic.svg"),
            asset!("cloud", "adwaita/weather-overcast-symbolic.svg"),
            asset!("this", "adwaita/computer-symbolic.svg"),
            asset!("other", "adwaita/drive-harddisk-symbolic.svg"),
            asset!("menu", "adwaita/org.gnome.Settings-symbolic.svg"),
            asset!("minimize", "adwaita/window-minimize-symbolic.svg"),
            asset!("maximize", "adwaita/window-maximize-symbolic.svg"),
            asset!("close", "adwaita/window-close-symbolic.svg"),
            asset!("search", "adwaita/edit-find-symbolic.svg"),
            asset!("empty-search", "adwaita/loupe-symbolic.svg"),
            asset!("play", "adwaita/media-playback-start-symbolic.svg"),
            asset!("external", "adwaita/adw-external-link-symbolic.svg"),
            asset!("steam", "steam.svg"),
            asset!("epic", "epicgames.svg"),
            asset!("ea", "ea.svg"),
            asset!("ubisoft", "ubisoft.svg"),
            asset!("xbox", "xbox.svg"),
            asset!("windows", "windows.svg"),
            asset!("linux", "linux.svg"),
        ]
        .into_iter()
        .map(|(name, bytes)| {
            (
                name,
                cx.svg_renderer()
                    .render_single_frame(bytes, 4.)
                    .expect("bundled icon SVG"),
            )
        })
        .collect(),
    )
}
