use crate::{epic, model::*, platform, steam};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub os: Os,
    pub local: bool,
    pub client: String,
    pub drive_c: Option<PathBuf>,
}

pub fn children(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(512)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.is_symlink())
        .collect()
}

pub fn roots(report: &Report) -> Vec<Location> {
    let mut result = Vec::new();
    for volume in &report.volumes {
        for point in &volume.mountpoints {
            let path = PathBuf::from(point);
            if platform::scan_mount(&path) {
                result.push(Location {
                    drive_c: Some(path.clone()),
                    path,
                    os: volume.os,
                    local: false,
                    client: String::new(),
                });
            }
        }
    }
    if cfg!(windows) {
        let path = PathBuf::from(format!(
            "{}/",
            env::var("SystemDrive").unwrap_or("C:".into())
        ));
        result.insert(
            0,
            Location {
                drive_c: Some(path.clone()),
                path,
                os: Os::Windows,
                local: true,
                client: String::new(),
            },
        );
    }
    dedup(&mut result);
    result
}

pub fn homes(report: &Report) -> Vec<Location> {
    let mut result = Vec::new();
    if let Some(path) = platform::home() {
        result.push(Location {
            path,
            os: Os::host(),
            local: true,
            client: String::new(),
            drive_c: None,
        });
    }
    for root in roots(report) {
        for directory in ["home", "var/home", "@home", "@/home", "@/var/home", "Users"] {
            let parent = root.path.join(directory);
            if parent.is_symlink() {
                continue;
            }
            for path in children(&parent) {
                result.push(Location {
                    path,
                    os: if directory == "Users" {
                        Os::Windows
                    } else {
                        Os::Linux
                    },
                    ..root.clone()
                });
            }
        }
        if !root.path.join("etc").exists() && !root.path.join("Windows").exists() {
            for path in children(&root.path) {
                result.push(Location {
                    path,
                    drive_c: None,
                    ..root.clone()
                });
            }
        }
    }
    dedup(&mut result);
    result
}

pub fn prefixes(report: &Report) -> Vec<Location> {
    let mut result = Vec::new();
    if let Some(path) = env::var_os("WINEPREFIX") {
        result.push(Location {
            path: path.into(),
            os: Os::Linux,
            local: true,
            client: "Wine".into(),
            drive_c: None,
        });
    }
    if let Some(data) = env::var_os("XDG_DATA_HOME") {
        for path in children(&PathBuf::from(data).join("bottles/bottles")) {
            result.push(Location {
                path,
                os: Os::Linux,
                local: true,
                client: "Bottles".into(),
                drive_c: None,
            });
        }
    }
    for home in homes(report) {
        for suffix in [
            ".wine",
            "Games/ea-app",
            "Games/ubisoft-connect",
            "Games/epic-games-store",
        ] {
            result.push(Location {
                path: home.path.join(suffix),
                os: Os::Linux,
                client: "Wine".into(),
                ..home.clone()
            });
        }
        for (suffix, client) in [
            ("Games", "Wine"),
            ("Games/Heroic/Prefixes", "Heroic"),
            ("Games/Heroic/Prefixes/default", "Heroic"),
            (".local/share/bottles/bottles", "Bottles"),
            (
                ".var/app/com.usebottles.bottles/data/bottles/bottles",
                "Bottles",
            ),
        ] {
            for path in children(&home.path.join(suffix)) {
                result.push(Location {
                    path,
                    os: Os::Linux,
                    client: client.into(),
                    ..home.clone()
                });
            }
        }
        for suffix in [
            ".config/lutris/games",
            ".var/app/net.lutris.Lutris/config/lutris/games",
        ] {
            for entry in fs::read_dir(home.path.join(suffix))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .take(2048)
            {
                if let Ok(content) = steam::read_text(&entry.path())
                    && let Ok(config) = serde_yaml_ng::from_str::<serde_json::Value>(&content)
                    && let Some(raw) = config.pointer("/game/prefix").and_then(|v| v.as_str())
                    && let Some(path) = resolve(raw, &home, report)
                {
                    result.push(Location {
                        path,
                        os: Os::Linux,
                        client: "Lutris".into(),
                        ..home.clone()
                    });
                }
            }
        }
        for suffix in [
            ".config/heroic/GamesConfig",
            ".var/app/com.heroicgameslauncher.hgl/config/heroic/GamesConfig",
            "AppData/Roaming/heroic/GamesConfig",
        ] {
            for entry in fs::read_dir(home.path.join(suffix))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .take(512)
            {
                if let Ok(value) = json(&entry.path()) {
                    for raw in strings(&value, "winePrefix", 0) {
                        if let Some(path) = resolve(&raw, &home, report) {
                            result.push(Location {
                                path,
                                os: Os::Linux,
                                client: "Heroic".into(),
                                ..home.clone()
                            });
                        }
                    }
                }
            }
        }
    }
    for library in &report.libraries {
        for entry in children(&Path::new(&library.path).join("compatdata")) {
            result.push(Location {
                path: entry.join("pfx"),
                os: Os::Linux,
                local: library.os == Os::host(),
                client: "Proton".into(),
                drive_c: None,
            });
        }
    }
    result.retain(|p| p.path.join("drive_c").is_dir());
    for prefix in &mut result {
        prefix.drive_c = Some(prefix.path.join("drive_c"));
    }
    result.sort_by_key(|p| match p.client.as_str() {
        "Proton" => 0,
        "Bottles" => 1,
        "Heroic" => 2,
        "Lutris" => 3,
        _ => 4,
    });
    dedup(&mut result);
    result
}

fn dedup(locations: &mut Vec<Location>) {
    let mut seen = BTreeSet::new();
    locations.retain(|l| seen.insert(epic::key(&l.path)));
}

pub fn json(path: &Path) -> Result<serde_json::Value, String> {
    serde_json::from_str(steam::read_text(path)?.trim_start_matches('\u{feff}'))
        .map_err(|_| format!("Invalid metadata {}", path.display()))
}

pub fn strings(value: &serde_json::Value, field: &str, depth: usize) -> Vec<String> {
    if depth > 4 {
        return vec![];
    }
    let mut values = Vec::new();
    if let Some(object) = value.as_object() {
        if let Some(raw) = object.get(field).and_then(|v| v.as_str()) {
            values.push(raw.into());
        }
        for value in object.values().take(512) {
            values.extend(strings(value, field, depth + 1));
        }
    }
    values
}

pub fn resolve(raw: &str, source: &Location, report: &Report) -> Option<PathBuf> {
    let raw = raw.replace('\\', "/");
    if raw.split('/').any(|p| p == "..") {
        return None;
    }
    if let Some(suffix) = raw.strip_prefix("~/") {
        return Some(source.path.join(suffix));
    }
    let windows = raw.as_bytes().get(1) == Some(&b':');
    if windows
        && source.local
        && let Some(prefix) = source
            .drive_c
            .as_ref()
            .filter(|p| p.ends_with("drive_c"))
            .and_then(|p| p.parent())
    {
        let drive = prefix
            .join("dosdevices")
            .join(raw[..2].to_ascii_lowercase());
        if let Ok(target) = drive.canonicalize() {
            return Some(target.join(raw[2..].trim_start_matches('/')));
        }
    }
    if windows
        && raw[..2].eq_ignore_ascii_case("C:")
        && let Some(drive) = &source.drive_c
    {
        return Some(drive.join(raw[2..].trim_start_matches('/')));
    }
    if source.local
        && ((windows && Os::host() == Os::Windows)
            || (!windows && raw.starts_with('/') && Os::host() == Os::Linux))
    {
        return Some(raw.into());
    }
    if !windows && !raw.starts_with('/') {
        return None;
    }
    let suffix = if windows { &raw[2..] } else { &raw };
    let mut candidates: Vec<_> = roots(report)
        .iter()
        .map(|l| l.path.join(suffix.trim_start_matches('/')))
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort_by_key(|p| epic::key(p));
    candidates.dedup_by_key(|p| epic::key(p));
    (candidates.len() == 1).then(|| candidates.remove(0))
}

pub fn program(name: &str) -> Option<String> {
    let names = if cfg!(windows) {
        vec![format!("{name}.exe"), name.into()]
    } else {
        vec![name.into()]
    };
    env::var_os("PATH")
        .into_iter()
        .flat_map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .flat_map(|p| names.iter().map(move |n| p.join(n)))
        .find(|p| {
            p.is_file()
                && (!p.starts_with("/app/bin")
                    || !Path::new("/.flatpak-info").is_file()
                    || host_program(name))
        })
        .map(|p| p.to_string_lossy().into())
}

fn host_program(name: &str) -> bool {
    use std::{
        collections::HashMap,
        process::{Command, Stdio},
        sync::{LazyLock, Mutex},
        time::{Duration, Instant},
    };
    static CACHE: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> =
        LazyLock::new(Default::default);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((time, found)) = cache.get(name)
        && time.elapsed() < Duration::from_secs(60)
    {
        return *found;
    }
    // Fixed shell program, with the requested executable passed as an argument.
    let found = Command::new("flatpak-spawn")
        .args([
            "--host",
            "sh",
            "-c",
            "command -v -- \"$1\" >/dev/null",
            "exposed-games",
            name,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .is_some_and(|mut child| {
            let started = Instant::now();
            loop {
                if let Ok(Some(status)) = child.try_wait() {
                    break status.success();
                }
                if started.elapsed() > Duration::from_secs(3) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    cache.insert(name.into(), (Instant::now(), found));
    found
}

pub fn desktop_command(needle: &str) -> Option<(String, Vec<String>)> {
    let mut folders: Vec<_> = env::var_os("XDG_DATA_DIRS")
        .map(|v| {
            env::split_paths(&v)
                .map(|p| p.join("applications"))
                .collect()
        })
        .unwrap_or_else(|| vec![PathBuf::from("/usr/share/applications")]);
    folders.push("/var/lib/flatpak/exports/share/applications".into());
    if let Some(p) = env::var_os("XDG_DATA_HOME") {
        folders.insert(0, PathBuf::from(p).join("applications"));
    }
    if let Some(home) = platform::home() {
        folders.insert(0, home.join(".local/share/applications"));
        folders.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    for folder in folders {
        for entry in fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(2048)
        {
            if !entry
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .contains(needle)
            {
                continue;
            }
            let Ok(text) = steam::read_text(&entry.path()) else {
                continue;
            };
            if let Some(line) = text.lines().find_map(|s| s.strip_prefix("Exec="))
                && let Some(mut args) = shlex::split(line)
            {
                args.retain(|a| !["%u", "%U", "%f", "%F", "%i", "%c", "%k"].contains(&a.as_str()));
                if !args.is_empty() {
                    return Some((args.remove(0), args));
                }
            }
        }
    }
    None
}
