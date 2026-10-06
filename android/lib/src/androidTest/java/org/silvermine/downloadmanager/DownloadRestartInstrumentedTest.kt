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
               receivedBytes = if (name == "partial") 3L else 0L,
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
         }

         for ((record, request) in records.zip(requests)) {
            val response = executor.submit { respond(server, record.receivedBytes > 0) }
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
            receivedBytes = 3L,
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
               workManager.getWorkInfoById(request.id).get(5, TimeUnit.SECONDS).state)

            // The worker has initialized the singleton, but has not received headers
            // that could overwrite a wrongly reconciled status with InProgress.
            val manager = DownloadManager.getInstance(context, directory)
            errors.checkThat(manager.get(record.path)?.status, equalTo(DownloadStatus.InProgress))
            errors.checkThat(manager.list().single().status, equalTo(DownloadStatus.InProgress))
            releaseResponse.countDown()
            response.get(15, TimeUnit.SECONDS)
            assertCompleted(request, record, manager)
         } finally {
            releaseResponse.countDown()
         }
      }
   }

   /** Holds a restored request until the test permits its next attempt. */
   private fun enqueue(record: DownloadRecord): OneTimeWorkRequest =
      OneTimeWorkRequestBuilder<DownloadWorker>()
         .setInputData(DownloadManager.inputDataFor(record, null, directory))
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

   /** Waits for the worker result and verifies the completed file and public store. */
   private fun assertCompleted(request: OneTimeWorkRequest, record: DownloadRecord, manager: DownloadManager) {
      val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(15)
      while (!workManager.getWorkInfoById(request.id).get(5, TimeUnit.SECONDS).state.isFinished &&
         System.nanoTime() < deadline) {
         Thread.sleep(20)
      }
      assertEquals(WorkInfo.State.SUCCEEDED,
         workManager.getWorkInfoById(request.id).get(5, TimeUnit.SECONDS).state)
      assertTrue(File(record.path).exists())
      assertEquals("abcdef", File(record.path).readText())
      assertNull(manager.get(record.path))
   }
}
