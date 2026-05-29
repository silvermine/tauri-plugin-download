package org.silvermine.downloadmanager

import kotlinx.serialization.decodeFromString
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DownloadItemTest {

   private val json = Json { ignoreUnknownKeys = true }

   @Test
   fun `decode older persisted item defaults byte tracking fields`() {
      val items = json.decodeFromString<List<DownloadItem>>(
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
      )

      val item = items.single()

      assertEquals(0L, item.transferredBytes)
      assertNull(item.totalBytes)
      assertEquals(42.5, item.progress, 0.0)
      assertEquals(DownloadStatus.Paused, item.status)
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
