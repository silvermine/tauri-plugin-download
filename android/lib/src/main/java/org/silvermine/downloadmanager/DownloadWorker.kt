package org.silvermine.downloadmanager

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Interceptor
import okhttp3.Response
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InterruptedIOException
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException

/**
 * WorkManager CoroutineWorker that performs the actual HTTP download.
 *
 * Mirrors the Rust downloader.rs pattern:
 * - Supports resume via Range headers
 * - Writes to a temp file (.download suffix), renames on completion
 * - Throttles progress updates to 1% increments
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

        val manager = DownloadManager.getInstance(applicationContext)
        val store = manager.store
        val tempFile = File("$path$DOWNLOAD_SUFFIX")

        try {
            setForeground(createForegroundInfo(path))
        } catch (e: Exception) {
            Log.w(TAG, "Failed to set foreground info: ${e.message}")
        }

        try {
            // Check the size of the already downloaded part, if any.
            var downloadedSize = if (tempFile.exists()) tempFile.length() else 0L

            // Build request with Range header for resuming.
            val requestBuilder = Request.Builder().url(url)
            if (downloadedSize > 0) {
                requestBuilder.header("Range", "bytes=$downloadedSize-")
            }

            val response = client.newCall(requestBuilder.build()).execute()

            response.use {
                // If we requested a Range but the server doesn't support partial downloads,
                // fall back to restarting from zero rather than failing.
                if (downloadedSize > 0 && response.code != 206) {
                    if (response.isSuccessful) {
                        Log.w(TAG, "Server does not support Range; restarting download from zero")
                        if (tempFile.exists()) tempFile.delete()
                        downloadedSize = 0L
                    } else {
                        return handleError(manager, store, path, tempFile, "HTTP ${response.code}: ${response.message}")
                    }
                }

                if (!response.isSuccessful && response.code != 206) {
                    return handleError(manager, store, path, tempFile, "HTTP ${response.code}: ${response.message}")
                }

                val body = response.body
                    ?: return handleError(manager, store, path, tempFile, "Empty response body")

                // Get the total size of the file from headers (if available).
                val contentLength = body.contentLength()
                val totalSize = if (contentLength > 0) contentLength + downloadedSize else 0L

                // Ensure the output folder exists.
                tempFile.parentFile?.let { parent ->
                    if (!parent.exists()) parent.mkdirs()
                }

                // Open the temp file in append mode (or truncate if restarting from zero).
                val append = downloadedSize > 0
                var downloaded = downloadedSize
                var lastEmittedProgress = 0.0

                // Update status to in-progress.
                store.findByPath(path)?.let { item ->
                    val updated = item.withStatus(DownloadStatus.InProgress)
                    store.update(updated)
                    manager.emitChanged(updated)
                }

                FileOutputStream(tempFile, append).use { output ->
                    val buffer = ByteArray(BUFFER_SIZE)
                    val source = body.byteStream()

                    while (true) {
                        // Check if the worker has been stopped (cancelled externally).
                        if (isStopped) {
                            source.close()
                            dismissNotification()
                            return Result.success()
                        }

                        val bytesRead = source.read(buffer)
                        if (bytesRead == -1) break

                        output.write(buffer, 0, bytesRead)
                        downloaded += bytesRead

                        val progress = if (totalSize > 0) {
                            (downloaded.toDouble() / totalSize.toDouble()) * 100.0
                        } else {
                            0.0
                        }

                        // Throttle progress updates — only emit if progress increases by at least 1%.
                        if (progress < 100.0 && progress - lastEmittedProgress <= PROGRESS_THRESHOLD) {
                            continue
                        }

                        lastEmittedProgress = progress
                        val currentItem = store.findByPath(path) ?: break

                        when (currentItem.status) {
                            DownloadStatus.InProgress -> {
                                if (progress < 100.0) {
                                    val updated = currentItem.withProgress(progress)
                                    store.update(updated, persist = false)
                                    manager.emitChanged(updated)
                                    updateNotificationProgress(path, progress.toInt())
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
            }

            // Download completed — rename temp file to final path and update store.
            val currentItem = store.findByPath(path)
            if (currentItem != null && currentItem.status == DownloadStatus.InProgress) {
                val finalFile = File(path)
                finalFile.parentFile?.let { parent ->
                    if (!parent.exists()) parent.mkdirs()
                }

                // Remove existing file (if found) and move downloaded file to destination.
                if (finalFile.exists()) finalFile.delete()
                if (!tempFile.renameTo(finalFile)) {
                    return handleError(manager, store, path, tempFile, "Failed to move download to ${finalFile.path}")
                }

                val completed = currentItem.withStatus(DownloadStatus.Completed)
                store.remove(currentItem)
                manager.emitChanged(completed)
            }

            dismissNotification()
            return Result.success()
        } catch (e: Exception) {
            return handleError(manager, store, path, tempFile, e.message ?: "Unknown error")
        }
    }

    private fun handleError(manager: DownloadManager, store: DownloadStore, path: String, tempFile: File, message: String): Result {
        Log.e(TAG, "Download failed for $path: $message")

        // Clean up temp file.
        // Match iOS behavior: hard failures cancel the download and remove from store.
        if (tempFile.exists()) tempFile.delete()
        store.findByPath(path)?.let { item ->
            val cancelled = item.withStatus(DownloadStatus.Cancelled)
            store.remove(item)
            manager.emitChanged(cancelled)
        }

        dismissNotification()
        return Result.failure()
    }

    private fun notificationId(): Int = id.hashCode()

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
            ForegroundInfo(notificationId(), notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(notificationId(), notification)
        }
    }

    private fun updateNotificationProgress(path: String, progress: Int) {
        val notification = buildNotification(File(path).name, progress, indeterminate = false)
        val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        notificationManager.notify(notificationId(), notification)
    }

    private fun dismissNotification() {
        val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        notificationManager.cancel(notificationId())
    }

    /**
     * OkHttp interceptor that retries transient failures with exponential backoff.
     * Mirrors the Rust reqwest-retry middleware (3 retries, exponential backoff).
     * Only retries on transient errors; permanent failures (DNS, TLS) fail immediately.
     */
    private class RetryInterceptor(
        private val maxRetries: Int = MAX_RETRIES,
    ) : Interceptor {
        override fun intercept(chain: Interceptor.Chain): Response {
            var lastException: IOException? = null

            for (attempt in 0..maxRetries) {
                if (attempt > 0) {
                    // Exponential backoff: 1s, 2s, 4s
                    Thread.sleep(1000L * (1 shl (attempt - 1)))
                }

                try {
                    val response = chain.proceed(chain.request())
                    if (response.code in 500..599 && attempt < maxRetries) {
                        response.close()
                        Log.w(TAG, "Retrying after HTTP ${response.code} (attempt ${attempt + 1}/$maxRetries)")
                        continue
                    }
                    return response
                } catch (e: IOException) {
                    if (!isTransient(e)) throw e
                    lastException = e
                    Log.w(TAG, "Retrying after ${e.message} (attempt ${attempt + 1}/$maxRetries)")
                }
            }

            throw lastException ?: IOException("Retry failed")
        }

        private fun isTransient(e: IOException): Boolean = when (e) {
            is UnknownHostException -> false  // DNS resolution failed
            is SSLException -> false          // TLS/certificate errors
            is InterruptedIOException -> e.message?.contains("timeout", ignoreCase = true) == true
            else -> true                      // Connection reset, broken pipe, etc.
        }
    }

    companion object {
        const val KEY_URL = "download_url"
        const val KEY_PATH = "download_path"

        internal const val TAG = "DownloadWorker"
        internal const val DOWNLOAD_SUFFIX = ".download"
        private const val BUFFER_SIZE = 8 * 1024
        private const val PROGRESS_THRESHOLD = 1.0
        private const val MAX_RETRIES = 3
        private const val NOTIFICATION_CHANNEL_ID = "download_manager_channel"

        private val client = OkHttpClient.Builder()
            .addInterceptor(RetryInterceptor())
            .connectTimeout(30, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .followRedirects(true)
            .followSslRedirects(true)
            .build()
    }
}
