package org.silvermine.downloadmanager

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Exercises the real AtomicFile and store initialization, which JVM tests cannot. */
@RunWith(AndroidJUnit4::class)
class DownloadStoreInstrumentedTest {
   private lateinit var directory: File

   @Before
   fun setUp() {
      val context = InstrumentationRegistry.getInstrumentation().targetContext
      directory = File(context.filesDir, "store-schema-test-${UUID.randomUUID()}")
   }

   @After
   fun tearDown() {
      directory.deleteRecursively()
   }

   private fun sampleRecord() = DownloadRecord(
      url = "https://example.com/file.mp4",
      path = "/tmp/file.mp4",
      options = CreateOptions(allowMetered = false),
      receivedBytes = 123,
      totalBytes = 456,
      status = DownloadStatus.Paused,
   )

   @Test
   fun firstWriteCreatesConfiguredDirectoryAndReloadsV2() {
      val nested = File(directory, "nested/store")
      val store = DownloadStore(nested)
      assertTrue(store.list().isEmpty())
      assertFalse(DownloadStore.storeFile(nested).exists())

      val record = sampleRecord()
      store.append(record)
      assertEquals(listOf(record), DownloadStore(nested).list())
      assertEquals(
         DownloadStore.encodeRecords(listOf(record)),
         DownloadStore.storeFile(nested).readText(),
      )

      store.remove(record)
      assertEquals("{\"version\":2,\"downloads\":[]}", DownloadStore.storeFile(nested).readText())
      assertTrue(DownloadStore(nested).list().isEmpty())
   }

   @Test
   fun rejectedDocumentsStayUntouchedUntilALaterSave() {
      assertTrue(directory.mkdirs())
      val file = DownloadStore.storeFile(directory)
      for (text in listOf("[]", "not json", """{"version":3,"downloads":[]}""")) {
         file.writeText(text)
         val store = DownloadStore(directory)
         assertTrue(store.list().isEmpty())
         assertEquals(text, file.readText())

         // This existing overwrite behavior is deliberately left to issue #64.
         store.append(sampleRecord())
         assertEquals(listOf(sampleRecord()), DownloadStore(directory).list())
      }
   }

   @Test
   fun atomicFileRecoversVersionedBackupBeforeDecoding() {
      assertTrue(directory.mkdirs())
      val file = DownloadStore.storeFile(directory)
      val backup = File(file.path + ".bak")
      val records = listOf(sampleRecord())
      backup.writeText(DownloadStore.encodeRecords(records))
      file.writeText("interrupted replacement")

      assertEquals(records, DownloadStore(directory).list())
      assertFalse(backup.exists())
      assertEquals(DownloadStore.encodeRecords(records), file.readText())
   }
   @Test
   fun failedWritesRollBackCommandMutationsButKeepCompletionVisible() {
      val store = DownloadStore(directory)
      val record = sampleRecord()
      val second = record.copy(path = "/tmp/other.mp4")
      store.append(record)
      store.append(second)
      val original = store.list()
      // A regular file blocks AtomicFile.startWrite without relying on disk capacity.
      directory.deleteRecursively()
      directory.writeText("not a directory")
      for (mutation in listOf<() -> Unit>(
         { store.append(record.copy(path = "/tmp/new.mp4")) },
         { store.append(record.withStatus(DownloadStatus.InProgress)) },
         { store.update(record.withStatus(DownloadStatus.InProgress)) },
         { store.update(original.map { it.withStatus(DownloadStatus.InProgress) }) },
         { store.remove(record) },
      )) {
         try {
            mutation()
            org.junit.Assert.fail("Expected a store failure")
         } catch (_: DownloadException.Store) {
            assertEquals(original, store.list())
         }
      }
      // Transfer completion is already a fact. Its final event must still be reachable.
      val completed = record.withStatus(DownloadStatus.Completed)
      val events = mutableListOf<DownloadRecord>()
      store.recordCompletion(completed) { events.add(it) }
      assertEquals(listOf(second), store.list())
      assertEquals(DownloadStatus.Completed, events.single().status)
   }
   @Test
   fun reconciliationKeepsRecoveredRecordsWhenSavingFails() {
      val storeDirectory = File(directory, "store")
      val store = DownloadStore(storeDirectory)
      val partial = File(directory, "partial.mp4.download")
      val active = sampleRecord().copy(path = File(directory, "partial.mp4").path, status = DownloadStatus.InProgress)
      val empty = active.copy(path = File(directory, "empty.mp4").path)
      store.append(active)
      store.append(empty)
      partial.writeText("abc")
      storeDirectory.deleteRecursively()
      storeDirectory.writeText("block persistence")

      DownloadManager.reconcileStoreOnInit(store)

      assertEquals(DownloadStatus.Paused, store.findByPath(active.path)?.status)
      assertEquals(3L, store.findByPath(active.path)?.receivedBytes)
      assertEquals(DownloadStatus.Idle, store.findByPath(empty.path)?.status)
      assertEquals(0L, store.findByPath(empty.path)?.receivedBytes)
      assertEquals("abc", partial.readText())
      // A later successful command persists the recovery that remained in memory.
      storeDirectory.delete()
      store.append(sampleRecord())
      assertEquals(store.list(), DownloadStore(storeDirectory).list())
   }

   @Test
   fun failedRecordWithoutErrorDoesNotLoadOtherRecordsOrRewriteStore() {
      directory.mkdirs()
      val file = DownloadStore.storeFile(directory)
      for (errorField in listOf("", ",\"error\":null")) {
         val text = """{"version":2,"downloads":[{"url":"https://example.com/good","path":"/tmp/good","options":{"allowMetered":true},"receivedBytes":0,"status":"idle"},{"url":"https://example.com/bad","path":"/tmp/bad","options":{"allowMetered":true},"receivedBytes":0,"status":"failed"$errorField}]}"""
         file.writeText(text)
         assertTrue(DownloadStore(directory).list().isEmpty())
         assertEquals(text, file.readText())
      }
   }
}
