use serde::de::DeserializeOwned;
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{AppHandle, Runtime};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_download);

pub fn init<R: Runtime, C: DeserializeOwned>(
   _app: &AppHandle<R>,
   _api: PluginApi<R, C>,
) -> crate::Result<Download<R>> {
   #[cfg(target_os = "ios")]
   let handle = _api.register_ios_plugin(init_plugin_download)?;
   Ok(Download(handle))
}

/// Access to the download APIs.
pub struct Download<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Download<R> {
   ///
   /// Initializes the API.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      // Not implemented on mobile platforms since initialization is handled by the plugin.
   }

   ///
   /// Lists all download operations.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   ///
   /// # Returns
   /// The list of download operations.
   pub fn list(&self, _app: AppHandle<R>) -> crate::Result<Vec<DownloadItem>> {
      self.0.run_mobile_plugin("list", ()).map_err(Into::into)
   }

   ///
   /// Gets a download operation.
   ///
   /// If the download exists in the store, returns it. If not found, returns a download
   /// in `Pending` state (not persisted to store). The caller can then call `create` to
   /// persist it and transition to `Idle` state.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn get(&self, _app: AppHandle<R>, path: String) -> crate::Result<DownloadItem> {
      self
         .0
         .run_mobile_plugin("get", PathArgs { path })
         .map_err(Into::into)
   }

   ///
   /// Creates a download operation.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   /// - `path` - The download path.
   /// - `url` - The download URL for the resource.
   ///
   /// # Returns
   /// The download operation.
   pub fn create(
      &self,
      _app: AppHandle<R>,
      path: String,
      url: String,
   ) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin("create", CreateArgs { path, url })
         .map_err(Into::into)
   }

   ///
   /// Starts a download operation.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn start(&self, _app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin("start", PathArgs { path })
         .map_err(Into::into)
   }

   ///
   /// Resumes a download operation.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn resume(&self, _app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin("resume", PathArgs { path })
         .map_err(Into::into)
   }

   ///
   /// Pauses a download operation.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn pause(&self, _app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin("pause", PathArgs { path })
         .map_err(Into::into)
   }

   ///
   /// Cancels a download operation.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn cancel(&self, _app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin("cancel", PathArgs { path })
         .map_err(Into::into)
   }
}
