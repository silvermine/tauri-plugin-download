use tauri::{
   Emitter, Manager, RunEvent, Runtime,
   plugin::{Builder, TauriPlugin},
};
use tracing::warn;

mod commands;
mod error;
mod models;

use error::Result;

#[cfg(desktop)]
use download_manager::DownloadManager;

#[cfg(mobile)]
mod mobile;
#[cfg(mobile)]
use mobile::Download;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the download APIs.
///
/// The trait is split by platform because the return type differs:
/// - Desktop uses the Tauri-agnostic `DownloadManager` (Rust implementation).
/// - Mobile delegates to the native plugin via a `PluginHandle`, so the return type
///   carries the `R: Runtime` generic required by Tauri's mobile plugin bridge.
#[cfg(desktop)]
pub trait DownloadExt<R: Runtime> {
   fn download(&self) -> &DownloadManager;
}

#[cfg(mobile)]
pub trait DownloadExt<R: Runtime> {
   fn download(&self) -> &Download<R>;
}

/// Blanket impl over any `T: Manager<R>` (i.e. `App`, `AppHandle`, `Window`) so callers
/// can use `app.download()` without explicitly referencing the managed state.
#[cfg(desktop)]
impl<R: Runtime, T: Manager<R>> crate::DownloadExt<R> for T {
   fn download(&self) -> &DownloadManager {
      self.state::<DownloadManager>().inner()
   }
}

#[cfg(mobile)]
impl<R: Runtime, T: Manager<R>> crate::DownloadExt<R> for T {
   fn download(&self) -> &Download<R> {
      self.state::<Download<R>>().inner()
   }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
   Builder::new("download")
      .invoke_handler(tauri::generate_handler![
         commands::create,
         commands::list,
         commands::get,
         commands::start,
         commands::cancel,
         commands::pause,
         commands::resume,
         commands::is_native,
      ])
      .setup(|app, _api| {
         #[cfg(desktop)]
         {
            // Resolve the app data directory for store persistence.
            let data_dir = app.path().app_data_dir().unwrap_or_else(|e| {
               warn!("Failed to resolve app data dir, falling back to '.': {}", e);
               std::path::PathBuf::from(".")
            });

            // Wire Tauri event emission as the on_changed callback.
            let app_handle = app.app_handle().clone();
            let manager = DownloadManager::new(
               data_dir,
               std::sync::Arc::new(move |item| {
                  if let Err(e) = app_handle.emit("tauri-plugin-download:changed", &item) {
                     warn!("Failed to emit change event: {}", e);
                  }
               }),
            );
            app.manage(manager);
         }

         #[cfg(mobile)]
         {
            // Mobile download management is handled natively by the platform plugin.
            let download = mobile::init(app, _api)?;
            app.manage(download);
         }

         Ok(())
      })
      .on_event(|app_handle, event| {
         if let RunEvent::Ready = event {
            // Initialize the download plugin.
            #[cfg(desktop)]
            app_handle.state::<DownloadManager>().init();
         }
      })
      .build()
}
