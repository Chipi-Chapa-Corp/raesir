use super::*;
use crate::model::Os;
use modde_core::installer::{self, InstallMethod};
use modde_games::GameRegistration;
use std::{collections::HashSet, io};

pub(super) fn registration(game: &LibraryGame) -> Option<&'static GameRegistration> {
    if !can_browse_nexus(game) {
        return None;
    }
    let mut matches = modde_games::all_games().iter().filter(|entry| {
        let id = match game.store.as_str() {
            "steam" => entry.launcher.steam_app_id,
            "epic" => entry.launcher.heroic_epic_app_id,
            "gog" => entry.launcher.heroic_gog_app_id,
            _ => None,
        };
        id == Some(&game.id) && entry.nexus_domain.is_some()
    });
    let entry = matches.next()?;
    matches.next().is_none().then_some(entry)
}

pub(super) struct Target {
    rule: &'static GameRegistration,
    root: PathBuf,
    mod_root: PathBuf,
}

impl Target {
    pub(super) fn resolve(game: &LibraryGame) -> Option<Self> {
        let rule = registration(game)?;
        let mut paths = game
            .installations
            .iter()
            .filter(|i| i.os == Os::host() && i.state == "installed")
            .map(|i| Path::new(&i.path))
            .filter(|p| p.is_absolute())
            .filter_map(|p| p.canonicalize().ok())
            .filter(|p| p.is_dir());
        let root = paths.next()?;
        if paths.any(|other| other != root) {
            return None;
        }
        let mod_root = rule.plugin.mod_root_at(&root, None).ok()?;
        let mod_root = mod_root.strip_prefix(&root).ok()?.to_owned();
        Some(Self {
            rule,
            root,
            mod_root,
        })
    }

    pub(super) fn domain(&self) -> &'static str {
        self.rule.nexus_domain.expect("Nexus registration")
    }

    fn receipt_dir(&self, manager: &Manager) -> PathBuf {
        manager
            .data
            .join("modde")
            .join(self.rule.game_id)
            .join(launch::hash(self.root.to_string_lossy().as_bytes()))
    }

    pub(super) fn profile(&self, manager: &Manager) -> Result<Profile, String> {
        let path = self.receipt_dir(manager);
        let mut installed = Vec::new();
        match fs::read_dir(&path) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                        && let Some(receipt) = Receipt::read(&path)?
                        && receipt.complete
                    {
                        installed.push(receipt.package);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        installed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Profile {
            id: "modde".into(),
            name: "Installed on this copy".into(),
            source: "Modde".into(),
            path,
            installed,
            active: true,
        })
    }

    pub(super) fn install(
        &self,
        manager: &Manager,
        id: u64,
        source: &Path,
        name: String,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
        let extracted = temp.path().join("archive");
        archive::extract(source, &extracted)?;
        let staged = temp.path().join(format!("nexus-{id}"));
        let package_files = self.stage(id, source, &extracted, &staged)?;
        let _lease = manager.game_lease(&self.root)?;
        if cancelled() || launch::game_running(&self.root) {
            return Err("Close the game before installing its mods".into());
        }
        let path = self.receipt_dir(manager).join(format!("{id}.json"));
        if let Some(previous) = Receipt::read(&path)? {
            if previous.complete {
                return Err(
                    "This mod is already installed. Uninstall it before replacing it.".into(),
                );
            }
            previous.remove_files(&self.root)?;
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }

        for (file, _) in &package_files {
            let target = archive::destination(&self.root, &file.relative)?;
            if target.symlink_metadata().is_ok() {
                return Err(format!(
                    "The game already has {}. No files were replaced.",
                    file.relative.display()
                ));
            }
        }
        let mut receipt = Receipt {
            package: InstalledMod {
                id: format!("nexus:{id}"),
                name,
                owner: String::new(),
                version: String::new(),
                enabled: true,
                local: false,
                dependencies: Vec::new(),
                automatic: false,
            },
            complete: false,
            files: package_files.iter().map(|(file, _)| file.clone()).collect(),
        };
        crate::storage::write_json(&path, &receipt)?;
        let mut published = 0;
        let result = (|| {
            for (file, source) in &package_files {
                let target = archive::destination(&self.root, &file.relative)?;
                publish_file(&staged.join(source), &target)?;
                published += 1;
            }
            receipt.complete = true;
            crate::storage::write_json(&path, &receipt)
        })();
        if let Err(error) = result {
            receipt.complete = false;
            receipt.files.truncate(published);
            crate::storage::write_json(&path, &receipt)?;
            receipt.remove_files(&self.root)?;
            fs::remove_file(path).map_err(|e| e.to_string())?;
            return Err(error);
        }
        Ok(())
    }

    fn stage(
        &self,
        id: u64,
        source: &Path,
        extracted: &Path,
        staged: &Path,
    ) -> Result<Vec<(InstalledFile, PathBuf)>, String> {
        let plugin = self.rule.plugin;
        let source_hash = installer::xxh64_file_hex(source).map_err(|e| e.to_string())?;
        let mut plan = installer::analyze(extracted, &modde_games::game_probe(plugin), source_hash)
            .map_err(|e| e.to_string())?;
        if !plan.method.is_ready() {
            return Err(match plan.method {
                InstallMethod::Unknown { .. } => {
                    "Modde could not recognize this archive layout".into()
                }
                _ => format!(
                    "This archive needs a {} installer with file choices",
                    plan.method.label()
                ),
            });
        }
        let base = match &plan.method {
            InstallMethod::MultiRootOverlay { .. } => PathBuf::new(),
            InstallMethod::DllOverlay { .. } => plugin
                .executable_dir(&self.root)
                .strip_prefix(&self.root)
                .map_err(|_| "This loader uses a directory outside the game")?
                .to_owned(),
            InstallMethod::BareExtract => {
                let effective = plan
                    .strip_prefix
                    .as_ref()
                    .map_or_else(|| extracted.to_owned(), |p| extracted.join(p));
                self.bare_root(&effective)?
            }
            InstallMethod::DirectoryModFromXml {
                marker,
                id_attr,
                fallback_name,
            } => {
                let effective = plan
                    .strip_prefix
                    .as_ref()
                    .map_or_else(|| extracted.to_owned(), |p| extracted.join(p));
                let marker = archive::destination(&effective, marker)?;
                let xml = fs::read_to_string(marker).map_err(|e| e.to_string())?;
                let name = roxmltree::Document::parse(&xml)
                    .ok()
                    .and_then(|doc| {
                        doc.descendants()
                            .find_map(|node| node.attribute(id_attr.as_str()).map(str::to_owned))
                    })
                    .or_else(|| fallback_name.clone())
                    .unwrap_or_else(|| format!("nexus-{id}"));
                if Path::new(&name).components().count() != 1 {
                    return Err("The module has an invalid directory name".into());
                }
                archive::destination(staged, Path::new(&name))?;
                plan.method = InstallMethod::DirectoryMod {
                    directory_name: Some(name),
                };
                self.mod_root.clone()
            }
            InstallMethod::REDmod { .. }
            | InstallMethod::UserConfigOverlay { .. }
            | InstallMethod::ScriptMerge { .. } => {
                return Err(format!(
                    "This archive needs the {} deployment step in Modde",
                    plan.method.label()
                ));
            }
            _ => self.mod_root.clone(),
        };
        let dll_overlay = matches!(&plan.method, InstallMethod::DllOverlay { .. });
        installer::execute(&mut plan, extracted, staged).map_err(|e| e.to_string())?;
        let mut destinations = HashSet::new();
        let mut files = Vec::new();
        for file in plan.staged_files {
            let source = PathBuf::from(file.rel_path);
            let file_path = archive::destination(staged, &source)?;
            let payload = if dll_overlay {
                source
                    .strip_prefix("__dll_overlay__")
                    .map_err(|_| "Invalid loader overlay path")?
            } else {
                &source
            };
            let relative = existing_case(&self.root, &base.join(payload))?;
            if !destinations.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("The mod contains conflicting filenames".into());
            }
            let hash = installer::xxh64_file_hex(&file_path).map_err(|e| e.to_string())?;
            files.push((InstalledFile { relative, hash }, source));
        }
        if files.is_empty() {
            return Err("The archive has no installable files".into());
        }
        Ok(files)
    }

    fn bare_root(&self, layout: &Path) -> Result<PathBuf, String> {
        // Some recognized bare archives mirror game-root directories even when
        // the plugin also exposes a separate mod folder. Require the installed
        // layout to confirm that routing before placing files at the root.
        let mut directories = Vec::new();
        for entry in fs::read_dir(layout).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                directories.push(entry.file_name());
            } else if !["txt", "md"].iter().any(|ext| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|p| p.eq_ignore_ascii_case(ext))
            }) {
                return Ok(self.mod_root.clone());
            }
        }
        if !directories.is_empty()
            && directories.iter().all(|dir| {
                existing_case(&self.root, Path::new(dir))
                    .is_ok_and(|rel| self.root.join(rel).is_dir())
                    && !self.root.join(&self.mod_root).join(dir).is_dir()
            })
        {
            Ok(PathBuf::new())
        } else {
            Ok(self.mod_root.clone())
        }
    }

    pub(super) fn uninstall(&self, manager: &Manager, id: u64) -> Result<(), String> {
        let _lease = manager.game_lease(&self.root)?;
        if launch::game_running(&self.root) {
            return Err("Close the game before removing its mods".into());
        }
        let path = self.receipt_dir(manager).join(format!("{id}.json"));
        let receipt = Receipt::read(&path)?.ok_or("This mod has no Raesir install receipt")?;
        receipt.remove_files(&self.root)?;
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

fn existing_case(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    archive::destination(root, relative)?;
    let path = modde_core::fs::case_match_path(root, relative).map_err(|e| e.to_string())?;
    let resolved = path.strip_prefix(root).map_err(|_| "Invalid mod path")?;
    archive::destination(root, resolved)?;
    Ok(resolved.to_owned())
}

fn publish_file(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("Mod file has no parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    fs::copy(source, temp.path()).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist_noclobber(target)
        .map_err(|e| format!("Could not install {}: {e}", target.display()))?;
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
struct InstalledFile {
    relative: PathBuf,
    hash: String,
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    package: InstalledMod,
    complete: bool,
    files: Vec<InstalledFile>,
}

impl Receipt {
    fn read(path: &Path) -> Result<Option<Self>, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn remove_files(&self, root: &Path) -> Result<(), String> {
        let mut paths = Vec::new();
        for file in &self.files {
            let path = archive::destination(root, &file.relative)?;
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    let hash = installer::xxh64_file_hex(&path).map_err(|e| e.to_string())?;
                    if hash != file.hash {
                        return Err(format!(
                            "The mod file {} was changed since installation",
                            file.relative.display()
                        ));
                    }
                    paths.push(path);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => {
                    return Err(format!(
                        "Could not verify mod file {}",
                        file.relative.display()
                    ));
                }
            }
        }
        for path in paths {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Installation;
    use std::io::Write;

    fn game(store: &str, id: &str, root: &Path) -> LibraryGame {
        crate::catalog::entry(
            store,
            id,
            "Display name is irrelevant",
            vec![Installation {
                library: root.to_string_lossy().into_owned(),
                path: root.to_string_lossy().into_owned(),
                os: Os::host(),
                state: "installed".into(),
                last_seen: 0,
            }],
            "fixture",
        )
    }

    fn write_archive(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (path, contents) in files {
            zip.start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn registry_matches_store_and_local_edition() {
        let temp = tempfile::tempdir().unwrap();
        let mut cyberpunk = game("steam", "1091500", temp.path());
        assert_eq!(
            Target::resolve(&cyberpunk).unwrap().domain(),
            "cyberpunk2077"
        );
        assert!(Target::resolve(&game("epic", "1091500", temp.path())).is_none());
        assert!(Target::resolve(&game("xbox", "1091500", temp.path())).is_none());
        cyberpunk.installations[0].state = "unavailable".into();
        assert!(Target::resolve(&cyberpunk).is_none());
    }

    #[test]
    fn standard_stardew_archive_uses_the_same_nexus_importer() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        let mut manager = Manager::new(&workspace.join("data"), &workspace.join("cache"));
        manager.leases = workspace.join("leases");
        let root = workspace.join("game");
        fs::create_dir(&root).unwrap();
        let steam = game("steam", "413150", &root);
        let xbox = game("xbox", "9MWR1NC6VQ6L", &root);
        let archive = workspace.join("fixture.zip");
        let manifest = r#"{"Name":"Fixture","UniqueID":"Test.Fixture","Version":"1.0.0"}"#;
        write_archive(&archive, &[("Fixture/manifest.json", manifest)]);
        let url = "https://www.nexusmods.com/stardewvalley/mods/42";
        assert!(manager.import_nexus(&xbox, url, &archive, "").is_err());
        let snapshot = manager.import_nexus(&steam, url, &archive, "").unwrap();
        assert_eq!(snapshot.profiles[0].source, "Modde");
        assert_eq!(snapshot.profiles[0].installed[0].id, "nexus:42");
        assert!(manager.summary(&steam).is_none());
        let file = root.join("Mods/nexus-42/manifest.json");
        assert_eq!(fs::read_to_string(&file).unwrap(), manifest);
        manager.uninstall_nexus(&steam, "nexus:42").unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn installs_and_removes_modde_layouts_without_replacing_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        let mut manager = Manager::new(&workspace.join("data"), &workspace.join("cache"));
        manager.leases = workspace.join("leases");
        let fixtures = [
            (
                "1091500",
                "cyberpunk2077",
                "Archive",
                "wrapper/archive/pc/mod/test.archive",
                "Archive/pc/mod/test.archive",
            ),
            (
                "489830",
                "skyrimspecialedition",
                "Data",
                "wrapper/Data/test.esp",
                "Data/test.esp",
            ),
            (
                "292030",
                "witcher3",
                "Mods",
                "wrapper/mods/modFixture/data.bundle",
                "Mods/modFixture/data.bundle",
            ),
        ];
        for (name, domain, existing_folder, packed, installed) in fixtures {
            let root = workspace.join(name);
            fs::create_dir_all(root.join(existing_folder)).unwrap();
            let target = Target::resolve(&game("steam", name, &root)).unwrap();
            assert_eq!(target.domain(), domain);
            let game = game("steam", name, &root);
            let empty = manager.nexus_snapshot(&game, false).unwrap().unwrap();
            assert_eq!(empty.profiles[0].source, "Modde");
            let archive = workspace.join(format!("{name}.zip"));
            write_archive(&archive, &[(packed, "payload")]);
            let path = root.join(installed);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "existing").unwrap();
            assert!(
                target
                    .install(&manager, 1, &archive, "Fixture mod".into(), || false)
                    .is_err()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), "existing");
            fs::remove_file(&path).unwrap();
            target
                .install(&manager, 1, &archive, "Fixture mod".into(), || false)
                .unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), "payload");
            assert_eq!(target.profile(&manager).unwrap().installed[0].id, "nexus:1");
            manager.uninstall_nexus(&game, "nexus:1").unwrap();
            assert!(!path.exists());
            assert!(target.profile(&manager).unwrap().installed.is_empty());
        }
    }
}
