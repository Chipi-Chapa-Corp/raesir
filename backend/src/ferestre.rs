use crate::{
    catalog, discovery,
    model::{FerestreState, FerestreTest, GameAction, Os, Report},
    platform, storage,
};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

const CLI: &str = "usr/bin/ferestre";
const XODUS: &str = "usr/lib/ferestre/client/xodus-cli";
const XODUS_SERVICE: &str = "usr/lib/ferestre/client/xodus-service";

pub const fn setup_supported() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

fn managed_path() -> Option<PathBuf> {
    if !setup_supported() {
        return None;
    }
    let home = platform::home()?;
    Some(cli_directory(&host_dir(
        &home,
        "XDG_DATA_HOME",
        ".local/share",
    )))
}

pub(crate) fn cli_directory(data_dir: &Path) -> PathBuf {
    data_dir.join(storage::APP_ID).join("ferestre/cli")
}

fn managed_cli() -> Option<PathBuf> {
    managed_path().filter(|dir| managed_cli_ready(dir))
}

fn managed_cli_ready(dir: &Path) -> bool {
    [
        CLI,
        XODUS,
        XODUS_SERVICE,
        "usr/lib/ferestre/scripts/proton-wine-shim.sh",
    ]
    .iter()
    .all(|file| {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        dir.join(file).metadata().is_ok_and(|metadata| {
            #[cfg(unix)]
            let executable = metadata.permissions().mode() & 0o111 != 0;
            #[cfg(not(unix))]
            let executable = false;
            metadata.is_file() && executable
        })
    })
}

pub fn managed_installed() -> bool {
    managed_cli().is_some()
}

pub(crate) fn host_dir(home: &Path, variable: &str, default: &str) -> PathBuf {
    let variable = if in_flatpak() {
        format!("HOST_{variable}")
    } else {
        variable.to_owned()
    };
    env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(default))
}

pub(crate) fn state_dir(home: &Path) -> PathBuf {
    host_dir(home, "XDG_STATE_HOME", ".local/state").join("ferestre")
}

fn in_flatpak() -> bool {
    cfg!(target_os = "linux") && Path::new("/.flatpak-info").is_file()
}

pub(crate) struct Client {
    pub program: String,
    managed: bool,
}

impl Client {
    pub fn discover() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        managed_cli()
            .and_then(|dir| {
                Some(Self {
                    program: dir.join(CLI).to_str()?.into(),
                    managed: true,
                })
            })
            .or_else(|| {
                discovery::program("ferestre").map(|program| Self {
                    program,
                    managed: false,
                })
            })
    }

    fn command(&self) -> Command {
        if self.managed {
            host_command(Path::new(&self.program))
        } else {
            Command::new(&self.program)
        }
    }

    pub fn action(&self, mut args: Vec<String>) -> GameAction {
        if self.managed && in_flatpak() {
            args.splice(0..0, ["--host".into(), self.program.clone()]);
            return catalog::command("Ferestre", "flatpak-spawn".into(), args);
        }
        catalog::command("Ferestre", self.program.clone(), args)
    }
}

fn host_command(program: &Path) -> Command {
    if in_flatpak() {
        let mut command = Command::new("flatpak-spawn");
        command.arg("--host").arg(program);
        command
    } else {
        Command::new(program)
    }
}

pub(crate) fn runtime_installed(home: &Path) -> bool {
    managed_runtime_installed(&state_dir(home))
        || env::var_os("XODUS_PROTON_DIR").is_some_and(|dir| runtime_ready(Path::new(&dir)))
        || platform::steam_in_home(home)
            .iter()
            .any(|steam| runtime_ready(&steam.join("compatibilitytools.d/xodus")))
}

fn runtime_ready(path: &Path) -> bool {
    path.join("proton").is_file()
        && path
            .join("files/lib/wine/x86_64-windows/xgameruntime.dll")
            .is_file()
}

pub(crate) fn managed_runtime_installed(state: &Path) -> bool {
    discovery::children(&state.join("runtimes"))
        .iter()
        .any(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.as_bytes().first().is_some_and(u8::is_ascii_digit))
                && runtime_ready(dir)
        })
}

pub const RUNTIME_JOB: &str = "ferestre:runtime";
pub(crate) const CONSOLE_ONLY_MESSAGE: &str =
    "This Xbox Store edition only contains console packages.";
pub(crate) const UNSUPPORTED_PACKAGE_MESSAGE: &str =
    "This Xbox edition can't be installed on Linux.";

pub(crate) fn install_blocker(product: &str) -> Option<&'static str> {
    let products = crate::xbox_store::products(&[product], "neutral")?;
    products
        .iter()
        .find(|entry| {
            entry["ProductId"]
                .as_str()
                .is_some_and(|id| id.eq_ignore_ascii_case(product))
        })
        .and_then(package_blocker)
}

fn package_blocker(product: &serde_json::Value) -> Option<&'static str> {
    match package_support(product) {
        PackageSupport::ConsoleOnly => Some(CONSOLE_ONLY_MESSAGE),
        PackageSupport::Unsupported => Some(UNSUPPORTED_PACKAGE_MESSAGE),
        PackageSupport::Msixvc | PackageSupport::Unknown => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackageSupport {
    Msixvc,
    ConsoleOnly,
    Unsupported,
    Unknown,
}

pub(crate) fn package_support(product: &serde_json::Value) -> PackageSupport {
    use PackageSupport::*;
    let Some(skus) = product["DisplaySkuAvailabilities"].as_array() else {
        return Unknown;
    };
    let properties = skus.iter().map(|sku| &sku["Sku"]["Properties"]);
    if properties.clone().any(|sku| {
        sku["IsBundle"] == true
            || sku["BundledSkus"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
    }) {
        return Unknown;
    }
    let packages = properties
        .filter_map(|sku| sku["Packages"].as_array())
        .flatten();
    let mut has_pc_package = false;
    let mut has_console_package = false;
    let mut has_phone_package = false;
    let mut unknown = false;
    for package in packages {
        let Some(platforms) = package["PlatformDependencies"]
            .as_array()
            .filter(|items| !items.is_empty())
        else {
            unknown = true;
            continue;
        };
        for dependency in platforms {
            match dependency["PlatformName"].as_str() {
                Some("Windows.Xbox") => has_console_package = true,
                Some("Windows.WindowsPhone" | "Windows.WindowsPhone8x") => has_phone_package = true,
                Some("Windows.Desktop" | "Windows.Universal" | "Windows.Windows8x") => {
                    match package["PackageFormat"].as_str() {
                        Some("MSIXVC") => return Msixvc,
                        Some(
                            "EAppx" | "Appx" | "AppxBundle" | "EAppxBundle" | "MSIX" | "EMSIX"
                            | "MSIXBundle" | "EMSIXBundle",
                        ) => has_pc_package = true,
                        _ => unknown = true,
                    }
                }
                _ => unknown = true,
            }
        }
    }
    if unknown {
        Unknown
    } else if has_pc_package {
        Unsupported
    } else if has_console_package && !has_phone_package {
        ConsoleOnly
    } else {
        Unknown
    }
}

pub fn mark_tested_titles(report: &mut Report) {
    if report.host_os != Os::Linux || !report.catalog.iter().any(|game| game.store == "xbox") {
        return;
    }
    let Some(client) = Client::discover() else {
        return;
    };
    let mut command = client.command();
    command.args(["--json", "titles"]);
    if let Ok(value) = account::query(
        command,
        std::time::Duration::from_secs(3),
        "Ferestre compatibility data could not be read",
    ) {
        apply_tested_titles(report, &value);
    }
}

fn apply_tested_titles(report: &mut Report, value: &serde_json::Value) {
    let Some(titles) = value["titles"].as_array().filter(|_| value["schema"] == 1) else {
        return;
    };
    let tested: std::collections::BTreeMap<_, _> = titles
        .iter()
        .filter_map(|row| {
            let id = row["product_id"]
                .as_str()
                .filter(|id| crate::xbox_store::product_id(id))?;
            let test: FerestreTest = serde_json::from_value(row.clone()).ok()?;
            if test.state != FerestreState::Untested
                && test
                    .last_verified
                    .as_deref()
                    .is_none_or(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err())
            {
                return None;
            }
            Some((id.to_ascii_uppercase(), test))
        })
        .collect();
    for game in report
        .catalog
        .iter_mut()
        .filter(|game| game.store == "xbox")
    {
        game.ferestre_test = tested.get(&game.id.to_ascii_uppercase()).cloned();
    }
}

#[cfg(test)]
#[test]
fn ferestre_results_match_dated_store_editions_only() {
    let mut report: Report = serde_json::from_value(serde_json::json!({
        "host_os": "linux", "games": [], "libraries": [], "volumes": [],
        "warnings": [], "owned_account": null
    }))
    .unwrap();
    for (store, id) in [
        ("xbox", "9P5786PJB9RP"),
        ("xbox", "9PM3J18QM4Q3"),
        ("steam", "9P5786PJB9RP"),
    ] {
        report.catalog.push(catalog::entry(
            store,
            id,
            "Minecraft Dungeons II",
            vec![],
            "fixture",
        ));
    }
    apply_tested_titles(
        &mut report,
        &serde_json::json!({
            "schema": 1, "titles": [
                {"product_id": "9p5786pjb9rp", "state": "playable", "last_verified": "2026-10-18"},
                {"product_id": "9PM3J18QM4Q3", "state": "playable"}
            ]
        }),
    );
    assert_eq!(
        report.catalog[0].ferestre_test.as_ref().unwrap().state,
        FerestreState::Playable
    );
    assert!(
        report.catalog[1..]
            .iter()
            .all(|g| g.ferestre_test.is_none())
    );
}

#[cfg(test)]
#[test]
fn console_package_check_allows_pc_products_bundles_and_unknown_metadata() {
    let product = |platforms: &[&str], bundle| {
        serde_json::json!({
            "DisplaySkuAvailabilities": [{"Sku":{"Properties":{
                "IsBundle": bundle,
                "Packages": [{"PlatformDependencies": platforms.iter()
                    .map(|name| serde_json::json!({"PlatformName":name})).collect::<Vec<_>>() }]
            }}}]
        })
    };
    assert_eq!(
        package_blocker(&product(&["Windows.Xbox"], false)),
        Some(CONSOLE_ONLY_MESSAGE)
    );
    assert!(package_blocker(&product(&["Windows.Xbox", "Windows.Desktop"], false)).is_none());
    assert!(package_blocker(&product(&["Windows.Xbox"], true)).is_none());
    assert!(package_blocker(&product(&[], false)).is_none());
    let mut pc = product(&["Windows.Desktop", "Windows.Xbox"], false);
    let format = "/DisplaySkuAvailabilities/0/Sku/Properties/Packages/0";
    pc.pointer_mut(format).unwrap()["PackageFormat"] = "EAppx".into();
    assert_eq!(package_blocker(&pc), Some(UNSUPPORTED_PACKAGE_MESSAGE));
    pc.pointer_mut("/DisplaySkuAvailabilities/0/Sku/Properties/Packages")
        .unwrap().as_array_mut().unwrap()
        .push(serde_json::json!({"PackageFormat":"Xap","PlatformDependencies":[{"PlatformName":"Windows.WindowsPhone8x"}]}));
    assert_eq!(package_blocker(&pc), Some(UNSUPPORTED_PACKAGE_MESSAGE));
    pc.pointer_mut(format).unwrap()["PackageFormat"] = "MSIXVC".into();
    assert_eq!(package_blocker(&pc), None);
    assert_eq!(package_support(&pc), PackageSupport::Msixvc);
}

pub fn start_runtime(
    downloads: &crate::downloads::Manager,
    maintenance: bool,
) -> Result<(), String> {
    use crate::{
        downloads::{Completion, Request},
        steam_control::Operation,
    };
    check_setup_enabled()?;
    let dir = managed_cli().ok_or("Install Xbox support first")?;
    let home = platform::home().ok_or("Could not locate Xbox runtime storage")?;
    if !maintenance && runtime_installed(&home) {
        return Ok(());
    }
    if downloads
        .snapshot()
        .get(RUNTIME_JOB)
        .is_some_and(|progress| progress.active())
    {
        return Ok(());
    }
    let program = dir.join(CLI);
    let state = state_dir(&home);
    let mut command = host_command(&program);
    command.args(["--json", "install-runtime"]);
    let plan = account::query(
        command,
        std::time::Duration::from_secs(30),
        "Could not check the published Xbox runtime release",
    )?;
    let version = plan["version"]
        .as_str()
        .filter(|version| runtime_version(version))
        .filter(|_| plan["schema"] == 1)
        .ok_or("Invalid Xbox runtime release metadata")?;
    let target = state.join("runtimes").join(version);
    if plan["destination"].as_str().map(Path::new) != Some(target.as_path()) {
        return Err("Ferestre uses a different runtime directory. Reset its runtime directory override and retry.".into());
    }
    if plan["newer_installed"].as_str().is_some() {
        return Err("A newer Xbox runtime is already installed".into());
    }
    let cache = host_dir(&home, "XDG_CACHE_HOME", ".cache").join("ferestre/runtime");
    quarantine_incomplete_runtime(&target, &state, &cache)?;
    downloads.start(Request {
        key: RUNTIME_JOB.into(),
        operation: if maintenance {
            Operation::Update
        } else {
            Operation::Install
        },
        program: program
            .to_str()
            .ok_or("Invalid Xbox runtime installer path")?
            .into(),
        args: vec!["install-runtime".into()],
        env: Default::default(),
        completion: Completion::FerestreRuntime { state, cache },
        version: version.into(),
        previous_version: None,
        game_dir: None,
        game_lock: None,
    })
}

fn runtime_version(version: &str) -> bool {
    version.split('.').count() >= 3
        && version.split('.').count() <= 5
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

pub(crate) fn managed_runtime_version_installed(state: &Path, version: &str) -> bool {
    if version.is_empty() {
        managed_runtime_installed(state)
    } else {
        runtime_version(version) && runtime_ready(&state.join("runtimes").join(version))
    }
}

fn quarantine_incomplete_runtime(target: &Path, state: &Path, cache: &Path) -> Result<(), String> {
    use std::fs;
    match fs::symlink_metadata(target) {
        Ok(info) if !info.is_dir() => return Err("The Xbox runtime path is not a directory".into()),
        Ok(_) if runtime_ready(target) => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect the Xbox runtime: {error}")),
    }
    fs::create_dir_all(cache)
        .map_err(|error| format!("Could not prepare Xbox runtime repair: {error}"))?;
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join(".install.lock"))
        .map_err(|error| format!("Could not open the Xbox runtime install lock: {error}"))?;
    lease
        .try_lock()
        .map_err(|error| format!("Could not reserve the Xbox runtime install lock: {error}"))?;
    if runtime_ready(target) {
        return Ok(());
    }
    // Keep incomplete data outside the runtime list and let Ferestre install
    // the signed release into its usual version directory.
    let backup = tempfile::Builder::new()
        .prefix("incomplete-runtime-")
        .tempdir_in(state)
        .map_err(|error| format!("Could not reserve an Xbox runtime backup: {error}"))?;
    fs::rename(target, backup.path().join("runtime"))
        .map_err(|error| format!("Could not set aside the incomplete Xbox runtime: {error}"))?;
    let _ = backup.keep();
    Ok(())
}

#[cfg(test)]
#[test]
fn repair_sets_aside_only_the_incomplete_target_runtime() {
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let state = temp.path();
    let runtimes = state.join("runtimes");
    let old = runtimes.join("11.0.1");
    let current = runtimes.join("11.0.2");
    fs::create_dir_all(old.join("files/lib/wine/x86_64-windows")).unwrap();
    fs::write(old.join("proton"), "").unwrap();
    fs::write(
        old.join("files/lib/wine/x86_64-windows/xgameruntime.dll"),
        "",
    )
    .unwrap();
    fs::create_dir_all(&current).unwrap();
    fs::write(current.join("proton"), "incomplete").unwrap();
    assert!(managed_runtime_version_installed(state, ""));
    assert!(!managed_runtime_version_installed(state, "11.0.2"));
    assert!(!managed_runtime_version_installed(state, "../old"));
    quarantine_incomplete_runtime(&old, state, &cache).unwrap();
    quarantine_incomplete_runtime(&current, state, &cache).unwrap();
    assert!(old.exists());
    assert!(!current.exists());
    assert!(discovery::children(state).iter().any(|dir| {
        fs::read_to_string(dir.join("runtime/proton"))
            .ok()
            .as_deref()
            == Some("incomplete")
    }));
}

pub fn install_managed(maintenance: bool) -> Result<(), String> {
    check_setup_enabled()?;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let path = managed_path().ok_or("Could not find a user data directory for Ferestre")?;
        install::run(&path, maintenance)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = maintenance;
        Err("Ferestre setup requires x86-64 Linux".into())
    }
}

fn check_setup_enabled() -> Result<(), String> {
    if env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
        || env::var_os("RAESIR_DISABLE_NETWORK").is_some()
    {
        return Err("Ferestre setup is disabled in this diagnostic profile".into());
    }
    Ok(())
}

pub(crate) mod account;
pub(crate) use account::{library_cache_path, library_cached, library_signed_out};
pub use account::{refresh_library, sign_in};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod install {
    use super::*;
    use minisign_verify::{PublicKey, Signature};
    use reqwest::blocking::{Client, Response};
    use std::{
        fs,
        io::{self, Read},
        os::unix::fs::PermissionsExt,
        time::Duration,
    };

    const RELEASE_API: &str = "https://api.github.com/repos/icex/ferestre/releases/latest";
    const RELEASE_KEY: &str = "RWQhHyR/MHoTJtPui/l2Xbhp3ETYauN4ErUGb6wYrjpw/rzt+874RvOb";
    const ARCHIVE_LIMIT: u64 = 128 * 1024 * 1024;

    pub(super) fn run(path: &Path, maintenance: bool) -> Result<(), String> {
        if !maintenance && managed_installed() {
            return Ok(());
        }
        let dir = path
            .parent()
            .ok_or("Invalid Ferestre installation directory")?;
        fs::create_dir_all(dir)
            .map_err(|err| format!("Could not create Ferestre directory: {err}"))?;
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("install.lock"))
            .map_err(|err| format!("Could not open Ferestre installation lock: {err}"))?;
        lease
            .try_lock()
            .map_err(|_| "Ferestre setup is already running")?;
        if !maintenance && managed_cli_ready(path) {
            return Ok(());
        }

        let http = Client::builder()
            .https_only(true)
            .user_agent("Raesir/0.1")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|err| err.to_string())?;
        let release: serde_json::Value =
            serde_json::from_str(&read_text(&http, RELEASE_API, 128 * 1024)?)
                .map_err(|_| "Ferestre returned invalid release metadata")?;
        let tag = release["tag_name"].as_str().unwrap_or_default();
        let version = release_version(tag).ok_or("Ferestre returned an unsupported release tag")?;
        if let Some(installed) = installed_version(path)
            && version_numbers(&installed) > version_numbers(version)
        {
            return Err("A newer Xbox client is already installed".into());
        }
        let package = format!("ferestre-{version}");
        let name = format!("{package}-x86_64.tar.zst");
        let url = format!("https://github.com/icex/ferestre/releases/download/{tag}/{name}");
        let signature = read_text(&http, &format!("{url}.minisig"), 4096)?;
        let signature = release_signature(&signature, &name, version)?;
        let response = download(&http, &url)?;
        let mut archive = tempfile::NamedTempFile::new_in(dir)
            .map_err(|err| format!("Could not stage Ferestre: {err}"))?;
        let size = io::copy(&mut response.take(ARCHIVE_LIMIT + 1), archive.as_file_mut())
            .map_err(|err| format!("Could not download Ferestre: {err}"))?;
        if size > ARCHIVE_LIMIT {
            return Err("Ferestre release exceeds the download size limit".into());
        }
        verify_archive(archive.path(), &signature)?;
        let staging = tempfile::tempdir_in(dir)
            .map_err(|err| format!("Could not stage Ferestre CLI: {err}"))?;
        extract_cli(archive.path(), staging.path(), &package)
            .map_err(|err| format!("Could not extract Ferestre CLI: {err}"))?;
        fs::write(staging.path().join("version"), version)
            .map_err(|err| format!("Could not record the Ferestre version: {err}"))?;
        publish(staging.path(), path)
            .map_err(|err| format!("Could not install Ferestre: {err}"))?;
        Ok(())
    }

    fn installed_version(path: &Path) -> Option<String> {
        if let Ok(version) = fs::read_to_string(path.join("version"))
            && version_numbers(&version).is_some()
        {
            return Some(version);
        }
        if !managed_cli_ready(path) {
            return None;
        }
        let mut command = host_command(&path.join(CLI));
        command.args(["--json", "version"]);
        let result = account::query(
            command,
            Duration::from_secs(5),
            "Could not read installed Ferestre version",
        )
        .ok()?;
        let version = result["version"].as_str()?;
        version_numbers(version).map(|_| version.to_owned())
    }

    fn version_numbers(version: &str) -> Option<Vec<u32>> {
        version
            .split('.')
            .map(|part| part.parse::<u32>().ok())
            .collect()
    }

    fn publish(staging: &Path, destination: &Path) -> io::Result<()> {
        match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::rename(staging, destination)
            }
            Err(error) => Err(error),
            Ok(info) if !info.is_dir() => Err(io::Error::other(
                "The managed Xbox CLI path is not a directory",
            )),
            Ok(_) => {
                use std::{ffi::CString, os::unix::ffi::OsStrExt};
                let from = CString::new(staging.as_os_str().as_bytes())?;
                let to = CString::new(destination.as_os_str().as_bytes())?;
                // Replace the verified installation in one operation. A failed
                // exchange leaves the current CLI at the original path.
                let result = unsafe {
                    libc::renameat2(
                        libc::AT_FDCWD,
                        from.as_ptr(),
                        libc::AT_FDCWD,
                        to.as_ptr(),
                        libc::RENAME_EXCHANGE,
                    )
                };
                if result == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn cli_publication_exchanges_complete_directories() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("cli");
        let staging = root.path().join("staging");
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("version"), "0.1.22").unwrap();
        publish(&staging, &current).unwrap();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("version"), "0.1.23").unwrap();
        publish(&staging, &current).unwrap();
        assert_eq!(
            fs::read_to_string(current.join("version")).unwrap(),
            "0.1.23"
        );
        assert_eq!(
            fs::read_to_string(staging.join("version")).unwrap(),
            "0.1.22"
        );
    }

    fn release_version(tag: &str) -> Option<&str> {
        let version = tag.strip_prefix('v')?;
        let parts: Vec<_> = version.split('.').collect();
        (parts.len() == 3
            && parts.iter().all(|part| {
                part.parse::<u32>().is_ok() && part.bytes().all(|byte| byte.is_ascii_digit())
            }))
        .then_some(version)
    }

    fn download(http: &Client, url: &str) -> Result<Response, String> {
        http.get(url)
            .send()
            .and_then(Response::error_for_status)
            .map_err(|err| format!("Could not fetch the Ferestre release: {err}"))
    }

    fn read_text(http: &Client, url: &str, limit: u64) -> Result<String, String> {
        let mut text = String::new();
        download(http, url)?
            .take(limit + 1)
            .read_to_string(&mut text)
            .map_err(|err| format!("Could not read Ferestre release metadata: {err}"))?;
        if text.len() as u64 > limit {
            return Err("Ferestre release metadata exceeds the download size limit".into());
        }
        Ok(text)
    }

    fn release_signature(text: &str, name: &str, version: &str) -> Result<Signature, String> {
        let signature = Signature::decode(text)
            .map_err(|err| format!("Invalid Ferestre release signature: {err}"))?;
        if signature.trusted_comment() != format!("file:{name} version:{version}") {
            return Err("Ferestre signature names a different release asset".into());
        }
        Ok(signature)
    }

    fn verify_archive(path: &Path, signature: &Signature) -> Result<(), String> {
        let key = PublicKey::from_base64(RELEASE_KEY).map_err(|err| err.to_string())?;
        let mut verifier = key
            .verify_stream(signature)
            .map_err(|err| err.to_string())?;
        let mut file = fs::File::open(path).map_err(|err| err.to_string())?;
        let mut buffer = vec![0; 128 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|err| err.to_string())?;
            if count == 0 {
                break;
            }
            verifier.update(&buffer[..count]);
        }
        verifier
            .finalize()
            .map_err(|_| "Ferestre release failed signature verification".into())
    }

    fn extract_cli(path: &Path, staging: &Path, package: &str) -> io::Result<()> {
        let invalid = || io::Error::other("Invalid Ferestre CLI archive");
        let decoder = zstd::stream::read::Decoder::new(fs::File::open(path)?)?;
        let mut tar = tar::Archive::new(decoder.take(256 * 1024 * 1024));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let relative = path.strip_prefix(package).map_err(|_| invalid())?;
            if !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(invalid());
            }
            if relative != Path::new(CLI)
                && !relative.starts_with("usr/lib/ferestre")
                && !relative.starts_with("usr/share/licenses/ferestre")
                && relative != Path::new("usr/share/doc/ferestre/NOTICE")
            {
                continue;
            }
            if entry.header().entry_type().is_dir() {
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(invalid());
            }
            let destination = staging.join(relative);
            fs::create_dir_all(destination.parent().ok_or_else(invalid)?)?;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(destination)?;
            io::copy(&mut entry, &mut file)?;
            file.set_permissions(fs::Permissions::from_mode(entry.header().mode()? & 0o755))?;
        }
        if [
            CLI,
            XODUS,
            XODUS_SERVICE,
            "usr/lib/ferestre/scripts/proton-wine-shim.sh",
        ]
        .iter()
        .any(|file| {
            !fs::metadata(staging.join(file)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o100 != 0
            })
        }) {
            return Err(invalid());
        }
        Ok(())
    }
}
