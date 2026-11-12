//! Background updates publish whole profile revisions, with a final lease and
//! source check. Downloads never hold the launcher's mutation lock.
use super::*;
use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thunderstore_engine::profile;

const INTERVAL: u64 = 3600;
const RETRY: u64 = 900;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateReport {
    pub checked_at: u64,
    pub checked_profiles: usize,
    pub updated: Vec<UpdatedMod>,
    pub issues: Vec<UpdateIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdatedMod {
    pub game: String,
    pub profile: String,
    pub package: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateIssue {
    pub game: String,
    #[serde(default)]
    pub community: String,
    pub profile: String,
    pub package: Option<String>,
    pub kind: String,
    pub message: String,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    profiles: HashMap<String, Checked>,
    report: UpdateReport,
}

#[derive(Serialize, Deserialize)]
struct Checked {
    revision: PathBuf,
    next_check: u64,
}

struct Lease(fs::File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Metadata catches edits made by a player or another manager without a pointer
/// change. A concurrent edit during the copy or download defers publication.
fn tree_stamp(root: &Path) -> Result<Vec<(PathBuf, u64, SystemTime)>, String> {
    fn visit(
        root: &Path,
        path: &Path,
        out: &mut Vec<(PathBuf, u64, SystemTime)>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err("Mod profiles cannot contain symbolic links".into());
            }
            if kind.is_dir() {
                visit(root, &entry.path(), out)?;
            } else if kind.is_file() {
                let info = entry.metadata().map_err(|e| e.to_string())?;
                out.push((
                    entry.path().strip_prefix(root).unwrap().into(),
                    info.len(),
                    info.modified().map_err(|e| e.to_string())?,
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

impl Manager {
    fn update_games(&self, games: &[LibraryGame]) -> BTreeMap<String, LibraryGame> {
        let mut result = BTreeMap::new();
        for game in games {
            let Some(slug) = community(game) else {
                continue;
            };
            let local = game.installations.iter().any(|i| {
                i.os == crate::model::Os::host()
                    && matches!(i.state.as_str(), "installed" | "external")
            });
            if local || !result.contains_key(slug) {
                result.insert(slug.to_owned(), game.clone());
            }
        }
        // Profiles remain managed when a game is uninstalled or leaves a store
        // catalog. Reconstruct only known games from our own profile directory.
        for entry in fs::read_dir(&self.data).into_iter().flatten().flatten() {
            let slug = entry.file_name().to_string_lossy().into_owned();
            if result.contains_key(&slug) || !entry.file_type().is_ok_and(|k| k.is_dir()) {
                continue;
            }
            let identity = recipes::by_id(&slug)
                .map(|g| ("steam", g.steam.as_str()))
                .or_else(|| {
                    ECOSYSTEM.game(&slug)?.distributions.iter().find_map(|d| {
                        let store = match d.platform.as_str() {
                            "steam" => "steam",
                            "epic-games-store" => "epic",
                            "xbox-game-pass" => "xbox",
                            "origin" => "ea",
                            _ => return None,
                        };
                        Some((store, d.identifier.as_str()))
                    })
                });
            if let Some((store, id)) = identity {
                result.insert(
                    slug.clone(),
                    crate::catalog::entry(store, id, &slug, Vec::new(), "mod-profile"),
                );
            }
        }
        result
    }

    pub fn auto_update(
        &self,
        games: &[LibraryGame],
        startup: bool,
        cancel: &AtomicBool,
    ) -> Result<UpdateReport, String> {
        if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_some()
            && std::env::var_os("EXPOSED_GAMES_MODS_TEST_UPDATES").is_none()
        {
            return Err("Automatic mod updates are disabled in this diagnostic profile".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(UpdateReport::default());
        }
        fs::create_dir_all(&self.data).map_err(|e| e.to_string())?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.data.join(".updates.lock"))
            .map_err(|e| e.to_string())?;
        if file.try_lock().is_err() {
            return Ok(UpdateReport::default());
        }
        let _lease = Lease(file);
        let state_path = self.data.join("update-status.json");
        let mut state: State = match fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Could not read mod update state: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.to_string()),
        };
        let timestamp = now();
        let mut result = UpdateReport {
            checked_at: timestamp,
            ..Default::default()
        };
        let mut refreshed = HashSet::new();
        let mut valid_profiles = HashSet::new();
        for (slug, game) in self.update_games(games) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let registry = match self.registry(&slug) {
                Ok(r) => r,
                Err(error) => {
                    result.issues.push(UpdateIssue {
                        game: game.key.clone(),
                        community: slug.clone(),
                        profile: String::new(),
                        package: None,
                        kind: "failed".into(),
                        message: error,
                    });
                    continue;
                }
            };
            for profile in registry.profiles {
                valid_profiles.insert(format!("{slug}/{}", profile.id));
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(source) = self.own_profile_path(&slug, &profile.id) else {
                    continue;
                };
                let key = format!("{slug}/{}", profile.id);
                if !startup
                    && state
                        .profiles
                        .get(&key)
                        .is_some_and(|s| s.revision == source && s.next_check > timestamp)
                {
                    continue;
                }
                result.checked_profiles += 1;
                state.report.issues.retain(|i| {
                    i.profile != profile.id || (i.game != game.key && i.community != slug)
                });
                let updated =
                    self.update_profile(&game, &profile.id, &source, &mut refreshed, cancel);
                let (delay, issues) = match updated {
                    Ok((packages, manual)) => {
                        result
                            .updated
                            .extend(packages.into_iter().map(|package| UpdatedMod {
                                game: game.key.clone(),
                                profile: profile.id.clone(),
                                package,
                            }));
                        (
                            INTERVAL,
                            manual
                                .into_iter()
                                .map(|(package, message)| UpdateIssue {
                                    game: game.key.clone(),
                                    community: slug.clone(),
                                    profile: profile.id.clone(),
                                    package: Some(package),
                                    kind: "manual".into(),
                                    message,
                                })
                                .collect::<Vec<_>>(),
                        )
                    }
                    Err(error) => {
                        let deferred = error.starts_with("Deferred:");
                        (
                            if deferred { 60 } else { RETRY },
                            vec![UpdateIssue {
                                game: game.key.clone(),
                                community: slug.clone(),
                                profile: profile.id.clone(),
                                package: None,
                                kind: if deferred { "deferred" } else { "failed" }.into(),
                                message: error.trim_start_matches("Deferred: ").into(),
                            }],
                        )
                    }
                };
                result.issues.extend(issues.clone());
                state.report.issues.extend(issues);
                state.profiles.insert(
                    key,
                    Checked {
                        revision: if result
                            .updated
                            .iter()
                            .any(|u| u.game == game.key && u.profile == profile.id)
                        {
                            self.own_profile_path(&slug, &profile.id).unwrap_or(source)
                        } else {
                            source
                        },
                        next_check: timestamp + delay,
                    },
                );
            }
        }
        if !cancel.load(Ordering::Relaxed) {
            state.profiles.retain(|key, _| valid_profiles.contains(key));
            state.report.issues.retain(|issue| {
                issue.community.is_empty()
                    || valid_profiles.contains(&format!("{}/{}", issue.community, issue.profile))
            });
            state.report.checked_at = timestamp;
            state.report.checked_profiles = result.checked_profiles;
            state.report.updated = result.updated.clone();
            crate::storage::write_json(&state_path, &state)?;
        }
        Ok(result)
    }

    pub(super) fn update_warnings(&self, game: &LibraryGame) -> Vec<String> {
        fs::read(self.data.join("update-status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .map(|s| {
                s.report
                    .issues
                    .into_iter()
                    .filter(|i| {
                        i.kind != "deferred"
                            && (i.game == game.key
                                || community(game).is_some_and(|slug| slug == i.community))
                    })
                    .map(|i| format!("Mod updates: {}", i.message))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn update_profile(
        &self,
        game: &LibraryGame,
        id: &str,
        source: &Path,
        refreshed: &mut HashSet<String>,
        cancel: &AtomicBool,
    ) -> Result<(Vec<String>, Vec<(String, String)>), String> {
        let slug = community(game).ok_or("Unknown mod community")?;
        let _source_lease = {
            let _guard = self
                .operation
                .try_lock()
                .map_err(|_| "Deferred: Another mod operation is in progress")?;
            let _profile_lease = self
                .try_profile_lease(slug)
                .map_err(|e| format!("Deferred: {e}"))?;
            self.revision_lease(source)
                .map_err(|e| format!("Deferred: {e}"))?
        };
        let stamp = tree_stamp(source)?;
        let noita = if recipes::for_game(game).is_some() {
            Some(self.noita_update_plan(game, source, refreshed)?)
        } else {
            None
        };
        let thunder = if noita.is_none() {
            let before = modlist::read(source).map_err(|e| e.to_string())?;
            if !before.iter().any(|m| m.online_source) {
                return Ok((Vec::new(), Vec::new()));
            }
            let key = format!("thunderstore:{slug}");
            let (index, _) = self.catalog(slug, !refreshed.contains(&key))?;
            refreshed.insert(key);
            let desired = thunderstore_updates(&before, &index);
            if desired.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }
            Some((index, desired))
        } else {
            None
        };
        if noita.as_ref().is_some_and(|p| p.is_empty()) {
            return Ok((Vec::new(), noita.unwrap().manual));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Deferred: Application is closing".into());
        }
        let revisions = self.revisions_dir_for(game, id)?;
        fs::create_dir_all(&revisions).map_err(|e| e.to_string())?;
        let pending = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&revisions)
            .map_err(|e| e.to_string())?;
        install::copy_profile(source, pending.path())?;
        if tree_stamp(source)? != stamp {
            return Err("Deferred: Profile changed while preparing its update".into());
        }
        let (changed, manual) = if let Some(plan) = noita {
            plan.apply(self, game, pending.path(), cancel)?
        } else {
            let (index, desired) = thunder.unwrap();
            (
                self.update_thunderstore_pending(slug, pending.path(), &index, &desired)?,
                Vec::new(),
            )
        };
        if changed.is_empty() {
            return Ok((changed, manual));
        }
        let _guard = self
            .operation
            .try_lock()
            .map_err(|_| "Deferred: Another mod operation is in progress")?;
        let _profile_lease = self
            .try_profile_lease(slug)
            .map_err(|e| format!("Deferred: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Deferred: Application is closing".into());
        }
        if !self.registry(slug)?.profiles.iter().any(|p| p.id == id)
            || self.own_profile_path(slug, id).as_deref() != Some(source)
            || tree_stamp(source)? != stamp
        {
            return Err("Deferred: Profile changed while downloading updates".into());
        }
        install::commit(&self.profile_root(slug, id), pending)?;
        self.prune_profile_revisions(slug, id);
        Ok((changed, manual))
    }

    fn update_thunderstore_pending(
        &self,
        slug: &str,
        pending: &Path,
        index: &PackageIndex,
        desired: &[String],
    ) -> Result<Vec<String>, String> {
        let before = modlist::read(pending).map_err(|e| e.to_string())?;
        if !before.iter().any(|m| m.online_source) {
            return Ok(Vec::new());
        }
        let resolution = thunderstore_engine::models::DependencyGraph::new(desired.to_vec())
            .resolve_report(index);
        for local in before.iter().filter(|m| !m.online_source) {
            if resolution
                .urls
                .keys()
                .any(|file| file.starts_with(&format!("{}-", local.name)))
            {
                return Err(format!(
                    "Updating this profile would replace local mod {}; update that mod manually",
                    local.name
                ));
            }
        }
        let mut reasons = dependencies::Reasons::read(pending)?;
        let batch =
            profile::plan_install_batch(pending, &desired, &[]).map_err(|e| e.to_string())?;
        let result = runtime()
            .block_on(profile::install_batch(
                pending,
                &self.cache,
                &ECOSYSTEM,
                &index,
                &self.client(slug)?,
                slug,
                &batch,
                modlist::now_millis(),
            ))
            .map_err(|e| e.to_string())?;
        if !result.is_ok() {
            return Err(result
                .failed
                .into_iter()
                .map(|(id, e)| format!("{id}: {e}"))
                .collect::<Vec<_>>()
                .join("; "));
        }
        let after = modlist::read(pending).map_err(|e| e.to_string())?;
        reasons.record_update(&before, &after);
        reasons.write(pending)?;
        Ok(after
            .iter()
            .filter(|m| {
                before
                    .iter()
                    .find(|old| old.name == m.name)
                    .is_none_or(|old| old.version_number != m.version_number)
            })
            .map(|m| m.name.clone())
            .collect())
    }
}

fn thunderstore_updates(before: &[modlist::ProfileMod], index: &PackageIndex) -> Vec<String> {
    before
        .iter()
        .filter(|m| m.online_source)
        .filter_map(|m| {
            let package = index.get_package_by_full_name(&m.name)?;
            let latest = package.latest_version()?;
            if latest.is_active == Some(false) || package.is_deprecated == Some(true) {
                return None;
            }
            let version = modlist::SemVer::parse(latest.version_number.as_deref()?);
            ((version.major, version.minor, version.patch)
                > (
                    m.version_number.major,
                    m.version_number.minor,
                    m.version_number.patch,
                ))
                .then(|| m.name.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests;
