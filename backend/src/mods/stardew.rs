use super::*;

pub(super) const DOMAIN: &str = "stardewvalley";
pub(super) const STEAM_ID: &str = "413150";
const XBOX_ID: &str = "9MWR1NC6VQ6L";
const STATE: &str = "smapi-mods.json";

pub(super) fn matches(game: &LibraryGame) -> bool {
    match game.store.as_str() {
        "steam" => game.id == STEAM_ID,
        "xbox" => !game.xbox_console_only && game.id.eq_ignore_ascii_case(XBOX_ID),
        _ => false,
    }
}

pub(super) fn smapi_target(game: &LibraryGame) -> Option<PathBuf> {
    if game.store != "steam" {
        return None;
    }
    let executable = match crate::model::Os::host() {
        crate::model::Os::Linux => "StardewModdingAPI",
        crate::model::Os::Windows => "StardewModdingAPI.exe",
        crate::model::Os::Unknown => return None,
    };
    game.installations
        .iter()
        .filter(|i| {
            i.os == crate::model::Os::host() && matches!(i.state.as_str(), "installed" | "external")
        })
        .find_map(|installation| {
            let dir = Path::new(&installation.path);
            let target = dir.join(executable);
            (target.is_file()
                && dir.join("StardewModdingAPI.dll").is_file()
                && dir.join("smapi-internal").is_dir())
            .then_some(target)
        })
}

#[derive(Serialize, Deserialize)]
struct Entry {
    nexus_id: u64,
    mod_ids: Vec<String>,
    package: Package,
}

fn read(profile: &Path) -> Result<Vec<Entry>, String> {
    match fs::read(profile.join(STATE)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("Could not read SMAPI profile: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn installed(profile: &Path) -> Result<Vec<InstalledMod>, String> {
    Ok(read(profile)?
        .into_iter()
        .map(|e| InstalledMod {
            id: e.package.id,
            name: e.package.name,
            owner: e.package.owner,
            version: e.package.version,
            enabled: true,
            local: false,
            dependencies: Vec::new(),
            automatic: false,
        })
        .collect())
}

impl Manager {
    pub(super) fn stardew_profile_packages(&self) -> Result<Arc<Vec<Package>>, String> {
        let Some(profile) = self.own_path(DOMAIN) else {
            return Ok(Arc::default());
        };
        Ok(Arc::new(
            read(&profile)?
                .into_iter()
                .map(|entry| entry.package)
                .collect(),
        ))
    }

    pub(super) fn stardew_details(
        &self,
        game: &LibraryGame,
        id: &str,
    ) -> Result<crate::details::Details, String> {
        let packages = self.stardew_profile_packages()?;
        let package = packages
            .iter()
            .find(|p| p.id == id)
            .ok_or("This mod is no longer listed")?;
        Ok(crate::details::Details {
            key: format!("{}:{id}", game.key),
            title: package.name.clone(),
            description: package.description.clone(),
            store_url: Some(package.page.clone()),
            developers: vec![package.owner.clone()],
            ..Default::default()
        })
    }

    fn edit_smapi(
        &self,
        game: &LibraryGame,
        edit: impl FnOnce(&Path, &mut Vec<Entry>) -> Result<(), String>,
    ) -> Result<Snapshot, String> {
        {
            let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
            let _profile_lease = self.profile_lease(DOMAIN)?;
            self.registry(DOMAIN)?;
            let root = self.profile_root(DOMAIN, &self.selected_id(DOMAIN));
            // Either storefront can edit a shared profile. Keep its revisions
            // in one location regardless of which copy supplied the action.
            let revisions = root.join("revisions");
            fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
            let pending = tempfile::Builder::new()
                .prefix("pending-")
                .tempdir_in(revisions)
                .map_err(|e| e.to_string())?;
            if let Some(source) = self.own_path(DOMAIN) {
                install::copy_profile(&source, pending.path())?;
            } else if root.join("current.json").exists() {
                return Err("The selected SMAPI profile revision could not be read".into());
            }
            let mut entries = read(pending.path())?;
            edit(pending.path(), &mut entries)?;
            crate::storage::write_json(&pending.path().join(STATE), &entries)?;
            install::commit(&root, pending)?;
            self.prune_revisions(DOMAIN);
        }
        self.snapshot(game, false)
    }

    pub(super) fn uninstall_smapi(
        &self,
        game: &LibraryGame,
        package: &str,
    ) -> Result<Snapshot, String> {
        install::changes_allowed()?;
        self.edit_smapi(game, |profile, entries| {
            let entry = entries
                .iter()
                .find(|entry| entry.package.id == package)
                .ok_or("This mod is not installed")?;
            fs::remove_dir_all(
                profile
                    .join("mods")
                    .join(format!("nexus-{}", entry.nexus_id)),
            )
            .map_err(|e| e.to_string())?;
            entries.retain(|entry| entry.package.id != package);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> Manager {
        let mut manager = Manager::new(&root.join("data"), &root.join("cache"));
        manager.leases = root.join("leases");
        manager
    }

    fn game(store: &str, id: &str) -> LibraryGame {
        crate::catalog::entry(store, id, "Stardew Valley", vec![], "fixture")
    }

    // Reproduce the profile data written by builds that had a SMAPI importer.
    fn seed_legacy_profile(manager: &Manager, game: &LibraryGame) {
        manager
            .edit_smapi(game, |profile, entries| {
                let folder = profile.join("mods/nexus-42/mod-fixture.mod");
                fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                fs::write(
                    folder.join("manifest.json"),
                    r#"{"Name":"Fixture mod","UniqueID":"Fixture.Mod","Version":"1.2.3"}"#,
                )
                .map_err(|e| e.to_string())?;
                entries.push(Entry {
                    nexus_id: 42,
                    mod_ids: vec!["fixture.mod".into()],
                    package: nexus::package(
                        DOMAIN,
                        42,
                        "Fixture mod".into(),
                        String::new(),
                        String::new(),
                        "1.2.3".into(),
                    ),
                });
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn legacy_profiles_remain_accessible_and_removable_from_both_editions() {
        let root = tempfile::tempdir().unwrap();
        let manager = fixture(root.path());
        let xbox = game("xbox", XBOX_ID);
        let steam = game("steam", STEAM_ID);
        assert!(manager.summary(&steam).is_none());
        seed_legacy_profile(&manager, &xbox);
        let steam_profile = manager.snapshot(&steam, false).unwrap();
        assert_eq!(steam_profile.profiles[0].installed[0].version, "1.2.3");
        assert_eq!(manager.summary(&xbox).unwrap().installed, 1);
        manager.uninstall(&xbox, "nexus:42").unwrap();
        assert_eq!(manager.summary(&steam).unwrap().installed, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_steam_launch_passes_selected_mods_folder_and_releases_receipt() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let manager = fixture(root.path());
        let mut steam = game("steam", STEAM_ID);
        let installation = root.path().join("game");
        fs::create_dir_all(installation.join("smapi-internal")).unwrap();
        fs::write(installation.join("StardewModdingAPI.dll"), "").unwrap();
        let executable = installation.join("StardewModdingAPI");
        fs::write(
            &executable,
            r#"#!/bin/sh
test "$SteamAppId" = 413150 && test -f "$SMAPI_MODS_PATH/nexus-42/mod-fixture.mod/manifest.json"
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        steam.installations.push(crate::model::Installation {
            os: crate::model::Os::Linux,
            path: installation.to_string_lossy().into_owned(),
            library: root.path().to_string_lossy().into_owned(),
            state: "installed".into(),
            last_seen: 0,
        });
        seed_legacy_profile(&manager, &steam);
        assert!(manager.summary(&steam).unwrap().launchable);
        let report = serde_json::from_value(serde_json::json!({
            "host_os": "linux", "games": [], "libraries": [], "volumes": [],
            "warnings": [], "owned_account": null, "catalog": []
        }))
        .unwrap();
        manager
            .launch_modded(&report, &steam, &[])
            .unwrap()
            .wait()
            .unwrap();
        assert!(
            !manager
                .stage_dir(&installation.canonicalize().unwrap())
                .exists()
        );
        assert!(!installation.join("Mods").exists());
        assert!(manager.vanilla_args(&steam).unwrap().is_empty());
    }
}
