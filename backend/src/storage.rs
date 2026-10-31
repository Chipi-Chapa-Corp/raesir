use crate::{inventory, model::*, steam};
use serde::{Serialize, de::DeserializeOwned};
use std::{fs, path::Path};

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
    crate::catalog::hydrate_os(&mut report);
    Some(report)
}

pub fn apply_owned(dir: &Path, owned: &OwnedLibrary, mut report: Report) -> Report {
    for game in &mut report.games {
        game.owned = owned.games.iter().any(|g| g.appid == game.appid);
    }
    for game in &owned.games {
        if !report.games.iter().any(|g| g.appid == game.appid) {
            report.games.push(Game {
                appid: game.appid,
                title: game.name.clone(),
                owned: true,
                installations: vec![],
            });
        }
    }
    report.owned_account = Some(owned.steamid.clone());
    report.owned_source = owned.source.clone();
    for game in &mut report.catalog {
        if game.store == "steam" {
            game.in_library = owned.games.iter().any(|g| g.appid.to_string() == game.id);
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
    report.warnings.extend(warnings);
    if let Err(e) = write_json(&dir.join("inventory.json"), &report) {
        report.warnings.push(e);
    }
    report
}
