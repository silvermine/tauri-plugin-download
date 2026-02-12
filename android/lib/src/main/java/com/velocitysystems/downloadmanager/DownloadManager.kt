package com.velocitysystems.downloadmanager

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
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
class DownloadManager private constructor(context: Context) {
    internal val store = DownloadStore(context)
    private val workManager = WorkManager.getInstance(context)

    private val _changed = MutableSharedFlow<DownloadItem>(extraBufferCapacity = 64)

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
        val items = store.list()
        for (item in items) {
            if (item.status == DownloadStatus.InProgress) {
                val newStatus = if (item.progress == 0.0) {
                    DownloadStatus.Idle
                } else {
                    DownloadStatus.Paused
                }

                val updated = item.withStatus(newStatus)
                store.update(updated)
                Log.i(TAG, "[${File(item.path).name}] Reconciled to $newStatus")
            }
        }
    }

    /**
     * Lists all download operations.
     *
     * @return The list of download operations.
     */
    fun list(): List<DownloadItem> = store.list()

    /**
     * Gets a download operation.
     *
     * If the download exists in the store, returns it. If not found, returns a download
     * in `Pending` state (not persisted to store). The caller can then call `create` to
     * persist it and transition to `Idle` state.
     *
     * @param path The download path.
     * @return The download operation.
     */
    fun get(path: String): DownloadItem {
        val existing = store.findByPath(path)
        if (existing != null) return existing

        return DownloadItem(
            url = "",
            path = path,
            progress = 0.0,
            status = DownloadStatus.Pending,
        )
    }

    /**
     * Creates a download operation.
     *
     * @param path The download path.
     * @param url The download URL for the resource.
     * @return The download action response.
     */
    fun create(path: String, url: String): DownloadActionResponse {
        val existing = store.findByPath(path)
        if (existing != null) {
            return DownloadActionResponse.withExpectedStatus(existing, DownloadStatus.Idle)
        }

        val item = DownloadItem(url = url, path = path)
        store.append(item)
        emitChanged(item)

        return DownloadActionResponse.new(item)
    }

    /**
     * Starts a download operation.
     *
     * @param path The download path.
     * @return The download action response.
     * @throws DownloadException if the download is not found.
     */
    fun start(path: String): DownloadActionResponse {
        val item = store.findByPath(path)
            ?: throw DownloadException.NotFound(path)

        if (item.status != DownloadStatus.Idle) {
            return DownloadActionResponse.withExpectedStatus(item, DownloadStatus.InProgress)
        }

        val updated = item.withStatus(DownloadStatus.InProgress)
        store.update(updated)
        emitChanged(updated)
        enqueueDownload(item)

        return DownloadActionResponse.new(updated)
    }

    /**
     * Resumes a download operation.
     *
     * @param path The download path.
     * @return The download action response.
     * @throws DownloadException if the download is not found.
     */
    fun resume(path: String): DownloadActionResponse {
        val item = store.findByPath(path)
            ?: throw DownloadException.NotFound(path)

        if (item.status != DownloadStatus.Paused) {
            return DownloadActionResponse.withExpectedStatus(item, DownloadStatus.InProgress)
        }

        val updated = item.withStatus(DownloadStatus.InProgress)
        store.update(updated)
        emitChanged(updated)
        enqueueDownload(item)

        return DownloadActionResponse.new(updated)
    }

    /**
     * Pauses a download operation.
     *
     * @param path The download path.
     * @return The download action response.
     * @throws DownloadException if the download is not found.
     */
    fun pause(path: String): DownloadActionResponse {
        val item = store.findByPath(path)
            ?: throw DownloadException.NotFound(path)

        if (item.status != DownloadStatus.InProgress) {
            return DownloadActionResponse.withExpectedStatus(item, DownloadStatus.Paused)
        }

        // Update status to paused — the DownloadWorker checks the store status
        // on each progress tick and will stop reading when it sees Paused.
        val updated = item.withStatus(DownloadStatus.Paused)
        store.update(updated)
        emitChanged(updated)

        // Also cancel the WorkManager work to stop the worker promptly.
        workManager.cancelUniqueWork(workName(path))

        return DownloadActionResponse.new(updated)
    }

    /**
     * Cancels a download operation.
     *
     * @param path The download path.
     * @return The download action response.
     * @throws DownloadException if the download is not found.
     */
    fun cancel(path: String): DownloadActionResponse {
        val item = store.findByPath(path)
            ?: throw DownloadException.NotFound(path)

        if (item.status != DownloadStatus.Idle &&
            item.status != DownloadStatus.InProgress &&
            item.status != DownloadStatus.Paused
        ) {
            return DownloadActionResponse.withExpectedStatus(item, DownloadStatus.Cancelled)
        }

        // Cancel the WorkManager work if running.
        workManager.cancelUniqueWork(workName(path))

        // Clean up temp file.
        val tempFile = File("${path}${DOWNLOAD_SUFFIX}")
        if (tempFile.exists()) tempFile.delete()

        // Remove from store and emit change.
        val cancelled = item.withStatus(DownloadStatus.Cancelled)
        store.remove(item)
        emitChanged(cancelled)

        return DownloadActionResponse.new(cancelled)
    }

    /**
     * Emits a download item change event.
     * Called by DownloadWorker to report progress and completion.
     */
    internal fun emitChanged(item: DownloadItem) {
        _changed.tryEmit(item)
    }

    /**
     * Enqueues a WorkManager work request for the download.
     * Uses unique work keyed by path to prevent duplicate workers.
     */
    private fun enqueueDownload(item: DownloadItem) {
        val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()

        val workRequest = OneTimeWorkRequestBuilder<DownloadWorker>()
            .setConstraints(constraints)
            .setInputData(
                workDataOf(
                    DownloadWorker.KEY_URL to item.url,
                    DownloadWorker.KEY_PATH to item.path,
                )
            )
            .addTag(WORK_TAG)
            .build()

        workManager.enqueueUniqueWork(
            workName(item.path),
            ExistingWorkPolicy.REPLACE,
            workRequest,
        )
    }

    private fun workName(path: String): String = "$WORK_TAG:$path"

    companion object {
        private const val TAG = "DownloadManager"
        private const val DOWNLOAD_SUFFIX = ".download"
        private const val WORK_TAG = "download_manager"

        @Volatile
        private var instance: DownloadManager? = null

        /**
         * Returns the singleton DownloadManager instance.
         * Must be called with an application context.
         */
        fun getInstance(context: Context): DownloadManager {
            return instance ?: synchronized(this) {
                instance ?: DownloadManager(context.applicationContext).also {
                    instance = it
                }
            }
        }
    }
}
