use futures::StreamExt;
use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use tauri::AppHandle;
use tauri::{Emitter, Runtime, plugin::PluginApi};
use tauri_plugin_http::reqwest;
use tauri_plugin_http::reqwest::header::{HeaderMap, RANGE};

use crate::Error;
use crate::{models::*, store};

static DOWNLOAD_SUFFIX: &str = ".download";

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
   /// Initializes the API.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      let items = match store::list(&self.0) {
         Ok(list) => list,
         Err(e) => {
            eprintln!("Failed to load download store: {}", e);
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
            eprintln!("[{}] Failed to update download status: {}", &item.path, e);
            continue;
         }

         println!("[{}] Found download item - {}", &item.path, new_status);
      }
   }

   ///
   /// Lists all download operations.
   ///
   /// # Arguments
   /// - `app` - The application handle.
   ///
   /// # Returns
   /// The list of download operations.
   pub fn list(&self, app: AppHandle<R>) -> crate::Result<Vec<DownloadItem>> {
      store::list(&app)
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
      match store::get(&self.0, path.clone())? {
         Some(item) => Ok(item),
         None => Ok(DownloadItem {
            url: String::new(),
            path,
            progress: 0.0,
            status: DownloadStatus::Pending,
         }),
      }
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
      app: AppHandle<R>,
      path: String,
      url: String,
   ) -> crate::Result<DownloadActionResponse> {
      // Check if item already exists
      if let Some(existing) = store::get(&app, path.clone())? {
         return Ok(DownloadActionResponse::with_expected_status(
            existing,
            DownloadStatus::Idle,
         ));
      }

      let item = store::create(
         &app,
         DownloadItem {
            url,
            path,
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
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn start(&self, app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&app, path.clone())?.ok_or(Error::NotFound(path))?;
      match item.status {
         // Allow download to be started when idle.
         DownloadStatus::Idle => {
            let item_started = item.with_status(DownloadStatus::InProgress);
            tokio::spawn(async move {
               Download::download(&app, item_started).await.unwrap();
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
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn resume(&self, app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&app, path.clone())?.ok_or(Error::NotFound(path))?;
      match item.status {
         // Allow download to be resumed when paused.
         DownloadStatus::Paused => {
            let item_resumed = item.with_status(DownloadStatus::InProgress);
            tokio::spawn(async move {
               Download::download(&app, item_resumed).await.unwrap();
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
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn pause(&self, app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&app, path.clone())?.ok_or(Error::NotFound(path))?;
      match item.status {
         // Allow download to be paused when in progress.
         DownloadStatus::InProgress => {
            store::update(&app, item.with_status(DownloadStatus::Paused)).unwrap();
            Download::emit_changed(&app, item.with_status(DownloadStatus::Paused));
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
   /// - `app` - The application handle.
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn cancel(&self, app: AppHandle<R>, path: String) -> crate::Result<DownloadActionResponse> {
      let item = store::get(&app, path.clone())?.ok_or(Error::NotFound(path))?;
      match item.status {
         // Allow download to be cancelled when created, in progress or paused.
         DownloadStatus::Idle | DownloadStatus::InProgress | DownloadStatus::Paused => {
            store::delete(&app, item.path.clone()).unwrap();
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               println!(
                  "[{}] File was not found or could not be deleted",
                  &item.path
               );
            }

            Download::emit_changed(&app, item.with_status(DownloadStatus::Cancelled));
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

   async fn download(app: &AppHandle<R>, item: DownloadItem) -> crate::Result<()> {
      let client = reqwest::Client::new();
      let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);

      // Check the size of the already downloaded part, if any.
      let downloaded_size = if std::path::Path::new(&temp_path).exists() {
         fs::metadata(&temp_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0)
      } else {
         0
      };

      // Set the Range header for resuming the download.
      let mut headers = HeaderMap::new();
      if downloaded_size > 0 {
         headers.insert(
            RANGE,
            format!("bytes={}-", downloaded_size).parse().unwrap(),
         );
      }

      // Send the request.
      let response = match client.get(&item.url).headers(headers).send().await {
         Ok(res) => res,
         Err(e) => {
            return Err(Error::Http(format!("Failed to send request: {}", e)));
         }
      };

      // Ensure the server supports partial downloads.
      if downloaded_size > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
         return Err(Error::Http(
            "Server does not support partial downloads".to_string(),
         ));
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
      let folder = Path::new(&temp_path).parent().unwrap();
      if !folder.exists() {
         fs::create_dir(folder).unwrap();
      }

      // Open the temp file in append mode.
      let mut file = OpenOptions::new()
         .create(true)
         .append(true)
         .open(&temp_path)
         .map_err(|e| Error::File(format!("Failed to open file: {}", e)))?;

      // Write the response body to the file in chunks.
      let mut downloaded = downloaded_size;
      let mut stream = response.bytes_stream();

      // Throttle progress updates.
      let mut last_emitted_progress = 0.0;
      const PROGRESS_THRESHOLD: f64 = 1.0; // Only update if progress increases by at least 1%.

      store::update(app, item.with_status(DownloadStatus::InProgress)).unwrap();
      Download::emit_changed(app, item.with_status(DownloadStatus::InProgress));

      'reader: while let Some(chunk) = stream.next().await {
         match chunk {
            Ok(data) => {
               file
                  .write_all(&data)
                  .map_err(|e| Error::File(format!("Failed to write file: {}", e)))?;

               downloaded += data.len() as u64;
               let progress = (downloaded as f64 / total_size as f64) * 100.0;
               if progress < 100.0 && progress - last_emitted_progress <= PROGRESS_THRESHOLD {
                  // Ignore any progress updates below the threshold.
                  continue;
               }

               last_emitted_progress = progress;
               if let Ok(Some(item)) = store::get(app, item.path.clone()) {
                  match item.status {
                     // Download is in progress.
                     DownloadStatus::InProgress => {
                        if progress < 100.0 {
                           // Download is not yet complete.
                           // Update item in store and emit change event.
                           store::update(app, item.with_progress(progress)).unwrap();
                           Download::emit_changed(app, item.with_progress(progress));
                        } else if progress == 100.0 {
                           // Download has completed.
                           // Remove item from store, rename temp file to final path and emit change event.
                           store::delete(app, item.path.clone()).unwrap();

                           let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
                           fs::rename(&temp_path, &item.path)?;
                           Download::emit_changed(app, item.with_status(DownloadStatus::Completed));
                        }
                     }
                     // Download was paused.
                     DownloadStatus::Paused => {
                        break 'reader;
                     }
                     _ => (),
                  }
               } else {
                  // Download item was not found i.e. removed.
                  break 'reader;
               }
            }
            Err(e) => {
               // Download error occured.
               // Remove item from store and partial download.
               store::delete(app, item.path.clone()).unwrap();
               let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
               if std::path::Path::new(&temp_path).exists() {
                  fs::remove_file(&temp_path)?;
               }

               return Err(Error::Http(format!("Failed to download: {}", e)));
            }
         }
      }

      Ok(())
   }

   fn emit_changed(app: &AppHandle<R>, item: DownloadItem) {
      app.emit("tauri-plugin-download:changed", &item).unwrap();
      println!("[{}] {} - {:.0}%", item.path, item.status, item.progress);
   }
}
