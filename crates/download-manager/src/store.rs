use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::Error;
use crate::models::{DownloadRecord, DownloadStatus};

const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Private on-disk format. Record types and migrations are introduced only when
/// a future schema needs them; v1 uses the current persisted record unchanged.
#[derive(Serialize, Deserialize)]
struct StoreDocument<T> {
   version: u32,
   downloads: Vec<T>,
}

/// Decode records after validating the store envelope and supported schema version.
/// Reject the whole document on invalid records, using errors that omit input data.
fn decode_store(data: &[u8]) -> crate::Result<Vec<DownloadRecord>> {
   // Require an object explicitly: serde can also deserialize structs from arrays.
   let fields: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(data)
      .map_err(|_| Error::Store("Malformed store envelope".to_string()))?;
   let document: StoreDocument<serde_json::Value> =
      serde_json::from_value(serde_json::Value::Object(fields))
         .map_err(|_| Error::Store("Malformed store envelope".to_string()))?;

   if document.version != CURRENT_SCHEMA_VERSION {
      return Err(Error::Store(format!(
         "Unsupported store version: {} (expected {})",
         document.version, CURRENT_SCHEMA_VERSION
      )));
   }

   // Keep decoder details out of logs: they can contain URLs and other input values.
   serde_json::from_value(serde_json::Value::Array(document.downloads))
      .map_err(|_| Error::Store("Invalid store records".to_string()))
}

pub(crate) enum UpdateIfStatusResult {
   Updated(DownloadRecord),
   Unchanged(DownloadRecord),
   NotFound,
}

pub(crate) enum DeleteIfStatusResult {
   Deleted(DownloadRecord),
   Unchanged(DownloadRecord),
   NotFound,
}

/// Controls whether a store mutation is limited to memory or also written to disk.
pub(crate) enum PersistMode {
   /// Update only the in-memory record.
   InMemoryOnly,
   /// Persist the updated record to disk.
   ToDisk,
}

/// Thread-safe JSON file store for download records, mirroring iOS `DownloadStore`.
#[derive(Clone, Debug)]
pub struct DownloadStore {
   inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
   downloads: Vec<DownloadRecord>,
   path: PathBuf,
}

impl DownloadStore {
   /// Creates a new store backed by the given file path.
   pub fn new(path: PathBuf) -> Self {
      Self {
         inner: Arc::new(Mutex::new(StoreInner {
            downloads: Vec::new(),
            path,
         })),
      }
   }

   pub fn list(&self) -> crate::Result<Vec<DownloadRecord>> {
      let inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;
      Ok(inner.downloads.clone())
   }

   pub fn find_by_path(&self, path: &str) -> crate::Result<Option<DownloadRecord>> {
      let inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;
      Ok(inner.downloads.iter().find(|i| i.path == path).cloned())
   }

   pub fn create(&self, item: DownloadRecord) -> crate::Result<DownloadRecord> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      if inner.downloads.iter().any(|i| i.path == item.path) {
         return Err(Error::Store(format!(
            "Item already exists for path: {}",
            &item.path
         )));
      }

      inner.downloads.push(item.clone());
      save_inner(&inner)?;
      Ok(item)
   }

   #[cfg(test)]
   pub fn update(&self, item: DownloadRecord) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      if let Some(existing) = inner.downloads.iter_mut().find(|i| i.path == item.path) {
         *existing = item;
      }
      save_inner(&inner)?;
      Ok(())
   }

   pub fn update_if_status(
      &self,
      path: &str,
      expected_status: DownloadStatus,
      new_status: DownloadStatus,
   ) -> crate::Result<UpdateIfStatusResult> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      let Some(index) = inner.downloads.iter().position(|item| item.path == path) else {
         return Ok(UpdateIfStatusResult::NotFound);
      };

      let existing = &mut inner.downloads[index];
      if existing.status != expected_status {
         return Ok(UpdateIfStatusResult::Unchanged(existing.clone()));
      }

      existing.status = new_status;
      let updated = existing.clone();
      if let Err(error) = save_inner(&inner) {
         // Callers must be able to retry a failed transition, including signaling
         // the runtime worker after a successful pause.
         inner.downloads[index].status = expected_status;
         return Err(error);
      }
      Ok(UpdateIfStatusResult::Updated(updated))
   }

   #[cfg(test)]
   pub fn update_no_persist(&self, item: DownloadRecord) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      if let Some(existing) = inner.downloads.iter_mut().find(|i| i.path == item.path) {
         *existing = item;
      }
      Ok(())
   }

   /// Returns the locked store and record index only while the download is active.
   fn lock_active(&self, path: &str) -> crate::Result<Option<(MutexGuard<'_, StoreInner>, usize)>> {
      let inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;
      let Some(index) = inner.downloads.iter().position(|item| item.path == path) else {
         return Ok(None);
      };
      if inner.downloads[index].status != DownloadStatus::InProgress {
         return Ok(None);
      }
      Ok(Some((inner, index)))
   }

   /// Updates byte counts only while the download is still active.
   ///
   /// The status check and byte update happen under the same lock so a progress
   /// checkpoint cannot overwrite a concurrent pause with a stale record.
   pub(crate) fn update_active_bytes(
      &self,
      path: &str,
      received_bytes: u64,
      total_bytes: Option<u64>,
      persist_mode: PersistMode,
   ) -> crate::Result<Option<DownloadRecord>> {
      let Some((mut inner, index)) = self.lock_active(path)? else {
         return Ok(None);
      };

      inner.downloads[index].received_bytes = received_bytes;
      inner.downloads[index].total_bytes = total_bytes;
      let updated = inner.downloads[index].clone();
      if matches!(persist_mode, PersistMode::ToDisk) {
         save_inner(&inner)?;
      }
      Ok(Some(updated))
   }

   /// Publishes and removes a completed download only while it is still active.
   ///
   /// `publish` runs while the store lock is held, so pause and cancel cannot
   /// interleave between the status check and publishing the completed file.
   /// It must not re-enter the store because the mutex is not reentrant.
   pub(crate) fn complete_active(
      &self,
      path: &str,
      received_bytes: u64,
      total_bytes: Option<u64>,
      publish: impl FnOnce() -> crate::Result<()>,
   ) -> crate::Result<Option<DownloadRecord>> {
      let Some((mut inner, index)) = self.lock_active(path)? else {
         return Ok(None);
      };

      publish()?;
      let completed = inner.downloads[index]
         .clone()
         .with_bytes(received_bytes, total_bytes)
         .with_status(DownloadStatus::Completed);
      inner.downloads.remove(index);
      save_inner(&inner)?;
      Ok(Some(completed))
   }

   /// Reverts a failed download only while it is still active.
   pub(crate) fn revert_active(
      &self,
      path: &str,
      received_bytes: u64,
      status: DownloadStatus,
   ) -> crate::Result<Option<DownloadRecord>> {
      let Some((mut inner, index)) = self.lock_active(path)? else {
         return Ok(None);
      };

      inner.downloads[index].received_bytes = received_bytes;
      inner.downloads[index].status = status;
      let reverted = inner.downloads[index].clone();
      save_inner(&inner)?;
      Ok(Some(reverted))
   }

   #[cfg(test)]
   pub fn delete(&self, path: &str) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      inner.downloads.retain(|i| i.path != path);
      save_inner(&inner)?;
      Ok(())
   }

   /// Deletes and returns a record only when its status is cancellable.
   pub(crate) fn delete_if_status(
      &self,
      path: &str,
      expected_statuses: &[DownloadStatus],
   ) -> crate::Result<DeleteIfStatusResult> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      let Some(index) = inner.downloads.iter().position(|item| item.path == path) else {
         return Ok(DeleteIfStatusResult::NotFound);
      };
      if !expected_statuses.contains(&inner.downloads[index].status) {
         return Ok(DeleteIfStatusResult::Unchanged(
            inner.downloads[index].clone(),
         ));
      }

      let deleted = inner.downloads.remove(index);
      if let Err(error) = save_inner(&inner) {
         // Keep the record and its worker consistent when cancellation fails.
         inner.downloads.insert(index, deleted);
         return Err(error);
      }
      Ok(DeleteIfStatusResult::Deleted(deleted))
   }

   /// Loads the store from disk. Should be called once at startup.
   pub fn load(&self) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      if !inner.path.exists() {
         return Ok(());
      }

      let data =
         fs::read(&inner.path).map_err(|e| Error::Store(format!("Failed to read store: {}", e)))?;
      inner.downloads = decode_store(&data)?;

      Ok(())
   }
}

/// Serializes and writes the store to disk.
///
/// Accepts `&StoreInner` directly rather than `&self` because callers already hold the
/// `MutexGuard` when they call this. Taking `&self` would attempt to re-acquire the lock
/// on the same thread, causing a deadlock since `Mutex` is not re-entrant.
///
/// Writes atomically, as iOS and Android do, so a crash mid-write leaves the previous store
/// intact rather than truncated JSON. Two consequences: the store takes the temp file's
/// `0600` on Unix, leaving it app-private as it is on mobile, and the directory is not
/// fsynced, so a power loss can cost the last update but never the file.
fn save_inner(inner: &StoreInner) -> crate::Result<()> {
   let parent = inner
      .path
      .parent()
      .ok_or_else(|| Error::Store("Store path has no parent directory".to_string()))?;

   fs::create_dir_all(parent)
      .map_err(|e| Error::Store(format!("Failed to create store directory: {}", e)))?;

   // Serialize before touching the disk so a failure here cannot leave a temp file behind.
   let document = StoreDocument {
      version: CURRENT_SCHEMA_VERSION,
      downloads: inner.downloads.iter().collect(),
   };
   let data = serde_json::to_vec(&document)
      .map_err(|e| Error::Store(format!("Failed to serialize store: {}", e)))?;

   // The temp file has to be a sibling of the store: `rename` cannot cross a mount point.
   // `NamedTempFile` removes itself on drop, so the early returns below leave no debris.
   let mut temp = NamedTempFile::new_in(parent)
      .map_err(|e| Error::Store(format!("Failed to create temp store file: {}", e)))?;

   temp
      .write_all(&data)
      .map_err(|e| Error::Store(format!("Failed to write store: {}", e)))?;

   // Sync before the rename, as Android's `AtomicFile.finishWrite` does. `flush` would be
   // a no-op — the writes are unbuffered — and the rename only publishes a directory entry,
   // so without this a crash can leave a correctly named file whose contents never landed.
   temp.as_file().sync_all().map_err(|e| {
      Error::Store(format!(
         "Failed to sync store to disk: {} at path {:?}",
         e,
         temp.path()
      ))
   })?;

   temp.persist(&inner.path).map_err(|e| {
      Error::Store(format!(
         "Failed to replace store: {} at path {:?}",
         e.error, inner.path
      ))
   })?;

   Ok(())
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::models::DownloadStatus;
   use std::fs;
   use tempfile::TempDir;

   fn temp_store() -> (DownloadStore, TempDir) {
      let dir = TempDir::new().unwrap();
      let store = DownloadStore::new(dir.path().join("downloads.json"));
      (store, dir)
   }

   fn sample_record(path: &str) -> DownloadRecord {
      DownloadRecord {
         url: "https://example.com/file.mp4".to_string(),
         path: path.to_string(),
         options: Default::default(),
         received_bytes: 0,
         total_bytes: None,
         status: DownloadStatus::Idle,
      }
   }

   #[test]
   fn test_list_empty() {
      let (store, _dir) = temp_store();
      assert!(store.list().unwrap().is_empty());
   }

   #[test]
   fn test_list_after_create() {
      let (store, _dir) = temp_store();
      store.create(sample_record("/tmp/a.mp4")).unwrap();
      store.create(sample_record("/tmp/b.mp4")).unwrap();
      assert_eq!(store.list().unwrap().len(), 2);
   }

   #[test]
   fn test_find_by_path_found() {
      let (store, _dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      let result = store.find_by_path("/tmp/file.mp4").unwrap();
      assert_eq!(result.unwrap().path, "/tmp/file.mp4");
   }

   #[test]
   fn test_find_by_path_not_found() {
      let (store, _dir) = temp_store();
      assert!(store.find_by_path("/tmp/missing.mp4").unwrap().is_none());
   }

   #[test]
   fn test_create_success() {
      let (store, _dir) = temp_store();
      let item = store.create(sample_record("/tmp/file.mp4")).unwrap();
      assert_eq!(item.path, "/tmp/file.mp4");
   }

   #[test]
   fn test_create_persists_to_disk() {
      let (store, dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      assert!(dir.path().join("downloads.json").exists());
   }

   #[test]
   fn test_create_options_persist_across_reload() {
      let (store, dir) = temp_store();
      let mut item = sample_record("/tmp/file.mp4");
      item.options.allow_metered = false;
      store.create(item).unwrap();

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();

      let found = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert!(!found.options.allow_metered);
   }

   #[test]
   fn test_create_duplicate_returns_error() {
      let (store, _dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      let result = store.create(sample_record("/tmp/file.mp4"));
      assert!(result.is_err());
   }

   #[test]
   fn test_update_persists_to_disk() {
      let (store, dir) = temp_store();
      let item = store.create(sample_record("/tmp/file.mp4")).unwrap();
      let updated = DownloadRecord {
         received_bytes: 500,
         total_bytes: Some(1000),
         status: DownloadStatus::InProgress,
         ..item
      };
      store.update(updated).unwrap();

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let found = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(found.received_bytes, 500);
      assert_eq!(found.total_bytes, Some(1000));
   }

   #[test]
   fn test_update_no_op_on_unknown_path() {
      let (store, _dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      let unknown = sample_record("/tmp/unknown.mp4");
      assert!(store.update(unknown).is_ok());
      assert_eq!(store.list().unwrap().len(), 1);
   }

   #[test]
   fn test_update_if_status_updates_matching_record() {
      let (store, dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();

      let result = store
         .update_if_status(
            "/tmp/file.mp4",
            DownloadStatus::Idle,
            DownloadStatus::InProgress,
         )
         .unwrap();

      assert!(matches!(
         result,
         UpdateIfStatusResult::Updated(item)
            if item.status == DownloadStatus::InProgress
      ));

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded
            .find_by_path("/tmp/file.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::InProgress
      );
   }

   #[test]
   fn test_update_if_status_returns_current_record_on_mismatch() {
      let (store, _dir) = temp_store();
      let mut item = sample_record("/tmp/file.mp4");
      item.status = DownloadStatus::Paused;
      store.create(item).unwrap();

      let result = store
         .update_if_status(
            "/tmp/file.mp4",
            DownloadStatus::Idle,
            DownloadStatus::InProgress,
         )
         .unwrap();

      assert!(matches!(
         result,
         UpdateIfStatusResult::Unchanged(item) if item.status == DownloadStatus::Paused
      ));
      assert_eq!(
         store.find_by_path("/tmp/file.mp4").unwrap().unwrap().status,
         DownloadStatus::Paused
      );
   }

   #[test]
   fn test_update_if_status_returns_not_found_for_unknown_path() {
      let (store, _dir) = temp_store();

      assert!(matches!(
         store
            .update_if_status(
               "/tmp/missing.mp4",
               DownloadStatus::Idle,
               DownloadStatus::InProgress,
            )
            .unwrap(),
         UpdateIfStatusResult::NotFound
      ));
   }

   #[test]
   fn test_update_no_persist_does_not_write_disk() {
      let (store, dir) = temp_store();
      let item = store.create(sample_record("/tmp/file.mp4")).unwrap();
      let updated = DownloadRecord {
         received_bytes: 750,
         total_bytes: Some(1000),
         ..item
      };
      store.update_no_persist(updated).unwrap();

      // In-memory reflects the change.
      let in_memory = store.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(in_memory.received_bytes, 750);

      // Disk still has the original value.
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let on_disk = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(on_disk.received_bytes, 0);
   }

   #[test]
   fn test_update_active_bytes_does_not_overwrite_pause() {
      let (store, _dir) = temp_store();
      let active = sample_record("/tmp/file.mp4").with_status(DownloadStatus::InProgress);
      store.create(active.clone()).unwrap();
      store
         .update(active.with_status(DownloadStatus::Paused))
         .unwrap();

      let updated = store
         .update_active_bytes("/tmp/file.mp4", 500, Some(1000), PersistMode::InMemoryOnly)
         .unwrap();

      assert!(updated.is_none());
      let stored = store.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, None);
   }

   #[test]
   fn test_persist_active_bytes_does_not_overwrite_pause() {
      let (store, dir) = temp_store();
      let active = sample_record("/tmp/file.mp4").with_status(DownloadStatus::InProgress);
      store.create(active.clone()).unwrap();
      store
         .update(active.with_status(DownloadStatus::Paused))
         .unwrap();

      let updated = store
         .update_active_bytes("/tmp/file.mp4", 500, Some(1000), PersistMode::ToDisk)
         .unwrap();

      assert!(updated.is_none());
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let stored = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, None);
   }

   #[test]
   fn test_delete_removes_item_and_persists() {
      let (store, dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      store.delete("/tmp/file.mp4").unwrap();

      assert!(store.list().unwrap().is_empty());

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert!(reloaded.list().unwrap().is_empty());
   }

   #[test]
   fn test_delete_if_status_removes_matching_record_and_persists() {
      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(sample_record(path).with_status(DownloadStatus::Paused))
         .unwrap();
      store.create(sample_record("/tmp/other.mp4")).unwrap();

      assert!(matches!(
         store.delete_if_status(path, &[DownloadStatus::Idle, DownloadStatus::InProgress, DownloadStatus::Paused]).unwrap(),
         DeleteIfStatusResult::Deleted(item) if item.path == path && item.status == DownloadStatus::Paused
      ));
      assert!(store.find_by_path(path).unwrap().is_none());
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let remaining = reloaded.list().unwrap();
      assert_eq!(remaining.len(), 1);
      assert_eq!(remaining[0].path, "/tmp/other.mp4");
   }

   #[test]
   fn test_delete_if_status_preserves_mismatched_record() {
      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(sample_record(path).with_status(DownloadStatus::Paused))
         .unwrap();

      assert!(matches!(
         store.delete_if_status(path, &[DownloadStatus::Idle]).unwrap(),
         DeleteIfStatusResult::Unchanged(item) if item.path == path && item.status == DownloadStatus::Paused
      ));
      assert_eq!(
         store.find_by_path(path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded.find_by_path(path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
   }

   #[test]
   fn test_delete_if_status_returns_not_found() {
      let (store, _dir) = temp_store();
      assert!(matches!(
         store
            .delete_if_status("/tmp/missing.mp4", &[DownloadStatus::InProgress])
            .unwrap(),
         DeleteIfStatusResult::NotFound
      ));
   }

   #[test]
   fn test_revert_active_preserves_latest_total_and_persists() {
      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(sample_record(path).with_status(DownloadStatus::InProgress))
         .unwrap();
      store
         .update_active_bytes(path, 100, Some(1000), PersistMode::InMemoryOnly)
         .unwrap();

      let reverted = store
         .revert_active(path, 150, DownloadStatus::Paused)
         .unwrap()
         .unwrap();
      assert_eq!(reverted.status, DownloadStatus::Paused);
      assert_eq!(reverted.received_bytes, 150);
      assert_eq!(reverted.total_bytes, Some(1000));
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let stored = reloaded.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 150);
      assert_eq!(stored.total_bytes, Some(1000));
   }

   #[test]
   fn test_revert_active_respects_pause_and_removal() {
      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(
            sample_record(path)
               .with_bytes(100, Some(1000))
               .with_status(DownloadStatus::Paused),
         )
         .unwrap();

      assert!(
         store
            .revert_active(path, 0, DownloadStatus::Idle)
            .unwrap()
            .is_none()
      );
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let stored = reloaded.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 100);
      assert_eq!(stored.total_bytes, Some(1000));
      assert_eq!(
         store.find_by_path(path).unwrap().unwrap().received_bytes,
         100
      );

      store
         .delete_if_status(path, &[DownloadStatus::Paused])
         .unwrap();
      assert!(
         store
            .revert_active(path, 0, DownloadStatus::Idle)
            .unwrap()
            .is_none()
      );
      assert!(store.find_by_path(path).unwrap().is_none());
      reloaded.load().unwrap();
      assert!(reloaded.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_complete_active_publish_failure_preserves_record() {
      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(sample_record(path).with_status(DownloadStatus::InProgress))
         .unwrap();

      let result = store.complete_active(path, 100, Some(100), || {
         Err(Error::File("publish failed".to_string()))
      });
      assert!(matches!(result, Err(Error::File(message)) if message == "publish failed"));
      assert_eq!(
         store.find_by_path(path).unwrap().unwrap().status,
         DownloadStatus::InProgress
      );
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded.find_by_path(path).unwrap().unwrap().status,
         DownloadStatus::InProgress
      );
   }

   #[test]
   fn test_complete_active_excludes_cancel_during_publish() {
      use std::sync::{TryLockError, mpsc};

      let (store, dir) = temp_store();
      let path = "/tmp/file.mp4";
      store
         .create(sample_record(path).with_status(DownloadStatus::InProgress))
         .unwrap();
      let (publishing, entered) = mpsc::channel();
      let (probed, probe_result) = mpsc::channel();

      std::thread::scope(|scope| {
         let cancel_store = &store;
         let cancel = scope.spawn(move || {
            entered.recv().unwrap();
            // Probe from another thread while publication is suspended. This
            // detects a split critical section without relying on scheduling delays.
            let locked = matches!(cancel_store.inner.try_lock(), Err(TryLockError::WouldBlock));
            probed.send(locked).unwrap();
            cancel_store
               .delete_if_status(path, &[DownloadStatus::InProgress])
               .unwrap()
         });
         let completed = store
            .complete_active(path, 100, Some(100), || {
               publishing.send(()).unwrap();
               assert!(
                  probe_result.recv().unwrap(),
                  "publication must hold the store lock"
               );
               Ok(())
            })
            .unwrap()
            .unwrap();
         assert_eq!(completed.status, DownloadStatus::Completed);
         assert_eq!(completed.received_bytes, 100);
         assert!(matches!(
            cancel.join().unwrap(),
            DeleteIfStatusResult::NotFound
         ));
      });

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert!(reloaded.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_delete_unknown_path_is_ok() {
      let (store, _dir) = temp_store();
      assert!(store.delete("/tmp/nonexistent.mp4").is_ok());
   }

   #[test]
   fn test_load_missing_file_is_ok() {
      let (store, _dir) = temp_store();
      assert!(store.load().is_ok());
      assert!(store.list().unwrap().is_empty());
   }

   #[test]
   fn test_load_from_valid_json() {
      let dir = TempDir::new().unwrap();
      let path = dir.path().join("downloads.json");
      let items = vec![sample_record("/tmp/file.mp4")];
      let document = serde_json::json!({ "version": 1, "downloads": items });
      fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

      let store = DownloadStore::new(path);
      store.load().unwrap();
      assert_eq!(store.list().unwrap().len(), 1);
   }

   #[test]
   fn test_load_invalid_json_returns_error() {
      let dir = TempDir::new().unwrap();
      let path = dir.path().join("downloads.json");
      fs::write(&path, b"not valid json").unwrap();

      let store = DownloadStore::new(path);
      assert!(store.load().is_err());
   }

   #[test]
   fn test_save_creates_parent_directory() {
      let dir = TempDir::new().unwrap();
      let store = DownloadStore::new(dir.path().join("nested/dir/downloads.json"));
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      assert!(dir.path().join("nested/dir/downloads.json").exists());
      let reloaded = DownloadStore::new(dir.path().join("nested/dir/downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(reloaded.list().unwrap().len(), 1);
   }

   #[test]
   fn test_writer_emits_v1_and_round_trips_all_record_fields() {
      let (store, dir) = temp_store();
      let mut item = sample_record("/tmp/file.mp4");
      item.options.allow_metered = false;
      item.received_bytes = 123;
      item.total_bytes = Some(456);
      item.status = DownloadStatus::Paused;
      store.create(item.clone()).unwrap();

      let bytes = fs::read(dir.path().join("downloads.json")).unwrap();
      let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
      assert_eq!(document["version"], serde_json::json!(1));
      assert_eq!(document["downloads"], serde_json::json!([item]));
      assert_eq!(
         serde_json::to_value(decode_store(&bytes).unwrap()).unwrap(),
         serde_json::json!([item])
      );

      store.delete(&item.path).unwrap();
      let bytes = fs::read(dir.path().join("downloads.json")).unwrap();
      assert_eq!(
         serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
         serde_json::json!({ "version": 1, "downloads": [] })
      );
      assert!(decode_store(&bytes).unwrap().is_empty());
   }

   #[test]
   fn test_missing_file_is_not_created_on_load() {
      let (store, dir) = temp_store();
      store.load().unwrap();
      assert!(!dir.path().join("downloads.json").exists());
   }

   #[test]
   fn test_rejects_legacy_arrays_and_malformed_envelopes() {
      for text in [
         "[]",
         "[1, []]",
         "[{}]",
         "null",
         "true",
         "1",
         "{}",
         r#"{"version":1}"#,
         r#"{"downloads":[]}"#,
         r#"{"version":1,"downloads":null}"#,
         r#"{"version":1,"downloads":{}}"#,
         r#"{"version":1,"downloads":"private input"}"#,
         "not json",
      ] {
         assert!(
            matches!(decode_store(text.as_bytes()), Err(Error::Store(message))
               if message == "Malformed store envelope"),
            "Accepted or misclassified {text}"
         );
      }
   }

   #[test]
   fn test_rejects_invalid_version_types_and_values() {
      for version in [
         r#""1""#, "true", "false", "null", "-1", "1.5", "1.0", "1e0", "+1", "01", "[]", "{}",
      ] {
         let text = format!(r#"{{"version":{version},"downloads":[]}}"#);
         assert!(
            matches!(decode_store(text.as_bytes()), Err(Error::Store(message))
               if message == "Malformed store envelope"),
            "Accepted or misclassified {version}"
         );
      }
   }

   #[test]
   fn test_unsupported_version_is_checked_before_record_decoding() {
      for version in [0, 2, u32::MAX] {
         let text = format!(r#"{{"version":{version},"downloads":[{{"future":"record"}}]}}"#);
         assert!(
            matches!(decode_store(text.as_bytes()), Err(Error::Store(message))
            if message == format!("Unsupported store version: {version} (expected 1)"))
         );
      }
   }

   #[test]
   fn test_unknown_fields_are_ignored_without_rewriting_on_load() {
      let (store, dir) = temp_store();
      let mut item = serde_json::to_value(sample_record("/tmp/file.mp4")).unwrap();
      item["extraRecordField"] = serde_json::json!(true);
      let document = serde_json::json!({
         "version": 1, "downloads": [item], "extraEnvelopeField": "ignored"
      });
      let bytes = serde_json::to_vec_pretty(&document).unwrap();
      let path = dir.path().join("downloads.json");
      fs::write(&path, &bytes).unwrap();

      store.load().unwrap();
      assert_eq!(store.list().unwrap().len(), 1);
      assert_eq!(fs::read(path).unwrap(), bytes);
   }

   #[test]
   fn test_invalid_record_does_not_publish_partial_state_or_expose_input() {
      let (store, dir) = temp_store();
      store.create(sample_record("/tmp/existing.mp4")).unwrap();
      let good = serde_json::to_value(sample_record("/tmp/new.mp4")).unwrap();
      let mut bad = good.clone();
      bad["status"] = serde_json::json!("private input");
      let bytes = serde_json::to_vec(&serde_json::json!({
         "version": 1, "downloads": [good, bad]
      }))
      .unwrap();
      let path = dir.path().join("downloads.json");
      fs::write(&path, &bytes).unwrap();

      assert!(matches!(store.load(), Err(Error::Store(message))
         if message == "Invalid store records"));
      let items = store.list().unwrap();
      assert_eq!(items.len(), 1);
      assert_eq!(items[0].path, "/tmp/existing.mp4");
      assert_eq!(fs::read(path).unwrap(), bytes);
   }

   /// The temp file is removed on drop, so an early return between creating it and
   /// renaming it into place leaves no debris. A directory at the destination makes the
   /// rename fail, which is the only publish error reachable without fault injection.
   #[test]
   fn test_failed_save_leaves_no_temp_files() {
      let dir = TempDir::new().unwrap();
      let path = dir.path().join("downloads.json");
      fs::create_dir(&path).unwrap();
      let store = DownloadStore::new(path);

      assert!(store.create(sample_record("/tmp/file.mp4")).is_err());

      let entries: Vec<_> = fs::read_dir(dir.path())
         .unwrap()
         .map(|entry| entry.unwrap().file_name())
         .collect();
      assert_eq!(entries, vec!["downloads.json"]);
   }

   #[test]
   fn test_save_leaves_no_temp_files() {
      let (store, dir) = temp_store();
      store.create(sample_record("/tmp/file.mp4")).unwrap();
      store.delete("/tmp/file.mp4").unwrap();

      let entries: Vec<_> = fs::read_dir(dir.path())
         .unwrap()
         .map(|entry| entry.unwrap().file_name())
         .collect();
      assert_eq!(entries, vec!["downloads.json"]);
   }

   /// The store is published by renaming a temp file over it, so each save replaces the
   /// file rather than rewriting it. An in-place write would keep the same inode.
   #[cfg(unix)]
   #[test]
   fn test_save_replaces_rather_than_rewriting_in_place() {
      use std::os::unix::fs::MetadataExt;

      let (store, dir) = temp_store();
      let path = dir.path().join("downloads.json");

      store.create(sample_record("/tmp/file.mp4")).unwrap();
      let first = fs::metadata(&path).unwrap().ino();

      store.create(sample_record("/tmp/other.mp4")).unwrap();
      let second = fs::metadata(&path).unwrap().ino();

      assert_ne!(first, second);
   }
}
