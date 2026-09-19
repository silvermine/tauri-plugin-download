use tauri::{AppHandle, Runtime, command};

use crate::DownloadExt;
use crate::Result;
use crate::models::*;

#[command]
pub(crate) async fn list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<DownloadItem>> {
   app.download().list()
}

#[command]
pub(crate) async fn get<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<Option<DownloadItem>> {
   app.download().get(&path)
}

#[command]
pub(crate) async fn create<R: Runtime>(
   app: AppHandle<R>,
   path: String,
   url: String,
   options: Option<CreateOptionsArgs>,
) -> Result<DownloadActionResponse> {
   let options = options.map(CreateOptions::from).unwrap_or_default();

   #[cfg(desktop)]
   {
      app.download().create_with_options(&path, &url, options)
   }
   #[cfg(mobile)]
   {
      app.download().create(&path, &url, options)
   }
}

#[command]
pub(crate) async fn start<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   #[cfg(desktop)]
   {
      app.download().start(&path).await
   }
   #[cfg(mobile)]
   {
      app.download().start(&path)
   }
}

#[command]
pub(crate) async fn resume<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   #[cfg(desktop)]
   {
      app.download().resume(&path).await
   }
   #[cfg(mobile)]
   {
      app.download().resume(&path)
   }
}

#[command]
pub(crate) async fn pause<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().pause(&path)
}

#[command]
pub(crate) async fn cancel<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().cancel(&path)
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn is_native<R: Runtime>(_app: AppHandle<R>) -> Result<bool> {
   #[cfg(mobile)]
   {
      Ok(true)
   }
   #[cfg(desktop)]
   {
      Ok(false)
   }
}
