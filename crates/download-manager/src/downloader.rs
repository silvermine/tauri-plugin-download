use futures::StreamExt;
use reqwest::header::{HeaderMap, RANGE};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::Error;
use crate::manager::{Active, ActiveDownload, DOWNLOAD_SUFFIX};
#[cfg(test)]
use crate::models::*;

/// Performs the actual HTTP download with resume support.
///
/// This function handles:
/// - HTTP client setup and request sending
/// - Resume logic via Range headers
/// - Streaming response chunks to disk
/// - Progress tracking and throttling
/// - State updates and event emission
pub(crate) async fn download(active: ActiveDownload<'_>) -> crate::Result<()> {
   download_with_header_hook(active, || {}).await
}

async fn download_with_header_hook(
   mut active: ActiveDownload<'_>,
   before_header_persist: impl FnOnce(),
) -> crate::Result<()> {
   // Check the size of the already downloaded part, if any.
   let temp_path = format!("{}{}", active.path(), DOWNLOAD_SUFFIX);
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

   // Race shutdown against the request so pause/resume cannot be held up by a
   // stalled server or retry delay. Cancellation is normal exit, not an error.
   let request = active
      .http_client()
      .get(active.url())
      .headers(headers)
      .send();
   let response = match tokio::select! {
      () = active.cancelled() => return Ok(()),
      response = request => response,
   } {
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

   // A 200 (rather than 206) response to a Range request means the server didn't
   // honor the range for this request, so resuming isn't possible. Discard the
   // existing temp file and restart from zero rather than failing — mirrors the
   // Kotlin fallback for transient server-config blips.
   if downloaded_size > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
      tracing::warn!(
         file = %active.path(),
         "Range not honored (got 200, not 206); restarting download from zero"
      );
      if Path::new(&temp_path).exists() {
         fs::remove_file(&temp_path)
            .map_err(|e| Error::File(format!("Failed to delete stale temp file: {}", e)))?;
      }
      downloaded_size = 0;
   }

   // Get the total size of the file from headers (if available).
   let total_size = response.content_length().map(|len| len + downloaded_size);

   // Persist a known total immediately. Progress updates deliberately avoid disk
   // writes, but the total must survive an abrupt process exit so init() can
   // combine it with the recovered temp-file length.
   if total_size.is_some() {
      before_header_persist();
      active = match active.persist_headers(downloaded_size, total_size)? {
         Active::Active(active) => active,
         Active::NoLongerActive => return Ok(()),
      };
   }

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
   let mut stream = response.bytes_stream();
   let mut progress = ProgressTracker::new(downloaded_size, total_size);

   loop {
      // Check shutdown even when the server stops delivering body chunks.
      let chunk = tokio::select! {
         () = active.cancelled() => return Ok(()),
         chunk = stream.next() => chunk,
      };
      let Some(chunk) = chunk else { break };
      match chunk {
         Ok(data) => {
            file
               .write_all(&data)
               .map_err(|e| Error::File(format!("Failed to write file: {}", e)))?;

            progress.advance(data.len() as u64);

            if !progress.should_emit() {
               continue;
            }

            progress.mark_emitted();
            if !progress.is_complete() {
               active = match active.checkpoint(progress.received_bytes, total_size)? {
                  Active::Active(active) => active,
                  Active::NoLongerActive => return Ok(()),
               };
            }
         }
         Err(e) => {
            return Err(Error::Http(format!("Failed to download: {}", e)));
         }
      }
   }

   // Download stream ended naturally. The status check, rename, and store
   // removal are one synchronized operation.
   active.finish(&temp_path, progress.received_bytes, total_size)?;

   Ok(())
}

/// Tracks download progress and controls when emissions are sent.
///
/// - Known size (`total_bytes` is `Some`): emits when the integer percent (0–100) changes.
/// - Unknown size (`None`): emits every `BYTES_THRESHOLD` bytes.
///
/// Tracks `received_bytes` internally so the download loop only needs to call
/// `advance()` with each chunk size and check `should_emit()`.
struct ProgressTracker {
   received_bytes: u64,
   total_bytes: Option<u64>,
   last_emitted_percent: u64,
   last_emitted_bytes: u64,
}

impl ProgressTracker {
   const BYTES_THRESHOLD: u64 = 1024 * 1024;

   fn new(received_bytes: u64, total_bytes: Option<u64>) -> Self {
      let last_emitted_percent = match total_bytes {
         Some(total) if total > 0 => received_bytes * 100 / total,
         _ => 0,
      };
      Self {
         received_bytes,
         total_bytes,
         last_emitted_percent,
         last_emitted_bytes: received_bytes,
      }
   }

   fn advance(&mut self, bytes_written: u64) {
      self.received_bytes += bytes_written;
   }

   fn should_emit(&self) -> bool {
      match self.total_bytes {
         Some(total) if total > 0 => {
            let percent = self.received_bytes * 100 / total;
            percent >= 100 || percent > self.last_emitted_percent
         }
         _ => self.received_bytes - self.last_emitted_bytes >= Self::BYTES_THRESHOLD,
      }
   }

   fn is_complete(&self) -> bool {
      matches!(self.total_bytes, Some(total) if self.received_bytes >= total)
   }

   fn mark_emitted(&mut self) {
      if let Some(total) = self.total_bytes
         && total > 0
      {
         self.last_emitted_percent = self.received_bytes * 100 / total;
      }
      self.last_emitted_bytes = self.received_bytes;
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::manager::{DownloadManager, DownloadManagerConfig, OnChanged};
   use crate::store::DownloadStore;
   use std::sync::{Arc, Mutex};
   use tempfile::TempDir;
   use wiremock::matchers::{header, method, path as wm_path};
   use wiremock::{Mock, MockServer, ResponseTemplate};

   async fn run_download(manager: &DownloadManager, item: DownloadRecord) -> crate::Result<()> {
      download(ActiveDownload::new(manager, item)).await
   }

   type EventLog = Arc<Mutex<Vec<DownloadItem>>>;

   struct TestFixture {
      manager: DownloadManager,
      events: EventLog,
      _dir: TempDir,
   }

   fn make_fixture() -> TestFixture {
      make_fixture_with_config(DownloadManagerConfig::default())
   }

   fn make_fixture_with_config(config: DownloadManagerConfig) -> TestFixture {
      let dir = TempDir::new().unwrap();
      let events: EventLog = Arc::new(Mutex::new(Vec::new()));
      let captured = events.clone();
      let on_changed: OnChanged = Arc::new(move |event| {
         captured.lock().unwrap().push(event);
      });
      let manager = DownloadManager::new(dir.path().to_path_buf(), on_changed, config);
      TestFixture {
         manager,
         events,
         _dir: dir,
      }
   }

   /// Seeds an `InProgress` record in the store and returns a `DownloadRecord`
   /// suitable for passing to `download()`.
   fn seed_in_progress(manager: &DownloadManager, dest_path: &str, url: &str) -> DownloadRecord {
      let item = DownloadRecord {
         url: url.to_string(),
         path: dest_path.to_string(),
         options: CreateOptions::default(),
         received_bytes: 0,
         total_bytes: None,
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

      run_download(&fixture.manager, item).await.unwrap();

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

      let completed = fixture
         .events
         .lock()
         .unwrap()
         .iter()
         .find(|e| e.status == DownloadStatus::Completed)
         .cloned()
         .unwrap();
      assert_eq!(completed.received_bytes, body.len() as u64);
      assert_eq!(completed.total_bytes, Some(body.len() as u64));
   }

   #[tokio::test]
   async fn test_pause_before_header_persist_stops_before_writing_body() {
      let fixture = make_fixture();
      let server = MockServer::start().await;
      let body = b"body must not be written".to_vec();

      Mock::given(method("GET"))
         .and(wm_path("/pause-at-headers"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "pause-at-headers.bin");
      let url = format!("{}/pause-at-headers", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      download_with_header_hook(ActiveDownload::new(&fixture.manager, item), || {
         fixture.manager.pause(&dest).unwrap();
      })
      .await
      .unwrap();

      let stored = fixture.manager.store.find_by_path(&dest).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, None);

      let reloaded = DownloadStore::new(fixture._dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let persisted = reloaded.find_by_path(&dest).unwrap().unwrap();
      assert_eq!(persisted.status, DownloadStatus::Paused);
      assert_eq!(persisted.received_bytes, 0);
      assert_eq!(persisted.total_bytes, None);

      assert!(!Path::new(&dest).exists());
      assert!(!Path::new(&format!("{}{}", dest, DOWNLOAD_SUFFIX)).exists());
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         0
      );
   }

   #[tokio::test]
   async fn test_completes_without_content_length() {
      // Regression: when the server omits Content-Length, total_size is None
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

      run_download(&fixture.manager, item).await.unwrap();

      assert_eq!(fs::read(&dest).unwrap(), body);
      assert!(fixture.manager.store.find_by_path(&dest).unwrap().is_none());
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         1
      );

      let completed = fixture
         .events
         .lock()
         .unwrap()
         .iter()
         .find(|e| e.status == DownloadStatus::Completed)
         .cloned()
         .unwrap();
      assert_eq!(completed.received_bytes, body.len() as u64);
      assert_eq!(completed.total_bytes, None);
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

      run_download(&fixture.manager, item).await.unwrap();

      let combined = [first_half.as_slice(), second_half.as_slice()].concat();
      assert_eq!(fs::read(&dest).unwrap(), combined);

      let completed = fixture
         .events
         .lock()
         .unwrap()
         .iter()
         .find(|e| e.status == DownloadStatus::Completed)
         .cloned()
         .unwrap();
      assert_eq!(completed.received_bytes, combined.len() as u64);
      assert_eq!(completed.total_bytes, Some(combined.len() as u64));
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

      run_download(&fixture.manager, item).await.unwrap();

      // Final file is the full body, not partial + full.
      assert_eq!(fs::read(&dest).unwrap(), full_body);
      // Temp file has been cleaned up.
      assert!(!Path::new(&temp_path).exists());

      let completed = fixture
         .events
         .lock()
         .unwrap()
         .iter()
         .find(|e| e.status == DownloadStatus::Completed)
         .cloned()
         .unwrap();
      assert_eq!(completed.received_bytes, full_body.len() as u64);
      assert_eq!(completed.total_bytes, Some(full_body.len() as u64));
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

      let err = run_download(&fixture.manager, item).await.unwrap_err();
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

      run_download(&fixture.manager, item).await.unwrap();

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

      run_download(&fixture.manager, item).await.unwrap();

      // At least one InProgress event with received_bytes > 0 but total_bytes == None
      // (unknown size), plus the final Completed event.
      let log = fixture.events.lock().unwrap().clone();
      assert!(
         log.iter().any(|e| e.status == DownloadStatus::InProgress
            && e.received_bytes > 0
            && e.total_bytes.is_none()),
         "expected at least one InProgress event with received_bytes > 0 for unknown size"
      );
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         1
      );
   }

   #[tokio::test]
   async fn test_pause_mid_stream_stops_and_preserves_temp_file() {
      // Flipping the status to Paused while the body is still streaming must
      // stop reading, skip completion, and leave the .download temp file intact
      // so the download can later resume. The body exceeds BYTES_THRESHOLD so the
      // in-loop progress checkpoint — where the status is re-read from the store —
      // is reached while data still remains, exercising the in-loop transition.
      let dir = TempDir::new().unwrap();
      let events: EventLog = Arc::new(Mutex::new(Vec::new()));

      // The callback flips the store status to Paused on the first in-progress
      // checkpoint, simulating a concurrent `pause()`. The store is Arc-backed,
      // so this clone shares state with the store `download()` reads from. The
      // cell defers capturing the store until after the manager is constructed.
      let store_cell: Arc<Mutex<Option<DownloadStore>>> = Arc::new(Mutex::new(None));
      let captured = events.clone();
      let cell = store_cell.clone();
      let on_changed: OnChanged = Arc::new(move |event: DownloadItem| {
         captured.lock().unwrap().push(event.clone());
         if event.status == DownloadStatus::InProgress
            && let Some(store) = cell.lock().unwrap().as_ref()
         {
            // Re-read the record from the store to get a DownloadRecord for with_status
            if let Ok(Some(item)) = store.find_by_path(&event.path) {
               store
                  .update_no_persist(item.with_status(DownloadStatus::Paused))
                  .unwrap();
            }
         }
      });

      let manager = DownloadManager::new(
         dir.path().to_path_buf(),
         on_changed,
         DownloadManagerConfig::default(),
      );
      *store_cell.lock().unwrap() = Some(manager.store.clone());

      let server = MockServer::start().await;
      // Several times BYTES_THRESHOLD with unknown size, so the body is delivered
      // over multiple stream chunks and several progress checkpoints are reached.
      let body = vec![0u8; 4 * 1024 * 1024];
      Mock::given(method("GET"))
         .and(wm_path("/pause"))
         .respond_with(
            ResponseTemplate::new(200)
               .set_body_bytes(body.clone())
               .append_header("Transfer-Encoding", "chunked"),
         )
         .mount(&server)
         .await;

      let dest = dir.path().join("pause.bin").to_string_lossy().into_owned();
      let url = format!("{}/pause", server.uri());
      let item = seed_in_progress(&manager, &dest, &url);

      run_download(&manager, item).await.unwrap();

      let temp_path = format!("{}{}", dest, DOWNLOAD_SUFFIX);
      // At least one in-progress event fired before the pause took effect.
      assert!(
         events_with_status(&events, DownloadStatus::InProgress) >= 1,
         "expected at least one InProgress event before the pause"
      );
      // Stopped gracefully: temp file preserved, final file never created.
      assert!(
         Path::new(&temp_path).exists(),
         "temp file should be preserved for resume"
      );
      assert!(
         !Path::new(&dest).exists(),
         "final file should not be created when paused mid-stream"
      );
      // No completion was emitted and the store entry survives for resume.
      assert_eq!(events_with_status(&events, DownloadStatus::Completed), 0);
      assert!(manager.store.find_by_path(&dest).unwrap().is_some());
   }

   #[tokio::test]
   async fn test_rename_failure_keeps_store_entry_and_temp_file() {
      // If the final rename fails, the store entry must survive (still InProgress, so
      // the caller's error handler can revert it to a resumable state) and the temp
      // file must be left intact. The download must not silently vanish.
      let fixture = make_fixture();
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/rename-fail"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(b"complete body".to_vec()))
         .mount(&server)
         .await;

      // Make the destination an existing directory so `fs::rename` cannot replace it
      // (and so the Windows pre-delete `remove_file` also fails on a directory).
      let dest = dest_path(&fixture, "occupied.bin");
      fs::create_dir(&dest).unwrap();

      let url = format!("{}/rename-fail", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      let err = run_download(&fixture.manager, item).await.unwrap_err();
      assert!(
         matches!(err, Error::File(_)),
         "expected Error::File with context, got {:?}",
         err
      );

      // Store entry survives and is still InProgress, ready to be reverted/resumed.
      let stored = fixture.manager.store.find_by_path(&dest).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::InProgress);
      assert_eq!(stored.total_bytes, Some(b"complete body".len() as u64));

      // The total was written to disk when the response headers arrived, not only
      // retained by the in-memory progress updates.
      let reloaded = DownloadStore::new(fixture._dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded.find_by_path(&dest).unwrap().unwrap().total_bytes,
         Some(b"complete body".len() as u64)
      );
      // Temp file is left intact rather than orphaned and forgotten.
      assert!(Path::new(&format!("{}{}", dest, DOWNLOAD_SUFFIX)).exists());
      // No completion event was emitted.
      assert_eq!(
         events_with_status(&fixture.events, DownloadStatus::Completed),
         0
      );
   }

   /// Returns the `User-Agent` the mock server actually received, if any.
   ///
   /// Asserted on the request the server saw rather than on the client, because the
   /// point of the setting is what reaches the far end of the wire.
   async fn received_user_agent(server: &MockServer) -> Option<String> {
      let requests = server.received_requests().await.unwrap();
      assert_eq!(requests.len(), 1);
      requests[0]
         .headers
         .get("user-agent")
         .map(|value| value.to_str().unwrap().to_string())
   }

   #[tokio::test]
   async fn test_configured_user_agent_is_sent_with_the_request() {
      let fixture = make_fixture_with_config(DownloadManagerConfig {
         user_agent: Some("my-app/1.0".to_string()),
      });
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/file"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(b"body".to_vec()))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "file.bin");
      let url = format!("{}/file", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      run_download(&fixture.manager, item).await.unwrap();

      assert_eq!(
         received_user_agent(&server).await,
         Some("my-app/1.0".to_string())
      );
   }

   #[tokio::test]
   async fn test_no_user_agent_is_sent_when_none_is_configured() {
      // The setting is opt-in: leaving it unset must not start sending a header
      // that consumers were not sending before.
      let fixture = make_fixture();
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/file"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(b"body".to_vec()))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "file.bin");
      let url = format!("{}/file", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      run_download(&fixture.manager, item).await.unwrap();

      assert_eq!(received_user_agent(&server).await, None);
   }

   #[tokio::test]
   async fn test_an_unusable_user_agent_is_dropped_rather_than_panicking() {
      // `DownloadManager::new` is public on a Tauri-agnostic crate, so it cannot
      // assume the caller validated first the way the Tauri plugin does. An unusable
      // value is dropped with a warning instead of panicking the constructor.
      let fixture = make_fixture_with_config(DownloadManagerConfig {
         user_agent: Some("bad\nvalue".to_string()),
      });
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/file"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(b"body".to_vec()))
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "file.bin");
      let url = format!("{}/file", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      run_download(&fixture.manager, item).await.unwrap();

      assert_eq!(received_user_agent(&server).await, None);
   }

   /// A configured user agent must not displace the `Range` header the resume path
   /// sets: `.headers(...)` replaces the per-request map, not the client default.
   #[tokio::test]
   async fn test_user_agent_and_range_header_are_both_sent_on_resume() {
      let fixture = make_fixture_with_config(DownloadManagerConfig {
         user_agent: Some("my-app/1.0".to_string()),
      });
      let server = MockServer::start().await;

      Mock::given(method("GET"))
         .and(wm_path("/file"))
         .and(header("Range", "bytes=4-"))
         .and(header("user-agent", "my-app/1.0"))
         .respond_with(ResponseTemplate::new(206).set_body_bytes(b"rest".to_vec()))
         .expect(1)
         .mount(&server)
         .await;

      let dest = dest_path(&fixture, "file.bin");
      fs::write(format!("{}{}", dest, DOWNLOAD_SUFFIX), b"part").unwrap();

      let url = format!("{}/file", server.uri());
      let item = seed_in_progress(&fixture.manager, &dest, &url);

      run_download(&fixture.manager, item).await.unwrap();

      server.verify().await;
   }
}
