package org.silvermine.downloadmanager

import android.annotation.SuppressLint
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Configuration
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequest
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.workDataOf
import androidx.work.impl.WorkManagerImpl
import androidx.work.testing.TestDriver
import androidx.work.testing.WorkManagerTestInitHelper
import org.hamcrest.CoreMatchers.equalTo
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.ErrorCollector
import org.junit.runner.RunWith
import java.io.File
import java.net.ServerSocket
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Recreates persisted downloads before letting their surviving work run again. */
@RunWith(AndroidJUnit4::class)
class DownloadRestartInstrumentedTest {
   @get:Rule
   val errors = ErrorCollector()

   private val context = InstrumentationRegistry.getInstrumentation().targetContext
   private val managerInstance = DownloadManager::class.java.getDeclaredField("instance").apply {
      isAccessible = true
   }
   private var previousManager: Any? = null
   private lateinit var previousWorkManager: WorkManagerImpl
   private lateinit var directory: File
   private lateinit var executor: ExecutorService
   private lateinit var workManager: WorkManager
   private lateinit var driver: TestDriver

   /** Isolates process-wide singletons so either test can initialize the manager first. */
   @Before
   @SuppressLint("RestrictedApi")
   fun setUp() {
      previousManager = managerInstance.get(null)
      previousWorkManager = WorkManagerImpl.getInstance(context)
      managerInstance.set(null, null)
      directory = File(context.filesDir, "restart-test-${UUID.randomUUID()}")
      executor = Executors.newCachedThreadPool()
      WorkManagerTestInitHelper.initializeTestWorkManager(
         context, Configuration.Builder().setExecutor(executor).build(),
      )
      workManager = WorkManager.getInstance(context)
      driver = requireNotNull(WorkManagerTestInitHelper.getTestDriver(context))
   }

   @After
   @SuppressLint("RestrictedApi")
   fun tearDown() {
      try {
         workManager.cancelAllWork().result.get(10, TimeUnit.SECONDS)
         executor.shutdownNow()
         assertTrue(executor.awaitTermination(15, TimeUnit.SECONDS))
         WorkManagerTestInitHelper.closeWorkDatabase()
      } finally {
         // closeWorkDatabase does not reset WorkManager's delegate. Restore both
         // singletons rather than leaving them attached to deleted test resources.
         WorkManagerImpl.setDelegate(previousWorkManager)
         managerInstance.set(null, previousManager)
         directory.deleteRecursively()
      }
   }

   @Test
   fun survivingWorkContinuesWithoutReportingPausedOrIdle() {
      ServerSocket(0).use { server ->
         server.soTimeout = 15000
         val store = DownloadStore(directory)
         val records = listOf("partial", "fresh").map { name ->
            DownloadRecord(
               url = "http://127.0.0.1:${server.localPort}/$name",
               path = File(directory, name).path,
               receivedBytes = if (name == "partial") 0L else 2L,
               totalBytes = 6L,
               status = DownloadStatus.InProgress,
            ).also { store.append(it) }
         }
         File("${records.first().path}${DownloadWorker.DOWNLOAD_SUFFIX}").writeText("abc")
         val requests = records.map { enqueue(it) }

         // A canceled job and a missing job must still recover from local bytes.
         val canceled = records.first().copy(path = File(directory, "canceled").path)
         val orphan = records.last().copy(path = File(directory, "orphan").path)
         store.append(canceled)
         store.append(orphan)
         File("${canceled.path}${DownloadWorker.DOWNLOAD_SUFFIX}").writeText("ab")
         val canceledRequest = OneTimeWorkRequestBuilder<DownloadWorker>()
            .addTag(DownloadManager.STORE_TAG)
            .setInitialDelay(1, TimeUnit.DAYS).build()
         workManager.enqueueUniqueWork(
            "download_manager:${canceled.path}", ExistingWorkPolicy.REPLACE, canceledRequest,
         ).result.get(10, TimeUnit.SECONDS)
         workManager.cancelWorkById(canceledRequest.id).result.get(10, TimeUnit.SECONDS)

         // The app initializes the manager before WorkManager releases pending work.
         val manager = DownloadManager.getInstance(context, directory)
         assertEquals(DownloadStatus.Paused, manager.get(canceled.path)?.status)
         assertEquals(2L, manager.get(canceled.path)?.receivedBytes)
         assertEquals(DownloadStatus.Idle, manager.get(orphan.path)?.status)
         assertEquals(0L, manager.get(orphan.path)?.receivedBytes)
         for (record in records) {
            // Collect failures so the baseline still proves the workers resume.
            errors.checkThat(manager.get(record.path)?.status, equalTo(DownloadStatus.InProgress))
            errors.checkThat(manager.list().single { it.path == record.path }.status,
               equalTo(DownloadStatus.InProgress))
            val measuredBytes = if (record == records.first()) 3L else 0L
            errors.checkThat(manager.get(record.path)?.receivedBytes, equalTo(measuredBytes))
            errors.checkThat(manager.list().single { it.path == record.path }.receivedBytes,
               equalTo(measuredBytes))
            assertEquals(measuredBytes, DownloadStore(directory).findByPath(record.path)?.receivedBytes)
         }

         for ((record, request) in records.zip(requests)) {
            val response = executor.submit { respond(server, record == records.first()) }
            driver.setInitialDelayMet(request.id)
            response.get(15, TimeUnit.SECONDS)
            assertCompleted(request, record, manager)
         }
      }
   }

   @Test
   fun restoredWorkerCanInitializeManagerBeforeTheApp() {
      ServerSocket(0).use { server ->
         server.soTimeout = 15000
         val record = DownloadRecord(
            url = "http://127.0.0.1:${server.localPort}/partial",
            path = File(directory, "partial").path,
            receivedBytes = 0L,
            totalBytes = 6L,
            status = DownloadStatus.InProgress,
         )
         DownloadStore(directory).append(record)
         File("${record.path}${DownloadWorker.DOWNLOAD_SUFFIX}").writeText("abc")
         val request = enqueue(record)
         val requested = CountDownLatch(1)
         val releaseResponse = CountDownLatch(1)
         val response = executor.submit {
            respond(server, partial = true) {
               requested.countDown()
               assertTrue(releaseResponse.await(15, TimeUnit.SECONDS))
            }
         }

         try {
            driver.setInitialDelayMet(request.id)
            assertTrue("Worker did not reach the server", requested.await(15, TimeUnit.SECONDS))
            assertEquals(WorkInfo.State.RUNNING,
               workInfo(request.id).state)

            // The worker has initialized the singleton, but has not received headers
            // that could overwrite a wrongly reconciled status with InProgress.
            val manager = DownloadManager.getInstance(context, directory)
            errors.checkThat(manager.get(record.path)?.status, equalTo(DownloadStatus.InProgress))
            errors.checkThat(manager.list().single().status, equalTo(DownloadStatus.InProgress))
            errors.checkThat(manager.get(record.path)?.receivedBytes, equalTo(3L))
            releaseResponse.countDown()
            response.get(15, TimeUnit.SECONDS)
            assertCompleted(request, record, manager)
         } finally {
            releaseResponse.countDown()
         }
      }
   }

   @Test
   fun newlyStartedDownloadCarriesStoreCompatibilityTag() {
      val manager = DownloadManager.getInstance(context, directory)
      val path = File(directory, "started").path
      manager.create(path, "http://127.0.0.1/unused")
      // TestDriver holds the network constraint, so no HTTP request is made.
      manager.start(path)
      val info = workManager.getWorkInfosForUniqueWork("download_manager:$path").get(5, TimeUnit.SECONDS).single()
      assertTrue(DownloadManager.STORE_TAG in info.tags)
      assertEquals(WorkInfo.State.ENQUEUED, info.state)
   }

   @Test
   fun startAppliesBothStoredNetworkPoliciesToQueuedWork() {
      for (allowed in listOf(false, true)) {
         val manager = DownloadManager.getInstance(context, directory)
         val path = File(directory, "policy-start-$allowed").path
         manager.create(path, "https://example.com/file", CreateOptions(allowMetered = allowed))
         manager.start(path)
         val info = workManager.getWorkInfosForUniqueWork("download_manager:$path")
            .get(5, TimeUnit.SECONDS).single()
         assertEquals(if (allowed) androidx.work.NetworkType.CONNECTED else androidx.work.NetworkType.UNMETERED,
            info.constraints.requiredNetworkType)
         assertEquals(allowed, manager.get(path)!!.options.allowMetered)
      }
   }

   @Test
   fun resumeAppliesBothStoredNetworkPoliciesToReplacementWork() {
      for (allowed in listOf(false, true)) {
         val manager = DownloadManager.getInstance(context, directory)
         val path = File(directory, "policy-resume-$allowed").path
         manager.create(path, "https://example.com/file", CreateOptions(allowMetered = allowed))
         manager.start(path)
         val previous = workManager.getWorkInfosForUniqueWork("download_manager:$path")
            .get(5, TimeUnit.SECONDS).single().id
         manager.pause(path)
         workManager.cancelUniqueWork("download_manager:$path").result.get(5, TimeUnit.SECONDS)
         manager.resume(path)
         val replacement = workManager.getWorkInfosForUniqueWork("download_manager:$path")
            .get(5, TimeUnit.SECONDS).single { it.id != previous && !it.state.isFinished }
         assertEquals(if (allowed) androidx.work.NetworkType.CONNECTED else androidx.work.NetworkType.UNMETERED,
            replacement.constraints.requiredNetworkType)
         assertEquals(allowed, manager.get(path)!!.options.allowMetered)
      }
   }

   @Test
   fun untaggedWorkIsCanceledWithOrWithoutStoreDirectory() {
      val store = DownloadStore(directory)
      val records = listOf("legacy-partial", "legacy-fresh", "compatible-partial", "compatible-fresh").map { name ->
         DownloadRecord(
            url = "http://127.0.0.1/unused",
            path = File(directory, name).path,
            receivedBytes = 1L,
            totalBytes = 6L,
            status = DownloadStatus.InProgress,
         ).also { store.append(it) }
      }
      for (record in records.filter { it.path.endsWith("partial") }) {
         File("${record.path}${DownloadWorker.DOWNLOAD_SUFFIX}").writeText("abc")
      }
      val requests = records.map { record ->
         OneTimeWorkRequestBuilder<DownloadWorker>()
            .setInputData(if (File(record.path).name.startsWith("compatible")) {
               DownloadManager.inputDataFor(record, null, directory)
            } else {
               workDataOf(DownloadWorker.KEY_URL to record.url, DownloadWorker.KEY_PATH to record.path)
            })
            .setInitialDelay(1, TimeUnit.DAYS)
            .build().also {
               workManager.enqueueUniqueWork("download_manager:${record.path}", ExistingWorkPolicy.REPLACE, it)
                  .result.get(10, TimeUnit.SECONDS)
            }
      }

      val manager = DownloadManager.getInstance(context, directory)
      for ((index, record) in records.withIndex()) {
         val partial = record.path.endsWith("partial")
         val expectedStatus = if (partial) DownloadStatus.Paused else DownloadStatus.Idle
         assertEquals(expectedStatus, manager.get(record.path)?.status)
         assertEquals(if (partial) 3L else 0L, manager.get(record.path)?.receivedBytes)
         assertEquals(WorkInfo.State.CANCELLED,
            workInfo(requests[index].id).state)
         if (partial) assertEquals("abc", File("${record.path}${DownloadWorker.DOWNLOAD_SUFFIX}").readText())
         // Explicit user action creates tagged replacement work, held by TestDriver's
         // network constraint until a later test permits it to run.
         if (partial) manager.resume(record.path) else manager.start(record.path)
         val replacement = workManager.getWorkInfosForUniqueWork("download_manager:${record.path}")
            .get(5, TimeUnit.SECONDS).single { !it.state.isFinished }
         assertTrue(DownloadManager.STORE_TAG in replacement.tags)
      }
   }

   @Test
   fun untaggedWorkerThatInitializesManagerCannotStartTransferring() {
      ServerSocket(0).use { server ->
         val record = DownloadRecord(
            url = "http://127.0.0.1:${server.localPort}/partial",
            path = File(directory, "partial").path,
            status = DownloadStatus.InProgress,
         )
         DownloadStore(directory).append(record)
         val partial = File("${record.path}${DownloadWorker.DOWNLOAD_SUFFIX}")
         partial.writeText("abc")
         val request = OneTimeWorkRequestBuilder<DownloadWorker>()
            .setInputData(DownloadManager.inputDataFor(record, null, directory))
            .setInitialDelay(1, TimeUnit.DAYS).build()
         workManager.enqueueUniqueWork("download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request)
            .result.get(10, TimeUnit.SECONDS)
         driver.setInitialDelayMet(request.id)
         val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(15)
         while (!workInfo(request.id).state.isFinished &&
            System.nanoTime() < deadline) {
            Thread.sleep(20)
         }
         assertEquals(WorkInfo.State.CANCELLED,
            workInfo(request.id).state)
         val manager = DownloadManager.getInstance(context, directory)
         assertEquals(DownloadStatus.Paused, manager.get(record.path)?.status)
         assertEquals(3L, manager.get(record.path)?.receivedBytes)
         assertEquals("abc", partial.readText())
         server.soTimeout = 200
         try {
            server.accept().use { org.junit.Assert.fail("Canceled worker opened an HTTP connection") }
         } catch (_: java.net.SocketTimeoutException) {
            // No request was made, including by a worker that initialized the manager.
         }
      }
   }

   /** Holds a restored request until the test permits its next attempt. */
   private fun enqueue(record: DownloadRecord): OneTimeWorkRequest =
      OneTimeWorkRequestBuilder<DownloadWorker>()
         .setInputData(DownloadManager.inputDataFor(record, null, directory))
         .addTag(DownloadManager.STORE_TAG)
         .setInitialRunAttemptCount(1)
         .setInitialDelay(1, TimeUnit.DAYS)
         .build().also { request ->
            workManager.enqueueUniqueWork(
               "download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request,
            ).result.get(10, TimeUnit.SECONDS)
         }

   /** Serves the real worker, optionally holding headers to expose startup state. */
   private fun respond(server: ServerSocket, partial: Boolean, beforeHeaders: () -> Unit = {}) {
      server.accept().use { socket ->
         socket.soTimeout = 10000
         val reader = socket.getInputStream().bufferedReader()
         val headers = generateSequence { reader.readLine() }.takeWhile { it.isNotEmpty() }.toList()
         assertEquals(partial, headers.any { it.equals("Range: bytes=3-", ignoreCase = true) })
         beforeHeaders()
         val body = if (partial) "def" else "abcdef"
         val status = if (partial) "206 Partial Content" else "200 OK"
         val range = if (partial) "Content-Range: bytes 3-5/6\r\n" else ""
         socket.getOutputStream().write(
            ("HTTP/1.1 $status\r\n${range}Content-Length: ${body.length}\r\nConnection: close\r\n\r\n$body").toByteArray(),
         )
      }
   }

   /**
    * Looks up a request already enqueued by the test, failing clearly if it is absent.
    *
    * @param id The enqueued request's ID.
    * @return The request's current work information.
    */
   private fun workInfo(id: UUID): WorkInfo =
      requireNotNull(workManager.getWorkInfoById(id).get(5, TimeUnit.SECONDS)) {
         "Missing work request $id"
      }

   /** Waits for the worker result and verifies the completed file and public store. */
   private fun assertCompleted(request: OneTimeWorkRequest, record: DownloadRecord, manager: DownloadManager) {
      val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(15)
      while (!workInfo(request.id).state.isFinished &&
         System.nanoTime() < deadline) {
         Thread.sleep(20)
      }
      assertEquals(WorkInfo.State.SUCCEEDED,
         workInfo(request.id).state)
      assertTrue(File(record.path).exists())
      assertEquals("abcdef", File(record.path).readText())
      assertNull(manager.get(record.path))
   }
}
