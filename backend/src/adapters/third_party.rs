use super::*;
use crate::{catalog, discovery, ea, epic};
use rusqlite::Connection;
use std::{collections::BTreeSet, path::PathBuf};

pub fn scan(scan: &mut Scan<'_>) {
    let mut seen = BTreeSet::new();
    for home in scan.homes.clone() {
        let mut paths = vec![
            home.path.join(".local/share/lutris/pga.db"),
            home.path
                .join(".var/app/net.lutris.Lutris/data/lutris/pga.db"),
        ];
        if home.local
            && let Some(path) = std::env::var_os("XDG_DATA_HOME")
        {
            paths.insert(0, PathBuf::from(path).join("lutris/pga.db"));
        }
        for path in paths {
            if !seen.insert(epic::key(&path)) {
                continue;
            }
            file(scan, &path, |scan| lutris(scan, &home, &path));
        }
        for suffix in [
            ".config/heroic",
            ".var/app/com.heroicgameslauncher.hgl/config/heroic",
            "AppData/Roaming/heroic",
        ] {
            let root = home.path.join(suffix);
            let path = root.join("store_cache/legendary_library.json");
            if let Ok(data) = discovery::json(&path) {
                for row in data
                    .get("library")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let Some(id) = text(row, "app_name") else {
                        continue;
                    };
                    if !epic::is_game(row) {
                        let mut excluded =
                            catalog::entry("epic", id, id, vec![], &epic::key(&path));
                        excluded.is_game = Some(false);
                        catalog::merge(&mut scan.games, excluded);
                        continue;
                    }
                    if let Some(game) = scan
                        .games
                        .iter_mut()
                        .find(|g| g.key == format!("epic:{id}"))
                    {
                        catalog::add_os(&mut game.client_os, home.os);
                        game.in_library = true;
                        if home.local && !catalog::local(&game.installations) {
                            game.client = "Heroic".into();
                        }
                        if let Some(cover) = image(row) {
                            game.cover = Some(cover);
                        }
                        if let Some(slug) = text(row, "store_url") {
                            game.store_page = Some(catalog::url("Epic", slug.into()));
                        }
                    } else if let Some(cover) = image(row) {
                        let mut game = catalog::entry(
                            "epic",
                            id,
                            text(row, "title").unwrap_or(id),
                            vec![],
                            "epic",
                        );
                        game.cover = Some(cover);
                        game.in_library = true;
                        game.client = "Heroic".into();
                        catalog::add_os(&mut game.client_os, home.os);
                        catalog::merge(&mut scan.games, game);
                    }
                }
            }
        }
    }
    attach_runtimes(scan);
    // Attach Bottles' own runner to installations already identified by a store.
    for prefix in scan
        .prefixes
        .clone()
        .into_iter()
        .filter(|p| p.client == "Bottles")
    {
        let path = prefix.path.join("bottle.yml");
        let Ok(content) = crate::steam::read_text(&path) else {
            continue;
        };
        let Ok(data) = serde_yaml_ng::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let name = data
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                prefix
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
            });
        if !prefix.local || name.is_empty() {
            continue;
        }
        let launcher = if prefix
            .path
            .to_string_lossy()
            .contains("com.usebottles.bottles")
        {
            discovery::program("flatpak").map(|b| {
                (
                    b,
                    vec![
                        "run".into(),
                        "--command=bottles-cli".into(),
                        "com.usebottles.bottles".into(),
                    ],
                )
            })
        } else if let Some(bin) = discovery::program("bottles-cli") {
            Some((bin, vec![]))
        } else {
            discovery::program("flatpak").map(|b| {
                (
                    b,
                    vec![
                        "run".into(),
                        "--command=bottles-cli".into(),
                        "com.usebottles.bottles".into(),
                    ],
                )
            })
        };
        let Some((binary, args)) = launcher else {
            continue;
        };
        for game in &mut scan.games {
            // WGC uses its own per-game API helper with the bottle's runner.
            if game.store == "wargaming" {
                continue;
            }
            if !prefix.local || !crate::runtime::matches(&prefix, game, scan.report) {
                continue;
            }
            if !catalog::local(&game.installations) {
                if game.in_library {
                    game.client = "Bottles".into();
                }
                continue;
            }
            let programs = data.get("External_Programs").and_then(|v| v.as_object());
            let program = programs.and_then(|p| {
                p.iter().find(|(_, p)| {
                    let executable = text(p, "path").or_else(|| text(p, "Path")).unwrap_or("");
                    discovery::resolve(executable, &prefix, scan.report)
                        .is_some_and(|p| game.installations.iter().any(|i| p.starts_with(&i.path)))
                })
            });
            if let Some((program_id, _)) = program {
                let mut launch = args.clone();
                launch.extend([
                    "run".into(),
                    "-b".into(),
                    name.into(),
                    "--program-id".into(),
                    program_id.into(),
                ]);
                game.launch = Some(catalog::command("Bottles", binary.clone(), launch));
                game.client = "Bottles".into();
            }
            if game.launch.is_none() {
                let executable=match game.store.as_str() {
                    "ea"=>prefix.path.join("drive_c/Program Files/Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe"),
                    "ubisoft"=>prefix.path.join("drive_c/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/UbisoftConnect.exe"),
                    "epic"=>prefix.path.join("drive_c/Program Files (x86)/Epic Games/Launcher/Portal/Binaries/Win64/EpicGamesLauncher.exe"),
                    _=>continue,
                };
                if executable.is_file() {
                    let uri = match game.store.as_str() {
                        "ea" => format!(
                            "origin2://game/launch?offerIds={}&autoDownload=1",
                            catalog::encode(&game.id)
                        ),
                        "ubisoft" => format!("uplay://launch/{}", catalog::encode(&game.id)),
                        _ => format!(
                            "com.epicgames.launcher://apps/{}?action=launch&silent=true",
                            catalog::encode(&game.id)
                        ),
                    };
                    let mut launch = args.clone();
                    launch.extend([
                        "run".into(),
                        "-b".into(),
                        name.into(),
                        "-e".into(),
                        executable.to_string_lossy().into(),
                        uri,
                    ]);
                    game.launch = Some(catalog::command("Bottles", binary.clone(), launch));
                    game.client = "Bottles".into();
                }
            }
            let mut remove = args.clone();
            remove.extend([
                "tools".into(),
                "-b".into(),
                name.into(),
                "uninstaller".into(),
            ]);
            let mut action = catalog::command("Bottles", binary.clone(), remove);
            action.note =
                Some("Select this game in the bottle's uninstaller to finish removal.".into());
            game.uninstall = Some(action);
        }
    }
}

fn uri(scan: &Scan<'_>, game: &LibraryGame) -> String {
    match game.store.as_str() {
        "ea" => {
            let ids = scan
                .report
                .ea
                .games
                .iter()
                .find(|g| g.id == game.id)
                .map(|g| g.aliases.join(","))
                .unwrap_or_else(|| game.id.clone());
            format!(
                "origin2://game/launch?offerIds={}&autoDownload=1",
                catalog::encode(&ids)
            )
        }
        "ubisoft" => format!("uplay://launch/{}", catalog::encode(&game.id)),
        _ => format!(
            "com.epicgames.launcher://apps/{}?action=launch&silent=true",
            catalog::encode(&game.id)
        ),
    }
}

fn attach_runtimes(scan: &mut Scan<'_>) {
    for prefix in scan
        .prefixes
        .clone()
        .into_iter()
        .filter(|p| p.local && p.client != "Bottles")
    {
        for (store, relative) in [
            (
                "ea",
                "drive_c/Program Files/Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe",
            ),
            (
                "ubisoft",
                "drive_c/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/UbisoftConnect.exe",
            ),
            (
                "epic",
                "drive_c/Program Files (x86)/Epic Games/Launcher/Portal/Binaries/Win64/EpicGamesLauncher.exe",
            ),
        ] {
            let executable = prefix.path.join(relative);
            if !executable.is_file() {
                continue;
            }
            let indices: Vec<_> = scan
                .games
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.store == store
                        && g.launch.is_none()
                        && (catalog::local(&g.installations) || g.in_library)
                        && crate::runtime::matches(&prefix, g, scan.report)
                })
                .map(|(i, _)| i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            let Some(template) = crate::runtime::wine(&prefix, &executable, vec![String::new()])
            else {
                continue;
            };
            for index in indices {
                if !catalog::local(&scan.games[index].installations) {
                    let game = &mut scan.games[index];
                    let address = match store {
                        "ubisoft" => format!("uplay://open/game/{}", catalog::encode(&game.id)),
                        "ea" => "origin2://library/open".into(),
                        _ => "com.epicgames.launcher://store/library".into(),
                    };
                    let mut action = template.clone();
                    if let ActionTarget::Command { args, .. } = &mut action.target
                        && let Some(last) = args.last_mut()
                    {
                        *last = address;
                    }
                    action.note = Some(format!(
                        "Select {} in the client to finish installing, then refresh your library.",
                        game.title
                    ));
                    game.install = Some(action);
                    continue;
                }
                let address = uri(scan, &scan.games[index]);
                let mut action = template.clone();
                if let ActionTarget::Command { args, .. } = &mut action.target
                    && let Some(last) = args.last_mut()
                {
                    *last = address;
                }
                let game = &mut scan.games[index];
                game.client = action.client.clone();
                game.launch = Some(action.clone());
                if store == "ubisoft" {
                    if let ActionTarget::Command { args, .. } = &mut action.target
                        && let Some(last) = args.last_mut()
                    {
                        *last = format!("uplay://uninstall/{}", catalog::encode(&game.id));
                    }
                } else {
                    if let ActionTarget::Command { args, .. } = &mut action.target {
                        args.pop();
                    }
                    action.note = Some(format!(
                        "Finish uninstalling {} in {}.",
                        game.title,
                        if store == "ea" {
                            "EA app"
                        } else {
                            "Epic Games Launcher"
                        }
                    ));
                }
                game.uninstall = Some(action);
            }
        }
    }
}

fn columns(db: &Connection, table: &str) -> BTreeSet<String> {
    let query = format!("PRAGMA table_info({table})");
    db.prepare(&query)
        .ok()
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(1))
                .ok()
                .map(|r| r.filter_map(Result::ok).collect())
        })
        .unwrap_or_default()
}
fn lutris(scan: &mut Scan<'_>, home: &Location, path: &Path) -> Result<(), String> {
    let source = epic::key(path);
    let (_temp, db) = ea::snapshot_db(path)?;
    let fields = columns(&db, "games");
    if fields.is_empty() {
        return Ok(());
    }
    let names = [
        "id",
        "service",
        "service_id",
        "name",
        "directory",
        "installed",
        "runner",
        "slug",
    ];
    let select: Vec<_> = names
        .iter()
        .map(|n| {
            if fields.contains(*n) {
                format!("CAST({n} AS TEXT)")
            } else {
                "NULL".into()
            }
        })
        .collect();
    let query = format!("SELECT {} FROM games LIMIT 20000", select.join(","));
    let mut statement = db.prepare(&query).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| {
            let mut values = Vec::new();
            for i in 0..names.len() {
                values.push(r.get::<_, Option<String>>(i)?);
            }
            Ok(values)
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let row = row.map_err(|e| e.to_string())?;
        let [
            Some(db_id),
            Some(service),
            Some(service_id),
            Some(title),
            directory,
            installed,
            _,
            slug,
        ] = row.as_slice()
        else {
            continue;
        };
        let store = match service.as_str() {
            "steam" => "steam",
            "epic" => "epic",
            "ea_app" | "origin" => "ea",
            "uplay" | "ubisoft" => "ubisoft",
            _ => continue,
        };
        let id = if store == "ea" {
            scan.report
                .ea
                .games
                .iter()
                .find(|g| g.aliases.contains(service_id))
                .map(|g| g.id.as_str())
                .unwrap_or(service_id)
        } else {
            service_id
        };
        let mut installs = vec![];
        if installed.as_deref() == Some("1")
            && let Some(raw) = directory
        {
            let resolved = discovery::resolve(raw, home, scan.report);
            let mut install = catalog::install(
                &source,
                resolved
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(raw.into()),
                Os::Linux,
                true,
            );
            if resolved.is_none() {
                install.state = "unavailable".into();
            }
            installs.push(install);
        }
        let mut game = catalog::entry(store, id, title, installs, &epic::key(path));
        game.in_library = true;
        game.client = "Lutris".into();
        catalog::add_os(&mut game.client_os, Os::Linux);
        if let Some(slug) = slug
            .as_deref()
            .filter(|s| !s.contains(['/', '\\']) && !s.contains(".."))
        {
            let cover = path
                .parent()
                .unwrap_or(Path::new("/"))
                .join("coverart")
                .join(format!("{slug}.jpg"));
            if cover.is_file() {
                game.cover = Some(cover.to_string_lossy().into());
            }
        }
        if home.local && catalog::local(&game.installations) && db_id.parse::<u64>().is_ok() {
            let (program, mut args) = if path.to_string_lossy().contains("net.lutris.Lutris") {
                (
                    discovery::program("flatpak"),
                    vec!["run".into(), "net.lutris.Lutris".into()],
                )
            } else {
                (discovery::program("lutris"), vec![])
            };
            if let Some(program) = program {
                let manage_args = args.clone();
                args.push(format!("lutris:rungameid/{db_id}"));
                game.launch = Some(catalog::command("Lutris", program.clone(), args));
                let mut manage = catalog::command("Lutris", program, manage_args);
                manage.note = Some(
                    "Use Remove in Lutris to uninstall this game with its configured runner."
                        .into(),
                );
                game.uninstall = Some(manage);
            }
        }
        if store == "steam" {
            game.store_page = Some(catalog::url(
                "Steam",
                format!("https://store.steampowered.com/app/{id}/"),
            ));
        }
        catalog::merge(&mut scan.games, game);
    }
    Ok(())
}
