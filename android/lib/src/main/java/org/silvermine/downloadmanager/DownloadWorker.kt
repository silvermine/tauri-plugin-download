package org.silvermine.downloadmanager

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import android.system.Os
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import kotlinx.coroutines.delay
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.TimeUnit

/**
 * WorkManager CoroutineWorker that performs the actual HTTP download.
 *
 * Mirrors the Rust downloader.rs pattern:
 * - Supports resume via Range headers
 * - Writes to a temp file (.download suffix), renames on completion
 * - Throttles progress updates via [ProgressTracker]
 * - Checks store status each progress tick to detect pause/cancel
 * - Runs as a foreground service with a notification
 */
internal class DownloadWorker(
   context: Context,
   params: WorkerParameters,
) : CoroutineWorker(context, params) {

   override suspend fun doWork(): Result {
      val url = inputData.getString(KEY_URL) ?: return Result.failure()
      val path = inputData.getString(KEY_PATH) ?: return Result.failure()

      // The store directory comes from the input data rather than the manager, for the
      // same reason the user agent does below: a worker re-run after process death has
      // no loaded plugin to have configured it, and would otherwise open the default
      // store and write this download's progress where the app never reads it.
      //
      // Refused rather than defaulted, as the url and path above are. Only a work
      // request enqueued before this key existed can lack it, and running it would
      // build the process singleton at the default directory — so a plugin that
      // configured one would be handed the wrong store for the rest of the session.
      // The record survives: reconciliation reverts it on the next manager init, and
      // the download can be started again.
      val storeDir = inputData.getString(KEY_STORE_DIR)?.let { File(it) }
         ?: return Result.failure()

      // Refused like a missing key: the singleton is already open on another store, so
      // this request's progress would land in the wrong one.
      val manager = try {
         DownloadManager.getInstance(applicationContext, storeDir)
      } catch (e: IllegalStateException) {
         Log.w(TAG, "Refusing work for a different store: ${e.message}")
         return Result.failure()
      }
      val store = manager.store
      val tempFile = File("$path$DOWNLOAD_SUFFIX")
      if (store.findByPath(path)?.status == DownloadStatus.Failed) return Result.success()

      try {
         setForeground(createForegroundInfo(path))
      } catch (e: Exception) {
         Log.w(TAG, "Failed to set foreground info: ${e.message}")
      }

      // Byte counts outlive the response scope so the completion block below can
      // report what the progress loop tracked.
      var finalReceivedBytes = 0L
      var finalTotalBytes: Long? = null

      try {
         // Check the size of the already downloaded part, if any.
         var downloadedSize = if (tempFile.exists()) tempFile.length() else 0L

         // The user agent comes from the input data rather than the manager: a worker
         // re-run after process death has no loaded plugin to have set it.
         val response = executeWithRetry(
            requestFor(url, inputData.getString(KEY_USER_AGENT), downloadedSize)
         )

         response.use {
            // If we requested a Range but the server doesn't support partial downloads,
            // fall back to restarting from zero rather than failing.
            if (downloadedSize > 0 && response.code != 206) {
               if (response.isSuccessful) {
                  Log.w(TAG, "Server does not support Range; restarting download from zero")
                  if (tempFile.exists()) tempFile.delete()
                  downloadedSize = 0L
               } else {
                  when (partialFileOutcomeFor(response.code, response.header("Content-Range"), downloadedSize)) {
                     PartialFileOutcome.Complete -> {
                        // Falls through to the rename below, which completes only an
                        // InProgress record — a re-run after process death finds it
                        // reconciled to Paused, as the streaming path does.
                        Log.i(TAG, "Partial already complete; finishing")
                        synchronized(manager) {
                           store.findByPath(path)?.let { store.update(it.withStatus(DownloadStatus.InProgress)) }
                        }
                        finalReceivedBytes = downloadedSize
                        finalTotalBytes = downloadedSize
                        return@use
                     }
                     PartialFileOutcome.Discard -> {
                        Log.w(TAG, "Range not satisfiable; discarding the unusable partial download")
                        if (tempFile.exists()) tempFile.delete()
                        return handleFailure(manager, store, path, DownloadFailure.http(response.code))
                     }
                     PartialFileOutcome.KeepPartial -> {
                        return handleFailure(manager, store, path, DownloadFailure.http(response.code))
                     }
                  }
               }
            }

            if (!response.isSuccessful && response.code != 206) {
               return handleFailure(manager, store, path, DownloadFailure.http(response.code))
            }

            val body = response.body
               ?: return handleFailure(manager, store, path, DownloadFailure("connection", "Empty response body", "unknown"))

            val totalSize = totalSizeFor(body.contentLength(), downloadedSize)

            // Ensure the output folder exists.
            tempFile.parentFile?.let { parent ->
               if (!parent.exists()) parent.mkdirs()
            }

            // Open the temp file in append mode (or truncate if restarting from zero).
            val append = downloadedSize > 0

            // The temp file is the authority: a server that ignores the Range header
            // restarts from zero and the record must follow it down. Synchronized
            // against pause/cancel; a pause landing first is undone by isStopped below.
            var effectiveTotal = totalSize

            synchronized(manager) {
               store.findByPath(path)?.let { record ->
                  val updated = record.withBytes(downloadedSize, totalSize)

                  effectiveTotal = updated.totalBytes
                  store.update(updated.withStatus(DownloadStatus.InProgress))
               }
            }

            // Falls back to the record's known total: without it an unknown content
            // length drops the transfer onto the coarse byte cadence and contradicts
            // the indeterminate flag, which keys off the coalesced emitted total.
            val progressTracker = ProgressTracker(downloadedSize, effectiveTotal)

            fileOperation { FileOutputStream(tempFile, append) }.use { output ->
               val buffer = ByteArray(BUFFER_SIZE)
               val source = body.byteStream()

               while (true) {
                  // Check if the worker has been stopped (canceled externally).
                  if (isStopped) {
                     source.close()
                     revertInProgressRecord(manager, store, path)
                     dismissNotification()
                     return Result.success()
                  }

                  val bytesRead = source.read(buffer)
                  if (bytesRead == -1) break

                  fileOperation { output.write(buffer, 0, bytesRead) }
                  progressTracker.advance(bytesRead.toLong())

                  if (!progressTracker.shouldEmit()) continue

                  progressTracker.markEmitted()

                  // Read and write as one step: pause() holds this same monitor, and a
                  // pause landing between them is overwritten back to InProgress. Since
                  // cancelUniqueWork() is async, a resume in that window then reports
                  // success and enqueues nothing. Emit and notify outside the lock.
                  val currentRecord = synchronized(manager) {
                     val record = store.findByPath(path)

                     if (record != null && record.status == DownloadStatus.InProgress && !progressTracker.isComplete()) {
                        val updated = record
                           .withBytes(progressTracker.receivedBytes, totalSize)
                           .withStatus(DownloadStatus.InProgress)

                        store.update(updated, persist = false)
                        updated
                     } else {
                        record
                     }
                  } ?: break

                  when (currentRecord.status) {
                     DownloadStatus.InProgress -> {
                        if (!progressTracker.isComplete()) {
                           // Emit the record just written.
                           val item = manager.emitChanged(currentRecord)
                           updateNotificationProgress(path, item.progress.toInt(), indeterminate = item.totalBytes == null)
                        }
                        // Completion is handled after the loop exits naturally.
                     }
                     DownloadStatus.Paused -> {
                        // Download was paused — stop reading and exit gracefully.
                        source.close()
                        dismissNotification()
                        return Result.success()
                     }
                     else -> {
                        // Download item was removed or in unexpected state.
                        source.close()
                        dismissNotification()
                        return Result.success()
                     }
                  }
               }
            }

            finalReceivedBytes = progressTracker.receivedBytes
            finalTotalBytes = totalSize
         }

         // Download completed — rename temp file to final path and update store.
         // Synchronized on manager to prevent interleaving with cancel/pause,
         // mirroring the iOS actor serialization pattern.
         synchronized<Unit>(manager) {
            val currentRecord = store.findByPath(path)
            if (currentRecord != null && currentRecord.status == DownloadStatus.InProgress) {
               val finalFile = File(path)
               finalFile.parentFile?.let { parent ->
                  if (!parent.exists()) parent.mkdirs()
               }

               // POSIX rename preserves the errno needed for file-error classification.
               fileOperation { Os.rename(tempFile.absolutePath, finalFile.absolutePath) }
               val completed = currentRecord
                  .withBytes(finalReceivedBytes, finalTotalBytes)
                  .withStatus(DownloadStatus.Completed)
               store.remove(currentRecord)
               manager.emitChanged(completed)
            } else {
               // A concurrent pause/cancel won; do not publish or discard a retained partial.
               Log.w(TAG, "Download item not found or not in expected state after download completed for $path")
            }
         }

         dismissNotification()
         return Result.success()
      } catch (e: Exception) {
         if (isStopped) {
            revertInProgressRecord(manager, store, path)
            return Result.success()
         }
         val failure = when (e) {
            is TransferException -> e.failure
            is DownloadException.Store -> DownloadFailure.command(e)
            is SecurityException -> DownloadFailure.file(e)
            else -> DownloadFailure.network(e)
         }
         return handleFailure(manager, store, path, failure)
      }
   }

   /**
    * Reverts a record still marked InProgress when this worker stops early —
    * either stopped externally, or out of WorkManager attempts on a transient error.
    *
    * A pause cancels the WorkManager work and sets the record to Paused, but the
    * worker may already have written InProgress back before observing isStopped.
    * Without this the record would stay InProgress with no worker behind it until
    * the next reconcileStoreOnInit().
    *
    * [DownloadManager.revertInProgress] decides the resulting status, so this and
    * reconciliation cannot drift apart. The temp file is left in place, and a
    * record that is no longer InProgress — a pause that landed first — is left
    * alone rather than overwritten.
    */
   private fun revertInProgressRecord(manager: DownloadManager, store: DownloadStore, path: String) {
      // Synchronized on manager to prevent interleaving with cancel/pause.
      synchronized(manager) {
         val record = store.findByPath(path) ?: return
         val reverted = DownloadManager.revertInProgress(record, tempFileLength(path)) ?: return

         store.update(reverted)
         manager.emitChanged(reverted)
      }
   }

   /** The two retry layers use the same classification; only exhausted work fails. */
   private fun handleFailure(manager: DownloadManager, store: DownloadStore, path: String, failure: DownloadFailure): Result {
      synchronized(manager) {
         val record = store.findByPath(path) ?: return Result.success()
         if (record.status != DownloadStatus.InProgress || isStopped) return Result.success()
         if (failure.retryability == "transient" && !isOutOfAttempts(runAttemptCount)) {
            dismissNotification()
            return Result.retry()
         }
         val failed = record.failed(failure, tempFileLength(path)) ?: return Result.success()
         store.recordFailure(failed)
         manager.emitChanged(failed)
      }
      dismissNotification()

      return Result.failure()
   }

   private fun notificationID(): Int = id.hashCode()

   private fun ensureNotificationChannel() {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
         val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
         val channel = NotificationChannel(
            NOTIFICATION_CHANNEL_ID,
            "Downloads",
            NotificationManager.IMPORTANCE_LOW,
         )
         notificationManager.createNotificationChannel(channel)
      }
   }

   private fun buildNotification(filename: String, progress: Int, indeterminate: Boolean): android.app.Notification {
      return NotificationCompat.Builder(applicationContext, NOTIFICATION_CHANNEL_ID)
         .setContentTitle("Downloading")
         .setContentText(filename)
         .setSmallIcon(android.R.drawable.stat_sys_download)
         .setOngoing(true)
         .setProgress(100, progress, indeterminate)
         .build()
   }

   private fun createForegroundInfo(path: String): ForegroundInfo {
      ensureNotificationChannel()
      val notification = buildNotification(File(path).name, 0, indeterminate = true)
      return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
         ForegroundInfo(notificationID(), notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
      } else {
         ForegroundInfo(notificationID(), notification)
      }
   }

   private fun updateNotificationProgress(path: String, progress: Int, indeterminate: Boolean) {
      val notification = buildNotification(File(path).name, progress, indeterminate)
      val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      notificationManager.notify(notificationID(), notification)
   }

   private fun dismissNotification() {
      val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      notificationManager.cancel(notificationID())
   }

   /**
    * Executes an OkHttp request with retries and exponential backoff.
    * Mirrors the Rust reqwest-retry middleware (3 retries, exponential backoff).
    * Only retries on transient errors; permanent failures (DNS, TLS) fail immediately.
    * Uses coroutine delay() instead of Thread.sleep() to avoid blocking the dispatcher.
    */
   private suspend fun executeWithRetry(request: Request): Response {
      var lastException: IOException? = null

      for (attempt in 0..MAX_RETRIES) {
         if (attempt > 0) {
            // Exponential backoff: 1s, 2s, 4s
            delay(1000L * (1 shl (attempt - 1)))
         }

         try {
            val response = client.newCall(request).execute()
            if (DownloadFailure.http(response.code).retryability == "transient" && attempt < MAX_RETRIES) {
               response.close()
               Log.w(TAG, "Retrying after HTTP ${response.code} (attempt ${attempt + 1}/$MAX_RETRIES)")
               continue
            }
            return response
         } catch (e: IOException) {
            if (DownloadFailure.network(e).retryability != "transient") throw e
            lastException = e
            Log.w(TAG, "Retrying after ${e.message} (attempt ${attempt + 1}/$MAX_RETRIES)")
         }
      }

      throw lastException ?: IOException("Retry failed")
   }

   internal enum class PartialFileOutcome { Discard, Complete, KeepPartial }

   companion object {
      const val KEY_URL = "download_url"
      const val KEY_PATH = "download_path"
      const val KEY_USER_AGENT = "download_user_agent"
      const val KEY_STORE_DIR = "download_store_dir"

      /**
       * Builds the download request.
       *
       * A null [userAgent] leaves OkHttp's own default in place; a [downloadedSize]
       * above zero asks the server to resume from there. Both headers are set here, so
       * neither can displace the other.
       *
       * Built here rather than inline in [doWork], which needs [WorkerParameters] and
       * so cannot be reached without WorkManager's test artifact.
       */
      internal fun requestFor(url: String, userAgent: String?, downloadedSize: Long): Request {
         val builder = Request.Builder().url(url)

         userAgent?.let { builder.header("User-Agent", it) }
         if (downloadedSize > 0) {
            builder.header("Range", "bytes=$downloadedSize-")
         }

         return builder.build()
      }

      /**
       * The download's total size, or `null` when the server stated none.
       *
       * OkHttp reports -1 for an unstated length. A stated zero is a known total
       * rather than an unknown one: an empty body is a complete download, and
       * collapsing it to null disagreed with desktop, which reports 0.
       *
       * A sum that wrapped negative is not a total either. The header is the
       * server's to choose, so one near [Long.MAX_VALUE] would otherwise reach the
       * caller as a negative byte count.
       *
       * Built here rather than inline in [doWork], which needs [WorkerParameters]
       * and so cannot be reached without WorkManager's test artifact.
       *
       * @param contentLength The body's content length, or -1 when unstated.
       * @param downloadedSize Bytes already on disk, which a Range request excludes.
       * @return The total size, or `null` when the server stated none.
       */
      internal fun totalSizeFor(contentLength: Long, downloadedSize: Long): Long? {
         val total = contentLength + downloadedSize

         return if (contentLength >= 0 && total >= 0) total else null
      }

      /**
       * What a failed resume means for the partial. The one failure allowed to delete
       * it is a 416: every resume would send the same unsatisfiable Range, and dropping
       * it reverts to Idle, which start() can run again — unless the 416's
       * `Content-Range` states a total equal to the partial, which is then complete.
       *
       * Built here rather than inline in [doWork], which cannot be reached without
       * WorkManager's test artifact.
       */
      internal fun partialFileOutcomeFor(responseCode: Int, contentRange: String?, downloadedSize: Long): PartialFileOutcome {
         if (responseCode != HTTP_RANGE_NOT_SATISFIABLE) {
            return PartialFileOutcome.KeepPartial
         }

         val statedTotal = contentRange?.trim()
            ?.takeIf { it.startsWith(RANGE_TOTAL_PREFIX) }
            ?.removePrefix(RANGE_TOTAL_PREFIX)
            ?.toLongOrNull()

         return if (statedTotal == downloadedSize) {
            PartialFileOutcome.Complete
         } else {
            PartialFileOutcome.Discard
         }
      }

      internal const val TAG = "DownloadWorker"
      internal const val DOWNLOAD_SUFFIX = ".download"
      private const val BUFFER_SIZE = 64 * 1024
      private const val HTTP_RANGE_NOT_SATISFIABLE = 416
      private const val RANGE_TOTAL_PREFIX = "bytes */"
      private const val MAX_RETRIES = 3

      /**
       * Five retries on WorkManager's default 30-second exponential backoff — relied
       * on here rather than set — is roughly a quarter hour.
       */
      private const val MAX_WORK_ATTEMPTS = 5
      private const val NOTIFICATION_CHANNEL_ID = "download_manager_channel"

      /**
       * The length of a download's temp file, or `null` when it is absent.
       *
       * @param path The download path.
       * @return The temp file's length, or `null` when there is no temp file.
       */
      internal fun tempFileLength(path: String): Long? {
         val tempFile = File("$path$DOWNLOAD_SUFFIX")
         return if (tempFile.exists()) tempFile.length() else null
      }

      /**
       * Whether a transient failure has run out of retries and should give up.
       *
       * WorkManager counts the runs before this one, so the first sees 0 and the
       * download gives up on the run that sees [MAX_WORK_ATTEMPTS].
       */
      internal fun isOutOfAttempts(runAttemptCount: Int): Boolean =
         runAttemptCount >= MAX_WORK_ATTEMPTS

      private val client = OkHttpClient.Builder()
         .connectTimeout(30, TimeUnit.SECONDS)
         .readTimeout(30, TimeUnit.SECONDS)
         .followRedirects(true)
         .followSslRedirects(false)
         .build()
   }
}
