package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadWorkerTest {

   // Recovery decisions run without a worker; runAttemptCount counts prior runs.

   @Test
   fun `a download has attempts left up to the cap`() {
      for (runAttemptCount in 0..4) {
         assertFalse(
            "run reporting $runAttemptCount should still have attempts",
            DownloadWorker.isOutOfAttempts(runAttemptCount),
         )
      }
   }

   @Test
   fun `a download is out of attempts once five are spent`() {
      // Above the cap is reachable, not merely defensive: a constraint interruption
      // increments the count without ever consulting the cap.
      assertTrue(DownloadWorker.isOutOfAttempts(5))
      assertTrue(DownloadWorker.isOutOfAttempts(6))
   }

   // -- Request construction --

   @Test
   fun `a configured user agent is set on the request`() {
      val request = DownloadWorker.requestFor("https://example.com/f.bin", "my-app/1.0", 0L)

      assertEquals("my-app/1.0", request.header("User-Agent"))
   }

   @Test
   fun `no user agent leaves the header unset`() {
      // Absent rather than empty: OkHttp then sends its own default.
      val request = DownloadWorker.requestFor("https://example.com/f.bin", null, 0L)

      assertNull(request.header("User-Agent"))
   }

   @Test
   fun `a fresh download sends no range header`() {
      // The common path, and the boundary of the resume condition: without this,
      // widening `downloadedSize > 0` to `>= 0` changes no test outcome.
      val request = DownloadWorker.requestFor("https://example.com/f.bin", "my-app/1.0", 0L)

      assertNull(request.header("Range"))
   }

   @Test
   fun `the user agent and range headers coexist on a resume`() {
      // Mirrors the Rust test_user_agent_and_range_header_are_both_sent_on_resume:
      // neither header may displace the other.
      val request = DownloadWorker.requestFor("https://example.com/f.bin", "my-app/1.0", 4L)

      assertEquals("my-app/1.0", request.header("User-Agent"))
      assertEquals("bytes=4-", request.header("Range"))
   }

   // -- Total size --

   @Test
   fun `an unstated content length has no total`() {
      // OkHttp's -1. The download runs on the coarse byte cadence and reports
      // indeterminate progress.
      assertNull(DownloadWorker.totalSizeFor(-1L, 0L))
      assertNull(DownloadWorker.totalSizeFor(-1L, 512L))
   }

   @Test
   fun `a stated zero content length is a known total`() {
      // An empty body is a complete download, not one of unknown length. Desktop
      // reports 0 for the same response, and collapsing it to null disagreed.
      assertEquals(0L, DownloadWorker.totalSizeFor(0L, 0L))
   }

   @Test
   fun `a content length that overflows the sum has no total`() {
      // The header is the server's to choose. Without the guard the wrapped Long
      // reaches the caller as a negative total.
      assertNull(DownloadWorker.totalSizeFor(Long.MAX_VALUE, 1L))
   }

   @Test
   fun `a resumed download adds the bytes already held`() {
      // The Range response counts only what is left to send.
      assertEquals(1000L, DownloadWorker.totalSizeFor(600L, 400L))
   }

   // -- Resume failure outcome --

   @Test
   fun `a 416 stating a total equal to the partial completes it`() {
      assertEquals(
         DownloadWorker.PartialFileOutcome.Complete,
         DownloadWorker.partialFileOutcomeFor(416, "bytes */1000", 1000L),
      )
   }

   @Test
   fun `any other 416 discards the partial`() {
      for (contentRange in listOf(null, "bytes */999", "bytes 0-499/1000", "1000", "bytes */abc")) {
         assertEquals(
            "Content-Range $contentRange",
            DownloadWorker.PartialFileOutcome.Discard,
            DownloadWorker.partialFileOutcomeFor(416, contentRange, 1000L),
         )
      }
   }

   @Test
   fun `other failures on a resume keep the partial`() {
      for (responseCode in listOf(503, 500, 404, 403)) {
         assertEquals(
            "HTTP $responseCode",
            DownloadWorker.PartialFileOutcome.KeepPartial,
            DownloadWorker.partialFileOutcomeFor(responseCode, "bytes */1000", 1000L),
         )
      }
   }

   @Test
   fun `only active transient failures with attempts left retry`() {
      val transient = DownloadFailure.http(503)
      for (attempt in 0..4) {
         assertEquals(DownloadWorker.FailureOutcome.Retry,
            DownloadWorker.failureOutcome(transient, attempt, DownloadStatus.InProgress, false))
      }
      for (attempt in 5..6) {
         assertEquals(DownloadWorker.FailureOutcome.Fail,
            DownloadWorker.failureOutcome(transient, attempt, DownloadStatus.InProgress, false))
      }
      for (failure in listOf(DownloadFailure.http(404), DownloadFailure.network(java.net.ConnectException()))) {
         assertEquals(DownloadWorker.FailureOutcome.Fail,
            DownloadWorker.failureOutcome(failure, 0, DownloadStatus.InProgress, false))
      }
   }

   @Test
   fun `stopped work reverts and inactive records cannot fail or retry`() {
      for (failure in listOf(DownloadFailure.http(503), DownloadFailure.http(404))) {
         assertEquals(DownloadWorker.FailureOutcome.Revert,
            DownloadWorker.failureOutcome(failure, 5, DownloadStatus.InProgress, true))
         for (status in DownloadStatus.entries.filter { it != DownloadStatus.InProgress } + listOf(null)) {
            for (stopped in listOf(false, true)) {
               assertEquals(DownloadWorker.FailureOutcome.Ignore,
                  DownloadWorker.failureOutcome(failure, 0, status, stopped))
            }
         }
      }
   }

   @Test
   fun `request retries use the public classification and their own limit`() {
      for (status in listOf(408, 429, 500, 502, 503, 504, 507)) {
         for (attempt in 0..2) assertTrue(DownloadWorker.shouldRetryRequest(DownloadFailure.http(status), attempt))
         assertFalse(DownloadWorker.shouldRetryRequest(DownloadFailure.http(status), 3))
      }
      for (status in listOf(401, 403, 404, 501, 505)) {
         assertFalse(DownloadWorker.shouldRetryRequest(DownloadFailure.http(status), 0))
      }
      assertFalse(DownloadWorker.shouldRetryRequest(DownloadFailure.network(java.net.ConnectException()), 0))
   }

   @Test
   fun `worker exception mapping preserves filesystem and store boundaries`() {
      val file = DownloadFailure("file", "write failed", "permanent")
      assertEquals(file, DownloadWorker.failureFor(TransferException(file)))
      assertEquals("store", DownloadWorker.failureFor(DownloadException.Store(java.io.IOException())).code)
      assertEquals("file", DownloadWorker.failureFor(SecurityException()).code)
      assertEquals("timeout", DownloadWorker.failureFor(java.net.SocketTimeoutException()).code)
      assertEquals("unknown", DownloadWorker.failureFor(IllegalArgumentException()).code)
      assertFalse(DownloadWorker.shouldRetryRequest(DownloadWorker.failureFor(java.io.InterruptedIOException()), 0))
   }
}
