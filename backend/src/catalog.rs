use crate::{discovery, model::*};
use std::{collections::BTreeMap, path::Path};

pub fn url(client: &str, url: String) -> GameAction {
    GameAction {
        client: client.into(),
        target: ActionTarget::Url { url },
        note: None,
    }
}
pub fn command(client: &str, program: String, args: Vec<String>) -> GameAction {
    GameAction {
        client: client.into(),
        target: ActionTarget::Command {
            program,
            args,
            env: BTreeMap::new(),
            cwd: None,
        },
        note: None,
    }
}
pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn local(installs: &[Installation]) -> bool {
    installs
        .iter()
        .any(|i| i.os == Os::host() && ["installed", "external"].contains(&i.state.as_str()))
}
pub fn entry(
    store: &str,
    id: &str,
    title: &str,
    installs: Vec<Installation>,
    source: &str,
) -> LibraryGame {
    let mut client_os = Vec::new();
    for install in &installs {
        add_os(&mut client_os, install.os);
    }
    LibraryGame {
        key: format!("{store}:{id}"),
        store: store.into(),
        id: id.into(),
        title: title.into(),
        in_library: false,
        installations: installs,
        client: store.into(),
        client_os,
        cover: None,
        launch: None,
        store_page: None,
        uninstall: None,
        source: source.into(),
        is_game: None,
    }
}

pub fn from_report(report: &Report) -> Vec<LibraryGame> {
    let mut result = Vec::new();
    let native_epic = cfg!(windows) || discovery::desktop_command("epic").is_some();
    let legendary = crate::epic_online::binary();
    for game in &report.games {
        let id = game.appid.to_string();
        let mut item = entry(
            "steam",
            &id,
            &game.title,
            game.installations.clone(),
            "steam",
        );
        item.in_library = game.owned;
        if game.owned {
            add_os(&mut item.client_os, report.host_os);
        }
        item.client = "Steam".into();
        item.cover = Some(steam_cover(&id, &report.libraries));
        item.store_page = Some(url(
            "Steam",
            format!("https://store.steampowered.com/app/{id}/"),
        ));
        if local(&item.installations) {
            item.launch = Some(url("Steam", format!("steam://rungameid/{id}")));
            item.uninstall = Some(url("Steam", format!("steam://uninstall/{id}")));
        }
        result.push(item);
    }
    for game in &report.epic.games {
        let mut item = entry(
            "epic",
            &game.app_name,
            &game.title,
            game.installations.clone(),
            "epic",
        );
        item.in_library = !game.library_sources.is_empty();
        for source in &game.library_sources {
            let os = report
                .epic
                .sources
                .iter()
                .find(|s| s.path == *source)
                .map(|s| s.os)
                .unwrap_or_else(|| source_os(source, report.host_os, &report.volumes));
            add_os(&mut item.client_os, os);
        }
        item.client = "Epic".into();
        item.store_page = Some(url(
            "Epic",
            format!(
                "https://store.epicgames.com/browse?q={}",
                encode(&game.title)
            ),
        ));
        let heroic = game
            .installations
            .iter()
            .any(|i| i.library.to_lowercase().contains("heroic"));
        let profile = report.epic.sources.iter().find(|s| {
            s.kind == "legendary"
                && s.local
                && game
                    .installations
                    .iter()
                    .any(|i| i.os == Os::host() && i.library == s.path)
        });
        if local(&item.installations) {
            if heroic {
                item.client = "Heroic".into();
                item.launch = Some(url(
                    "Heroic",
                    format!("heroic://launch/legendary/{}", encode(&game.app_name)),
                ));
            } else if native_epic {
                item.launch = Some(url(
                    "Epic",
                    format!(
                        "com.epicgames.launcher://apps/{}?action=launch&silent=true",
                        encode(&game.app_name)
                    ),
                ));
            } else if let (Some(program), Some(profile)) = (&legendary, profile) {
                let mut action = command(
                    "Legendary",
                    program.to_string_lossy().into(),
                    vec!["launch".into(), "--".into(), game.app_name.clone()],
                );
                if let ActionTarget::Command { env, .. } = &mut action.target {
                    env.insert("LEGENDARY_CONFIG_PATH".into(), profile.path.clone());
                }
                item.client = "Legendary".into();
                item.launch = Some(action);
            }
            if let (Some(program), Some(profile)) = (&legendary, profile) {
                let mut action = command(
                    "Legendary",
                    program.to_string_lossy().into(),
                    vec![
                        "-y".into(),
                        "uninstall".into(),
                        "--".into(),
                        game.app_name.clone(),
                    ],
                );
                if let ActionTarget::Command { env, .. } = &mut action.target {
                    env.insert("LEGENDARY_CONFIG_PATH".into(), profile.path.clone());
                }
                item.uninstall = Some(action);
            }
        }
        if local(&item.installations) && item.uninstall.is_none() {
            let mut manage = if heroic {
                discovery::desktop_command("heroic").map(|(p, a)| command("Heroic", p, a))
            } else if native_epic {
                Some(url("Epic", "com.epicgames.launcher://store/library".into()))
            } else {
                None
            };
            if let Some(action) = &mut manage {
                action.note = Some(format!(
                    "Select {} in {} to finish uninstalling.",
                    game.title, action.client
                ));
            }
            item.uninstall = manage;
        }
        result.push(item);
    }
    for game in &report.ea.games {
        let mut item = entry(
            "ea",
            &game.id,
            &game.title,
            game.installations.clone(),
            "ea",
        );
        item.in_library = !game.library_sources.is_empty();
        for source in &game.library_sources {
            add_os(
                &mut item.client_os,
                source_os(source, report.host_os, &report.volumes),
            );
        }
        item.client = "EA app".into();
        let launch_url = format!(
            "origin2://game/launch?offerIds={}&autoDownload=1",
            encode(&game.aliases.join(","))
        );
        item.store_page = Some(url(
            "EA",
            format!("https://www.ea.com/search?search={}", encode(&game.title)),
        ));
        if cfg!(windows) && local(&item.installations) {
            item.launch = Some(url("EA app", launch_url));
            let mut action = url("Windows", "ms-settings:appsfeatures".into());
            action.note =
                Some("Select this game in Windows Installed Apps to complete removal.".into());
            item.uninstall = Some(action);
        }
        result.push(item);
    }
    result
}

fn steam_cover(id: &str, libraries: &[Source]) -> String {
    for source in libraries {
        let library = Path::new(&source.path);
        let root = if library.file_name().is_some_and(|n| n == "steamapps") {
            library.parent().unwrap_or(library)
        } else {
            library
        };
        let cache = root.join("appcache/librarycache");
        for path in [
            cache.join(format!("{id}_library_600x900.jpg")),
            cache.join(id).join("library_600x900.jpg"),
        ] {
            if path.is_file() {
                return path.to_string_lossy().into_owned();
            }
        }
        if let Ok(entries) = std::fs::read_dir(cache.join(id)) {
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|e| e.path().join("library_600x900.jpg"))
                .collect();
            paths.sort();
            if let Some(path) = paths.into_iter().find(|p| p.is_file()) {
                return path.to_string_lossy().into_owned();
            }
        }
    }
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/library_600x900.jpg")
}

pub fn merge(games: &mut Vec<LibraryGame>, incoming: LibraryGame) {
    if let Some(existing) = games.iter_mut().find(|g| g.key == incoming.key) {
        for os in incoming.client_os {
            add_os(&mut existing.client_os, os);
        }
        let mut seen: std::collections::BTreeSet<_> = existing
            .installations
            .iter()
            .map(|i| (i.path.clone(), format!("{:?}", i.os)))
            .collect();
        existing.installations.extend(
            incoming
                .installations
                .into_iter()
                .filter(|i| seen.insert((i.path.clone(), format!("{:?}", i.os)))),
        );
        existing.in_library |= incoming.in_library;
        existing.is_game = match (existing.is_game, incoming.is_game) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), _) | (_, Some(true)) => Some(true),
            _ => None,
        };
        if incoming.cover.is_some() {
            existing.cover = incoming.cover;
        }
        if incoming.store_page.is_some() {
            existing.store_page = incoming.store_page;
        }
        if incoming.launch.is_some() {
            existing.launch = incoming.launch;
            existing.client = incoming.client;
        }
        if incoming.uninstall.is_some() {
            existing.uninstall = incoming.uninstall;
        }
        if (existing.title == existing.id || incoming.in_library) && incoming.title != incoming.id {
            existing.title = incoming.title;
        }
    } else {
        games.push(incoming);
    }
}

pub fn add_os(systems: &mut Vec<Os>, os: Os) {
    if os != Os::Unknown && !systems.contains(&os) {
        systems.push(os);
    }
}

fn source_os(source: &str, host: Os, volumes: &[Volume]) -> Os {
    let path = source.replace('\\', "/").to_lowercase();
    if [
        "/drive_c/",
        "/lutris/",
        "/ferestre/",
        "/xodus",
        "/.config/",
        "/.local/",
        "/.var/app/",
    ]
    .iter()
    .any(|part| path.contains(part))
    {
        return Os::Linux;
    }
    if path.contains("/appdata/")
        || path.as_bytes().get(1) == Some(&b':')
        || path.starts_with("windows:")
    {
        return Os::Windows;
    }
    volumes
        .iter()
        .filter(|v| v.os != Os::Unknown)
        .flat_map(|v| v.mountpoints.iter().map(move |point| (point, v.os)))
        .filter(|(point, _)| Path::new(source).starts_with(point))
        .max_by_key(|(point, _)| point.len())
        .map(|(_, os)| os)
        .unwrap_or(host)
}

pub fn hydrate_os(report: &mut Report) {
    if report.catalog.iter().all(|game| !game.client_os.is_empty()) {
        return;
    }
    let core = from_report(report);
    for game in &mut report.catalog {
        if !game.client_os.is_empty() {
            continue;
        }
        for install in &game.installations {
            add_os(&mut game.client_os, install.os);
        }
        if let Some(current) = core.iter().find(|g| g.key == game.key) {
            for os in &current.client_os {
                add_os(&mut game.client_os, *os);
            }
        } else if !game.source.is_empty() {
            add_os(
                &mut game.client_os,
                source_os(&game.source, report.host_os, &report.volumes),
            );
        }
    }
}

pub fn visible(game: &LibraryGame) -> bool {
    if game.is_game == Some(false) {
        return false;
    }
    // Older Ubisoft snapshots included products without checking launch metadata.
    !(game.store == "ubisoft"
        && (game.source.ends_with("/configurations") || game.source.ends_with("\\configurations"))
        && game.is_game.is_none()
        && game.installations.is_empty())
}

pub fn install(source: &str, path: String, os: Os, complete: bool) -> Installation {
    let state = if Path::new(&path).is_dir() {
        if complete { "installed" } else { "incomplete" }
    } else {
        "unavailable"
    };
    Installation {
        library: source.into(),
        path,
        os,
        state: state.into(),
        last_seen: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_cover_reads_hashed_and_legacy_library_cache() {
        let dir = tempfile::tempdir().unwrap();
        let libraries = vec![Source {
            path: dir.path().join("steamapps").to_string_lossy().into_owned(),
            os: Os::Linux,
        }];
        let cache = dir.path().join("appcache/librarycache");
        let portrait = cache.join("3527290/asset-hash/library_600x900.jpg");
        std::fs::create_dir_all(portrait.parent().unwrap()).unwrap();
        std::fs::write(&portrait, b"portrait").unwrap();
        assert_eq!(
            steam_cover("3527290", &libraries),
            portrait.to_string_lossy()
        );
        let legacy = cache.join("3527290_library_600x900.jpg");
        std::fs::write(&legacy, b"portrait").unwrap();
        assert_eq!(steam_cover("3527290", &libraries), legacy.to_string_lossy());
        assert!(steam_cover("42", &libraries).starts_with("https://"));
    }

    #[test]
    fn cached_uninstalled_libraries_keep_their_client_os_and_merge_across_systems() {
        let mut report = crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        report.host_os = Os::Linux;
        report.catalog.push(entry(
            "ubisoft",
            "42",
            "Game",
            vec![],
            "/mounted/Windows/Users/user/AppData/Local/Ubisoft/configurations",
        ));
        hydrate_os(&mut report);
        assert_eq!(report.catalog[0].client_os, vec![Os::Windows]);
        let mut linux = entry(
            "ubisoft",
            "42",
            "Game",
            vec![],
            "/home/user/Games/prefix/drive_c/users/user/AppData/Local/Ubisoft/configurations",
        );
        linux
            .client_os
            .push(source_os(&linux.source, report.host_os, &report.volumes));
        merge(&mut report.catalog, linux.clone());
        merge(&mut report.catalog, linux);
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].client_os, vec![Os::Windows, Os::Linux]);
        assert!(report.catalog[0].installations.is_empty());
    }

    #[test]
    fn owned_steam_and_foreign_epic_libraries_have_provenance_without_installs() {
        let mut report = crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        report.host_os = Os::Linux;
        report.games.push(Game {
            appid: 42,
            title: "Steam game".into(),
            owned: true,
            installations: vec![],
        });
        report.epic.sources.push(EpicSource {
            path: "foreign-profile".into(),
            kind: "heroic_cache".into(),
            os: Os::Windows,
            local: false,
            drive_c: None,
        });
        report.epic.games.push(EpicGame {
            app_name: "epic-game".into(),
            title: "Epic game".into(),
            library_sources: vec!["foreign-profile".into()],
            installations: vec![],
        });
        let games = from_report(&report);
        assert_eq!(
            games.iter().find(|g| g.store == "steam").unwrap().client_os,
            vec![Os::Linux]
        );
        assert_eq!(
            games.iter().find(|g| g.store == "epic").unwrap().client_os,
            vec![Os::Windows]
        );
    }
}
