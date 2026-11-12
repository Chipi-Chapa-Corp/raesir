use crate::model::*;
use std::process::{Command, Stdio};

pub fn perform(report: &Report, key: &str, kind: &str) -> Result<String, String> {
    let action = resolve(report, key, kind)?;
    dispatch(&action.target)?;
    Ok(action
        .note
        .unwrap_or_else(|| format!("Sent to {}", action.client)))
}

/// Preserve Steam's normal client routing while explicitly disabling a loader
/// for Play when mods were found in a separate profile or the game directory.
pub fn launch_with_args(report: &Report, key: &str, args: &[String]) -> Result<String, String> {
    let mut action = resolve(report, key, "launch")?;
    let game = report
        .catalog
        .iter()
        .find(|g| g.key == key)
        .ok_or("Game is no longer in the library")?;
    if game.store != "steam" || !matches!(action.target, ActionTarget::Url { .. }) {
        return Err("This client does not support launch arguments".into());
    }
    action.target = if cfg!(windows) {
        let installation = game
            .installations
            .iter()
            .find(|i| i.os == Os::host() && matches!(i.state.as_str(), "installed" | "external"))
            .ok_or("This game is not installed on this OS")?;
        let root = crate::runtime::steam_client_root(std::path::Path::new(&installation.library))
            .ok_or("Could not find Steam")?;
        let exe = root.join("steam.exe");
        if !exe.is_file() {
            return Err("Could not find Steam's executable".into());
        }
        let mut argv = vec!["-applaunch".into(), game.id.clone()];
        argv.extend_from_slice(args);
        crate::catalog::command("Steam", exe.to_string_lossy().into_owned(), argv).target
    } else {
        ActionTarget::Url {
            url: steam_launch_url(&game.id, args)?,
        }
    };
    dispatch(&action.target)?;
    Ok("Sent to Steam".into())
}

fn steam_launch_url(id: &str, args: &[String]) -> Result<String, String> {
    let args = shlex::try_join(args.iter().map(String::as_str)).map_err(|e| e.to_string())?;
    Ok(format!(
        "steam://run/{}//{}/",
        crate::catalog::encode(id),
        crate::catalog::encode(&args)
    ))
}

pub fn resolve(report: &Report, key: &str, kind: &str) -> Result<GameAction, String> {
    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some() {
        return Err("Client actions are disabled in this diagnostic profile".into());
    }
    let game = report
        .catalog
        .iter()
        .find(|g| g.key == key)
        .ok_or("Game is no longer in the library")?;
    let library = (kind == "library")
        .then(|| library_action(game))
        .transpose()?;
    let action = match kind {
        "launch" => &game.launch,
        "install" => &game.install,
        "store" => &game.store_page,
        "uninstall" => &game.uninstall,
        "library" => &library,
        _ => return Err("Unknown game action".into()),
    }
    .as_ref()
    .ok_or("This client does not expose that action here")?;
    if let ActionTarget::Steam {
        appid, operation, ..
    } = &action.target
        && (game.store != "steam"
            || game.id.parse::<u32>().ok() != Some(*appid)
            || operation.as_str() != kind)
    {
        return Err("This Steam action no longer matches the selected game".into());
    }
    if kind == "install" {
        check_install(game)?;
    }
    if ["launch", "uninstall"].contains(&kind) && !crate::catalog::local(&game.installations) {
        return Err("This game is not installed on this OS".into());
    }
    Ok(action.clone())
}

fn check_install(game: &LibraryGame) -> Result<(), String> {
    if !game.in_library || !game.client_os.contains(&Os::host()) {
        return Err("This game is not available to install on this OS".into());
    }
    if crate::catalog::local(&game.installations) {
        return Err("This game is already installed on this OS".into());
    }
    Ok(())
}

pub fn can_install(game: &LibraryGame) -> bool {
    game.install.is_some() && check_install(game).is_ok()
}

pub(crate) fn install_action(game: &LibraryGame) -> Result<GameAction, String> {
    check_install(game)?;
    if game.store == "steam" {
        return Ok(crate::catalog::url(
            "Steam",
            format!("steam://install/{}", crate::catalog::encode(&game.id)),
        ));
    }
    let mut action = library_action(game)?;
    action.note = Some(format!(
        "Select {} in {} to finish installing, then refresh your library.",
        game.title, action.client
    ));
    Ok(action)
}

fn library_action(game: &LibraryGame) -> Result<GameAction, String> {
    if !game.client_os.contains(&Os::host()) {
        return Err("This game's client library is on another OS".into());
    }
    if let Some(action) = &game.library {
        return Ok(action.clone());
    }
    let id = crate::catalog::encode(&game.id);
    let address = match game.store.as_str() {
        "steam" => format!("steam://nav/games/details/{id}"),
        "ubisoft" => format!("uplay://open/game/{id}"),
        "ea" => "origin2://library/open".into(),
        "epic" => "com.epicgames.launcher://store/library".into(),
        "xbox" => "ms-xbox://".into(),
        _ => return Err("This client does not expose its library".into()),
    };
    if let Some(mut action) = game.launch.as_ref().or(game.install.as_ref()).cloned()
        && let ActionTarget::Command { args, .. } = &mut action.target
        && let Some(arg) = args.iter_mut().find(|s| {
            ["origin2://", "uplay://", "com.epicgames.launcher://"]
                .iter()
                .any(|prefix| s.starts_with(prefix))
        })
    {
        *arg = address;
        return Ok(action);
    }
    if cfg!(windows) || game.store == "steam" {
        return Ok(crate::catalog::url(&game.client, address));
    }
    let desktop = match game.client.as_str() {
        "Heroic" => "heroic",
        "Epic" | "Epic Preview" => "epic",
        "Lutris" => "lutris",
        "Bottles" => "bottles",
        "Ferestre" => "ferestre",
        "Xodus Nexus" | "Nexus" => "nexus",
        _ => return Err("No client library navigation is available on this OS".into()),
    };
    let (program, args) = crate::discovery::desktop_command(desktop)
        .ok_or("The client could not be found on this OS")?;
    Ok(crate::catalog::command(&game.client, program, args))
}

pub fn dispatch(target: &ActionTarget) -> Result<(), String> {
    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some() {
        return Err("Client actions are disabled in this diagnostic profile".into());
    }
    let mut command = match target {
        ActionTarget::Steam { .. } => {
            return Err("Steam installation requests must be handled by the launcher".into());
        }
        ActionTarget::Url { url } => return open_url(url),
        ActionTarget::Command {
            program,
            args,
            env,
            cwd,
        } => {
            if program.trim().is_empty() || program.contains(['\0', '\r', '\n']) {
                return Err("Invalid client executable".into());
            }
            let mut command = Command::new(program);
            command.args(args).envs(env);
            if let Some(path) = cwd {
                command.current_dir(path);
            }
            command
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not open the client: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn open_url(url: &str) -> Result<(), String> {
    let scheme = url.split(':').next().unwrap_or("");
    if ![
        "https",
        "steam",
        "uplay",
        "origin",
        "origin2",
        "heroic",
        "com.epicgames.launcher",
        "ms-settings",
        "ms-xbox",
    ]
    .contains(&scheme)
        || url.contains(['\0', '\r', '\n'])
    {
        return Err("Unsupported client link".into());
    }
    #[cfg(target_os = "linux")]
    if let Some(result) = portal_open(url) {
        return result;
    }
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(windows))]
    let mut command = Command::new("xdg-open");
    command.arg(url);
    check_opener(&mut command)
}

fn check_opener(command: &mut Command) -> Result<(), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not open the link: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "The system could not open this link ({status}). Check your default browser or client association."
                ))
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn portal_open(url: &str) -> Option<Result<(), String>> {
    use zbus::blocking::{Proxy, connection::Builder};
    let connection = Builder::session()
        .ok()?
        .method_timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let portal = Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
    )
    .ok()?;
    portal.get_property::<u32>("version").ok()?;
    Some(portal_request(&connection, url))
}

#[cfg(target_os = "linux")]
fn portal_request(connection: &zbus::blocking::Connection, url: &str) -> Result<(), String> {
    use std::collections::HashMap;
    use zbus::{
        blocking::Proxy,
        zvariant::{OwnedObjectPath, OwnedValue, Value},
    };
    let sender = connection
        .unique_name()
        .ok_or("Desktop connection has no identity")?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/exposed_games");
    let request = Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .map_err(|e| e.to_string())?;
    let mut signals = request
        .receive_signal("Response")
        .map_err(|e| e.to_string())?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = signals
            .next()
            .ok_or("Desktop portal disconnected".to_owned())
            .and_then(|message| {
                message
                    .body()
                    .deserialize::<(u32, HashMap<String, OwnedValue>)>()
                    .map(|v| v.0)
                    .map_err(|e| e.to_string())
            });
        let _ = send.send(result);
    });
    let portal = Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
    )
    .map_err(|e| e.to_string())?;
    let options = HashMap::from([("handle_token", Value::from("exposed_games"))]);
    let opened: zbus::Result<OwnedObjectPath> = portal.call("OpenURI", &("", url, options));
    let result = match opened {
        Err(e) => Err(format!("Could not open the link through your desktop: {e}")),
        Ok(handle) if handle.as_str() != path => Err("Desktop portal returned an unexpected request".into()),
        Ok(_) => match receive.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(Ok(0)) => Ok(()),
            Ok(Ok(1)) => Err("Opening the link was cancelled".into()),
            Ok(Ok(_)) => Err("Your desktop could not open this link. Check its default browser or client association.".into()),
            Ok(Err(e)) => Err(e),
            Err(_) => { let _: zbus::Result<()> = request.call("Close", &()); Err("The desktop did not finish opening the link".into()) }
        }
    };
    let _ = connection.clone().close();
    result
}

pub fn support(project: &str) -> Result<(), String> {
    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some() {
        return Err("Client actions are disabled in this diagnostic profile".into());
    }
    let url = match project {
        "ferestre" => "https://github.com/icex/ferestre",
        "nexus" => "https://github.com/domsvoboda/xodus-nexus",
        _ => return Err("Unknown support project".into()),
    };
    dispatch(&ActionTarget::Url { url: url.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_launch_url_preserves_argument_boundaries_and_url_delimiters() {
        let args = vec![
            "--doorstop-enabled".into(),
            "false".into(),
            "path with spaces;and?query#fragment".into(),
            "quote'and\"literal".into(),
        ];
        let url = steam_launch_url("42", &args).unwrap();
        let encoded = url
            .strip_prefix("steam://run/42//")
            .unwrap()
            .strip_suffix('/')
            .unwrap();
        let parsed =
            reqwest::Url::parse(&format!("https://example.invalid/?argv={encoded}")).unwrap();
        let decoded = parsed.query_pairs().next().unwrap().1.into_owned();
        assert_eq!(shlex::split(&decoded).unwrap(), args);
    }

    #[test]
    fn native_steam_action_must_match_the_selected_game_and_operation() {
        let mut game = crate::catalog::entry("steam", "42", "Game", vec![], "fixture");
        game.in_library = true;
        game.client_os.push(Os::host());
        let mut report = Report {
            host_os: Os::host(),
            games: vec![],
            libraries: vec![],
            volumes: vec![],
            linux_systems: vec![],
            warnings: vec![],
            owned_account: None,
            owned_source: String::new(),
            epic: Default::default(),
            ea: Default::default(),
            catalog: vec![game],
            scanned_at: 0,
        };
        let key = report.catalog[0].key.clone();
        for (appid, operation, allowed) in [
            (43, crate::steam_control::Operation::Install, false),
            (42, crate::steam_control::Operation::Uninstall, false),
            (42, crate::steam_control::Operation::Install, true),
        ] {
            report.catalog[0].install = Some(GameAction {
                client: "Steam".into(),
                note: None,
                target: ActionTarget::Steam {
                    root: "/fixture".into(),
                    appid,
                    operation,
                },
            });
            assert_eq!(resolve(&report, &key, "install").is_ok(), allowed);
        }
    }

    #[test]
    fn explicit_wargaming_library_handoff_keeps_runner_and_checks_host_os() {
        let mut game = crate::catalog::entry(
            "wargaming",
            "WOT.EU.PRODUCTION",
            "World of Tanks",
            vec![],
            "fixture",
        );
        game.in_library = true;
        game.library = Some(crate::catalog::command(
            "Wargaming Game Center",
            "fixture-runner".into(),
            vec!["wgc_api.exe".into(), "--open".into()],
        ));
        assert!(library_action(&game).is_err());
        game.client_os.push(Os::host());
        assert!(
            matches!(library_action(&game).unwrap().target, ActionTarget::Command { program, args, .. }
            if program == "fixture-runner" && args == ["wgc_api.exe", "--open"])
        );
        game.install = Some(install_action(&game).unwrap());
        assert!(can_install(&game));
        game.client_os.clear();
        assert!(!can_install(&game));
        assert!(library_action(&game).is_err());
    }

    #[test]
    fn install_requires_local_membership_and_no_completed_local_install() {
        let mut game = crate::catalog::entry("steam", "42", "Game", vec![], "fixture");
        assert!(install_action(&game).is_err());
        game.in_library = true;
        game.client_os = vec![Os::Unknown];
        assert!(install_action(&game).is_err());
        let foreign = if Os::host() == Os::Linux {
            Os::Windows
        } else {
            Os::Linux
        };
        game.client_os = vec![foreign];
        assert!(install_action(&game).is_err());
        game.client_os.push(Os::host());
        game.install = Some(install_action(&game).unwrap());
        assert!(can_install(&game));
        assert!(matches!(&game.install.as_ref().unwrap().target,
            ActionTarget::Url { url } if url == "steam://install/42"));

        game.installations.push(Installation {
            library: "fixture".into(),
            path: "/fixture".into(),
            os: foreign,
            state: "installed".into(),
            last_seen: 0,
        });
        assert!(can_install(&game));
        game.installations[0].os = Os::host();
        for state in ["incomplete", "unavailable", "files_missing"] {
            game.installations[0].state = state.into();
            assert!(can_install(&game), "{state}");
        }
        for state in ["installed", "external"] {
            game.installations[0].state = state.into();
            assert!(!can_install(&game), "{state}");
            assert!(
                check_install(&game)
                    .unwrap_err()
                    .contains("already installed")
            );
        }
        game.installations.clear();
        game.in_library = false;
        assert!(!can_install(&game));
    }

    #[test]
    fn install_handoff_preserves_the_client_runtime_and_never_launches_the_game() {
        let mut game = crate::catalog::entry("ubisoft", "42", "Game", vec![], "fixture");
        game.in_library = true;
        game.client_os.push(Os::host());
        game.install = Some(crate::catalog::command(
            "Proton",
            "proton".into(),
            vec![
                "run".into(),
                "UbisoftConnect.exe".into(),
                "uplay://open/game/42".into(),
            ],
        ));
        if let ActionTarget::Command { env, cwd, .. } = &mut game.install.as_mut().unwrap().target {
            env.insert("WINEPREFIX".into(), "/fixture/prefix".into());
            *cwd = Some("/fixture".into());
        }
        let action = install_action(&game).unwrap();
        assert!(action.note.unwrap().contains("finish installing"));
        let ActionTarget::Command { args, env, cwd, .. } = action.target else {
            panic!("lost runtime")
        };
        assert_eq!(args, ["run", "UbisoftConnect.exe", "uplay://open/game/42"]);
        assert_eq!(env["WINEPREFIX"], "/fixture/prefix");
        assert_eq!(cwd.as_deref(), Some("/fixture"));
        assert!(game.launch.is_none());
    }

    #[test]
    fn library_navigation_never_launches_a_game_and_keeps_its_runtime() {
        let mut game = crate::catalog::entry("steam", "42", "Game", vec![], "fixture");
        game.client_os.push(Os::host());
        assert!(
            matches!(library_action(&game).unwrap().target, ActionTarget::Url { url } if url == "steam://nav/games/details/42")
        );
        game.store = "ubisoft".into();
        game.launch = Some(crate::catalog::command(
            "Proton",
            "proton".into(),
            vec![
                "run".into(),
                "UbisoftConnect.exe".into(),
                "uplay://launch/42".into(),
            ],
        ));
        assert!(
            matches!(library_action(&game).unwrap().target, ActionTarget::Command { args, .. } if args == ["run", "UbisoftConnect.exe", "uplay://open/game/42"])
        );
        game.client_os.clear();
        assert!(library_action(&game).unwrap_err().contains("another OS"));
    }
    #[test]
    fn untrusted_schemes_and_newlines_never_reach_a_shell() {
        for url in [
            "file:///tmp/program",
            "javascript:alert(1)",
            "https://example.com\ncommand",
        ] {
            assert!(dispatch(&ActionTarget::Url { url: url.into() }).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn opener_failures_reach_the_user_instead_of_reporting_success() {
        let mut failed = Command::new("/bin/sh");
        failed.args(["-c", "exit 42"]);
        assert!(
            check_opener(&mut failed)
                .unwrap_err()
                .contains("default browser")
        );
        let mut accepted = Command::new("/bin/sh");
        accepted.args(["-c", "exit 0"]);
        assert!(check_opener(&mut accepted).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_portal_opens_exact_store_url_and_reports_cancellation_and_failure() {
        use std::{
            collections::HashMap,
            io::{BufRead, BufReader},
            sync::{
                Arc,
                atomic::{AtomicU32, Ordering},
            },
        };
        use zbus::{
            blocking::connection::Builder,
            zvariant::{OwnedObjectPath, OwnedValue},
        };
        struct Daemon(std::process::Child);
        impl Drop for Daemon {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut daemon = Daemon(
            Command::new("/usr/bin/dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let response = Arc::new(AtomicU32::new(0));
        struct Portal {
            response: Arc<AtomicU32>,
        }
        #[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
        impl Portal {
            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                3
            }
            #[zbus(name = "OpenURI")]
            async fn open_uri(
                &self,
                parent: &str,
                uri: &str,
                options: HashMap<String, OwnedValue>,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] connection: &zbus::Connection,
            ) -> zbus::fdo::Result<OwnedObjectPath> {
                assert_eq!(parent, "");
                assert_eq!(
                    uri,
                    "https://store.steampowered.com/app/10/?a=1&b=two%20words"
                );
                assert_eq!(
                    options
                        .get("handle_token")
                        .and_then(|v| <&str>::try_from(v).ok()),
                    Some("exposed_games")
                );
                let sender = header
                    .sender()
                    .unwrap()
                    .as_str()
                    .trim_start_matches(':')
                    .replace('.', "_");
                let path =
                    format!("/org/freedesktop/portal/desktop/request/{sender}/exposed_games");
                connection
                    .emit_signal(
                        None::<&str>,
                        path.as_str(),
                        "org.freedesktop.portal.Request",
                        "Response",
                        &(
                            self.response.load(Ordering::SeqCst),
                            HashMap::<String, OwnedValue>::new(),
                        ),
                    )
                    .await
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
                Ok(OwnedObjectPath::try_from(path).unwrap())
            }
        }
        let _server = Builder::address(address.trim())
            .unwrap()
            .name("org.freedesktop.portal.Desktop")
            .unwrap()
            .serve_at(
                "/org/freedesktop/portal/desktop",
                Portal {
                    response: response.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        for code in [0, 1, 2] {
            response.store(code, Ordering::SeqCst);
            let client = Builder::address(address.trim()).unwrap().build().unwrap();
            let result = portal_request(
                &client,
                "https://store.steampowered.com/app/10/?a=1&b=two%20words",
            );
            match code {
                0 => assert!(result.is_ok(), "{result:?}"),
                1 => assert!(result.unwrap_err().contains("cancelled")),
                _ => assert!(result.unwrap_err().contains("default browser")),
            }
        }
    }
}
