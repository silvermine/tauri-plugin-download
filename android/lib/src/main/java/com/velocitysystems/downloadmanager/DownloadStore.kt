package com.velocitysystems.downloadmanager

import android.content.Context
import android.util.AtomicFile
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File

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
    fun findByUrl(url: String): DownloadItem? =
        downloads.values.firstOrNull { it.url == url }

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
            val items: List<DownloadItem> = json.decodeFromString(String(bytes))
            downloads.clear()
            for (item in items) {
                downloads[item.path] = item
            }
        } catch (_: Exception) {
            // File doesn't exist or is corrupt — start with empty store.
        }
    }

    private fun save() {
        val items = downloads.values.toList()
        val bytes = json.encodeToString(items).toByteArray()
        val stream = file.startWrite()
        try {
            stream.write(bytes)
            file.finishWrite(stream)
        } catch (e: Exception) {
            file.failWrite(stream)
        }
    }

    companion object {
        private const val STORE_FILENAME = "downloads.json"
    }
}
