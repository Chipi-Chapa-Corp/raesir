//! Noita installation adapter shared by ModWorkshop, Nexus imports and recipes.
use super::*;
use serde_json::Value;
use std::io::{Read, Write};

mod archive;
mod providers;
mod session;
#[cfg(target_os = "linux")]
mod steam_companion;
pub(crate) use session::{disable_for_vanilla, restore_session};
#[cfg(test)]
mod tests;

const STATE: &str = "noita-mods.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    package: Package,
    folders: Vec<String>,
    unsafe_mod: bool,
    companion: Option<InstalledTool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstalledTool {
    executable: String,
    arguments: Vec<String>,
}

fn read(path: &Path) -> Result<Vec<Entry>, String> {
    match fs::read(path.join(STATE)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("Could not read Noita profile: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn installed(path: &Path) -> Result<Vec<InstalledMod>, String> {
    Ok(read(path)?
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

fn allowed() -> Result<(), String> {
    if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_some()
        && std::env::var_os("RAESIR_MODS_TEST_INSTALL").is_none()
    {
        return Err("Mod changes are disabled in this diagnostic profile".into());
    }
    Ok(())
}

impl Manager {
    fn noita_pending(&self, game: &LibraryGame) -> Result<tempfile::TempDir, String> {
        let slug = community(game).ok_or("Unsupported game")?;
        self.registry(slug)?;
        let revisions = self.revisions_dir(game)?;
        fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .map_err(|e| e.to_string())?;
        if let Some(source) = self.own_path(slug) {
            read(&source)?;
            super::install::copy_profile(&source, pending.path())?;
        }
        Ok(pending)
    }

    fn noita_commit(
        &self,
        game: &LibraryGame,
        pending: tempfile::TempDir,
        entries: &[Entry],
    ) -> Result<Snapshot, String> {
        let slug = community(game).ok_or("Unsupported game")?;
        crate::storage::write_json(&pending.path().join(STATE), &entries)?;
        super::install::commit(&self.profile_root(slug, &self.selected_id(slug)), pending)?;
        self.prune_revisions(slug);
        self.snapshot(game, false)
    }

    pub(super) fn noita_install(&self, game: &LibraryGame, id: &str) -> Result<Snapshot, String> {
        allowed()?;
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let _lease = self.modification_lease(game)?;
        let definition = recipes::for_game(game).ok_or("Unsupported game")?;
        let (package, path, folder, companion) = if let Some(id) = id.strip_prefix("recipe:") {
            let recipe = definition
                .mods
                .iter()
                .find(|r| r.id == id)
                .ok_or("Unknown bundled recipe")?;
            self.download_recipe(recipe)?
        } else if let Some(id) = id.strip_prefix("modworkshop:") {
            let (id, file) = match id.split_once(':') {
                Some((id, file)) => (
                    id,
                    Some(
                        file.parse::<u64>()
                            .map_err(|_| "Invalid ModWorkshop file identifier")?,
                    ),
                ),
                None => (id, None),
            };
            self.download_modworkshop(definition, id, file)?
        } else {
            return Err("Use the Nexus import flow for browser downloads".into());
        };
        let pending = self.noita_pending(game)?;
        let mut entries = read(pending.path())?;
        let mut entry = unpack_mod(&path, pending.path(), package, &folder, &entries)?;
        if let Some((archive, executable, arguments)) = companion {
            let tool = pending
                .path()
                .join("tools")
                .join(&entry.package.id.replace(':', "-"));
            if tool.exists() {
                fs::remove_dir_all(&tool).map_err(|e| e.to_string())?;
            }
            archive::extract(&archive, &tool)?;
            let exe = tool.join(&executable);
            if !exe.is_file() {
                return Err("The companion archive is missing its declared executable".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))
                    .map_err(|e| e.to_string())?;
            }
            entry.companion = Some(InstalledTool {
                executable: exe
                    .strip_prefix(pending.path())
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into(),
                arguments,
            });
        }
        entries.retain(|e| e.package.id != entry.package.id);
        entries.push(entry);
        self.noita_commit(game, pending, &entries)
    }

    pub(super) fn noita_uninstall(&self, game: &LibraryGame, id: &str) -> Result<Snapshot, String> {
        allowed()?;
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let _lease = self.modification_lease(game)?;
        let pending = self.noita_pending(game)?;
        let mut entries = read(pending.path())?;
        let entry = entries
            .iter()
            .find(|e| e.package.id == id)
            .ok_or("This mod is not installed")?;
        for folder in &entry.folders {
            if !recipes::safe_name(folder) {
                return Err("Invalid mod folder in profile".into());
            }
            fs::remove_dir_all(pending.path().join("mods").join(folder))
                .map_err(|e| e.to_string())?;
        }
        if let Some(tool) = &entry.companion {
            let path = archive::destination(pending.path(), Path::new(&tool.executable))?;
            let parent = path.parent().ok_or("Invalid companion path")?;
            if !parent.starts_with(pending.path().join("tools")) {
                return Err("Invalid companion folder".into());
            }
            fs::remove_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entries.retain(|e| e.package.id != id);
        self.noita_commit(game, pending, &entries)
    }

    /// Only Nexus imports are exposed; arbitrary GitHub URLs/local mods are not.
    pub fn import_nexus(
        &self,
        game: &LibraryGame,
        url: &str,
        archive: &Path,
        folder: &str,
    ) -> Result<Snapshot, String> {
        allowed()?;
        let definition =
            recipes::for_game(game).ok_or("Nexus imports are not supported for this game")?;
        let id = nexus_id(url, definition)?;
        if !archive.is_file() {
            return Err("Choose the archive downloaded from Nexus".into());
        }
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let _lease = self.modification_lease(game)?;
        let pending = self.noita_pending(game)?;
        let mut entries = read(pending.path())?;
        let package = Package {
            id: format!("nexus:{id}"),
            name: format!("Nexus mod {id}"),
            owner: String::new(),
            description: "Imported from Nexus Mods".into(),
            version: "Imported".into(),
            page: url.into(),
            icon: String::new(),
            downloads: 0,
            deprecated: false,
            categories: Vec::new(),
            source: "Nexus Mods".into(),
        };
        let entry = unpack_mod(archive, pending.path(), package, folder, &entries)?;
        entries.retain(|e| e.package.id != entry.package.id);
        entries.push(entry);
        self.noita_commit(game, pending, &entries)
    }
}

fn nexus_id(url: &str, definition: &recipes::Game) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| "Enter a Nexus mod page URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("www.nexusmods.com" | "nexusmods.com")
        )
    {
        return Err("Enter a Nexus Mods HTTPS page URL".into());
    }
    let parts = parsed
        .path_segments()
        .ok_or("Invalid Nexus page")?
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let wanted = reqwest::Url::parse(&definition.nexus).map_err(|e| e.to_string())?;
    let game = wanted
        .path_segments()
        .and_then(|mut s| s.next())
        .ok_or("Invalid Nexus game")?;
    let parts = if parts.first() == Some(&"games") {
        &parts[1..]
    } else {
        &parts[..]
    };
    if parts.len() < 3
        || parts[0] != game
        || parts[1] != "mods"
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
        || parts[2].is_empty()
    {
        return Err(format!("Choose a {game} mod page on Nexus"));
    }
    Ok(parts[2].into())
}

fn unpack_mod(
    path: &Path,
    pending: &Path,
    mut package: Package,
    flat_folder: &str,
    entries: &[Entry],
) -> Result<Entry, String> {
    let extracted = tempfile::tempdir().map_err(|e| e.to_string())?;
    archive::extract(path, extracted.path())?;
    let mut roots = Vec::new();
    fn find(path: &Path, depth: usize, roots: &mut Vec<PathBuf>) -> Result<(), String> {
        if path.join("mod.xml").is_file() {
            roots.push(path.into());
            return Ok(());
        }
        if depth == 0 {
            return Ok(());
        }
        for child in fs::read_dir(path).map_err(|e| e.to_string())? {
            let child = child.map_err(|e| e.to_string())?;
            if child.file_type().map_err(|e| e.to_string())?.is_dir() {
                find(&child.path(), depth - 1, roots)?;
            }
        }
        Ok(())
    }
    find(extracted.path(), 4, &mut roots)?;
    if roots.is_empty() {
        return Err("This archive does not contain a Noita mod.xml".into());
    }
    let mut folders = Vec::new();
    let mut unsafe_mod = false;
    for root in roots {
        let folder = if root == extracted.path() {
            flat_folder.to_owned()
        } else {
            root.file_name()
                .and_then(|n| n.to_str())
                .ok_or("Invalid mod folder")?
                .to_owned()
        };
        if !recipes::safe_name(&folder) {
            return Err(
                "For a flat archive, enter the mod folder name specified by its author".into(),
            );
        }
        if folders.contains(&folder)
            || entries
                .iter()
                .any(|e| e.package.id != package.id && e.folders.contains(&folder))
        {
            return Err(format!(
                "Another installed mod already owns the folder {folder}"
            ));
        }
        let xml = fs::read_to_string(root.join("mod.xml")).map_err(|e| e.to_string())?;
        let document =
            roxmltree::Document::parse(&xml).map_err(|e| format!("Invalid mod.xml: {e}"))?;
        let manifest = document.root_element();
        if manifest.tag_name().name() != "Mod" {
            return Err("Expected a Noita Mod manifest".into());
        }
        unsafe_mod |= manifest.attribute("request_no_api_restrictions") == Some("1");
        if package.source == "Nexus Mods"
            && let Some(name) = manifest.attribute("name")
        {
            package.name = name.into();
        }
        let target = pending.join("mods").join(&folder);
        if target.exists() {
            fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        super::install::copy_profile(&root, &target)?;
        folders.push(folder);
    }
    if let Some(old) = entries.iter().find(|e| e.package.id == package.id) {
        for folder in &old.folders {
            if !folders.contains(folder) && recipes::safe_name(folder) {
                let _ = fs::remove_dir_all(pending.join("mods").join(folder));
            }
        }
    }
    Ok(Entry {
        package,
        folders,
        unsafe_mod,
        companion: None,
    })
}
