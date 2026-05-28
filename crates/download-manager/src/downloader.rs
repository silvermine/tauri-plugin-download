use futures::StreamExt;
use reqwest::header::{HeaderMap, RANGE};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::Error;
use crate::manager::{DOWNLOAD_SUFFIX, DownloadManager};
use crate::models::*;

/// Performs the actual HTTP download with resume support.
///
/// This function handles:
/// - HTTP client setup and request sending
/// - Resume logic via Range headers
/// - Streaming response chunks to disk
/// - Progress tracking and throttling
/// - State updates and event emission
pub(crate) async fn download(manager: &DownloadManager, item: DownloadItem) -> crate::Result<()> {
   // Build client with retry middleware for transient failures.
   let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
   let client = ClientBuilder::new(reqwest::Client::new())
      .with(RetryTransientMiddleware::new_with_policy(retry_policy))
      .build();

   // Check the size of the already downloaded part, if any.
   let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
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
         format!("bytes={}-", downloaded_size)
            .parse()
            .map_err(|e| Error::Http(format!("Invalid range header: {}", e)))?,
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
   let folder = Path::new(&temp_path)
      .parent()
      .ok_or_else(|| Error::File("File path has no parent directory".to_string()))?;
   if !folder.exists() {
      fs::create_dir_all(folder)
         .map_err(|e| Error::File(format!("Failed to create directory: {}", e)))?;
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

   // Throttle progress updates:
   // - Known size: emit when progress increases by at least 1%.
   // - Unknown size: emit every BYTES_THRESHOLD bytes.
   let mut last_emitted_progress = 0.0;
   let mut last_emitted_bytes = downloaded_size;
   const PROGRESS_THRESHOLD: f64 = 1.0;
   const BYTES_THRESHOLD: u64 = 1024 * 1024;

   while let Some(chunk) = stream.next().await {
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

            let should_throttle = if total_size > 0 {
               progress < 100.0 && progress - last_emitted_progress <= PROGRESS_THRESHOLD
            } else {
               downloaded - last_emitted_bytes < BYTES_THRESHOLD
            };
            if should_throttle {
               continue;
            }

            last_emitted_progress = progress;
            last_emitted_bytes = downloaded;
            let Ok(Some(current_item)) = manager.store.find_by_path(&item.path) else {
               // Download item was not found i.e. removed.
               return Ok(());
            };
            match current_item.status {
               // Download is in progress.
               DownloadStatus::InProgress => {
                  if progress < 100.0 {
                     // Download is not yet complete.
                     // Update item in store and emit change event.
                     manager
                        .store
                        .update_no_persist(current_item.with_progress(progress))?;
                     manager.emit_changed(current_item.with_progress(progress));
                  }
                  // Completion is handled after the loop exits naturally.
               }
               // Download was paused or removed — stop reading and exit gracefully.
               DownloadStatus::Paused => return Ok(()),
               _ => return Ok(()),
            }
         }
         Err(e) => {
            return Err(Error::Http(format!("Failed to download: {}", e)));
         }
      }
   }

   // Download stream ended naturally — rename temp file to final path and emit completion.
   if let Ok(Some(current_item)) = manager.store.find_by_path(&item.path)
      && matches!(current_item.status, DownloadStatus::InProgress)
   {
      manager.store.delete(&item.path)?;

      // On Windows `fs::rename` fails if the destination exists, so remove it first.
      // On Unix `fs::rename` replaces atomically — skipping the pre-delete preserves that.
      #[cfg(windows)]
      if Path::new(&item.path).exists() {
         fs::remove_file(&item.path)?;
      }

      fs::rename(&temp_path, &item.path)?;
      manager.emit_changed(current_item.with_status(DownloadStatus::Completed));
   }

   Ok(())
}
