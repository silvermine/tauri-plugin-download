use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{DownloadItem, DownloadStatus, Error};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedDownloadItem {
   url: String,
   path: String,
   #[serde(default, skip_serializing, rename = "progress")]
   legacy_progress: Option<f64>,
   #[serde(default)]
   transferred_bytes: u64,
   total_bytes: Option<u64>,
   status: DownloadStatus,
}

impl From<&DownloadItem> for PersistedDownloadItem {
   fn from(item: &DownloadItem) -> Self {
      Self {
         url: item.url.clone(),
         path: item.path.clone(),
         legacy_progress: None,
         transferred_bytes: item.transferred_bytes,
         total_bytes: item.total_bytes,
         status: item.status.clone(),
      }
   }
}

impl From<PersistedDownloadItem> for DownloadItem {
   fn from(item: PersistedDownloadItem) -> Self {
      let progress = if item.transferred_bytes == 0 && item.total_bytes.is_none() {
         item.legacy_progress.unwrap_or_else(|| {
            DownloadItem::progress_for(item.transferred_bytes, item.total_bytes, &item.status)
         })
      } else {
         DownloadItem::progress_for(item.transferred_bytes, item.total_bytes, &item.status)
      };
      DownloadItem {
         url: item.url,
         path: item.path,
         progress,
         transferred_bytes: item.transferred_bytes,
         total_bytes: item.total_bytes,
         status: item.status,
      }
   }
}

/// Thread-safe JSON file store for download items, mirroring iOS `DownloadStore`.
#[derive(Clone, Debug)]
pub struct DownloadStore {
   inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
   downloads: Vec<DownloadItem>,
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

   pub fn list(&self) -> crate::Result<Vec<DownloadItem>> {
      let inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;
      Ok(inner.downloads.clone())
   }

   pub fn find_by_path(&self, path: &str) -> crate::Result<Option<DownloadItem>> {
      let inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;
      Ok(inner.downloads.iter().find(|i| i.path == path).cloned())
   }

   pub fn create(&self, item: DownloadItem) -> crate::Result<DownloadItem> {
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

   pub fn update(&self, item: DownloadItem) -> crate::Result<()> {
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

   pub fn update_no_persist(&self, item: DownloadItem) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      if let Some(existing) = inner.downloads.iter_mut().find(|i| i.path == item.path) {
         *existing = item;
      }
      Ok(())
   }

   pub fn delete(&self, path: &str) -> crate::Result<()> {
      let mut inner = self
         .inner
         .lock()
         .map_err(|e| Error::Store(format!("Lock poisoned: {}", e)))?;

      inner.downloads.retain(|i| i.path != path);
      save_inner(&inner)?;
      Ok(())
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
      let persisted: Vec<PersistedDownloadItem> = serde_json::from_slice(&data)
         .map_err(|e| Error::Store(format!("Failed to parse store: {}", e)))?;
      inner.downloads = persisted.into_iter().map(Into::into).collect();

      Ok(())
   }
}

/// Serializes and writes the store to disk.
///
/// Accepts `&StoreInner` directly rather than `&self` because callers already hold the
/// `MutexGuard` when they call this. Taking `&self` would attempt to re-acquire the lock
/// on the same thread, causing a deadlock since `Mutex` is not re-entrant.
fn save_inner(inner: &StoreInner) -> crate::Result<()> {
   if let Some(parent) = Path::new(&inner.path).parent()
      && !parent.exists()
   {
      fs::create_dir_all(parent)
         .map_err(|e| Error::Store(format!("Failed to create store directory: {}", e)))?;
   }

   let persisted: Vec<PersistedDownloadItem> = inner
      .downloads
      .iter()
      .map(PersistedDownloadItem::from)
      .collect();
   let data = serde_json::to_vec(&persisted)
      .map_err(|e| Error::Store(format!("Failed to serialize store: {}", e)))?;
   fs::write(&inner.path, &data)
      .map_err(|e| Error::Store(format!("Failed to write store: {}", e)))?;
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

   fn sample_item(path: &str) -> DownloadItem {
      DownloadItem {
         url: "https://example.com/file.mp4".to_string(),
         path: path.to_string(),
         progress: 0.0,
         transferred_bytes: 0,
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
      store.create(sample_item("/tmp/a.mp4")).unwrap();
      store.create(sample_item("/tmp/b.mp4")).unwrap();
      assert_eq!(store.list().unwrap().len(), 2);
   }

   #[test]
   fn test_find_by_path_found() {
      let (store, _dir) = temp_store();
      store.create(sample_item("/tmp/file.mp4")).unwrap();
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
      let item = store.create(sample_item("/tmp/file.mp4")).unwrap();
      assert_eq!(item.path, "/tmp/file.mp4");
   }

   #[test]
   fn test_create_persists_to_disk() {
      let (store, dir) = temp_store();
      store.create(sample_item("/tmp/file.mp4")).unwrap();
      assert!(dir.path().join("downloads.json").exists());
   }

   #[test]
   fn test_create_duplicate_returns_error() {
      let (store, _dir) = temp_store();
      store.create(sample_item("/tmp/file.mp4")).unwrap();
      let result = store.create(sample_item("/tmp/file.mp4"));
      assert!(result.is_err());
   }

   #[test]
   fn test_update_persists_to_disk() {
      let (store, dir) = temp_store();
      let item = store.create(sample_item("/tmp/file.mp4")).unwrap();
      let updated = DownloadItem {
         progress: 50.0,
         transferred_bytes: 50,
         total_bytes: Some(100),
         status: DownloadStatus::InProgress,
         ..item
      };
      store.update(updated).unwrap();

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let found = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(found.progress, 50.0);
   }

   #[test]
   fn test_persisted_store_omits_progress_field() {
      let (store, dir) = temp_store();
      let item = store.create(sample_item("/tmp/file.mp4")).unwrap();
      let updated = DownloadItem {
         progress: 50.0,
         transferred_bytes: 50,
         total_bytes: Some(100),
         status: DownloadStatus::Paused,
         ..item
      };
      store.update(updated).unwrap();

      let json = fs::read_to_string(dir.path().join("downloads.json")).unwrap();
      assert!(
         !json.contains("progress"),
         "progress should be derived, not persisted: {}",
         json
      );
   }

   #[test]
   fn test_load_derives_progress_from_byte_counts() {
      let dir = TempDir::new().unwrap();
      let path = dir.path().join("downloads.json");
      fs::write(
         &path,
         serde_json::json!([
            {
               "url": "https://example.com/file.mp4",
               "path": "/tmp/file.mp4",
               "transferredBytes": 50,
               "totalBytes": 100,
               "status": "paused"
            }
         ])
         .to_string(),
      )
      .unwrap();

      let store = DownloadStore::new(path);
      store.load().unwrap();
      let found = store.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(found.progress, 50.0);
      assert_eq!(found.transferred_bytes, 50);
      assert_eq!(found.total_bytes, Some(100));
   }

   #[test]
   fn test_load_preserves_legacy_progress_when_byte_counts_are_missing() {
      let dir = TempDir::new().unwrap();
      let path = dir.path().join("downloads.json");
      fs::write(
         &path,
         serde_json::json!([
            {
               "url": "https://example.com/file.mp4",
               "path": "/tmp/file.mp4",
               "progress": 42.5,
               "status": "paused"
            }
         ])
         .to_string(),
      )
      .unwrap();

      let store = DownloadStore::new(path);
      store.load().unwrap();
      let found = store.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(found.progress, 42.5);
      assert_eq!(found.transferred_bytes, 0);
      assert_eq!(found.total_bytes, None);
      assert_eq!(found.status, DownloadStatus::Paused);
   }

   #[test]
   fn test_update_no_op_on_unknown_path() {
      let (store, _dir) = temp_store();
      store.create(sample_item("/tmp/file.mp4")).unwrap();
      let unknown = sample_item("/tmp/unknown.mp4");
      assert!(store.update(unknown).is_ok());
      assert_eq!(store.list().unwrap().len(), 1);
   }

   #[test]
   fn test_update_no_persist_does_not_write_disk() {
      let (store, dir) = temp_store();
      let item = store.create(sample_item("/tmp/file.mp4")).unwrap();
      let updated = DownloadItem {
         progress: 75.0,
         transferred_bytes: 75,
         total_bytes: Some(100),
         ..item
      };
      store.update_no_persist(updated).unwrap();

      // In-memory reflects the change.
      let in_memory = store.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(in_memory.progress, 75.0);

      // Disk still has the original value.
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let on_disk = reloaded.find_by_path("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(on_disk.progress, 0.0);
   }

   #[test]
   fn test_delete_removes_item_and_persists() {
      let (store, dir) = temp_store();
      store.create(sample_item("/tmp/file.mp4")).unwrap();
      store.delete("/tmp/file.mp4").unwrap();

      assert!(store.list().unwrap().is_empty());

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert!(reloaded.list().unwrap().is_empty());
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
      let items = vec![sample_item("/tmp/file.mp4")];
      fs::write(&path, serde_json::to_vec(&items).unwrap()).unwrap();

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
      store.create(sample_item("/tmp/file.mp4")).unwrap();
      assert!(dir.path().join("nested/dir/downloads.json").exists());
   }
}
