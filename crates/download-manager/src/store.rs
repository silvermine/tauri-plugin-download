use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{DownloadItem, Error};

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
      inner.downloads = serde_json::from_slice(&data)
         .map_err(|e| Error::Store(format!("Failed to parse store: {}", e)))?;

      Ok(())
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
}

fn save_inner(inner: &StoreInner) -> crate::Result<()> {
   if let Some(parent) = Path::new(&inner.path).parent()
      && !parent.exists()
   {
      fs::create_dir_all(parent)
         .map_err(|e| Error::Store(format!("Failed to create store directory: {}", e)))?;
   }

   let data = serde_json::to_vec(&inner.downloads)
      .map_err(|e| Error::Store(format!("Failed to serialize store: {}", e)))?;
   fs::write(&inner.path, &data)
      .map_err(|e| Error::Store(format!("Failed to write store: {}", e)))?;
   Ok(())
}
