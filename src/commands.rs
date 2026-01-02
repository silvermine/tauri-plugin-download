use tauri::{AppHandle, Runtime, command};

use crate::DownloadExt;
use crate::Result;
use crate::models::*;

#[command]
pub(crate) async fn list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<DownloadItem>> {
   app.download().list(app.clone())
}

#[command]
pub(crate) async fn get<R: Runtime>(app: AppHandle<R>, path: String) -> Result<DownloadItem> {
   app.download().get(app.clone(), path)
}

#[command]
pub(crate) async fn create<R: Runtime>(
   app: AppHandle<R>,
   path: String,
   url: String,
) -> Result<DownloadActionResponse> {
   app.download().create(app.clone(), path, url)
}

#[command]
pub(crate) async fn start<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().start(app.clone(), path)
}

#[command]
pub(crate) async fn resume<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().resume(app.clone(), path)
}

#[command]
pub(crate) async fn pause<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().pause(app.clone(), path)
}

#[command]
pub(crate) async fn cancel<R: Runtime>(
   app: AppHandle<R>,
   path: String,
) -> Result<DownloadActionResponse> {
   app.download().cancel(app.clone(), path)
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn is_native<R: Runtime>(_app: AppHandle<R>) -> Result<bool> {
   #[cfg(target_os = "ios")]
   {
      Ok(true)
   }
   #[cfg(any(desktop, target_os = "android"))]
   {
      Ok(false)
   }
}
