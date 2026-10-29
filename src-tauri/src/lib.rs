pub mod client_control;
pub mod epic;
pub mod epic_online;
pub mod inventory;
pub mod model;
pub mod mount;
pub mod owned;
pub mod platform;
pub mod steam;
pub mod steam_ipc;
pub mod storage;
mod vdf;

#[cfg(feature = "desktop")]
mod desktop {
    use crate::{client_control, epic_online, model::*, mount, storage};
    use std::sync::{Arc, atomic::Ordering};
    use tauri::{Emitter, Manager};

    fn data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
        if let Some(path) = std::env::var_os("RAESIR_DATA_DIR") {
            return Ok(path.into());
        }
        app.path().app_data_dir().map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn scan_library(app: tauri::AppHandle) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            storage::scan_and_save(&dir, &ScanOptions::default())
        })
        .await
        .map_err(|e| e.to_string())
    }

    #[tauri::command]
    fn steam_status(
        manager: tauri::State<'_, Arc<client_control::Manager>>,
    ) -> client_control::Status {
        manager.status()
    }

    #[tauri::command]
    async fn import_client_library(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<client_control::Manager>>,
        root: String,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let owned = manager.read_library(&root)?;
            storage::write_json(&dir.join("owned.json"), &owned)?;
            Ok(storage::scan_and_save(&dir, &ScanOptions::default()))
        })
        .await
        .map_err(|_| "Steam library lookup could not finish".to_string())?
    }

    #[tauri::command]
    async fn mount_volume(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<mount::Manager>>,
        id: String,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            manager.mount(&id)?;
            Ok(storage::scan_and_save(&dir, &ScanOptions::default()))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn refresh_epic(
        app: tauri::AppHandle,
        manager: tauri::State<'_, Arc<epic_online::Manager>>,
        profile: String,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let library = manager.refresh(&profile)?;
            epic_online::save(&dir, library)?;
            Ok(storage::scan_and_save(&dir, &ScanOptions::default()))
        })
        .await
        .map_err(|_| "Epic library refresh could not finish".to_string())?
    }

    pub fn run() {
        let manager = Arc::new(client_control::Manager::default());
        let epic_manager = Arc::new(epic_online::Manager::default());
        let mount_manager = Arc::new(mount::Manager::default());
        tauri::Builder::default()
            .manage(manager.clone())
            .manage(epic_manager.clone())
            .manage(mount_manager.clone())
            .invoke_handler(tauri::generate_handler![
                scan_library,
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
