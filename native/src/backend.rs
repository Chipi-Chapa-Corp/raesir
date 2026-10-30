use raesir_lib::{actions, artwork, client_control, epic_online, model::*, mount, storage};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Backend {
    pub data: PathBuf,
    pub cache: PathBuf,
    fixture: Option<PathBuf>,
    report: RwLock<Option<Arc<Report>>>,
    operation: Mutex<()>,
    cleanup: Mutex<()>,
    closing: AtomicBool,
    closed: AtomicBool,
    steam: client_control::Manager,
    epic: epic_online::Manager,
    mounts: mount::Manager,
}

impl Backend {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: std::env::var_os("RAESIR_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::data_dir()
                        .expect("user data directory")
                        .join("dev.raesir.launcher")
                }),
            cache: std::env::var_os("RAESIR_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::cache_dir()
                        .expect("user cache directory")
                        .join("dev.raesir.launcher")
                }),
            fixture: std::env::var_os("RAESIR_LIBRARY_FIXTURE").map(PathBuf::from),
            report: RwLock::new(None),
            operation: Mutex::new(()),
            cleanup: Mutex::new(()),
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            steam: Default::default(),
            epic: Default::default(),
            mounts: Default::default(),
        })
    }

    fn ready(&self) -> Result<(), String> {
        if self.closing.load(Ordering::SeqCst) {
            Err("Raesir is closing".into())
        } else {
            Ok(())
        }
    }

    fn set(&self, mut report: Report) -> Arc<Report> {
        let mut current = self.report.write().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = current.as_ref() {
            for game in &mut report.catalog {
                if game.title == game.id
                    && let Some(old) = previous
                        .catalog
                        .iter()
                        .find(|g| g.key == game.key && g.title != g.id)
                {
                    game.title = old.title.clone();
                }
            }
        }
        let report = Arc::new(report);
        *current = Some(report.clone());
        report
    }

    pub fn cached(&self) -> Result<Option<Arc<Report>>, String> {
        let report = if let Some(path) = &self.fixture {
            Some(
                serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            storage::cached(&self.data)
        };
        Ok(report.map(|r| self.set(r)))
    }

    pub fn scan(&self) -> Result<Arc<Report>, String> {
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.ready()?;
        if self.fixture.is_some() {
            return self.cached()?.ok_or("No fixture library".into());
        }
        Ok(self.set(storage::scan_and_save(&self.data, &ScanOptions::default())))
    }

    pub fn ownership(&self) -> Result<Option<Arc<Report>>, String> {
        if self.fixture.is_some() {
            return Ok(None);
        }
        self.ready()?;
        let status = self.steam.status();
        let root = status
            .managed_root
            .or_else(|| status.clients.first().map(|c| c.root.clone()));
        let Some(root) = root.filter(|_| status.supported) else {
            return Ok(None);
        };
        let owned = self.steam.read_library(&root)?;
        let _guard = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.ready()?;
        storage::write_json(&self.data.join("owned.json"), &owned)?;
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Library is still loading")?;
        Ok(Some(self.set(storage::apply_owned(
            &self.data,
            &owned,
            (*report).clone(),
        ))))
    }

    pub fn epic_profiles(&self) -> Vec<String> {
        if self.fixture.is_some() {
            return vec![];
        }
        self.report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|r| r.epic.legendary_available)
            .map(|r| {
                r.epic
                    .sources
                    .iter()
                    .filter(|s| s.local && s.kind == "legendary")
                    .map(|s| s.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn refresh_epic(&self, profile: &str) -> Result<Arc<Report>, String> {
        self.ready()?;
        let library = self.epic.refresh(profile)?;
        self.ready()?;
        epic_online::save(&self.data, library)?;
        self.scan()
    }

    pub fn mount(&self, id: &str) -> Result<Arc<Report>, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return Err("Mounting is disabled in the fixture profile".into());
        }
        if std::env::var_os("RAESIR_DISABLE_MOUNTS").is_some() {
            return Err("Mounting is disabled in this diagnostic profile".into());
        }
        self.mounts.mount(id)?;
        self.scan()
    }

    pub fn action(&self, key: &str, kind: &str) -> Result<String, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return Err("Client actions are disabled in the fixture profile".into());
        }
        let report = self
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Library is still loading")?;
        actions::perform(&report, key, kind)
    }

    pub fn artwork(&self, game: &LibraryGame) -> Result<artwork::Artwork, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return game
                .cover
                .as_ref()
                .filter(|p| Path::new(p).is_file())
                .map(|path| artwork::Artwork {
                    path: path.clone(),
                    title: None,
                })
                .ok_or("No fixture cover".into());
        }
        artwork::fetch(game, &self.cache.join("artwork"))
    }

    pub fn cached_details(&self, game: &LibraryGame) -> Option<raesir_lib::details::Details> {
        raesir_lib::details::cached(game, &self.cache.join("details"))
    }

    pub fn details(&self, game: &LibraryGame) -> Result<raesir_lib::details::Details, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return self
                .cached_details(game)
                .ok_or("No details in this diagnostic profile".into());
        }
        raesir_lib::details::fetch(game, &self.cache.join("details"))
    }

    pub fn detail_media(&self, url: &str) -> Result<String, String> {
        self.ready()?;
        if self.fixture.is_some() {
            return Path::new(url)
                .is_file()
                .then(|| url.to_owned())
                .ok_or("No local diagnostic image".into());
        }
        let mut game = raesir_lib::catalog::entry("media", url, "", vec![], "media");
        game.cover = Some(url.into());
        artwork::fetch(&game, &self.cache.join("artwork")).map(|art| art.path)
    }

    pub fn zoom(&self) -> f32 {
        std::fs::read(self.data.join("native-ui.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("zoom").and_then(|v| v.as_f64()))
            .filter(|v| v.is_finite())
            .map(|v| (v as f32).clamp(12., 22.))
            .unwrap_or(16.)
    }

    pub fn save_zoom(&self, zoom: f32) {
        let _ = storage::write_json(
            &self.data.join("native-ui.json"),
            &serde_json::json!({"zoom": zoom}),
        );
    }

    pub fn begin_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.steam.closing.store(true, Ordering::SeqCst);
        self.mounts.begin_closing();
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let _guard = self.cleanup.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.begin_closing();
        self.epic.shutdown();
        let steam = self.steam.stop();
        let mounts = self.mounts.shutdown();
        let errors: Vec<_> = [steam, mounts]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        if errors.is_empty() {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            self.closing.store(false, Ordering::SeqCst);
            self.steam.closing.store(false, Ordering::SeqCst);
            self.epic.resume();
            self.mounts.resume();
            Err(errors.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &Path) -> Backend {
        let mut backend = Arc::try_unwrap(Backend::new()).ok().unwrap();
        backend.data = dir.join("data");
        backend.cache = dir.join("cache");
        let path = dir.join("library.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "host_os": "linux", "games": [], "libraries": [], "volumes": [],
                "warnings": [], "owned_account": null, "catalog": []
            }))
            .unwrap(),
        )
        .unwrap();
        backend.fixture = Some(path);
        backend
    }

    #[test]
    fn fixture_refresh_is_read_only_and_cannot_launch_clients_or_mount() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        assert!(backend.cached().unwrap().is_some());
        assert!(backend.scan().unwrap().catalog.is_empty());
        assert!(backend.ownership().unwrap().is_none());
        assert!(backend.epic_profiles().is_empty());
        assert!(
            backend
                .action("steam:1", "launch")
                .unwrap_err()
                .contains("fixture")
        );
        assert!(backend.mount("fixture").unwrap_err().contains("fixture"));
        backend.shutdown().unwrap();
        backend.shutdown().unwrap();
        assert!(backend.scan().is_err());
        assert!(!backend.data.exists());
    }

    #[test]
    fn text_zoom_round_trips_and_invalid_settings_keep_a_readable_default() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        backend.save_zoom(19.);
        assert_eq!(backend.zoom(), 19.);
        backend.save_zoom(999.);
        assert_eq!(backend.zoom(), 22.);
        std::fs::write(backend.data.join("native-ui.json"), b"bad json").unwrap();
        assert_eq!(backend.zoom(), 16.);
    }

    #[test]
    fn existing_artwork_cache_with_img_extension_decodes_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fixture(dir.path());
        let path = dir.path().join("existing-cache.img");
        image_codec::RgbImage::from_pixel(8, 12, image_codec::Rgb([40, 80, 120]))
            .save_with_format(&path, image_codec::ImageFormat::Jpeg)
            .unwrap();
        let mut game = raesir_lib::catalog::entry("steam", "1", "Cached game", vec![], "fixture");
        game.cover = Some(path.to_string_lossy().into_owned());
        let (image, _, _) = crate::artwork::load(&backend, &game).unwrap();
        assert_eq!(image.frame_count(), 1);
        assert!(!image.as_bytes(0).unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(backend.cache.join("thumbnails"))
                .unwrap()
                .count(),
            1
        );
    }
}
