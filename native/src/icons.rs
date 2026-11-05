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
        asset!("tux", "tux.png"),
        asset!("macos", "macos.svg"),
        asset!("spinner", "lucide/loader-circle.svg"),
        asset!("mods", "lucide/puzzle.svg"),
        asset!("edit", "lucide/pencil.svg"),
        asset!("check", "adwaita/object-select-symbolic.svg"),
        asset!("calendar", "adwaita/x-office-calendar-symbolic.svg"),
        asset!("esrb-e", "ratings/esrb-e.svg"),
        asset!("esrb-e10", "ratings/esrb-e10.svg"),
        asset!("esrb-t", "ratings/esrb-t.svg"),
        asset!("esrb-m", "ratings/esrb-m.svg"),
        asset!("esrb-ao", "ratings/esrb-ao.svg"),
        asset!("esrb-rp", "ratings/esrb-rp.svg"),
        asset!("pegi-3", "ratings/pegi-3.jpg"),
        asset!("pegi-7", "ratings/pegi-7.jpg"),
        asset!("pegi-12", "ratings/pegi-12.jpg"),
        asset!("pegi-16", "ratings/pegi-16.jpg"),
        asset!("pegi-18", "ratings/pegi-18.jpg"),
        asset!("star", "adwaita/starred-symbolic.svg"),
        asset!("star-empty", "adwaita/non-starred-symbolic.svg"),
        asset!("age", "adwaita/security-high-symbolic.svg"),
        asset!("warning", "adwaita/dialog-warning-symbolic.svg"),
        asset!("expand", "adwaita/go-down-symbolic.svg"),
        asset!("collapse", "adwaita/go-up-symbolic.svg"),
        asset!("more", "adwaita/list-add-symbolic.svg"),
        asset!("achievement-more", "adwaita/dots-horizontal-symbolic.svg"),
        asset!("rare", "material/flash_on.svg"),
        asset!("thumb-up", "material/thumb_up.svg"),
        asset!("thumb-down", "material/thumb_down.svg"),
        asset!("next", "adwaita/go-next-symbolic.svg"),
        asset!("pause", "adwaita/media-playback-pause-symbolic.svg"),
        asset!("rewind", "adwaita/media-seek-backward-symbolic.svg"),
        asset!("forward", "adwaita/media-seek-forward-symbolic.svg"),
        asset!("volume", "adwaita/audio-volume-high-symbolic.svg"),
        asset!("muted", "adwaita/audio-volume-muted-symbolic.svg"),
        asset!("fullscreen", "adwaita/view-fullscreen-symbolic.svg"),
        asset!("restore", "adwaita/view-restore-symbolic.svg"),
        asset!("back", "adwaita/go-previous-symbolic.svg"),
        asset!("sync", "adwaita/arrows-circle-rotated-symbolic.svg"),
        asset!("all", "adwaita/view-grid-symbolic.svg"),
        asset!("wallet", "adwaita/wallet-symbolic.svg"),
        asset!("game-pass", "game-pass.svg"),
        asset!("installed", "adwaita/input-gaming-symbolic.svg"),
        asset!("not-installed", "adwaita/forbidden-symbolic.svg"),
        asset!("install", "adwaita/folder-download-symbolic.svg"),
        asset!("stop", "adwaita/media-playback-stop-symbolic.svg"),
        asset!("trash", "adwaita/user-trash-symbolic.svg"),
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
        asset!("store", "adwaita/org.gnome.Software-symbolic.svg"),
        asset!("steam", "steam.svg"),
        asset!("epic", "epicgames.svg"),
        asset!("ea", "ea.svg"),
        asset!("ubisoft", "ubisoft.svg"),
        asset!("xbox", "xbox.svg"),
        asset!("wargaming", "wargaming.svg"),
        asset!("windows", "windows.svg"),
        asset!("linux", "linux.svg"),
    ]
    .into_iter()
    .collect()
}

pub fn icon(name: &'static str) -> Svg {
    let (color, size) = match name {
        "steam" => (0x66c0f4, 1.55),
        "epic" | "ubisoft" | "wargaming" => (0xf5f5f5, 1.55),
        "ea" => (0xff585d, 1.55),
        "xbox" => (0x107c10, 1.55),
        "game-pass" => (0xf5f5f5, 1.55),
        "windows" => (0x00a4ef, 1.55),
        "linux" => (0xf6c94c, 1.55),
        "play" => (0x202020, 1.25),
        _ => (0xd8d8d8, 1.25),
    };
    svg()
        .path(name)
        .size(rems(size))
        .flex_none()
        .text_color(rgb(color))
}

pub fn system_icon(platform: &str) -> Img {
    let platform = platform.to_ascii_lowercase();
    let name = if platform.contains("linux") {
        "tux"
    } else if platform.contains("mac") || platform.contains("darwin") || platform.contains("osx") {
        "macos"
    } else if platform.contains("xbox") {
        "xbox"
    } else {
        "windows"
    };
    img(name)
        .size(rems(1.55))
        .flex_none()
        .object_fit(ObjectFit::Contain)
}

pub fn rating_image(rating: &str) -> Option<&'static str> {
    let code = rating
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();
    Some(match code.as_str() {
        "ESRBE" | "ESRBEVERYONE" => "esrb-e",
        "ESRBE10" | "ESRBE10PLUS" | "ESRBEVERYONE10" => "esrb-e10",
        "ESRBT" | "ESRBTEEN" => "esrb-t",
        "ESRBM" | "ESRBMATURE" | "ESRBMATURE17" => "esrb-m",
        "ESRBAO" | "ESRBADULTSONLY18" => "esrb-ao",
        "ESRBRP" | "ESRBRATINGPENDING" => "esrb-rp",
        "PEGI3" => "pegi-3",
        "PEGI7" => "pegi-7",
        "PEGI12" => "pegi-12",
        "PEGI16" => "pegi-16",
        "PEGI18" => "pegi-18",
        _ => return None,
    })
}
