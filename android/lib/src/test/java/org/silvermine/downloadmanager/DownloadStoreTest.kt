package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Test
import java.io.File

/**
 * Covers the store's decode and encode, which is where a malformed file decides
 * whether one record or every record is lost.
 *
 * The `AtomicFile` wiring around them needs an Android runtime, so it is not
 * exercised here — `android.util.AtomicFile` and `android.util.Log` are throwing
 * stubs in a JVM unit test.
 */
class DownloadStoreTest {

   private fun sampleRecord(path: String, receivedBytes: Long = 0L): DownloadRecord = DownloadRecord(
      url = "http://example.com/$path",
      path = "/tmp/$path",
      receivedBytes = receivedBytes,
      totalBytes = 1000L,
      status = DownloadStatus.Paused,
   )

   // -- Decoding --

   @Test
   fun `decodes persisted records`() {
      val decoded = DownloadStore.decodeRecords(
         """{"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":500,"totalBytes":1000,"status":"paused"}]}"""
      )

      assertEquals(1, decoded.size)
      assertEquals(500L, decoded.first().receivedBytes)
      assertEquals(1000L, decoded.first().totalBytes)
      assertEquals(DownloadStatus.Paused, decoded.first().status)
   }

   @Test
   fun `one unreadable record discards the whole store`() {
      // Records are accepted as a whole; #64 will preserve unreadable files,
      // rather than salvage individual records.
      assertThrows(SerializationException::class.java) {
         DownloadStore.decodeRecords(
            """
            {"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":1,"status":"paused"},
             {"url":"http://example.com/b.mp4","status":"paused"},
             {"url":"http://example.com/c.mp4","path":"/tmp/c.mp4","options":{"allowMetered":true},"receivedBytes":3,"status":"idle"}]}
            """.trimIndent()
         )
      }
   }

   @Test
   fun `a record of the wrong shape fails the decode`() {
      assertThrows(SerializationException::class.java) {
         DownloadStore.decodeRecords("""{"version":1,"downloads":["not an object"]}""")
      }
   }

   @Test
   fun `unknown keys are ignored`() {
      // The store's Json is configured with ignoreUnknownKeys, so a field added by
      // a later version does not cost the record.
      val decoded = DownloadStore.decodeRecords(
         """{"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"idle","somethingNew":42}]}"""
      )

      assertEquals(7L, decoded.first().receivedBytes)
   }

   @Test
   fun `a record without received bytes fails to decode`() {
      // Matches the Rust and Swift records: everything but totalBytes is stated.
      assertThrows(SerializationException::class.java) {
         DownloadStore.decodeRecords(
            """{"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"status":"idle"}]}"""
         )
      }
   }

   @Test
   fun `an omitted total is decoded as null`() {
      // totalBytes stays optional on all three platforms: absent means the server
      // reported no content length.
      val decoded = DownloadStore.decodeRecords(
         """{"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"idle"}]}"""
      )

      assertEquals(7L, decoded.first().receivedBytes)
      assertNull(decoded.first().totalBytes)
   }

   @Test
   fun `an empty v1 store decodes to nothing`() {
      val decoded = DownloadStore.decodeRecords("""{"version":1,"downloads":[]}""")

      assertEquals(0, decoded.size)
   }

   @Test
   fun `text that is not a store envelope is rejected`() {
      // load() catches these and leaves the store empty rather than half-built.
      for (text in listOf("this is not json", """{"url":"http://example.com/a.mp4"}""", "")) {
         assertThrows(SerializationException::class.java) { DownloadStore.decodeRecords(text) }
      }
   }

   // -- Round trip --

   @Test
   fun `rejects legacy arrays and malformed envelopes`() {
      for (text in listOf(
         "[]", "[1, []]", "[{}]", "null", "true", "1", "{}",
         """{"version":1}""",
         """{"downloads":[]}""",
         """{"version":1,"downloads":null}""",
         """{"version":1,"downloads":{}}""",
         """{"version":1,"downloads":"private input"}""",
      )) {
         val error = assertThrows(SerializationException::class.java) {
            DownloadStore.decodeRecords(text)
         }
         assertEquals(text, "Malformed store envelope", error.message)
      }
   }

   @Test
   fun `rejects invalid version types and values`() {
      for (version in listOf(
         "\"1\"", "true", "false", "null", "-1", "1.5", "1.0", "1e0", "+1", "01", "[]", "{}",
      )) {
         val error = assertThrows(SerializationException::class.java) {
            DownloadStore.decodeRecords("""{"version":$version,"downloads":[]}""")
         }
         assertEquals(version, "Malformed store envelope", error.message)
      }
   }

   @Test
   fun `unsupported versions are checked before record decoding`() {
      for (version in listOf(0L, 3L, 4294967295L)) {
         val error = assertThrows(SerializationException::class.java) {
            DownloadStore.decodeRecords("""{"version":$version,"downloads":[{"future":"record"}]}""")
         }
         assertEquals("Unsupported store version: $version (expected 2)", error.message)
      }
   }

   @Test
   fun `unknown envelope fields are ignored`() {
      val decoded = DownloadStore.decodeRecords(
         """{"version":1,"downloads":[],"somethingNew":{"ignored":true}}"""
      )
      assertTrue(decoded.isEmpty())
   }

   @Test
   fun `decoder errors do not expose private input`() {
      val error = assertThrows(SerializationException::class.java) {
         DownloadStore.decodeRecords(
            """{"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"private input"}]}"""
         )
      }
      assertEquals("Invalid store records", error.message)
      assertNull(error.cause)

      val malformed = assertThrows(SerializationException::class.java) {
         DownloadStore.decodeRecords("private input")
      }
      assertEquals("Malformed store envelope", malformed.message)
      assertNull(malformed.cause)
   }

   @Test
   fun `writes both envelope fields even for an empty store`() {
      val encoded = DownloadStore.encodeRecords(emptyList())
      assertEquals(
         Json.parseToJsonElement("""{"version":2,"downloads":[]}"""),
         Json.parseToJsonElement(encoded),
      )
      assertTrue(DownloadStore.decodeRecords(encoded).isEmpty())
   }

   @Test
   fun `encoded records decode back unchanged`() {
      val records = listOf(
         sampleRecord("a.mp4", 10L).copy(options = CreateOptions(allowMetered = false)),
         sampleRecord("b.mp4", 20L),
      )

      val encoded = DownloadStore.encodeRecords(records)
      assertEquals(JsonPrimitive(2), Json.parseToJsonElement(encoded).jsonObject["version"])
      val decoded = DownloadStore.decodeRecords(encoded)

      assertEquals(records, decoded)
   }

   @Test
   fun `a record of every default survives the round trip`() {
      // The store's Json leaves encodeDefaults off, so only @Required properties
      // survive it — the shape production actually writes. totalBytes is the one
      // field that may legitimately be absent, so it alone is dropped.
      val record = DownloadRecord(url = "http://example.com/a.mp4", path = "/tmp/a.mp4")

      val encoded = DownloadStore.encodeRecords(listOf(record))
      val decoded = DownloadStore.decodeRecords(encoded)

      assertTrue(encoded.contains(""""options":{"allowMetered":true}"""))
      assertTrue(encoded.contains(""""receivedBytes":0"""))
      assertTrue(encoded.contains(""""status":"idle"""))
      assertFalse(encoded.contains("totalBytes"))
      assertEquals(listOf(record), decoded)
      assertNull(decoded.first().totalBytes)
   }

   @Test
   fun `the store file is resolved inside the directory it was given`() {
      // The one step on Android that honours a configured directory rather than just
      // carrying it. Hardcode a directory here and no other Kotlin test fails.
      val configured = File("/data/user/0/com.example/files/downloads")

      assertEquals(
         File("/data/user/0/com.example/files/downloads/downloads.json"),
         DownloadStore.storeFile(configured),
      )
   }

   @Test
   fun `two directories resolve to two different store files`() {
      // Pairs with the case above, which a `storeFile` returning a fixed path would
      // still satisfy.
      assertNotEquals(
         DownloadStore.storeFile(File("/data/user/0/com.example/files")),
         DownloadStore.storeFile(File("/data/user/0/com.example/downloads")),
      )
   }
}
