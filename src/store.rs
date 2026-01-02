use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use crate::{DownloadItem, Error};

static DOWNLOAD_STORE_PATH: &str = "downloads.json";

pub fn list<R: Runtime>(app: &AppHandle<R>) -> crate::Result<Vec<DownloadItem>> {
   let store = app
      .store(DOWNLOAD_STORE_PATH)
      .map_err(|e| Error::Store(format!("Failed to load store: {}", e)))?;

   let mut items = Vec::new();
   for key in store.keys() {
      if let Some(value) = store.get(&key) {
         let item: DownloadItem = serde_json::from_value(value)
            .map_err(|e| Error::Store(format!("Failed to parse item: {}", e)))?;
         items.push(item);
      }
   }

   Ok(items)
}

pub fn get<R: Runtime>(app: &AppHandle<R>, path: String) -> crate::Result<Option<DownloadItem>> {
   let store = app
      .store(DOWNLOAD_STORE_PATH)
      .map_err(|e| Error::Store(format!("Failed to load store: {}", e)))?;

   match store.get(&path) {
      Some(value) => Ok(Some(serde_json::from_value(value).unwrap())),
      None => Ok(None),
   }
}

pub fn create<R: Runtime>(app: &AppHandle<R>, item: DownloadItem) -> crate::Result<DownloadItem> {
   let store = app
      .store(DOWNLOAD_STORE_PATH)
      .map_err(|e| Error::Store(format!("Failed to load store: {}", e)))?;

   match store.get(&item.path) {
      Some(_) => {
         return Err(Error::Store(format!(
            "Item already exists for path: {}",
            &item.path
         )));
      }
      None => {
         store.set(&item.path, serde_json::to_value(&item).unwrap());
         store
            .save()
            .map_err(|e| Error::Store(format!("Failed to save store: {}", e)))?;
      }
   }

   Ok(item)
}

pub fn update<R: Runtime>(app: &AppHandle<R>, item: DownloadItem) -> crate::Result<()> {
   let store = app
      .store(DOWNLOAD_STORE_PATH)
      .map_err(|e| Error::Store(format!("Failed to load store: {}", e)))?;

   store.set(&item.path, serde_json::to_value(&item).unwrap());
   store
      .save()
      .map_err(|e| Error::Store(format!("Failed to save store: {}", e)))?;

   Ok(())
}

pub fn delete<R: Runtime>(app: &AppHandle<R>, key: String) -> crate::Result<()> {
   let store = app
      .store(DOWNLOAD_STORE_PATH)
      .map_err(|e| Error::Store(format!("Failed to load store: {}", e)))?;

   if store.has(&key) {
      store.delete(&key);
   }

   store
      .save()
      .map_err(|e| Error::Store(format!("Failed to save store: {}", e)))?;

   Ok(())
}
