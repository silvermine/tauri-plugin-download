use futures::StreamExt;
use reqwest::header::{HeaderMap, RANGE};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use tauri::{AppHandle, Runtime};

use crate::Error;
use crate::manager::{DOWNLOAD_SUFFIX, Download};
use crate::models::*;
use crate::store;

/// Performs the actual HTTP download with resume support.
///
/// This function handles:
/// - HTTP client setup and request sending
/// - Resume logic via Range headers
/// - Streaming response chunks to disk
/// - Progress tracking and throttling
/// - State updates and event emission
pub(crate) async fn download<R: Runtime>(
   app: &AppHandle<R>,
   item: DownloadItem,
) -> crate::Result<()> {
   let client = reqwest::Client::new();
   let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);

   // Check the size of the already downloaded part, if any.
   let downloaded_size = if Path::new(&temp_path).exists() {
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
      fs::create_dir_all(folder).unwrap();
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
            let progress = if total_size > 0 {
               (downloaded as f64 / total_size as f64) * 100.0
            } else {
               0.0
            };
            if progress < 100.0 && progress - last_emitted_progress <= PROGRESS_THRESHOLD {
               // Ignore any progress updates below the threshold.
               continue;
            }

            last_emitted_progress = progress;
            if let Ok(Some(item)) = store::get(app, &item.path) {
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
                        store::delete(app, &item.path).unwrap();

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
            // Download error occurred.
            // Remove item from store and partial download.
            store::delete(app, &item.path).unwrap();
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if Path::new(&temp_path).exists() {
               fs::remove_file(&temp_path)?;
            }

            return Err(Error::Http(format!("Failed to download: {}", e)));
         }
      }
   }

   Ok(())
}
