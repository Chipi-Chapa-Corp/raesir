use super::*;
use thunderstore_engine::profile::layout;

#[derive(Clone, Serialize, Deserialize)]
struct NamedProfile {
    id: String,
    name: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Registry {
    selected: String,
    profiles: Vec<NamedProfile>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            selected: "raesir".into(),
            profiles: vec![NamedProfile {
                id: "raesir".into(),
                name: "Default".into(),
            }],
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    layout::validate_name(name).map_err(|e| e.to_string())?;
    if name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("Use a profile name of up to 80 characters".into());
    }
    Ok(name.into())
}

impl Manager {
    pub(super) fn registry(&self, community: &str) -> Result<Registry, String> {
        let bytes = match fs::read(self.data.join(community).join("profiles.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::default()),
            Err(e) => return Err(format!("Could not read mod profiles: {e}")),
        };
        let registry: Registry = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Could not read mod profiles: {e}"))?;
        let mut ids = std::collections::HashSet::new();
        if registry.profiles.is_empty()
            || registry
                .profiles
                .iter()
                .any(|p| !valid_id(&p.id) || !ids.insert(&p.id))
            || !registry.profiles.iter().any(|p| p.id == registry.selected)
        {
            return Err("Invalid mod profile registry; existing files were kept".into());
        }
        Ok(registry)
    }

    fn save_registry(&self, community: &str, registry: &Registry) -> Result<(), String> {
        crate::storage::write_json(&self.data.join(community).join("profiles.json"), registry)
    }

    pub(super) fn selected_id(&self, community: &str) -> String {
        self.registry(community)
            .map_or_else(|_| "raesir".into(), |r| r.selected)
    }

    pub(super) fn profile_root(&self, community: &str, id: &str) -> PathBuf {
        if id == "raesir" {
            self.data.join(community)
        } else {
            layout::profile_dir(&self.data, community, id)
        }
    }

    pub fn profiles(&self, community: &str) -> Vec<Profile> {
        let Ok(registry) = self.registry(community) else {
            return Vec::new();
        };
        registry
            .profiles
            .into_iter()
            .map(|p| {
                let path = self
                    .own_profile_path(community, &p.id)
                    .unwrap_or_else(|| self.profile_root(community, &p.id).join("empty"));
                // Browsing remains conservative if metadata is unreadable;
                // mutations report the error instead of guessing.
                let installed = dependencies::Reasons::read(&path)
                    .unwrap_or_default()
                    .installed(modlist::read(&path).unwrap_or_default());
                Profile {
                    active: p.id == registry.selected,
                    id: p.id,
                    name: p.name,
                    source: "Raesir".into(),
                    path,
                    installed,
                }
            })
            .collect()
    }

    pub fn selected<'a>(&self, community: &str, profiles: &'a [Profile]) -> Option<&'a Profile> {
        let id = self.selected_id(community);
        profiles.iter().find(|p| p.id == id)
    }

    pub fn select(&self, game: &LibraryGame, id: &str) -> Result<(), String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let mut registry = self.registry(slug)?;
        if !registry.profiles.iter().any(|p| p.id == id) {
            return Err("Mod profile no longer exists".into());
        }
        registry.selected = id.into();
        self.save_registry(slug, &registry)
    }

    pub fn create_profile(&self, game: &LibraryGame, name: &str) -> Result<(), String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let name = valid_name(name)?;
        let mut registry = self.registry(slug)?;
        if registry
            .profiles
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&name))
        {
            return Err("A profile with this name already exists".into());
        }
        let reservation = tempfile::Builder::new()
            .prefix("profile-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let id = reservation
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let path = layout::create(&self.data, slug, &id).map_err(|e| e.to_string())?;
        registry.profiles.push(NamedProfile {
            id: id.clone(),
            name,
        });
        registry.selected = id;
        if let Err(error) = self.save_registry(slug, &registry) {
            let _ = fs::remove_dir(path);
            return Err(error);
        }
        Ok(())
    }

    pub fn rename_profile(&self, game: &LibraryGame, id: &str, name: &str) -> Result<(), String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let name = valid_name(name)?;
        let mut registry = self.registry(slug)?;
        if registry
            .profiles
            .iter()
            .any(|p| p.id != id && p.name.eq_ignore_ascii_case(&name))
        {
            return Err("A profile with this name already exists".into());
        }
        let profile = registry
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("Mod profile no longer exists")?;
        profile.name = name;
        // Stable IDs keep loader paths, installed mods and running sessions intact.
        self.save_registry(slug, &registry)
    }

    pub fn delete_profile(&self, game: &LibraryGame, id: &str) -> Result<(), String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let mut registry = self.registry(slug)?;
        if !registry.profiles.iter().any(|p| p.id == id) {
            return Err("Mod profile no longer exists".into());
        }
        if registry.profiles.len() == 1 {
            return Err("Keep at least one mod profile".into());
        }
        let _lease = self.modification_lease(game)?;
        let revisions = self
            .own_profile_path(slug, id)
            .and_then(|p| p.parent().map(Path::to_path_buf));
        registry.profiles.retain(|p| p.id != id);
        if registry.selected == id {
            registry.selected = registry.profiles[0].id.clone();
        }
        self.save_registry(slug, &registry)?;
        if let Some(path) = revisions {
            fs::remove_dir_all(path)
                .map_err(|e| format!("Profile deleted, but its files could not be removed: {e}"))?;
        }
        if id == "raesir" {
            let _ = fs::remove_file(self.data.join(slug).join("current.json"));
        } else {
            layout::delete(&self.data, slug, id).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(super) fn modification_lease(
        &self,
        game: &LibraryGame,
    ) -> Result<Option<fs::File>, String> {
        let install = game.installations.iter().find(|i| {
            i.os == crate::model::Os::host() && matches!(i.state.as_str(), "installed" | "external")
        });
        let Some(install) = install else {
            return Ok(None);
        };
        let dir = Path::new(&install.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let lease = self.game_lease(&dir)?;
        if launch::game_running(&dir)
            || self
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&dir)
        {
            return Err("Close the game before changing its mods or deleting profiles".into());
        }
        Ok(Some(lease))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(
        name: &str,
        dependencies: Vec<&str>,
    ) -> thunderstore_engine::profile::modlist::ProfileMod {
        let version: thunderstore_engine::models::Version = serde_json::from_value(serde_json::json!({
            "name": name, "version_number":"1.0.0", "dependencies":dependencies, "date_created":"1970-01-01T00:00:00Z"
        })).unwrap();
        thunderstore_engine::profile::mod_entry_from_version(
            &format!("Owner-{name}"),
            "Owner",
            &version,
            0,
        )
    }

    #[test]
    fn uninstall_and_default_profile_deletion_preserve_the_other_profile() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(&dir.path().join("data"), &dir.path().join("cache"));
        let game = crate::catalog::entry("steam", "3527290", "Fixture", vec![], "test");
        manager.catalogs.lock().unwrap().insert(
            "peak".into(),
            Catalog {
                updated: Instant::now(),
                index: Arc::new(Vec::<thunderstore_engine::models::Package>::new().into()),
                packages: Arc::new(Vec::new()),
            },
        );
        let original = manager
            .profile_root("peak", "raesir")
            .join("revisions/profile-original");
        let entries = vec![
            entry("Dependency", vec![]),
            entry("Test", vec!["Owner-Dependency-1.0.0"]),
        ];
        modlist::write(&original, &entries).unwrap();
        for name in ["Dependency", "Test"] {
            let path = original.join(format!("BepInEx/plugins/Owner-{name}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("mod.dll"), name).unwrap();
        }
        crate::storage::write_json(
            &manager.profile_root("peak", "raesir").join("current.json"),
            &serde_json::json!({"revision":"profile-original"}),
        )
        .unwrap();
        manager.create_profile(&game, "Other").unwrap();
        let other = manager.selected_id("peak");
        let copy = manager
            .profile_root("peak", &other)
            .join("revisions/profile-copy");
        fs::create_dir_all(&copy).unwrap();
        super::super::install::copy_profile(&original, &copy).unwrap();
        crate::storage::write_json(
            &manager.profile_root("peak", &other).join("current.json"),
            &serde_json::json!({"revision":"profile-copy"}),
        )
        .unwrap();
        let pointer = fs::read(manager.profile_root("peak", &other).join("current.json")).unwrap();
        assert!(manager.uninstall(&game, "Owner-Dependency").is_err());
        assert_eq!(
            fs::read(manager.profile_root("peak", &other).join("current.json")).unwrap(),
            pointer
        );
        manager.uninstall(&game, "Owner-Test").unwrap();
        let selected = manager.own_path("peak").unwrap();
        assert!(!selected.join("BepInEx/plugins/Owner-Test").exists());
        assert_eq!(
            fs::read(selected.join("BepInEx/plugins/Owner-Dependency/mod.dll")).unwrap(),
            b"Dependency"
        );
        assert!(original.join("BepInEx/plugins/Owner-Test/mod.dll").exists());
        manager.delete_profile(&game, "raesir").unwrap();
        assert!(!original.exists());
        assert_eq!(manager.selected_id("peak"), other);
        assert_eq!(manager.profiles("peak")[0].installed.len(), 1);
        assert!(
            selected
                .join("BepInEx/plugins/Owner-Dependency/mod.dll")
                .exists()
        );
    }

    #[test]
    fn locked_games_and_corrupt_registries_are_kept_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(&dir.path().join("data"), &dir.path().join("cache"));
        manager.leases = dir.path().join("leases");
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let game = crate::catalog::entry(
            "steam",
            "3527290",
            "Fixture",
            vec![crate::model::Installation {
                library: "fixture".into(),
                path: game_dir.to_string_lossy().into(),
                os: crate::model::Os::host(),
                state: "installed".into(),
                last_seen: 0,
            }],
            "test",
        );
        manager.create_profile(&game, "Other").unwrap();
        let id = manager.selected_id("peak");
        let lease = manager.game_lease(&game_dir).unwrap();
        assert!(manager.delete_profile(&game, &id).is_err());
        assert_eq!(manager.profiles("peak").len(), 2);
        drop(lease);
        manager.delete_profile(&game, &id).unwrap();
        let registry = manager.data.join("peak/profiles.json");
        fs::write(&registry, b"invalid registry").unwrap();
        assert!(manager.create_profile(&game, "New").is_err());
        assert_eq!(fs::read(registry).unwrap(), b"invalid registry");
    }
    #[test]
    fn profiles_are_independent_and_names_do_not_move_installed_files() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(&dir.path().join("data"), &dir.path().join("cache"));
        let game = crate::catalog::entry("steam", "3527290", "Fixture", vec![], "test");
        let path = manager.data.join("peak/revisions/profile-original");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("mod.dll"), b"keep original").unwrap();
        crate::storage::write_json(
            &manager.data.join("peak/current.json"),
            &serde_json::json!({"revision":"profile-original", "selected":"foreign:1"}),
        )
        .unwrap();
        assert_eq!(manager.profiles("peak").len(), 1);
        manager.create_profile(&game, "New profile").unwrap();
        let id = manager.selected_id("peak");
        assert_ne!(id, "raesir");
        assert!(
            manager
                .selected("peak", &manager.profiles("peak"))
                .unwrap()
                .installed
                .is_empty()
        );
        manager.rename_profile(&game, &id, "Renamed").unwrap();
        assert_eq!(manager.selected_id("peak"), id);
        manager.delete_profile(&game, &id).unwrap();
        assert_eq!(manager.selected_id("peak"), "raesir");
        assert_eq!(fs::read(path.join("mod.dll")).unwrap(), b"keep original");
        assert!(manager.delete_profile(&game, "raesir").is_err());
        assert!(manager.create_profile(&game, "../escape").is_err());
        assert!(manager.create_profile(&game, " ").is_err());
        assert!(manager.rename_profile(&game, "missing", "Name").is_err());
    }
}
