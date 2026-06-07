package org.silvermine.downloadmanager

import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class DownloadItemTest {

   private val json = Json { ignoreUnknownKeys = true }

   @Test
   fun `persisted item decoding preserves legacy progress when byte tracking fields are missing`() {
      val elements = json.parseToJsonElement(
         """
         [
            {
               "url": "https://example.com/file.bin",
               "path": "/tmp/file.bin",
               "progress": 42.5,
               "status": "paused"
            }
         ]
         """.trimIndent(),
      ).jsonArray

      val element = elements.single()
      val persisted = json.decodeFromJsonElement(PersistedDownloadItem.serializer(), element)
      val item = persisted.toDownloadItem(element.jsonObject["progress"]?.jsonPrimitive?.doubleOrNull)

      assertEquals(0L, item.transferredBytes)
      assertNull(item.totalBytes)
      assertEquals(42.5, item.progress, 0.0)
      assertEquals(DownloadStatus.Paused, item.status)
   }

   @Test
   fun `persisted item encoding omits progress`() {
      val jsonString = json.encodeToString(
         PersistedDownloadItem(
            url = "https://example.com/file.bin",
            path = "/tmp/file.bin",
            transferredBytes = 512L,
            totalBytes = 1_024L,
            status = DownloadStatus.Paused,
         ),
      )

      assertFalse(jsonString.contains("progress"))
   }

   @Test
   fun `withTransfer tracks bytes and resets progress when total size is unknown`() {
      val item = DownloadItem(
         url = "https://example.com/file.bin",
         path = "/tmp/file.bin",
      )

      val updated = item.withTransfer(1_024L, null)

      assertEquals(1_024L, updated.transferredBytes)
      assertNull(updated.totalBytes)
      assertEquals(0.0, updated.progress, 0.0)
   }

   @Test
   fun `reconcileRecoveredTransfer promotes stale transferred bytes when pausing`() {
      val item = DownloadItem(
         url = "https://example.com/file.bin",
         path = "/tmp/file.bin",
         status = DownloadStatus.InProgress,
      )

      val paused = reconcileRecoveredTransfer(item, 2_048L, DownloadStatus.Paused, null)

      assertEquals(2_048L, paused.transferredBytes)
      assertNull(paused.totalBytes)
      assertEquals(0.0, paused.progress, 0.0)
      assertEquals(DownloadStatus.Paused, paused.status)
   }

   @Test
   fun `reconcileRecoveredTransfer preserves legacy progress when total size is unknown`() {
      val item = DownloadItem(
         url = "https://example.com/file.bin",
         path = "/tmp/file.bin",
         progress = 42.5,
         status = DownloadStatus.InProgress,
      )

      val paused = reconcileRecoveredTransfer(item, 2_048L, DownloadStatus.Paused, null)

      assertEquals(2_048L, paused.transferredBytes)
      assertNull(paused.totalBytes)
      assertEquals(42.5, paused.progress, 0.0)
      assertEquals(DownloadStatus.Paused, paused.status)
   }

   @Test
   fun `withStatus completed infers total bytes from transferred bytes when unknown`() {
      val item = DownloadItem(
         url = "https://example.com/file.bin",
         path = "/tmp/file.bin",
      ).withTransfer(2_048L, null)

      val completed = item.withStatus(DownloadStatus.Completed)

      assertEquals(2_048L, completed.transferredBytes)
      assertEquals(2_048L, completed.totalBytes)
      assertEquals(100.0, completed.progress, 0.0)
      assertEquals(DownloadStatus.Completed, completed.status)
   }
}
