use gpui::{prelude::*, *};
use std::borrow::Cow;

pub fn assets() -> AssetRegistry {
    macro_rules! asset {
        ($name:literal, $file:literal) => {
            (
                $name,
                Cow::Borrowed(include_bytes!(concat!("../../static/icons/", $file)).as_slice()),
            )
        };
    }
    [
        asset!("sync", "adwaita/arrows-circle-rotated-symbolic.svg"),
        asset!("all", "adwaita/view-grid-symbolic.svg"),
        asset!("installed", "adwaita/games-symbolic.svg"),
        asset!("not-installed", "adwaita/document-save-symbolic.svg"),
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
    .collect()
}

pub fn icon(name: &'static str) -> Svg {
    let (color, size) = match name {
        "steam" => (0x66c0f4, 1.55),
        "epic" | "ubisoft" => (0xf5f5f5, 1.55),
        "ea" => (0xff585d, 1.55),
        "xbox" => (0x107c10, 1.55),
        "windows" => (0x00a4ef, 1.55),
        "linux" => (0xf6c94c, 1.55),
        "play" => (0x172b22, 1.25),
        _ => (0xbccfc7, 1.25),
    };
    svg()
        .path(name)
        .size(rems(size))
        .flex_none()
        .text_color(rgb(color))
}
