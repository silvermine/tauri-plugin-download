package org.silvermine.downloadmanager

import android.content.Context
import android.util.AtomicFile
import android.util.Log
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File

@Serializable
internal data class PersistedDownloadItem(
   @SerialName("url")
   val url: String,

   @SerialName("path")
   val path: String,

   @SerialName("transferredBytes")
   val transferredBytes: Long = 0,

   @SerialName("totalBytes")
   val totalBytes: Long? = null,

   @SerialName("status")
   val status: DownloadStatus = DownloadStatus.Idle,
)

internal fun DownloadItem.toPersistedDownloadItem(): PersistedDownloadItem =
   PersistedDownloadItem(
      url = url,
      path = path,
      transferredBytes = transferredBytes,
      totalBytes = totalBytes,
      status = status,
   )

internal fun PersistedDownloadItem.toDownloadItem(legacyProgress: Double? = null): DownloadItem {
   val item = DownloadItem(
      url = url,
      path = path,
      transferredBytes = transferredBytes,
      totalBytes = totalBytes,
      status = status,
   ).withStatus(status)

   return if (item.transferredBytes == 0L && item.totalBytes == null && legacyProgress != null) {
      item.copy(progress = legacyProgress)
   } else {
      item
   }
}

/**
 * Thread-safe store for download items backed by an atomic JSON file.
 *
 * All public methods are synchronized to ensure consistency when accessed
 * from multiple threads (e.g. WorkManager workers and the main thread).
 * Mirrors the iOS DownloadStore actor pattern.
 */
internal class DownloadStore(context: Context) {
   private val json = Json { ignoreUnknownKeys = true }
   private val file = AtomicFile(File(context.filesDir, STORE_FILENAME))
   private val downloads = mutableMapOf<String, DownloadItem>()

   init {
      load()
   }

   @Synchronized
   fun list(): List<DownloadItem> = downloads.values.toList()

   @Synchronized
   fun findByPath(path: String): DownloadItem? = downloads[path]

   @Synchronized
   fun append(item: DownloadItem) {
      downloads[item.path] = item
      save()
   }

   @Synchronized
   fun update(item: DownloadItem, persist: Boolean = true) {
      if (downloads.containsKey(item.path)) {
         downloads[item.path] = item
      }
      if (persist) {
         save()
      }
   }

   @Synchronized
   fun remove(item: DownloadItem) {
      downloads.remove(item.path)
      save()
   }

   private fun load() {
      try {
         val bytes = file.readFully()
         val items = json.parseToJsonElement(String(bytes)).jsonArray
         downloads.clear()
         for (element in items) {
            val item = json.decodeFromJsonElement(PersistedDownloadItem.serializer(), element)
            val legacyProgress = element.jsonObject["progress"]?.jsonPrimitive?.doubleOrNull
            downloads[item.path] = item.toDownloadItem(legacyProgress)
         }
      } catch (e: Exception) {
         Log.e(TAG, "Failed to load download store: ${e.message}")
      }
   }

   private fun save() {
      val items = downloads.values.map { it.toPersistedDownloadItem() }
      val bytes = json.encodeToString(items).toByteArray()
      val stream = file.startWrite()
      try {
         stream.write(bytes)
         file.finishWrite(stream)
      } catch (e: Exception) {
         file.failWrite(stream)
         Log.e(TAG, "Failed to save download store: ${e.message}")
      }
   }

   companion object {
      private const val TAG = "DownloadStore"
      private const val STORE_FILENAME = "downloads.json"
   }
}
