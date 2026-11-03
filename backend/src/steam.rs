use crate::{model::*, platform, vdf};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn read_text(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut data = String::new();
    file.take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut data)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() > 8 * 1024 * 1024 {
        return Err(format!("{} is too large to scan", path.display()));
    }
    Ok(data)
}

fn path_key(path: &Path) -> String {
    let path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    }
}

fn steamapps(path: &Path) -> PathBuf {
    if path
        .file_name()
        .is_some_and(|p| p.eq_ignore_ascii_case("steamapps"))
    {
        path.into()
    } else {
        path.join("steamapps")
    }
}

fn normalize(s: &str) -> String {
    s.replace('\\', "/").trim_end_matches('/').to_owned()
}

/// An explicit longest-prefix mapping wins. Otherwise foreign paths are only
/// resolved when exactly one readable volume contains the referenced library.
pub fn resolve_library(
    raw: &str,
    os: Os,
    mounts: &[PathBuf],
    mappings: &[PathMapping],
) -> Result<PathBuf, String> {
    let normalized = normalize(raw);
    let mut sorted: Vec<_> = mappings
        .iter()
        .filter(|m| !m.original.trim().is_empty() && !m.mounted.trim().is_empty())
        .collect();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.original.len()));
    for mapping in sorted {
        let prefix = normalize(&mapping.original);
        let matches = if os == Os::Windows {
            normalized.eq_ignore_ascii_case(&prefix)
                || normalized
                    .to_lowercase()
                    .starts_with(&(prefix.to_lowercase() + "/"))
        } else {
            normalized == prefix || normalized.starts_with(&(prefix.clone() + "/"))
        };
        if matches {
            let suffix = &normalized[prefix.len()..];
            if suffix.split('/').any(|s| s == "..") {
                return Err(format!("Unsafe parent path in library {raw}"));
            }
            return Ok(Path::new(&mapping.mounted).join(suffix.trim_start_matches('/')));
        }
    }
    let windows_path = normalized.as_bytes().get(1) == Some(&b':');
    let native = if windows_path {
        Os::host() == Os::Windows
    } else {
        normalized.starts_with('/') && Os::host() != Os::Windows
    };
    // Do not accidentally resolve a foreign /home/user path into the host's home.
    if native && (os == Os::host() || os == Os::Unknown) {
        return Ok(PathBuf::from(raw));
    }
    let suffix = if windows_path {
        &normalized[2..]
    } else {
        &normalized
    };
    if suffix.split('/').any(|s| s == "..") {
        return Err(format!("Unsafe parent path in library {raw}"));
    }
    let mut candidates = BTreeMap::new();
    for mount in mounts {
        let path = mount.join(suffix.trim_start_matches('/'));
        if steamapps(&path).is_dir() {
            candidates.insert(path_key(&path), path);
        }
    }
    if candidates.len() == 1 {
        return Ok(candidates.into_values().next().unwrap());
    }
    Err(if candidates.len() > 1 {
        format!("Ambiguous foreign library {raw}. Add a path mapping to choose its mounted volume.")
    } else {
        format!(
            "Cannot read foreign library {raw}. Mount its volume and add a path mapping (for example D: → /mnt/games)."
        )
    })
}

pub fn scan(options: &ScanOptions, owned: &OwnedLibrary, previous: Option<&Report>) -> Report {
    let (volumes, mut warnings) = platform::volumes();
    let mut sources = platform::local_sources();
    let mounts: Vec<PathBuf> = volumes
        .iter()
        .flat_map(|v| &v.mountpoints)
        .map(PathBuf::from)
        .filter(|p| platform::scan_mount(p))
        .collect();
    for volume in &volumes {
        for mount in &volume.mountpoints {
            let path = Path::new(mount);
            // Container bind mounts of the live home aren't alternate OS roots.
            if !platform::scan_mount(path) {
                continue;
            }
            sources.extend(platform::sources_at(path, volume.os));
        }
    }
    // A manually added OS mount is also a root for translating foreign paths.
    let mut mounts = mounts;
    for source in &options.sources {
        let p = PathBuf::from(&source.path);
        if platform::os_at(&p) != Os::Unknown {
            mounts.push(p.clone());
        }
        let discovered = platform::sources_at(&p, source.os);
        if discovered.is_empty() && crate::epic::sources_at(&p, source.os).is_empty() {
            warnings.push(format!("No readable Steam or Epic library found at {} or its known locations. Check the mount, permissions, or add the client metadata folder directly.", p.display()));
        }
        sources.extend(discovered);
    }
    scan_sources(
        &sources,
        &mounts,
        &options.mappings,
        owned,
        previous,
        volumes,
        warnings,
    )
}

pub fn scan_sources(
    sources: &[Source],
    mounts: &[PathBuf],
    mappings: &[PathMapping],
    owned: &OwnedLibrary,
    previous: Option<&Report>,
    volumes: Vec<Volume>,
    mut warnings: Vec<String>,
) -> Report {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut games = BTreeMap::<u32, Game>::new();
    for game in &owned.games {
        games.insert(
            game.appid,
            Game {
                appid: game.appid,
                title: game.name.clone(),
                owned: true,
                is_demo: game.is_demo,
                installations: Vec::new(),
            },
        );
    }
    let mut libraries = Vec::new();
    let mut visited = BTreeSet::new();
    let mut verified = BTreeSet::new();
    let mut queue: Vec<Source> = sources.to_vec();
    let mut i = 0;
    while i < queue.len() && i < 1024 {
        let source = queue[i].clone();
        i += 1;
        let root = Path::new(&source.path);
        let library = steamapps(root);
        let key = path_key(&library);
        if !visited.insert(key.clone()) {
            continue;
        }
        let os = if root.join("steam.exe").is_file() {
            Os::Windows
        } else {
            source.os
        };
        let entries = match fs::read_dir(&library) {
            Ok(entries) => entries,
            Err(e) => {
                warnings.push(format!("Cannot read Steam library {}: {e}. Previously seen installations are retained.", library.display()));
                continue;
            }
        };
        libraries.push(Source {
            path: key.clone(),
            os,
        });
        let mut complete = true;
        for result in entries {
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    complete = false;
                    warnings.push(format!("Cannot enumerate {}: {e}", library.display()));
                    continue;
                }
            };
            let filename = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = filename
                .strip_prefix("appmanifest_")
                .and_then(|s| s.strip_suffix(".acf"))
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            match read_manifest(&entry.path(), id, &library, os, now) {
                Ok(Some((title, installation))) => {
                    let game = games.entry(id).or_insert_with(|| Game {
                        appid: id,
                        title,
                        owned: false,
                        is_demo: false,
                        installations: Vec::new(),
                    });
                    game.installations.push(installation);
                }
                Ok(None) => {}
                Err(e) => {
                    complete = false;
                    warnings.push(e);
                }
            }
        }
        if complete {
            verified.insert(key);
        }
        let steam_root = library.parent().unwrap_or(root);
        for config in [
            library.join("libraryfolders.vdf"),
            steam_root.join("config/libraryfolders.vdf"),
        ] {
            if !config.is_file() {
                continue;
            }
            let parsed = read_text(&config).and_then(|s| vdf::parse(&s));
            match parsed {
                Ok(value) => {
                    if let Some(entries) = value.get("libraryfolders").and_then(vdf::Value::entries)
                    {
                        for (id, value) in entries {
                            if id.parse::<u32>().is_err() {
                                continue;
                            }
                            let Some(raw) = value.text().or_else(|| value.field("path")) else {
                                continue;
                            };
                            // The recorded path of this library is a useful mount anchor,
                            // including when a SteamLibrary was added directly by the user.
                            let mut inferred = mappings.to_vec();
                            if same_library_suffix(raw, steam_root) {
                                inferred.push(PathMapping {
                                    original: raw.into(),
                                    mounted: steam_root.to_string_lossy().into(),
                                });
                            }
                            match resolve_library(raw, os, mounts, &inferred) {
                                Ok(path) => queue.push(Source {
                                    path: path.to_string_lossy().into(),
                                    os,
                                }),
                                Err(e) => warnings.push(e),
                            }
                        }
                    } else {
                        warnings.push(format!("{} has no libraryfolders object", config.display()));
                    }
                }
                Err(e) => warnings.push(format!("Cannot parse {}: {e}", config.display())),
            }
        }
    }
    if i >= 1024 {
        warnings.push("Steam library discovery limit reached.".into());
    }
    // Keep a disappeared/unreadable library's installations as unverified. A
    // successful scan (including an empty library) can confirm removal.
    if let Some(previous) = previous {
        for old in &previous.games {
            for installation in &old.installations {
                if verified.contains(&installation.library) {
                    continue;
                }
                let game = games.entry(old.appid).or_insert_with(|| Game {
                    appid: old.appid,
                    title: old.title.clone(),
                    owned: owned.games.iter().any(|g| g.appid == old.appid),
                    is_demo: old.is_demo,
                    installations: Vec::new(),
                });
                if game
                    .installations
                    .iter()
                    .any(|i| i.library == installation.library)
                {
                    continue;
                }
                let mut cached = installation.clone();
                cached.state = "unavailable".into();
                game.installations.push(cached);
            }
        }
    }
    let mut games: Vec<_> = games.into_values().collect();
    games.sort_by_cached_key(|game| (game.title.to_lowercase(), game.appid));
    warnings.sort();
    warnings.dedup();
    Report {
        host_os: Os::host(),
        games,
        libraries,
        volumes,
        linux_systems: vec![],
        warnings,
        owned_account: (!owned.steamid.is_empty()).then(|| owned.steamid.clone()),
        owned_source: owned.source.clone(),
        epic: EpicReport::default(),
        ea: EaReport::default(),
        catalog: vec![],
        scanned_at: 0,
    }
}

fn same_library_suffix(raw: &str, mounted: &Path) -> bool {
    let normalized = normalize(raw);
    let foreign = if normalized.as_bytes().get(1) == Some(&b':') {
        &normalized[2..]
    } else {
        &normalized
    };
    let mounted = normalize(&mounted.to_string_lossy());
    // Never guess from just a drive/root. Require a directory suffix.
    let suffix = foreign.trim_start_matches('/');
    !suffix.is_empty()
        && mounted
            .to_lowercase()
            .ends_with(&format!("/{}", suffix.to_lowercase()))
}

fn read_manifest(
    path: &Path,
    expected_id: u32,
    library: &Path,
    os: Os,
    now: u64,
) -> Result<Option<(String, Installation)>, String> {
    let parsed = vdf::parse(&read_text(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    let app = parsed
        .get("AppState")
        .ok_or_else(|| format!("{}: missing AppState", path.display()))?;
    if app.field("appid").and_then(|s| s.parse::<u32>().ok()) != Some(expected_id) {
        return Err(format!(
            "{}: app ID does not match its filename",
            path.display()
        ));
    }
    let title = app
        .field("name")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{}: missing game title", path.display()))?
        .to_string();
    // Hide Steam's shared runtimes, not arbitrary titles cached by the client.
    if [228980, 1070560, 1391110, 1628350, 1493710].contains(&expected_id)
        || title.starts_with("Proton ")
        || title.starts_with("Steam Linux Runtime")
    {
        return Ok(None);
    }
    let dir = app
        .field("installdir")
        .ok_or_else(|| format!("{}: missing installdir", path.display()))?;
    if dir.is_empty()
        || dir.contains(['/', '\\', ':'])
        || Path::new(dir)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "{}: invalid installation directory",
            path.display()
        ));
    }
    let install = library.join("common").join(dir);
    let flags = app.field("StateFlags").and_then(|s| s.parse::<u64>().ok());
    let state = match fs::metadata(&install) {
        Ok(metadata) if metadata.is_dir() => {
            if flags.is_some_and(|flags| flags & 4 != 0 && flags & !(2 | 4 | 64) == 0) {
                "installed"
            } else {
                "incomplete"
            }
        }
        Ok(_) => "files_missing",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "files_missing",
        Err(_) => "unavailable",
    };
    Ok(Some((
        title,
        Installation {
            library: path_key(library),
            path: install.to_string_lossy().into(),
            os,
            state: state.into(),
            last_seen: now,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture(root: &Path, id: u32, title: &str, flags: u32, exists: bool) {
        fs::create_dir_all(root.join("steamapps")).unwrap();
        if exists {
            fs::create_dir_all(root.join("steamapps/common").join(title)).unwrap();
        }
        fs::write(root.join(format!("steamapps/appmanifest_{id}.acf")),
            format!(r#""AppState" {{ "appid" "{id}" "name" "{title}" "installdir" "{title}" "StateFlags" "{flags}" }}"#)).unwrap();
    }
    fn run(sources: &[Source], previous: Option<&Report>) -> Report {
        scan_sources(
            sources,
            &[],
            &[],
            &OwnedLibrary::default(),
            previous,
            vec![],
            vec![],
        )
    }
    #[test]
    fn two_operating_systems_merge_without_claiming_ownership() {
        let linux = TempDir::new().unwrap();
        let windows = TempDir::new().unwrap();
        fixture(linux.path(), 42, "Same game", 4, true);
        fixture(windows.path(), 42, "Same game", 4, true);
        let report = run(
            &[
                Source {
                    path: linux.path().to_string_lossy().into(),
                    os: Os::Linux,
                },
                Source {
                    path: windows.path().to_string_lossy().into(),
                    os: Os::Windows,
                },
            ],
            None,
        );
        assert_eq!(report.games.len(), 1);
        assert!(!report.games[0].owned);
        assert_eq!(report.games[0].installations.len(), 2);
        assert_eq!(report.games[0].installations[1].os, Os::Windows);
    }
    #[test]
    fn retains_unreadable_installation_but_confirms_actual_removal() {
        let dir = TempDir::new().unwrap();
        fixture(dir.path(), 42, "Game", 4, true);
        let sources = [Source {
            path: dir.path().to_string_lossy().into(),
            os: Os::Windows,
        }];
        let first = run(&sources, None);
        fs::rename(dir.path().join("steamapps"), dir.path().join("offline")).unwrap();
        let second = run(&sources, Some(&first));
        assert_eq!(second.games[0].installations[0].state, "unavailable");
        fs::create_dir(dir.path().join("steamapps")).unwrap();
        assert!(run(&sources, Some(&second)).games.is_empty());
    }
    #[test]
    fn incomplete_missing_and_corrupt_manifests_are_not_ready() {
        let dir = TempDir::new().unwrap();
        fixture(dir.path(), 1, "Downloading", 1026, true);
        fixture(dir.path(), 2, "Missing", 4, false);
        fixture(dir.path(), 3, "Corrupt", 4, true);
        fs::write(dir.path().join("steamapps/appmanifest_3.acf"), "AppState {").unwrap();
        let report = run(
            &[Source {
                path: dir.path().to_string_lossy().into(),
                os: Os::Linux,
            }],
            None,
        );
        assert_eq!(report.games.len(), 2);
        assert_eq!(report.games[0].installations[0].state, "incomplete");
        assert_eq!(report.games[1].installations[0].state, "files_missing");
        assert_eq!(report.warnings.len(), 1);
    }
    #[test]
    fn follows_foreign_libraries_and_resolves_explicit_drive_mapping() {
        let os = TempDir::new().unwrap();
        let extra = TempDir::new().unwrap();
        let steam = os.path().join("Program Files (x86)/Steam");
        fixture(&steam, 1, "Local", 4, true);
        fixture(extra.path(), 2, "External", 4, true);
        fs::write(steam.join("steamapps/libraryfolders.vdf"), r#"libraryfolders { "0" { path "C:\\Program Files (x86)\\Steam" } "1" { path "D:\\SteamLibrary" } }"#).unwrap();
        let mapping = PathMapping {
            original: r"D:\SteamLibrary".into(),
            mounted: extra.path().to_string_lossy().into(),
        };
        let report = scan_sources(
            &[Source {
                path: steam.to_string_lossy().into(),
                os: Os::Windows,
            }],
            &[os.path().into()],
            &[mapping],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        assert_eq!(report.games.len(), 2);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
    #[test]
    fn does_not_guess_between_duplicate_foreign_paths() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        for p in [a.path(), b.path()] {
            fs::create_dir_all(p.join("SteamLibrary/steamapps")).unwrap();
        }
        let result = resolve_library(
            r"D:\SteamLibrary",
            Os::Windows,
            &[a.path().into(), b.path().into()],
            &[],
        );
        if Os::host() != Os::Windows {
            assert!(result.unwrap_err().contains("Ambiguous"));
        }
    }
    #[test]
    fn owned_titles_merge_with_installed_titles_and_aliases_deduplicate() {
        let dir = TempDir::new().unwrap();
        fixture(dir.path(), 1, "Installed", 4, true);
        let source = Source {
            path: dir.path().to_string_lossy().into(),
            os: Os::host(),
        };
        let owned = OwnedLibrary {
            steamid: "test".into(),
            source: "test".into(),
            games: vec![
                OwnedGame {
                    appid: 1,
                    name: "Installed".into(),
                    is_demo: false,
                },
                OwnedGame {
                    appid: 2,
                    name: "Owned only".into(),
                    is_demo: false,
                },
            ],
        };
        let report = scan_sources(
            &[source.clone(), source],
            &[],
            &[],
            &owned,
            None,
            vec![],
            vec![],
        );
        assert_eq!(report.games.len(), 2);
        assert!(report.games.iter().all(|g| g.owned));
        assert_eq!(report.games[0].installations.len(), 1);
        assert!(report.games[1].installations.is_empty());
    }
}
