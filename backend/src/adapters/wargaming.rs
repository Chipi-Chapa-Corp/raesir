//! Read-only WGC discovery. Formats: WGC's apps path index, preferences.xml,
//! game_info.xml and metadata.xml v5/v6/v7. No account credentials are read.
use super::*;
use crate::{epic, runtime, steam};
use roxmltree::{Document, Node};
use std::{fs, path::PathBuf};

const CLIENT: &str = "Wargaming Game Center";
const DATA: &str = "ProgramData/Wargaming.net/GameCenter";

pub fn scan(scan: &mut Scan<'_>) {
    let mut locations = scan.prefixes.clone();
    locations.extend(
        scan.roots
            .iter()
            .filter(|p| p.os != Os::Linux)
            .cloned()
            .map(|mut p| {
                p.os = Os::Windows;
                // Additional Windows drives belong to the running Windows installation.
                p.local = Os::host() == Os::Windows;
                p
            }),
    );
    let mut seen = BTreeSet::new();
    for location in locations {
        let drive = location.drive_c.as_ref().unwrap_or(&location.path);
        let data = drive.join(DATA);
        let mut launchers = vec![
            data.clone(),
            drive.join("Program Files (x86)/Wargaming.net/GameCenter"),
            drive.join("Program Files/Wargaming.net/GameCenter"),
        ];
        let mut indexes = vec![data.join("apps")];
        if let Ok(raw) = steam::read_text(&data.join("data/wgc_path.dat"))
            && let Some(path) = resolve(raw.trim(), &location, scan.report)
        {
            launchers.insert(0, path);
        }
        if location.local && location.os == Os::Windows {
            for variable in ["ProgramData", "ProgramFiles(x86)", "ProgramFiles"] {
                if let Some(path) = std::env::var_os(variable) {
                    let directory = PathBuf::from(path).join("Wargaming.net/GameCenter");
                    if variable == "ProgramData" {
                        indexes.push(directory.join("apps"));
                        if let Ok(raw) = steam::read_text(&directory.join("data/wgc_path.dat"))
                            && let Some(path) = resolve(raw.trim(), &location, scan.report)
                        {
                            launchers.insert(0, path);
                        }
                    }
                    launchers.push(directory);
                }
            }
        }
        let launcher = launchers
            .iter()
            .map(|p| p.join("wgc.exe"))
            .find(|p| p.is_file());
        // apps may be nested by application ID and instance. Only this small
        // index is walked; game payloads are never recursively searched.
        for index in indexes {
            if seen.insert(epic::key(&index)) && index.is_dir() {
                match index_paths(&index, 0) {
                    Ok(paths) => {
                        let mut games = Vec::new();
                        let mut valid = true;
                        for path in paths {
                            match steam::read_text(&path) {
                                Ok(raw) => {
                                    if let Some(resolved) =
                                        resolve(raw.trim(), &location, scan.report)
                                    {
                                        games.push(resolved);
                                    } else {
                                        valid = false;
                                        scan.warnings.push(format!(
                                            "{}: Wargaming game path cannot be resolved",
                                            path.display()
                                        ));
                                    }
                                }
                                Err(e) => {
                                    scan.warnings.push(e);
                                    valid = false;
                                }
                            }
                        }
                        read_games(scan, &location, launcher.as_deref(), &index, games, valid);
                    }
                    Err(e) => scan.warnings.push(e),
                }
            }
        }
        for directory in launchers {
            let preferences = directory.join("preferences.xml");
            if !preferences.is_file() || !seen.insert(epic::key(&preferences)) {
                continue;
            }
            let result = steam::read_text(&preferences).and_then(|text| {
                let doc = xml(&text)?;
                let mut games = Vec::new();
                let mut valid = true;
                for manager in doc
                    .descendants()
                    .filter(|n| n.has_tag_name("games_manager"))
                {
                    for node in manager
                        .descendants()
                        .filter(|n| n.has_tag_name("working_dir"))
                    {
                        if let Some(raw) = node.text() {
                            if let Some(path) = resolve(raw, &location, scan.report) {
                                games.push(path);
                            } else {
                                valid = false;
                            }
                        }
                    }
                    if let Some(root) = value(manager, "default_install_path")
                        .and_then(|p| resolve(p, &location, scan.report))
                    {
                        valid &= root.is_dir();
                        games.extend(
                            discovery::children(&root)
                                .into_iter()
                                .filter(|p| p.join("game_info.xml").is_file()),
                        );
                    }
                }
                Ok((games, valid))
            });
            match result {
                Ok((games, valid)) => read_games(
                    scan,
                    &location,
                    launcher.as_deref(),
                    &preferences,
                    games,
                    valid,
                ),
                Err(e) => scan
                    .warnings
                    .push(format!("{}: {e}", preferences.display())),
            }
        }
        // Offline Windows drives can expose games even when the system drive
        // containing WGC's index is not mounted.
        if location.os == Os::Windows {
            for suffix in ["Games", "Wargaming Games"] {
                let root = drive.join(suffix);
                if root.is_dir() && seen.insert(epic::key(&root)) {
                    let games = discovery::children(&root)
                        .into_iter()
                        .filter(|p| p.join("game_info.xml").is_file())
                        .collect();
                    read_games(scan, &location, launcher.as_deref(), &root, games, true);
                }
            }
        }
    }
}

fn xml(text: &str) -> Result<Document<'_>, String> {
    Document::parse(text.trim_start_matches('\u{feff}')).map_err(|_| "Invalid Wargaming XML".into())
}

fn value<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(name))?
        .text()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn resolve(raw: &str, location: &Location, report: &Report) -> Option<PathBuf> {
    let raw = raw.trim().trim_start_matches('\u{feff}').trim_matches('"');
    if raw.is_empty() || raw.contains(['\0', '\r', '\n']) {
        return None;
    }
    if cfg!(windows)
        && location.local
        && location.os == Os::Windows
        && Path::new(raw).is_absolute()
        && !raw.replace('\\', "/").split('/').any(|p| p == "..")
    {
        return Some(PathBuf::from(raw));
    }
    discovery::resolve(raw, location, report)
}

fn index_paths(directory: &Path, depth: usize) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), depth)];
    let mut visited = 0;
    while let Some((directory, depth)) = pending.pop() {
        for entry in
            fs::read_dir(&directory).map_err(|e| format!("{}: {e}", directory.display()))?
        {
            visited += 1;
            if visited > 2048 {
                return Err("Wargaming installation index is too large".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_file() {
                paths.push(entry.path());
            } else if kind.is_dir() {
                if depth >= 3 {
                    return Err("Wargaming installation index is too deeply nested".into());
                }
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(paths)
}

fn read_games(
    scan: &mut Scan<'_>,
    location: &Location,
    launcher: Option<&Path>,
    source: &Path,
    mut paths: Vec<PathBuf>,
    mut valid: bool,
) {
    paths.sort_by_key(|p| epic::key(p));
    paths.dedup_by_key(|p| epic::key(p));
    for path in paths {
        // A stale index entry is not proof of an installed or owned game.
        if !path.join("game_info.xml").is_file() {
            continue;
        }
        match read_game(&path, location, launcher, source) {
            Ok(Some(game)) => catalog::merge(&mut scan.games, game),
            Ok(None) => {}
            Err(e) => {
                valid = false;
                scan.warnings.push(format!("{}: {e}", path.display()));
            }
        }
    }
    if valid {
        scan.verified.insert(epic::key(source));
    }
}

fn valid_id(id: &str) -> bool {
    id.len() <= 128
        && id.split('.').count() >= 3
        && !id.contains("..")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !id
            .split('.')
            .next()
            .is_some_and(|p| p.eq_ignore_ascii_case("WGC"))
}

fn relative_file(root: &Path, raw: &str) -> Option<PathBuf> {
    let normalized = raw.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains([':', '\0', '\r', '\n'])
        || normalized.split('/').any(|p| p == "..")
    {
        return None;
    }
    Some(root.join(normalized))
}

fn read_game(
    path: &Path,
    location: &Location,
    launcher: Option<&Path>,
    source: &Path,
) -> Result<Option<LibraryGame>, String> {
    let info_text = steam::read_text(&path.join("game_info.xml"))?;
    let info = xml(&info_text)?;
    let metadata_text = steam::read_text(&path.join("game_metadata/metadata.xml"))?;
    let metadata = xml(&metadata_text)?;
    // Steam-published WGC games belong to their original Steam entries.
    for doc in [&info, &metadata] {
        if doc
            .root_element()
            .attribute("wgc_publisher_id")
            .is_some_and(|p| !p.eq_ignore_ascii_case("wargaming"))
        {
            return Ok(None);
        }
    }
    let root = metadata.root_element();
    let section = root
        .children()
        .find(|n| n.has_tag_name("predefined_section"))
        .unwrap_or(root);
    if value(section, "is_tool") == Some("true") {
        return Ok(None);
    }
    let id = value(section, "app_id")
        .ok_or("Wargaming application ID is missing")?
        .to_ascii_uppercase();
    if !valid_id(&id) {
        return Ok(None);
    }
    let game_info = info
        .root_element()
        .children()
        .find(|n| n.has_tag_name("game"))
        .ok_or("Wargaming game state is missing")?;
    if !value(game_info, "id").is_some_and(|s| s.eq_ignore_ascii_case(&id)) {
        return Err("Wargaming metadata and installation IDs do not match".into());
    }
    let title = value(section, "name")
        .or_else(|| value(section, "shortcut_name"))
        .ok_or("Wargaming game title is missing")?;
    let title = if title == "STD2" {
        "Steel Division 2"
    } else {
        title
    };
    let mut title = title.to_owned();
    if let Some((_, variant)) = id.split_once('.')
        && (!id.ends_with(".PRODUCTION") || matches!(id.split('.').nth(1), Some("CT" | "ST")))
    {
        title.push_str(&format!(" ({variant})"));
    }
    let executable = value(section, "executable_name")
        .or_else(|| {
            section
                .children()
                .find(|n| n.has_tag_name("executables"))?
                .children()
                .find(|n| n.has_tag_name("executable") && n.attribute("emul").is_none())?
                .text()
        })
        .and_then(|raw| relative_file(path, raw.trim()));
    let helper = path.join("wgc_api.exe");
    let complete = value(game_info, "installed") == Some("true");
    let mut installation = catalog::install(
        &epic::key(source),
        path.to_string_lossy().into(),
        location.os,
        complete,
    );
    if complete && !executable.as_ref().is_some_and(|p| p.is_file()) {
        installation.state = "files_missing".into();
    }
    let ready = matches!(installation.state.as_str(), "installed" | "external");
    let mut game = catalog::entry(
        "wargaming",
        &id,
        &title,
        vec![installation],
        &epic::key(source),
    );
    game.client = CLIENT.into();
    game.is_game = Some(true);
    game.cover = steam_app(&id).map(|id| {
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/library_600x900.jpg")
    });
    // Registered installations are local library evidence, not a complete account library.
    game.in_library = true;
    game.store_page = Some(catalog::url(
        "Wargaming",
        store_url(&id)
            .unwrap_or("https://wargaming.net/en/games/")
            .into(),
    ));
    game.library = if helper.is_file() {
        action(location, &helper, vec!["--open".into()], path)
    } else {
        launcher.and_then(|exe| action(location, exe, vec![], exe.parent()?))
    };
    if ready {
        // The game's helper preserves WGC's update/authentication handoff.
        game.launch = game.library.clone().map(|mut action| {
            action.note = Some(format!("Select Play for {} in {CLIENT}.", game.title));
            action
        });
        game.uninstall =
            if helper.is_file() && !game.library.as_ref().is_some_and(|a| a.client == "Lutris") {
                action(location, &helper, vec!["--uninstall".into()], path)
            } else {
                game.library.clone().map(|mut action| {
                    action.note = Some(format!(
                        "Use Game Settings to uninstall {} in {CLIENT}.",
                        game.title
                    ));
                    action
                })
            };
    } else if location.local {
        game.install = game.library.clone().map(|mut action| {
            action.note = Some(format!(
                "Select {} in {CLIENT} to finish installing, then refresh your library.",
                game.title
            ));
            action
        });
    }
    Ok(Some(game))
}

fn action(location: &Location, exe: &Path, args: Vec<String>, cwd: &Path) -> Option<GameAction> {
    if !location.local || location.os != Os::host() || !exe.is_file() {
        return None;
    }
    let mut action = if location.os == Os::Windows {
        catalog::command(CLIENT, exe.to_string_lossy().into(), args)
    } else if location.client == "Lutris"
        && let Some(action) = lutris_client(location, &args)
    {
        action
    } else if location.client == "Bottles" {
        let data = steam::read_text(&location.path.join("bottle.yml")).ok()?;
        let config: serde_json::Value = serde_yaml_ng::from_str(&data).ok()?;
        let name = text(&config, "Name")?;
        let (program, mut values) = if location
            .path
            .to_string_lossy()
            .contains("com.usebottles.bottles")
        {
            (
                discovery::program("flatpak")?,
                vec![
                    "run".into(),
                    "--command=bottles-cli".into(),
                    "com.usebottles.bottles".into(),
                ],
            )
        } else {
            (discovery::program("bottles-cli")?, vec![])
        };
        values.extend([
            "run".into(),
            "-b".into(),
            name.into(),
            "-e".into(),
            exe.to_string_lossy().into(),
        ]);
        if !args.is_empty() {
            values.push(format!("--args={}", args.join(" ")));
        }
        catalog::command("Bottles", program, values)
    } else {
        runtime::wine(location, exe, args)?
    };
    if let ActionTarget::Command { cwd: working, .. } = &mut action.target {
        *working = Some(cwd.to_string_lossy().into());
    }
    Some(action)
}

fn lutris_client(location: &Location, requested: &[String]) -> Option<GameAction> {
    let home = crate::platform::home()?;
    let mut profiles = vec![
        (
            home.join(".config/lutris"),
            home.join(".local/share/lutris"),
            false,
        ),
        (
            home.join(".var/app/net.lutris.Lutris/config/lutris"),
            home.join(".var/app/net.lutris.Lutris/data/lutris"),
            true,
        ),
    ];
    if std::env::var_os("XDG_CONFIG_HOME").is_some() || std::env::var_os("XDG_DATA_HOME").is_some()
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        profiles.insert(0, (config.join("lutris"), data.join("lutris"), false));
    }
    for (config, data, flatpak) in profiles {
        for entry in fs::read_dir(config.join("games"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(2048)
        {
            let Ok(content) = steam::read_text(&entry.path()) else {
                continue;
            };
            let Ok(value) = serde_yaml_ng::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            let Some(raw) = value.pointer("/game/prefix").and_then(|v| v.as_str()) else {
                continue;
            };
            let prefix = raw
                .strip_prefix("~/")
                .map(|p| home.join(p))
                .unwrap_or_else(|| PathBuf::from(raw));
            if epic::key(&prefix) != epic::key(&location.path) {
                continue;
            }
            let Some(exe) = value.pointer("/game/exe").and_then(|v| v.as_str()) else {
                continue;
            };
            if !exe
                .replace('\\', "/")
                .rsplit('/')
                .next()
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case("wgc.exe") || name.eq_ignore_ascii_case("wgc_api.exe")
                })
            {
                continue;
            }
            let Ok((_temp, db)) = crate::ea::snapshot_db(&data.join("pga.db")) else {
                continue;
            };
            let Some(stem) = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(id) = lutris_id(&db, &stem) else {
                continue;
            };
            let (program, mut args) = if flatpak {
                (
                    discovery::program("flatpak")?,
                    vec!["run".into(), "net.lutris.Lutris".into()],
                )
            } else {
                (discovery::program("lutris")?, vec![])
            };
            args.push(format!("lutris:rungameid/{id}"));
            let mut action = catalog::command("Lutris", program, args);
            if requested.iter().any(|s| s == "--uninstall") {
                action.note = Some(format!(
                    "Use Game Settings in {CLIENT} to uninstall this game."
                ));
            }
            return Some(action);
        }
    }
    None
}

fn lutris_id(db: &rusqlite::Connection, config: &str) -> Option<u64> {
    let mut query = db
        .prepare("SELECT id FROM games WHERE configpath = ?1 OR configpath = ?2 LIMIT 2")
        .ok()?;
    let ids = query
        .query_map(rusqlite::params![config, format!("{config}.yml")], |row| {
            row.get::<_, i64>(0)
        })
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (ids.len() == 1 && ids[0] > 0).then(|| ids[0] as u64)
}

pub(crate) fn store_url(id: &str) -> Option<&'static str> {
    let mut parts = id.split('.');
    let family = parts.next()?;
    let region = parts.next()?;
    match (family, region) {
        ("WOT", "EU") => Some("https://worldoftanks.eu/en/game/"),
        ("WOT", "ASIA") => Some("https://worldoftanks.asia/en/game/"),
        ("WOT", _) => Some("https://worldoftanks.com/en/game/"),
        ("WOWS", "EU") => Some("https://worldofwarships.eu/en/content/game/"),
        ("WOWS", "ASIA") => Some("https://worldofwarships.asia/en/content/game/"),
        ("WOWS", _) => Some("https://worldofwarships.com/en/content/game/"),
        ("WOWP", "EU") => Some("https://worldofwarplanes.eu/en/game/"),
        ("WOWP", _) => Some("https://worldofwarplanes.com/en/game/"),
        ("WOTB", _) => Some("https://wotblitz.com/en/"),
        _ => None,
    }
}

pub(crate) fn steam_app(id: &str) -> Option<&'static str> {
    // Only production releases: test/sandbox clients are distinct editions.
    if !id.ends_with(".PRODUCTION")
        || !matches!(id.split('.').nth(1), Some("EU" | "NA" | "ASIA" | "WW"))
    {
        return None;
    }
    match id.split('.').next()? {
        "WOT" => Some("1407200"),
        "WOWS" => Some("552990"),
        "WOWP" => Some("790710"),
        "WOTB" => Some("444200"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_boot_copies_merge_but_regions_remain_separate() {
        let dir = tempfile::tempdir().unwrap();
        let windows = dir.path().join("windows");
        let linux = dir.path().join("linux");
        game(&windows, "WOT.EU.PRODUCTION", true, false);
        game(&linux, "WOT.EU.PRODUCTION", true, false);
        let mut games = vec![];
        for (path, os) in [(&windows, Os::Windows), (&linux, Os::Linux)] {
            catalog::merge(
                &mut games,
                read_game(path, &location(path, os, false), None, path)
                    .unwrap()
                    .unwrap(),
            );
        }
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].installations.len(), 2);
        assert_eq!(games[0].client_os, vec![Os::Windows, Os::Linux]);
        game(&linux, "WOT.NA.PRODUCTION", true, false);
        catalog::merge(
            &mut games,
            read_game(&linux, &location(&linux, Os::Linux, false), None, &linux)
                .unwrap()
                .unwrap(),
        );
        assert_eq!(games.len(), 2);
    }

    #[test]
    fn lutris_config_identity_must_resolve_to_one_database_entry() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE games(id INTEGER, configpath TEXT); INSERT INTO games VALUES (42,'wargaming-center'),(77,'unrelated');").unwrap();
        assert_eq!(lutris_id(&db, "wargaming-center"), Some(42));
        assert_eq!(lutris_id(&db, "missing"), None);
        db.execute_batch("INSERT INTO games VALUES (43,'wargaming-center.yml');")
            .unwrap();
        assert_eq!(lutris_id(&db, "wargaming-center"), None);
    }

    #[cfg(unix)]
    #[test]
    fn installation_index_does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("apps");
        fs::create_dir(&index).unwrap();
        write(&dir.path().join("outside/game"), "C:\\Games\\Outside");
        std::os::unix::fs::symlink(dir.path().join("outside"), index.join("linked")).unwrap();
        assert!(index_paths(&index, 0).unwrap().is_empty());
        write(&index.join("deep/a/b/c/game"), "C:\\Games\\TooDeep");
        assert!(index_paths(&index, 0).is_err());
    }

    fn report() -> Report {
        crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        )
    }
    fn location(root: &Path, os: Os, local: bool) -> Location {
        Location {
            path: root.into(),
            os,
            local,
            client: "Wine".into(),
            drive_c: Some(root.into()),
        }
    }
    fn fixture<'a>(report: &'a Report, location: Location) -> Scan<'a> {
        Scan {
            report,
            homes: vec![],
            prefixes: vec![location],
            roots: vec![],
            games: vec![],
            verified: BTreeSet::new(),
            warnings: vec![],
        }
    }
    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn game(path: &Path, id: &str, installed: bool, legacy: bool) {
        write(
            &path.join("game_info.xml"),
            &format!(
                "<protocol name='game_info' wgc_publisher_id='wargaming'><game><id>{id}</id><installed>{installed}</installed></game></protocol>"
            ),
        );
        let section = format!(
            "<app_id>{id}</app_id><name>World of Tanks</name><executables><executable>win64/WorldOfTanks.exe</executable></executables>"
        );
        let metadata = if legacy {
            format!(
                "<protocol><app_id>{id}</app_id><shortcut_name>World of Tanks</shortcut_name><executable_name>win64\\WorldOfTanks.exe</executable_name></protocol>"
            )
        } else {
            format!(
                "<protocol wgc_publisher_id='wargaming'><predefined_section>{section}</predefined_section></protocol>"
            )
        };
        write(&path.join("game_metadata/metadata.xml"), &metadata);
        write(
            &path.join("win64/WorldOfTanks.exe"),
            "fixture, never executed",
        );
        write(&path.join("wgc_api.exe"), "fixture, never executed");
    }

    #[test]
    fn mounted_index_preferences_and_default_folder_merge_without_writes_or_actions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let game_path = root.join("Games/World_of_Tanks");
        game(&game_path, "WOT.EU.PRODUCTION", true, false);
        let data = root.join(DATA);
        write(
            &data.join("apps/WOT/EU/production"),
            "C:\\Games\\World_of_Tanks",
        );
        write(&data.join("data/wgc_path.dat"), "C:\\Custom Client");
        let preferences = root.join("Custom Client/preferences.xml");
        write(
            &preferences,
            "<protocol><application><games_manager><games><game><working_dir>C:\\Games\\World_of_Tanks</working_dir></game></games><default_install_path>C:\\Games</default_install_path></games_manager></application></protocol>",
        );
        let before = fs::read(&preferences).unwrap();
        let report = report();
        let mut scan = fixture(&report, location(root, Os::Windows, false));
        super::scan(&mut scan);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.games.len(), 1);
        let game = &scan.games[0];
        assert_eq!(game.key, "wargaming:WOT.EU.PRODUCTION");
        assert_eq!(game.installations.len(), 1);
        assert_eq!(game.installations[0].os, Os::Windows);
        assert_eq!(game.installations[0].state, "installed");
        assert_eq!(fs::read(preferences).unwrap(), before);
        if Os::host() != Os::Windows {
            assert!(game.launch.is_none() && game.library.is_none() && game.uninstall.is_none());
        }
    }

    #[test]
    fn legacy_metadata_and_test_realms_keep_distinct_ids_and_editions() {
        let dir = tempfile::tempdir().unwrap();
        let location = location(dir.path(), Os::Windows, false);
        let production = dir.path().join("production");
        let test = dir.path().join("test");
        game(&production, "WOT.EU.PRODUCTION", true, true);
        game(&test, "WOT.CT.PRODUCTION", true, false);
        let first = read_game(&production, &location, None, dir.path())
            .unwrap()
            .unwrap();
        let second = read_game(&test, &location, None, dir.path())
            .unwrap()
            .unwrap();
        assert_ne!(first.key, second.key);
        assert!(second.title.contains("CT"));
        assert!(second.cover.is_none());
        assert_eq!(first.title, "World of Tanks");
        game(&test, "WOT.EU.SANDBOX", true, false);
        let sandbox = read_game(&test, &location, None, dir.path())
            .unwrap()
            .unwrap();
        assert!(sandbox.title.contains("SANDBOX"));
        assert!(sandbox.cover.is_none());
        assert!(steam_app(&sandbox.id).is_none());
    }

    #[test]
    fn incomplete_missing_payload_and_conflicting_metadata_never_become_ready() {
        let dir = tempfile::tempdir().unwrap();
        let location = location(dir.path(), Os::Windows, false);
        game(dir.path(), "WOT.EU.PRODUCTION", false, false);
        let read = || {
            read_game(dir.path(), &location, None, dir.path())
                .unwrap()
                .unwrap()
        };
        assert_eq!(read().installations[0].state, "incomplete");
        game(dir.path(), "WOT.EU.PRODUCTION", true, false);
        fs::remove_file(dir.path().join("win64/WorldOfTanks.exe")).unwrap();
        assert_eq!(read().installations[0].state, "files_missing");
        assert!(read().launch.is_none());
        write(
            &dir.path().join("game_info.xml"),
            "<protocol><game><id>WOWS.EU.PRODUCTION</id><installed>true</installed></game></protocol>",
        );
        assert!(read_game(dir.path(), &location, None, dir.path()).is_err());
    }

    #[test]
    fn steam_products_tools_and_unsafe_executable_paths_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let location = location(dir.path(), Os::Windows, false);
        game(dir.path(), "WOT.EU.PRODUCTION", true, false);
        let metadata = dir.path().join("game_metadata/metadata.xml");
        let original = fs::read_to_string(&metadata).unwrap();
        fs::write(&metadata, original.replace("wargaming", "steam")).unwrap();
        assert!(
            read_game(dir.path(), &location, None, dir.path())
                .unwrap()
                .is_none()
        );
        fs::write(
            &metadata,
            original.replace("<name>", "<is_tool>true</is_tool><name>"),
        )
        .unwrap();
        assert!(
            read_game(dir.path(), &location, None, dir.path())
                .unwrap()
                .is_none()
        );
        for raw in [
            "../outside.exe",
            "C:\\outside.exe",
            "\\outside.exe",
            "bin/../../outside.exe",
        ] {
            assert!(relative_file(dir.path(), raw).is_none());
        }
        assert!(!valid_id("WGC.EU.PRODUCTION"));
    }

    #[test]
    fn removed_index_entries_disappear_but_damaged_metadata_retains_unavailable_history() {
        let dir = tempfile::tempdir().unwrap();
        let payload = dir.path().join("Custom/Tanks");
        game(&payload, "WOT.EU.PRODUCTION", true, false);
        let index = dir.path().join(DATA).join("apps/fixture");
        write(&index, "C:\\Custom\\Tanks");
        let mut previous = report();
        {
            let mut scan = fixture(&previous, location(dir.path(), Os::Windows, false));
            super::scan(&mut scan);
            previous.catalog = scan.games;
        }
        write(&payload.join("game_metadata/metadata.xml"), "<broken");
        let mut scan = fixture(&previous, location(dir.path(), Os::Windows, false));
        super::scan(&mut scan);
        super::super::retain(&mut scan, &previous);
        assert_eq!(scan.games.len(), 1);
        assert_eq!(scan.games[0].installations[0].state, "unavailable");
        assert!(scan.games[0].library.is_none());
        assert!(!scan.warnings.is_empty());
        fs::remove_file(index).unwrap();
        let mut scan = fixture(&previous, location(dir.path(), Os::Windows, false));
        super::scan(&mut scan);
        super::super::retain(&mut scan, &previous);
        assert!(scan.games.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proton_actions_preserve_prefix_working_directory_and_helper_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join("steamapps/compatdata/42");
        let prefix = compat.join("pfx");
        let payload = prefix.join("drive_c/Games/Tanks");
        game(&payload, "WOT.EU.PRODUCTION", true, false);
        let proton = dir.path().join("Proton fixture/proton");
        write(&proton, "fixture, never executed");
        write(&compat.join("config_info"), &proton.to_string_lossy());
        let mut location = location(&prefix, Os::Linux, true);
        location.client = "Proton".into();
        location.drive_c = Some(prefix.join("drive_c"));
        let game = read_game(&payload, &location, None, dir.path())
            .unwrap()
            .unwrap();
        for (action, argument) in [
            (&game.launch, "--open"),
            (&game.library, "--open"),
            (&game.uninstall, "--uninstall"),
        ] {
            let ActionTarget::Command { args, env, cwd, .. } = &action.as_ref().unwrap().target
            else {
                panic!("expected command")
            };
            assert_eq!(args.last().unwrap(), argument);
            assert!(args.iter().any(|arg| arg.ends_with("wgc_api.exe")));
            assert_eq!(env["WINEPREFIX"], prefix.to_string_lossy());
            assert_eq!(cwd.as_deref(), Some(payload.to_str().unwrap()));
        }
        assert!(
            game.launch
                .as_ref()
                .unwrap()
                .note
                .as_ref()
                .unwrap()
                .contains("Play")
        );
    }
}
