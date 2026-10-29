//! EA/Origin install manifests and cached Lutris library, without client auth.
use crate::{epic, model::*, platform};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeSet,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
struct Context {
    os: Os,
    local: bool,
    drive_c: Option<PathBuf>,
    home: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct Source {
    path: PathBuf,
    kind: &'static str,
    context: Context,
}

#[derive(Default)]
struct Discovery {
    sources: Vec<Source>,
    seen: BTreeSet<(String, &'static str)>,
    prefixes: BTreeSet<String>,
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

impl Discovery {
    fn add(&mut self, path: PathBuf, kind: &'static str, context: &Context) {
        if path.exists() && self.seen.insert((epic::key(&path), kind)) {
            self.sources.push(Source {
                path,
                kind,
                context: context.clone(),
            });
        }
    }
    fn windows(&mut self, root: &Path, context: &Context) {
        for suffix in [
            "Program Files/EA Games",
            "Program Files (x86)/EA Games",
            "Program Files/Origin Games",
            "Program Files (x86)/Origin Games",
            "EA Games",
            "Origin Games",
            "Games/EA Games",
            "Games/Origin Games",
        ] {
            self.add(root.join(suffix), "library", context);
        }
        self.add(
            root.join("ProgramData/EA Desktop/machine.ini"),
            "ini",
            context,
        );
        self.add(
            root.join("ProgramData/Origin/local.xml"),
            "origin_config",
            context,
        );
        for user in children(&root.join("users"))
            .into_iter()
            .chain(children(&root.join("Users")))
        {
            let folder = user.join("AppData/Local/Electronic Arts/EA Desktop");
            for entry in fs::read_dir(folder)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .take(256)
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("user_") && name.ends_with(".ini") {
                    self.add(entry.path(), "ini", context);
                }
            }
        }
    }
    fn wine(&mut self, prefix: &Path, context: &Context) {
        for suffix in ["drive_c", "pfx/drive_c"] {
            let drive = prefix.join(suffix);
            if !drive.is_dir() || !self.prefixes.insert(epic::key(&drive)) {
                continue;
            }
            let mut context = context.clone();
            // A Wine/Proton prefix belongs to Linux even when discovered from
            // Windows or a separate home partition with no OS-release marker.
            context.os = Os::Linux;
            context.drive_c = Some(drive.clone());
            self.windows(&drive, &context);
            self.add(
                drive.parent().unwrap().join("system.reg"),
                "wine_registry",
                &context,
            );
        }
    }
    fn heroic(&mut self, root: &Path, context: &Context) {
        self.add(root.join("GamesConfig"), "heroic_configs", context);
    }
    fn home(&mut self, home: &Path, context: &Context) {
        let mut context = context.clone();
        context.home = Some(home.into());
        for suffix in [".wine", "Games/ea-app", "Games/ea-desktop", "Games/origin"] {
            self.wine(&home.join(suffix), &context);
        }
        for suffix in [
            "Games",
            "Games/Heroic/Prefixes",
            "Games/Heroic/Prefixes/default",
            ".local/share/bottles/bottles",
            ".var/app/com.usebottles.bottles/data/bottles/bottles",
        ] {
            for prefix in children(&home.join(suffix)) {
                self.wine(&prefix, &context);
            }
        }
        for suffix in [
            ".config/heroic",
            "AppData/Roaming/heroic",
            ".var/app/com.heroicgameslauncher.hgl/config/heroic",
        ] {
            self.heroic(&home.join(suffix), &context);
        }
        for suffix in [
            ".local/share/lutris/pga.db",
            ".var/app/net.lutris.Lutris/data/lutris/pga.db",
        ] {
            self.add(home.join(suffix), "lutris", &context);
        }
    }
    fn root(&mut self, root: &Path, hint: Os) {
        let os = match platform::os_at(root) {
            Os::Unknown => hint,
            os => os,
        };
        let context = Context {
            os,
            local: false,
            drive_c: Some(root.into()),
            home: None,
        };
        self.windows(root, &context);
        for directory in ["home", "var/home", "@home", "@/home", "@/var/home", "Users"] {
            let parent = root.join(directory);
            if parent.is_symlink() {
                continue;
            }
            for home in children(&parent) {
                let user_os = if directory == "Users" {
                    Os::Windows
                } else {
                    Os::Linux
                };
                self.home(
                    &home,
                    &Context {
                        os: user_os,
                        drive_c: None,
                        ..context.clone()
                    },
                );
            }
        }
        if !root.join("etc").exists() && !root.join("Windows").exists() {
            for home in children(root) {
                self.home(
                    &home,
                    &Context {
                        drive_c: None,
                        ..context.clone()
                    },
                );
            }
        }
    }
}

pub fn scan(
    report: &Report,
    previous: Option<&EaReport>,
    prefixes: &[crate::discovery::Location],
) -> (EaReport, Vec<String>) {
    let context = Context {
        os: Os::host(),
        local: true,
        drive_c: None,
        home: platform::home(),
    };
    let mut discovery = Discovery::default();
    if let Some(home) = &context.home {
        discovery.home(home, &context);
    }
    if let Some(path) = env::var_os("WINEPREFIX") {
        discovery.wine(Path::new(&path), &context);
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        discovery.heroic(&PathBuf::from(path).join("heroic"), &context);
    }
    if let Some(path) = env::var_os("APPDATA") {
        discovery.heroic(&PathBuf::from(path).join("heroic"), &context);
    }
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        discovery.add(
            PathBuf::from(path).join("lutris/pga.db"),
            "lutris",
            &context,
        );
    }
    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(path) = env::var_os(name) {
            for folder in ["EA Games", "Origin Games"] {
                discovery.add(PathBuf::from(&path).join(folder), "library", &context);
            }
        }
    }
    if let Some(path) = env::var_os("PROGRAMDATA") {
        discovery.add(
            PathBuf::from(path).join("EA Desktop/machine.ini"),
            "ini",
            &context,
        );
    }
    #[cfg(windows)]
    for path in registry_paths() {
        discovery.add(path, "manifest", &context);
    }
    let mounts: Vec<_> = report
        .volumes
        .iter()
        .flat_map(|v| &v.mountpoints)
        .map(PathBuf::from)
        .filter(|p| platform::scan_mount(p))
        .collect();
    for volume in &report.volumes {
        for point in &volume.mountpoints {
            if platform::scan_mount(Path::new(point)) {
                discovery.root(Path::new(point), volume.os);
            }
        }
    }
    for library in &report.libraries {
        for prefix in children(&Path::new(&library.path).join("compatdata")) {
            discovery.wine(
                &prefix,
                &Context {
                    os: library.os,
                    ..context.clone()
                },
            );
        }
    }
    for prefix in prefixes {
        discovery.wine(
            &prefix.path,
            &Context {
                os: Os::Linux,
                local: prefix.local,
                drive_c: prefix.drive_c.clone(),
                home: None,
            },
        );
    }
    scan_sources(discovery, &mounts, previous)
}

fn read_text(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("EA metadata is too large".into());
    }
    let big_endian = bytes.starts_with(&[0xfe, 0xff]) || bytes.starts_with(b"\0<\0?");
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(b"<\0?\0") || big_endian {
        let start = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
            2
        } else {
            0
        };
        if (bytes.len() - start) % 2 != 0 {
            return Err("Invalid UTF-16 EA metadata".into());
        }
        let units: Vec<_> = bytes[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                if big_endian {
                    u16::from_be_bytes([b[0], b[1]])
                } else {
                    u16::from_le_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| "Invalid UTF-16 EA metadata".into())
    } else {
        String::from_utf8(bytes)
            .map(|s| s.trim_start_matches('\u{feff}').into())
            .map_err(|_| "Invalid UTF-8 EA metadata".into())
    }
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: 100_000,
            ..Default::default()
        },
    )
    .map_err(|_| "Invalid EA XML metadata".into())
}

fn manifest(path: &Path) -> Result<(Vec<String>, String), String> {
    let text = read_text(&path.join("__Installer/installerdata.xml"))?;
    let document = xml(&text)?;
    let root = document.root_element();
    if root.tag_name().name() != "DiPManifest" {
        return Err("Unknown EA install manifest format".into());
    }
    let mut ids: Vec<_> = root
        .children()
        .filter(|n| n.has_tag_name("contentIDs"))
        .flat_map(|n| n.children())
        .filter(|n| n.has_tag_name("contentID"))
        .filter_map(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("EA manifest has no content IDs".into());
    }
    let titles: Vec<_> = root
        .children()
        .filter(|n| n.has_tag_name("gameTitles"))
        .flat_map(|n| n.children())
        .filter(|n| n.has_tag_name("gameTitle"))
        .filter(|n| n.text().is_some_and(|t| !t.trim().is_empty()))
        .collect();
    let title = titles
        .iter()
        .find(|n| {
            n.attribute("locale")
                .is_some_and(|l| l.replace('-', "_").eq_ignore_ascii_case("en_US"))
        })
        .or_else(|| titles.first())
        .and_then(|n| n.text())
        .map(str::trim)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        });
    Ok((ids, title))
}

fn resolve(raw: &str, context: &Context, mounts: &[PathBuf]) -> Result<PathBuf, String> {
    let normalized = raw.trim_matches('"').replace('\\', "/");
    if normalized.split('/').any(|s| s == "..") {
        return Err("Unsafe EA metadata path".into());
    }
    if let Some(suffix) = normalized.strip_prefix("~/") {
        return context
            .home
            .as_ref()
            .map(|h| h.join(suffix))
            .ok_or("Cannot resolve EA home path".into());
    }
    let windows = normalized.as_bytes().get(1) == Some(&b':');
    if windows {
        if let Some(drive) = &context.drive_c {
            if normalized[..2].eq_ignore_ascii_case("C:") {
                return Ok(drive.join(normalized[2..].trim_start_matches('/')));
            }
            if context.local {
                let letter = format!("{}:", normalized[..1].to_ascii_lowercase());
                if let Some(prefix) = drive.parent() {
                    let link = prefix.join("dosdevices").join(letter);
                    if let Ok(target) = link.canonicalize() {
                        return Ok(target.join(normalized[2..].trim_start_matches('/')));
                    }
                }
            }
        }
        if context.local && Os::host() == Os::Windows {
            return Ok(PathBuf::from(raw.trim_matches('"')));
        }
    } else if normalized.starts_with('/') && context.local && Os::host() != Os::Windows {
        return Ok(PathBuf::from(&normalized));
    }
    if !windows && !normalized.starts_with('/') {
        return Err("Relative EA metadata path".into());
    }
    let suffix = if windows {
        &normalized[2..]
    } else {
        &normalized
    };
    let mut candidates: Vec<_> = mounts
        .iter()
        .map(|p| p.join(suffix.trim_start_matches('/')))
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort_by_key(|p| epic::key(p));
    candidates.dedup_by_key(|p| epic::key(p));
    if candidates.len() == 1 {
        Ok(candidates.remove(0))
    } else {
        Err("EA game path is unavailable or ambiguous across mounted volumes".into())
    }
}

fn merge(games: &mut Vec<EaGame>, mut incoming: EaGame) {
    // Merge overlapping content IDs, never display titles or another store's IDs.
    let mut changed = true;
    while changed {
        changed = false;
        for i in (0..games.len()).rev() {
            if games[i]
                .aliases
                .iter()
                .any(|id| incoming.aliases.contains(id))
            {
                let old = games.remove(i);
                incoming.aliases.extend(old.aliases);
                incoming.library_sources.extend(old.library_sources);
                incoming.installations.extend(old.installations);
                if incoming.title == incoming.id {
                    incoming.title = old.title;
                }
                changed = true;
            }
        }
    }
    incoming.aliases.sort();
    incoming.aliases.dedup();
    incoming.id = incoming.aliases[0].clone();
    incoming.library_sources.sort();
    incoming.library_sources.dedup();
    let mut seen = BTreeSet::new();
    incoming
        .installations
        .retain(|i| seen.insert((epic::key(Path::new(&i.path)), format!("{:?}", i.os))));
    games.push(incoming);
}

fn install(path: &Path, source: &Source, games: &mut Vec<EaGame>, now: u64) -> Result<(), String> {
    let (ids, title) = manifest(path)?;
    let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    let mut payload = false;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && !name.starts_with("__") {
            payload = true;
        }
    }
    merge(
        games,
        EaGame {
            id: ids[0].clone(),
            aliases: ids,
            title,
            library_sources: vec![],
            installations: vec![Installation {
                library: epic::key(&source.path),
                path: epic::key(path),
                os: source.context.os,
                state: if payload { "installed" } else { "incomplete" }.into(),
                last_seen: now,
            }],
        },
    );
    Ok(())
}

fn profile_values(value: &serde_json::Value, values: &mut Vec<String>, depth: usize) {
    if depth > 4 || values.len() >= 256 {
        return;
    }
    if let Some(object) = value.as_object() {
        if let Some(prefix) = object.get("winePrefix").and_then(serde_json::Value::as_str) {
            values.push(prefix.into());
        }
        for value in object.values() {
            profile_values(value, values, depth + 1);
        }
    }
}

fn scan_sources(
    mut discovery: Discovery,
    mounts: &[PathBuf],
    previous: Option<&EaReport>,
) -> (EaReport, Vec<String>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut games = Vec::new();
    let mut warnings = Vec::new();
    let mut verified = BTreeSet::new();
    let mut catalogs = BTreeSet::new();
    let mut index = 0;
    while index < discovery.sources.len() && index < 4096 {
        let source = discovery.sources[index].clone();
        index += 1;
        let result = read_source(
            &source,
            &mut discovery,
            mounts,
            &mut games,
            now,
            &mut verified,
            &mut catalogs,
            previous,
            &mut warnings,
        );
        if let Err(error) = result {
            warnings.push(format!(
                "EA {}: {error}. Previous installations were retained.",
                source.path.display()
            ));
        }
    }
    if index >= 4096 {
        warnings.push("EA discovery limit reached".into());
    }
    if let Some(previous) = previous {
        for old in &previous.games {
            let mut retained = old.clone();
            retained.library_sources.retain(|p| !catalogs.contains(p));
            retained.installations.retain(|i| {
                !verified.contains(&i.library)
                    && !games.iter().any(|g: &EaGame| {
                        g.installations
                            .iter()
                            .any(|current| current.path == i.path && current.os == i.os)
                    })
            });
            for install in &mut retained.installations {
                install.state = "unavailable".into();
            }
            if !retained.installations.is_empty() || !retained.library_sources.is_empty() {
                merge(&mut games, retained);
            }
        }
    }
    games.sort_by_cached_key(|g| (g.title.to_lowercase(), g.id.clone()));
    (EaReport { games }, warnings)
}

#[allow(clippy::too_many_arguments)]
fn read_source(
    source: &Source,
    discovery: &mut Discovery,
    mounts: &[PathBuf],
    games: &mut Vec<EaGame>,
    now: u64,
    verified: &mut BTreeSet<String>,
    catalogs: &mut BTreeSet<String>,
    previous: Option<&EaReport>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let context = &source.context;
    match source.kind {
        "library" => {
            let mut complete = true;
            for entry in fs::read_dir(&source.path).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if !path.is_dir() || (!context.local && path.is_symlink()) {
                    continue;
                }
                if path.join("__Installer/installerdata.xml").exists() {
                    if let Err(e) = install(&path, source, games, now) {
                        complete = false;
                        warnings.push(format!("EA {}: {e}", path.display()));
                    }
                } else if previous.is_some_and(|r| {
                    r.games
                        .iter()
                        .any(|g| g.installations.iter().any(|i| i.path == epic::key(&path)))
                }) {
                    complete = false;
                }
            }
            if complete {
                verified.insert(epic::key(&source.path));
            }
        }
        "manifest" => {
            install(&source.path, source, games, now)?;
            verified.insert(epic::key(&source.path));
        }
        "ini" => {
            for line in read_text(&source.path)?.lines() {
                if let Some((key, value)) = line.split_once('=')
                    && key.trim().eq_ignore_ascii_case("downloadinplacedir")
                {
                    let path = resolve(value.trim(), context, mounts)?;
                    discovery.add(path, "library", context);
                }
            }
        }
        "origin_config" => {
            let text = read_text(&source.path)?;
            for node in xml(&text)?.descendants().filter(|n| n.is_element()) {
                if node
                    .attribute("key")
                    .is_some_and(|s| s.eq_ignore_ascii_case("DownloadInPlaceDir"))
                    && let Some(raw) = node.attribute("value")
                {
                    discovery.add(resolve(raw, context, mounts)?, "library", context);
                }
            }
        }
        "wine_registry" => {
            let mut ea_section = false;
            for line in read_text(&source.path)?.lines() {
                if line.starts_with('[') {
                    let normalized = line.to_ascii_lowercase().replace("\\\\", "\\");
                    ea_section = [
                        "\\ea games\\",
                        "\\origin games\\",
                        "\\respawn\\",
                        "\\bioware\\",
                        "\\maxis\\",
                    ]
                    .iter()
                    .any(|p| normalized.contains(p));
                } else if ea_section
                    && let Some(raw) = line
                        .strip_prefix("\"Install Dir\"=\"")
                        .and_then(|s| s.strip_suffix('"'))
                {
                    let path = resolve(&raw.replace("\\\\", "\\"), context, mounts)?;
                    discovery.add(path, "manifest", context);
                }
            }
        }
        "heroic_configs" => {
            for entry in fs::read_dir(&source.path)
                .map_err(|e| e.to_string())?
                .take(256)
            {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_none_or(|s| s != "json") {
                    continue;
                }
                let value = serde_json::from_str(&read_text(&path)?)
                    .map_err(|_| "Invalid Heroic game configuration")?;
                let mut values = Vec::new();
                profile_values(&value, &mut values, 0);
                for raw in values {
                    discovery.wine(&resolve(&raw, context, mounts)?, context);
                }
            }
        }
        "lutris" => {
            let (_snapshot, database) = snapshot_db(&source.path)?;
            if has_table(&database, "service_games")? {
                let mut statement = database.prepare("SELECT CAST(appid AS TEXT), name FROM service_games WHERE service IN ('ea_app', 'origin') LIMIT 20001").map_err(|e| e.to_string())?;
                let rows = statement
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?;
                let mut count = 0;
                for row in rows {
                    count += 1;
                    if count > 20000 {
                        return Err("EA cached library is too large".into());
                    }
                    let (Some(id), Some(title)) = row.map_err(|e| e.to_string())? else {
                        return Err("EA cached library has incomplete records".into());
                    };
                    if id.trim().is_empty() || title.trim().is_empty() {
                        return Err("EA cached library has empty records".into());
                    }
                    merge(
                        games,
                        EaGame {
                            id: id.clone(),
                            aliases: vec![id],
                            title,
                            library_sources: vec![epic::key(&source.path)],
                            installations: vec![],
                        },
                    );
                }
                catalogs.insert(epic::key(&source.path));
            }
            if has_table(&database, "games")? {
                let mut statement = database.prepare("SELECT directory FROM games WHERE service IN ('ea_app','origin') OR slug IN ('ea-app','origin')").map_err(|e| e.to_string())?;
                let rows = statement
                    .query_map([], |r| r.get::<_, Option<String>>(0))
                    .map_err(|e| e.to_string())?;
                for row in rows.take(256) {
                    let Some(directory) = row.map_err(|e| e.to_string())? else {
                        continue;
                    };
                    let normalized = directory.replace('\\', "/");
                    let prefix = normalized.split("/drive_c").next().unwrap_or(&normalized);
                    discovery.wine(&resolve(prefix, context, mounts)?, context);
                }
            }
        }
        _ => return Err("Unknown EA metadata source".into()),
    }
    Ok(())
}

fn has_table(db: &Connection, table: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Copy DB/journal files before opening SQLite: never create SHM files or write
/// into a user's live/foreign profile. Reject a snapshot if its files changed.
pub(crate) fn snapshot_db(path: &Path) -> Result<(tempfile::TempDir, Connection), String> {
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut before = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let from = PathBuf::from(format!("{}{suffix}", path.display()));
        let metadata = match fs::metadata(&from) {
            Ok(m) => Some(m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => None,
            Err(e) => return Err(e.to_string()),
        };
        let stamp = metadata.as_ref().map(|m| (m.len(), m.modified().ok()));
        if let Some(m) = metadata {
            if m.len() > 128 * 1024 * 1024 {
                return Err("Lutris database snapshot is too large".into());
            }
            fs::copy(&from, temp.path().join(format!("pga.db{suffix}")))
                .map_err(|e| e.to_string())?;
        }
        before.push((from, stamp));
    }
    for (path, stamp) in before {
        let current = fs::metadata(path)
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        if current != stamp {
            return Err("Lutris updated its library during scanning; rescan to retry".into());
        }
    }
    let db = Connection::open_with_flags(
        temp.path().join("pga.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    Ok((temp, db))
}

#[cfg(windows)]
fn registry_paths() -> Vec<PathBuf> {
    use std::os::windows::process::CommandExt;
    // Fixed read-only query. The XML, not a registry key's display name, supplies IDs.
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding
$paths = @(foreach ($view in @('Registry64','Registry32')) {
  $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine',$view)
  foreach ($vendor in @('EA Games','Origin Games','Respawn','BioWare','Maxis','Electronic Arts')) {
    $root = $base.OpenSubKey(('SOFTWARE\' + $vendor))
    if ($root) {
      foreach ($name in $root.GetSubKeyNames()) {
        $game = $root.OpenSubKey($name)
        foreach ($field in @('Install Dir','InstallDir','InstallLocation')) {
          $value = $game.GetValue($field)
          if ($value -is [string] -and $value) { $value }
        }
        $game.Dispose()
      }
      $root.Dispose()
    }
  }
  $base.Dispose()
})
ConvertTo-Json -Compress -InputObject $paths
"#;
    let output = std::process::Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    output
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Vec<String>>(&o.stdout).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(os: Os) -> Context {
        Context {
            os,
            local: true,
            drive_c: None,
            home: None,
        }
    }
    fn game(root: &Path, folder: &str, ids: &[&str], title: &str, payload: bool) -> PathBuf {
        let path = root.join(folder);
        fs::create_dir_all(path.join("__Installer")).unwrap();
        let ids: String = ids
            .iter()
            .map(|id| format!("<contentID>{id}</contentID>"))
            .collect();
        fs::write(path.join("__Installer/installerdata.xml"), format!("<DiPManifest><contentIDs>{ids}</contentIDs><gameTitles><gameTitle locale=\"en_US\">{title}</gameTitle></gameTitles></DiPManifest>")).unwrap();
        if payload {
            fs::write(path.join("game.exe"), b"fixture payload, never executed").unwrap();
        }
        path
    }
    fn run(root: &Path, previous: Option<&EaReport>) -> (EaReport, Vec<String>) {
        let mut discovery = Discovery::default();
        discovery.add(root.into(), "library", &context(Os::host()));
        scan_sources(discovery, &[], previous)
    }
    #[test]
    fn installer_manifests_have_stable_content_ids_titles_and_no_ownership_claim() {
        let dir = tempfile::tempdir().unwrap();
        let path = game(
            dir.path(),
            "Game",
            &["B", "A", "A"],
            "日本語 &amp; English",
            true,
        );
        let (report, warnings) = run(dir.path(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].id, "A");
        assert_eq!(report.games[0].aliases, ["A", "B"]);
        assert_eq!(report.games[0].title, "日本語 & English");
        assert!(report.games[0].library_sources.is_empty());
        assert_eq!(report.games[0].installations[0].path, epic::key(&path));
        assert_eq!(report.games[0].installations[0].state, "installed");
    }
    #[test]
    fn utf16_localized_xml_uses_english_title_and_rejects_dtd() {
        let dir = tempfile::tempdir().unwrap();
        let path = game(dir.path(), "Game", &["id"], "placeholder", true);
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><DiPManifest><contentIDs><contentID>id</contentID></contentIDs><gameTitles><gameTitle locale=\"fr_FR\">Français</gameTitle><gameTitle locale=\"en_US\">English</gameTitle></gameTitles></DiPManifest>";
        let bytes: Vec<_> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        fs::write(path.join("__Installer/installerdata.xml"), bytes).unwrap();
        assert_eq!(manifest(&path).unwrap().1, "English");
        assert!(xml("<!DOCTYPE DiPManifest [<!ENTITY file SYSTEM 'file:///nonexistent'>]><DiPManifest>&file;</DiPManifest>").is_err());
    }
    #[test]
    fn aliases_merge_dualboot_installs_while_equal_titles_with_different_ids_stay_separate() {
        let linux = tempfile::tempdir().unwrap();
        let windows = tempfile::tempdir().unwrap();
        game(linux.path(), "Linux game", &["A", "B"], "Same title", true);
        game(
            windows.path(),
            "Windows game",
            &["B", "C"],
            "Same title",
            true,
        );
        game(windows.path(), "Other edition", &["D"], "Same title", true);
        let mut discovery = Discovery::default();
        discovery.add(linux.path().into(), "library", &context(Os::Linux));
        discovery.add(windows.path().into(), "library", &context(Os::Windows));
        let (report, warnings) = scan_sources(discovery, &[], None);
        assert!(warnings.is_empty());
        assert_eq!(report.games.len(), 2);
        let merged = report.games.iter().find(|g| g.id == "A").unwrap();
        assert_eq!(merged.aliases, ["A", "B", "C"]);
        assert_eq!(merged.installations.len(), 2);
        assert!(merged.installations.iter().any(|i| i.os == Os::Linux));
        assert!(merged.installations.iter().any(|i| i.os == Os::Windows));
    }
    #[test]
    fn unreadable_or_corrupt_sources_retain_previous_installs_valid_empty_library_removes_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = game(dir.path(), "Game", &["id"], "Game", true);
        let first = run(dir.path(), None).0;
        fs::write(path.join("__Installer/installerdata.xml"), "broken").unwrap();
        game(dir.path(), "Other", &["other"], "Other", true);
        let (second, warnings) = run(dir.path(), Some(&first));
        assert!(!warnings.is_empty());
        assert_eq!(
            second
                .games
                .iter()
                .find(|g| g.id == "id")
                .unwrap()
                .installations[0]
                .state,
            "unavailable"
        );
        assert_eq!(
            second
                .games
                .iter()
                .find(|g| g.id == "other")
                .unwrap()
                .installations[0]
                .state,
            "installed"
        );
        let (missing, _) = scan_sources(Discovery::default(), &[], Some(&first));
        assert_eq!(missing.games[0].installations[0].state, "unavailable");
        fs::remove_dir_all(path).unwrap();
        fs::remove_dir_all(dir.path().join("Other")).unwrap();
        assert!(run(dir.path(), Some(&second)).0.games.is_empty());
    }
    #[test]
    fn metadata_only_installations_are_incomplete_and_random_folders_are_not_games() {
        let dir = tempfile::tempdir().unwrap();
        game(dir.path(), "Partial", &["partial"], "Partial", false);
        fs::create_dir(dir.path().join("Unidentified game folder")).unwrap();
        let (report, _) = run(dir.path(), None);
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].installations[0].state, "incomplete");
    }
    #[test]
    fn discovers_heroic_default_prefixes_and_mounted_windows_installations() {
        let dir = tempfile::tempdir().unwrap();
        let linux = dir.path().join("linux");
        fs::create_dir_all(linux.join("etc")).unwrap();
        fs::write(linux.join("etc/os-release"), "ID=fixture").unwrap();
        let home = linux.join("home/test");
        game(
            &home.join("Games/Heroic/Prefixes/default/EA App/drive_c/Program Files/EA Games"),
            "EA game",
            &["linux"],
            "Linux EA",
            true,
        );
        game(&home.join(".var/app/com.usebottles.bottles/data/bottles/bottles/EA/drive_c/Program Files/EA Games"), "Bottled", &["bottle"], "Bottled EA", true);
        let windows = dir.path().join("windows");
        fs::create_dir_all(windows.join("Windows/System32")).unwrap();
        game(
            &windows.join("Program Files/EA Games"),
            "Windows game",
            &["windows"],
            "Windows EA",
            true,
        );
        let mut discovery = Discovery::default();
        discovery.root(&linux, Os::Unknown);
        discovery.root(&windows, Os::Unknown);
        let (report, warnings) = scan_sources(discovery, &[], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 3);
        assert_eq!(
            report
                .games
                .iter()
                .find(|g| g.id == "linux")
                .unwrap()
                .installations[0]
                .os,
            Os::Linux
        );
        assert_eq!(
            report
                .games
                .iter()
                .find(|g| g.id == "windows")
                .unwrap()
                .installations[0]
                .os,
            Os::Windows
        );
    }
    #[test]
    fn client_ini_resolves_foreign_drive_paths_without_reusing_host_paths() {
        let dir = tempfile::tempdir().unwrap();
        let drive = dir.path().join("windows");
        game(
            &drive.join("Custom/EA Games"),
            "Custom",
            &["id"],
            "Configured game",
            true,
        );
        fs::create_dir_all(drive.join("ProgramData/EA Desktop")).unwrap();
        fs::write(
            drive.join("ProgramData/EA Desktop/machine.ini"),
            "[Settings]\nDownloadInPlaceDir=C:\\Custom\\EA Games\n",
        )
        .unwrap();
        let mut discovery = Discovery::default();
        discovery.root(&drive, Os::Windows);
        let (report, warnings) = scan_sources(discovery, &[], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
        let foreign = Context {
            os: Os::Linux,
            local: false,
            drive_c: None,
            home: None,
        };
        assert!(resolve("/home/test/Games", &foreign, &[]).is_err());
        assert!(resolve("C:\\..\\secret", &foreign, &[]).is_err());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::create_dir(a.path().join("Games")).unwrap();
        fs::create_dir(b.path().join("Games")).unwrap();
        assert!(resolve("D:\\Games", &foreign, &[a.path().into(), b.path().into()]).is_err());
    }
    #[test]
    fn heroic_game_configuration_finds_custom_ea_app_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("custom-prefix");
        game(
            &prefix.join("drive_c/Program Files/EA Games"),
            "Game",
            &["id"],
            "Heroic EA",
            true,
        );
        let configs = dir.path().join("heroic/GamesConfig");
        fs::create_dir_all(&configs).unwrap();
        fs::write(
            configs.join("ea.json"),
            serde_json::json!({"ea-app":{"winePrefix":prefix}}).to_string(),
        )
        .unwrap();
        let mut discovery = Discovery::default();
        discovery.add(configs, "heroic_configs", &context(Os::host()));
        let (report, warnings) = scan_sources(discovery, &[], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
    }
    #[test]
    fn lutris_library_reads_uncheckpointed_wal_without_touching_live_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pga.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE service_games (service TEXT, appid TEXT, name TEXT); CREATE TABLE games (service TEXT, slug TEXT, directory TEXT); INSERT INTO service_games VALUES ('ea_app','owned','Owned EA game'),('steam','ignored','Other store');").unwrap();
        let before: Vec<_> = ["", "-wal", "-shm"]
            .iter()
            .map(|s| {
                (
                    PathBuf::from(format!("{}{s}", path.display())),
                    fs::read(format!("{}{s}", path.display())).unwrap(),
                )
            })
            .collect();
        let mut discovery = Discovery::default();
        discovery.add(path.clone(), "lutris", &context(Os::host()));
        let (report, warnings) = scan_sources(discovery, &[], None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(report.games.len(), 1);
        assert_eq!(report.games[0].id, "owned");
        assert!(!report.games[0].library_sources.is_empty());
        assert!(report.games[0].installations.is_empty());
        for (path, bytes) in before {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        db.execute("DELETE FROM service_games WHERE service='ea_app'", [])
            .unwrap();
        let mut discovery = Discovery::default();
        discovery.add(path, "lutris", &context(Os::host()));
        assert!(
            scan_sources(discovery, &[], Some(&report))
                .0
                .games
                .is_empty()
        );
    }
    #[test]
    fn old_inventory_remains_compatible_and_corrupt_lutris_cache_preserves_library() {
        let value = serde_json::json!({"host_os":"linux","games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null});
        assert!(
            serde_json::from_value::<Report>(value)
                .unwrap()
                .ea
                .games
                .is_empty()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pga.db");
        fs::write(&path, "broken sqlite").unwrap();
        let previous = EaReport {
            games: vec![EaGame {
                id: "id".into(),
                aliases: vec!["id".into()],
                title: "EA game".into(),
                library_sources: vec![epic::key(&path)],
                installations: vec![],
            }],
        };
        let mut discovery = Discovery::default();
        discovery.add(path, "lutris", &context(Os::host()));
        let (report, warnings) = scan_sources(discovery, &[], Some(&previous));
        assert!(!warnings.is_empty());
        assert_eq!(report.games[0].id, "id");
    }
}
