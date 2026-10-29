pub mod actions;
pub mod adapters;
pub mod artwork;
pub mod catalog;
pub mod client_control;
pub mod discovery;
pub mod ea;
pub mod epic;
pub mod epic_online;
pub mod inventory;
pub mod model;
pub mod mount;
pub mod owned;
pub mod platform;
pub mod runtime;
pub mod steam;
pub mod steam_ipc;
pub mod storage;
mod vdf;

#[cfg(feature = "desktop")]
mod desktop {
    use crate::{actions, artwork, client_control, epic_online, model::*, mount, storage};
    use std::sync::{Arc, Mutex, RwLock, atomic::Ordering};
    use tauri::{Emitter, Manager};

    struct LibraryState {
        report: RwLock<Option<Report>>,
        refresh: Mutex<()>,
        artwork: Arc<tokio::sync::Semaphore>,
    }
    impl Default for LibraryState {
        fn default() -> Self {
            Self {
                report: RwLock::new(None),
                refresh: Mutex::new(()),
                artwork: Arc::new(tokio::sync::Semaphore::new(4)),
            }
        }
    }
    impl LibraryState {
        fn set(&self, mut report: Report) -> Report {
            let mut current = self.report.write().unwrap_or_else(|e| e.into_inner());
            if let Some(old) = &*current {
                for game in &mut report.catalog {
                    if game.title == game.id
                        && let Some(title) = old
                            .catalog
                            .iter()
                            .find(|g| g.key == game.key && g.title != g.id)
                            .map(|g| &g.title)
                    {
                        game.title = title.clone();
                    }
                }
            }
            *current = Some(report.clone());
            report
        }
        fn title(&self, key: &str, title: Option<&str>) {
            if let Some(title) = title
                && let Some(report) = self
                    .report
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                && let Some(game) = report
                    .catalog
                    .iter_mut()
                    .find(|g| g.key == key && g.title == g.id)
            {
                game.title = title.into();
            }
        }
    }

    fn data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
        if let Some(path) = std::env::var_os("RAESIR_DATA_DIR") {
            return Ok(path.into());
        }
        app.path().app_data_dir().map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn scan_library(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<LibraryState>>,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = state.refresh.lock().unwrap_or_else(|e| e.into_inner());
            state.set(storage::scan_and_save(&dir, &ScanOptions::default()))
        })
        .await
        .map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn load_library(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<LibraryState>>,
    ) -> Result<Option<Report>, String> {
        let dir = data_dir(&app)?;
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let report = storage::cached(&dir);
            if let Some(report) = &report {
                state.set(report.clone());
            }
            report
        })
        .await
        .map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn game_action(
        state: tauri::State<'_, Arc<LibraryState>>,
        key: String,
        kind: String,
    ) -> Result<String, String> {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let report = state.report.read().unwrap_or_else(|e| e.into_inner());
            actions::perform(
                report.as_ref().ok_or("Library is still loading")?,
                &key,
                &kind,
            )
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn open_support(project: String) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(move || actions::support(&project))
            .await
            .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn game_artwork(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<LibraryState>>,
        key: String,
    ) -> Result<artwork::Artwork, String> {
        let game = state
            .report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|r| r.catalog.iter().find(|g| g.key == key))
            .cloned()
            .ok_or("Game is no longer in the library")?;
        let root = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("artwork");
        if let Some(art) = artwork::cached(&game, &root) {
            state.title(&key, art.title.as_deref());
            return Ok(art);
        }
        let permit = state
            .artwork
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| e.to_string())?;
        let result = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            artwork::fetch(&game, &root)
        })
        .await
        .map_err(|e| e.to_string())??;
        state.title(&key, result.title.as_deref());
        Ok(result)
    }

    #[tauri::command]
    async fn steam_status(
        manager: tauri::State<'_, Arc<client_control::Manager>>,
    ) -> Result<client_control::Status, String> {
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || manager.status())
            .await
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn import_client_library(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<client_control::Manager>>,
        root: String,
        state: tauri::State<'_, Arc<LibraryState>>,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let owned = manager.read_library(&root)?;
            storage::write_json(&dir.join("owned.json"), &owned)?;
            let _guard = state.refresh.lock().unwrap_or_else(|e| e.into_inner());
            let report = state
                .report
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .or_else(|| storage::cached(&dir))
                .unwrap_or_else(|| storage::scan_and_save(&dir, &ScanOptions::default()));
            Ok(state.set(storage::apply_owned(&dir, &owned, report)))
        })
        .await
        .map_err(|_| "Steam library lookup could not finish".to_string())?
    }

    #[tauri::command]
    async fn mount_volume(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<mount::Manager>>,
        id: String,
        state: tauri::State<'_, Arc<LibraryState>>,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            manager.mount(&id)?;
            let _guard = state.refresh.lock().unwrap_or_else(|e| e.into_inner());
            Ok(state.set(storage::scan_and_save(&dir, &ScanOptions::default())))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn refresh_epic(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<epic_online::Manager>>,
        profile: String,
        state: tauri::State<'_, Arc<LibraryState>>,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let library = manager.refresh(&profile)?;
            epic_online::save(&dir, library)?;
            let _guard = state.refresh.lock().unwrap_or_else(|e| e.into_inner());
            Ok(state.set(storage::scan_and_save(&dir, &ScanOptions::default())))
        })
        .await
        .map_err(|_| "Epic library refresh could not finish".to_string())?
    }

    pub fn run() {
        let manager = Arc::new(client_control::Manager::default());
        let epic_manager = Arc::new(epic_online::Manager::default());
        let mount_manager = Arc::new(mount::Manager::default());
        tauri::Builder::default()
            .manage(Arc::new(LibraryState::default()))
            .manage(manager.clone())
            .manage(epic_manager.clone())
            .manage(mount_manager.clone())
            .setup(|app| {
                if let Some(window) = app.get_webview_window("main")
                    && let Some(monitor) = window
                        .current_monitor()
                        .ok()
                        .flatten()
                        .or_else(|| window.primary_monitor().ok().flatten())
                {
                    let area = monitor.work_area();
                    let _ = window.set_position(area.position);
                    let _ = window.set_size(area.size);
                }
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                scan_library,
                load_library,
                game_action,
                game_artwork,
                open_support,
                steam_status,
                import_client_library,
                mount_volume,
                refresh_epic
            ])
            .on_window_event(|window, event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    window.app_handle().exit(0);
                }
            })
            .build(tauri::generate_context!())
            .expect("error while building Raesir")
            .run(move |app, event| {
                if let tauri::RunEvent::ExitRequested { api, .. } = event {
                    if manager.cleaned.load(Ordering::SeqCst) {
                        return;
                    }
                    api.prevent_exit();
                    if manager.closing.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let manager = manager.clone();
                    let epic_manager = epic_manager.clone();
                    let mount_manager = mount_manager.clone();
                    mount_manager.begin_closing();
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = app.emit("app-shutting-down", ());
                        epic_manager.shutdown();
                        let steam = manager.stop();
                        let mounts = mount_manager.shutdown();
                        match steam.and(mounts) {
                            Ok(()) => {
                                manager.cleaned.store(true, Ordering::SeqCst);
                                app.exit(0);
                            }
                            Err(error) => {
                                epic_manager.resume();
                                mount_manager.resume();
                                manager.closing.store(false, Ordering::SeqCst);
                                let _ = app.emit("app-shutdown-error", error);
                            }
                        }
                    });
                }
            });
    }
}

#[cfg(feature = "desktop")]
pub use desktop::run;
