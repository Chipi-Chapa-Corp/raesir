pub mod client_control;
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
    use crate::{client_control, model::*, mount, owned, storage};
    use std::sync::{Arc, atomic::Ordering};
    use tauri::{Emitter, Manager};

    fn data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
        if let Some(path) = std::env::var_os("RAESIR_DATA_DIR") {
            return Ok(path.into());
        }
        app.path().app_data_dir().map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn scan_steam(app: tauri::AppHandle, options: ScanOptions) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        tauri::async_runtime::spawn_blocking(move || storage::scan_and_save(&dir, &options))
            .await
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    async fn import_owned(
        app: tauri::AppHandle,
        steamid: String,
        api_key: String,
        options: ScanOptions,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            let owned = owned::fetch(&steamid, &api_key)?;
            storage::write_json(&dir.join("owned.json"), &owned)?;
            Ok(storage::scan_and_save(&dir, &options))
        })
        .await
        .map_err(|_| "Steam library lookup could not finish".to_string())?
    }

    #[tauri::command]
    async fn forget_owned(app: tauri::AppHandle, options: ScanOptions) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            let path = dir.join("owned.json");
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            Ok(storage::scan_and_save(&dir, &options))
        })
        .await
        .map_err(|e| e.to_string())?
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
        options: ScanOptions,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            let owned = manager.read_library(&root)?;
            storage::write_json(&dir.join("owned.json"), &owned)?;
            Ok(storage::scan_and_save(&dir, &options))
        })
        .await
        .map_err(|_| "Steam library lookup could not finish".to_string())?
    }

    #[tauri::command]
    async fn stop_steam(
        manager: tauri::State<'_, Arc<client_control::Manager>>,
    ) -> Result<(), String> {
        let manager = manager.inner().clone();
        tauri::async_runtime::spawn_blocking(move || manager.stop())
            .await
            .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn mount_volume(
        app: tauri::AppHandle,
        id: String,
        options: ScanOptions,
    ) -> Result<Report, String> {
        let dir = data_dir(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            mount::mount_read_only(&id)?;
            Ok(storage::scan_and_save(&dir, &options))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub fn run() {
        let manager = Arc::new(client_control::Manager::default());
        tauri::Builder::default()
            .manage(manager.clone())
            .invoke_handler(tauri::generate_handler![
                scan_steam,
                import_owned,
                forget_owned,
                steam_status,
                import_client_library,
                stop_steam,
                mount_volume
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
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = app.emit("steam-shutting-down", ());
                        match manager.stop() {
                            Ok(()) => {
                                manager.cleaned.store(true, Ordering::SeqCst);
                                app.exit(0);
                            }
                            Err(error) => {
                                manager.closing.store(false, Ordering::SeqCst);
                                let _ = app.emit("steam-shutdown-error", error);
                            }
                        }
                    });
                }
            });
    }
}

#[cfg(feature = "desktop")]
pub use desktop::run;
