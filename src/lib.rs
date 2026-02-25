use tauri::{
   Manager, RunEvent, Runtime,
   plugin::{Builder, TauriPlugin},
};

use tauri_plugin_store::StoreExt;

mod commands;
mod error;
mod models;

use error::Result;

#[cfg(desktop)]
use download_manager::{Download, init as desktop_init};

#[cfg(mobile)]
mod mobile;
#[cfg(mobile)]
use mobile::Download;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the download APIs.
pub trait DownloadExt<R: Runtime> {
   fn download(&self) -> &Download<R>;
}

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
      .setup(|app, api| {
         #[cfg(desktop)]
         let download = desktop_init(app, api)?;

         #[cfg(mobile)]
         let download = mobile::init(app, api)?;

         app.manage(download);
         if cfg!(desktop) {
            // Initialize the store plugin.
            // https://docs.rs/tauri/latest/tauri/struct.AppHandle.html#method.plugin
            let handle = app.app_handle().clone();
            std::thread::spawn(move || {
               handle
                  .plugin(tauri_plugin_store::Builder::new().build())
                  .unwrap();
               handle.store("downloads.json").unwrap();
            });
         }

         Ok(())
      })
      .on_event(|app_handle, event| {
         if let RunEvent::Ready = event {
            // Initialize the download plugin.
            app_handle.state::<Download<R>>().init();
         }
      })
      .build()
}
