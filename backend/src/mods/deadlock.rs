use super::*;
use std::{collections::BTreeSet, io::Read};

pub(super) const DOMAIN: &str = "deadlock";
const GAMEINFO: &str = "game/citadel/gameinfo.gi";
const ADDONS: &str = "game/citadel/raesir_addons";
const SEARCH_PATH: &str = "citadel/raesir_addons";
const MANIFEST: &str = "deadlock-mods.json";

#[derive(Serialize, Deserialize)]
struct Entry {
    package: InstalledMod,
    files: Vec<PathBuf>,
}

pub(super) fn matches(game: &LibraryGame) -> bool {
    game.store == "steam" && game.id == "1422450"
}

pub(super) fn installed(path: &Path) -> Result<Vec<InstalledMod>, String> {
    Ok(entries(path)?
        .into_iter()
        .map(|entry| entry.package)
        .collect())
}

fn entries(path: &Path) -> Result<Vec<Entry>, String> {
    match fs::read(path.join(MANIFEST)) {
        Ok(data) => serde_json::from_slice(&data).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

impl Manager {
    pub(super) fn install_deadlock(
        &self,
        game: &LibraryGame,
        selected_file: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<Snapshot, String> {
        install::changes_allowed()?;
        ensure_installation(game)?;
        let (id, file) = selected_file
            .rsplit_once(':')
            .ok_or("Choose a GameBanana file first")?;
        let file = file.parse().map_err(|_| "Invalid GameBanana file ID")?;
        self.registry(DOMAIN)?;
        let selected = self.selected_id(DOMAIN);
        let (package, download) = self.deadlock_download(id, file, &cancelled)?;
        self.install_deadlock_archive(game, &selected, download.path(), package, cancelled)
    }

    pub fn import_deadlock_mod(
        &self,
        game: &LibraryGame,
        archive: &Path,
    ) -> Result<Snapshot, String> {
        install::changes_allowed()?;
        ensure_installation(game)?;
        self.registry(DOMAIN)?;
        let selected = self.selected_id(DOMAIN);
        let name = archive
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Local VPK");
        let hash =
            modde_core::installer::xxh64_file_hex(archive).map_err(|error| error.to_string())?;
        let package = InstalledMod {
            id: format!("deadlock-local:{hash}"),
            name: name.into(),
            owner: String::new(),
            version: String::new(),
            enabled: true,
            local: true,
            dependencies: Vec::new(),
            automatic: false,
        };
        self.install_deadlock_archive(game, &selected, archive, package, || false)
    }

    fn install_deadlock_archive(
        &self,
        game: &LibraryGame,
        selected: &str,
        source: &Path,
        package: InstalledMod,
        cancelled: impl Fn() -> bool,
    ) -> Result<Snapshot, String> {
        const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
        if !source
            .metadata()
            .is_ok_and(|data| data.is_file() && data.len() <= MAX_ARCHIVE)
        {
            return Err("Choose a VPK or archive of up to 1 GiB".into());
        }
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let extracted = temp.path().join("extracted");
        let mut signature = [0; 4];
        fs::File::open(source)
            .and_then(|mut file| file.read_exact(&mut signature))
            .map_err(|error| error.to_string())?;
        if u32::from_le_bytes(signature) == 0x55aa1234 {
            fs::create_dir(&extracted).map_err(|error| error.to_string())?;
            fs::copy(source, extracted.join("mod.vpk")).map_err(|error| error.to_string())?;
        } else {
            archive::extract(source, &extracted)?;
        }
        let files = vpk_group(&extracted)?;
        if cancelled() {
            return Err("Mod installation cancelled".into());
        }
        self.edit_deadlock_revision(game, selected, |path, manifest| {
            let id = package.id.as_str();
            if manifest.iter().any(|entry| entry.package.id == id) {
                return Err("This mod is already installed in the selected profile".into());
            }
            if manifest.len() >= 99 {
                return Err("This profile already has 99 Deadlock VPK packages".into());
            }
            let folder = PathBuf::from("packages").join(launch::hash(id.as_bytes()));
            fs::create_dir_all(path.join(&folder)).map_err(|error| error.to_string())?;
            let files = files
                .into_iter()
                .enumerate()
                .map(|(i, file)| {
                    let name = if i == 0 {
                        std::ffi::OsStr::new("dir.vpk")
                    } else {
                        file.file_name().ok_or("Invalid VPK filename")?
                    };
                    let relative = folder.join(name);
                    fs::copy(file, archive::destination(path, &relative)?)
                        .map_err(|error| error.to_string())?;
                    Ok(relative)
                })
                .collect::<Result<_, String>>()?;
            manifest.push(Entry { package, files });
            Ok(())
        })
    }

    pub(super) fn uninstall_deadlock(
        &self,
        game: &LibraryGame,
        id: &str,
    ) -> Result<Snapshot, String> {
        install::changes_allowed()?;
        self.registry(DOMAIN)?;
        self.edit_deadlock_revision(game, &self.selected_id(DOMAIN), |path, manifest| {
            let index = manifest
                .iter()
                .position(|entry| entry.package.id == id)
                .ok_or("This mod is missing from the selected profile")?;
            for path_to_remove in manifest.remove(index).files {
                fs::remove_file(archive::destination(path, &path_to_remove)?)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }

    fn edit_deadlock_revision(
        &self,
        game: &LibraryGame,
        selected: &str,
        edit: impl FnOnce(&Path, &mut Vec<Entry>) -> Result<(), String>,
    ) -> Result<Snapshot, String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _lease = self.profile_lease(DOMAIN)?;
        self.registry(DOMAIN)?;
        if self.selected_id(DOMAIN) != selected {
            return Err(
                "The selected mod profile changed. Try again in the intended profile".into(),
            );
        }
        let revisions = self.revisions_dir(game)?;
        fs::create_dir_all(&revisions).map_err(|error| error.to_string())?;
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(revisions)
            .map_err(|error| error.to_string())?;
        if let Some(source) = self.own_profile_path(DOMAIN, selected) {
            install::copy_profile(&source, pending.path())?;
        }
        let mut manifest = entries(pending.path())?;
        edit(pending.path(), &mut manifest)?;
        crate::storage::write_json(&pending.path().join(MANIFEST), &manifest)?;
        install::commit(&self.profile_root(DOMAIN, selected), pending)?;
        self.prune_revisions(DOMAIN);
        self.snapshot(game, false)
    }

    pub(super) fn stage_deadlock(
        &self,
        profile: &Path,
        game: &Path,
        receipt: &Path,
    ) -> Result<(), String> {
        let mirror = tempfile::tempdir().map_err(|error| error.to_string())?;
        let output = mirror.path().join(ADDONS);
        fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        let mut files = Vec::new();
        for (slot, entry) in entries(profile)?
            .into_iter()
            .filter(|e| e.package.enabled)
            .enumerate()
        {
            if slot >= 99 {
                return Err("Deadlock supports up to 99 VPK packages in this profile".into());
            }
            for relative in entry.files {
                let source = archive::destination(profile, &relative)?;
                let suffix = relative
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.rsplit_once('_').map(|(_, suffix)| suffix))
                    .filter(|suffix| {
                        suffix.len() == 3 && suffix.bytes().all(|b| b.is_ascii_digit())
                    })
                    .unwrap_or("dir");
                let name = format!("pak{:02}_{suffix}.vpk", slot + 1);
                let destination = archive::destination(game, &Path::new(ADDONS).join(&name))?;
                if destination.symlink_metadata().is_ok() {
                    return Err(format!(
                        "An existing file occupies Raesir's Deadlock add-on slot {name}"
                    ));
                }
                let target = output.join(name);
                fs::copy(source, &target).map_err(|error| error.to_string())?;
                files.push(target);
            }
        }
        if files.is_empty() {
            return Err("This profile has no enabled Deadlock mods".into());
        }
        let config = archive::destination(game, Path::new(GAMEINFO))?;
        let original = fs::read_to_string(config).map_err(|error| error.to_string())?;
        let updated = mount(&original)?;
        let target = mirror.path().join(GAMEINFO);
        fs::write(&target, updated).map_err(|error| error.to_string())?;
        files.push(target);
        self.stage_mirror(profile, game, mirror.path(), &files, receipt)
    }
}

fn ensure_installation(game: &LibraryGame) -> Result<(), String> {
    if !matches(game) {
        return Err("This installer is for Deadlock".into());
    }
    let root = local_game_folder(game).ok_or("Install Deadlock on this system first")?;
    if !archive::destination(&root, Path::new(GAMEINFO))?.is_file() {
        return Err("Could not find Deadlock's gameinfo.gi".into());
    }
    Ok(())
}

fn search_paths(document: &crate::vdf::Value) -> Result<&[(String, crate::vdf::Value)], String> {
    document
        .get("GameInfo")
        .and_then(|value| value.get("FileSystem"))
        .and_then(|value| value.get("SearchPaths"))
        .and_then(|value| value.entries())
        .ok_or_else(|| "Deadlock's gameinfo.gi has no SearchPaths section".into())
}

fn mount(original: &str) -> Result<String, String> {
    let document = crate::vdf::parse(original)?;
    let paths = search_paths(&document)?;
    let has = |key: &str| paths.iter().any(|(name, _)| name.eq_ignore_ascii_case(key));
    let index = paths
        .iter()
        .position(|(key, value)| {
            key.eq_ignore_ascii_case("Game")
                && value
                    .text()
                    .is_some_and(|v| v.eq_ignore_ascii_case("citadel"))
        })
        .ok_or("Could not locate Deadlock's base content path")?;
    if paths
        .iter()
        .any(|(_, value)| value.text() == Some(SEARCH_PATH))
    {
        return Err("Raesir's Deadlock search path is already present in gameinfo.gi".into());
    }
    let default_base = paths
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Game"))
        .and_then(|(_, value)| value.text())
        .ok_or("Missing Deadlock game path")?;
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let mut fields = line
            .split("//")
            .next()
            .unwrap_or_default()
            .split_whitespace();
        let is_anchor = fields
            .next()
            .is_some_and(|v| v.trim_matches('"').eq_ignore_ascii_case("Game"))
            && fields
                .next()
                .is_some_and(|v| v.trim_matches('"').eq_ignore_ascii_case("citadel"))
            && fields.next().is_none();
        if is_anchor {
            let indent = &line[..line.len() - line.trim_start().len()];
            let mut added = format!("{indent}Game\t{SEARCH_PATH}{newline}");
            for key in ["Mod", "Write"] {
                if !has(key) {
                    added.push_str(&format!("{indent}{key}\t\"{default_base}\"{newline}"));
                }
            }
            let mut result = original.to_owned();
            result.insert_str(offset, &added);
            let check = crate::vdf::parse(&result)?;
            if search_paths(&check)?
                .get(index)
                .is_some_and(|(key, value)| key == "Game" && value.text() == Some(SEARCH_PATH))
            {
                return Ok(result);
            }
        }
        offset += line.len();
    }
    Err("Could not safely edit Deadlock's SearchPaths".into())
}

fn vpk_group(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        for result in fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = result.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if kind.is_dir() {
                visit(&path, files)?;
            } else if kind.is_file() && ext == "vpk" {
                files.push(path);
            } else if !kind.is_file()
                || !matches!(
                    ext.as_str(),
                    "txt" | "md" | "png" | "jpg" | "jpeg" | "webp" | "url" | "ds_store"
                )
            {
                return Err(
                    "This archive contains extra files that need manual installation".into(),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, &mut files)?;
    let mut directories = files.iter().filter(|path| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with("_dir"))
    });
    let primary = directories
        .next()
        .or_else(|| files.first())
        .ok_or("This archive has no Deadlock VPK")?;
    if directories.next().is_some() {
        return Err(
            "This archive contains multiple VPK variants. Import one package at a time".into(),
        );
    }
    let stem = primary
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid VPK filename")?;
    let base = stem.strip_suffix("_dir").unwrap_or(stem);
    let expected = vpk_archives(primary)?;
    let mut found = BTreeSet::new();
    for path in files.iter().filter(|&path| path != primary) {
        let file = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid VPK filename")?;
        let (prefix, index) = file
            .rsplit_once('_')
            .ok_or("This archive contains multiple VPK variants")?;
        let index = index
            .parse::<u16>()
            .ok()
            .filter(|value| file.ends_with(&format!("_{value:03}")))
            .ok_or("Invalid VPK archive number")?;
        if prefix != base || path.parent() != primary.parent() || !found.insert(index) {
            return Err(
                "This archive contains multiple VPK variants. Import one package at a time".into(),
            );
        }
    }
    if expected != found {
        return Err(
            "The Deadlock VPK is missing companion archives or includes an unexpected variant"
                .into(),
        );
    }
    let position = files.iter().position(|file| file == primary).unwrap();
    files.swap(0, position);
    Ok(files)
}

fn vpk_archives(path: &Path) -> Result<BTreeSet<u16>, String> {
    let invalid = "Invalid Source 2 VPK directory";
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut head = [0_u8; 28];
    file.read_exact(&mut head).map_err(|_| invalid)?;
    let number = |offset| u32::from_le_bytes(head[offset..offset + 4].try_into().unwrap());
    let length = number(8) as usize;
    if number(0) != 0x55aa1234 || number(4) != 2 || !(1..=16 * 1024 * 1024).contains(&length) {
        return Err(invalid.into());
    }
    let mut tree = vec![0; length];
    file.read_exact(&mut tree).map_err(|_| invalid)?;
    let mut tree = tree.as_slice();
    let string = |input: &mut &[u8]| {
        let end = input.iter().position(|&byte| byte == 0).ok_or(invalid)?;
        *input = &input[end + 1..];
        Ok::<_, &str>(end != 0)
    };
    let mut archives = BTreeSet::new();
    let mut entries = 0;
    while string(&mut tree)? {
        while string(&mut tree)? {
            while string(&mut tree)? {
                let item = tree.get(..18).ok_or(invalid)?;
                let preload = u16::from_le_bytes([item[4], item[5]]) as usize;
                let index = u16::from_le_bytes([item[6], item[7]]);
                if item[16..18] != [0xff, 0xff] || (index > 999 && index != 0x7fff) {
                    return Err(invalid.into());
                }
                if index != 0x7fff {
                    archives.insert(index);
                }
                entries += 1;
                tree = tree.get(18 + preload..).ok_or(invalid)?;
            }
        }
    }
    if entries == 0 || !tree.is_empty() {
        return Err(invalid.into());
    }
    Ok(archives)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Installation, Os};

    const STOCK: &str = r#"GameInfo
{
    FileSystem
    {
        SearchPaths
        {
            // Keep priority for language-specific resources.
            Game_UILanguage citadel_*LANGUAGE*
            Game_LowViolence citadel_lv
            Game citadel
            Game core
        }
    }
}
"#;

    fn vpk(archive_index: u16) -> Vec<u8> {
        let mut tree = b"vmdl_c\0models\0test\0".to_vec();
        tree.extend(0_u32.to_le_bytes());
        tree.extend(0_u16.to_le_bytes());
        tree.extend(archive_index.to_le_bytes());
        tree.extend([0; 8]);
        tree.extend([0xff, 0xff, 0, 0, 0]);
        let mut data = 0x55aa1234_u32.to_le_bytes().to_vec();
        data.extend(2_u32.to_le_bytes());
        data.extend((tree.len() as u32).to_le_bytes());
        data.extend([0; 16]);
        data.extend(tree);
        data
    }

    #[test]
    fn profile_import_stage_and_restore_are_reversible() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Deadlock");
        fs::create_dir_all(root.join("game/citadel")).unwrap();
        let gameinfo = root.join(GAMEINFO);
        fs::write(&gameinfo, STOCK).unwrap();
        let source = temp.path().join("Model_007.vpk");
        fs::write(&source, vpk(0x7fff)).unwrap();
        let game = crate::catalog::entry(
            "steam",
            "1422450",
            "Deadlock",
            vec![Installation {
                library: String::new(),
                path: root.to_str().unwrap().to_owned(),
                os: Os::host(),
                state: "installed".into(),
                last_seen: 0,
            }],
            "test",
        );
        let manager = Manager::new(&temp.path().join("data"), &temp.path().join("cache"));

        let snapshot = manager.import_deadlock_mod(&game, &source).unwrap();
        assert_eq!(snapshot.profiles[0].installed.len(), 1);
        assert_eq!(fs::read_to_string(&gameinfo).unwrap(), STOCK);
        let id = snapshot.profiles[0].installed[0].id.clone();
        assert!(manager.import_deadlock_mod(&game, &source).is_err());

        let receipt = manager.stage_dir(&root);
        manager
            .stage_deadlock(&snapshot.profiles[0].path, &root, &receipt)
            .unwrap();
        let staged = root.join(ADDONS).join("pak01_dir.vpk");
        assert_eq!(fs::read(&staged).unwrap(), vpk(0x7fff));
        let config = fs::read_to_string(&gameinfo).unwrap();
        assert!(config.find("Game_LowViolence").unwrap() < config.find(SEARCH_PATH).unwrap());
        assert!(config.find(SEARCH_PATH).unwrap() < config.find("Game citadel").unwrap());
        let document = crate::vdf::parse(&config).unwrap();
        let paths = search_paths(&document).unwrap();
        for binding in ["Mod", "Write"] {
            assert!(
                paths
                    .iter()
                    .any(|(key, value)| key == binding && value.text() == Some("citadel"))
            );
        }

        // A newer profile can be published while the game uses the staged revision.
        let uninstalled = manager.uninstall(&game, &id).unwrap();
        assert!(uninstalled.profiles[0].installed.is_empty());
        assert!(staged.is_file());
        launch::steam_launch::restore_session(&receipt).unwrap();
        assert!(!staged.exists());
        assert_eq!(fs::read_to_string(gameinfo).unwrap(), STOCK);
    }

    #[test]
    fn numbered_vpks_require_their_directory_and_companions() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("pak52_dir.vpk");
        fs::write(&directory, vpk(1)).unwrap();
        assert!(vpk_group(temp.path()).is_err());
        let shard = temp.path().join("pak52_001.vpk");
        fs::write(&shard, b"contents").unwrap();
        assert_eq!(vpk_group(temp.path()).unwrap(), [directory, shard]);
        fs::write(temp.path().join("gameinfo.gi"), STOCK).unwrap();
        assert!(vpk_group(temp.path()).is_err());
    }
}
