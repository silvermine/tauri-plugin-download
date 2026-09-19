package org.silvermine.downloadmanager

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import java.io.File

/**
 * A manager class responsible for handling download operations.
 * Provides functionality for downloading files, tracking download progress and handling completion events.
 *
 * Mirrors the iOS DownloadManager and Rust Download<R> API surface.
 */
class DownloadManager private constructor(context: Context, private val storeDir: File) {
   internal val store = DownloadStore(storeDir)
   private val workManager = WorkManager.getInstance(context)

   /**
    * The user agent sent with every download request, or `null` to leave OkHttp's
    * own default in place.
    *
    * Set by the Tauri plugin during setup. Volatile because it is written on the
    * main thread and read when a download is enqueued.
    */
   @Volatile
   var userAgent: String? = null

   private val _changed = MutableSharedFlow<DownloadItem>(
      extraBufferCapacity = 64,
      onBufferOverflow = BufferOverflow.DROP_OLDEST,
   )

   /**
    * A flow that emits download items whenever their state changes.
    * Mirrors the iOS `changed: AsyncStream<DownloadItem>`.
    */
   val changed: SharedFlow<DownloadItem> = _changed.asSharedFlow()

   init {
      reconcileStoreOnInit()
   }

   /**
    * Reconciles the store on initialization.
    * Updates the state of any download operations which are still marked as "In Progress".
    * This can occur if the application was terminated before a download was completed.
    * Mirrors the Rust Download.init() method.
    */
   private fun reconcileStoreOnInit() {
      val reconciled = mutableListOf<DownloadRecord>()

      for (record in store.list()) {
         val reverted = revertInProgress(record, DownloadWorker.tempFileLength(record.path)) ?: continue

         reconciled.add(reverted)
         Log.d(TAG, "[${File(record.path).name}] Reconciled to ${reverted.status}")
      }

      store.update(reconciled)
   }

   /**
    * Lists all download operations.
    *
    * @return The list of download operations.
    */
   fun list(): List<DownloadItem> = store.list().map { it.toItem() }

   /**
    * Gets a download operation.
    *
    * @param path The download path.
    * @return The download operation, or `null` if no download exists for the path.
    */
   fun get(path: String): DownloadItem? = store.findByPath(path)?.toItem()

   /**
    * Creates a download operation.
    *
    * Options are fixed on creation: an existing record is returned unchanged,
    * keeping its original options, as on desktop.
    *
    * @param path The download path.
    * @param url The download URL for the resource.
    * @param options Network policy persisted with the download.
    * @return The download action response.
    */
   @Synchronized
   fun create(
      path: String,
      url: String,
      options: CreateOptions = CreateOptions(),
   ): DownloadActionResponse {
      val existing = store.findByPath(path)
      if (existing != null) {
         return DownloadActionResponse.withExpectedStatus(existing.toItem(), DownloadStatus.Idle)
      }

      val record = DownloadRecord(url = url, path = path, options = options)
      store.append(record)

      return DownloadActionResponse.new(emitChanged(record))
   }

   /**
    * Starts a download operation.
    *
    * @param path The download path.
    * @return The download action response.
    * @throws DownloadException if the download is not found.
    */
   @Synchronized
   fun start(path: String): DownloadActionResponse {
      val record = store.findByPath(path)
         ?: throw DownloadException.NotFound(path)

      if (record.status != DownloadStatus.Idle) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.InProgress)
      }

      val updated = record.withStatus(DownloadStatus.InProgress)
      store.update(updated)
      val item = emitChanged(updated)
      enqueueDownload(record)

      return DownloadActionResponse.new(item)
   }

   /**
    * Resumes a download operation.
    *
    * @param path The download path.
    * @return The download action response.
    * @throws DownloadException if the download is not found.
    */
   @Synchronized
   fun resume(path: String): DownloadActionResponse {
      val record = store.findByPath(path)
         ?: throw DownloadException.NotFound(path)

      if (record.status != DownloadStatus.Paused) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.InProgress)
      }

      val updated = record.withStatus(DownloadStatus.InProgress)
      store.update(updated)
      val item = emitChanged(updated)
      enqueueDownload(record)

      return DownloadActionResponse.new(item)
   }

   /**
    * Pauses a download operation.
    *
    * @param path The download path.
    * @return The download action response.
    * @throws DownloadException if the download is not found.
    */
   @Synchronized
   fun pause(path: String): DownloadActionResponse {
      val record = store.findByPath(path)
         ?: throw DownloadException.NotFound(path)

      if (record.status != DownloadStatus.InProgress) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.Paused)
      }

      // Update status to paused — the DownloadWorker checks the store status
      // on each progress tick and will stop reading when it sees Paused. This
      // also persists the byte count from the last tick.
      val updated = record.withStatus(DownloadStatus.Paused)
      store.update(updated)
      val item = emitChanged(updated)

      // Also cancel the WorkManager work to stop the worker promptly.
      workManager.cancelUniqueWork(workName(path))

      return DownloadActionResponse.new(item)
   }

   /**
    * Cancels a download operation.
    *
    * @param path The download path.
    * @return The download action response.
    * @throws DownloadException if the download is not found.
    */
   @Synchronized
   fun cancel(path: String): DownloadActionResponse {
      val record = store.findByPath(path)
         ?: throw DownloadException.NotFound(path)

      if (record.status != DownloadStatus.Idle &&
         record.status != DownloadStatus.InProgress &&
         record.status != DownloadStatus.Paused
      ) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.Canceled)
      }

      // Cancel the WorkManager work if running.
      workManager.cancelUniqueWork(workName(path))

      // Clean up temp file.
      val tempFile = File("${path}${DownloadWorker.DOWNLOAD_SUFFIX}")
      if (tempFile.exists()) tempFile.delete()

      // Remove from store and emit change.
      val canceled = record.withStatus(DownloadStatus.Canceled)
      store.remove(record)

      return DownloadActionResponse.new(emitChanged(canceled))
   }

   /**
    * Emits a download change event, derived from the persisted record.
    * Called by DownloadWorker to report progress and completion.
    *
    * Returns the emitted payload for reuse in a [DownloadActionResponse].
    */
   internal fun emitChanged(record: DownloadRecord): DownloadItem {
      val item = record.toItem()
      _changed.tryEmit(item)
      return item
   }

   /**
    * Enqueues a WorkManager work request for the download.
    * Uses unique work keyed by path to prevent duplicate workers.
    */
   private fun enqueueDownload(record: DownloadRecord) {
      val workRequest = OneTimeWorkRequestBuilder<DownloadWorker>()
         .setConstraints(constraintsFor(record.options))
         .setInputData(inputDataFor(record, userAgent, storeDir))
         .addTag(WORK_TAG)
         .build()

      workManager.enqueueUniqueWork(
         workName(record.path),
         ExistingWorkPolicy.REPLACE,
         workRequest,
      )
   }

   private fun workName(path: String): String = "$WORK_TAG:$path"

   companion object {
      /**
       * Builds the work request's input data.
       *
       * The user agent and store directory are captured here rather than read by the
       * worker: WorkManager can re-run a stranded worker in a fresh process with no
       * Tauri activity, where the plugin never loads and neither has been configured.
       * Only input data survives.
       *
       * The store directory matters more than the user agent does. An unconfigured
       * worker sends the wrong `User-Agent`; one that opened the default store would
       * write a second `downloads.json` and record every byte of progress somewhere the
       * app never reads.
       *
       * Built here for the same reason as [constraintsFor].
       */
      internal fun inputDataFor(record: DownloadRecord, userAgent: String?, storeDir: File): Data =
         workDataOf(
            DownloadWorker.KEY_URL to record.url,
            DownloadWorker.KEY_PATH to record.path,
            DownloadWorker.KEY_USER_AGENT to userAgent,
            DownloadWorker.KEY_STORE_DIR to storeDir.absolutePath,
         )

      /**
       * Builds the work request's constraints from a download's network policy.
       *
       * [NetworkType.UNMETERED] holds a restricted download rather than failing it,
       * and stops then re-runs a worker whose network stops qualifying. That leaves
       * the record Paused in between (see [DownloadWorker]), so a restricted download
       * can move between Paused and InProgress without the caller asking.
       *
       * Built here rather than inline in [enqueueDownload], which needs a Context and
       * so cannot be reached from a JVM test.
       */
      internal fun constraintsFor(options: CreateOptions): Constraints =
         Constraints.Builder()
            .setRequiredNetworkType(
               if (options.allowMetered) NetworkType.CONNECTED else NetworkType.UNMETERED
            )
            .build()

      /**
       * Decides what a record left `InProgress` with no worker behind it should
       * become, or `null` when it needs no change.
       *
       * A worker can stop without clearing the status — process death, cancellation,
       * a transient error out of retries. The temp file is the only honest account of
       * what survived: only flushed bytes are resumable, and with none the download
       * restarts from scratch.
       *
       * Mirrors `Manager::revert_in_progress` in
       * `crates/download-manager/src/manager.rs`, kept pure so it can be tested
       * without WorkManager, as on iOS.
       *
       * @param record The record to reconcile.
       * @param tempFileLength The temp file's length, or `null` when it is absent.
       * @return The reverted record, or `null` when no change is needed.
       */
      internal fun revertInProgress(record: DownloadRecord, tempFileLength: Long?): DownloadRecord? {
         if (record.status != DownloadStatus.InProgress) {
            return null
         }

         return if (tempFileLength != null) {
            record.withBytes(tempFileLength).withStatus(DownloadStatus.Paused)
         } else {
            record.withBytes(0L).withStatus(DownloadStatus.Idle)
         }
      }

      private const val TAG = "DownloadManager"
      private const val WORK_TAG = "download_manager"

      @Volatile
      private var instance: DownloadManager? = null

      /**
       * Returns the singleton DownloadManager instance.
       * Must be called with an application context.
       *
       * @param context The application context.
       * @param storeDir The directory holding the download store, or `null` for the
       *    app's internal storage. Honoured only when this call is the one that builds
       *    the instance: [DownloadStore] reads its file in its own constructor and
       *    [reconcileStoreOnInit] consumes it before this returns, so a store cannot be
       *    moved afterwards.
       * @throws IllegalStateException if the instance already exists at a different
       *    directory. Thrown rather than logged, so `configure` rejects and plugin
       *    initialization aborts — a store quietly left elsewhere is the failure this
       *    parameter exists to prevent.
       */
      @JvmOverloads
      fun getInstance(context: Context, storeDir: File? = null): DownloadManager {
         val existing = instance ?: synchronized(this) {
            instance ?: DownloadManager(
               context.applicationContext,
               storeDir ?: defaultStoreDir(context.applicationContext),
            ).also {
               instance = it
            }
         }

         check(storeDir == null || storeDir == existing.storeDir) {
            "Cannot use store directory $storeDir; already built at ${existing.storeDir}"
         }

         return existing
      }

      /**
       * The store directory used when none is configured.
       *
       * Internal storage, so the store stays app-private and needs no permission.
       *
       * @param context The application context.
       * @return The default store directory.
       */
      internal fun defaultStoreDir(context: Context): File = context.filesDir
   }
}
