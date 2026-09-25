package org.silvermine.downloadmanager

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.io.IOException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.security.cert.CertificateException
import javax.net.ssl.SSLHandshakeException

class DownloadFailureTest {
   @Test
   fun `HTTP classification matches shared fixture`() {
      val cases = Json.parseToJsonElement(File("../../fixtures/http-errors.json").readText()).jsonArray
      for (case in cases) {
         val status = case.jsonObject.getValue("status").jsonPrimitive.int
         val error = DownloadFailure.http(status)
         assertEquals("http", error.code)
         assertEquals(status, error.httpStatus)
         assertEquals(case.jsonObject.getValue("retryability").jsonPrimitive.content, error.retryability)
      }
   }

   @Test
   fun `native causes never use message text to decide retryability`() {
      val certificate = SSLHandshakeException("timeout").apply { initCause(CertificateException("bad issuer")) }
      for ((native, expected) in listOf(
         SocketTimeoutException("Connect timed out") to ("timeout" to "transient"),
         UnknownHostException("timeout") to ("connection" to "unknown"),
         IOException("lost connection") to ("connection" to "transient"),
         certificate to ("tls" to "permanent"),
         Exception("HTTP 404") to ("unknown" to "unknown"),
      )) {
         val error = DownloadFailure.network(native)
         assertEquals(expected.first, error.code)
         assertEquals(expected.second, error.retryability)
      }
      assertEquals("permanent", DownloadFailure.file(SecurityException("timeout")).retryability)
      assertEquals("unknown", DownloadFailure.file(IOException("disk full")).retryability)
   }

   @Test
   fun `failed partial and error round trip and resume clears the error`() {
      val active = DownloadRecord("https://example.com/file", "/tmp/file", status = DownloadStatus.InProgress)
      val failure = DownloadFailure.http(503)
      val failed = active.failed(failure, 123L)!!
      val reloaded = DownloadStore.decodeRecords(DownloadStore.encodeRecords(listOf(failed))).single()
      assertEquals(failed, reloaded)
      assertEquals(123L, reloaded.receivedBytes)
      assertEquals(failure, reloaded.toItem().error)
      assertNull(DownloadManager.revertInProgress(reloaded, 123L))
      assertNull(reloaded.withStatus(DownloadStatus.InProgress).error)
      assertNull(reloaded.withStatus(DownloadStatus.Canceled).error)
      assertNull(active.withStatus(DownloadStatus.Paused).failed(failure, 123L))
      assertNull(active.withStatus(DownloadStatus.Canceled).failed(failure, 123L))
   }

   @Test
   fun `v1 records load without errors and write as v2`() {
      val text = """{"version":1,"downloads":[{"url":"https://example.com/file","path":"/tmp/file","options":{"allowMetered":false},"receivedBytes":123,"status":"paused"}]}"""
      val records = DownloadStore.decodeRecords(text)
      assertNull(records.single().error)
      assertEquals(123L, records.single().receivedBytes)
      assertEquals(2, Json.parseToJsonElement(DownloadStore.encodeRecords(records)).jsonObject.getValue("version").jsonPrimitive.int)
   }
}
