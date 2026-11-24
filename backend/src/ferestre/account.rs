use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    process::Stdio,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

const CLIENT_NOT_FOUND: &str =
    "Could not find the Xbox helper. Use Xbox support in Settings to install or repair it.";
static SIGNED_OUT_CACHE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Roles {
    store: String,
    player: String,
    generation: u64,
}

fn profile_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn config_dir(home: &Path) -> PathBuf {
    host_dir(home, "XDG_CONFIG_HOME", ".config").join("ferestre")
}

fn roles_path(config: &Path) -> PathBuf {
    config.join("account-roles.json")
}

fn roles(config: &Path) -> Result<Option<Roles>, String> {
    let path = roles_path(config);
    if !path.try_exists().map_err(|error| {
        format!(
            "Could not read Xbox account settings at {} ({error})",
            path.display()
        )
    })? {
        return Ok(None);
    }
    let text = crate::steam::read_text(&path)
        .map_err(|error| format!("Could not read Xbox account settings ({error})"))?;
    let roles: Roles = serde_json::from_str(&text).map_err(|error| {
        format!(
            "Invalid Xbox account settings at {} (line {}, column {})",
            path.display(),
            error.line(),
            error.column(),
        )
    })?;
    if !profile_id(&roles.store) || !profile_id(&roles.player) {
        return Err(format!(
            "Invalid Xbox profile selection at {}",
            path.display()
        ));
    }
    Ok(Some(roles))
}

pub(crate) fn library_cache_path(state: &Path, config: &Path) -> Result<PathBuf, String> {
    Ok(cache_path(state, roles(config)?.as_ref()))
}

fn cache_path(state: &Path, roles: Option<&Roles>) -> PathBuf {
    match roles {
        Some(roles) => state
            .join("accounts")
            .join(&roles.store)
            .join("library.json"),
        None => state.join("library.json"),
    }
}

pub(crate) fn library_cached(home: &Path) -> bool {
    library_cache_path(&state_dir(home), &config_dir(home))
        .is_ok_and(|path| !library_signed_out(&path) && path.is_file())
}

pub(crate) fn library_signed_out(path: &Path) -> bool {
    SIGNED_OUT_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_deref()
        == Some(path)
}

pub fn sign_in() -> Result<(), String> {
    check_setup_enabled()?;
    let client = xodus_client()?;
    sign_in_preflight(&client)?;
    let home = platform::home().ok_or("Could not find Xbox account storage")?;
    let config = config_dir(&home);
    let previous = roles(&config)?;
    let message = "Xbox sign-in did not complete. Open Sign in again to retry.";
    // Xodus renews an existing profile under the same account identity when
    // add-account signs it in again. Profile-scoped login is refused by Xodus.
    let mut command = host_command(&client);
    command.arg("add-account");
    let result = query(command, Duration::from_secs(600), message)?;
    let id = result["profile"]["id"]
        .as_str()
        .filter(|id| profile_id(id))
        .ok_or("Xbox sign-in returned an invalid account")?;
    if roles(&config)? != previous {
        return Err(
            "The Xbox account changed during sign-in. Refresh the library and retry.".into(),
        );
    }
    let player = previous
        .as_ref()
        .filter(|roles| roles.store == id)
        .map_or(id, |roles| roles.player.as_str());
    let generation = previous
        .as_ref()
        .map_or(Some(1), |roles| roles.generation.checked_add(1))
        .ok_or("Could not save the new Xbox account selection")?;
    let selected = Roles {
        store: id.into(),
        player: player.into(),
        generation,
    };
    storage::write_json(&roles_path(&config), &selected)?;
    fetch_library(&client, &state_dir(&home), Some(&selected))
}

pub fn refresh_library() -> Result<(), String> {
    let (Some(client), Some(home)) = (Client::discover(), platform::home()) else {
        return Ok(());
    };
    if check_setup_enabled().is_err() {
        return Ok(());
    }
    let state = state_dir(&home);
    let roles = roles(&config_dir(&home))?;
    if client.managed && roles.is_none() && !cache_path(&state, None).is_file() {
        return Ok(());
    }
    let client = resolve_xodus_client(&client)?;
    if selected_profile_signed_out(&client, roles.as_ref()) {
        *SIGNED_OUT_CACHE
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(cache_path(&state, roles.as_ref()));
        return Err(
            "Your Xbox account is signed out. Open Xbox account in Settings to sign in again."
                .into(),
        );
    }
    fetch_library(&client, &state, roles.as_ref())
}

pub(crate) fn xodus_client() -> Result<PathBuf, String> {
    resolve_xodus_client(&Client::discover().ok_or("Install Xbox support first")?)
}

pub(crate) fn player_profile() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let home = platform::home()?;
    if let Some(roles) = roles(&config_dir(&home)).ok()? {
        return Some(roles.player);
    }
    let profiles = account_profiles(&xodus_client().ok()?)?;
    default_player(&profiles).map(str::to_owned)
}

fn default_player(profiles: &Value) -> Option<&str> {
    let id = profiles["default_profile"]
        .as_str()
        .filter(|id| profile_id(id))?;
    (profiles["schema"] == 1
        && profiles["profiles"]
            .as_array()?
            .iter()
            .any(|profile| profile["id"] == id && profile["signed_in"] == true))
    .then_some(id)
}

pub(crate) fn player_achievement_page(
    client: &Path,
    profile: &str,
    title_id: u32,
    continuation: Option<&str>,
    timeout: Duration,
) -> Option<Value> {
    let url = achievements_url(title_id, continuation);
    let mut command = host_command(client);
    command.args([
        "--profile",
        profile,
        "collections",
        &url,
        "--relying-party",
        "http://xboxlive.com",
        "--header",
        "x-xbl-contract-version:2",
    ]);
    query(command, timeout, "Xbox achievements could not be loaded").ok()
}

fn achievements_url(title_id: u32, continuation: Option<&str>) -> String {
    let mut url = format!(
        "https://achievements.xboxlive.com/users/xuid({{XUID}})/achievements?titleId={title_id}&types=Persistent&maxItems=100"
    );
    if let Some(token) = continuation {
        url.push_str("&continuationToken=");
        url.push_str(&catalog::encode(token));
    }
    url
}

fn resolve_xodus_client(client: &Client) -> Result<PathBuf, String> {
    if client.managed {
        return Ok(managed_cli().ok_or(CLIENT_NOT_FOUND)?.join(XODUS));
    }
    let mut command = client.command();
    command.args(["--json", "env"]);
    let result = query(command, Duration::from_secs(8), CLIENT_NOT_FOUND)?;
    external_client_path(&result)
        .filter(|path| {
            if !in_flatpak() {
                return path.is_file();
            }
            let mut test = host_command(Path::new("test"));
            test.arg("-f").arg(path);
            successful_output(test, Duration::from_secs(5), CLIENT_NOT_FOUND).is_ok()
        })
        .ok_or_else(|| CLIENT_NOT_FOUND.into())
}

fn external_client_path(result: &Value) -> Option<PathBuf> {
    if result["schema"] != 1 {
        return None;
    }
    let dir = Path::new(result["env"]["XODUS_CLI_DIR"].as_str()?);
    dir.is_absolute().then(|| dir.join("xodus-cli"))
}

fn selected_profile_signed_out(client: &Path, selected: Option<&Roles>) -> bool {
    account_profiles(client)
        .is_some_and(|profiles| signed_out(&profiles, selected.map(|roles| roles.store.as_str())))
}

fn account_profiles(client: &Path) -> Option<Value> {
    let mut command = host_command(client);
    command.args(["account-profiles", "--json"]);
    query(
        command,
        Duration::from_secs(6),
        "Could not check Xbox account status",
    )
    .ok()
}

fn signed_out(value: &Value, store: Option<&str>) -> bool {
    let Some(store) = store.or_else(|| value["default_profile"].as_str()) else {
        // Without profile selection, a legacy keyring-only login can exist.
        return false;
    };
    value["schema"] == 1
        && profile_id(store)
        && value["profiles"].as_array().is_some_and(|profiles| {
            profiles.iter().all(|profile| {
                profile["id"].as_str().is_some_and(profile_id) && profile["signed_in"].is_boolean()
            }) && !profiles
                .iter()
                .any(|profile| profile["id"] == store && profile["signed_in"] == true)
        })
}

fn sign_in_preflight(client: &Path) -> Result<(), String> {
    let mut command = host_command(client);
    command.arg("--help");
    if successful_output(command, Duration::from_secs(5), "Xbox client cannot start").is_err() {
        let mut command = host_command(Path::new("ldd"));
        command.arg(client);
        if successful_output(
            command,
            Duration::from_secs(5),
            "Could not check Xbox client libraries",
        )
        .is_ok_and(|text| missing_webkit(&text))
        {
            return Err("Install WebKitGTK 4.1 on the Linux host to open Xbox sign-in. On Fedora the package is webkit2gtk4.1, and on Debian or Ubuntu it is libwebkit2gtk-4.1-0.".into());
        }
        return Err("The Xbox client cannot start. Check its host libraries or repair the managed Xbox CLI in Settings.".into());
    }
    keyring_preflight()
}

fn missing_webkit(output: &str) -> bool {
    output.lines().any(|line| {
        [
            "libwebkit2gtk-4.1.so.0",
            "libjavascriptcoregtk-4.1.so.0",
            "libgtk-3.so.0",
            "libsoup-3.0.so.0",
        ]
        .iter()
        .any(|library| {
            line.trim()
                .strip_prefix(library)
                .is_some_and(|status| status.trim() == "=> not found")
        })
    })
}

fn keyring_preflight() -> Result<(), String> {
    let error = "Could not contact the desktop keyring. Start or unlock the keyring in your Linux desktop session, then retry Xbox sign-in.";
    let call = |arguments: &[&str]| {
        let mut command = host_command(Path::new("gdbus"));
        command.args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.secrets",
            "--timeout",
            "5",
        ]);
        command.args(arguments);
        successful_output(command, Duration::from_secs(6), error)
    };
    let alias = call(&[
        "--object-path",
        "/org/freedesktop/secrets",
        "--method",
        "org.freedesktop.Secret.Service.ReadAlias",
        "default",
    ])?;
    let path = keyring_collection(&alias).ok_or(error)?;
    if path != "/" {
        let locked = call(&[
            "--object-path",
            path,
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.freedesktop.Secret.Collection",
            "Locked",
        ])?;
        match locked.trim() {
            "(<false>,)" => {},
            "(<true>,)" => return Err("The desktop keyring is locked. Unlock it in your desktop session, then retry Xbox sign-in.".into()),
            _ => return Err(error.into()),
        }
    }
    Ok(())
}

fn keyring_collection(alias: &str) -> Option<&str> {
    alias.split('\'').nth(1).filter(|path| {
        *path == "/"
            || path
                .strip_prefix("/org/freedesktop/secrets/collection/")
                .is_some_and(|name| {
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                })
    })
}

fn fetch_library(client: &Path, state: &Path, roles: Option<&Roles>) -> Result<(), String> {
    let mut command = host_command(client);
    if let Some(roles) = roles {
        command.args(["--profile", &roles.store]);
    }
    command.args(["library", "--json", "--all"]);
    let result = query(
        command,
        Duration::from_secs(90),
        "Could not refresh the Xbox library. Check your sign-in and try again.",
    )?;
    let items = result
        .as_array()
        .or_else(|| result["items"].as_array())
        .filter(|items| items.iter().all(|row| row["productId"].is_string()))
        .ok_or("Xbox returned an invalid library")?;
    let path = cache_path(state, roles);
    storage::write_json(&path, &json!({"count": items.len(), "items": items}))?;
    let mut signed_out = SIGNED_OUT_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if signed_out.as_deref() == Some(&path) {
        *signed_out = None;
    }
    Ok(())
}

pub(super) fn query(command: Command, timeout: Duration, error: &str) -> Result<Value, String> {
    serde_json::from_str(&successful_output(command, timeout, error)?).map_err(|_| error.into())
}

fn successful_output(
    mut command: Command,
    timeout: Duration,
    error: &str,
) -> Result<String, String> {
    // The CLI owns credentials. Read only the requested JSON response.
    let mut output: File = tempfile::tempfile().map_err(|_| error)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|_| error)?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| error)?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(error.into()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    }
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0)).map_err(|_| error)?;
    let bytes = crate::util::read_limited(output, 8 * 1024 * 1024)
        .map_err(|_| error)?
        .ok_or(error)?;
    String::from_utf8(bytes).map_err(|_| error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_ferestre_can_only_point_at_an_absolute_xodus_directory() {
        let data = |dir| json!({"schema":1,"env":{"XODUS_CLI_DIR":dir}});
        assert_eq!(
            external_client_path(&data("/usr/lib/ferestre/client")),
            Some(PathBuf::from("/usr/lib/ferestre/client/xodus-cli"))
        );
        assert!(external_client_path(&data("client")).is_none());
        assert!(external_client_path(&data("")).is_none());
        assert!(
            external_client_path(&json!({"schema":2,"env":{"XODUS_CLI_DIR":"/client"}})).is_none()
        );
    }

    #[test]
    fn sign_out_only_invalidates_the_selected_account_with_known_status() {
        let live = "0".repeat(32);
        let signed = "1".repeat(32);
        let profiles = json!({"schema":1,"default_profile":live,"profiles":[
            {"id":live,"signed_in":true}, {"id":signed,"signed_in":false}
        ]});
        assert!(!signed_out(&profiles, None));
        assert_eq!(default_player(&profiles), Some(live.as_str()));
        assert!(signed_out(&profiles, Some(&signed)));
        let mut signed_out_default = profiles.clone();
        signed_out_default["default_profile"] = json!(signed);
        assert_eq!(default_player(&signed_out_default), None);
        assert!(!signed_out(&json!({"schema":1,"profiles":[]}), None));
        assert!(!signed_out(
            &json!({"schema":1,"profiles":[{"id":live}]}),
            Some(&live)
        ));
    }

    #[test]
    fn achievement_request_encodes_the_continuation_without_substituting_it() {
        assert_eq!(
            achievements_url(123, Some("next&{RAWTOKEN}")),
            "https://achievements.xboxlive.com/users/xuid({XUID})/achievements?titleId=123&types=Persistent&maxItems=100&continuationToken=next%26%7BRAWTOKEN%7D"
        );
    }

    #[test]
    fn preflight_recognizes_webkit_and_keyring_results() {
        assert!(missing_webkit(" libwebkit2gtk-4.1.so.0 => not found\n"));
        assert!(!missing_webkit(
            " libwebkit2gtk-4.1.so.0 => /lib64/libwebkit2gtk-4.1.so.0\n"
        ));
        assert_eq!(keyring_collection("(objectpath '/',)"), Some("/"));
        assert_eq!(
            keyring_collection("(objectpath '/org/freedesktop/secrets/collection/login',)"),
            Some("/org/freedesktop/secrets/collection/login")
        );
        assert!(keyring_collection("(objectpath '/org/freedesktop/new',)").is_none());
    }
}
