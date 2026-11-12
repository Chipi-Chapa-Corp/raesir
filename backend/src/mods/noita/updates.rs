use super::*;
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

pub(in crate::mods) struct NoitaUpdatePlan {
    changes: Vec<(String, Option<u64>)>,
    pub(in crate::mods) manual: Vec<(String, String)>,
}

impl NoitaUpdatePlan {
    pub(in crate::mods) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(in crate::mods) fn apply(
        self,
        manager: &Manager,
        game: &LibraryGame,
        pending: &Path,
        cancel: &AtomicBool,
    ) -> Result<(Vec<String>, Vec<(String, String)>), String> {
        let definition = recipes::for_game(game).ok_or("Unknown Noita adapter")?;
        let mut entries = read(pending)?;
        let mut changed = Vec::new();
        for (id, file) in self.changes {
            if cancel.load(Ordering::Relaxed) {
                return Err("Deferred: Application is closing".into());
            }
            let download = if let Some(recipe) = id.strip_prefix("recipe:") {
                manager.download_recipe(
                    definition
                        .mods
                        .iter()
                        .find(|r| r.id == recipe)
                        .ok_or("Recipe no longer exists")?,
                )?
            } else {
                manager.download_modworkshop(
                    definition,
                    id.strip_prefix("modworkshop:")
                        .ok_or("Unknown mod source")?,
                    file,
                )?
            };
            apply_download(pending, &mut entries, download)?;
            changed.push(id);
        }
        crate::storage::write_json(&pending.join(STATE), &entries)?;
        Ok((changed, self.manual))
    }
}

fn file_version(value: &Value) -> String {
    value["version"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("File {}", value["id"]))
}

fn identity(value: &Value) -> Result<SourceFile, String> {
    Ok(SourceFile {
        id: value["id"].as_u64().ok_or("Missing ModWorkshop file ID")?,
        name: value["name"].as_str().unwrap_or("").into(),
        kind: value["type"].as_str().unwrap_or("").into(),
        label: value["label"].as_str().unwrap_or("").into(),
        mod_type: value["mod_type"].as_str().unwrap_or("").into(),
        revision: value["updated_at"].as_str().unwrap_or("").into(),
        url: value["download_url"]
            .as_str()
            .ok_or("Missing ModWorkshop file URL")?
            .into(),
    })
}

fn select_file<'a>(entry: &Entry, files: &'a [Value]) -> Result<&'a Value, String> {
    let eligible = files
        .iter()
        .filter(|f| f["version_type"].as_str().is_none_or(|v| v == "release"))
        .collect::<Vec<_>>();
    let Some(old) = &entry.source_file else {
        return match eligible.as_slice() {
            [file] => Ok(*file),
            _ => Err(
                "Choose this mod's file once to enable automatic updates for its variant".into(),
            ),
        };
    };
    let mut candidates = eligible
        .into_iter()
        .filter(|f| {
            f["name"].as_str().unwrap_or("") == old.name
                && f["type"].as_str().unwrap_or("") == old.kind
                && f["label"].as_str().unwrap_or("") == old.label
                && f["mod_type"].as_str().unwrap_or("") == old.mod_type
        })
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    candidates.sort_by_key(|f| {
        f["updated_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp_millis())
    });
    if candidates.len() > 1 {
        let last = candidates[candidates.len() - 1];
        let previous = candidates[candidates.len() - 2];
        if last["updated_at"].as_str().is_some() && last["updated_at"] != previous["updated_at"] {
            return Ok(last);
        }
    }
    Err("The selected ModWorkshop variant changed or is ambiguous; choose its file again".into())
}

fn not_older(installed: &str, candidate: &str) -> bool {
    fn number(s: &str) -> Option<(u64, u64, u64)> {
        let mut parts = s.trim_start_matches('v').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.split('-').next()?.parse().ok()?;
        Some((major, minor, patch))
    }
    match (number(installed), number(candidate)) {
        (Some(old), Some(new)) if old != new => new > old,
        (Some(_), Some(_)) => {
            // Fork releases can keep the multiplayer protocol version while
            // publishing fixes. A stale /latest response must not undo them.
            let fork_revision = |version: &str| {
                version
                    .split_once("-exposed-games.")
                    .or_else(|| version.split_once("-raesir."))
                    .and_then(|(_, revision)| revision.parse::<u64>().ok())
            };
            match (fork_revision(installed), fork_revision(candidate)) {
                (Some(old), Some(new)) => new >= old,
                (Some(_), None) => false,
                _ => true,
            }
        }
        _ => true,
    }
}

impl Manager {
    pub(in crate::mods) fn noita_update_plan(
        &self,
        game: &LibraryGame,
        source: &Path,
        refreshed: &mut HashSet<String>,
    ) -> Result<NoitaUpdatePlan, String> {
        let definition = recipes::for_game(game).ok_or("Unknown Noita adapter")?;
        let mut plan = NoitaUpdatePlan {
            changes: Vec::new(),
            manual: Vec::new(),
        };
        for entry in read(source)? {
            let id = &entry.package.id;
            if let Some(recipe) = id.strip_prefix("recipe:") {
                let Some(recipe) = definition.mods.iter().find(|r| r.id == recipe) else {
                    plan.manual
                        .push((id.clone(), "This recipe is no longer in the catalog".into()));
                    continue;
                };
                let url = format!(
                    "https://api.github.com/repos/{}/releases/latest",
                    recipe.repository
                );
                let release = if !refreshed.contains(&url) {
                    let value = self.refresh_provider_json(&url)?;
                    refreshed.insert(url.clone());
                    value
                } else {
                    self.provider_json(&url, false)?
                };
                if release["draft"].as_bool() == Some(true)
                    || release["prerelease"].as_bool() == Some(true)
                {
                    continue;
                }
                let version = release["tag_name"]
                    .as_str()
                    .ok_or("Release has no version")?;
                let source_changed =
                    entry.package.page != format!("https://github.com/{}", recipe.repository);
                if source_changed
                    || (version != entry.package.version
                        && not_older(&entry.package.version, version))
                {
                    plan.changes.push((id.clone(), None));
                }
            } else if let Some(mod_id) = id.strip_prefix("modworkshop:") {
                let url = format!("https://api.modworkshop.net/mods/{mod_id}");
                let value = if !refreshed.contains(&url) {
                    let value = self.refresh_provider_json(&url)?;
                    refreshed.insert(url.clone());
                    value
                } else {
                    self.provider_json(&url, false)?
                };
                if value["game"]["short_name"].as_str() != Some(&definition.modworkshop) {
                    return Err("ModWorkshop mod no longer belongs to this game".into());
                }
                if value["disable_mod_managers"].as_bool() == Some(true) {
                    plan.manual.push((
                        id.clone(),
                        "The author requests browser installation".into(),
                    ));
                    continue;
                }
                let url = format!("{url}/files");
                let value = if !refreshed.contains(&url) {
                    let value = self.refresh_provider_json(&url)?;
                    refreshed.insert(url.clone());
                    value
                } else {
                    self.provider_json(&url, false)?
                };
                let files = value["data"]
                    .as_array()
                    .ok_or("Invalid ModWorkshop file list")?;
                let chosen = match select_file(&entry, files) {
                    Ok(file) => file,
                    Err(reason) => {
                        plan.manual.push((id.clone(), reason));
                        continue;
                    }
                };
                let new = identity(chosen)?;
                let candidate = file_version(chosen);
                let newer_time = entry.source_file.as_ref().is_none_or(|old| {
                    match (
                        chrono::DateTime::parse_from_rfc3339(&old.revision),
                        chrono::DateTime::parse_from_rfc3339(&new.revision),
                    ) {
                        (Ok(old), Ok(new)) => new >= old,
                        _ => true,
                    }
                });
                if newer_time
                    && not_older(&entry.package.version, &candidate)
                    && (entry.source_file.as_ref() != Some(&new)
                        || candidate != entry.package.version)
                {
                    plan.changes.push((id.clone(), Some(new.id)));
                }
            } else {
                plan.manual.push((id.clone(),if id.starts_with("nexus:") {"Nexus imports require a browser download; import the newer archive to update"} else {"This mod source does not provide automatic updates"}.into()));
            }
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests;
