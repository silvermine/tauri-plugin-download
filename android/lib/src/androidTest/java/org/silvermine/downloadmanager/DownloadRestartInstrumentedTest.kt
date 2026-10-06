package org.silvermine.downloadmanager

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Configuration
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.WorkManagerTestInitHelper
import org.hamcrest.CoreMatchers.equalTo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.ErrorCollector
import org.junit.runner.RunWith
import java.io.File
import java.net.ServerSocket
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Recreates persisted downloads before letting their surviving work run again. */
@RunWith(AndroidJUnit4::class)
class DownloadRestartInstrumentedTest {
   @get:Rule
   val errors = ErrorCollector()

   @Test
   fun survivingWorkContinuesWithoutReportingPausedOrIdle() {
      val context = InstrumentationRegistry.getInstrumentation().targetContext
      val directory = File(context.filesDir, "restart-test-${UUID.randomUUID()}")
      val executor = Executors.newCachedThreadPool()
      WorkManagerTestInitHelper.initializeTestWorkManager(
         context, Configuration.Builder().setExecutor(executor).build(),
      )
      val workManager = WorkManager.getInstance(context)
      val driver = requireNotNull(WorkManagerTestInitHelper.getTestDriver(context))

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
         // Delay holds the persisted work until after initialization and get/list.
         val requests = records.map { record ->
            OneTimeWorkRequestBuilder<DownloadWorker>()
               .setInputData(DownloadManager.inputDataFor(record, null, directory))
               .setInitialRunAttemptCount(1)
               .setInitialDelay(1, TimeUnit.DAYS)
               .build().also { request ->
                  workManager.enqueueUniqueWork(
                     "download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request,
                  ).result.get(10, TimeUnit.SECONDS)
               }
         }

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

         try {
            // This is the first manager in the process, as after an abrupt exit.
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
               val response = executor.submit {
                  server.accept().use { socket ->
                     socket.soTimeout = 10000
                     val reader = socket.getInputStream().bufferedReader()
                     val headers = generateSequence { reader.readLine() }.takeWhile { it.isNotEmpty() }.toList()
                     val partial = record.receivedBytes > 0
                     assertEquals(partial, headers.any { it.equals("Range: bytes=3-", ignoreCase = true) })
                     val body = if (partial) "def" else "abcdef"
                     val status = if (partial) "206 Partial Content" else "200 OK"
                     val range = if (partial) "Content-Range: bytes 3-5/6\r\n" else ""
                     socket.getOutputStream().write(
                        ("HTTP/1.1 $status\r\n${range}Content-Length: ${body.length}\r\nConnection: close\r\n\r\n$body").toByteArray(),
                     )
                  }
               }
               driver.setInitialDelayMet(request.id)
               response.get(15, TimeUnit.SECONDS)
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
         } finally {
            for (request in requests) workManager.cancelWorkById(request.id).result.get(10, TimeUnit.SECONDS)
            WorkManagerTestInitHelper.closeWorkDatabase()
            executor.shutdownNow()
            directory.deleteRecursively()
         }
      }
   }
}
