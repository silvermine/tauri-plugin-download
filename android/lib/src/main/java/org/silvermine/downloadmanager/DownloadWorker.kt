package org.silvermine.downloadmanager

import android.annotation.SuppressLint
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
import androidx.work.WorkInfo
import androidx.work.WorkerParameters
import kotlinx.coroutines.CancellationException
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
 * - Checks request ownership before file and store mutations to detect pause/cancel
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
      // These old requests also lack STORE_TAG, so startup reconciliation cancels
      // them and restores a state the caller can start or resume.
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
      // An untagged worker can initialize the manager first and be canceled by its
      // own startup reconciliation. Do not rely on asynchronous stop delivery: reject
      // it here before any HTTP request or file write, even if its inputs are valid.
      if (DownloadManager.STORE_TAG !in tags) return Result.failure()

      val store = manager.store
      val tempFile = File("$path$DOWNLOAD_SUFFIX")
      // Byte counts outlive the response scope so the completion block below can
      // report what the progress loop tracked.
      var finalReceivedBytes = 0L
      var finalTotalBytes: Long? = null

      try {
         synchronized(manager) {
            if (!canTransfer(manager, store, path)) return Result.success()
         }
         try {
            setForeground(createForegroundInfo(path))
         } catch (e: CancellationException) {
            throw e
         } catch (e: Exception) {
            Log.w(TAG, "Failed to set foreground info: ${e.message}")
         }

         // Check the size of the already downloaded part, if any.
         var downloadedSize = if (tempFile.exists()) tempFile.length() else 0L

         // The user agent comes from the input data rather than the manager: a worker
         // re-run after process death has no loaded plugin to have set it.
         val response = executeWithRetry(
            requestFor(url, inputData.getString(KEY_USER_AGENT), downloadedSize)
         ) {
            synchronized(manager) { canTransfer(manager, store, path) }
         }

         response.use {
            // If we requested a Range but the server doesn't support partial downloads,
            // fall back to restarting from zero rather than failing.
            if (downloadedSize > 0 && response.code != 206) {
               if (response.isSuccessful) {
                  Log.w(TAG, "Server does not support Range; restarting download from zero")
                  synchronized(manager) {
                     if (!canTransfer(manager, store, path)) return Result.success()
                     if (tempFile.exists()) tempFile.delete()
                     downloadedSize = 0L
                  }
               } else {
                  when (partialFileOutcomeFor(response.code, response.header("Content-Range"), downloadedSize)) {
                     PartialFileOutcome.Complete -> {
                        // The completion block checks ownership again before renaming.
                        Log.i(TAG, "Partial already complete; finishing")
                        synchronized(manager) {
                           if (!canTransfer(manager, store, path)) return Result.success()
                           store.findByPath(path)?.let { store.update(it.withStatus(DownloadStatus.InProgress)) }
                        }
                        finalReceivedBytes = downloadedSize
                        finalTotalBytes = downloadedSize
                        return@use
                     }
                     PartialFileOutcome.Discard -> {
                        Log.w(TAG, "Range not satisfiable; discarding the unusable partial download")
                        synchronized(manager) {
                           if (!canTransfer(manager, store, path)) return Result.success()
                           if (tempFile.exists()) tempFile.delete()
                        }
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

            // Open the temp file in append mode (or truncate if restarting from zero).
            val append = downloadedSize > 0

            // The temp file is the authority: a server that ignores the Range header
            // restarts from zero and the record must follow it down. Ownership checks
            // prevent a late response from undoing pause or modifying replacement work.
            var effectiveTotal = totalSize

            synchronized(manager) {
               if (!canTransfer(manager, store, path)) return Result.success()
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

            val output = synchronized(manager) {
               if (!canTransfer(manager, store, path)) return Result.success()
               fileOperation {
                  tempFile.parentFile?.let { parent -> if (!parent.exists()) parent.mkdirs() }
                  FileOutputStream(tempFile, append)
               }
            }
            output.use {
               val buffer = ByteArray(BUFFER_SIZE)
               val source = body.byteStream()

               while (true) {
                  synchronized(manager) {
                     if (!canTransfer(manager, store, path)) return Result.success()
                  }

                  val bytesRead = source.read(buffer)
                  if (bytesRead == -1) break

                  // A blocking read can return after pause/resume or cancel/recreate.
                  // Check and write under the same monitor as those commands.
                  synchronized(manager) {
                     if (!canTransfer(manager, store, path)) return Result.success()
                     fileOperation { output.write(buffer, 0, bytesRead) }
                     progressTracker.advance(bytesRead.toLong())
                  }

                  if (!progressTracker.shouldEmit()) continue

                  progressTracker.markEmitted()

                  // Order progress writes and events with explicit pause/cancel.
                  // Notification updates do not mutate the download record.
                  val currentRecord = synchronized(manager) {
                     if (!canTransfer(manager, store, path)) return Result.success()
                     val record = store.findByPath(path)

                     if (record != null && record.status == DownloadStatus.InProgress && !progressTracker.isComplete()) {
                        val updated = record
                           .withBytes(progressTracker.receivedBytes, totalSize)
                           .withStatus(DownloadStatus.InProgress)

                        store.update(updated, persist = false)
                        manager.emitChanged(updated)
                        updated
                     } else {
                        record
                     }
                  } ?: break

                  when (currentRecord.status) {
                     DownloadStatus.InProgress -> {
                        if (!progressTracker.isComplete()) {
                           // Update the notification from the record just emitted.
                           val item = currentRecord.toItem()
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
            if (canTransfer(manager, store, path) && currentRecord?.status == DownloadStatus.InProgress) {
               val finalFile = File(path)
               finalFile.parentFile?.let { parent ->
                  if (!parent.exists()) parent.mkdirs()
               }

               // POSIX rename preserves the errno needed for file-error classification.
               fileOperation { Os.rename(tempFile.absolutePath, finalFile.absolutePath) }
               val completed = currentRecord
                  .withBytes(finalReceivedBytes, finalTotalBytes)
                  .withStatus(DownloadStatus.Completed)
               store.recordCompletion(completed) {
                  manager.workOwnership.revoke(path)
                  manager.emitChanged(it)
               }
            } else {
               // An obsolete worker must not remove a replacement's partial file.
               Log.w(TAG, "Download item not found or not in expected state after download completed for $path")
            }
         }

         dismissNotification()
         return Result.success()
      } catch (e: CancellationException) {
         throw e
      } catch (e: Exception) {
         return handleFailure(manager, store, path, failureFor(e))
      } finally {
         checkpointProgress(manager, store, path)
         dismissNotification()
      }
   }

   /**
    * Saves resumable bytes during scheduler holds, or reverts app-canceled work.
    * A replaced or revoked worker cannot checkpoint over the newer command.
    */
   private fun checkpointProgress(manager: DownloadManager, store: DownloadStore, path: String) {
      synchronized(manager) {
         val updated = interruptedRecord(
            store.findByPath(path), tempFileLength(path), manager.workOwnership.owns(path, id),
            isCanceledByApp(),
         ) ?: return
         try {
            store.update(updated)
         } catch (error: DownloadException.Store) {
            store.update(updated, persist = false)
            Log.w(TAG, "Failed to persist interrupted progress", error)
         }
         if (updated.status != DownloadStatus.InProgress) manager.workOwnership.revoke(path)
         manager.emitChanged(updated)
      }
   }

   // WorkManager records this value itself on every API level, including below 31.
   @SuppressLint("NewApi")
   private fun isCanceledByApp(): Boolean = stopReason == WorkInfo.STOP_REASON_CANCELLED_BY_APP

   /** Called under the manager monitor before each transfer side effect. */
   private fun canTransfer(manager: DownloadManager, store: DownloadStore, path: String): Boolean =
      !isStopped && manager.workOwnership.owns(path, id) &&
         store.findByPath(path)?.status == DownloadStatus.InProgress

   /** The two retry layers use the same classification; only exhausted work fails. */
   private fun handleFailure(manager: DownloadManager, store: DownloadStore, path: String, failure: DownloadFailure): Result {
      synchronized(manager) {
         val record = store.findByPath(path)
         when (failureOutcome(failure, runAttemptCount, record?.status,
            isStopped, manager.workOwnership.owns(path, id))) {
            FailureOutcome.Ignore -> { dismissNotification(); return Result.success() }
            FailureOutcome.Retry -> { dismissNotification(); return Result.retry() }
            FailureOutcome.Fail -> {
               val failed = record?.failed(failure, tempFileLength(path)) ?: return Result.success()
               store.recordFailure(failed)
               manager.emitChanged(failed)
            }
         }
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
    * Only retries transient failures; permanent and unknown failures stop immediately.
    * Uses coroutine delay() instead of Thread.sleep() to avoid blocking the dispatcher.
    */
   private suspend fun executeWithRetry(request: Request, canContinue: () -> Boolean): Response {
      var lastException: IOException? = null

      for (attempt in 0..MAX_RETRIES) {
         if (attempt > 0) {
            // Exponential backoff: 1s, 2s, 4s
            delay(1000L * (1 shl (attempt - 1)))
         }

         if (!canContinue()) throw CancellationException("Download work no longer active")
         try {
            val response = client.newCall(request).execute()
            if (shouldRetryRequest(DownloadFailure.http(response.code), attempt)) {
               response.close()
               Log.w(TAG, "Retrying after HTTP ${response.code} (attempt ${attempt + 1}/$MAX_RETRIES)")
               continue
            }
            return response
         } catch (e: IOException) {
            if (!shouldRetryRequest(failureFor(e), attempt)) throw e
            lastException = e
            Log.w(TAG, "Retrying after ${e.message} (attempt ${attempt + 1}/$MAX_RETRIES)")
         }
      }

      throw lastException ?: IOException("Retry failed")
   }

   internal enum class PartialFileOutcome { Discard, Complete, KeepPartial }

   internal enum class FailureOutcome { Ignore, Retry, Fail }

   companion object {
      /**
       * Checkpoints scheduler-held work and reverts work explicitly canceled by the host.
       *
       * @param record The current record, absent after cancellation or completion.
       * @param tempFileLength Bytes actually retained on disk, or null without a partial.
       * @param ownsWork Whether this request still owns the path after live commands.
       * @param canceledByApp Whether WorkManager permanently canceled this running request.
       * @return Updated record, or null when a newer command owns the state.
       */
      internal fun interruptedRecord(
         record: DownloadRecord?, tempFileLength: Long?, ownsWork: Boolean, canceledByApp: Boolean = false,
      ): DownloadRecord? = if (ownsWork && record?.status == DownloadStatus.InProgress) {
         if (canceledByApp) DownloadManager.revertInProgress(record, tempFileLength)
         else record.withBytes(tempFileLength ?: 0L)
      } else {
         null
      }

      /** Decides recovery without WorkManager or Android runtime dependencies. */
      internal fun failureOutcome(
         failure: DownloadFailure,
         attempt: Int,
         status: DownloadStatus?,
         stopped: Boolean,
         ownsWork: Boolean = true,
      ): FailureOutcome = when {
         !ownsWork || stopped || status != DownloadStatus.InProgress -> FailureOutcome.Ignore
         failure.retryability == "transient" && !isOutOfAttempts(attempt) -> FailureOutcome.Retry
         else -> FailureOutcome.Fail
      }

      /** Keeps request retries consistent with the final failure classification. */
      internal fun shouldRetryRequest(failure: DownloadFailure, attempt: Int): Boolean =
         failure.retryability == "transient" && attempt < MAX_RETRIES

      /** Preserves file/store boundaries before treating an exception as transport failure. */
      internal fun failureFor(error: Exception): DownloadFailure = when (error) {
         is TransferException -> error.failure
         is DownloadException.Store -> DownloadFailure.command(error)
         is SecurityException -> DownloadFailure.file(error)
         else -> DownloadFailure.network(error)
      }

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
       * it leaves a Failed record that resume() can restart from zero — unless the 416's
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
