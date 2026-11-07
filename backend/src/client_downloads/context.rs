use super::Client;
use crate::{
    discovery::{self, Location},
    model::*,
    runtime,
};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(super) struct Context {
    pub client: Client,
    pub root: PathBuf,
    pub prefix: Option<Location>,
}

impl Context {
    pub fn owns(&self, game: &LibraryGame, report: &Report) -> bool {
        if game.store != self.client.store() || !crate::catalog::visible(game) {
            return false;
        }
        if let Some(prefix) = &self.prefix {
            for action in [&game.install, &game.launch].into_iter().flatten() {
                if let ActionTarget::Command { env, .. } = &action.target
                    && let Some(path) = env.get("WINEPREFIX")
                {
                    return crate::epic::key(std::path::Path::new(path))
                        == crate::epic::key(&prefix.path);
                }
            }
        }
        self.prefix
            .as_ref()
            .is_none_or(|p| runtime::matches(p, game, report))
    }

    pub fn verified(&self, port: u16) -> bool {
        super::peer::verified(
            port,
            self.client,
            &self.root,
            self.prefix.as_ref().map(|p| p.path.as_path()),
        )
    }

    pub fn start(&self) -> Result<(), String> {
        let exe = self.root.join(self.client.executable());
        let args: Vec<String> = match self.client {
            Client::Ubisoft => vec!["-upc_stealth".into(), "-upc_enable_dev_ui_tools".into()],
            Client::Ea => vec!["--remote-debugging-port=9394".into()],
        };
        let action = if let Some(prefix) = &self.prefix {
            if prefix.client == "Bottles" {
                bottle(prefix, &exe, args)?
            } else {
                runtime::wine(prefix, &exe, args)
                    .ok_or("The configured client runner is unavailable")?
            }
        } else {
            crate::catalog::command(self.client.name(), exe.to_string_lossy().into_owned(), args)
        };
        crate::actions::dispatch(&action.target)
    }
}

pub(super) fn contexts(report: &Report) -> Vec<Context> {
    let mut result = Vec::new();
    #[cfg(windows)]
    for (client, variable, relative) in [
        (
            Client::Ea,
            "ProgramFiles",
            "Electronic Arts/EA Desktop/EA Desktop",
        ),
        (
            Client::Ubisoft,
            "ProgramFiles(x86)",
            "Ubisoft/Ubisoft Game Launcher",
        ),
    ] {
        if let Some(root) = std::env::var_os(variable).map(|p| PathBuf::from(p).join(relative))
            && root.join(client.executable()).is_file()
        {
            result.push(Context {
                client,
                root,
                prefix: None,
            });
        }
    }
    #[cfg(windows)]
    {
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        for process in system.processes().values() {
            let Some(exe) = process.exe() else { continue };
            for client in [Client::Ea, Client::Ubisoft] {
                if exe.file_name().is_some_and(|s| {
                    s.to_string_lossy()
                        .eq_ignore_ascii_case(client.executable())
                }) && let Some(root) = exe.parent()
                    && !result.iter().any(|c| {
                        c.client == client && crate::epic::key(&c.root) == crate::epic::key(root)
                    })
                {
                    result.push(Context {
                        client,
                        root: root.into(),
                        prefix: None,
                    });
                }
            }
        }
    }
    if cfg!(target_os = "linux") {
        for prefix in discovery::prefixes(report).into_iter().filter(|p| p.local) {
            for (client, relative) in [
                (
                    Client::Ea,
                    "Program Files/Electronic Arts/EA Desktop/EA Desktop",
                ),
                (
                    Client::Ubisoft,
                    "Program Files (x86)/Ubisoft/Ubisoft Game Launcher",
                ),
            ] {
                let root = prefix
                    .drive_c
                    .clone()
                    .unwrap_or_else(|| prefix.path.join("drive_c"))
                    .join(relative);
                let context = Context {
                    client,
                    root,
                    prefix: Some(prefix.clone()),
                };
                if context.root.join(client.executable()).is_file()
                    && report.catalog.iter().any(|g| context.owns(g, report))
                    && !result.iter().any(|c: &Context| {
                        c.client == client
                            && crate::epic::key(&c.root) == crate::epic::key(&context.root)
                    })
                {
                    result.push(context);
                }
            }
        }
    }
    result
}

pub(super) fn game<'a>(report: &'a Report, context: &Context, id: &str) -> Option<&'a LibraryGame> {
    let matches: Vec<_> = report
        .catalog
        .iter()
        .filter(|game| context.owns(game, report))
        .filter(|game| {
            game.id == id
                || context.client == Client::Ea
                    && report
                        .ea
                        .games
                        .iter()
                        .any(|g| g.id == game.id && g.aliases.iter().any(|alias| alias == id))
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

fn bottle(
    prefix: &Location,
    exe: &std::path::Path,
    args: Vec<String>,
) -> Result<GameAction, String> {
    let data = crate::steam::read_text(&prefix.path.join("bottle.yml"))?;
    let config: serde_json::Value =
        serde_yaml_ng::from_str(&data).map_err(|_| "Invalid bottle configuration")?;
    let name = config["Name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("The bottle has no name")?;
    let (program, mut values) = if prefix
        .path
        .to_string_lossy()
        .contains("com.usebottles.bottles")
    {
        (
            discovery::program("flatpak").ok_or("Flatpak is unavailable")?,
            vec![
                "run".into(),
                "--command=bottles-cli".into(),
                "com.usebottles.bottles".into(),
            ],
        )
    } else {
        (
            discovery::program("bottles-cli").ok_or("Bottles is unavailable")?,
            vec![],
        )
    };
    values.extend([
        "run".into(),
        "-b".into(),
        name.into(),
        "-e".into(),
        exe.to_string_lossy().into(),
    ]);
    values.push(format!("--args={}", args.join(" ")));
    Ok(crate::catalog::command("Bottles", program, values))
}
