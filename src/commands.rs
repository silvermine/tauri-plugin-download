use tauri::{AppHandle, Manager, Runtime, command};

use crate::DownloadExt;
use crate::Result;
use crate::models::*;
use crate::scope::DownloadScope;

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
   app.state::<DownloadScope>().check(&path)?;

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
   app.state::<DownloadScope>().check(&path)?;

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
   app.state::<DownloadScope>().check(&path)?;

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

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn test_the_commands_reject_a_path_outside_the_download_dirs() {
      // The scope check is a single line in each of `create`, `start` and `resume`, and
      // nothing else reaches it: every other test calls `DownloadScope::check` itself or
      // reads the managed state. Deleting those lines leaves the rest of the suite
      // passing while `create` returns `Ok` for a path outside every configured
      // directory.
      let dir = tempfile::tempdir().unwrap();
      let downloads_dir = dir.path().join("downloads");

      // Siblings, since a download directory holding the store directory fails plugin
      // initialization. A real directory for the store too: an unwritable one would let
      // an unchecked `create` fail on the store rather than on the scope, and the
      // assertions below would pass for the wrong reason.
      let store = dir.path().join("store");
      let downloads = downloads_dir.clone();
      let app = tauri::test::mock_builder()
         .plugin(
            crate::Builder::new()
               .on_setup(move |_app, config| {
                  config.store_dir(store.clone());
                  config.download_dirs([downloads.clone()]);
                  Ok(())
               })
               .build(),
         )
         .build(tauri::test::mock_context(tauri::test::noop_assets()))
         .unwrap();

      let handle = app.handle().clone();

      // A sibling of the configured directory rather than something like `/etc/passwd`,
      // so the scope is the only thing that can reject it.
      let path = dir
         .path()
         .join("elsewhere/f.mp4")
         .to_str()
         .unwrap()
         .to_string();

      // All three run before anything is asserted, so a missing check in `start` or
      // `resume` is not hidden by `create` failing first.
      let created = tauri::async_runtime::block_on(create(
         handle.clone(),
         path.clone(),
         "https://example.com/f.mp4".to_string(),
         None,
      ));
      let started = tauri::async_runtime::block_on(start(handle.clone(), path.clone()));
      let resumed = tauri::async_runtime::block_on(resume(handle.clone(), path.clone()));

      // The exact message, which separates a scope rejection from any other failure.
      let message = "Path Error: path must be inside a download directory";

      for result in [created, started, resumed] {
         let error = result.unwrap_err();
         assert_eq!(error.to_string(), message);
         assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
               "code": "invalid input",
               "message": message,
               "retryability": "permanent"
            })
         );
      }

      // Pairs with the cases above: a check that rejected everything, or one inverted,
      // would satisfy them.
      let inside = downloads_dir.join("f.mp4").to_str().unwrap().to_string();
      let created = tauri::async_runtime::block_on(create(
         handle,
         inside,
         "https://example.com/f.mp4".to_string(),
         None,
      ));

      assert!(created.is_ok(), "in-scope create failed: {:?}", created);
   }
}
