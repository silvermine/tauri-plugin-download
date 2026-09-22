package org.silvermine.downloadmanager

import android.util.AtomicFile
import android.util.Log
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.longOrNull
import java.io.File

/** The envelope fields have no defaults so both are always required and written. */
@Serializable
private data class StoreDocument(val version: Int, val downloads: List<DownloadRecord>)

/**
 * Thread-safe store for download records backed by an atomic JSON file.
 *
 * All public methods are synchronized to ensure consistency when accessed
 * from multiple threads (e.g. WorkManager workers and the main thread).
 * Mirrors the iOS DownloadStore actor pattern.
 *
 * @param directory The directory holding [STORE_FILENAME]. Taken rather than derived
 *    from a Context so the location is configurable; the caller resolves the default.
 *    Read here and nowhere else, which is why it cannot be changed after construction.
 */
internal class DownloadStore(directory: File) {
   private val file = AtomicFile(storeFile(directory))
   private val downloads = mutableMapOf<String, DownloadRecord>()

   init {
      load()
   }

   @Synchronized
   fun list(): List<DownloadRecord> = downloads.values.toList()

   @Synchronized
   fun findByPath(path: String): DownloadRecord? = downloads[path]

   @Synchronized
   fun append(record: DownloadRecord) {
      downloads[record.path] = record
      save()
   }

   @Synchronized
   fun update(record: DownloadRecord, persist: Boolean = true) {
      val previous = downloads[record.path]
      if (previous != null) downloads[record.path] = record
      if (persist) {
         try {
            save()
         } catch (error: Exception) {
            if (previous != null) downloads[record.path] = previous
            throw error
         }
      }
   }

   /** A full disk must not hide the original failure from the current session. */
   @Synchronized
   fun recordFailure(record: DownloadRecord) {
      if (!downloads.containsKey(record.path)) return
      downloads[record.path] = record
      try {
         save()
      } catch (error: Exception) {
         Log.e(TAG, "Failed to persist download failure", error)
      }
   }

   /**
    * Applies several record updates with a single write.
    *
    * Reconciliation can revert many records at once; one write per record would
    * rewrite the whole file that many times.
    *
    * @param records The records to update. Unknown paths are ignored.
    */
   @Synchronized
   fun update(records: List<DownloadRecord>) {
      if (records.isEmpty()) {
         return
      }

      for (record in records) {
         if (downloads.containsKey(record.path)) {
            downloads[record.path] = record
         }
      }

      save()
   }

   @Synchronized
   fun remove(record: DownloadRecord) {
      downloads.remove(record.path)
      save()
   }

   private fun load() {
      try {
         val records = decodeRecords(String(file.readFully()))
         downloads.clear()
         for (record in records) {
            downloads[record.path] = record
         }
      } catch (e: Exception) {
         Log.e(TAG, "Failed to load download store: ${e.message}")
      }
   }

   private fun save() {
      val bytes = encodeRecords(downloads.values.toList()).toByteArray()
      val stream = try {
         file.startWrite()
      } catch (e: Exception) {
         throw DownloadException.Store(e)
      }
      try {
         stream.write(bytes)
         file.finishWrite(stream)
      } catch (e: Exception) {
         file.failWrite(stream)
         throw DownloadException.Store(e)
      }
   }

   companion object {
      private const val TAG = "DownloadStore"
      private const val STORE_FILENAME = "downloads.json"
      private const val CURRENT_SCHEMA_VERSION = 2

      /**
       * Resolves the store file inside a directory.
       *
       * @param directory The directory holding the store.
       * @return The store file.
       */
      internal fun storeFile(directory: File): File = File(directory, STORE_FILENAME)

      private val json = Json { ignoreUnknownKeys = true }
      private val schemaVersionPattern = Regex("0|[1-9][0-9]*")

      /**
       * Decodes persisted records.
       *
       * Rejects the whole document if any record is malformed. Preserving an
       * unreadable file before continuing empty is tracked separately in #64.
       *
       * Extracted from [load] so it can be unit-tested, and deliberately free of
       * logging to keep it so: `android.util.Log` is a throwing stub off-device.
       *
       * @param text The persisted store's contents.
       * @return The decoded records.
       */
      internal fun decodeRecords(text: String): List<DownloadRecord> {
         val root = try {
            json.parseToJsonElement(text)
         } catch (_: SerializationException) {
            // Decoder messages can contain private values from the input.
            throw SerializationException("Malformed store envelope")
         }
         val document = root as? JsonObject
            ?: throw SerializationException("Malformed store envelope")
         val versionField = document["version"] as? JsonPrimitive
         // Tree parsing accepts primitive tokens such as +1 and 01. Require JSON
         // integer syntax before converting so malformed numbers cannot become v1.
         val version = versionField
            ?.takeUnless { it.isString }
            ?.takeIf { schemaVersionPattern.matches(it.content) }
            ?.longOrNull
         val records = document["downloads"] as? JsonArray
         if (version == null || records == null) {
            throw SerializationException("Malformed store envelope")
         }
         if (version != 1L && version != CURRENT_SCHEMA_VERSION.toLong()) {
            throw SerializationException("Unsupported store version: $version (expected $CURRENT_SCHEMA_VERSION)")
         }

         return try {
            json.decodeFromJsonElement<List<DownloadRecord>>(records).also { decoded ->
            if (decoded.any { it.status == DownloadStatus.Failed && it.error == null }) {
               throw SerializationException("Invalid store records")
            }
         }
         } catch (_: SerializationException) {
            throw SerializationException("Invalid store records")
         }
      }

      /**
       * Encodes records for persistence.
       *
       * @param records The records to encode.
       * @return The JSON text to persist.
       */
      internal fun encodeRecords(records: List<DownloadRecord>): String =
         json.encodeToString(StoreDocument(CURRENT_SCHEMA_VERSION, records))
   }
}
