//! Mod catalogs and profiles, with Thunderstore and game-specific installers.
use crate::model::LibraryGame;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use thunderstore_engine::{
    client::ThunderstoreClient, ecosystem::Ecosystem, models::PackageIndex, profile::modlist,
};

mod dependencies;
mod details;
mod install;
mod launch;
mod noita;
mod profiles;
pub mod recipes;
mod steam;
mod updates;
pub use launch::LaunchTask;
pub use steam::helper as steam_helper;
pub use updates::{UpdateIssue, UpdateReport};

static ECOSYSTEM: LazyLock<Ecosystem> = LazyLock::new(Ecosystem::bundled);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mod worker runtime")
    });
    &RUNTIME
}

/// A deliberate debug-only fixture can launch a real game on isolated-gui.
/// Other fixture actions remain disabled, and Steam requires the private env file.
pub fn diagnostic_launch_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("ISOLATED_GUI_SESSION").is_some()
        && std::env::var_os("RAESIR_MODS_TEST_LAUNCH").is_some_and(|v| v == "1")
        && std::env::var_os("RAESIR_MODS_TEST_ENV").is_some()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub community: String,
    pub installed: usize,
    pub launchable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstalledMod {
    /// Provider-qualified ID; Thunderstore packages use Owner-Name.
    pub id: String,
    pub name: String,
    pub owner: String,
    pub version: String,
    pub enabled: bool,
    pub local: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub automatic: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: PathBuf,
    pub installed: Vec<InstalledMod>,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub description: String,
    pub version: String,
    pub page: String,
    pub icon: String,
    pub downloads: u64,
    pub deprecated: bool,
    pub categories: Vec<String>,
    #[serde(default = "thunderstore_source")]
    pub source: String,
}

fn thunderstore_source() -> String {
    "Thunderstore".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModFile {
    pub id: u64,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub community: String,
    pub selected: String,
    pub profiles: Vec<Profile>,
    pub packages: Arc<Vec<Package>>,
    #[serde(default)]
    pub nexus_url: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

struct Catalog {
    updated: Instant,
    index: Arc<PackageIndex>,
    packages: Arc<Vec<Package>>,
}

pub struct Manager {
    pub data: PathBuf,
    cache: PathBuf,
    catalogs: Mutex<HashMap<String, Catalog>>,
    operation: Arc<Mutex<()>>,
    active: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
    leases: PathBuf,
}

pub fn community(game: &LibraryGame) -> Option<&'static str> {
    if let Some(recipe) = recipes::for_game(game) {
        return Some(&recipe.id);
    }
    ECOSYSTEM
        .games()
        .find(|candidate| {
            candidate.distributions.iter().any(|distribution| {
                distribution.identifier == game.id
                    && match game.store.as_str() {
                        "steam" => distribution.platform == "steam",
                        "epic" => distribution.platform == "epic-games-store",
                        "xbox" => distribution.platform == "xbox-game-pass",
                        "ea" => distribution.platform == "origin",
                        _ => false,
                    }
            })
        })
        .map(|g| g.slug.as_str())
}

impl Manager {
    pub fn new(data: &Path, cache: &Path) -> Self {
        Self {
            data: std::env::var_os("RAESIR_MODS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("mods")),
            cache: cache.join("mods"),
            catalogs: Mutex::new(HashMap::new()),
            operation: Arc::new(Mutex::new(())),
            active: Default::default(),
            leases: crate::platform::home()
                .unwrap_or_else(std::env::temp_dir)
                .join(".cache/dev.raesir.launcher/mod-leases"),
        }
    }

    fn client(&self, community: &str) -> Result<ThunderstoreClient, String> {
        ThunderstoreClient::builder()
            .community(community)
            .cache_dir(self.cache.join(community))
            .download_concurrency(4)
            .download_timeout(Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())
    }

    fn catalog(
        &self,
        community: &str,
        refresh: bool,
    ) -> Result<(Arc<PackageIndex>, Arc<Vec<Package>>), String> {
        if !refresh
            && let Some(catalog) = self
                .catalogs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(community)
            && catalog.updated.elapsed() < Duration::from_secs(1800)
        {
            return Ok((catalog.index.clone(), catalog.packages.clone()));
        }
        if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
            return Err("Mod networking is disabled in this diagnostic profile".into());
        }
        let client = self.client(community)?;
        let manifest = if refresh {
            runtime().block_on(client.refresh_index())
        } else {
            runtime().block_on(client.get_manifest())
        };
        let index = Arc::new(manifest.map_err(|e| e.to_string())?);
        let mut packages = (0..index.len())
            .filter_map(|i| {
                let package = index.get_package_at(i)?;
                let version = package.latest_version()?;
                if version.is_active == Some(false) {
                    return None;
                }
                let id = package.full_name.as_ref()?;
                let (owner, _) = id.split_once('-')?;
                Some(Package {
                    id: id.clone(),
                    name: package.name.clone().unwrap_or_else(|| id.clone()),
                    owner: owner.into(),
                    description: version.description.clone().unwrap_or_default(),
                    version: version.version_number.clone().unwrap_or_default(),
                    page: format!(
                        "https://thunderstore.io/c/{community}/p/{}/",
                        id.replace('-', "/")
                    ),
                    icon: version.icon.clone().unwrap_or_default(),
                    downloads: package
                        .versions
                        .iter()
                        .map(|v| v.downloads.unwrap_or(0) as u64)
                        .sum(),
                    deprecated: package.is_deprecated.unwrap_or(false),
                    categories: package.categories.clone(),
                    source: thunderstore_source(),
                })
            })
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| {
            b.downloads
                .cmp(&a.downloads)
                .then_with(|| a.name.cmp(&b.name))
        });
        let packages = Arc::new(packages);
        self.catalogs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                community.into(),
                Catalog {
                    updated: Instant::now(),
                    index: index.clone(),
                    packages: packages.clone(),
                },
            );
        Ok((index, packages))
    }

    fn own_path(&self, community: &str) -> Option<PathBuf> {
        self.own_profile_path(community, &self.selected_id(community))
    }

    fn own_profile_path(&self, community: &str, profile: &str) -> Option<PathBuf> {
        let root = self.profile_root(community, profile);
        let pointer: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("current.json")).ok()?).ok()?;
        let revision = pointer["revision"].as_str()?;
        if revision.is_empty()
            || !revision
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return None;
        }
        let storage = pointer["storage"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        if storage != root
            && !crate::platform::local_sources().iter().any(|s| {
                let base = Path::new(&s.path).join("raesir/mods");
                let suffix = if profile == "raesir" {
                    PathBuf::from(community)
                } else {
                    PathBuf::from(community).join("profiles").join(profile)
                };
                if profile == "raesir" && base.join(community) == storage {
                    return true;
                }
                let Ok(relative) = storage.strip_prefix(&base) else {
                    return false;
                };
                let mut parts = relative.components();
                parts
                    .next()
                    .and_then(|p| p.as_os_str().to_str())
                    .is_some_and(|tag| {
                        tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    && parts.as_path() == suffix
            })
        {
            return None;
        }
        let path = storage.join("revisions").join(revision);
        path.canonicalize().ok().filter(|p| p.is_dir())
    }

    fn revisions_dir(&self, game: &LibraryGame) -> Result<PathBuf, String> {
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        self.revisions_dir_for(game, &self.selected_id(slug))
    }

    fn revisions_dir_for(&self, game: &LibraryGame, id: &str) -> Result<PathBuf, String> {
        let slug = community(game).ok_or("Unsupported mod community")?;
        // Flatpak Steam cannot read another application's private data directory.
        // Keep its profiles inside its own data root, without broadening sandbox
        // permissions or copying plugins into the game's installation.
        if game.store == "steam"
            && let Some(install) = game
                .installations
                .iter()
                .find(|i| i.os == crate::model::Os::host())
            && let Some(root) = crate::runtime::steam_client_root(Path::new(&install.library))
            && root
                .components()
                .any(|c| c.as_os_str() == "com.valvesoftware.Steam")
        {
            fs::create_dir_all(&self.data).map_err(|e| e.to_string())?;
            let namespace = launch::hash(
                self.data
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .as_bytes(),
            );
            let root = root.join("raesir/mods").join(namespace).join(slug);
            return Ok(if id == "raesir" {
                root
            } else {
                root.join("profiles").join(id)
            }
            .join("revisions"));
        }
        Ok(self.profile_root(slug, &id).join("revisions"))
    }

    pub fn summary(&self, game: &LibraryGame) -> Option<Summary> {
        let community = community(game)?;
        let profiles = self.profiles(community);
        let selected = self.selected(community, &profiles);
        let installed = selected.map_or(0, |p| p.installed.len());
        let local = game.installations.iter().any(|i| {
            i.os == crate::model::Os::host() && matches!(i.state.as_str(), "installed" | "external")
        });
        Some(Summary {
            community: community.into(),
            installed,
            launchable: local && selected.is_some_and(|p| p.installed.iter().any(|m| m.enabled)),
        })
    }

    pub fn snapshot(&self, game: &LibraryGame, refresh: bool) -> Result<Snapshot, String> {
        let community = community(game).ok_or("Thunderstore does not list this game")?;
        self.registry(community)?;
        let (packages, mut warnings) = if recipes::for_game(game).is_some() {
            self.noita_catalog(game, refresh)?
        } else {
            (self.catalog(community, refresh)?.1, Vec::new())
        };
        warnings.extend(self.update_warnings(game));
        let profiles = self.profiles(community);
        let selected = self
            .selected(community, &profiles)
            .map(|p| p.id.clone())
            .unwrap_or_else(|| "raesir".into());
        Ok(Snapshot {
            community: community.into(),
            profiles,
            selected,
            packages,
            nexus_url: recipes::for_game(game).map(|g| g.nexus.clone()),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_upstream_store_ids_without_guessing_from_titles() {
        let mut game = crate::catalog::entry("steam", "3527290", "Unknown name", vec![], "fixture");
        assert_eq!(community(&game), Some("peak"));
        game.store = "epic".into();
        assert!(community(&game).is_none());
        game.store = "steam".into();
        game.id = "0".into();
        game.title = "PEAK".into();
        assert!(community(&game).is_none());
    }
    #[test]
    fn revision_pointer_cannot_escape_the_managed_profile_root() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager {
            data: dir.path().join("mods"),
            cache: dir.path().join("cache"),
            catalogs: Mutex::new(HashMap::new()),
            operation: Arc::new(Mutex::new(())),
            active: Default::default(),
            leases: dir.path().join("leases"),
        };
        fs::create_dir_all(manager.data.join("peak")).unwrap();
        crate::storage::write_json(
            &manager.data.join("peak/current.json"),
            &serde_json::json!({"revision":"../../outside"}),
        )
        .unwrap();
        assert!(manager.own_path("peak").is_none());
    }
}
