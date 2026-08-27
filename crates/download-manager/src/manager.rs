use reqwest_middleware::ClientBuilder;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;
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
   /// Shares task ownership and shutdown signals across manager clones.
   tasks: TaskRegistry,
}

/// Reserves each destination for one desktop worker until its cleanup finishes.
/// Tracks runtime lifecycle only; persisted download state stays in the store.
#[derive(Clone, Default)]
struct TaskRegistry {
   /// Serializes task registration, removal, and pause/cancel transitions.
   inner: Arc<Mutex<HashMap<String, Arc<TaskControl>>>>,
}

/// Keeps registered task identities stable while a store transition signals them.
struct TaskRegistryGuard<'a> {
   registry: &'a TaskRegistry,
   /// Held only for synchronous work, never across awaits or event callbacks.
   tasks: MutexGuard<'a, HashMap<String, Arc<TaskControl>>>,
}

/// Signals for one worker, separate from the download's persisted status.
struct TaskControl {
   /// Set once to request shutdown; a later resume gets a fresh channel.
   cancel: watch::Sender<bool>,
   /// Cloned by callers waiting for this worker to release its destination.
   finished: watch::Receiver<bool>,
}

/// Worker-owned reservation; dropping it releases the path and wakes waiters.
struct TaskRegistration {
   /// Destination reserved by this worker.
   path: String,
   /// Identifies this registration independently of the reusable path.
   control: Arc<TaskControl>,
   /// Shared registry from which this reservation is removed on drop.
   registry: TaskRegistry,
   /// Signals completion after the reservation has been removed.
   finished: watch::Sender<bool>,
}

/// Capabilities available to one running download task.
///
/// Its fields are private so the downloader cannot bypass the status-aware
/// operations and write directly to the store.
pub(crate) struct ActiveDownload<'a> {
   manager: &'a DownloadManager,
   item: DownloadRecord,
   /// Shutdown signal for this worker.
   cancel: watch::Receiver<bool>,
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
      let store = DownloadStore::new(data_dir.join(crate::STORE_FILE_NAME));
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
         tasks: TaskRegistry::default(),
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
            // A canceled or failed worker may still be cleaning up this path.
            self.tasks.wait(path).await?;
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
            // Paused is persisted before the old worker releases the partial file.
            self.tasks.wait(path).await?;
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

   /// Reserves the destination and starts a worker if the expected status still holds.
   fn spawn_download(
      &self,
      item: DownloadRecord,
      expected_status: DownloadStatus,
      err_msg: &'static str,
   ) -> crate::Result<DownloadActionResponse> {
      let (registration, cancel, item_in_progress) = {
         // Serialize the store transition and reservation with pause/cancel.
         // Otherwise cancel/recreate could signal the new worker before it starts.
         let mut tasks = self.tasks.lock()?;
         if tasks.tasks.contains_key(&item.path) {
            let current = self
               .store
               .find_by_path(&item.path)?
               .ok_or_else(|| Error::NotFound(item.path.clone()))?;
            return Ok(DownloadActionResponse::with_expected_status(
               current.to_item(),
               DownloadStatus::InProgress,
            ));
         }
         // An Idle record has no resumable bytes. Discard leftovers only after
         // excluding other workers and rechecking the current status. The registry
         // lock also excludes start/resume/pause/cancel; create cannot replace an
         // existing record, so Idle remains stable through cleanup and registration.
         if expected_status == DownloadStatus::Idle
            && self
               .store
               .find_by_path(&item.path)?
               .is_some_and(|current| current.status == DownloadStatus::Idle)
         {
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if let Err(error) = fs::remove_file(&temp_path)
               && error.kind() != std::io::ErrorKind::NotFound
            {
               return Err(Error::File(format!(
                  "Failed to delete stale temp file: {error}"
               )));
            }
         }
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
         let (registration, cancel) = tasks
            .register(&item.path)
            .expect("path is unreserved while holding the registry lock");
         (registration, cancel, item_in_progress)
      };

      let manager = self.clone();
      let path = item.path.clone();
      // Build the item without emitting — the download task will emit progress updates.
      let public_item = item_in_progress.to_item();
      tokio::spawn(async move {
         // Keep the reservation through error recovery and final file cleanup.
         let _registration = registration;
         let active = ActiveDownload::new(&manager, item_in_progress, cancel);
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

         // cancel() may be unable to unlink an open file on Windows. Once the
         // downloader has released it, make one final best-effort cleanup.
         if matches!(manager.store.find_by_path(&path), Ok(None)) {
            let _ = fs::remove_file(format!("{}{}", path, DOWNLOAD_SUFFIX));
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

      let result = {
         // Keep the task identity stable from the state transition through the
         // cancellation signal. An exiting worker cannot unregister in between.
         let tasks = self.tasks.lock()?;
         let result = self.store.update_if_status(
            path,
            DownloadStatus::InProgress,
            DownloadStatus::Paused,
         )?;
         if matches!(&result, UpdateIfStatusResult::Updated(_)) {
            tasks.cancel(path);
         }
         result
      };

      match result {
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

      let result = {
         let tasks = self.tasks.lock()?;
         let result = self.store.delete_if_status(
            path,
            &[
               DownloadStatus::Idle,
               DownloadStatus::InProgress,
               DownloadStatus::Paused,
            ],
         )?;
         if let DeleteIfStatusResult::Deleted(item) = &result {
            tasks.cancel(path);
            // A replacement worker must not start before this deletion finishes.
            let temp_path = format!("{}{}", item.path, DOWNLOAD_SUFFIX);
            if fs::remove_file(&temp_path).is_err() {
               debug!(file = %filename(&item.path), "Temp file was not found or could not be deleted");
            }
         }
         result
      };

      match result {
         DeleteIfStatusResult::Deleted(item) => {
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
   /// Binds a download to the shutdown signal for its worker.
   pub(crate) fn new(
      manager: &'a DownloadManager,
      item: DownloadRecord,
      cancel: watch::Receiver<bool>,
   ) -> Self {
      Self {
         manager,
         item,
         cancel,
      }
   }

   /// Waits until pause or cancel requests cooperative task shutdown.
   pub(crate) async fn cancelled(&mut self) {
      let cancel = &mut self.cancel;
      while !*cancel.borrow() && cancel.changed().await.is_ok() {}
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

impl TaskRegistry {
   /// Locks task ownership so a store transition can signal the same worker.
   /// Acquire before the store lock; release before awaits or event callbacks.
   fn lock(&self) -> crate::Result<TaskRegistryGuard<'_>> {
      let tasks = self
         .inner
         .lock()
         .map_err(|e| Error::Internal(format!("Task registry lock poisoned: {e}")))?;
      Ok(TaskRegistryGuard {
         registry: self,
         tasks,
      })
   }

   #[cfg(test)]
   fn register(
      &self,
      path: &str,
   ) -> crate::Result<Option<(TaskRegistration, watch::Receiver<bool>)>> {
      Ok(self.lock()?.register(path))
   }

   /// Waits for the current reservation to end without holding the registry lock.
   /// Does not reserve the path: a competing caller can still register first.
   async fn wait(&self, path: &str) -> crate::Result<()> {
      let mut finished = {
         let tasks = self
            .inner
            .lock()
            .map_err(|e| Error::Internal(format!("Task registry lock poisoned: {e}")))?;
         tasks.get(path).map(|task| task.finished.clone())
      };
      if let Some(finished) = &mut finished {
         while !*finished.borrow() && finished.changed().await.is_ok() {}
      }
      Ok(())
   }
}

impl TaskRegistryGuard<'_> {
   /// Reserves a free path, returning its lifetime guard and shutdown receiver.
   /// Returns `None` while another worker still owns the path.
   fn register(&mut self, path: &str) -> Option<(TaskRegistration, watch::Receiver<bool>)> {
      if self.tasks.contains_key(path) {
         return None;
      }

      let (cancel, cancel_rx) = watch::channel(false);
      let (finished, finished_rx) = watch::channel(false);
      let control = Arc::new(TaskControl {
         cancel,
         finished: finished_rx,
      });
      self.tasks.insert(path.to_string(), control.clone());
      Some((
         TaskRegistration {
            path: path.to_string(),
            control,
            registry: self.registry.clone(),
            finished,
         },
         cancel_rx,
      ))
   }

   /// Requests shutdown without waiting; the guard prevents task replacement.
   fn cancel(&self, path: &str) {
      if let Some(task) = self.tasks.get(path) {
         let _ = task.cancel.send(true);
      }
   }
}

impl Drop for TaskRegistration {
   fn drop(&mut self) {
      let Ok(mut tasks) = self.registry.inner.lock() else {
         return;
      };
      // Path reuse must never let an old guard remove a different registration.
      if tasks
         .get(&self.path)
         .is_some_and(|current| Arc::ptr_eq(current, &self.control))
      {
         tasks.remove(&self.path);
      }
      drop(tasks);
      // Woken callers can now attempt to reserve the path themselves.
      let _ = self.finished.send(true);
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

   #[tokio::test]
   async fn test_cancel_before_worker_starts_allows_path_reuse() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      manager.create(&path, &url).unwrap();
      manager.start(&path).await.unwrap();

      // Cancel before the spawned task is polled, then reuse its path.
      manager.cancel(&path).unwrap();
      manager.create(&path, &url).unwrap();
      manager.start(&path).await.unwrap();
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_pause_stalled_body_allows_resume() {
      use tokio::io::{AsyncReadExt, AsyncWriteExt};
      use tokio::net::{TcpListener, TcpStream};

      async fn read_request(stream: &mut TcpStream) -> String {
         let mut request = Vec::new();
         while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.unwrap());
         }
         String::from_utf8(request).unwrap().to_ascii_lowercase()
      }

      let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
      let url = format!("http://{}/file", listener.local_addr().unwrap());
      let server = tokio::spawn(async move {
         let (mut stalled, _) = listener.accept().await.unwrap();
         let request = read_request(&mut stalled).await;
         assert!(!request.contains("\r\nrange:"));
         stalled
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello")
            .await
            .unwrap();

         // Keep the first body incomplete and its connection open throughout
         // resume. Only the worker's cancellation signal can unblock it.
         let (mut resumed, _) = listener.accept().await.unwrap();
         let request = read_request(&mut resumed).await;
         assert!(request.contains("\r\nrange: bytes=5-\r\n"));
         resumed
            .write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 5-9/10\r\nConnection: close\r\n\r\nworld")
            .await
            .unwrap();
         drop(stalled);
      });

      let (manager, dir, events) = make_manager();
      let path = dir.path().join("file").to_string_lossy().into_owned();
      manager.create(&path, &url).unwrap();
      manager.start(&path).await.unwrap();
      tokio::time::timeout(Duration::from_secs(5), async {
         loop {
            if event_log(&events)
               .iter()
               .any(|event| event.status == DownloadStatus::InProgress && event.received_bytes == 5)
            {
               break;
            }
            tokio::task::yield_now().await;
         }
      })
      .await
      .expect("worker did not write the first part of the body");

      manager.pause(&path).unwrap();
      // Resume immediately, while the old worker still owns its registration.
      let response = tokio::time::timeout(Duration::from_secs(5), manager.resume(&path))
         .await
         .expect("resume waited indefinitely for the stalled worker")
         .unwrap();
      assert_eq!(response.download.status, DownloadStatus::InProgress);
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), b"helloworld");
      tokio::time::timeout(Duration::from_secs(5), server)
         .await
         .expect("server did not finish")
         .unwrap();
   }

   #[tokio::test]
   async fn test_start_discards_canceled_workers_leftover_bytes() {
      let (manager, dir, _events) = make_manager();
      let server = MockServer::start().await;
      Mock::given(method("GET"))
         .respond_with(|request: &wiremock::Request| {
            // Honor a range request so the downloader's 200 fallback cannot
            // conceal an attempt to resume the canceled download's bytes.
            if request.headers.contains_key("range") {
               ResponseTemplate::new(206).set_body_bytes(b"world".to_vec())
            } else {
               ResponseTemplate::new(200).set_body_bytes(b"helloworld".to_vec())
            }
         })
         .expect(1)
         .mount(&server)
         .await;
      let path = dir.path().join("file").to_string_lossy().into_owned();
      let temp_path = format!("{path}{DOWNLOAD_SUFFIX}");
      seed(&manager, &path, DownloadStatus::InProgress);
      let (registration, _cancel) = manager.tasks.register(&path).unwrap().unwrap();
      manager.cancel(&path).unwrap();
      manager.create(&path, &server.uri()).unwrap();

      // Model an old worker recreating the temp file after cancel removed it.
      fs::write(&temp_path, b"stale").unwrap();
      let start = manager.start(&path);
      tokio::pin!(start);
      assert!(
         tokio::time::timeout(Duration::from_millis(20), &mut start)
            .await
            .is_err()
      );
      assert_eq!(fs::read(&temp_path).unwrap(), b"stale");
      drop(registration);
      start.await.unwrap();
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), b"helloworld");
      let requests = server.received_requests().await.unwrap();
      assert_eq!(requests.len(), 1);
      assert!(!requests[0].headers.contains_key("range"));
      server.verify().await;
   }

   #[tokio::test]
   async fn test_start_cleanup_failure_leaves_idle_and_can_be_retried() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      manager.create(&path, &url).unwrap();
      let temp_path = format!("{path}{DOWNLOAD_SUFFIX}");
      // A directory reliably makes remove_file fail on every platform.
      fs::create_dir(&temp_path).unwrap();
      assert!(matches!(manager.start(&path).await, Err(Error::File(_))));
      assert_eq!(
         manager.get(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      assert!(!manager.tasks.inner.lock().unwrap().contains_key(&path));
      assert!(server.received_requests().await.unwrap().is_empty());
      fs::remove_dir(&temp_path).unwrap();
      manager.start(&path).await.unwrap();
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[test]
   fn test_stale_start_preserves_active_and_paused_partial_file() {
      let (manager, dir, _events) = make_manager();
      let path = dir.path().join("file").to_string_lossy().into_owned();
      seed(&manager, &path, DownloadStatus::Idle);
      let stale_item = manager.store.find_by_path(&path).unwrap().unwrap();

      // Another start won ownership after the first caller read Idle.
      manager
         .store
         .update_if_status(&path, DownloadStatus::Idle, DownloadStatus::InProgress)
         .unwrap();
      let (registration, _cancel) = manager.tasks.register(&path).unwrap().unwrap();
      let temp_path = format!("{path}{DOWNLOAD_SUFFIX}");
      fs::write(&temp_path, b"partial").unwrap();
      let response = manager
         .spawn_download(stale_item.clone(), DownloadStatus::Idle, "failed to start")
         .unwrap();
      assert_eq!(response.download.status, DownloadStatus::InProgress);
      assert_eq!(fs::read(&temp_path).unwrap(), b"partial");

      // Even after that worker exits, a stale start must preserve paused bytes.
      manager.pause(&path).unwrap();
      drop(registration);
      let response = manager
         .spawn_download(stale_item, DownloadStatus::Idle, "failed to start")
         .unwrap();
      assert_eq!(response.download.status, DownloadStatus::Paused);
      assert_eq!(fs::read(&temp_path).unwrap(), b"partial");
   }

   async fn check_failed_shutdown_can_be_retried(cancel_download: bool) {
      let (manager, dir, events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      let before_path = format!("{path}.before");
      let after_path = format!("{path}.after");
      seed(&manager, &before_path, DownloadStatus::Idle);
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::InProgress,
         CreateOptions::default(),
      );
      seed(&manager, &after_path, DownloadStatus::Idle);
      clear_events(&events);
      let (registration, mut cancel) = manager.tasks.register(&path).unwrap().unwrap();

      // Make persisting the transition fail without relying on file permissions.
      let store_path = dir.path().join("downloads.json");
      fs::remove_file(&store_path).unwrap();
      fs::create_dir(&store_path).unwrap();
      let result = if cancel_download {
         manager.cancel(&path)
      } else {
         manager.pause(&path)
      };
      assert!(result.is_err());
      assert_eq!(
         manager.get(&path).unwrap().unwrap().status,
         DownloadStatus::InProgress
      );
      // A failed cancel must restore the middle record at its original index.
      let paths: Vec<_> = manager
         .list()
         .unwrap()
         .into_iter()
         .map(|item| item.path)
         .collect();
      assert_eq!(paths, vec![before_path, path.clone(), after_path]);
      assert!(!*cancel.borrow());
      assert!(event_log(&events).is_empty());

      // Model a worker stalled until it receives the shutdown signal.
      let worker = tokio::spawn(async move {
         cancel.wait_for(|cancelled| *cancelled).await.unwrap();
         drop(registration);
      });
      fs::remove_dir(&store_path).unwrap();
      if cancel_download {
         manager.cancel(&path).unwrap();
      } else {
         manager.pause(&path).unwrap();
      }
      tokio::time::timeout(Duration::from_secs(1), worker)
         .await
         .expect("retry did not signal the worker")
         .unwrap();

      if cancel_download {
         manager.create(&path, &url).unwrap();
         manager.start(&path).await.unwrap();
      } else {
         manager.resume(&path).await.unwrap();
      }
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_pause_save_failure_can_be_retried() {
      check_failed_shutdown_can_be_retried(false).await;
   }

   #[tokio::test]
   async fn test_cancel_save_failure_can_be_retried() {
      check_failed_shutdown_can_be_retried(true).await;
   }

   #[tokio::test]
   async fn test_start_save_failure_leaves_no_reservation() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      manager.create(&path, &url).unwrap();
      let store_path = dir.path().join("downloads.json");
      fs::remove_file(&store_path).unwrap();
      fs::create_dir(&store_path).unwrap();
      assert!(manager.start(&path).await.is_err());
      assert_eq!(
         manager.get(&path).unwrap().unwrap().status,
         DownloadStatus::Idle
      );
      assert!(!manager.tasks.inner.lock().unwrap().contains_key(&path));
      fs::remove_dir(&store_path).unwrap();
      manager.start(&path).await.unwrap();
      wait_for_download(&manager, &path).await;
      server.verify().await;
   }

   #[tokio::test]
   async fn test_task_registry_waits_for_previous_task_to_exit() {
      let registry = TaskRegistry::default();
      let (registration, _cancel) = registry.register("/tmp/file.mp4").unwrap().unwrap();

      assert!(
         tokio::time::timeout(Duration::from_millis(20), registry.wait("/tmp/file.mp4"))
            .await
            .is_err()
      );

      drop(registration);
      tokio::time::timeout(Duration::from_millis(100), registry.wait("/tmp/file.mp4"))
         .await
         .expect("task handoff did not finish")
         .unwrap();
   }

   #[test]
   fn test_task_registry_signals_cancellation() {
      let registry = TaskRegistry::default();
      let (_registration, cancel) = registry.register("/tmp/file.mp4").unwrap().unwrap();

      registry.lock().unwrap().cancel("/tmp/file.mp4");

      assert!(*cancel.borrow());
   }

   #[tokio::test]
   async fn test_pause_signals_worker_before_resume() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::InProgress,
         CreateOptions::default(),
      );

      let (registration, cancel) = manager.tasks.register(&path).unwrap().unwrap();
      let response = manager.pause(&path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Paused);
      assert!(*cancel.borrow());
      drop(registration);

      assert_eq!(
         manager.resume(&path).await.unwrap().download.status,
         DownloadStatus::InProgress
      );
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
   }

   #[tokio::test]
   async fn test_cancel_signals_worker_before_path_reuse() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::InProgress,
         CreateOptions::default(),
      );
      let temp_path = format!("{path}{DOWNLOAD_SUFFIX}");
      fs::write(&temp_path, b"old partial download").unwrap();

      let (registration, cancel) = manager.tasks.register(&path).unwrap().unwrap();
      let response = manager.cancel(&path).unwrap();
      assert_eq!(response.download.status, DownloadStatus::Canceled);
      assert!(*cancel.borrow());
      drop(registration);

      assert!(!Path::new(&temp_path).exists());
      manager.create(&path, &url).unwrap();
      assert_eq!(
         manager.start(&path).await.unwrap().download.status,
         DownloadStatus::InProgress
      );
      wait_for_download(&manager, &path).await;
      assert_eq!(fs::read(&path).unwrap(), MOCK_BODY);
      server.verify().await;
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
      let (_cancel_sender, cancel) = watch::channel(false);
      let active = ActiveDownload::new(&manager, item, cancel);

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
      let (_cancel_sender, cancel) = watch::channel(false);
      let active = ActiveDownload::new(&manager, item, cancel);

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
      let (_cancel_sender, cancel) = watch::channel(false);
      let active = ActiveDownload::new(&manager, item, cancel);

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
      let (_cancel_sender, cancel) = watch::channel(false);
      let active = ActiveDownload::new(&manager, item, cancel);

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
      let (_cancel_sender, cancel) = watch::channel(false);
      let active = ActiveDownload::new(&manager, item, cancel);

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
      let (_cancel_sender, cancel) = watch::channel(false);
      let headers = ActiveDownload::new(&manager, item.clone(), cancel.clone());
      let checkpoint = ActiveDownload::new(&manager, item.clone(), cancel.clone());
      let finish = ActiveDownload::new(&manager, item, cancel);

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
   async fn test_resume_waits_for_previous_runtime_task() {
      let (manager, dir, _events) = make_manager();
      let (server, path, url) = make_mock_download(&dir).await;
      seed_with_url_and_options(
         &manager,
         &path,
         &url,
         DownloadStatus::Paused,
         CreateOptions::default(),
      );
      let (previous_task, _cancel) = manager.tasks.register(&path).unwrap().unwrap();

      let resume_manager = manager.clone();
      let resume_path = path.clone();
      let resume = tokio::spawn(async move { resume_manager.resume(&resume_path).await });
      tokio::time::sleep(Duration::from_millis(20)).await;
      assert!(!resume.is_finished());

      drop(previous_task);
      let response = resume.await.unwrap().unwrap();
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
