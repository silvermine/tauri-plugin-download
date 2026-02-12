package com.velocitysystems.downloadmanager

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
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.TimeUnit

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

    private val client = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .followRedirects(true)
        .followSslRedirects(true)
        .build()

    override suspend fun doWork(): Result {
        val url = inputData.getString(KEY_URL) ?: return Result.failure()
        val path = inputData.getString(KEY_PATH) ?: return Result.failure()

        val store = DownloadManager.getInstance(applicationContext).store
        val tempPath = "$path$DOWNLOAD_SUFFIX"
        val tempFile = File(tempPath)

        try {
            setForeground(createForegroundInfo(path))
        } catch (e: Exception) {
            Log.w(TAG, "Failed to set foreground info: ${e.message}")
        }

        try {
            // Check the size of the already downloaded part, if any.
            val downloadedSize = if (tempFile.exists()) tempFile.length() else 0L

            // Build request with Range header for resuming.
            val requestBuilder = Request.Builder().url(url)
            if (downloadedSize > 0) {
                requestBuilder.header("Range", "bytes=$downloadedSize-")
            }

            val response = client.newCall(requestBuilder.build()).execute()

            // Ensure the server supports partial downloads.
            if (downloadedSize > 0 && response.code != 206) {
                response.close()
                return handleError(store, path, tempFile, "Server does not support partial downloads")
            }

            if (!response.isSuccessful && response.code != 206) {
                response.close()
                return handleError(store, path, tempFile, "HTTP ${response.code}: ${response.message}")
            }

            val body = response.body ?: run {
                response.close()
                return handleError(store, path, tempFile, "Empty response body")
            }

            // Get the total size of the file from headers (if available).
            val contentLength = body.contentLength()
            val totalSize = if (contentLength > 0) contentLength + downloadedSize else 0L

            // Ensure the output folder exists.
            tempFile.parentFile?.let { parent ->
                if (!parent.exists()) parent.mkdirs()
            }

            // Open the temp file in append mode.
            var downloaded = downloadedSize
            var lastEmittedProgress = 0.0

            // Update status to in-progress.
            store.findByPath(path)?.let { item ->
                val updated = item.withStatus(DownloadStatus.InProgress)
                store.update(updated)
                DownloadManager.getInstance(applicationContext).emitChanged(updated)
            }

            FileOutputStream(tempFile, true).use { output ->
                val buffer = ByteArray(BUFFER_SIZE)
                val source = body.byteStream()

                while (true) {
                    // Check if the worker has been stopped (cancelled externally).
                    if (isStopped) {
                        source.close()
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
                                store.update(updated)
                                DownloadManager.getInstance(applicationContext).emitChanged(updated)
                            }
                            // Completion is handled after the loop exits naturally.
                        }
                        DownloadStatus.Paused -> {
                            // Download was paused — stop reading and exit gracefully.
                            source.close()
                            return Result.success()
                        }
                        else -> {
                            // Download item was removed or in unexpected state.
                            source.close()
                            return Result.success()
                        }
                    }
                }
            }

            response.close()

            // Download completed — rename temp file to final path and update store.
            val currentItem = store.findByPath(path)
            if (currentItem != null && currentItem.status == DownloadStatus.InProgress) {
                val finalFile = File(path)
                finalFile.parentFile?.let { parent ->
                    if (!parent.exists()) parent.mkdirs()
                }

                // Remove existing file (if found) and move downloaded file to destination.
                if (finalFile.exists()) finalFile.delete()
                tempFile.renameTo(finalFile)

                val completed = currentItem.withStatus(DownloadStatus.Completed)
                store.remove(currentItem)
                DownloadManager.getInstance(applicationContext).emitChanged(completed)
            }

            return Result.success()
        } catch (e: Exception) {
            return handleError(store, path, tempFile, e.message ?: "Unknown error")
        }
    }

    private fun handleError(store: DownloadStore, path: String, tempFile: File, message: String): Result {
        Log.e(TAG, "Download failed for $path: $message")

        store.findByPath(path)?.let { item ->
            store.remove(item)
            if (tempFile.exists()) tempFile.delete()
            DownloadManager.getInstance(applicationContext).emitChanged(item.withStatus(DownloadStatus.Cancelled))
        }

        return Result.failure()
    }

    private fun createForegroundInfo(path: String): ForegroundInfo {
        val channelId = NOTIFICATION_CHANNEL_ID
        val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

        val channel = NotificationChannel(
            channelId,
            "Downloads",
            NotificationManager.IMPORTANCE_LOW,
        )
        notificationManager.createNotificationChannel(channel)

        val filename = File(path).name
        val notification = NotificationCompat.Builder(applicationContext, channelId)
            .setContentTitle("Downloading")
            .setContentText(filename)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setOngoing(true)
            .setProgress(0, 0, true)
            .build()

        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(path.hashCode(), notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(path.hashCode(), notification)
        }
    }

    companion object {
        const val KEY_URL = "download_url"
        const val KEY_PATH = "download_path"

        internal const val TAG = "DownloadWorker"
        private const val DOWNLOAD_SUFFIX = ".download"
        private const val BUFFER_SIZE = 8 * 1024
        private const val PROGRESS_THRESHOLD = 1.0
        private const val NOTIFICATION_CHANNEL_ID = "download_manager_channel"
    }
}
