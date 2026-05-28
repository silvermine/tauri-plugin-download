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
   let mut downloaded_size = if Path::new(&temp_path).exists() {
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

   // Validate response status before streaming the body.
   let status = response.status();
   if !status.is_success() {
      return Err(Error::Http(format!(
         "HTTP {}: {}",
         status.as_u16(),
         status.canonical_reason().unwrap_or("Unknown")
      )));
   }

   // If the server returned 200 to a Range request, it does not support partial
   // downloads. Discard the existing temp file and restart from zero rather than
   // failing — mirrors the Kotlin fallback for transient server-config blips.
   if downloaded_size > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
      tracing::warn!(
         file = %item.path,
         "Server does not support Range; restarting download from zero"
      );
      if Path::new(&temp_path).exists() {
         fs::remove_file(&temp_path)
            .map_err(|e| Error::File(format!("Failed to delete stale temp file: {}", e)))?;
      }
      downloaded_size = 0;
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

#[cfg(test)]
mod tests {
   use super::*;
   use crate::manager::{DownloadManager, OnChanged};
   use std::sync::{Arc, Mutex};
   use tempfile::TempDir;
   use wiremock::matchers::{header, method, path as wm_path};
   use wiremock::{Mock, MockServer, ResponseTemplate};

   type EventLog = Arc<Mutex<Vec<DownloadItem>>>;

   struct TestFixture {
      manager: DownloadManager,
      events: EventLog,
      _dir: TempDir,
   }

   fn make_fixture() -> TestFixture {
      let dir = TempDir::new().unwrap();
      let events: EventLog = Arc::new(Mutex::new(Vec::new()));
      let captured = events.clone();
      let on_changed: OnChanged = Arc::new(move |item| {
         captured.lock().unwrap().push(item);
      });
      let manager = DownloadManager::new(dir.path().to_path_buf(), on_changed);
      TestFixture {
         manager,
         events,
         _dir: dir,
      }
   }

   /// Seeds an `InProgress` item in the store and returns a `DownloadItem`
   /// suitable for passing to `download()`.
   fn seed_in_progress(manager: &DownloadManager, dest_path: &str, url: &str) -> DownloadItem {
      let item = DownloadItem {
         url: url.to_string(),
         path: dest_path.to_string(),
         progress: 0.0,
         status: DownloadStatus::InProgress,
      };
      manager.store.create(item.clone()).unwrap();
      item
   }

   fn dest_path(fixture: &TestFixture, name: &str) -> String {
      fixture
         ._dir
         .path()
         .join(name)
         .to_string_lossy()
         .into_owned()
   }

   fn events_with_status(events: &EventLog, status: DownloadStatus) -> usize {
      events
         .lock()
         .unwrap()
         .iter()
         .filter(|e| e.status == status)
         .count()
   }

   #[tokio::test]
   async fn test_completes_with_content_length() {
      let fixture = make_fixture();
      let server = MockServer::start().await;
      let body = b"hello, world!".to_vec();

      Mock::given(method("GET"))
         .and(wm_path("/file"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "file.bin");
      let url = format!("{}/file", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      // Final file exists with expected bytes; temp file gone.
      assert_eq!(fs::read(&dest).unwrap(), body);
      assert!(!Path::new(&format!("{}{}", dest, DOWNLOAD_SUFFIX)).exists());

      // Store entry removed.
      assert!(fixture.manager.store.find_by_path(&dest).unwrap().is_none());

      // Exactly one Completed event; no duplicate 100% progress event.
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         1
      );
   }

   #[tokio::test]
   async fn test_completes_without_content_length() {
      // Regression: when the server omits Content-Length, total_size is 0
      // and progress stays at 0.0. Completion must still trigger when the
      // stream ends naturally.
      let fixture = make_fixture();
      let server = MockServer::start().await;
      let body = b"streamed body with no length header".to_vec();

      Mock::given(method("GET"))
         .and(wm_path("/stream"))
         .respond_with(
            ResponseTemplate::new(200)
               .set_body_bytes(body.clone())
               .append_header("Transfer-Encoding", "chunked"),
         )
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "stream.bin");
      let url = format!("{}/stream", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      assert_eq!(fs::read(&dest).unwrap(), body);
      assert!(fixture.manager.store.find_by_path(&dest).unwrap().is_none());
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         1
      );
   }

   #[tokio::test]
   async fn test_resume_appends_to_temp_file() {
      let fixture = make_fixture();
      let server = MockServer::start().await;

      // Pre-existing temp file with the first half of the body.
      let dest = dest_path(&fixture, "resume.bin");
      let temp_path = format!("{}{}", dest, DOWNLOAD_SUFFIX);
      let first_half = b"first-half-";
      let second_half = b"second-half";
      fs::write(&temp_path, first_half).unwrap();

      // Server expects a Range request and returns 206 with only the second half.
      Mock::given(method("GET"))
         .and(wm_path("/resume"))
         .and(header(
            "range",
            format!("bytes={}-", first_half.len()).as_str(),
         ))
         .respond_with(ResponseTemplate::new(206).set_body_bytes(second_half.to_vec()))
         .mount(&server)
         .await;

      let url = format!("{}/resume", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      let combined = [first_half.as_slice(), second_half.as_slice()].concat();
      assert_eq!(fs::read(&dest).unwrap(), combined);
   }

   #[tokio::test]
   async fn test_resume_restarts_from_zero_when_server_returns_200() {
      // When the server ignores the Range header and returns 200 with the
      // full body, the downloader discards the stale temp file and restarts
      // from zero rather than erroring.
      let fixture = make_fixture();
      let server = MockServer::start().await;

      let dest = dest_path(&fixture, "fallback.bin");
      let temp_path = format!("{}{}", dest, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"stale partial bytes").unwrap();

      let full_body = b"full body content";
      Mock::given(method("GET"))
         .and(wm_path("/fallback"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(full_body.to_vec()))
         .mount(&server)
         .await;

      let url = format!("{}/fallback", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      // Final file is the full body, not partial + full.
      assert_eq!(fs::read(&dest).unwrap(), full_body);
      // Temp file has been cleaned up.
      assert!(!Path::new(&temp_path).exists());
   }

   #[tokio::test]
   async fn test_http_error_returns_err_and_creates_no_file() {
      let fixture = make_fixture();
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/missing"))
         .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "missing.bin");
      let url = format!("{}/missing", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      let err = download(&fixture.manager, item).await.unwrap_err();
      match err {
         Error::Http(msg) => assert!(msg.contains("404"), "expected status in message: {}", msg),
         other => panic!("expected Error::Http, got {:?}", other),
      }

      // No file is created at the destination on HTTP error.
      assert!(!Path::new(&dest).exists());
      // No temp file is created either (we error before opening it).
      assert!(!Path::new(&format!("{}{}", dest, DOWNLOAD_SUFFIX)).exists());
   }

   #[tokio::test]
   async fn test_creates_output_folder_when_missing() {
      let fixture = make_fixture();
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/nested"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data".to_vec()))
         .mount(&server)
         .await;

      // Use a nested subdir that does not yet exist.
      let dest = fixture
         ._dir
         .path()
         .join("a/b/c/file.bin")
         .to_string_lossy()
         .into_owned();
      let url = format!("{}/nested", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      assert_eq!(fs::read(&dest).unwrap(), b"data");
   }

   #[tokio::test]
   async fn test_unknown_size_emits_progress_at_byte_threshold() {
      // Body larger than BYTES_THRESHOLD (1 MiB) ensures at least one
      // progress event fires for the unknown-size path before completion.
      let fixture = make_fixture();
      let server = MockServer::start().await;
      let body = vec![0u8; (1024 * 1024) + 1024]; // 1 MiB + 1 KiB

      Mock::given(method("GET"))
         .and(wm_path("/big"))
         .respond_with(
            ResponseTemplate::new(200)
               .set_body_bytes(body.clone())
               .append_header("Transfer-Encoding", "chunked"),
         )
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "big.bin");
      let url = format!("{}/big", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download(&fixture.manager, item).await.unwrap();

      // At least one InProgress progress event with progress == 0.0 (unknown size),
      // plus the final Completed event.
      let log = fixture.events.lock().unwrap().clone();
      assert!(
         log.iter()
            .any(|e| e.status == DownloadStatus::InProgress && e.progress == 0.0),
         "expected at least one InProgress(0.0) progress event for unknown size"
      );
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         1
      );
   }
}
