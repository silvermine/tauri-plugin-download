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
import java.io.IOException

/**
 * A manager class responsible for handling download operations.
 * Provides functionality for downloading files, tracking download progress and handling completion events.
 *
 * Mirrors the iOS DownloadManager and Rust Download<R> API surface.
 */
class DownloadManager private constructor(context: Context, private val storeDir: File) {
   internal val store = DownloadStore(storeDir)
   private val workManager = WorkManager.getInstance(context)
   internal val workOwnership = DownloadWorkOwnership(store.list().map { it.path })

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
      reconcileStoreOnInit(store) { path ->
         // WorkManager retains queued, running and blocked work across process death.
         // Await its database query before exposing the recovered store to callers.
         val unfinished = workManager.getWorkInfosForUniqueWork(workName(path)).get().filter {
            !it.state.isFinished
         }
         // Upgrade policy: cancel all pre-tag requests, including ones that already
         // carry a store directory. Otherwise they could resume automatically after
         // we report Paused/Idle. Keep the partial file for an explicit resume/start,
         // which enqueues a tagged replacement. Await cancellation before recovery.
         for (work in unfinished.filter { STORE_TAG !in it.tags }) {
            workManager.cancelWorkById(work.id).result.get()
         }
         unfinished.any { STORE_TAG in it.tags }
      }
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

      // A canceled record may have been removed before its partial was deleted.
      // A new URL must never resume those orphaned bytes. Fail before persistence
      // if cleanup fails, so a later start cannot reuse the stale content.
      val partial = File("${path}${DownloadWorker.DOWNLOAD_SUFFIX}")
      if (partial.exists() && !partial.delete()) {
         throw IOException("Cannot remove stale partial file: ${partial.path}")
      }

      val record = DownloadRecord(url = url, path = path, options = options)
      store.append(record)
      workOwnership.revoke(path)

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
      enqueueDownload(record)

      return DownloadActionResponse.new(emitChanged(updated))
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

      if (record.status != DownloadStatus.Paused && record.status != DownloadStatus.Failed) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.InProgress)
      }

      val updated = record.withStatus(DownloadStatus.InProgress)
      store.update(updated)
      enqueueDownload(record)

      return DownloadActionResponse.new(emitChanged(updated))
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

      // Persist the pause before revoking the worker. A failed write leaves the
      // active download untouched. This also saves the last reported byte count.
      val updated = record.withStatus(DownloadStatus.Paused)
      store.update(updated)

      // Revoke ownership synchronously: WorkManager cancellation is asynchronous.
      workOwnership.revoke(path)

      // Also cancel the WorkManager work to stop the worker promptly.
      workManager.cancelUniqueWork(workName(path))

      return DownloadActionResponse.new(emitChanged(updated))
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
         record.status != DownloadStatus.Paused &&
         record.status != DownloadStatus.Failed
      ) {
         return DownloadActionResponse.withExpectedStatus(record.toItem(), DownloadStatus.Canceled)
      }

      // Commit the removal first: if persistence fails, keep the record, partial,
      // and worker intact. All subsequent effects remain under the same monitor.
      store.remove(record)

      // Old workers must not touch a later download created at the same path.
      workOwnership.revoke(path)

      // Cancel the WorkManager work if running.
      workManager.cancelUniqueWork(workName(path))

      // Clean up temp file.
      val tempFile = File("${path}${DownloadWorker.DOWNLOAD_SUFFIX}")
      if (tempFile.exists()) tempFile.delete()

      // Emit only after the cancellation has committed and cleanup has run.
      val canceled = record.withStatus(DownloadStatus.Canceled)

      return DownloadActionResponse.new(emitChanged(canceled))
   }

   /**
    * Emits a download change event, derived from the persisted record.
    * Called by DownloadWorker to report progress and completion.
    *
    * Publish after finishing ownership and scheduling changes: a collector may
    * synchronously call back into the manager to pause, resume, or replace work.
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
         .addTag(STORE_TAG)
         .build()

      workOwnership.replace(record.path, workRequest.id)
      workManager.enqueueUniqueWork(
         workName(record.path),
         ExistingWorkPolicy.REPLACE,
         workRequest,
      )
   }

   private fun workName(path: String): String = "$WORK_TAG:$path"

   companion object {
      /**
       * Reconciles the store on initialization.
       * Reverts InProgress records only when WorkManager has no compatible unfinished work
       * for their path. Pending work survives process death and can run again without
       * a resume command, including while waiting for constraints or retry backoff.
       * Records without work recover from the partial file, as on desktop.
       * Preserved records keep their status and refresh their bytes from the partial file.
       *
       * @param store The download store to reconcile.
       * @param hasUnfinishedWork Whether the path has unfinished work tagged as carrying
       *    the store directory required by the current worker.
       */
      internal fun reconcileStoreOnInit(store: DownloadStore, hasUnfinishedWork: (String) -> Boolean) {
         val reconciled = mutableListOf<DownloadRecord>()

         for (record in store.list()) {
            if (record.status != DownloadStatus.InProgress) continue
            val updated = reconcileRecordOnInit(
               record, DownloadWorker.tempFileLength(record.path), hasUnfinishedWork(record.path),
            ) ?: continue

            reconciled.add(updated)
            Log.d(TAG, "[${File(record.path).name}] Reconciled to ${updated.status}")
         }

         try {
            store.update(reconciled)
         } catch (error: DownloadException.Store) {
            // Batch command updates roll back on failure, but startup recovery must
            // remain available in memory and be persisted by the next successful write.
            store.update(reconciled, persist = false)
            Log.e(TAG, "Failed to persist reconciled downloads", error)
         }
      }

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
       * and stops then re-runs a worker whose network stops qualifying. The record
       * remains InProgress while waiting; explicit pause and cancel revoke its work.
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
       * Reconciles a startup record with its partial file and compatible pending work.
       * Kept pure so the startup decision can be covered by JVM tests.
       *
       * @param record The record to reconcile.
       * @param tempFileLength The temp file's length, or `null` when it is absent.
       * @param hasUnfinishedWork Whether compatible unfinished work exists for the record.
       * @return The updated record, or `null` when no change is needed.
       */
      internal fun reconcileRecordOnInit(
         record: DownloadRecord,
         tempFileLength: Long?,
         hasUnfinishedWork: Boolean,
      ): DownloadRecord? {
         if (record.status != DownloadStatus.InProgress) return null
         if (!hasUnfinishedWork) return revertInProgress(record, tempFileLength)

         return record.withBytes(tempFileLength ?: 0L).takeIf { it != record }
      }

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
      // WorkInfo exposes tags, but not input data. This marks requests that carry KEY_STORE_DIR.
      internal const val STORE_TAG = "download_manager:store_dir"

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
