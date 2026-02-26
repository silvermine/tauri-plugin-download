use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::Error;
use crate::downloader;
use crate::models::*;
use crate::store::DownloadStore;
use crate::validate;

pub(crate) static DOWNLOAD_SUFFIX: &str = ".download";

/// Callback invoked whenever a download item changes state.
pub type OnChanged = Arc<dyn Fn(DownloadItem) + Send + Sync + 'static>;

/// Tauri-agnostic download manager, mirroring the iOS/Android `DownloadManager`.
#[derive(Clone)]
pub struct DownloadManager {
   pub(crate) store: DownloadStore,
   pub(crate) on_changed: OnChanged,
}

impl DownloadManager {
   /// Creates a new `DownloadManager`, loading persisted state from disk.
   ///
   /// # Arguments
   /// - `data_dir` - Directory where `downloads.json` will be stored.
   /// - `on_changed` - Callback invoked on every state/progress change.
   pub fn new(data_dir: PathBuf, on_changed: OnChanged) -> Self {
      let store = DownloadStore::new(data_dir.join("downloads.json"));
      if let Err(e) = store.load() {
         warn!("Failed to load download store: {}", e);
      }
      Self { store, on_changed }
   }

   ///
   /// Initializes the manager.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      let items = match self.store.list() {
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

         if let Err(e) = self.store.update(item.with_status(new_status.clone())) {
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
      self.store.list()
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
      validate::path(path)?;

      match self.store.find_by_path(path)? {
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
      validate::path(path)?;
      validate::url(url)?;

      // Check if item already exists
      if let Some(existing) = self.store.find_by_path(path)? {
         return Ok(DownloadActionResponse::with_expected_status(
            existing,
            DownloadStatus::Idle,
         ));
      }

      let item = self.store.create(DownloadItem {
         url: url.to_string(),
         path: path.to_string(),
         progress: 0.0,
         status: DownloadStatus::Idle,
      })?;

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
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be started when idle.
         DownloadStatus::Idle => {
            let original_item = item.clone();
            let item_started = item.with_status(DownloadStatus::InProgress);
            let manager = self.clone();
            let path = item.path.clone();
            tokio::spawn(async move {
               if let Err(e) = downloader::download(&manager, item_started).await {
                  error!(file = %filename(&path), "Download failed to start: {}", e);
                  if let Err(e) = manager.store.update(original_item.clone()) {
                     error!(file = %filename(&path), "Failed to update store on failure: {}", e);
                  }
                  manager.emit_changed(original_item);
               }
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
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be resumed when paused.
         DownloadStatus::Paused => {
            let original_item = item.clone();
            let item_resumed = item.with_status(DownloadStatus::InProgress);
            let manager = self.clone();
            let path = item.path.clone();
            tokio::spawn(async move {
               if let Err(e) = downloader::download(&manager, item_resumed).await {
                  error!(file = %filename(&path), "Download failed to resume: {}", e);
                  if let Err(e) = manager.store.update(original_item.clone()) {
                     error!(file = %filename(&path), "Failed to update store on failure: {}", e);
                  }
                  manager.emit_changed(original_item);
               }
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
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be paused when in progress.
         DownloadStatus::InProgress => {
            self
               .store
               .update(item.with_status(DownloadStatus::Paused))?;
            self.emit_changed(item.with_status(DownloadStatus::Paused));
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
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be cancelled when created, in progress or paused.
         DownloadStatus::Idle | DownloadStatus::InProgress | DownloadStatus::Paused => {
            self.store.delete(&item.path)?;
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               debug!(file = %filename(&item.path), "Temp file was not found or could not be deleted");
            }

            self.emit_changed(item.with_status(DownloadStatus::Cancelled));
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

   pub(crate) fn emit_changed(&self, item: DownloadItem) {
      debug!(file = %filename(&item.path), status = %item.status, progress = item.progress);
      (self.on_changed)(item);
   }
}

fn filename(path: &str) -> &str {
   Path::new(path)
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or(path)
}
