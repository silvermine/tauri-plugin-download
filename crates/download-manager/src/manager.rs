use reqwest_middleware::ClientBuilder;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::Error;
use crate::downloader;
use crate::models::*;
use crate::store::{DeleteIfStatusResult, DownloadStore, PersistMode, UpdateIfStatusResult};
use crate::validate;

type HttpClient = reqwest_middleware::ClientWithMiddleware;
type ConnectionStatusProvider =
   Arc<dyn Fn() -> connectivity::Result<connectivity::ConnectionStatus> + Send + Sync>;

pub(crate) static DOWNLOAD_SUFFIX: &str = ".download";

/// Callback invoked whenever a download item changes state.
pub type OnChanged = Arc<dyn Fn(DownloadItem) + Send + Sync + 'static>;

/// Manager-wide settings, fixed when the manager is constructed.
#[derive(Debug, Clone, Default)]
pub struct DownloadManagerConfig {
   /// User agent sent with every download request. `None` leaves the transport's
   /// own default in place.
   pub user_agent: Option<String>,
}

/// Tauri-agnostic download manager, mirroring the iOS/Android `DownloadManager`.
#[derive(Clone)]
pub struct DownloadManager {
   pub(crate) http_client: HttpClient,
   pub(crate) store: DownloadStore,
   pub(crate) on_changed: OnChanged,
   connection_status: ConnectionStatusProvider,
}

/// Capabilities available to one running download task.
///
/// Its fields are private so the downloader cannot bypass the status-aware
/// operations and write directly to the store.
pub(crate) struct ActiveDownload<'a> {
   manager: &'a DownloadManager,
   item: DownloadRecord,
}

/// Outcome of checking whether a download remains active.
pub(crate) enum Active<T> {
   /// The download remains active. Contains the capability required to continue.
   Active(T),
   /// The download was paused, canceled, removed, or otherwise no longer active.
   NoLongerActive,
}

impl DownloadManager {
   /// Creates a new `DownloadManager`, loading persisted state from disk.
   ///
   /// # Arguments
   /// - `data_dir` - Directory where `downloads.json` will be stored.
   /// - `on_changed` - Callback invoked on every state/progress change.
   /// - `config` - Manager-wide settings.
   pub fn new(data_dir: PathBuf, on_changed: OnChanged, config: DownloadManagerConfig) -> Self {
      #[cfg(target_os = "macos")]
      {
         // Start the path monitor early so its initial asynchronous update has
         // normally populated the cache before the first policy check.
         let _ = connectivity::connection_status();
      }

      Self::with_connection_status_provider(
         data_dir,
         on_changed,
         config,
         Arc::new(connectivity::connection_status),
      )
   }

   fn with_connection_status_provider(
      data_dir: PathBuf,
      on_changed: OnChanged,
      config: DownloadManagerConfig,
      connection_status: ConnectionStatusProvider,
   ) -> Self {
      let store = DownloadStore::new(data_dir.join("downloads.json"));
      if let Err(e) = store.load() {
         warn!("Failed to load download store: {}", e);
      }
      // Build client with retry middleware for transient failures.
      let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);

      let mut client_builder = reqwest::Client::builder();
      if let Some(ref user_agent) = config.user_agent {
         // Checked rather than trusted: a public constructor cannot assume its caller
         // validated, and an unusable value would panic the build below. A direct
         // consumer gets no user agent and no warning — `release_max_level_off` elides it.
         match validate::user_agent(user_agent) {
            Ok(()) => client_builder = client_builder.user_agent(user_agent.clone()),
            Err(e) => warn!("Ignoring invalid user agent: {}", e),
         }
      }

      // Mirrors `reqwest::Client::new()`, which expects on the same builder. With the
      // user agent checked above, only a TLS or resolver failure can reach this.
      let client = client_builder
         .build()
         .expect("Failed to build the download HTTP client");

      let http_client = ClientBuilder::new(client)
         .with(RetryTransientMiddleware::new_with_policy(retry_policy))
         .build();
      Self {
         http_client,
         store,
         on_changed,
         connection_status,
      }
   }

   ///
   /// Initializes the manager.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      let items = match self.store.list() {
         Ok(list) => list,
         Err(e) => {
            error!("Failed to load download store: {}", e);
            return;
         }
      };

      for item in items
         .into_iter()
         .filter(|item| item.status == DownloadStatus::InProgress)
      {
         // Revert to a recoverable state so the download can be retried.
         match self.revert_in_progress(&item.path) {
            Ok(Some(reverted)) => {
               info!(file = %filename(&reverted.path), status = %reverted.status, "Reverted download item")
            }
            Ok(None) => {}
            Err(e) => warn!(file = %filename(&item.path), "Failed to revert download item: {}", e),
         }
      }
   }

   ///
   /// Lists all download operations.
   ///
   /// # Returns
   /// The list of download operations.
   pub fn list(&self) -> crate::Result<Vec<DownloadItem>> {
      Ok(self
         .store
         .list()?
         .into_iter()
         .map(|i| i.to_item())
         .collect())
   }

   ///
   /// Gets a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation, or `None` if no download exists for the path.
   pub fn get(&self, path: &str) -> crate::Result<Option<DownloadItem>> {
      validate::path(path)?;

      Ok(self.store.find_by_path(path)?.map(|item| item.to_item()))
   }

   ///
   /// Creates a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   /// - `url` - The download URL for the resource.
   ///
   /// # Returns
   /// The download operation.
   pub fn create(&self, path: &str, url: &str) -> crate::Result<DownloadActionResponse> {
      self.create_with_options(path, url, CreateOptions::default())
   }

   /// Creates a download operation with network policy options.
   ///
   /// Existing records are returned unchanged, including their original options.
   /// Options are fixed on initial creation and cannot be updated by calling this
   /// method again.
   ///
   /// # Arguments
   /// - `path` - The download path.
   /// - `url` - The download URL for the resource.
   /// - `options` - Network policy persisted with the download.
   pub fn create_with_options(
      &self,
      path: &str,
      url: &str,
      options: CreateOptions,
   ) -> crate::Result<DownloadActionResponse> {
      validate::path(path)?;
      validate::url(url)?;

      // Check if item already exists
      if let Some(existing) = self.store.find_by_path(path)? {
         return Ok(DownloadActionResponse::with_expected_status(
            existing.to_item(),
            DownloadStatus::Idle,
         ));
      }

      let item = self.store.create(DownloadRecord {
         url: url.to_string(),
         path: path.to_string(),
         options,
         received_bytes: 0,
         total_bytes: None,
         status: DownloadStatus::Idle,
      })?;

      let event = self.emit_changed(&item);
      Ok(DownloadActionResponse::new(event))
   }

   ///
   /// Starts a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub async fn start(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be started when idle.
         DownloadStatus::Idle => {
            self.ensure_network_allowed(&item).await?;
            self.spawn_download(item, DownloadStatus::Idle, "failed to start")
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item.to_item(),
            DownloadStatus::InProgress,
         )),
      }
   }

   ///
   /// Resumes a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub async fn resume(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      validate::path(path)?;

      let item = self
         .store
         .find_by_path(path)?
         .ok_or_else(|| Error::NotFound(path.to_string()))?;
      match item.status {
         // Allow download to be resumed when paused.
         DownloadStatus::Paused => {
            self.ensure_network_allowed(&item).await?;
            self.spawn_download(item, DownloadStatus::Paused, "failed to resume")
         }

         // Return current state if in any other state.
         _ => Ok(DownloadActionResponse::with_expected_status(
            item.to_item(),
            DownloadStatus::InProgress,
         )),
      }
   }

   fn spawn_download(
      &self,
      item: DownloadRecord,
      expected_status: DownloadStatus,
      err_msg: &'static str,
   ) -> crate::Result<DownloadActionResponse> {
      let item_in_progress = match self.store.update_if_status(
         &item.path,
         expected_status,
         DownloadStatus::InProgress,
      )? {
         UpdateIfStatusResult::Updated(item) => item,
         UpdateIfStatusResult::Unchanged(current) => {
            return Ok(DownloadActionResponse::with_expected_status(
               current.to_item(),
               DownloadStatus::InProgress,
            ));
         }
         UpdateIfStatusResult::NotFound => return Err(Error::NotFound(item.path)),
      };

      let manager = self.clone();
      let path = item.path.clone();
      // Build the item without emitting — the download task will emit progress updates.
      let public_item = item_in_progress.to_item();
      tokio::spawn(async move {
         let active = ActiveDownload::new(&manager, item_in_progress);
         if let Err(e) = downloader::download(active).await {
            error!(file = %filename(&path), "Download {}: {}", err_msg, e);

            // Revert atomically unless already paused or canceled.
            match manager.revert_in_progress(&path) {
               Ok(Some(reverted)) => {
                  info!(file = %filename(&reverted.path), status = %reverted.status, "Reverted download item")
               }
               Ok(None) => {}
               Err(e) => warn!(file = %filename(&path), "Failed to revert download item: {}", e),
            }
         }
      });

      Ok(DownloadActionResponse::new(public_item))
   }

   async fn ensure_network_allowed(&self, item: &DownloadRecord) -> crate::Result<()> {
      if item.options.allow_metered {
         return Ok(());
      }

      let connection_status = self.connection_status.clone();
      let status = tokio::task::spawn_blocking(move || connection_status())
         .await
         .map_err(|error| Error::Internal(format!("Connectivity worker failed: {error}")))?
         .map_err(|error| Error::Connectivity(error.to_string()))?;

      if !status.connected {
         return Err(Error::NetworkUnavailable);
      }
      if status.metered != Some(false) || status.constrained != Some(false) {
         return Err(Error::NetworkRestricted);
      }

      Ok(())
   }

   ///
   /// Pauses a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn pause(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      validate::path(path)?;

      match self
         .store
         .update_if_status(path, DownloadStatus::InProgress, DownloadStatus::Paused)?
      {
         UpdateIfStatusResult::Updated(paused) => {
            let event = self.emit_changed(&paused);
            Ok(DownloadActionResponse::new(event))
         }
         UpdateIfStatusResult::Unchanged(item) => Ok(DownloadActionResponse::with_expected_status(
            item.to_item(),
            DownloadStatus::Paused,
         )),
         UpdateIfStatusResult::NotFound => Err(Error::NotFound(path.to_string())),
      }
   }

   ///
   /// Cancels a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn cancel(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      validate::path(path)?;

      match self.store.delete_if_status(
         path,
         &[
            DownloadStatus::Idle,
            DownloadStatus::InProgress,
            DownloadStatus::Paused,
         ],
      )? {
         DeleteIfStatusResult::Deleted(item) => {
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               debug!(file = %filename(&item.path), "Temp file was not found or could not be deleted");
            }

            let canceled = item.with_status(DownloadStatus::Canceled);
            let event = self.emit_changed(&canceled);
            Ok(DownloadActionResponse::new(event))
         }
         DeleteIfStatusResult::Unchanged(item) => Ok(DownloadActionResponse::with_expected_status(
            item.to_item(),
            DownloadStatus::Canceled,
         )),
         DeleteIfStatusResult::NotFound => Err(Error::NotFound(path.to_string())),
      }
   }

   /// Reverts an `InProgress` download record to `Paused` or `Idle` based on
   /// whether a temp file exists on disk. No-op for other statuses.
   fn revert_in_progress(&self, path: &str) -> crate::Result<Option<DownloadRecord>> {
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      let (received_bytes, status) = if let Ok(meta) = fs::metadata(&temp_path) {
         // Metadata succeeded, so the temp file exists — recover byte count from it.
         (meta.len(), DownloadStatus::Paused)
      } else {
         (0, DownloadStatus::Idle)
      };

      let reverted = self.store.revert_active(path, received_bytes, status)?;
      if let Some(reverted) = &reverted {
         self.emit_changed(reverted);
      }
      Ok(reverted)
   }

   pub(crate) fn emit_changed(&self, item: &DownloadRecord) -> DownloadItem {
      let public_item = item.to_item();
      debug!(file = %filename(&item.path), status = %item.status, received_bytes = item.received_bytes, total_bytes = ?item.total_bytes);
      (self.on_changed)(public_item.clone());
      public_item
   }
}

impl<'a> ActiveDownload<'a> {
   /// Creates a restricted view of `manager` for the running download represented by `item`.
   pub(crate) fn new(manager: &'a DownloadManager, item: DownloadRecord) -> Self {
      Self { manager, item }
   }

   /// Returns the final destination path for this download.
   pub(crate) fn path(&self) -> &str {
      &self.item.path
   }

   /// Returns the source URL for this download.
   pub(crate) fn url(&self) -> &str {
      &self.item.url
   }

   /// Returns the shared HTTP client used to fetch this download.
   pub(crate) fn http_client(&self) -> &HttpClient {
      &self.manager.http_client
   }

   /// Updates header-derived byte counts if the download is still in progress.
   /// The record is persisted only when the known total changes.
   ///
   /// Returns [`Active::Active`] when the record was updated, or
   /// [`Active::NoLongerActive`] when an external action has already paused,
   /// canceled, or otherwise ended the download.
   pub(crate) fn persist_headers(
      mut self,
      received_bytes: u64,
      total_bytes: Option<u64>,
   ) -> crate::Result<Active<Self>> {
      let persist_mode = if self.item.total_bytes == total_bytes {
         PersistMode::InMemoryOnly
      } else {
         PersistMode::ToDisk
      };
      let updated = self.manager.store.update_active_bytes(
         self.path(),
         received_bytes,
         total_bytes,
         persist_mode,
      )?;
      Ok(match updated {
         Some(updated) => {
            self.item = updated;
            Active::Active(self)
         }
         None => Active::NoLongerActive,
      })
   }

   /// Records and emits a progress checkpoint if the download is still in progress.
   ///
   /// Returns [`Active::Active`] when the checkpoint was recorded, or
   /// [`Active::NoLongerActive`] when an external action has already paused,
   /// canceled, or otherwise ended the download.
   pub(crate) fn checkpoint(
      self,
      received_bytes: u64,
      total_bytes: Option<u64>,
   ) -> crate::Result<Active<Self>> {
      let Some(updated) = self.manager.store.update_active_bytes(
         self.path(),
         received_bytes,
         total_bytes,
         PersistMode::InMemoryOnly,
      )?
      else {
         return Ok(Active::NoLongerActive);
      };
      self.manager.emit_changed(&updated);
      Ok(Active::Active(self))
   }

   /// Publishes a completed file only if the download is still active.
   pub(crate) fn finish(
      self,
      temp_path: &str,
      received_bytes: u64,
      total_bytes: Option<u64>,
   ) -> crate::Result<()> {
      let completed =
         self
            .manager
            .store
            .complete_active(self.path(), received_bytes, total_bytes, || {
               // On Windows rename does not replace an existing destination.
               #[cfg(windows)]
               if Path::new(self.path()).exists() {
                  fs::remove_file(self.path()).map_err(|e| {
                     Error::File(format!("Failed to remove existing destination file: {}", e))
                  })?;
               }

               fs::rename(temp_path, self.path()).map_err(|e| {
                  Error::File(format!("Failed to rename temp file to destination: {}", e))
               })
            })?;
      if let Some(completed) = completed {
         self.manager.emit_changed(&completed);
      }
      Ok(())
   }
}

fn filename(path: &str) -> &str {
   Path::new(path)
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or(path)
}

#[cfg(test)]
mod tests {
   use super::*;
   use connectivity::{ConnectionStatus, ConnectionType};
   use std::sync::{Barrier, Mutex};
   use std::time::Duration;
   use tempfile::TempDir;
   use wiremock::matchers::{method, path as wm_path};
   use wiremock::{Mock, MockServer, ResponseTemplate};

   const VALID_URL: &str = "https://example.com/file.mp4";
   const MOCK_BODY: &[u8] = b"manager test download";

   type EventLog = Arc<Mutex<Vec<DownloadItem>>>;

   fn make_manager() -> (DownloadManager, TempDir, EventLog) {
      make_manager_with_provider(|| Ok(ConnectionStatus::disconnected()))
   }

   fn make_manager_with_provider(
      connection_status: impl Fn() -> connectivity::Result<ConnectionStatus> + Send + Sync + 'static,
   ) -> (DownloadManager, TempDir, EventLog) {
      make_manager_with_config(DownloadManagerConfig::default(), connection_status)
   }

   fn make_manager_with_config(
      config: DownloadManagerConfig,
      connection_status: impl Fn() -> connectivity::Result<ConnectionStatus> + Send + Sync + 'static,
   ) -> (DownloadManager, TempDir, EventLog) {
      let dir = TempDir::new().unwrap();
      let events: EventLog = Arc::new(Mutex::new(Vec::new()));
      let captured = events.clone();
      let on_changed: OnChanged = Arc::new(move |event| {
         captured.lock().unwrap().push(event);
      });
      let manager = DownloadManager::with_connection_status_provider(
         dir.path().to_path_buf(),
         on_changed,
         config,
         Arc::new(connection_status),
      );
      (manager, dir, events)
   }

   fn connected_status(metered: bool, constrained: bool) -> ConnectionStatus {
      connected_status_with_policy(Some(metered), Some(constrained))
   }

   fn connected_status_with_policy(
      metered: Option<bool>,
      constrained: Option<bool>,
   ) -> ConnectionStatus {
      ConnectionStatus {
         connected: true,
         metered,
         constrained,
         connection_type: ConnectionType::Wifi,
      }
   }

   fn unexpected_connectivity_check() -> connectivity::Result<ConnectionStatus> {
      panic!("connectivity should not be checked")
   }

   async fn make_mock_download_expecting(
      dir: &TempDir,
      expected_requests: u64,
   ) -> (MockServer, String, String) {
      let server = MockServer::start().await;
      Mock::given(method("GET"))
         .and(wm_path("/file.mp4"))
         .respond_with(ResponseTemplate::new(200).set_body_bytes(MOCK_BODY.to_vec()))
         .expect(expected_requests)
         .mount(&server)
         .await;

      let path = dir.path().join("file.mp4").to_string_lossy().into_owned();
      let url = format!("{}/file.mp4", server.uri());
      (server, path, url)
   }

   async fn make_mock_download(dir: &TempDir) -> (MockServer, String, String) {
      make_mock_download_expecting(dir, 1).await
   }

   async fn verify_no_requests(server: &MockServer) {
      // `#[tokio::test]` uses a current-thread runtime, so a newly spawned
      // downloader may not be polled before an immediate verification.
      tokio::time::sleep(Duration::from_millis(100)).await;
      server.verify().await;
   }

   async fn wait_for_download(manager: &DownloadManager, path: &str) {
      tokio::time::timeout(Duration::from_secs(5), async {
         loop {
            if manager.store.find_by_path(path).unwrap().is_none() {
               break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
         }
      })
      .await
      .expect("mock download did not complete");
   }

   fn event_log(events: &EventLog) -> Vec<DownloadItem> {
      events.lock().unwrap().clone()
   }

   fn clear_events(events: &EventLog) {
      events.lock().unwrap().clear();
   }

   fn seed(manager: &DownloadManager, path: &str, status: DownloadStatus) {
      seed_with_options(manager, path, status, CreateOptions::default());
   }

   fn seed_with_options(
      manager: &DownloadManager,
      path: &str,
      status: DownloadStatus,
      options: CreateOptions,
   ) {
      seed_with_url_and_options(manager, path, VALID_URL, status, options);
   }

   fn seed_with_url_and_options(
      manager: &DownloadManager,
      path: &str,
      url: &str,
      status: DownloadStatus,
      options: CreateOptions,
   ) {
      manager
         .store
         .create(DownloadRecord {
            url: url.to_string(),
            path: path.to_string(),
            options,
            received_bytes: 0,
            total_bytes: None,
            status,
         })
         .unwrap();
   }

   #[test]
   fn test_active_download_checkpoint_respects_pause() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/checkpoint.mp4";
      seed(&manager, path, DownloadStatus::InProgress);
      let item = manager.store.find_by_path(path).unwrap().unwrap();
      let active = ActiveDownload::new(&manager, item);

      manager.pause(path).unwrap();
      clear_events(&events);

      assert!(matches!(
         active.checkpoint(500, Some(1000)).unwrap(),
         Active::NoLongerActive
      ));
      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, None);
      assert!(event_log(&events).is_empty());
   }

   #[test]
   fn test_active_download_header_persist_respects_pause() {
      let (manager, dir, _events) = make_manager();
      let path = "/tmp/headers.mp4";
      seed(&manager, path, DownloadStatus::InProgress);
      let item = manager.store.find_by_path(path).unwrap().unwrap();
      let active = ActiveDownload::new(&manager, item);

      manager.pause(path).unwrap();

      assert!(matches!(
         active.persist_headers(500, Some(1000)).unwrap(),
         Active::NoLongerActive
      ));

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      let stored = reloaded.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, None);
   }

   #[test]
   fn test_active_download_does_not_persist_unchanged_total() {
      let (manager, dir, _events) = make_manager();
      let path = "/tmp/headers.mp4";
      let item = manager
         .store
         .create(DownloadRecord {
            url: VALID_URL.to_string(),
            path: path.to_string(),
            options: CreateOptions::default(),
            received_bytes: 0,
            total_bytes: Some(1000),
            status: DownloadStatus::InProgress,
         })
         .unwrap();
      let active = ActiveDownload::new(&manager, item);

      assert!(matches!(
         active.persist_headers(500, Some(1000)).unwrap(),
         Active::Active(_)
      ));
      assert_eq!(
         manager
            .store
            .find_by_path(path)
            .unwrap()
            .unwrap()
            .received_bytes,
         500
      );

      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded.find_by_path(path).unwrap().unwrap().received_bytes,
         0
      );
   }

   #[test]
   fn test_active_download_finish_respects_pause() {
      let (manager, dir, events) = make_manager();
      let path = dir
         .path()
         .join("complete.mp4")
         .to_string_lossy()
         .into_owned();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();
      seed(&manager, &path, DownloadStatus::InProgress);
      let item = manager.store.find_by_path(&path).unwrap().unwrap();
      let active = ActiveDownload::new(&manager, item);

      manager.pause(&path).unwrap();
      clear_events(&events);
      active.finish(&temp_path, 7, Some(7)).unwrap();

      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
      assert!(Path::new(&temp_path).exists());
      assert!(!Path::new(&path).exists());
      assert!(event_log(&events).is_empty());
   }

   #[test]
   fn test_active_download_finish_publishes_active_download() {
      let (manager, dir, events) = make_manager();
      let path = dir
         .path()
         .join("complete.mp4")
         .to_string_lossy()
         .into_owned();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"complete").unwrap();
      seed(&manager, &path, DownloadStatus::InProgress);
      let item = manager.store.find_by_path(&path).unwrap().unwrap();
      let active = ActiveDownload::new(&manager, item);

      active.finish(&temp_path, 8, Some(8)).unwrap();

      assert!(manager.store.find_by_path(&path).unwrap().is_none());
      assert_eq!(fs::read(&path).unwrap(), b"complete");
      let log = event_log(&events);
      assert_eq!(log.last().unwrap().status, DownloadStatus::Completed);
      assert_eq!(log.last().unwrap().received_bytes, 8);
   }

   #[test]
   fn test_active_download_operations_respect_cancel() {
      let (manager, dir, events) = make_manager();
      let path = dir
         .path()
         .join("canceled.bin")
         .to_string_lossy()
         .into_owned();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();
      seed(&manager, &path, DownloadStatus::InProgress);
      let item = manager.store.find_by_path(&path).unwrap().unwrap();
      let headers = ActiveDownload::new(&manager, item.clone());
      let checkpoint = ActiveDownload::new(&manager, item.clone());
      let finish = ActiveDownload::new(&manager, item);

      manager.cancel(&path).unwrap();
      clear_events(&events);
      assert!(!Path::new(&temp_path).exists());

      assert!(matches!(
         headers.persist_headers(7, Some(7)).unwrap(),
         Active::NoLongerActive
      ));
      assert!(matches!(
         checkpoint.checkpoint(7, Some(7)).unwrap(),
         Active::NoLongerActive
      ));
      // A leftover file must not be published, even if cleanup could not remove it.
      fs::write(&temp_path, b"partial").unwrap();
      finish.finish(&temp_path, 7, Some(7)).unwrap();

      assert!(manager.store.find_by_path(&path).unwrap().is_none());
      assert!(!Path::new(&path).exists());
      assert_eq!(fs::read(&temp_path).unwrap(), b"partial");
      assert!(event_log(&events).is_empty());
   }

   // ---------- get ----------

   #[test]
   fn test_get_returns_none_for_unknown_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.get("/tmp/unknown.mp4").unwrap().is_none());
   }

   #[test]
   fn test_get_returns_persisted_item() {
      let (manager, _dir, _events) = make_manager();
      manager.create("/tmp/file.mp4", VALID_URL).unwrap();
      let item = manager.get("/tmp/file.mp4").unwrap().unwrap();
      assert_eq!(item.status, DownloadStatus::Idle);
      assert_eq!(item.url, VALID_URL);
      assert!(item.options.allow_metered);
   }

   #[test]
   fn test_list_returns_persisted_options() {
      let (manager, _dir, _events) = make_manager();
      let options = CreateOptions {
         allow_metered: false,
      };
      manager
         .create_with_options("/tmp/file.mp4", VALID_URL, options)
         .unwrap();

      let items = manager.list().unwrap();

      assert_eq!(items.len(), 1);
      assert_eq!(items[0].options, options);
   }

   #[test]
   fn test_get_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.get("").is_err());
   }

   // ---------- create ----------

   #[test]
   fn test_create_persists_idle_item_and_emits() {
      let (manager, _dir, events) = make_manager();
      let response = manager.create("/tmp/file.mp4", VALID_URL).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert!(response.is_expected_status);

      let stored = manager
         .store
         .find_by_path("/tmp/file.mp4")
         .unwrap()
         .unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
      assert_eq!(stored.url, VALID_URL);
      assert!(stored.options.allow_metered);

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].path, "/tmp/file.mp4");
      assert_eq!(log[0].status, DownloadStatus::Idle);
   }

   #[test]
   fn test_create_existing_does_not_overwrite_url() {
      let (manager, _dir, events) = make_manager();
      manager.create("/tmp/file.mp4", VALID_URL).unwrap();

      let other_url = "https://example.com/other.mp4";
      let response = manager.create("/tmp/file.mp4", other_url).unwrap();
      assert_eq!(response.download.url, VALID_URL);

      let stored = manager
         .store
         .find_by_path("/tmp/file.mp4")
         .unwrap()
         .unwrap();
      assert_eq!(stored.url, VALID_URL);

      // Only the first create emitted a change event.
      assert_eq!(event_log(&events).len(), 1);
   }

   #[test]
   fn test_create_with_options_persists_network_policy() {
      let (manager, _dir, _events) = make_manager();
      let options = CreateOptions {
         allow_metered: false,
      };

      manager
         .create_with_options("/tmp/file.mp4", VALID_URL, options)
         .unwrap();

      let stored = manager
         .store
         .find_by_path("/tmp/file.mp4")
         .unwrap()
         .unwrap();
      assert_eq!(stored.options, options);
   }

   #[test]
   fn test_create_existing_does_not_overwrite_options() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      let restricted = CreateOptions {
         allow_metered: false,
      };
      manager
         .create_with_options(path, VALID_URL, restricted)
         .unwrap();

      let response = manager.create(path, VALID_URL).unwrap();

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.options, restricted);
      assert_eq!(response.download.options, restricted);
   }

   #[test]
   fn test_create_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.create("", VALID_URL).is_err());
   }

   #[test]
   fn test_create_rejects_invalid_url() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.create("/tmp/file.mp4", "not-a-url").is_err());
   }

   // ---------- start ----------

   #[tokio::test]
   async fn test_start_http_failure_reverts_to_idle() {
      let (manager, dir, events) = make_manager();
      let server = MockServer::start().await;
      Mock::given(method("GET"))
         .and(wm_path("/missing"))
         .respond_with(ResponseTemplate::new(404))
         .expect(1)
         .mount(&server)
         .await;
      let path = dir
         .path()
         .join("missing.bin")
         .to_string_lossy()
         .into_owned();
      manager
         .create(&path, &format!("{}/missing", server.uri()))
         .unwrap();
      clear_events(&events);

      let response = manager.start(&path).await.unwrap();
      assert_eq!(response.download.status, DownloadStatus::InProgress);
      tokio::time::timeout(Duration::from_secs(5), async {
         loop {
            if event_log(&events)
               .iter()
               .any(|event| event.status == DownloadStatus::Idle)
            {
               break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
         }
      })
      .await
      .expect("failed download did not emit recovery");

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
      assert_eq!(stored.received_bytes, 0);
      assert!(!Path::new(&path).exists());
      assert!(!Path::new(&format!("{}{}", path, DOWNLOAD_SUFFIX)).exists());
      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].path, path);
      assert_eq!(log[0].status, DownloadStatus::Idle);
      assert_eq!(log[0].received_bytes, 0);
      let reloaded = DownloadStore::new(dir.path().join("downloads.json"));
      reloaded.load().unwrap();
      assert_eq!(
         reloaded.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      server.verify().await;
   }

   #[tokio::test]
   async fn test_start_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.start("/tmp/unknown.mp4").await,
         Err(Error::NotFound(_))
      ));
   }

   #[tokio::test]
   async fn test_start_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.start("").await.is_err());
   }

   #[tokio::test]
   async fn test_start_from_non_idle_does_not_change_state() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.start(path).await.unwrap();
      assert_eq!(response.download.status, DownloadStatus::InProgress);
      assert_eq!(response.expected_status, DownloadStatus::InProgress);
      assert!(response.is_expected_status);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::InProgress);
   }

   #[tokio::test]
   async fn test_start_unrestricted_skips_connectivity_check() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download(&dir).await;
      manager.create(&path, &url).unwrap();

      let response = manager.start(&path).await.unwrap();

      assert_eq!(response.download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_start_restricted_allows_unmetered_connection() {
      let (manager, dir, _events) =
         make_manager_with_provider(|| Ok(connected_status(false, false)));
      let (server, path, url) = make_mock_download(&dir).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      let response = manager.start(&path).await.unwrap();

      assert_eq!(response.download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_start_restricted_concurrent_calls_spawn_once() {
      let checks_ready = Arc::new(Barrier::new(2));
      let provider_checks_ready = checks_ready.clone();
      let (manager, dir, _events) = make_manager_with_provider(move || {
         provider_checks_ready.wait();
         Ok(connected_status(false, false))
      });
      let (server, path, url) = make_mock_download(&dir).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      let (first, second) = tokio::join!(manager.start(&path), manager.start(&path));

      assert_eq!(first.unwrap().download.status, DownloadStatus::InProgress);
      assert_eq!(second.unwrap().download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_start_restricted_canceled_during_check_does_not_spawn() {
      let check_entered = Arc::new(Barrier::new(2));
      let release_check = Arc::new(Barrier::new(2));
      let provider_check_entered = check_entered.clone();
      let provider_release_check = release_check.clone();
      let (manager, dir, _events) = make_manager_with_provider(move || {
         provider_check_entered.wait();
         provider_release_check.wait();
         Ok(connected_status(false, false))
      });
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      let cancel_manager = manager.clone();
      let cancel_path = path.clone();
      let wait_for_check = check_entered.clone();
      let cancel = async move {
         tokio::task::spawn_blocking(move || wait_for_check.wait())
            .await
            .unwrap();
         let response = cancel_manager.cancel(&cancel_path).unwrap();
         release_check.wait();
         response
      };
      let (start_result, cancel_response) = tokio::join!(manager.start(&path), cancel);

      assert!(matches!(start_result, Err(Error::NotFound(_))));
      assert_eq!(cancel_response.download.status, DownloadStatus::Canceled);
      assert!(manager.store.find_by_path(&path).unwrap().is_none());
      assert!(!Path::new(&format!("{}{}", path, DOWNLOAD_SUFFIX)).exists());
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_rejects_metered_connection_without_state_change() {
      let (manager, dir, events) = make_manager_with_provider(|| Ok(connected_status(true, false)));
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();
      clear_events(&events);

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::NetworkRestricted)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      assert!(event_log(&events).is_empty());
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_rejects_constrained_connection() {
      let (manager, dir, _events) =
         make_manager_with_provider(|| Ok(connected_status(false, true)));
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::NetworkRestricted)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_rejects_unknown_metering() {
      let (manager, dir, events) =
         make_manager_with_provider(|| Ok(connected_status_with_policy(None, Some(false))));
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();
      clear_events(&events);

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::NetworkRestricted)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      assert!(event_log(&events).is_empty());
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_rejects_disconnected_network() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::NetworkUnavailable)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_propagates_connectivity_error() {
      let (manager, dir, _events) = make_manager_with_provider(|| {
         Err(connectivity::Error::DetectionFailed {
            message: "backend unavailable".to_string(),
            code: None,
         })
      });
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::Connectivity(_))
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_restricted_fails_closed_when_connectivity_worker_panics() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      manager
         .create_with_options(
            &path,
            &url,
            CreateOptions {
               allow_metered: false,
            },
         )
         .unwrap();

      assert!(matches!(
         manager.start(&path).await,
         Err(Error::Internal(_))
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_start_from_non_idle_skips_connectivity_check() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::InProgress,
         CreateOptions {
            allow_metered: false,
         },
      );

      let response = manager.start(&path).await.unwrap();

      assert_eq!(response.download.status, DownloadStatus::InProgress);
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::InProgress
      );
      verify_no_requests(&server).await;
   }

   // ---------- resume ----------

   #[tokio::test]
   async fn test_resume_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.resume("/tmp/unknown.mp4").await,
         Err(Error::NotFound(_))
      ));
   }

   #[tokio::test]
   async fn test_resume_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.resume("").await.is_err());
   }

   #[tokio::test]
   async fn test_resume_from_non_paused_does_not_change_state() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Idle,
         CreateOptions {
            allow_metered: false,
         },
      );

      let response = manager.resume(&path).await.unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert_eq!(response.expected_status, DownloadStatus::InProgress);
      assert!(!response.is_expected_status);

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_resume_unrestricted_skips_connectivity_check() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions::default(),
      );

      let response = manager.resume(&path).await.unwrap();

      assert_eq!(response.download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_resume_restricted_allows_unmetered_connection() {
      let (manager, dir, _events) =
         make_manager_with_provider(|| Ok(connected_status(false, false)));
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions {
            allow_metered: false,
         },
      );

      let response = manager.resume(&path).await.unwrap();

      assert_eq!(response.download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_resume_restricted_concurrent_calls_spawn_once() {
      let checks_ready = Arc::new(Barrier::new(2));
      let provider_checks_ready = checks_ready.clone();
      let (manager, dir, _events) = make_manager_with_provider(move || {
         provider_checks_ready.wait();
         Ok(connected_status(false, false))
      });
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions {
            allow_metered: false,
         },
      );

      let (first, second) = tokio::join!(manager.resume(&path), manager.resume(&path));

      assert_eq!(first.unwrap().download.status, DownloadStatus::InProgress);
      assert_eq!(second.unwrap().download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_resume_restricted_rejects_metered_connection_without_state_change() {
      let (manager, dir, events) = make_manager_with_provider(|| Ok(connected_status(true, false)));
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions {
            allow_metered: false,
         },
      );

      assert!(matches!(
         manager.resume(&path).await,
         Err(Error::NetworkRestricted)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
      assert!(event_log(&events).is_empty());
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_resume_restricted_rejects_unknown_constraint() {
      let (manager, dir, events) =
         make_manager_with_provider(|| Ok(connected_status_with_policy(Some(false), None)));
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions {
            allow_metered: false,
         },
      );

      assert!(matches!(
         manager.resume(&path).await,
         Err(Error::NetworkRestricted)
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
      assert!(event_log(&events).is_empty());
      verify_no_requests(&server).await;
   }

   #[tokio::test]
   async fn test_resume_restricted_fails_closed_when_connectivity_worker_panics() {
      let (manager, dir, _events) = make_manager_with_provider(unexpected_connectivity_check);
      let (server, path, url) = make_mock_download_expecting(&dir, 0).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions {
            allow_metered: false,
         },
      );

      assert!(matches!(
         manager.resume(&path).await,
         Err(Error::Internal(_))
      ));
      assert_eq!(
         manager.store.find_by_path(&path).unwrap().unwrap().status,
         DownloadStatus::Paused
      );
      verify_no_requests(&server).await;
   }

   // ---------- pause ----------

   #[test]
   fn test_pause_from_in_progress_updates_and_emits() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.pause(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Paused);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].status, DownloadStatus::Paused);
   }

   #[test]
   fn test_pause_from_non_in_progress_is_no_op() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Idle);

      let response = manager.pause(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Idle);
      assert_eq!(response.expected_status, DownloadStatus::Paused);
      assert!(!response.is_expected_status);

      let stored = manager.store.find_by_path(path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);

      assert!(event_log(&events).is_empty());
   }

   #[test]
   fn test_pause_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.pause("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_pause_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.pause("").is_err());
   }

   // ---------- cancel ----------

   #[test]
   fn test_cancel_idle_removes_and_emits_canceled() {
      let (manager, _dir, events) = make_manager();
      let path = "/tmp/file.mp4";
      manager.create(path, VALID_URL).unwrap();
      clear_events(&events);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);

      assert!(manager.store.find_by_path(path).unwrap().is_none());

      let log = event_log(&events);
      assert_eq!(log.len(), 1);
      assert_eq!(log[0].status, DownloadStatus::Canceled);
   }

   #[test]
   fn test_cancel_in_progress_removes_and_emits_canceled() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::InProgress);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);
      assert!(manager.store.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_cancel_paused_removes_and_emits_canceled() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Paused);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);
      assert!(manager.store.find_by_path(path).unwrap().is_none());
   }

   #[test]
   fn test_cancel_removes_temp_file_when_present() {
      let (manager, dir, _events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();

      seed(&manager, &path, DownloadStatus::Paused);
      manager.cancel(&path).unwrap();

      assert!(!Path::new(&temp_path).exists());
   }

   #[test]
   fn test_cancel_handles_missing_temp_file_gracefully() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Idle);
      // No temp file written; cancel should still succeed.
      assert!(manager.cancel(path).is_ok());
   }

   #[test]
   fn test_cancel_from_terminal_status_does_not_remove() {
      let (manager, _dir, _events) = make_manager();
      let path = "/tmp/file.mp4";
      seed(&manager, path, DownloadStatus::Completed);

      let response = manager.cancel(path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Completed);
      assert_eq!(response.expected_status, DownloadStatus::Canceled);
      assert!(!response.is_expected_status);
      assert!(manager.store.find_by_path(path).unwrap().is_some());
   }

   #[test]
   fn test_cancel_unknown_path_returns_not_found() {
      let (manager, _dir, _events) = make_manager();
      assert!(matches!(
         manager.cancel("/tmp/unknown.mp4"),
         Err(Error::NotFound(_))
      ));
   }

   #[test]
   fn test_cancel_rejects_invalid_path() {
      let (manager, _dir, _events) = make_manager();
      assert!(manager.cancel("").is_err());
   }

   // ---------- init / revert_in_progress ----------

   #[test]
   fn test_init_reverts_in_progress_with_temp_file_to_paused() {
      let (manager, dir, events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      let temp_path = format!("{}{}", path, DOWNLOAD_SUFFIX);
      fs::write(&temp_path, b"partial").unwrap();
      seed(&manager, &path, DownloadStatus::InProgress);

      manager.init();

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Paused);
      assert_eq!(stored.received_bytes, b"partial".len() as u64);

      assert!(
         event_log(&events)
            .iter()
            .any(|e| e.path == path && e.status == DownloadStatus::Paused)
      );
   }

   #[test]
   fn test_init_reverts_in_progress_without_temp_file_to_idle() {
      let (manager, dir, _events) = make_manager();
      let path = dir.path().join("file.mp4").to_string_lossy().to_string();
      seed(&manager, &path, DownloadStatus::InProgress);
      let stale = manager
         .store
         .find_by_path(&path)
         .unwrap()
         .unwrap()
         .with_bytes(500, Some(1000));
      manager.store.update(stale).unwrap();

      manager.init();

      let stored = manager.store.find_by_path(&path).unwrap().unwrap();
      assert_eq!(stored.status, DownloadStatus::Idle);
      assert_eq!(stored.received_bytes, 0);
      assert_eq!(stored.total_bytes, Some(1000));
   }

   #[test]
   fn test_init_leaves_non_in_progress_unchanged() {
      let (manager, _dir, _events) = make_manager();
      seed(&manager, "/tmp/a.mp4", DownloadStatus::Idle);
      seed(&manager, "/tmp/b.mp4", DownloadStatus::Paused);
      seed(&manager, "/tmp/c.mp4", DownloadStatus::Completed);

      manager.init();

      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/a.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Idle
      );
      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/b.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Paused
      );
      assert_eq!(
         manager
            .store
            .find_by_path("/tmp/c.mp4")
            .unwrap()
            .unwrap()
            .status,
         DownloadStatus::Completed
      );
   }

   // ---------- filename helper ----------

   #[test]
   fn test_filename_with_separators() {
      assert_eq!(filename("/tmp/dir/file.mp4"), "file.mp4");
   }

   #[test]
   fn test_filename_without_separators() {
      assert_eq!(filename("file.mp4"), "file.mp4");
   }

   #[test]
   fn test_filename_falls_back_for_empty_input() {
      assert_eq!(filename(""), "");
   }
}
