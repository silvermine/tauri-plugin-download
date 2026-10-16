package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class DownloadWorkOwnershipTest {
   private val path = "/downloads/file"
   private val oldRequest = UUID.randomUUID()
   private val newRequest = UUID.randomUUID()
   private val active = DownloadRecord("https://example.com/file", path, status = DownloadStatus.InProgress)

   @Test
   fun `scheduler stop retains ownership for automatic resume and transient retry`() {
      val work = DownloadWorkOwnership()
      work.replace(path, oldRequest)
      assertEquals(DownloadWorker.FailureOutcome.Ignore, outcome(work, oldRequest, stopped = true))
      assertTrue(work.owns(path, oldRequest))
      assertEquals(DownloadStatus.InProgress,
         DownloadWorker.interruptedRecord(active, 123L, work.owns(path, oldRequest))?.status)
      assertEquals(DownloadWorker.FailureOutcome.Retry, outcome(work, oldRequest))
   }

   @Test
   fun `pause followed immediately by resume fences old failures before cancellation arrives`() {
      val work = DownloadWorkOwnership()
      work.replace(path, oldRequest)
      work.revoke(path)
      assertFalse(work.owns(path, oldRequest))
      work.replace(path, newRequest)
      // The store is InProgress again and the old worker has not seen isStopped.
      for (httpStatus in listOf(503, 404, 416)) {
         assertEquals(DownloadWorker.FailureOutcome.Ignore, outcome(work, oldRequest, httpStatus))
      }
      assertNull(DownloadWorker.interruptedRecord(active, 10L, work.owns(path, oldRequest)))
      assertEquals(DownloadWorker.FailureOutcome.Retry, outcome(work, newRequest))
      assertEquals(DownloadWorker.FailureOutcome.Fail, outcome(work, newRequest, 404))
   }

   @Test
   fun `cancel and recreate cannot restore old ownership or interfere with another path`() {
      val work = DownloadWorkOwnership()
      work.replace(path, oldRequest)
      work.replace("/downloads/other", oldRequest)
      work.revoke(path)
      assertFalse(work.owns(path, oldRequest))
      assertFalse(work.owns(path, newRequest))
      assertTrue(work.owns("/downloads/other", oldRequest))
      work.replace(path, newRequest)
      assertFalse(work.owns(path, oldRequest))
      assertTrue(work.owns(path, newRequest))
      work.revoke(path)
      assertFalse(work.owns(path, newRequest))
   }

   @Test
   fun `persisted work is accepted until a live command revokes or replaces it`() {
      val work = DownloadWorkOwnership(listOf(path))
      assertFalse(work.owns("/downloads/unknown", oldRequest))
      assertTrue(work.owns(path, oldRequest))
      work.revoke(path)
      assertFalse(work.owns(path, oldRequest))
      work.replace(path, newRequest)
      assertFalse(work.owns(path, oldRequest))
      assertTrue(work.owns(path, newRequest))
   }

   private fun outcome(
      work: DownloadWorkOwnership, request: UUID, httpStatus: Int = 503, stopped: Boolean = false,
   ): DownloadWorker.FailureOutcome = DownloadWorker.failureOutcome(
      DownloadFailure.http(httpStatus), 1, DownloadStatus.InProgress, stopped, work.owns(path, request),
   )
}
