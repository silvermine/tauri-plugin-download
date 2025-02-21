use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use tauri::AppHandle;
use tauri::{plugin::PluginApi, Runtime, Emitter};
use tauri_plugin_http::reqwest;
use tauri_plugin_http::reqwest::header::{HeaderMap, RANGE};
use tauri_plugin_store::StoreExt;

use crate::Error;
use crate::models::*;

pub fn init<R: Runtime, C: DeserializeOwned>(
  app: &AppHandle<R>,
  _api: PluginApi<R, C>,
) -> crate::Result<Download<R>> {
  Ok(Download(app.clone()))
}

/// Access to the download APIs.
pub struct Download<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Download<R> {
  ///
  /// Creates a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// - `url` - The download URL  for the resource.
  /// - `path` - The download path on the filesystem.
  /// 
  /// # Returns
  /// The download operation.
  pub fn create(&self, app: AppHandle<R>, key: String, url: String, path: String) -> crate::Result<DownloadRecord> {
    Download::create_record(&app, DownloadRecord {
      key,
      url,
      path,
      progress: 0.0,
      state: DownloadState::Created
    })
  }

  ///
  /// Gets a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// 
  /// # Returns
  /// The download operation.
  pub fn get(&self, app: AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    Download::get_record(&app, key)
  }

  ///
  /// Lists all download operations.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// 
  /// # Returns
  /// The list of download operations.
  pub fn list(&self, app: AppHandle<R>) -> crate::Result<Vec<DownloadRecord>> {
    Download::get_records(&app)
  }
  
  ///
  /// Starts a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// 
  /// # Returns
  /// The download operation.
  pub fn start(&self, app: AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    let record = Download::get_record(&app, key).unwrap();
    match record.state {
      // Allow download to be started when created.
      DownloadState::Created => {
          let record_started = record.with_state(DownloadState::InProgress);
          tokio::spawn(async move {
              Download::download(&app, record_started).await.unwrap();
          });

          Ok(record.with_state(DownloadState::InProgress))
      }

      // Throw if in any other state.
      _ => Err(Error::InvalidState),
    }
  }

  ///
  /// Cancels a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// 
  /// # Returns
  /// The download operation.
  pub fn cancel(&self, app: AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    let record = Download::get_record(&app, key).unwrap();
    match record.state {
      // Allow download to be cancelled when created, in progress or paused.
      DownloadState::Created | DownloadState::InProgress | DownloadState::Paused => {
        Download::remove_record(&app, record.key.clone()).unwrap();
        if fs::remove_file(record.path.clone()).is_err() {
          println!("[{}] File was not found or could not be deleted", &record.key);
        }

        Download::emit_state(&app, record.key.clone(), DownloadState::Cancelled);
        Ok(record.with_state(DownloadState::Cancelled))
      }

      // Throw if in any other state.
      _ => Err(Error::InvalidState),
    }
  }

  ///
  /// Pauses a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// 
  /// # Returns
  /// The download operation.
  pub fn pause(&self, app: AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    let record = Download::get_record(&app, key).unwrap();
    match record.state {
      // Allow download to be paused when in progress.
      DownloadState::InProgress => {
        let record_paused = record.with_state(DownloadState::Paused);
        Download::update_record(&app, record_paused.clone()).unwrap();
        Download::emit_state(&app, record.key.clone(), DownloadState::Paused);
        Ok(record.with_state(DownloadState::Paused))
      }

      // Throw if in any other state.
      _ => Err(Error::InvalidState),
    }
  }

  ///
  /// Resumes a download operation.
  ///
  /// # Arguments
  /// - `app` - The application handle.
  /// - `key` - The key identifier.
  /// 
  /// # Returns
  /// The download operation.
  pub fn resume(&self, app: AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    let record = Download::get_record(&app, key).unwrap();
    match record.state {
      // Allow download to be resumed when paused.
      DownloadState::Paused => {
          let record_resumed = record.with_state(DownloadState::InProgress);
          tokio::spawn(async move {
            Download::download(&app, record_resumed).await.unwrap();
          });

          Ok(record.with_state(DownloadState::InProgress))
      }

      // Throw if in any other state.
      _ => Err(Error::InvalidState),
    }
  }

  async fn download(app: &AppHandle<R>, record: DownloadRecord) -> crate::Result<()> {
    let client = reqwest::Client::new();

    // Check the size of the already downloaded part, if any.
    let downloaded_size = if std::path::Path::new(&record.path).exists() {
      fs::metadata(&record.path)
          .map(|metadata| metadata.len())
          .unwrap_or(0)
    } else {
      0
    };

    // Set the Range header for resuming the download.
    let mut headers = HeaderMap::new();
    if downloaded_size > 0 {
      headers.insert(RANGE, format!("bytes={}-", downloaded_size).parse().unwrap());
    }

    // Send the request.
    let response = match client.get(&record.url).headers(headers).send().await {
      Ok(res) => res,
      Err(e) => {
          return Err(Error::HttpError(format!("Failed to send request: {}", e)));
      },
    };

    // Ensure the server supports partial downloads.
    if downloaded_size > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
      return Err(Error::HttpError("Server does not support partial downloads".to_string()));
    }

    // Get the total size of the file from headers (if available).
    let total_size = response
      .headers()
      .get("content-length")
      .and_then(|len| len.to_str().ok())
      .and_then(|len| len.parse::<u64>().ok())
      .map(|len| len + downloaded_size)
      .unwrap_or(0);

    // Ensure the output folder exists.
    let folder = Path::new(&record.path).parent().unwrap();
    if !folder.exists() {
      fs::create_dir(folder).unwrap();
    }

    // Open the file in append mode.
    let mut file = OpenOptions::new()
      .create(true)
      .append(true)
      .open(&record.path)
      .map_err(|e| Error::FileError(format!("Failed to open file: {}", e)))?;

    // Write the response body to the file in chunks.
    let mut downloaded = downloaded_size;
    let mut stream = response.bytes_stream();

    Download::update_record(&app, record.with_state(DownloadState::InProgress)).unwrap();
    Download::emit_state(&app, record.key.clone(), DownloadState::InProgress);

    'reader: while let Some(chunk) = stream.next().await {
       match chunk {
          Ok(data) => {
            file
              .write_all(&data)
              .map_err(|e| Error::FileError(format!("Failed to write file: {}", e)))?;

            downloaded += data.len() as u64; 
            let progress = (downloaded as f64 / total_size as f64) * 100.0;

            if let Ok(record) = Download::get_record(&app, record.key.clone()) {
              match record.state {
                // Download is in progress.
                DownloadState::InProgress => {
                  if progress < 100.0 {                    
                    Download::update_record(app, record.with_progress(progress)).unwrap();
                    Download::emit_progress(&app, record.key.clone(), progress);
                  }
                  else if progress == 100.0 {
                    Download::remove_record(&app, record.key.clone()).unwrap();
                    Download::emit_state(&app, record.key, DownloadState::Completed);
                  }
                },
                // Download was paused.
                DownloadState::Paused => {
                  break 'reader;
                }
                _  => (),
              }
            }
            else {
                // Download was removed.
                println!("[{}] Download was removed", &record.key);
                break 'reader;
            }
          }
          Err(e) => return Err(Error::HttpError(format!("Failed to download: {}", e))),
       }
    }

    Ok(())
  }

  fn emit_state(app: &AppHandle<R>, key: String, state: DownloadState) {
    app.emit("tauri-plugin-download:state", DownloadEvent { key: key.clone(), progress: None, state: state.clone() }).unwrap();
    println!("[{}] State: {}", key, state.to_string());
  }

  fn emit_progress(app: &AppHandle<R>, key: String, progress: f64) {
    app.emit("tauri-plugin-download:progress", DownloadEvent { key: key.clone(), progress: Some(progress), state: DownloadState::InProgress }).unwrap();
    println!("[{}] Progress: {}", key, progress.to_string());
  }

  fn update_record(app: &AppHandle<R>, record: DownloadRecord) -> crate::Result<()> {
    let store = app
      .store("downloads.json")
      .map_err(|e| Error::StoreError(format!("Failed to load store: {}", e)))?;

    store.set(&record.key, serde_json::to_value(&record).unwrap());
    store
       .save()
       .map_err(|e| Error::StoreError(format!("Failed to save store: {}", e)))?;

    Ok(())
  }

  fn create_record(app: &AppHandle<R>, record: DownloadRecord) -> crate::Result<DownloadRecord> {
    let store = app
      .store("downloads.json")
      .map_err(|e| Error::StoreError(format!("Failed to load store: {}", e)))?;

    match store.get(&record.key) {
      Some(_) => return Err(Error::StoreError(format!("Record already exists for key: {}", &record.key))),
      None => {
        store.set(&record.key, serde_json::to_value(&record).unwrap());
        store
          .save()
          .map_err(|e| Error::StoreError(format!("Failed to save store: {}", e)))?;
      }
    }

    Ok(record)
  }
  
  fn get_records(app: &AppHandle<R>) -> crate::Result<Vec<DownloadRecord>> {
    let store = app
      .store("downloads.json")
      .map_err(|e| Error::StoreError(format!("Failed to load store: {}", e)))?;

    let mut records = Vec::new();
    for key in store.keys() {
      if let Some(value) = store.get(&key) {
          let record: DownloadRecord = serde_json::from_value(value)
              .map_err(|e| Error::StoreError(format!("Failed to parse record: {}", e)))?;
          records.push(record);
      }
    }

    Ok(records)
  }

  fn get_record(app: &AppHandle<R>, key: String) -> crate::Result<DownloadRecord> {
    let store = app
      .store("downloads.json")
      .map_err(|e| Error::StoreError(format!("Failed to load store: {}", e)))?;

    match store.get(&key) {
      Some(value) => {
        Ok(serde_json::from_value(value).unwrap())
      },
      None => Err(Error::StoreError(format!("No download record found for key: {}", key))),
    }
  }
 
  fn remove_record(app: &AppHandle<R>, key: String) -> crate::Result<()> {
    let store = app
      .store("downloads.json")
      .map_err(|e| Error::StoreError(format!("Failed to load store: {}", e)))?;

    if store.has(&key) {
      store.delete(&key);
    }

    store
      .save()
      .map_err(|e| Error::StoreError(format!("Failed to save store: {}", e)))?;

    Ok(())
  }
}
