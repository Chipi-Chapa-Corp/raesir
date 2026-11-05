//! Launch a native companion in its owning Steam session, then let its game
//! handoff use the prefix's Proton and that tool's declared runtime chain.
use super::*;
use crate::{discovery, model::Installation, steam, steam_control, vdf};
use std::process::{Command, Output, Stdio};

const HELPER: &str = include_str!("steam_companion.py");

#[derive(Debug, Serialize)]
struct Plan {
    root: PathBuf,
    game_dir: PathBuf,
    compat_data: PathBuf,
    libraries: Vec<PathBuf>,
    tools: Vec<PathBuf>,
    executables: Vec<PathBuf>,
    game_command: Vec<String>,
    proxy: PathBuf,
    proxy_args: Vec<String>,
    runtime: PathBuf,
}

fn text_vdf(path: &Path) -> Result<vdf::Value, String> {
    vdf::parse(&steam::read_text(path)?)
}

fn app_dir(root: &Path, id: u32) -> Result<PathBuf, String> {
    let library = steam_control::app_library(root, id)
        .ok_or_else(|| format!("Install Steam compatibility tool {id} in Steam, then retry"))?;
    let manifest = text_vdf(&library.join(format!("steamapps/appmanifest_{id}.acf")))?;
    let folder = manifest
        .get("AppState")
        .and_then(|v| v.field("installdir"))
        .ok_or("Steam compatibility tool manifest has no installation directory")?;
    let relative = Path::new(folder);
    if relative.components().count() != 1
        || !matches!(
            relative.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err("Invalid Steam compatibility tool installation directory".into());
    }
    library
        .join("steamapps/common")
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())
}

fn mapping(root: &Path) -> Option<String> {
    let config = text_vdf(&root.join("config/config.vdf")).ok()?;
    let mut node = &config;
    for key in [
        "InstallConfigStore",
        "Software",
        "Valve",
        "Steam",
        "CompatToolMapping",
        "881100",
    ] {
        node = node.get(key)?;
    }
    node.field("name")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn official_tool_id(apps: &steam_vdf_parser::Obj<'_>, name: &str) -> Option<u32> {
    let entries = apps
        .iter()
        .filter_map(|(_, value)| {
            value
                .get("appinfo")
                .unwrap_or(value)
                .get("extended")?
                .get("compat_tools")?
                .as_obj()
        })
        .flat_map(|tools| tools.iter())
        .collect::<Vec<_>>();
    // Exact names win over aliases, which can overlap across architectures.
    if let Some((_, entry)) = entries.iter().find(|(key, _)| key.as_ref() == name) {
        return entry.get_str(&["appid"])?.parse().ok();
    }
    let ids = entries
        .iter()
        .filter(|(_, tool)| {
            tool.get_str(&["aliases"])
                .is_some_and(|s| s.split(',').any(|alias| alias.trim() == name))
        })
        .filter_map(|(_, tool)| tool.get_str(&["appid"])?.parse::<u32>().ok())
        .collect::<std::collections::HashSet<_>>();
    (ids.len() == 1).then(|| *ids.iter().next().unwrap())
}

fn proton(root: &Path, compat_data: &Path) -> Result<PathBuf, String> {
    // Honor an explicit per-game override ahead of the previously used prefix.
    if let Some(name) = mapping(root) {
        for base in std::iter::once(root.join("compatibilitytools.d")).chain(
            steam_control::libraries(root)
                .into_iter()
                .map(|(_, p)| p.join("steamapps/common")),
        ) {
            for dir in discovery::children(&base) {
                if let Ok(v) = text_vdf(&dir.join("compatibilitytool.vdf"))
                    && let Some(tool) = v
                        .get("compatibilitytools")
                        .and_then(|v| v.get("compat_tools"))
                        .and_then(|v| v.get(&name))
                {
                    let relative = Path::new(tool.field("install_path").unwrap_or("."));
                    if relative.is_absolute()
                        || relative
                            .components()
                            .any(|c| c == std::path::Component::ParentDir)
                    {
                        return Err("Invalid Proton install path".into());
                    }
                    let path = dir.join(relative);
                    if path.join("proton").is_file() {
                        return Ok(path);
                    }
                }
                if let Ok(v) = text_vdf(&dir.join("toolmanifest.vdf"))
                    && v.get("manifest")
                        .and_then(|m| m.get("compat_tools"))
                        .and_then(|t| t.get(&name))
                        .is_some()
                    && dir.join("proton").is_file()
                {
                    return Ok(dir);
                }
            }
        }
        // Steam's official tools need not ship compatibilitytool.vdf. Read the
        // installed appinfo's internal tool name, never guess a folder/version.
        let cache = root.join("appcache/appinfo.vdf");
        if let Ok(file) = fs::File::open(cache) {
            let mut bytes = Vec::new();
            file.take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() <= 64 * 1024 * 1024 {
                bytes.resize(bytes.len() + 68, 0);
                if let Ok(apps) = steam_vdf_parser::parse_appinfo(&bytes)
                    && let Some(apps) = apps.as_obj()
                    && let Some(id) = official_tool_id(apps, &name)
                {
                    let path = app_dir(root, id)?;
                    if path.join("proton").is_file() {
                        return Ok(path);
                    }
                }
            }
        }
        return Err(format!(
            "Could not resolve the selected Proton tool {name}. Launch Noita once in Steam with that tool, then retry"
        ));
    }
    let info = steam::read_text(&compat_data.join("config_info"))
        .map_err(|_| "Launch Noita once in Steam to establish its Proton prefix")?;
    for line in info.lines().filter(|l| l.starts_with('/')) {
        for dir in Path::new(line).ancestors().take(8) {
            if dir.join("proton").is_file() {
                return dir.canonicalize().map_err(|e| e.to_string());
            }
        }
    }
    Err(
        "The Proton recorded for Noita is not installed. Launch Noita once in Steam, then retry"
            .into(),
    )
}

fn tool_chain(
    root: &Path,
    tool: PathBuf,
    tools: &mut Vec<PathBuf>,
    executables: &mut Vec<PathBuf>,
    tail: Vec<String>,
) -> Result<Vec<String>, String> {
    if tools.len() >= 8 || tools.contains(&tool) {
        return Err("Steam compatibility tool dependency cycle".into());
    }
    tools.push(tool.clone());
    let value = text_vdf(&tool.join("toolmanifest.vdf"))?;
    let manifest = value
        .get("manifest")
        .ok_or("Invalid Steam compatibility tool manifest")?;
    let line = manifest
        .field("commandline")
        .ok_or("Steam tool manifest has no commandline")?;
    let mut command = shlex::split(line).ok_or("Invalid Steam tool commandline")?;
    let Some(program) = command.first_mut() else {
        return Err("Empty Steam tool commandline".into());
    };
    let relative = Path::new(program.trim_start_matches('/'));
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err("Invalid Steam tool executable".into());
    }
    let executable = tool.join(relative);
    if !executable.is_file() {
        return Err(format!(
            "Steam tool is incomplete: {}. Repair it in Steam",
            executable.display()
        ));
    }
    *program = executable.to_string_lossy().into_owned();
    executables.push(executable);
    for argument in command.iter_mut().skip(1) {
        *argument = argument.replace("%verb%", "run");
    }
    command.extend(tail);
    match manifest.field("require_tool_appid").filter(|s| *s != "0") {
        Some(id) => tool_chain(
            root,
            app_dir(
                root,
                id.parse()
                    .map_err(|_| "Invalid required Steam runtime app ID")?,
            )?,
            tools,
            executables,
            command,
        ),
        None => Ok(command),
    }
}

fn plan(
    root: &Path,
    install: &Installation,
    proxy: &Path,
    runtime: &Path,
    proxy_args: &[String],
    game_args: &[String],
) -> Result<Plan, String> {
    let game_dir = Path::new(&install.path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let library = Path::new(&install.library)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let compat_data = [
        library.join("compatdata/881100"),
        root.join("steamapps/compatdata/881100"),
    ]
    .into_iter()
    .find(|p| p.join("config_info").is_file())
    .ok_or("Launch Noita once in Steam to establish its Proton prefix")?;
    let mut tools = Vec::new();
    let mut executables = Vec::new();
    let mut tail = vec![game_dir.join("noita.exe").to_string_lossy().into_owned()];
    tail.extend_from_slice(game_args);
    let game_command = tool_chain(
        root,
        proton(root, &compat_data)?,
        &mut tools,
        &mut executables,
        tail,
    )?;
    Ok(Plan {
        root: root.into(),
        game_dir,
        compat_data,
        tools,
        game_command,
        executables,
        libraries: steam_control::libraries(root)
            .into_iter()
            .map(|(_, p)| p.join("steamapps"))
            .collect(),
        proxy: proxy.into(),
        proxy_args: proxy_args.into(),
        runtime: runtime.into(),
    })
}

fn flatpak() -> Result<Command, String> {
    let mut command = if Path::new("/run/.containerenv").exists()
        && let Some(host) = discovery::program("distrobox-host-exec")
    {
        let mut c = Command::new(host);
        c.arg("flatpak");
        c
    } else if Path::new("/.flatpak-info").exists() {
        let mut c = Command::new(
            discovery::program("flatpak-spawn").ok_or("Flatpak host access is unavailable")?,
        );
        c.args(["--host", "flatpak"]);
        c
    } else {
        Command::new(discovery::program("flatpak").ok_or("Flatpak is not installed")?)
    };
    command.stdin(Stdio::null());
    Ok(command)
}

fn output(mut command: Command) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not inspect the Steam session: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return child.wait_with_output().map_err(|e| e.to_string());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Inspecting the Steam session timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Probe without opening a proxy or game. Never start a second Steam sandbox.
fn session(root: &Path, helper: &Path, plan: &Path) -> Result<Command, String> {
    session_with(root, helper, plan, &flatpak)
}

fn session_with(
    root: &Path,
    helper: &Path,
    plan: &Path,
    flatpak: &impl Fn() -> Result<Command, String>,
) -> Result<Command, String> {
    let flatpak_client = root
        .components()
        .any(|c| c.as_os_str() == "com.valvesoftware.Steam")
        || crate::platform::local_sources().iter().any(|s| {
            Path::new(&s.path)
                .components()
                .any(|c| c.as_os_str() == "com.valvesoftware.Steam")
                && Path::new(&s.path).canonicalize().ok().as_deref() == Some(root)
        });
    if !flatpak_client {
        let mut command = Command::new("python3");
        command.arg(helper).arg("probe").arg(plan);
        let result = output(command)?;
        if !result.status.success()
            || String::from_utf8_lossy(&result.stdout).trim() != "steam-session-ready"
        {
            return Err(format!(
                "Steam session check failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let mut command = Command::new("python3");
        command.arg(helper).arg("proxy").arg(plan);
        return Ok(command);
    }
    let mut list = flatpak()?;
    list.args(["ps", "--columns=instance,application"]);
    let result = output(list)?;
    if !result.status.success() {
        return Err("Could not list running Flatpak applications. Start Steam and retry".into());
    }
    let mut reason = String::new();
    for line in String::from_utf8_lossy(&result.stdout).lines() {
        let cols = line.split_whitespace().collect::<Vec<_>>();
        if cols.len() != 2
            || cols[1] != "com.valvesoftware.Steam"
            || !cols[0].bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let mut probe = flatpak()?;
        probe
            .args(["enter", cols[0], "/usr/bin/python3"])
            .arg(helper)
            .arg("probe")
            .arg(plan);
        let result = output(probe)?;
        if result.status.success()
            && String::from_utf8_lossy(&result.stdout).trim() == "steam-session-ready"
        {
            let mut command = flatpak()?;
            command
                .args(["enter", cols[0], "/usr/bin/python3"])
                .arg(helper)
                .arg("proxy")
                .arg(plan);
            return Ok(command);
        }
        reason = String::from_utf8_lossy(&result.stderr)
            .trim()
            .chars()
            .take(1000)
            .collect();
    }
    Err(format!(
        "No matching Flatpak Steam session is ready. Start Steam and retry. {reason}"
    ))
}

pub(super) fn prepare(
    install: &Installation,
    proxy: &Path,
    runtime: &Path,
    proxy_args: &[String],
    game_args: &[String],
) -> Result<Command, String> {
    // A real Steam session carries the user's display/bus. GUI diagnostics must
    // use fixtures instead of forwarding a proxy onto that desktop.
    if std::env::var_os("ISOLATED_GUI_SESSION").is_some() {
        return Err(
            "Real Steam companions cannot be launched from a private GUI diagnostic".into(),
        );
    }
    let root = crate::runtime::steam_client_root(Path::new(&install.library))
        .ok_or("Could not locate the owning Steam client")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let proxy = proxy.canonicalize().map_err(|e| e.to_string())?;
    let runtime = runtime.canonicalize().map_err(|e| e.to_string())?;
    let plan = plan(&root, install, &proxy, &runtime, proxy_args, game_args)?;
    let helper = runtime.join("steam-companion.py");
    fs::write(&helper, HELPER).map_err(|e| e.to_string())?;
    let file = runtime.join("launch.json");
    crate::storage::write_json(&file, &plan)?;
    let wrapper = runtime.join("launch-noita.sh");
    let contents = format!(
        "#!/bin/sh\nexec /usr/bin/python3 {} game {} \"$@\"\n",
        shlex::try_quote(helper.to_str().ok_or("Invalid helper path")?)
            .map_err(|e| e.to_string())?,
        shlex::try_quote(file.to_str().ok_or("Invalid plan path")?).map_err(|e| e.to_string())?
    );
    fs::write(&wrapper, contents).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    session(&root, &helper, &file)
}

#[cfg(test)]
mod tests;
