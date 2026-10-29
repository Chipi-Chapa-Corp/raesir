//! Read-only Epic inventory. Client metadata is never treated as a login token.
use crate::{epic_online, model::*, platform, steam};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn key(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.into());
    let value = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

fn children(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.is_symlink())
        .take(256)
        .collect()
}

fn add(out: &mut Vec<EpicSource>, path: PathBuf, kind: &str, os: Os, drive: Option<&Path>) {
    if !path.exists() {
        return;
    }
    let path = key(&path);
    if !out.iter().any(|s| s.path == path && s.kind == kind) {
        out.push(EpicSource {
            path,
            kind: kind.into(),
            os,
            local: false,
            drive_c: drive.map(|p| p.to_string_lossy().into()),
        });
    }
}

fn config_at(root: &Path, os: Os, out: &mut Vec<EpicSource>) {
    if root.join("installed.json").exists()
        || root.join("metadata").is_dir()
        || root.join("assets.json").exists()
        || root.join("config.ini").is_file()
    {
        add(out, root.into(), "legendary", os, None);
        add(
            out,
            root.join("third-party-installed.json"),
            "heroic_external",
            os,
            None,
        );
    }
    for suffix in [
        "store_cache/legendary_library.json",
        "store/legendary_library.json",
    ] {
        add(out, root.join(suffix), "heroic_cache", os, None);
    }
    for suffix in ["legendaryConfig/legendary", "legendary"] {
        let path = root.join(suffix);
        if path.is_dir() {
            config_at_leaf(&path, os, out);
        }
    }
}

fn config_at_leaf(root: &Path, os: Os, out: &mut Vec<EpicSource>) {
    // Heroic can have an authenticated, not-yet-refreshed Legendary profile.
    add(out, root.into(), "legendary", os, None);
    add(
        out,
        root.join("third-party-installed.json"),
        "heroic_external",
        os,
        None,
    );
}

fn windows_at(root: &Path, os: Os, drive: Option<&Path>, out: &mut Vec<EpicSource>) {
    let os = if os == Os::Unknown { Os::Windows } else { os };
    for prefix in ["", "ProgramData"] {
        let data = root.join(prefix);
        add(
            out,
            data.join("Epic/EpicGamesLauncher/Data/Manifests"),
            "egl_manifests",
            os,
            drive,
        );
        add(
            out,
            data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat"),
            "egl_dat",
            os,
            drive,
        );
    }
}

fn wine_at(prefix: &Path, os: Os, out: &mut Vec<EpicSource>) {
    for suffix in ["drive_c", "pfx/drive_c", ""] {
        let drive = prefix.join(suffix);
        if drive.join("ProgramData/Epic").is_dir() {
            windows_at(&drive, os, Some(&drive), out);
        }
    }
}

fn home_at(home: &Path, os: Os, out: &mut Vec<EpicSource>) {
    for suffix in [
        ".config/legendary",
        ".config/heroic",
        "AppData/Roaming/heroic",
        "AppData/Roaming/legendary",
        "Library/Application Support/heroic",
        ".var/app/com.heroicgameslauncher.hgl/config/heroic",
        ".var/app/com.heroicgameslauncher.hgl/config/legendary",
    ] {
        config_at(&home.join(suffix), os, out);
    }
    for suffix in [
        ".wine",
        "Games/epic-games-store",
        "Games/epic-games-launcher",
    ] {
        wine_at(&home.join(suffix), os, out);
    }
    for suffix in [
        "Games",
        ".local/share/bottles/bottles",
        ".var/app/com.usebottles.bottles/data/bottles/bottles",
    ] {
        for prefix in children(&home.join(suffix)) {
            wine_at(&prefix, os, out);
        }
    }
}

/// Known bounded layouts, including mounted OS roots, homes and explicit files.
pub fn sources_at(root: &Path, hint: Os) -> Vec<EpicSource> {
    let mut found = Vec::new();
    let os = match platform::os_at(root) {
        Os::Unknown => hint,
        os => os,
    };
    if root.is_file() {
        match root.file_name().and_then(|p| p.to_str()).unwrap_or("") {
            "installed.json" | "assets.json" => config_at(root.parent().unwrap(), os, &mut found),
            "LauncherInstalled.dat" => add(&mut found, root.into(), "egl_dat", os, None),
            "legendary_library.json" => add(&mut found, root.into(), "heroic_cache", os, None),
            name if name.ends_with(".item") => add(&mut found, root.into(), "egl_item", os, None),
            name if name.ends_with(".json") => {
                add(&mut found, root.into(), "legendary_export", os, None)
            }
            _ => {}
        }
        return found;
    }
    config_at(root, os, &mut found);
    home_at(root, os, &mut found);
    windows_at(root, os, Some(root), &mut found);
    wine_at(root, os, &mut found);
    if root
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("Manifests"))
    {
        add(&mut found, root.into(), "egl_manifests", os, None);
    }
    for directory in ["home", "var/home", "@home", "Users", "@/home", "@/var/home"] {
        let parent = root.join(directory);
        if parent.is_symlink() {
            continue;
        }
        for user in children(&parent) {
            let user_os = if directory == "Users" {
                Os::Windows
            } else {
                Os::Linux
            };
            home_at(&user, user_os, &mut found);
        }
    }
    // A separate home partition exposes user directories directly.
    if !root.join("etc").exists() && !root.join("Windows").exists() {
        for user in children(root) {
            home_at(&user, os, &mut found);
        }
    }
    found
}

pub fn local_sources() -> Vec<EpicSource> {
    let mut sources = Vec::new();
    if let Some(home) = platform::home() {
        home_at(&home, Os::host(), &mut sources);
    }
    if let Some(root) = env::var_os("LEGENDARY_CONFIG_PATH") {
        config_at_leaf(Path::new(&root), Os::host(), &mut sources);
    }
    if let Some(root) = env::var_os("XDG_CONFIG_HOME") {
        for name in ["heroic", "legendary"] {
            config_at(&PathBuf::from(&root).join(name), Os::host(), &mut sources);
        }
    }
    if let Some(root) = env::var_os("APPDATA") {
        for name in ["heroic", "legendary"] {
            config_at(&PathBuf::from(&root).join(name), Os::host(), &mut sources);
        }
    }
    if let Some(root) = env::var_os("PROGRAMDATA") {
        windows_at(Path::new(&root), Os::host(), None, &mut sources);
    }
    if cfg!(windows) {
        windows_at(Path::new("C:/"), Os::Windows, None, &mut sources);
    }
    if let Some(prefix) = env::var_os("WINEPREFIX") {
        wine_at(Path::new(&prefix), Os::host(), &mut sources);
    }
    for source in &mut sources {
        source.local = true;
    }
    sources
}

pub fn scan(
    options: &ScanOptions,
    report: &Report,
    online: &[EpicOnlineLibrary],
    previous: Option<&EpicReport>,
    prefixes: &[crate::discovery::Location],
) -> (EpicReport, Vec<String>) {
    let mut sources = local_sources();
    let mut mounts: Vec<_> = report
        .volumes
        .iter()
        .flat_map(|v| &v.mountpoints)
        .map(PathBuf::from)
        .filter(|p| platform::scan_mount(p))
        .collect();
    for volume in &report.volumes {
        for mount in &volume.mountpoints {
            if platform::scan_mount(Path::new(mount)) {
                sources.extend(sources_at(Path::new(mount), volume.os));
            }
        }
    }
    for source in &options.sources {
        let root = Path::new(&source.path);
        if platform::os_at(root) != Os::Unknown {
            mounts.push(root.into());
        }
        sources.extend(sources_at(root, source.os));
    }
    // Epic installed in a Steam shortcut's Proton prefix.
    for library in &report.libraries {
        for prefix in children(&Path::new(&library.path).join("compatdata")) {
            wine_at(&prefix, library.os, &mut sources);
        }
    }
    for prefix in prefixes {
        let start = sources.len();
        wine_at(&prefix.path, Os::Linux, &mut sources);
        for source in &mut sources[start..] {
            source.local = prefix.local;
        }
    }
    let mut seen = BTreeSet::new();
    sources.retain(|s| seen.insert((s.path.clone(), s.kind.clone())));
    let (mut epic, warnings) = scan_sources(&sources, &mounts, &options.mappings, online, previous);
    epic.legendary_available = epic_online::binary().is_some();
    (epic, warnings)
}

fn json(path: &Path) -> Result<Value, String> {
    let mut bytes = String::new();
    fs::File::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .take(32 * 1024 * 1024 + 1)
        .read_to_string(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(format!("{} is too large to scan", path.display()));
    }
    serde_json::from_str(bytes.trim_start_matches('\u{feff}')).map_err(|_| {
        format!(
            "Cannot parse Epic metadata {}. Previous records were kept.",
            path.display()
        )
    })
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str().filter(|s| !s.trim().is_empty())
}

pub(crate) fn is_game(value: &Value) -> bool {
    let meta = value.get("metadata").unwrap_or(value);
    if value.get("is_dlc").and_then(Value::as_bool) == Some(true)
        || value.pointer("/install/is_dlc").and_then(Value::as_bool) == Some(true)
        || meta.get("mainGameItem").is_some_and(|v| !v.is_null())
        || text(meta, "namespace") == Some("ue")
        || text(value, "namespace") == Some("ue")
        || text(value, "CatalogNamespace") == Some("ue")
        || text(value, "NamespaceId") == Some("ue")
        || value.get("bIsExecutable").and_then(Value::as_bool) == Some(false)
    {
        return false;
    }
    if let (Some(app), Some(main)) = (text(value, "AppName"), text(value, "MainGameAppName"))
        && app != main
    {
        return false;
    }
    let category_excluded = meta
        .get("categories")
        .and_then(Value::as_array)
        .is_some_and(|categories| {
            categories.iter().filter_map(|c| text(c, "path")).any(|c| {
                [
                    "assets",
                    "asset-format",
                    "plugins",
                    "projects",
                    "mods",
                    "addons",
                    "dlc",
                    "currency",
                    "virtualcurrency",
                    "consumables",
                    "soundtrack",
                    "digitalextras",
                ]
                .iter()
                .any(|kind| {
                    c.eq_ignore_ascii_case(kind)
                        || c.to_ascii_lowercase().starts_with(&format!("{kind}/"))
                })
            })
        });
    !category_excluded
}

pub fn parse_library(value: &Value) -> Result<Vec<EpicTitle>, String> {
    let rows = value
        .as_array()
        .or_else(|| value.get("library").and_then(Value::as_array))
        .ok_or("Expected a Legendary JSON list or Heroic library array")?;
    let mut games = BTreeMap::new();
    for row in rows {
        if !is_game(row) {
            continue;
        }
        let id = text(row, "app_name").ok_or("Epic library record has no app_name")?;
        let title = text(row, "app_title")
            .or_else(|| text(row, "title"))
            .or_else(|| row.get("metadata").and_then(|m| text(m, "title")))
            .unwrap_or(id);
        games.insert(
            id.to_string(),
            EpicTitle {
                app_name: id.into(),
                title: title.into(),
            },
        );
    }
    Ok(games.into_values().collect())
}

fn resolve_path(
    raw: &str,
    source: &EpicSource,
    mounts: &[PathBuf],
    mappings: &[PathMapping],
) -> Result<PathBuf, String> {
    let normalized = raw.replace('\\', "/");
    if normalized.split('/').any(|s| s == "..") {
        return Err("Unsafe parent path in Epic install record".into());
    }
    let mut mappings = mappings.to_vec();
    if let Some(drive) = &source.drive_c {
        // Explicit user mappings take precedence over this source's C: anchor.
        if !mappings.iter().any(|m| {
            m.original
                .trim_end_matches(['/', '\\'])
                .eq_ignore_ascii_case("C:")
        }) {
            mappings.push(PathMapping {
                original: "C:".into(),
                mounted: drive.clone(),
            });
        }
    }
    let windows = normalized.as_bytes().get(1) == Some(&b':');
    if !windows && !normalized.starts_with('/') {
        return Err("Epic install record contains a relative path".into());
    }
    // resolve_library implements prefix mapping but its automatic lookup is
    // Steam-specific. Reuse it for mappings/native paths, then match game dirs.
    let path_os = if windows { Os::Windows } else { source.os };
    let mapped = mappings.iter().any(|m| {
        let prefix = m
            .original
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_owned();
        !prefix.is_empty()
            && if windows {
                normalized.eq_ignore_ascii_case(&prefix)
                    || normalized
                        .to_lowercase()
                        .starts_with(&(prefix.to_lowercase() + "/"))
            } else {
                normalized == prefix || normalized.starts_with(&(prefix + "/"))
            }
    });
    if (source.local || mapped)
        && let Ok(path) = steam::resolve_library(raw, path_os, &[], &mappings)
    {
        return Ok(path);
    }
    let suffix = if windows {
        &normalized[2..]
    } else {
        &normalized
    };
    let candidates: BTreeMap<_, _> = mounts
        .iter()
        .map(|m| m.join(suffix.trim_start_matches('/')))
        .filter(|p| p.is_dir())
        .map(|p| (key(&p), p))
        .collect();
    if candidates.len() == 1 {
        return Ok(candidates.into_values().next().unwrap());
    }
    Err(format!(
        "Cannot uniquely resolve Epic installation {raw}. Mount its volume and add an explicit path mapping."
    ))
}

fn game<'a>(games: &'a mut BTreeMap<String, EpicGame>, id: &str, title: &str) -> &'a mut EpicGame {
    let game = games.entry(id.into()).or_insert_with(|| EpicGame {
        app_name: id.into(),
        title: title.into(),
        library_sources: vec![],
        installations: vec![],
    });
    if game.title == id && title != id {
        game.title = title.into();
    }
    game
}

#[allow(clippy::too_many_arguments)]
fn install(
    row: &Value,
    source: &EpicSource,
    mounts: &[PathBuf],
    mappings: &[PathMapping],
    games: &mut BTreeMap<String, EpicGame>,
    now: u64,
    warnings: &mut Vec<String>,
    fallback: Option<&str>,
) -> Result<(), String> {
    if !is_game(row) {
        return Ok(());
    }
    let id = text(row, "AppName")
        .or_else(|| text(row, "app_name"))
        .or(fallback)
        .ok_or("Epic installation has no AppName")?;
    let title = text(row, "DisplayName")
        .or_else(|| text(row, "title"))
        .unwrap_or(id);
    let raw = text(row, "InstallLocation")
        .or_else(|| text(row, "install_path"))
        .ok_or_else(|| format!("Epic {id} has no installation path"))?;
    let (path, state) = match resolve_path(raw, source, mounts, mappings) {
        Ok(path) => {
            let incomplete = [
                "bIsIncompleteInstall",
                "bNeedsValidation",
                "needs_verification",
                "is_preloaded",
            ]
            .iter()
            .any(|k| row.get(k).and_then(Value::as_bool) == Some(true));
            let state = match fs::metadata(&path) {
                Ok(m) if m.is_dir() => {
                    if incomplete {
                        "incomplete"
                    } else {
                        "installed"
                    }
                }
                Ok(_) => "files_missing",
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "files_missing",
                Err(_) => "unavailable",
            };
            (path.to_string_lossy().into_owned(), state)
        }
        Err(e) => {
            warnings.push(e);
            (raw.into(), "unavailable")
        }
    };
    game(games, id, title).installations.push(Installation {
        library: source.path.clone(),
        path,
        os: source.os,
        state: state.into(),
        last_seen: now,
    });
    Ok(())
}

pub fn scan_sources(
    sources: &[EpicSource],
    mounts: &[PathBuf],
    mappings: &[PathMapping],
    online: &[EpicOnlineLibrary],
    previous: Option<&EpicReport>,
) -> (EpicReport, Vec<String>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut games = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut verified_installs = BTreeSet::new();
    let mut verified_libraries = BTreeSet::new();
    for source in sources {
        let result = read_source(
            source,
            mounts,
            mappings,
            &mut games,
            now,
            &mut warnings,
            &mut verified_installs,
            &mut verified_libraries,
        );
        if let Err(e) = result {
            warnings.push(format!(
                "Epic {}: {e}. Previously seen records are retained.",
                source.path
            ));
        }
    }
    // An explicit online snapshot supersedes that profile's metadata cache for
    // library membership (including games removed from that account).
    for library in online {
        let assets_updated = fs::metadata(Path::new(&library.profile).join("assets.json"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .is_some_and(|t| t.as_secs() > library.checked_at);
        if !assets_updated
            && sources
                .iter()
                .any(|s| s.kind == "legendary" && s.path == library.profile)
        {
            let mut superseded = BTreeSet::from([library.profile.clone()]);
            for source in sources.iter().filter(|s| s.kind == "heroic_cache") {
                if let Some(root) = Path::new(&source.path).parent().and_then(Path::parent)
                    && key(&root.join("legendaryConfig/legendary")) == library.profile
                {
                    superseded.insert(source.path.clone());
                }
            }
            for game in games.values_mut() {
                game.library_sources.retain(|p| !superseded.contains(p));
            }
            verified_libraries.extend(superseded);
            for title in &library.games {
                game(&mut games, &title.app_name, &title.title)
                    .library_sources
                    .push(library.profile.clone());
            }
        }
    }
    if let Some(previous) = previous {
        for old in &previous.games {
            for source in &old.library_sources {
                if !verified_libraries.contains(source) {
                    game(&mut games, &old.app_name, &old.title)
                        .library_sources
                        .push(source.clone());
                }
            }
            for installation in &old.installations {
                if verified_installs.contains(&installation.library) {
                    continue;
                }
                let game = game(&mut games, &old.app_name, &old.title);
                if game
                    .installations
                    .iter()
                    .any(|i| i.library == installation.library)
                {
                    continue;
                }
                let mut old = installation.clone();
                old.state = "unavailable".into();
                game.installations.push(old);
            }
        }
    }
    let mut games: Vec<_> = games
        .into_values()
        .filter(|g: &EpicGame| !g.installations.is_empty() || !g.library_sources.is_empty())
        .collect();
    for game in &mut games {
        game.library_sources.sort();
        game.library_sources.dedup();
    }
    games.sort_by_cached_key(|g| (g.title.to_lowercase(), g.app_name.clone()));
    (
        EpicReport {
            games,
            sources: sources.to_vec(),
            legendary_available: false,
        },
        warnings,
    )
}

#[allow(clippy::too_many_arguments)]
fn read_source(
    source: &EpicSource,
    mounts: &[PathBuf],
    mappings: &[PathMapping],
    games: &mut BTreeMap<String, EpicGame>,
    now: u64,
    warnings: &mut Vec<String>,
    verified_installs: &mut BTreeSet<String>,
    verified_libraries: &mut BTreeSet<String>,
) -> Result<(), String> {
    let path = Path::new(&source.path);
    match source.kind.as_str() {
        "heroic_cache" | "legendary_export" => {
            let titles = parse_library(&json(path)?)?;
            for title in titles {
                game(games, &title.app_name, &title.title)
                    .library_sources
                    .push(source.path.clone());
            }
            verified_libraries.insert(source.path.clone());
        }
        "legendary" => {
            let installed = path.join("installed.json");
            if installed.exists() {
                let value = json(&installed)?;
                let records = value
                    .as_object()
                    .ok_or("installed.json must be an object")?;
                let mut complete = true;
                for (id, row) in records {
                    if let Err(e) = install(
                        row,
                        source,
                        mounts,
                        mappings,
                        games,
                        now,
                        warnings,
                        Some(id),
                    ) {
                        complete = false;
                        warnings.push(format!("{}: {e}", installed.display()));
                    }
                }
                if complete {
                    verified_installs.insert(source.path.clone());
                }
            }
            // assets.json enumerates the cached account's app IDs. Metadata by
            // itself can outlive logout/account changes and proves no ownership.
            let assets = path.join("assets.json");
            if assets.exists() {
                let value = json(&assets)?;
                let platforms = value
                    .as_object()
                    .ok_or("assets.json must be a platform-to-array object")?;
                let mut ids = BTreeSet::new();
                for rows in platforms.values() {
                    for row in rows
                        .as_array()
                        .ok_or("Epic assets platform must be an array")?
                    {
                        if text(row, "namespace") == Some("ue") {
                            continue;
                        }
                        let id = text(row, "app_name").ok_or("Epic asset has no app_name")?;
                        if id.contains(['/', '\\']) || id == ".." {
                            return Err("Unsafe Epic app_name".into());
                        }
                        ids.insert(id);
                    }
                }
                for id in ids {
                    let file = path.join("metadata").join(format!("{id}.json"));
                    let meta = if file.exists() {
                        Some(json(&file)?)
                    } else {
                        None
                    };
                    if meta.as_ref().is_some_and(|m| !is_game(m)) {
                        continue;
                    }
                    let title = meta
                        .as_ref()
                        .and_then(|m| text(m, "app_title"))
                        .unwrap_or(id);
                    game(games, id, title)
                        .library_sources
                        .push(source.path.clone());
                }
                verified_libraries.insert(source.path.clone());
            }
        }
        "egl_manifests" => {
            let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
            let mut complete = true;
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.path().extension().is_none_or(|e| e != "item") {
                    continue;
                }
                let result = json(&entry.path()).and_then(|row| {
                    install(&row, source, mounts, mappings, games, now, warnings, None)
                });
                if let Err(e) = result {
                    complete = false;
                    warnings.push(format!("{}: {e}", entry.path().display()));
                }
            }
            if complete {
                verified_installs.insert(source.path.clone());
            }
        }
        "egl_item" => {
            install(
                &json(path)?,
                source,
                mounts,
                mappings,
                games,
                now,
                warnings,
                None,
            )?;
            verified_installs.insert(source.path.clone());
        }
        "heroic_external" => {
            let value = json(path)?;
            let rows = value
                .as_array()
                .ok_or("third-party-installed.json must be an array")?;
            for row in rows {
                let id = row
                    .get(0)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or("External-client install has no app_name")?;
                if id.contains(['/', '\\']) || id == ".." {
                    return Err("Unsafe Epic app_name".into());
                }
                let metadata = path
                    .parent()
                    .unwrap()
                    .join("metadata")
                    .join(format!("{id}.json"));
                let meta = if metadata.is_file() {
                    Some(json(&metadata)?)
                } else {
                    None
                };
                if meta.as_ref().is_some_and(|m| !is_game(m)) {
                    continue;
                }
                let title = meta
                    .as_ref()
                    .and_then(|m| text(m, "app_title"))
                    .unwrap_or(id);
                game(games, id, title).installations.push(Installation {
                    library: source.path.clone(),
                    path: String::new(),
                    os: source.os,
                    state: "external".into(),
                    last_seen: now,
                });
            }
            verified_installs.insert(source.path.clone());
        }
        "egl_dat" => {
            let value = json(path)?;
            let rows = value
                .get("InstallationList")
                .and_then(Value::as_array)
                .ok_or("LauncherInstalled.dat has no InstallationList")?;
            let mut complete = true;
            for row in rows {
                // This index is a fallback. A detailed .item record for the same
                // installation has the title and incomplete/validation state.
                if let (Some(id), Some(raw)) = (text(row, "AppName"), text(row, "InstallLocation"))
                    && let Ok(path) = resolve_path(raw, source, mounts, mappings)
                    && games.get(id).is_some_and(|g| {
                        g.installations
                            .iter()
                            .any(|i| i.path == path.to_string_lossy())
                    })
                {
                    continue;
                }
                if let Err(e) = install(row, source, mounts, mappings, games, now, warnings, None) {
                    complete = false;
                    warnings.push(e);
                }
            }
            if complete {
                verified_installs.insert(source.path.clone());
            }
        }
        _ => return Err("Unknown Epic source format".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn write(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn source(path: &Path, kind: &str, os: Os) -> EpicSource {
        EpicSource {
            path: key(path),
            kind: kind.into(),
            os,
            local: true,
            drive_c: None,
        }
    }
    fn run(sources: &[EpicSource], previous: Option<&EpicReport>) -> (EpicReport, Vec<String>) {
        scan_sources(sources, &[], &[], &[], previous)
    }
    #[test]
    fn currency_and_extras_are_filtered_by_category_not_game_title() {
        for category in [
            "addons",
            "addons/currency",
            "currency",
            "soundtrack",
            "digitalextras",
        ] {
            assert!(!is_game(
                &json!({"metadata":{"categories":[{"path":category}]}})
            ));
        }
        assert!(is_game(
            &json!({"app_title":"Coin","metadata":{"categories":[{"path":"games/edition"}]}})
        ));
    }
    #[test]
    fn official_manifests_and_fallback_index_keep_install_state_without_claiming_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let manifests = dir
            .path()
            .join("ProgramData/Epic/EpicGamesLauncher/Data/Manifests");
        fs::create_dir_all(dir.path().join("Games/Example")).unwrap();
        write(
            &manifests.join("a.item"),
            json!({"AppName":"opaque-id", "DisplayName":"Epic example", "InstallLocation":"C:\\Games\\Example", "bIsIncompleteInstall":true}),
        );
        write(
            &manifests.join("ue.item"),
            json!({"AppName":"UE_5", "CatalogNamespace":"ue"}),
        );
        write(
            &manifests.join("dlc.item"),
            json!({"AppName":"extra", "MainGameAppName":"opaque-id"}),
        );
        write(
            &dir.path()
                .join("ProgramData/Epic/UnrealEngineLauncher/LauncherInstalled.dat"),
            json!({"InstallationList":[{"AppName":"opaque-id", "InstallLocation":"C:\\Games\\Example"}]}),
        );
        let sources = sources_at(dir.path(), Os::Windows);
        let (report, warnings) = run(&sources, None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
        let game = &report.games[0];
        assert_eq!(game.title, "Epic example");
        assert!(game.library_sources.is_empty());
        assert_eq!(game.installations.len(), 1);
        assert_eq!(game.installations[0].state, "incomplete");
        assert_eq!(game.installations[0].os, Os::Windows);
    }
    #[test]
    fn legendary_assets_add_uninstalled_games_but_metadata_alone_does_not_prove_membership() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("assets.json"),
            json!({"Windows":[{"app_name":"base"},{"app_name":"uninstalled"},{"app_name":"dlc"}], "Mac":[{"app_name":"base"}]}),
        );
        write(
            &dir.path().join("metadata/base.json"),
            json!({"app_name":"base", "app_title":"日本語 game"}),
        );
        write(
            &dir.path().join("metadata/dlc.json"),
            json!({"metadata":{"mainGameItem":{"id":"base"}}}),
        );
        write(
            &dir.path().join("metadata/stale.json"),
            json!({"app_name":"stale", "app_title":"Old account"}),
        );
        fs::create_dir_all(dir.path().join("payload")).unwrap();
        write(
            &dir.path().join("installed.json"),
            json!({"base":{"title":"日本語 game","install_path":dir.path().join("payload"),"platform":"Windows"}}),
        );
        let (report, warnings) = run(&[source(dir.path(), "legendary", Os::Linux)], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 2);
        let game = report.games.iter().find(|g| g.app_name == "base").unwrap();
        assert_eq!(game.library_sources.len(), 1);
        assert_eq!(game.installations[0].os, Os::Linux); // Wine payload, Linux client.
        assert_eq!(game.installations[0].state, "installed");
        assert!(
            report
                .games
                .iter()
                .any(|g| g.app_name == "uninstalled" && g.installations.is_empty())
        );
    }
    #[test]
    fn detects_current_flatpak_heroic_legacy_legendary_and_wine_bottles_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home/test");
        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(dir.path().join("etc/os-release"), "ID=fixture").unwrap();
        for suffix in [
            ".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/installed.json",
            ".var/app/com.heroicgameslauncher.hgl/config/legendary/installed.json",
            ".local/share/bottles/bottles/epic/drive_c/ProgramData/Epic/EpicGamesLauncher/Data/Manifests/test.item",
        ] {
            write(&home.join(suffix), json!({}));
        }
        let sources = sources_at(dir.path(), Os::Unknown);
        assert_eq!(sources.iter().filter(|s| s.kind == "legendary").count(), 2);
        assert!(
            sources
                .iter()
                .any(|s| s.kind == "egl_manifests" && s.os == Os::Linux && s.drive_c.is_some())
        );
        assert!(sources.iter().all(|s| !s.local));
    }
    #[test]
    fn heroic_cache_and_legendary_exports_share_ids_and_filter_dlc_and_unreal_content() {
        let rows = json!([
            {"app_name":"one", "title":"Same title"}, {"app_name":"two", "app_title":"Same title"},
            {"app_name":"extra", "install":{"is_dlc":true}},
            {"app_name":"ue", "metadata":{"namespace":"ue"}},
            {"app_name":"asset", "metadata":{"categories":[{"path":"assets"}]}}
        ]);
        assert_eq!(parse_library(&rows).unwrap().len(), 2);
        assert_eq!(parse_library(&json!({"library":rows})).unwrap().len(), 2);
        assert!(parse_library(&json!({"library":"corrupt"})).is_err());
    }
    #[test]
    fn missing_and_corrupt_sources_retain_installations_but_verified_empty_records_remove_them() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("installed.json"),
            json!({"id":{"title":"Example", "install_path":dir.path().join("missing")}}),
        );
        let sources = vec![source(dir.path(), "legendary", Os::host())];
        let (first, _) = run(&sources, None);
        assert_eq!(first.games[0].installations[0].state, "files_missing");
        fs::write(dir.path().join("installed.json"), "{").unwrap();
        let (corrupt, warnings) = run(&sources, Some(&first));
        assert!(!warnings.is_empty());
        assert_eq!(corrupt.games[0].installations[0].state, "unavailable");
        let (missing, _) = run(&[], Some(&first));
        assert_eq!(missing.games[0].installations[0].state, "unavailable");
        write(&dir.path().join("installed.json"), json!({}));
        assert!(run(&sources, Some(&first)).0.games.is_empty());
    }
    #[test]
    fn foreign_paths_require_unique_mounted_locations_or_explicit_mappings() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            fs::create_dir_all(root.join("Games/Example")).unwrap();
        }
        let mut source = source(a.path(), "legendary", Os::Windows);
        source.local = false;
        let mounts = vec![a.path().into(), b.path().into()];
        assert!(resolve_path("D:\\Games\\Example", &source, &mounts, &[]).is_err());
        let mappings = [PathMapping {
            original: "D:".into(),
            mounted: a.path().to_string_lossy().into(),
        }];
        assert_eq!(
            resolve_path("D:\\Games\\Example", &source, &mounts, &mappings).unwrap(),
            a.path().join("Games/Example")
        );
        assert!(resolve_path("D:\\..\\Example", &source, &mounts, &mappings).is_err());
        source.os = Os::Linux;
        assert!(resolve_path("/home/someone/Games/Example", &source, &[], &[]).is_err());
    }
    #[test]
    fn two_operating_systems_merge_by_epic_id_and_keep_separate_installations() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            write(
                &root.join("installed.json"),
                json!({"id":{"title":"Example", "install_path":root.join("game")}}),
            );
        }
        let (report, _) = run(
            &[
                source(a.path(), "legendary", Os::Linux),
                source(b.path(), "legendary", Os::Windows),
            ],
            None,
        );
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].installations.len(), 2);
    }
    #[test]
    fn online_snapshot_can_confirm_empty_membership_and_preserves_installed_records() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("assets.json"),
            json!({"Windows":[{"app_name":"old"}]}),
        );
        write(
            &dir.path().join("installed.json"),
            json!({"old":{"title":"Example", "install_path":dir.path().join("game")}}),
        );
        let source = source(dir.path(), "legendary", Os::host());
        let online = EpicOnlineLibrary {
            profile: source.path.clone(),
            checked_at: u64::MAX,
            games: vec![],
        };
        let (report, _) = scan_sources(&[source], &[], &[], &[online], None);
        assert_eq!(report.games.len(), 1);
        assert!(report.games[0].library_sources.is_empty());
    }
    #[test]
    fn earlier_steam_only_inventory_loads_with_empty_epic_section() {
        let value = json!({"host_os":"linux","games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null});
        let report: Report = serde_json::from_value(value).unwrap();
        assert!(report.epic.games.is_empty());
    }
    #[test]
    fn successful_online_refresh_supersedes_the_same_heroic_profiles_stale_library_cache() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("legendaryConfig/legendary");
        let cache = dir.path().join("store_cache/legendary_library.json");
        write(
            &profile.join("assets.json"),
            json!({"Windows":[{"app_name":"old"}]}),
        );
        write(
            &cache,
            json!({"library":[{"app_name":"old","title":"Old"}]}),
        );
        let sources = [
            source(&profile, "legendary", Os::host()),
            source(&cache, "heroic_cache", Os::host()),
        ];
        let online = EpicOnlineLibrary {
            profile: key(&profile),
            checked_at: u64::MAX,
            games: vec![EpicTitle {
                app_name: "new".into(),
                title: "New".into(),
            }],
        };
        let (report, warnings) = scan_sources(&sources, &[], &[], &[online], None);
        assert!(warnings.is_empty());
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].app_name, "new");
    }
    #[test]
    fn heroic_external_markers_do_not_invent_payload_paths_or_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("third-party-installed.json");
        write(&file, json!([["external", "Windows"]]));
        write(
            &dir.path().join("metadata/external.json"),
            json!({"app_title":"EA example"}),
        );
        let sources = [source(&file, "heroic_external", Os::Linux)];
        let (report, warnings) = run(&sources, None);
        assert!(warnings.is_empty());
        assert_eq!(report.games[0].title, "EA example");
        assert!(report.games[0].library_sources.is_empty());
        assert!(report.games[0].installations[0].path.is_empty());
        assert_eq!(report.games[0].installations[0].state, "external");
        write(&file, json!([]));
        assert!(run(&sources, Some(&report)).0.games.is_empty());
    }
    #[test]
    fn fallback_install_index_works_without_item_manifests_and_handles_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("LauncherInstalled.dat");
        fs::create_dir_all(dir.path().join("game")).unwrap();
        let mut source = source(&file, "egl_dat", Os::Windows);
        source.drive_c = Some(dir.path().to_string_lossy().into());
        let value = json!({"InstallationList":[{"AppName":"id","InstallLocation":"C:\\game"},{"AppName":"UE_5","NamespaceId":"ue"}]});
        fs::write(&file, format!("\u{feff}{value}")).unwrap();
        let (report, warnings) = run(&[source], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].installations[0].state, "installed");
    }
}
