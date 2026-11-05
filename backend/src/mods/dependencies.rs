use super::*;
use std::collections::{BTreeSet, HashSet};

const FILE: &str = "raesir-install-reasons.json";

/// Kept inside each revision so files, dependency edges and install reasons
/// are published together. Missing reasons mean user-managed, never automatic.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Reasons {
    automatic: BTreeSet<String>,
}

impl Reasons {
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        match fs::read(path.join(FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                format!("Could not read mod install reasons; existing files were kept: {e}")
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Could not read mod install reasons: {e}")),
        }
    }

    pub(super) fn write(&self, path: &Path) -> Result<(), String> {
        crate::storage::write_json(&path.join(FILE), self)
    }

    pub(super) fn record_install(
        &mut self,
        before: &[modlist::ProfileMod],
        after: &[modlist::ProfileMod],
        requested: &str,
    ) {
        let previous: HashSet<_> = before.iter().map(|m| m.name.as_str()).collect();
        self.automatic
            .retain(|id| after.iter().any(|m| &m.name == id));
        for m in after {
            if m.online_source && m.name != requested && !previous.contains(m.name.as_str()) {
                self.automatic.insert(m.name.clone());
            }
        }
        // Explicitly requesting a previously automatic dependency pins it.
        self.automatic.remove(requested);
    }

    pub(super) fn record_update(
        &mut self,
        before: &[modlist::ProfileMod],
        after: &[modlist::ProfileMod],
    ) {
        let old = before
            .iter()
            .map(|m| m.name.as_str())
            .collect::<HashSet<_>>();
        self.automatic
            .retain(|id| after.iter().any(|m| &m.name == id));
        for entry in after {
            if entry.online_source && !old.contains(entry.name.as_str()) {
                self.automatic.insert(entry.name.clone());
            }
        }
    }

    pub(super) fn forget(&mut self, mods: &[InstalledMod]) {
        for m in mods {
            self.automatic.remove(&m.id);
        }
    }

    pub(super) fn installed(&self, mods: Vec<modlist::ProfileMod>) -> Vec<InstalledMod> {
        mods.into_iter()
            .map(|m| InstalledMod {
                automatic: m.online_source && self.automatic.contains(&m.name),
                id: m.name,
                name: m.display_name,
                owner: m.author_name,
                version: m.version_number.to_string(),
                enabled: m.enabled,
                local: !m.online_source,
                dependencies: m.dependencies,
            })
            .collect()
    }
}

fn dependency_id(value: &str) -> Option<String> {
    // Thunderstore identifiers are Owner-Name-Version; owner/name may contain
    // underscores, but not hyphens. Preserve any prerelease suffix on Version.
    let (owner, rest) = value.split_once('-')?;
    let (name, _) = rest.split_once('-')?;
    Some(format!("{owner}-{name}"))
}

impl Profile {
    /// The requested mod and automatic dependencies no remaining user mod needs.
    /// Disabled mods still retain their dependencies, as do unknown/local mods.
    pub fn removal_plan(&self, package: &str) -> Result<Vec<InstalledMod>, String> {
        let requested = self
            .installed
            .iter()
            .find(|m| m.id == package)
            .ok_or("This mod is not installed in this profile")?;
        let by_id: HashMap<_, _> = self.installed.iter().map(|m| (m.id.as_str(), m)).collect();
        let mut keep = HashSet::new();
        let mut pending = self
            .installed
            .iter()
            .filter(|m| m.id != package && (!m.automatic || m.local))
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        while let Some(id) = pending.pop() {
            if !keep.insert(id.clone()) {
                continue;
            }
            if let Some(m) = by_id.get(id.as_str()) {
                pending.extend(m.dependencies.iter().filter_map(|d| dependency_id(d)));
            }
        }
        let dependents = self
            .installed
            .iter()
            .filter(|m| {
                m.id != package
                    && m.dependencies
                        .iter()
                        .any(|d| dependency_id(d).as_deref() == Some(package))
            })
            .map(|m| m.name.replace('_', " "))
            .collect::<Vec<_>>();
        if keep.contains(package) {
            return Err(format!(
                "Required by {}. Remove those mods first.",
                dependents.join(", ")
            ));
        }
        let mut orphans = self
            .installed
            .iter()
            .filter(|m| m.id != package && m.automatic && !m.local && !keep.contains(&m.id))
            .cloned()
            .collect::<Vec<_>>();
        orphans.sort_by(|a, b| a.id.cmp(&b.id));
        let mut removed = vec![requested.clone()];
        removed.extend(orphans);
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dependencies: &[&str]) -> modlist::ProfileMod {
        let version: thunderstore_engine::models::Version =
            serde_json::from_value(serde_json::json!({
                "name":name, "version_number":"1.0.0", "dependencies":dependencies,
                "date_created":"1970-01-01T00:00:00Z"
            }))
            .unwrap();
        thunderstore_engine::profile::mod_entry_from_version(
            &format!("Owner-{name}"),
            "Owner",
            &version,
            0,
        )
    }

    fn profile(entries: Vec<modlist::ProfileMod>, automatic: &[&str]) -> Profile {
        let reasons = Reasons {
            automatic: automatic.iter().map(|id| format!("Owner-{id}")).collect(),
        };
        Profile {
            id: "test".into(),
            name: "Test".into(),
            source: "Raesir".into(),
            path: PathBuf::new(),
            active: true,
            installed: reasons.installed(entries),
        }
    }

    fn names(profile: &Profile, package: &str) -> Vec<String> {
        profile
            .removal_plan(&format!("Owner-{package}"))
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect()
    }

    #[test]
    fn tracks_only_new_dependencies_and_explicit_requests_pin_automatic_mods() {
        let old = entry("Old", &[]);
        let dependency = entry("Dependency", &[]);
        let root = entry("Root", &["Owner-Dependency-1.0.0"]);
        let after = vec![old.clone(), dependency, root];
        let mut reasons = Reasons::default();
        reasons.record_install(&[old], &after, "Owner-Root");
        assert_eq!(
            reasons.automatic,
            BTreeSet::from(["Owner-Dependency".into()])
        );
        reasons.record_install(&after, &after, "Owner-Dependency");
        assert!(reasons.automatic.is_empty());
        let dir = tempfile::tempdir().unwrap();
        reasons.write(dir.path()).unwrap();
        assert!(Reasons::read(dir.path()).unwrap().automatic.is_empty());
    }

    #[test]
    fn removes_transitive_orphans_but_preserves_shared_and_explicit_dependencies() {
        let mods = vec![
            entry(
                "Root",
                &[
                    "Owner-Bridge-1.0.0",
                    "Owner-Shared-1.0.0",
                    "Owner-Explicit-1.0.0",
                ],
            ),
            entry("Bridge", &["Owner-Leaf-1.0.0"]),
            entry("Leaf", &[]),
            entry("Shared", &[]),
            entry("Explicit", &[]),
            entry("Other", &["Owner-Shared-1.0.0"]),
        ];
        let mut p = profile(mods, &["Bridge", "Leaf", "Shared"]);
        p.installed
            .iter_mut()
            .find(|m| m.id == "Owner-Other")
            .unwrap()
            .enabled = false;
        assert_eq!(
            names(&p, "Root"),
            ["Owner-Root", "Owner-Bridge", "Owner-Leaf"]
        );
        assert!(
            p.removal_plan("Owner-Shared")
                .unwrap_err()
                .contains("Other")
        );
        assert!(p.removal_plan("Owner-Leaf").unwrap_err().contains("Bridge"));
    }

    #[test]
    fn handles_cycles_and_preserves_untracked_and_local_mods() {
        let mods = vec![
            entry("Root", &["Owner-A-1.0.0"]),
            entry("A", &["Owner-B-1.0.0-beta-1"]),
            entry("B", &["Owner-A-1.0.0"]),
            entry("Unknown", &[]),
            entry("Local", &[]),
        ];
        let mut p = profile(mods, &["A", "B", "Local"]);
        p.installed
            .iter_mut()
            .find(|m| m.id == "Owner-Local")
            .unwrap()
            .local = true;
        assert_eq!(names(&p, "Root"), ["Owner-Root", "Owner-A", "Owner-B"]);
        p.installed[2].dependencies.push("Owner-Root-1.0.0".into());
        assert_eq!(names(&p, "Root"), ["Owner-Root", "Owner-A", "Owner-B"]);
        assert!(p.removal_plan("Owner-missing").is_err());
    }

    #[test]
    fn cleanup_is_published_atomically_and_corrupt_reasons_do_not_mutate_live_files() {
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
        let root = manager.profile_root("peak", "raesir");
        let original = root.join("revisions/profile-original");
        let entries = vec![
            entry("Root", &["Owner-Dependency-1.0.0"]),
            entry("Dependency", &[]),
        ];
        modlist::write(&original, &entries).unwrap();
        for name in ["Root", "Dependency"] {
            let path = original.join(format!("BepInEx/plugins/Owner-{name}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("mod.dll"), name).unwrap();
        }
        crate::storage::write_json(
            &root.join("current.json"),
            &serde_json::json!({"revision":"profile-original"}),
        )
        .unwrap();
        let pointer = fs::read(root.join("current.json")).unwrap();
        fs::write(original.join(FILE), b"corrupt").unwrap();
        assert!(
            manager
                .uninstall(&game, "Owner-Root")
                .unwrap_err()
                .contains("install reasons")
        );
        assert_eq!(fs::read(root.join("current.json")).unwrap(), pointer);
        assert!(original.join("BepInEx/plugins/Owner-Root/mod.dll").exists());
        let mut reasons = Reasons::default();
        reasons.record_install(&[], &entries, "Owner-Root");
        reasons.write(&original).unwrap();
        assert!(manager.profiles("peak")[0].installed[1].automatic);
        assert_eq!(
            names(&manager.profiles("peak")[0], "Root"),
            ["Owner-Root", "Owner-Dependency"]
        );
        // Failure removing the second package must roll back the first too.
        let state = thunderstore_engine::install::state_file_path(&original, "Owner-Dependency");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, b"invalid state").unwrap();
        assert!(manager.uninstall(&game, "Owner-Root").is_err());
        assert_eq!(fs::read(root.join("current.json")).unwrap(), pointer);
        assert!(original.join("BepInEx/plugins/Owner-Root/mod.dll").exists());
        assert_eq!(modlist::read(&original).unwrap().len(), 2);
        fs::remove_file(state).unwrap();
        manager.uninstall(&game, "Owner-Root").unwrap();
        let current = manager.own_path("peak").unwrap();
        assert_ne!(current, original);
        assert!(modlist::read(&current).unwrap().is_empty());
        assert!(Reasons::read(&current).unwrap().automatic.is_empty());
        assert!(
            !current
                .join("BepInEx/plugins/Owner-Dependency/mod.dll")
                .exists()
        );
        assert!(
            original
                .join("BepInEx/plugins/Owner-Dependency/mod.dll")
                .exists()
        );
    }
}
