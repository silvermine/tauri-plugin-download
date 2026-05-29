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
         // Revert to a recoverable state so the download can be retried.
         match self.revert_in_progress(&item) {
            Ok(reverted) => {
               info!(file = %filename(&reverted.path), status = %reverted.status, "Reverted download item")
            }
            Err(e) => warn!(file = %filename(&item.path), "Failed to revert download item: {}", e),
         }
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
            transferred_bytes: 0,
            total_bytes: None,
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
         transferred_bytes: 0,
         total_bytes: None,
         status: DownloadStatus::Idle,
      })?;

      self.emit_changed(item.clone());
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
         DownloadStatus::Idle => self.spawn_download(item, "failed to start"),

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
         DownloadStatus::Paused => self.spawn_download(item, "failed to resume"),

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::InProgress,
         )),
      }
   }

   fn spawn_download(
      &self,
      item: DownloadItem,
      err_msg: &'static str,
   ) -> crate::Result<DownloadActionResponse> {
      let item_in_progress = item.with_status(DownloadStatus::InProgress);
      self.store.update(item_in_progress.clone())?;

      let manager = self.clone();
      let path = item.path.clone();
      let item_in_progress_response = item_in_progress.clone();
      tokio::spawn(async move {
         if let Err(e) = downloader::download(&manager, item_in_progress).await {
            error!(file = %filename(&path), "Download {}: {}", err_msg, e);

            // Revert unless already paused or canceled.
            if let Ok(Some(current)) = manager.store.find_by_path(&path)
               && current.status == DownloadStatus::InProgress
            {
               match manager.revert_in_progress(&current) {
                  Ok(reverted) => {
                     info!(file = %filename(&reverted.path), status = %reverted.status, "Reverted download item")
                  }
                  Err(e) => warn!(file = %filename(&path), "Failed to revert download item: {}", e),
               }
            }
         }
      });

      Ok(DownloadActionResponse::new(item_in_progress_response))
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
            let paused = item.with_status(DownloadStatus::Paused);
            self.store.update(paused.clone())?;
            self.emit_changed(paused.clone());
            Ok(DownloadActionResponse::new(paused))
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
         // Allow download to be canceled when created, in progress or paused.
         DownloadStatus::Idle | DownloadStatus::InProgress | DownloadStatus::Paused => {
            self.store.delete(&item.path)?;
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               debug!(file = %filename(&item.path), "Temp file was not found or could not be deleted");
            }

            self.emit_changed(item.with_status(DownloadStatus::Canceled));
            Ok(DownloadActionResponse::new(
               item.with_status(DownloadStatus::Canceled),
            ))
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item,
            DownloadStatus::Canceled,
         )),
      }
   }

   /// Reverts an `InProgress` download item to `Paused` or `Idle` based on
   /// whether a temp file exists on disk. No-op for other statuses.
   fn revert_in_progress(&self, item: &DownloadItem) -> crate::Result<DownloadItem> {
      if item.status != DownloadStatus::InProgress {
         return Ok(item.clone());
      }

      let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
      let reverted = if Path::new(&temp_path).exists() {
         item.with_status(DownloadStatus::Paused)
      } else {
         item.with_status(DownloadStatus::Idle)
      };

      self.store.update(reverted.clone())?;
      self.emit_changed(reverted.clone());
      Ok(reverted)
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

#[cfg(test)]
mod tests {
   use super::*;
   use std::sync::Mutex;
   use tempfile::TempDir;

   const VALID_URL: &str = "https://example.com/file.mp4";

   type EventLog = Arc<Mutex<Vec<DownloadItem>>>;

   fn make_manager() -> (DownloadManager, TempDir, EventLog) {
      let dir = TempDir::new().unwrap();
      let events: EventLog = Arc::new(Mutex::new(Vec::new()));
      let captured = events.clone();
      let on_changed: OnChanged = Arc::new(move |item| {
         captured.lock().unwrap().push(item);
      });
      let manager = DownloadManager::new(dir.path().to_path_buf(), on_changed);
      (manager, dir, events)
   }

   fn event_log(events: &EventLog) -> Vec<DownloadItem> {
      events.lock().unwrap().clone()
   }

   fn clear_events(events: &EventLog) {
      events.lock().unwrap().clear();
   }

   fn seed(manager: &DownloadManager, path: &str, status: DownloadStatus) {
      manager
         .store
         .create(DownloadItem {
            url: VALID_URL.to_string(),
            path: path.to_string(),
            progress: 0.0,
            transferred_bytes: 0,
            total_bytes: None,
            status,
         })
         .unwrap();
   }

   // ---------- get ----------

   #[test]
   fn test_get_returns_pending_for_unknown_path() {
      let (manager, _dir, _events) = make_manager();
      let item = manager.get("/tmp/unknown.mp4").unwrap();
      assert_eq!(item.path, "/tmp/unknown.mp4");
      assert_eq!(item.status, DownloadStatus::Pending);
      assert_eq!(item.url, "");
      assert_eq!(item.progress, 0.0);
   }

   #[test]
   fn test_get_returns_persisted_item() {
      let (manager, _dir, _events) = make_manager();
      manager.create("/tmp/file.mp4", VALID_URL).unwrap();
      let item = manager.get("/tmp/file.mp4").unwrap();
      assert_eq!(item.status, DownloadStatus::Idle);
      assert_eq!(item.url, VALID_URL);
   }

   #[test]
   fn test_get_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.get("").is_err());
   }

   // ---------- create ----------

   #[test]
   fn test_create_persists_idle_item_and_emits() {
      let (manager, _dir, events) = make_manager();
      let response = manager.create("/tmp/file.mp4", VALID_URL).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert!(response.is_expected_status);

      let stored = manager
         .store
         .find_by_path("/tmp/file.mp4")
         .unwrap()
         .unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
      assert_eq!(stored.url, VALID_URL);

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].path, "/tmp/file.mp4");
      assert_eq!(log[0].status, DownloadStatus::Idle);
   }

   #[test]
   fn test_create_existing_does_not_overwrite_url() {
      let (manager, _dir, events) = make_manager();
      manager.create("/tmp/file.mp4", VALID_URL).unwrap();

      let other_url = "https://example.com/other.mp4";
      let response = manager.create("/tmp/file.mp4", other_url).unwrap();
      assert_eq!(response.download.url, VALID_URL);

      let stored = manager
         .store
         .find_by_path("/tmp/file.mp4")
         .unwrap()
         .unwrap();
      assert_eq!(stored.url, VALID_URL);

      // Only the first create emitted a change event.
      assert_eq!(event_log(&events).len(), 1);
   }

   #[test]
   fn test_create_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.create("", VALID_URL).is_err());
   }

   #[test]
   fn test_create_rejects_invalid_url() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.create("/tmp/file.mp4", "not-a-url").is_err());
   }

   // ---------- start ----------

   #[test]
   fn test_start_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.start("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_start_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.start("").is_err());
   }

   #[test]
   fn test_start_from_non_idle_does_not_change_state() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.start(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::InProgress);
      assert_eq!(response.expected_status, DownloadStatus::InProgress);
      assert!(response.is_expected_status);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::InProgress);
   }

   // ---------- resume ----------

   #[test]
   fn test_resume_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.resume("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_resume_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.resume("").is_err());
   }

   #[test]
   fn test_resume_from_non_paused_does_not_change_state() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Idle);

      let response = manager.resume(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert_eq!(response.expected_status, DownloadStatus::InProgress);
      assert!(!response.is_expected_status);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
   }

   // ---------- pause ----------

   #[test]
   fn test_pause_from_in_progress_updates_and_emits() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.pause(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Paused);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].status, DownloadStatus::Paused);
   }

   #[test]
   fn test_pause_from_non_in_progress_is_no_op() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Idle);

      let response = manager.pause(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert_eq!(response.expected_status, DownloadStatus::Paused);
      assert!(!response.is_expected_status);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);

      assert!(event_log(&events).is_empty());
   }

   #[test]
   fn test_pause_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.pause("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_pause_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.pause("").is_err());
   }

   // ---------- cancel ----------

   #[test]
   fn test_cancel_idle_removes_and_emits_canceled() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      manager.create(path, VALID_URL).unwrap();
      clear_events(&events);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);

      assert!(manager.store.find_by_path(path).unwrap().is_none());

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].status, DownloadStatus::Canceled);
   }

   #[test]
   fn test_cancel_in_progress_removes_and_emits_canceled() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);
      assert!(manager.store.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_cancel_paused_removes_and_emits_canceled() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Paused);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);
      assert!(manager.store.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_cancel_removes_temp_file_when_present() {
      let (manager, dir, _events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();

      seed(&manager, &path, DownloadStatus::Paused);
      manager.cancel(&path).unwrap();

      assert!(!Path::new(&temp_path).exists());
   }

   #[test]
   fn test_cancel_handles_missing_temp_file_gracefully() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Idle);
      // No temp file written; cancel should still succeed.
      assert!(manager.cancel(path).is_ok());
   }

   #[test]
   fn test_cancel_from_terminal_status_does_not_remove() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Completed);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Completed);
      assert_eq!(response.expected_status, DownloadStatus::Canceled);
      assert!(!response.is_expected_status);
      assert!(manager.store.find_by_path(path).unwrap().is_some());
   }

   #[test]
   fn test_cancel_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.cancel("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_cancel_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.cancel("").is_err());
   }

   // ---------- init / revert_in_progress ----------

   #[test]
   fn test_init_reverts_in_progress_with_temp_file_to_paused() {
      let (manager, dir, events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();
      seed(&manager, &path, DownloadStatus::InProgress);

      manager.init();

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);

      assert!(
         event_log(&events)
            .iter()
            .any(|e| e.path == path && e.status == DownloadStatus::Paused)
      );
   }

   #[test]
   fn test_init_reverts_in_progress_without_temp_file_to_idle() {
      let (manager, dir, _events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      seed(&manager, &path, DownloadStatus::InProgress);

      manager.init();

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
   }

   #[test]
   fn test_init_leaves_non_in_progress_unchanged() {
      let (manager, _dir, _events) = make_manager();
      seed(&manager, "/tmp/a.mp4", DownloadStatus::Idle);
      seed(&manager, "/tmp/b.mp4", DownloadStatus::Paused);
      seed(&manager, "/tmp/c.mp4", DownloadStatus::Completed);

      manager.init();

      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/a.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Idle
      );
      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/b.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Paused
      );
      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/c.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Completed
      );
   }

   // ---------- filename helper ----------

   #[test]
   fn test_filename_with_separators() {
      assert_eq!(filename("/tmp/dir/file.mp4"), "file.mp4");
   }

   #[test]
   fn test_filename_without_separators() {
      assert_eq!(filename("file.mp4"), "file.mp4");
   }

   #[test]
   fn test_filename_falls_back_for_empty_input() {
      assert_eq!(filename(""), "");
   }
}
