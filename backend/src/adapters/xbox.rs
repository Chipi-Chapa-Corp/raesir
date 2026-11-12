use super::*;
use crate::{catalog, discovery, epic, steam};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub fn scan(scan: &mut Scan<'_>) {
    for root in scan.roots.clone() {
        let packages = root.path.join("Program Files/WindowsApps");
        if packages.is_dir() {
            if let Err(e) = fs::read_dir(&packages) {
                scan.warnings.push(format!(
                    "Xbox packages at {} are protected: {e}",
                    packages.display()
                ));
            }
            for folder in discovery::children(&packages) {
                let path = folder.join("xboxservices.config");
                file(scan, &path, |scan| legacy_config(scan, &folder, &path));
            }
        }
        for game in discovery::children(&root.path.join("XboxGames")) {
            for folder in [game.join("Content"), game.clone()] {
                if let Some(path) = config(&folder) {
                    file(scan, &path, |scan| {
                        native_config(scan, &root, &folder, &path)
                    });
                }
            }
        }
    }
    for home in scan.homes.clone() {
        let mut states = vec![home.path.join(".local/state/ferestre")];
        if home.local
            && let Some(path) = std::env::var_os("XDG_STATE_HOME")
        {
            states.insert(0, PathBuf::from(path).join("ferestre"));
        }
        let mut seen = BTreeSet::new();
        for state in states {
            if !seen.insert(epic::key(&state)) {
                continue;
            }
            let installed = state.join("installed");
            if installed.is_dir() {
                file(scan, &installed, |scan| {
                    for row in fs::read_dir(&installed)
                        .map_err(|e| e.to_string())?
                        .take(4096)
                    {
                        let path = row.map_err(|e| e.to_string())?.path();
                        if path.extension().is_none_or(|e| e != "json") {
                            continue;
                        }
                        let value = discovery::json(&path)?;
                        let id = text(&value, "product_id")
                            .filter(|s| store_id(s))
                            .ok_or("Invalid Ferestre product ID")?;
                        let raw = text(&value, "dir").ok_or("Ferestre install has no directory")?;
                        let resolved = discovery::resolve(raw, &home, scan.report);
                        let path = resolved
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or(raw.into());
                        let mut install =
                            catalog::install(&epic::key(&installed), path, Os::Linux, true);
                        if !ferestre_ready(Path::new(&install.path)) {
                            install.state = "incomplete".into();
                        }
                        let mut game = xbox(id, id, vec![install], &epic::key(&installed));
                        game.client = "Ferestre".into();
                        if home.local
                            && catalog::local(&game.installations)
                            && let Some(program) = discovery::program("ferestre")
                        {
                            game.launch = Some(catalog::command(
                                "Ferestre",
                                program.clone(),
                                vec!["run".into(), id.into()],
                            ));
                            game.uninstall = Some(catalog::command(
                                "Ferestre",
                                program,
                                vec!["uninstall".into(), id.into(), "--yes".into()],
                            ));
                        }
                        if let Some(path) = config(Path::new(&game.installations[0].path))
                            && let Ok(title) = config_title(&path)
                        {
                            game.title = title;
                        }
                        catalog::merge(&mut scan.games, game);
                    }
                    Ok(())
                });
            }
            let path = state.join("library.json");
            file(scan, &path, |scan| {
                let value = discovery::json(&path)?;
                let rows = value
                    .get("items")
                    .and_then(|v| v.as_array())
                    .ok_or("Unsupported Ferestre account cache")?;
                for row in rows {
                    if text(row, "productType") != Some("Game")
                        || text(row, "status") != Some("Active")
                    {
                        continue;
                    }
                    let Some(id) = text(row, "productId").filter(|s| store_id(s)) else {
                        continue;
                    };
                    let mut game = xbox(id, id, vec![], &epic::key(&path));
                    game.in_library = true;
                    game.client = "Ferestre".into();
                    catalog::add_os(&mut game.client_os, Os::Linux);
                    catalog::merge(&mut scan.games, game);
                }
                Ok(())
            });
        }
        for suffix in [
            ".config/xodus-nexus",
            ".config/Xodus Nexus",
            ".config/xodus-nexus-launcher",
        ] {
            let profile = home.path.join(suffix);
            let path = profile.join("launcher-state.json");
            file(scan, &path, |scan| nexus(scan, &home, &profile, &path));
        }
    }
    #[cfg(windows)]
    windows_packages(scan);
}

fn xbox(id: &str, title: &str, installs: Vec<Installation>, source: &str) -> LibraryGame {
    let mut game = catalog::entry("xbox", id, title, installs, source);
    game.client = "Xbox".into();
    if store_id(id) {
        game.store_page = Some(catalog::url(
            "Microsoft Store",
            format!("https://apps.microsoft.com/detail/{id}"),
        ));
    }
    game
}
fn store_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn config(folder: &Path) -> Option<PathBuf> {
    for name in ["MicrosoftGame.config", "MicrosoftGame.Config"] {
        let p = folder.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .take(512)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("MicrosoftGame.config"))
        })
}
fn config_title(path: &Path) -> Result<String, String> {
    let text = steam::read_text(path)?;
    let doc = roxmltree::Document::parse(&text).map_err(|_| "Invalid Xbox game config")?;
    doc.descendants()
        .find_map(|n| n.attribute("DefaultDisplayName"))
        .filter(|s| !s.starts_with("ms-resource:"))
        .map(str::to_owned)
        .ok_or("Xbox title needs Store metadata".into())
}
fn native_config(
    scan: &mut Scan<'_>,
    location: &Location,
    folder: &Path,
    path: &Path,
) -> Result<(), String> {
    let content = steam::read_text(path)?;
    let doc = roxmltree::Document::parse(&content).map_err(|_| "Invalid MicrosoftGame.config")?;
    let id = doc
        .descendants()
        .find(|n| n.has_tag_name("StoreId"))
        .and_then(|n| n.text())
        .filter(|s| store_id(s))
        .ok_or("Xbox package has no Store ID")?;
    let exe = doc
        .descendants()
        .find(|n| n.has_tag_name("Executable"))
        .and_then(|n| n.attribute("Name").or_else(|| n.attribute("File")))
        .unwrap_or("");
    if [
        "eadesktop",
        "ealink",
        "ubisoft",
        "upc.exe",
        "epicgameslauncher",
        "steam.exe",
    ]
    .iter()
    .any(|s| exe.to_lowercase().contains(s))
    {
        return Ok(());
    }
    let title = config_title(path).unwrap_or_else(|_| {
        folder
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .into()
    });
    let source = epic::key(path);
    let mut game = xbox(
        id,
        &title,
        vec![catalog::install(
            &source,
            folder.to_string_lossy().into(),
            Os::Windows,
            true,
        )],
        &source,
    );
    if location.local && cfg!(windows) {
        game.uninstall = Some(catalog::url("Windows", "ms-settings:appsfeatures".into()));
        if let Some(a) = &mut game.uninstall {
            a.note =
                Some("Windows will open Installed Apps to complete this game's uninstall.".into());
        }
    }
    catalog::merge(&mut scan.games, game);
    Ok(())
}

fn legacy_config(scan: &mut Scan<'_>, folder: &Path, path: &Path) -> Result<(), String> {
    let services = discovery::json(path)?;
    if services
        .get("TitleId")
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .is_none()
        || text(&services, "PrimaryServiceConfigId").is_none()
    {
        return Ok(());
    }
    let xml = steam::read_text(&folder.join("AppxManifest.xml"))?;
    let doc = roxmltree::Document::parse(&xml).map_err(|_| "Invalid Xbox UWP manifest")?;
    let identity = doc
        .descendants()
        .find(|n| n.has_tag_name("Identity"))
        .and_then(|n| n.attribute("Name"))
        .ok_or("UWP package has no identity")?;
    let title = doc
        .descendants()
        .find(|n| n.has_tag_name("DisplayName"))
        .and_then(|n| n.text())
        .filter(|s| !s.starts_with("ms-resource:"))
        .unwrap_or(identity);
    let source = epic::key(path);
    let id = format!("uwp:{identity}");
    let mut game = xbox(
        &id,
        title,
        vec![catalog::install(
            &source,
            folder.to_string_lossy().into(),
            Os::Windows,
            true,
        )],
        &source,
    );
    game.store_page = Some(catalog::url(
        "Microsoft Store",
        format!(
            "https://apps.microsoft.com/search?query={}",
            catalog::encode(title)
        ),
    ));
    if let Some(logo) = doc
        .descendants()
        .find_map(|n| n.attribute("Square150x150Logo"))
    {
        let path = folder.join(logo.replace('\\', "/"));
        if path.is_file() && path.starts_with(folder) {
            game.cover = Some(path.to_string_lossy().into());
        }
    }
    catalog::merge(&mut scan.games, game);
    Ok(())
}

fn nexus(scan: &mut Scan<'_>, home: &Location, profile: &Path, path: &Path) -> Result<(), String> {
    let data = discovery::json(path)?;
    let rows = data
        .get("games")
        .and_then(|v| v.as_array())
        .ok_or("Invalid Xodus Nexus state")?;
    for row in rows {
        let Some(id) = text(row, "storeId").filter(|s| store_id(s)) else {
            continue;
        };
        let title = text(row, "title").unwrap_or(id);
        let raw = text(row, "path").ok_or("Nexus game has no path")?;
        let path_value = discovery::resolve(raw, home, scan.report)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(raw.into());
        let install = catalog::install(
            &epic::key(path),
            path_value,
            Os::Linux,
            text(row, "installState") == Some("ready")
                && row.get("updateIncomplete").and_then(|v| v.as_bool()) != Some(true),
        );
        let mut game = xbox(id, title, vec![install], &epic::key(path));
        game.client = "Xodus Nexus".into();
        game.cover = image(row);
        if home.local && catalog::local(&game.installations) {
            let helper = profile.join("shortcuts/launch-game");
            let game_id = text(row, "id")
                .filter(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()));
            if let Some(game_id) = game_id {
                if helper.is_file() {
                    game.launch = Some(catalog::command(
                        "Xodus Nexus",
                        helper.to_string_lossy().into(),
                        vec![game_id.into()],
                    ));
                } else if let Some((program, mut args)) = discovery::desktop_command("xodus") {
                    args.extend([
                        "--nexus-profile".into(),
                        profile.to_string_lossy().into(),
                        "--launch-game".into(),
                        game_id.into(),
                    ]);
                    game.launch = Some(catalog::command("Xodus Nexus", program, args));
                }
            }
            let manage = profile.join("shortcuts/launch-nexus");
            if manage.is_file() {
                let mut a =
                    catalog::command("Xodus Nexus", manage.to_string_lossy().into(), vec![]);
                a.note=Some("Finish removing this game in Xodus Nexus. Saved-game prefixes stay under its control.".into());
                game.uninstall = Some(a);
            }
        }
        catalog::merge(&mut scan.games, game);
    }
    Ok(())
}

#[cfg(windows)]
fn windows_packages(scan: &mut Scan<'_>) {
    use std::os::windows::process::CommandExt;
    let script = r#"[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding
$start=@(Get-StartApps)
$rows=@(Get-AppxPackage | Where-Object { -not $_.IsFramework -and -not $_.IsResourcePackage } | ForEach-Object {
 $p=$_;$config=Join-Path $p.InstallLocation 'MicrosoftGame.config';$legacy=Join-Path $p.InstallLocation 'xboxservices.config'
 if((Test-Path $config) -or (Test-Path $legacy)){$a=$start|Where-Object{$_.AppID -like ($p.PackageFamilyName+'!*')}|Select-Object -First 1
 [PSCustomObject]@{path=$p.InstallLocation;config=$config;legacy=$legacy;app=$a.AppID;title=$a.Name;package=$p.PackageFullName}}
});ConvertTo-Json -Compress -InputObject $rows"#;
    if let Ok(out) = std::process::Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        && out.status.success()
        && let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
    {
        let source = "windows:xbox-packages";
        for row in rows {
            let (Some(raw), Some(config_path)) = (text(&row, "path"), text(&row, "config")) else {
                continue;
            };
            let location = Location {
                path: raw.into(),
                os: Os::Windows,
                local: true,
                client: "Xbox".into(),
                drive_c: None,
            };
            if Path::new(config_path).is_file() {
                let _ = native_config(scan, &location, Path::new(raw), Path::new(config_path));
            } else if let Some(path) = text(&row, "legacy") {
                let _ = legacy_config(scan, Path::new(raw), Path::new(path));
            }
            if let Some(game) = scan
                .games
                .iter_mut()
                .find(|g| g.store == "xbox" && g.installations.iter().any(|i| i.path == raw))
            {
                game.source = source.into();
                if let Some(app) = text(&row, "app") {
                    game.launch = Some(catalog::command(
                        "Xbox",
                        "explorer.exe".into(),
                        vec![format!("shell:AppsFolder\\{app}")],
                    ));
                }
                if let Some(package) = text(&row, "package").filter(|p| {
                    p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                }) {
                    let mut action = catalog::command(
                        "Xbox",
                        "powershell.exe".into(),
                        vec![
                            "-NoProfile".into(),
                            "-NonInteractive".into(),
                            "-Command".into(),
                            "Remove-AppxPackage -Package $env:EXPOSED_GAMES_PACKAGE_FULL_NAME -ErrorAction Stop".into(),
                        ],
                    );
                    if let ActionTarget::Command { env, .. } = &mut action.target {
                        env.insert("EXPOSED_GAMES_PACKAGE_FULL_NAME".into(), package.into());
                    }
                    game.uninstall = Some(action);
                }
                if let Some(title) = text(&row, "title") {
                    game.title = title.into();
                }
            }
        }
        scan.verified.insert(source.into());
    }
}

fn ferestre_ready(dir: &Path) -> bool {
    dir.join(".xodus-streaming.msixvc").is_file()
        || (["appxmanifest.xml", "AppxManifest.xml"]
            .iter()
            .any(|name| dir.join(name).is_file())
            && !dir.join(".xodus-streaming-tmp.msixvc").exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_ferestre_downloads_are_not_playable() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("MicrosoftGame.config"), "metadata only").unwrap();
        assert!(!ferestre_ready(dir.path()));
        fs::write(dir.path().join("AppxManifest.xml"), "manifest").unwrap();
        assert!(ferestre_ready(dir.path()));
        fs::write(dir.path().join(".xodus-streaming-tmp.msixvc"), "partial").unwrap();
        assert!(!ferestre_ready(dir.path()));
        fs::write(
            dir.path().join(".xodus-streaming.msixvc"),
            "finished header",
        )
        .unwrap();
        assert!(ferestre_ready(dir.path()));
    }
    #[test]
    fn uwp_games_need_xbox_services_identity_not_merely_an_app_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xboxservices.config");
        fs::write(dir.path().join("AppxManifest.xml"), r#"<Package><Identity Name="Fixture.Game"/><Properties><DisplayName>UWP fixture</DisplayName></Properties></Package>"#).unwrap();
        fs::write(
            &path,
            r#"{"TitleId":1234,"PrimaryServiceConfigId":"fixture"}"#,
        )
        .unwrap();
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
            homes: vec![],
            prefixes: vec![],
            roots: vec![],
            games: vec![],
            verified: BTreeSet::new(),
            warnings: vec![],
        };
        legacy_config(&mut scan, dir.path(), &path).unwrap();
        assert_eq!(scan.games[0].key, "xbox:uwp:Fixture.Game");
        assert_eq!(scan.games[0].installations[0].os, Os::Windows);
        assert!(scan.games[0].launch.is_none());
        scan.games.clear();
        fs::write(&path, "{}").unwrap();
        legacy_config(&mut scan, dir.path(), &path).unwrap();
        assert!(scan.games.is_empty());
    }
}
