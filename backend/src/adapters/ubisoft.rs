use super::*;
use crate::{catalog, discovery, epic, steam};
use prost::Message;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

#[derive(Clone, PartialEq, Message)]
struct Cache {
    #[prost(message, repeated, tag = "1")]
    games: Vec<Product>,
}
#[derive(Clone, PartialEq, Message)]
struct Product {
    #[prost(uint32, tag = "1")]
    id: u32,
    #[prost(uint32, tag = "2")]
    install_id: u32,
    #[prost(string, tag = "3")]
    yaml: String,
}

pub fn scan(scan: &mut Scan<'_>) {
    let mut profiles = Vec::new();
    for home in &scan.homes {
        profiles.push(Location {
            path: home
                .path
                .join("AppData/Local/Ubisoft Game Launcher/cache/configuration/configurations"),
            ..home.clone()
        });
    }
    for prefix in scan.prefixes.clone() {
        for user in discovery::children(&prefix.path.join("drive_c/users")) {
            profiles.push(Location {
                path: user
                    .join("AppData/Local/Ubisoft Game Launcher/cache/configuration/configurations"),
                ..prefix.clone()
            });
        }
        let registry = prefix.path.join("system.reg");
        file(scan, &registry, |scan| {
            read_registry(scan, &prefix, &registry)
        });
    }
    #[cfg(windows)]
    native_registry(scan);
    let mut seen = BTreeSet::new();
    for profile in profiles {
        if !seen.insert(epic::key(&profile.path)) {
            continue;
        }
        let path = profile.path.clone();
        file(scan, &path, |scan| {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            if bytes.len() > 32 * 1024 * 1024 {
                return Err("Ubisoft cache is too large".into());
            }
            let cache = Cache::decode(bytes.as_slice()).map_err(|_| "Unsupported Ubisoft cache")?;
            let products = cache
                .games
                .into_iter()
                .filter(|p| !p.yaml.trim().is_empty())
                .map(|p| {
                    serde_yaml_ng::from_str::<serde_json::Value>(&p.yaml)
                        .map(|data| (p, data))
                        .map_err(|_| "Invalid Ubisoft product data")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let addons: BTreeSet<u32> = products
                .iter()
                .flat_map(|(_, data)| {
                    data.pointer("/root/addons")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                })
                .filter_map(|row| {
                    row.get("id")
                        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                        .and_then(|n| u32::try_from(n).ok())
                })
                .collect();
            for (product, data) in products {
                if product.id == 0 || product.yaml.trim().is_empty() {
                    continue;
                }
                let Some(root) = data.get("root") else {
                    continue;
                };
                if !playable(root) || addons.contains(&product.id) {
                    let mut excluded = catalog::entry(
                        "ubisoft",
                        &product.id.to_string(),
                        &product.id.to_string(),
                        vec![],
                        &epic::key(&path),
                    );
                    excluded.is_game = Some(false);
                    catalog::merge(&mut scan.games, excluded);
                    continue;
                }
                let mut title = text(root, "name").unwrap_or("").to_owned();
                if let Some(localized) = data
                    .pointer("/localizations/default")
                    .and_then(|v| v.get(&title))
                    .and_then(|v| v.as_str())
                {
                    title = localized.into();
                }
                if title.is_empty() {
                    continue;
                }
                let id = product.id.to_string();
                if product.install_id != 0 && product.install_id != product.id {
                    let alias = format!("ubisoft:{}", product.install_id);
                    if let Some(game) = scan.games.iter_mut().find(|g| g.key == alias) {
                        game.key = format!("ubisoft:{id}");
                        game.id = id.clone();
                        if cfg!(windows) && catalog::local(&game.installations) {
                            game.launch = Some(catalog::url(
                                "Ubisoft Connect",
                                format!("uplay://launch/{id}"),
                            ));
                            game.uninstall = Some(catalog::url(
                                "Ubisoft Connect",
                                format!("uplay://uninstall/{id}"),
                            ));
                        }
                    }
                }

                let mut game = catalog::entry("ubisoft", &id, &title, vec![], &epic::key(&path));
                game.client = "Ubisoft Connect".into();
                catalog::add_os(&mut game.client_os, profile.os);
                game.is_game = Some(true);
                game.in_library = true;
                game.cover = cover_url(&data);
                game.store_page = Some(catalog::url(
                    "Ubisoft Connect",
                    format!(
                        "https://store.ubisoft.com/search?q={}",
                        catalog::encode(&title)
                    ),
                ));
                catalog::merge(&mut scan.games, game);
            }
            Ok(())
        });
    }
}

fn cover_url(data: &serde_json::Value) -> Option<String> {
    let value = text(&data["root"], "thumb_image")?;
    let value = data
        .pointer("/localizations/default")
        .and_then(|v| v.get(value))
        .and_then(|v| v.as_str())
        .unwrap_or(value);
    if value.starts_with("https://") {
        Some(value.into())
    } else if value.ends_with(".jpg") || value.ends_with(".png") || value.ends_with(".webp") {
        Some(format!(
            "https://ubistatic3-a.akamaihd.net/orbit/uplay_launcher_3_0/assets/{value}"
        ))
    } else {
        None
    }
}

pub(crate) fn cached_cover(game: &LibraryGame) -> Option<String> {
    let bytes = fs::read(&game.source).ok()?;
    if bytes.len() > 32 * 1024 * 1024 {
        return None;
    }
    let cache = Cache::decode(bytes.as_slice()).ok()?;
    let product = cache
        .games
        .into_iter()
        .find(|p| p.id.to_string() == game.id)?;
    let data = serde_yaml_ng::from_str(&product.yaml).ok()?;
    cover_url(&data)
}

fn playable(root: &serde_json::Value) -> bool {
    let ulc = root.get("is_ulc");
    !(ulc.and_then(|v| v.as_bool()) == Some(true)
        || ulc.and_then(|v| v.as_u64()) == Some(1)
        || ulc
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("true") || s == "1"))
        && root.get("start_game").is_some_and(|v| v.is_object())
}

fn read_registry(scan: &mut Scan<'_>, prefix: &Location, registry: &Path) -> Result<(), String> {
    let content = steam::read_text(registry)?;
    let mut id = None;
    let mut installs = BTreeMap::new();
    for line in content.lines() {
        if line.starts_with('[') {
            let key = line
                .split(']')
                .next()
                .unwrap_or("")
                .replace("\\\\", "\\")
                .to_lowercase();
            id = key
                .split("\\ubisoft\\launcher\\installs\\")
                .nth(1)
                .and_then(|s| s.split('\\').next())
                .filter(|s| s.parse::<u32>().is_ok())
                .map(str::to_owned);
        } else if let Some(id) = &id
            && let Some(raw) = line
                .strip_prefix("\"InstallDir\"=\"")
                .and_then(|s| s.strip_suffix('"'))
        {
            installs.insert(id.clone(), raw.replace("\\\\", "\\"));
        }
    }
    for (id, raw) in installs {
        record(scan, prefix, &epic::key(registry), &id, &raw);
    }
    Ok(())
}

fn record(scan: &mut Scan<'_>, location: &Location, source: &str, id: &str, raw: &str) {
    let resolved = discovery::resolve(raw, location, scan.report);
    let title = resolved
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("Ubisoft game {id}"));
    let missing = resolved.is_none();
    let path = resolved
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(raw.into());
    let mut install = catalog::install(source, path, location.os, true);
    if missing {
        install.state = "unavailable".into();
    }
    let mut game = catalog::entry("ubisoft", id, &title, vec![install], source);
    game.client = "Ubisoft Connect".into();
    game.store_page = Some(catalog::url(
        "Ubisoft",
        format!(
            "https://store.ubisoft.com/search?q={}",
            catalog::encode(&title)
        ),
    ));
    if cfg!(windows) && catalog::local(&game.installations) {
        game.launch = Some(catalog::url(
            "Ubisoft Connect",
            format!("uplay://launch/{id}"),
        ));
        game.uninstall = Some(catalog::url(
            "Ubisoft Connect",
            format!("uplay://uninstall/{id}"),
        ));
    }
    catalog::merge(&mut scan.games, game);
}

#[cfg(windows)]
fn native_registry(scan: &mut Scan<'_>) {
    use std::os::windows::process::CommandExt;
    let script = r#"[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding
$out=@(foreach($key in @('HKLM:\SOFTWARE\Ubisoft\Launcher\Installs','HKLM:\SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs')){Get-ChildItem $key -ErrorAction SilentlyContinue|ForEach-Object{$p=Get-ItemProperty $_.PSPath;[PSCustomObject]@{id=$_.PSChildName;path=$p.InstallDir}}});ConvertTo-Json -Compress -InputObject $out"#;
    if let Ok(output) = std::process::Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        && output.status.success()
        && let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
    {
        let source = "registry:ubisoft";
        let location = Location {
            path: std::path::PathBuf::new(),
            os: Os::Windows,
            local: true,
            client: "Ubisoft Connect".into(),
            drive_c: None,
        };
        for row in rows {
            if let (Some(id), Some(path)) = (text(&row, "id"), text(&row, "path")) {
                if id.parse::<u32>().is_ok() {
                    record(scan, &location, source, id, path);
                }
            }
        }
        scan.verified.insert(source.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portraits_resolve_localization_keys_and_reject_unresolved_keys() {
        let mut data = serde_json::json!({"root":{"thumb_image":"l2"},"localizations":{"default":{"l2":"portrait.jpg"},"ru-RU":{"l2":"russian.jpg"}}});
        assert!(cover_url(&data).unwrap().ends_with("/portrait.jpg"));
        data["root"]["thumb_image"] = "unknown-key".into();
        assert!(cover_url(&data).is_none());
        data["root"]["thumb_image"] = "https://cdn.example/portrait.png".into();
        assert_eq!(
            cover_url(&data).unwrap(),
            "https://cdn.example/portrait.png"
        );
    }
    #[test]
    fn cached_configuration_localizes_titles_and_filters_dlc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("AppData/Local/Ubisoft Game Launcher/cache/configuration/configurations");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cache = Cache { games: vec![Product { id:42, install_id:42, yaml:"root:\n  name: NAME\n  start_game: {}\n  thumb_image: cover.jpg\nlocalizations:\n  default:\n    NAME: Ubisoft fixture\n".into() },Product { id:43, install_id:43, yaml:"root:\n  name: DLC\n  is_ulc: true\n".into() }] };
        fs::write(&path, cache.encode_to_vec()).unwrap();
        let report = crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        let mut scan = Scan {
            report: &report,
            homes: vec![Location {
                path: dir.path().into(),
                os: Os::Windows,
                local: false,
                client: String::new(),
                drive_c: None,
            }],
            prefixes: vec![],
            roots: vec![],
            games: vec![],
            verified: BTreeSet::new(),
            warnings: vec![],
        };
        super::scan(&mut scan);
        assert!(scan.warnings.is_empty());
        scan.games.retain(catalog::visible);
        assert_eq!(scan.games.len(), 1);
        assert_eq!(scan.games[0].title, "Ubisoft fixture");
        assert_eq!(scan.games[0].client_os, vec![Os::Windows]);
        assert!(scan.games[0].installations.is_empty());
        assert!(scan.games[0].launch.is_none());
    }
    #[test]
    fn only_launchable_products_are_games_and_old_cache_needs_revalidation() {
        for row in [
            serde_json::json!({"name":"1,200 Welcome Pack"}),
            serde_json::json!({"name":"Soundtrack","start_game":null}),
            serde_json::json!({"name":"DLC","is_ulc":"true","start_game":{}}),
            serde_json::json!({"name":"DLC","is_ulc":1,"start_game":{}}),
        ] {
            assert!(!playable(&row));
        }
        assert!(playable(
            &serde_json::json!({"name":"Coin","start_game":{"online":{}}})
        ));
        let mut old = catalog::entry(
            "ubisoft",
            "42",
            "Cached product",
            vec![],
            "/cache/configuration/configurations",
        );
        assert!(!catalog::visible(&old));
        old.is_game = Some(true);
        assert!(catalog::visible(&old));
        let mut excluded = old.clone();
        excluded.is_game = Some(false);
        let mut games = vec![old];
        catalog::merge(&mut games, excluded);
        assert!(!catalog::visible(&games[0]));
    }
}
