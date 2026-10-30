use crate::model::*;
use std::process::{Command, Stdio};

pub fn perform(report: &Report, key: &str, kind: &str) -> Result<String, String> {
    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
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
        "store" => &game.store_page,
        "uninstall" => &game.uninstall,
        "library" => &library,
        _ => return Err("Unknown game action".into()),
    }
    .as_ref()
    .ok_or("This client does not expose that action here")?;
    if ["launch", "uninstall"].contains(&kind) && !crate::catalog::local(&game.installations) {
        return Err("This game is not installed on this OS".into());
    }
    dispatch(&action.target)?;
    Ok(action
        .note
        .clone()
        .unwrap_or_else(|| format!("Sent to {}", action.client)))
}

fn library_action(game: &LibraryGame) -> Result<GameAction, String> {
    if !game.client_os.contains(&Os::host()) {
        return Err("This game's client library is on another OS".into());
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
    if let Some(mut action) = game.launch.clone()
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

fn dispatch(target: &ActionTarget) -> Result<(), String> {
    let mut command = match target {
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
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/raesir");
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
    let options = HashMap::from([("handle_token", Value::from("raesir"))]);
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
    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some() {
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
                    Some("raesir")
                );
                let sender = header
                    .sender()
                    .unwrap()
                    .as_str()
                    .trim_start_matches(':')
                    .replace('.', "_");
                let path = format!("/org/freedesktop/portal/desktop/request/{sender}/raesir");
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
