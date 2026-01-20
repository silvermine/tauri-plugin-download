use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;
use tauri::AppHandle;
use tauri::{Emitter, Runtime, plugin::PluginApi};
use tracing::{debug, error, info, warn};

use crate::Error;
use crate::downloader;
use crate::models::*;
use crate::store;

pub(crate) static DOWNLOAD_SUFFIX: &str = ".download";

pub fn init<R: Runtime, C: DeserializeOwned>(
   app: &AppHandle<R>,
   _api: PluginApi<R, C>,
) -> crate::Result<Download<R>> {
   Ok(Download(app.clone()))
}

/// Access to the download APIs.
pub struct Download<R: Runtime>(pub(crate) AppHandle<R>);

impl<R: Runtime> Download<R> {
   ///
   /// Initializes the API.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      let items = match store::list(&self.0) {
         Ok(list) => list,
         Err(e) => {
            error!("Failed to load download store: {}", e);
            return;
         }
      };

      for item in items
         .into_iter()
         .filter(|item| item.status == DownloadStatus::InProgress)
      {
         let new_status = if item.progress == 0.0 {
            DownloadStatus::Idle
         } else {
            DownloadStatus::Paused
         };

         if let Err(e) = store::update(&self.0, item.with_status(new_status.clone())) {
            warn!(file = %filename(&item.path), "Failed to update download status: {}", e);
            continue;
         }

         info!(file = %filename(&item.path), status = %new_status, "Found download item");
      }
   }

   ///
   /// Lists all download operations.
   ///
   /// # Returns
   /// The list of download operations.
   pub fn list(&self) -> crate::Result<Vec<DownloadItem>> {
      store::list(&self.0)
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
   pub fn get(&self, path: &str) -> crate::Result<DownloadItem> {
      match store::get(&self.0, path)? {
         Some(item) => Ok(item),
         None => Ok(DownloadItem {
            url: String::new(),
            path: path.to_string(),
            progress: 0.0,
            status: DownloadStatus::Pending,
         }),
      }
   }

   ///
   /// Creates a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   /// - `url` - The download URL for the resource.
   ///
   /// # Returns
   /// The download operation.
   pub fn create(&self, path: &str, url: &str) -> crate::Result<DownloadActionResponse> {
      // Check if item already exists
      if let Some(existing) = store::get(&self.0, path)? {
         return Ok(DownloadActionResponse::with_expected_status(
            existing,
            DownloadStatus::Idle,
         ));
      }

      let item = store::create(
         &self.0,
         DownloadItem {
            url: url.to_string(),
            path: path.to_string(),
            progress: 0.0,
            status: DownloadStatus::Idle,
         },
      )?;

      Ok(DownloadActionResponse::new(item))
   }

   ///
   /// Starts a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn start(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&self.0, path)?.ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be started when idle.
         DownloadStatus::Idle => {
            let item_started = item.with_status(DownloadStatus::InProgress);
            let app = self.0.clone();
            tokio::spawn(async move {
               downloader::download(&app, item_started).await.unwrap();
            });

            Ok(DownloadActionResponse::new(
               item.with_status(DownloadStatus::InProgress),
            ))
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::InProgress,
         )),
      }
   }

   ///
   /// Resumes a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn resume(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&self.0, path)?.ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be resumed when paused.
         DownloadStatus::Paused => {
            let item_resumed = item.with_status(DownloadStatus::InProgress);
            let app = self.0.clone();
            tokio::spawn(async move {
               downloader::download(&app, item_resumed).await.unwrap();
            });

            Ok(DownloadActionResponse::new(
               item.with_status(DownloadStatus::InProgress),
            ))
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::InProgress,
         )),
      }
   }

   ///
   /// Pauses a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn pause(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&self.0, path)?.ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be paused when in progress.
         DownloadStatus::InProgress => {
            store::update(&self.0, item.with_status(DownloadStatus::Paused)).unwrap();
            Self::emit_changed(&self.0, item.with_status(DownloadStatus::Paused));
            Ok(DownloadActionResponse::new(
               item.with_status(DownloadStatus::Paused),
            ))
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::Paused,
         )),
      }
   }

   ///
   /// Cancels a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn cancel(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&self.0, path)?.ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be cancelled when created, in progress or paused.
         DownloadStatus::Idle | DownloadStatus::InProgress | DownloadStatus::Paused => {
            store::delete(&self.0, &item.path).unwrap();
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               debug!(file = %filename(&item.path), "Temp file was not found or could not be deleted");
            }

            Self::emit_changed(&self.0, item.with_status(DownloadStatus::Cancelled));
            Ok(DownloadActionResponse::new(
               item.with_status(DownloadStatus::Cancelled),
            ))
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::Cancelled,
         )),
      }
   }

   pub(crate) fn emit_changed(app: &AppHandle<R>, item: DownloadItem) {
      app.emit("tauri-plugin-download:changed", &item).unwrap();
      debug!(file = %filename(&item.path), status = %item.status, progress = item.progress);
   }
}

fn filename(path: &str) -> &str {
   Path::new(path)
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or(path)
}
