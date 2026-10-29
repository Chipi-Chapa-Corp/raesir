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
