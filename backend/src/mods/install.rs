use super::*;
use thunderstore_engine::profile;

impl Manager {
    pub fn uninstall(&self, game: &LibraryGame, package: &str) -> Result<Snapshot, String> {
        if recipes::for_game(game).is_some() {
            return self.noita_uninstall(game, package);
        }
        if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some()
            && std::env::var_os("EXPOSED_GAMES_MODS_TEST_INSTALL").is_none()
        {
            return Err("Mod changes are disabled in this diagnostic profile".into());
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let _profile_lease =
            self.profile_lease(community(game).ok_or("Unsupported mod community")?)?;
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let profiles = self.profiles(slug);
        let source = self
            .selected(slug, &profiles)
            .ok_or("Select a mod profile first")?;
        let mut reasons = dependencies::Reasons::read(&source.path)?;
        let mut source = source.clone();
        source.installed =
            reasons.installed(modlist::read(&source.path).map_err(|e| e.to_string())?);
        let removed = source.removal_plan(package)?;
        let revisions = self.revisions_dir(game)?;
        fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .map_err(|e| e.to_string())?;
        profile::portability::ensure_disjoint_trees(&source.path, pending.path())
            .map_err(|e| e.to_string())?;
        copy_profile(&source.path, pending.path())?;
        for m in &removed {
            profile::uninstall_mod_in(pending.path(), &ECOSYSTEM, slug, &m.id)
                .map_err(|e| e.to_string())?;
        }
        reasons.forget(&removed);
        reasons.write(pending.path())?;
        commit(&self.profile_root(slug, &self.selected_id(slug)), pending)?;
        self.prune_revisions(slug);
        self.snapshot(game, false)
    }

    /// Install into a new revision and publish its pointer only after the engine
    /// has installed the complete dependency closure.
    pub fn install(&self, game: &LibraryGame, package: &str) -> Result<Snapshot, String> {
        if recipes::for_game(game).is_some() {
            return self.noita_install(game, package);
        }
        if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some()
            && std::env::var_os("EXPOSED_GAMES_MODS_TEST_INSTALL").is_none()
        {
            return Err("Mod installation is disabled in this diagnostic profile".into());
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let _profile_lease =
            self.profile_lease(community(game).ok_or("Unsupported mod community")?)?;
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let (index, _) = self.catalog(slug, false)?;
        if index.find_index_by_full_name(package).is_none() {
            return Err("This mod is not in the game's Thunderstore community".into());
        }
        let profiles = self.profiles(slug);
        let selected = self.selected(slug, &profiles);
        let root = self.profile_root(slug, &self.selected_id(slug));
        let revisions = self.revisions_dir(game)?;
        fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .map_err(|e| e.to_string())?;
        if let Some(source) = selected.filter(|p| p.path.is_dir()) {
            profile::portability::ensure_disjoint_trees(&source.path, pending.path())
                .map_err(|e| e.to_string())?;
            copy_profile(&source.path, pending.path())?;
        }
        let mut reasons = dependencies::Reasons::read(pending.path())?;
        let before = modlist::read(pending.path()).map_err(|e| e.to_string())?;
        let client = self.client(slug)?;
        runtime()
            .block_on(profile::install_mod_in(
                pending.path(),
                &self.cache,
                &ECOSYSTEM,
                &index,
                &client,
                slug,
                package,
                modlist::now_millis(),
            ))
            .map_err(|e| format!("Could not install mod: {e}"))?;
        let installed = modlist::read(pending.path()).map_err(|e| e.to_string())?;
        if !installed.iter().any(|m| m.name == package && m.enabled) {
            return Err("The mod installer did not produce an enabled install record".into());
        }
        reasons.record_install(&before, &installed, package);
        reasons.write(pending.path())?;
        commit(&root, pending)?;
        self.prune_revisions(slug);
        self.snapshot(game, false)
    }
}

impl Manager {
    fn lovely_repairs(
        &self,
        slug: &str,
        source: &Path,
    ) -> Result<Vec<(String, bool, thunderstore_engine::install::InstallPlan)>, String> {
        use thunderstore_engine::{ecosystem::LoaderKind, install};
        let Some(schema) = ECOSYSTEM
            .game(slug)
            .and_then(|g| g.profile())
            .filter(|p| p.package_loader == LoaderKind::Lovely)
        else {
            return Ok(Vec::new());
        };
        let mut repairs = Vec::new();
        for entry in modlist::read(source).map_err(|e| e.to_string())? {
            if !entry.online_source || ECOSYSTEM.modloader_package(&entry.name).is_some() {
                continue;
            }
            let cache = profile::cache::package_cache_dir(
                &self.cache,
                slug,
                &entry.name,
                &entry.version_number.to_string(),
            );
            if !cache.is_dir() {
                // A complete profile can still launch after its download cache
                // was removed. A broken legacy install must be reinstalled.
                let folder = source.join("mods").join(&entry.name);
                if folder.is_dir() {
                    continue;
                }
                return Err(format!(
                    "Reinstall {} to restore its missing mod files",
                    entry.name
                ));
            }
            let plan = install::plan_install(&ECOSYSTEM, schema, &entry.name, &cache, None)
                .map_err(|e| e.to_string())?;
            if !install::plan_is_on_disk(&plan, source, &entry.name, entry.enabled) {
                repairs.push((entry.name, entry.enabled, plan));
            }
        }
        Ok(repairs)
    }

    /// Move existing revisions into the owning client’s accessible storage when needed.
    pub(super) fn prepare_selected_for_launch(&self, game: &LibraryGame) -> Result<(), String> {
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let _profile_lease = self.profile_lease(slug)?;
        let profiles = self.profiles(slug);
        let Some(source) = self.selected(slug, &profiles) else {
            return Ok(());
        };
        let revisions = self.revisions_dir(game)?;
        fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
        let repairs = self.lovely_repairs(slug, &source.path)?;
        if repairs.is_empty() && source.path.parent() == revisions.canonicalize().ok().as_deref() {
            return Ok(());
        }
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .map_err(|e| e.to_string())?;
        profile::portability::ensure_disjoint_trees(&source.path, pending.path())
            .map_err(|e| e.to_string())?;
        copy_profile(&source.path, pending.path())?;
        for (name, enabled, plan) in repairs {
            thunderstore_engine::install::apply_install(&plan, pending.path())
                .map_err(|e| e.to_string())?;
            if !enabled {
                thunderstore_engine::install::disable_mod(
                    ECOSYSTEM.game(slug).and_then(|g| g.profile()).unwrap(),
                    pending.path(),
                    &name,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        commit(&self.profile_root(slug, &self.selected_id(slug)), pending)?;
        self.prune_revisions(slug);
        Ok(())
    }

    pub(super) fn prune_revisions(&self, community: &str) {
        self.prune_profile_revisions(community, &self.selected_id(community));
    }

    pub(super) fn prune_profile_revisions(&self, community: &str, id: &str) {
        let current = self.own_profile_path(community, id);
        let mut in_use = Vec::new();
        for dir in fs::read_dir(self.data.join("staged"))
            .into_iter()
            .flatten()
            .flatten()
        {
            for file in ["receipt.json", "steam.json", "noita.json"] {
                if let Ok(bytes) = fs::read(dir.path().join(file))
                    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && let Some(profile) = value["profile"].as_str()
                {
                    in_use.push(PathBuf::from(profile));
                }
            }
        }
        let root = current
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.data.join(community).join("revisions"));
        let mut revisions = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|d| {
                d.file_name().to_string_lossy().starts_with("profile-")
                    && d.file_type().is_ok_and(|t| t.is_dir())
            })
            .collect::<Vec<_>>();
        revisions.sort_by_key(|d| std::cmp::Reverse(d.metadata().and_then(|m| m.modified()).ok()));
        // Retain current + previous revision and anything referenced by a launch.
        for revision in revisions.into_iter().skip(2) {
            let path = revision.path();
            if current.as_ref() != Some(&path) && !in_use.contains(&path) {
                // Another launcher/session may still use this revision even
                // when this manager has no staging receipt (e.g. script loaders).
                let Ok(_unused) = self.game_lease(&path) else {
                    continue;
                };
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}

pub(super) fn copy_profile(source: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let destination = target.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!(
                "Cannot safely copy a profile containing a symbolic link: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
            copy_profile(&entry.path(), &destination)?;
        } else if kind.is_file() {
            // Actual copies: an update in the staging tree must never mutate a
            // hard-linked source profile or a running game's plugin assembly.
            fs::copy(entry.path(), destination).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(super) fn commit(root: &Path, pending: tempfile::TempDir) -> Result<(), String> {
    let suffix = pending
        .path()
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid profile revision")?
        .trim_start_matches("pending-");
    let revision = format!("profile-{suffix}");
    let revisions = pending
        .path()
        .parent()
        .ok_or("Profile staging directory has no parent")?;
    let storage = revisions
        .parent()
        .ok_or("Profile storage directory has no parent")?;
    let destination = revisions.join(&revision);
    fs::rename(pending.path(), &destination).map_err(|e| e.to_string())?;
    let result = crate::storage::write_json(
        &root.join("current.json"),
        &serde_json::json!({
            "revision":revision, "storage":storage
        }),
    );
    if result.is_err() {
        let _ = fs::remove_dir_all(&destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lovely_install_preserves_nested_payload_and_supports_toggle_and_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let extracted = dir.path().join("extracted");
        fs::create_dir_all(extracted.join("src")).unwrap();
        fs::create_dir_all(extracted.join("lovely")).unwrap();
        fs::write(extracted.join("src/main.lua"), b"lua").unwrap();
        fs::write(extracted.join("lovely/patch.toml"), b"patch").unwrap();
        let target = dir.path().join("profile");
        let version = serde_json::from_value(
            serde_json::json!({"name":"Mod", "version_number":"1.0.0", "dependencies":[], "date_created":"2026-01-01T00:00:00Z"}),
        )
        .unwrap();
        profile::install_extracted_in(
            &target,
            &ECOSYSTEM,
            "balatro",
            "Owner-Mod",
            "Owner",
            &version,
            &extracted,
            None,
            0,
        )
        .unwrap();
        assert_eq!(
            fs::read(target.join("mods/Owner-Mod/src/main.lua")).unwrap(),
            b"lua"
        );
        assert!(target.join("mods/Owner-Mod/lovely/patch.toml").is_file());
        profile::set_enabled_in(&target, &ECOSYSTEM, "balatro", "Owner-Mod", false).unwrap();
        assert!(!target.join("mods/Owner-Mod/src/main.lua").exists());
        assert!(target.join("mods/Owner-Mod/src/main.lua.old").is_file());
        profile::set_enabled_in(&target, &ECOSYSTEM, "balatro", "Owner-Mod", true).unwrap();
        profile::uninstall_mod_in(&target, &ECOSYSTEM, "balatro", "Owner-Mod").unwrap();
        assert!(!target.join("mods/Owner-Mod").exists());
        assert!(modlist::read(&target).unwrap().is_empty());
    }

    #[test]
    fn lovely_launch_repairs_record_only_profiles_atomically_and_preserves_disabled_mods() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(&dir.path().join("data"), &dir.path().join("cache"));
        manager.leases = dir.path().join("leases");
        let game = crate::catalog::entry("steam", "2379780", "Balatro", vec![], "fixture");
        let root = manager.profile_root("balatro", "exposed-games");
        let revisions = manager.revisions_dir(&game).unwrap();
        fs::create_dir_all(&revisions).unwrap();
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .unwrap();
        let mut entries = Vec::new();
        for (name, enabled) in [("Owner-Enabled", true), ("Owner-Disabled", false)] {
            let version = serde_json::from_value(
                serde_json::json!({"name":name, "version_number":"1.0.0", "dependencies":[], "date_created":"2026-01-01T00:00:00Z"}),
            )
            .unwrap();
            let mut entry = profile::mod_entry_from_version(name, "Owner", &version, 42);
            entry.enabled = enabled;
            entries.push(entry);
            let cache = profile::cache::package_cache_dir(&manager.cache, "balatro", name, "1.0.0");
            fs::create_dir_all(cache.join("src")).unwrap();
            fs::write(cache.join("src/main.lua"), name).unwrap();
        }
        modlist::write(pending.path(), &entries).unwrap();
        commit(&root, pending).unwrap();
        let original = manager.own_path("balatro").unwrap();
        manager.prepare_selected_for_launch(&game).unwrap();
        let current = manager.own_path("balatro").unwrap();
        assert_ne!(original, current);
        assert!(!original.join("mods").exists());
        assert!(current.join("mods/Owner-Enabled/src/main.lua").is_file());
        assert!(
            current
                .join("mods/Owner-Disabled/src/main.lua.old")
                .is_file()
        );
        assert!(!current.join("mods/Owner-Disabled/src/main.lua").exists());
        assert_eq!(modlist::read(&current).unwrap()[0].installed_at_time, 42);
        manager.prepare_selected_for_launch(&game).unwrap();
        assert_eq!(manager.own_path("balatro").unwrap(), current);
    }

    #[test]
    fn cancelled_staging_leaves_the_live_profile_unchanged_and_commit_publishes_a_complete_revision()
     {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let revisions = root.join("revisions");
        fs::create_dir_all(&revisions).unwrap();
        let first = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .unwrap();
        fs::write(first.path().join("mod.dll"), b"old").unwrap();
        commit(root, first).unwrap();
        let original = fs::read(root.join("current.json")).unwrap();
        let old: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let source = revisions.join(old["revision"].as_str().unwrap());
        {
            let staging = tempfile::Builder::new()
                .prefix("pending-")
                .tempdir_in(&revisions)
                .unwrap();
            copy_profile(&source, staging.path()).unwrap();
            fs::write(staging.path().join("mod.dll"), b"partial").unwrap();
        }
        assert_eq!(fs::read(root.join("current.json")).unwrap(), original);
        assert_eq!(fs::read(source.join("mod.dll")).unwrap(), b"old");
        let staging = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .unwrap();
        copy_profile(&source, staging.path()).unwrap();
        fs::write(staging.path().join("mod.dll"), b"new").unwrap();
        commit(root, staging).unwrap();
        let current: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("current.json")).unwrap()).unwrap();
        assert_eq!(
            fs::read(
                revisions
                    .join(current["revision"].as_str().unwrap())
                    .join("mod.dll")
            )
            .unwrap(),
            b"new"
        );
        assert_eq!(fs::read(source.join("mod.dll")).unwrap(), b"old");
    }
}
