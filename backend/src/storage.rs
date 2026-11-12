use crate::{inventory, model::*, steam};
use serde::{Serialize, de::DeserializeOwned};
use std::{fs, path::Path};

pub const APP_ID: &str = "com.chipichapa.exposedgames";
const PREVIOUS_APP_IDS: [&str; 2] = ["dev.exposedgames.launcher", "dev.raesir.launcher"];

/// Keep existing installations on their original storage root: saved mod
/// revisions and recovery receipts can contain absolute paths into it.
pub fn application_dir(base: &Path) -> std::path::PathBuf {
    let current = base.join(APP_ID);
    if current.exists() {
        return current;
    }
    for id in PREVIOUS_APP_IDS {
        let previous = base.join(id);
        if previous.is_dir() {
            return previous;
        }
    }
    // Flatpak changes XDG roots along with the application ID. Its manifest
    // permits reading ~/.var/app, including both previous sandboxes.
    if let (Some(sandbox), Some(kind)) = (base.parent(), base.file_name())
        && sandbox.file_name().is_some_and(|name| name == APP_ID)
    {
        for id in PREVIOUS_APP_IDS {
            let previous = sandbox.with_file_name(id).join(kind).join(id);
            if previous.is_dir() {
                return previous;
            }
        }
    }
    current
}

fn load<T: DeserializeOwned>(path: &Path, warnings: &mut Vec<String>) -> Option<T> {
    if !path.exists() {
        return None;
    }
    match steam::read_text(path).and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())) {
        Ok(value) => Some(value),
        Err(e) => {
            warnings.push(format!(
                "Could not read cached inventory {}: {e}",
                path.display()
            ));
            None
        }
    }
}

pub fn cached(dir: &Path) -> Option<Report> {
    let mut report = load::<Report>(&dir.join("inventory.json"), &mut vec![])?;
    if report.catalog.is_empty() {
        report.catalog = crate::catalog::from_report(&report);
    }
    report.catalog.retain(crate::catalog::visible);
    crate::adapters::hydrate_game_pass(&mut report.catalog);
    crate::game_pass::load(dir, &mut report);
    crate::catalog::hydrate_os(&mut report);
    crate::catalog::hydrate_install_actions(&mut report);
    Some(report)
}

pub fn apply_owned(dir: &Path, owned: &OwnedLibrary, mut report: Report) -> Report {
    for game in &mut report.games {
        game.owned = owned.games.iter().any(|g| g.appid == game.appid);
        game.is_demo |= owned
            .games
            .iter()
            .any(|g| g.appid == game.appid && g.is_demo);
    }
    for game in &owned.games {
        if !report.games.iter().any(|g| g.appid == game.appid) {
            report.games.push(Game {
                appid: game.appid,
                title: game.name.clone(),
                owned: true,
                is_demo: game.is_demo,
                installations: vec![],
            });
        }
    }
    report.owned_account = Some(owned.steamid.clone());
    report.owned_source = owned.source.clone();
    for game in &mut report.catalog {
        if game.store == "steam" {
            game.in_library = owned.games.iter().any(|g| g.appid.to_string() == game.id);
            game.is_demo |= owned
                .games
                .iter()
                .any(|g| g.appid.to_string() == game.id && g.is_demo);
        }
    }
    for game in crate::catalog::from_report(&report)
        .into_iter()
        .filter(|g| g.store == "steam")
    {
        crate::catalog::merge(&mut report.catalog, game);
    }
    report
        .catalog
        .sort_by_cached_key(|g| (g.title.to_lowercase(), g.key.clone()));
    crate::catalog::hydrate_install_actions(&mut report);
    if let Err(error) = write_json(&dir.join("inventory.json"), &report) {
        report.warnings.push(error);
    }
    report
}

pub fn write_json(path: &Path, data: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or("Inventory path has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|e| format!("Could not replace {}: {e}", path.display()))?;
    Ok(())
}

pub fn scan_and_save(dir: &Path, options: &ScanOptions) -> Report {
    let mut warnings = Vec::new();
    let owned = load::<OwnedLibrary>(&dir.join("owned.json"), &mut warnings).unwrap_or_default();
    let previous = load::<Report>(&dir.join("inventory.json"), &mut warnings);
    let online = load::<Vec<EpicOnlineLibrary>>(&dir.join("epic-owned.json"), &mut warnings)
        .unwrap_or_default();
    let mut report = inventory::scan(options, &owned, &online, previous.as_ref());
    crate::game_pass::load(dir, &mut report);
    report.warnings.extend(warnings);
    if let Err(e) = write_json(&dir.join("inventory.json"), &report) {
        report.warnings.push(e);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_app_keeps_existing_storage_without_moving_saved_paths() {
        for id in PREVIOUS_APP_IDS {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join(APP_ID);
            let previous = root.path().join(id);
            assert_eq!(application_dir(root.path()), current);
            fs::create_dir(&previous).unwrap();
            fs::write(previous.join("ui.json"), b"saved settings").unwrap();
            assert_eq!(application_dir(root.path()), previous);
            assert_eq!(
                fs::read(previous.join("ui.json")).unwrap(),
                b"saved settings"
            );
            fs::create_dir(&current).unwrap();
            assert_eq!(application_dir(root.path()), current);
        }
    }

    #[test]
    fn renamed_flatpak_reads_storage_from_both_previous_sandboxes() {
        for id in PREVIOUS_APP_IDS {
            let root = tempfile::tempdir().unwrap();
            for kind in ["data", "cache"] {
                let base = root.path().join(APP_ID).join(kind);
                let previous = root.path().join(id).join(kind).join(id);
                fs::create_dir_all(&previous).unwrap();
                assert_eq!(application_dir(&base), previous);
                let current = base.join(APP_ID);
                fs::create_dir_all(&current).unwrap();
                assert_eq!(application_dir(&base), current);
            }
        }
    }

    #[test]
    fn most_recent_legacy_storage_takes_precedence() {
        let root = tempfile::tempdir().unwrap();
        for id in PREVIOUS_APP_IDS {
            fs::create_dir(root.path().join(id)).unwrap();
        }
        assert_eq!(
            application_dir(root.path()),
            root.path().join(PREVIOUS_APP_IDS[0])
        );
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join(APP_ID).join("data");
        for id in PREVIOUS_APP_IDS {
            fs::create_dir_all(root.path().join(id).join("data").join(id)).unwrap();
        }
        assert_eq!(
            application_dir(&base),
            root.path()
                .join(PREVIOUS_APP_IDS[0])
                .join("data")
                .join(PREVIOUS_APP_IDS[0])
        );
    }
}
