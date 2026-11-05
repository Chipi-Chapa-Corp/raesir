use crate::{
    catalog,
    discovery::{self, Location},
    model::*,
    platform, steam,
};
use std::path::{Path, PathBuf};

pub fn wine(prefix: &Location, exe: &Path, args: Vec<String>) -> Option<GameAction> {
    if !prefix.local || !exe.is_file() {
        return None;
    }
    let mut program = discovery::program("wine");
    let mut proton = None;
    let mut steam_root = None;
    if prefix.client == "Proton"
        && let Some(apps) = prefix
            .path
            .ancestors()
            .find(|p| p.file_name().is_some_and(|s| s == "steamapps"))
    {
        steam_root = steam_client_root(apps);
        proton = remembered_proton(&prefix.path).or_else(|| {
            discovery::children(&apps.join("common"))
                .into_iter()
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("Proton"))
                })
                .map(|p| p.join("proton"))
                .find(|p| p.is_file())
        });
    }
    if let Some(home) = platform::home() {
        for root in [
            home.join(".config/heroic"),
            home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
        ] {
            for file in std::fs::read_dir(root.join("GamesConfig"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .take(512)
            {
                let Ok(value) = discovery::json(&file.path()) else {
                    continue;
                };
                for row in value.as_object().into_iter().flat_map(|m| m.values()) {
                    if row
                        .get("winePrefix")
                        .and_then(|v| v.as_str())
                        .is_some_and(|p| {
                            crate::epic::key(Path::new(p)) == crate::epic::key(&prefix.path)
                        })
                        && let Some(bin) = row.pointer("/wineVersion/bin").and_then(|v| v.as_str())
                    {
                        if row.pointer("/wineVersion/type").and_then(|v| v.as_str())
                            == Some("proton")
                        {
                            proton = Some(PathBuf::from(bin));
                        } else if Path::new(bin).is_file() {
                            program = Some(bin.into());
                        }
                    }
                }
            }
        }
    }
    let mut extra_env = std::collections::BTreeMap::new();
    if prefix.client == "Lutris" {
        let (configured, env) = lutris_runner(&prefix.path)?;
        program = Some(configured);
        extra_env = env;
    }
    let mut values = vec![exe.to_string_lossy().into_owned()];
    values.extend(args);
    let mut action = if let Some(proton) = proton {
        if let Some(umu) = discovery::program("umu-run") {
            let mut action = catalog::command("Proton", umu, values);
            if let ActionTarget::Command { env, .. } = &mut action.target {
                env.insert(
                    "PROTONPATH".into(),
                    proton.parent()?.to_string_lossy().into(),
                );
                env.insert("GAMEID".into(), "umu-default".into());
            }
            action
        } else if prefix.client == "Proton" {
            values.insert(0, "run".into());
            let mut action = catalog::command("Proton", proton.to_string_lossy().into(), values);
            if let ActionTarget::Command { env, .. } = &mut action.target {
                env.insert(
                    "STEAM_COMPAT_DATA_PATH".into(),
                    prefix.path.parent()?.to_string_lossy().into(),
                );
                if let Some(root) = steam_root {
                    env.insert(
                        "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                        root.to_string_lossy().into(),
                    );
                }
            }
            action
        } else {
            return None;
        }
    } else {
        catalog::command(&prefix.client, program?, values)
    };
    if let ActionTarget::Command { env, .. } = &mut action.target {
        env.extend(extra_env);
        env.insert("WINEPREFIX".into(), prefix.path.to_string_lossy().into());
    }
    Some(action)
}

pub(crate) fn steam_client_root(apps: &Path) -> Option<PathBuf> {
    let canonical_apps = apps.canonicalize().ok()?;
    apps.parent()
        .filter(|p| p.join("linux64/steamclient.so").is_file())
        .map(Path::to_path_buf)
        .or_else(|| {
            platform::local_sources()
                .into_iter()
                .map(|s| PathBuf::from(s.path))
                .find(|p| {
                    crate::steam_control::libraries(p)
                        .iter()
                        .any(|(_, library)| {
                            library.join("steamapps").canonicalize().ok().as_ref()
                                == Some(&canonical_apps)
                        })
                })
        })
        .or_else(|| apps.parent().map(Path::to_path_buf))
}

pub fn matches(prefix: &Location, game: &LibraryGame, report: &Report) -> bool {
    if prefix.local && !catalog::local(&game.installations) {
        let sources = match game.store.as_str() {
            "ea" => report
                .ea
                .games
                .iter()
                .find(|g| g.id == game.id)
                .map(|g| &g.library_sources),
            "epic" => report
                .epic
                .games
                .iter()
                .find(|g| g.app_name == game.id)
                .map(|g| &g.library_sources),
            _ => None,
        };
        if Path::new(&game.source).starts_with(&prefix.path)
            || sources.is_some_and(|sources| {
                sources
                    .iter()
                    .any(|source| Path::new(source).starts_with(&prefix.path))
            })
        {
            return true;
        }
    }
    if game.installations.iter().any(|i| {
        i.os == Os::host()
            && (Path::new(&i.path).starts_with(&prefix.path)
                || Path::new(&i.library).starts_with(&prefix.path))
    }) {
        return true;
    }
    let ini = prefix
        .path
        .join("drive_c/ProgramData/EA Desktop/machine.ini");
    if let Ok(text) = steam::read_text(&ini) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=')
                && key.trim().eq_ignore_ascii_case("downloadinplacedir")
                && let Some(path) =
                    discovery::resolve(value.trim().trim_matches('"'), prefix, report)
                && game
                    .installations
                    .iter()
                    .any(|i| i.os == Os::host() && Path::new(&i.path).starts_with(&path))
            {
                return true;
            }
        }
    }
    false
}

fn remembered_proton(prefix: &Path) -> Option<PathBuf> {
    let text = steam::read_text(&prefix.parent()?.join("config_info")).ok()?;
    for line in text.lines().filter(|s| s.starts_with('/')) {
        for parent in Path::new(line).ancestors().take(6) {
            let script = parent.join("proton");
            if script.is_file() {
                return Some(script);
            }
        }
    }
    None
}

fn lutris_runner(prefix: &Path) -> Option<(String, std::collections::BTreeMap<String, String>)> {
    let home = platform::home()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    for file in std::fs::read_dir(config.join("lutris/games"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(2048)
    {
        let Ok(text) = steam::read_text(&file.path()) else {
            continue;
        };
        let Ok(value) = serde_yaml_ng::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(raw) = value.pointer("/game/prefix").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = raw
            .strip_prefix("~/")
            .map(|p| home.join(p))
            .unwrap_or_else(|| PathBuf::from(raw));
        if crate::epic::key(&path) != crate::epic::key(prefix) {
            continue;
        }
        let version = value.pointer("/wine/version").and_then(|v| v.as_str())?;
        let program = match version {
            "system" => discovery::program("wine")?,
            "custom" => value
                .pointer("/wine/custom_wine_path")
                .and_then(|v| v.as_str())?
                .into(),
            name if !name.contains(['/', '\\']) && !name.contains("..") => data
                .join("lutris/runners/wine")
                .join(name)
                .join("bin/wine")
                .to_string_lossy()
                .into(),
            _ => return None,
        };
        if !Path::new(&program).is_file() {
            return None;
        }
        let env = value
            .pointer("/system/env")
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
            .collect();
        return Some((program, env));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstalled_games_match_their_local_client_profile_without_changing_installed_routing() {
        let mut report = crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        let mut prefix = Location {
            path: "/fixture/prefix".into(),
            os: Os::host(),
            local: true,
            client: "Proton".into(),
            drive_c: None,
        };
        let mut game = catalog::entry(
            "ubisoft",
            "42",
            "Game",
            vec![],
            "/fixture/prefix/drive_c/users/user/cache/configurations",
        );
        assert!(matches(&prefix, &game, &report));
        prefix.local = false;
        assert!(!matches(&prefix, &game, &report));
        prefix.local = true;
        game.store = "epic".into();
        game.source = "epic".into();
        report.epic.games.push(EpicGame {
            app_name: game.id.clone(),
            title: game.title.clone(),
            installations: vec![],
            library_sources: vec!["/fixture/prefix/drive_c/profile".into()],
        });
        assert!(matches(&prefix, &game, &report));
        game.installations.push(Installation {
            library: "/another/prefix/profile".into(),
            path: "/another/prefix/game".into(),
            os: Os::host(),
            state: "installed".into(),
            last_seen: 0,
        });
        assert!(!matches(&prefix, &game, &report));
    }

    #[test]
    fn proton_reuses_the_runner_recorded_by_the_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let runner = dir.path().join("selected-proton");
        std::fs::create_dir_all(runner.join("files/share/fonts")).unwrap();
        std::fs::write(runner.join("proton"), "fixture, never executed").unwrap();
        let compat = dir.path().join("compatdata/42");
        std::fs::create_dir_all(compat.join("pfx")).unwrap();
        std::fs::write(
            compat.join("config_info"),
            format!("version\n{}\n", runner.join("files/share/fonts").display()),
        )
        .unwrap();
        assert_eq!(
            remembered_proton(&compat.join("pfx")),
            Some(runner.join("proton"))
        );
        std::fs::remove_file(runner.join("proton")).unwrap();
        assert_eq!(remembered_proton(&compat.join("pfx")), None);
    }
}
