package org.silvermine.downloadmanager

import android.annotation.SuppressLint
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Clock
import androidx.work.Configuration
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundUpdater
import androidx.work.ListenableWorker
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.impl.WorkManagerImpl
import androidx.work.impl.utils.futures.SettableFuture
import androidx.work.testing.TestListenableWorkerBuilder
import androidx.work.testing.WorkManagerTestInitHelper
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.IOException
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * Runs the real worker against loopback HTTP with deterministic response barriers.
 * Direct workers test stop/command races; scheduled work tests WorkManager results.
 * Physical network constraint tracking still requires a separate example-app test.
 */
@RunWith(AndroidJUnit4::class)
@SuppressLint("RestrictedApi")
class DownloadLiveControlInstrumentedTest {
   private val context = InstrumentationRegistry.getInstrumentation().targetContext
   private val singleton = DownloadManager::class.java.getDeclaredField("instance").apply { isAccessible = true }
   private var previousManager: Any? = null
   private lateinit var previousWorkManager: WorkManagerImpl
   private lateinit var workManager: WorkManager
   private lateinit var manager: DownloadManager
   private lateinit var directory: File
   private lateinit var storeDirectory: File
   private val executor = Executors.newCachedThreadPool()
   private val server = ServerSocket(0).apply { soTimeout = 15000 }
   private val clock = AtomicLong(System.currentTimeMillis())
   private val gates = mutableListOf<CountDownLatch>()

   @Before
   fun setUp() {
      previousManager = singleton.get(null)
      previousWorkManager = WorkManagerImpl.getInstance(context)
      singleton.set(null, null)
      directory = File(context.filesDir, "live-control-${UUID.randomUUID()}")
      storeDirectory = File(directory, "store")
      WorkManagerTestInitHelper.initializeTestWorkManager(
         context, Configuration.Builder().setExecutor(executor)
            .setClock(object : Clock { override fun currentTimeMillis() = clock.get() }).build(),
      )
      workManager = WorkManager.getInstance(context)
      manager = DownloadManager.getInstance(context, storeDirectory)
   }

   @After
   fun tearDown() {
      try {
         gates.forEach { it.countDown() }
         server.close()
         workManager.cancelAllWork().result.get(10, TimeUnit.SECONDS)
         executor.shutdownNow()
         assertTrue(executor.awaitTermination(15, TimeUnit.SECONDS))
         WorkManagerTestInitHelper.closeWorkDatabase()
      } finally {
         WorkManagerImpl.setDelegate(previousWorkManager)
         singleton.set(null, previousManager)
         directory.deleteRecursively()
      }
   }

   @Test
   fun schedulerStopPreservesActiveStateAndAutomaticResumeCanRetry() {
      val record = interruptTransfer()
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertEquals("abc", partial(record).readText())
      assertEquals(3L, manager.list().single().receivedBytes)
      assertEquals(DownloadStatus.InProgress, DownloadStore(storeDirectory).findByPath(record.path)?.status)

      val failures = executor.submit { repeat(4) { respond(503) } }
      assertEquals(ListenableWorker.Result.retry(), runBlocking { worker(record, attempt = 1).doWork() })
      failures.get(15, TimeUnit.SECONDS)
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertNull(manager.get(record.path)?.error)
      val success = executor.submit { respond(206, "def", "Content-Range: bytes 3-5/6\r\n") }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record, attempt = 2).doWork() })
      success.get(15, TimeUnit.SECONDS)
      assertEquals("abcdef", File(record.path).readText())
      assertNull(manager.get(record.path))
   }

   @Test
   fun failureAfterSchedulerStopCannotDisappear() {
      val record = interruptTransfer()
      val failures = executor.submit { repeat(4) { respond(503) } }
      // Check the second reproduced bug independently of the held-status assertion.
      assertEquals(ListenableWorker.Result.retry(), runBlocking { worker(record, attempt = 1).doWork() })
      failures.get(15, TimeUnit.SECONDS)
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
   }

   @Test
   fun automaticResumeCanCompleteBeforeTheStoppedReadReturns() {
      lateReadRace(replace = false)
   }

   @Test
   fun explicitResumeCanCompleteBeforeThePausedReadReturns() {
      lateReadRace(replace = true)
   }

   @Test
   fun replacementCanCompleteBeforeTheCanceledReadReturns() {
      lateReadRace(replace = true, cancel = true)
   }

   @Test
   fun pauseAndResumeIgnoreOldTerminalFailure() {
      lateResponseRace(status = 404, cancel = false, resume = true)
   }

   @Test
   fun pauseCannotBeUndoneByAlreadyCompletePartial() {
      lateResponseRace(status = 416, cancel = false, resume = false, headers = "Content-Range: bytes */3\r\n")
   }

   @Test
   fun pauseDuringHeadersCannotBeUndoneByRangeFallback() {
      lateResponseRace(status = 200, cancel = false, resume = false)
   }

   @Test
   fun pauseAndImmediateResumeIgnoreOldRangeFallback() {
      lateResponseRace(status = 200, cancel = false, resume = true)
   }

   @Test
   fun cancelAndRecreateIgnoreOldUnsatisfiableRange() {
      lateResponseRace(status = 416, cancel = true, resume = true)
   }

   @Test
   fun terminalFailureAfterAutomaticResumeIsVisible() {
      val record = createActive()
      partial(record).writeText("abc")
      val response = executor.submit { respond(404) }
      assertEquals(ListenableWorker.Result.failure(), runBlocking { worker(record, attempt = 1).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals(DownloadStatus.Failed, manager.get(record.path)?.status)
      assertEquals(404, manager.list().single().error?.httpStatus)
      assertEquals("abc", partial(record).readText())
   }

   @Test
   fun exhausted503BecomesWorkManagerFailure() {
      val record = createActive()
      val request = OneTimeWorkRequestBuilder<DownloadWorker>()
         .apply { pendingWork(record).tags.forEach { addTag(it) } }
         .setInputData(DownloadManager.inputDataFor(record, null, storeDirectory))
         .setInitialRunAttemptCount(5)
         .setInitialDelay(1, TimeUnit.DAYS)
         .build()
      synchronized(manager) { manager.workOwnership.replace(record.path, request.id) }
      val responses = executor.submit { repeat(4) { respond(503) } }
      workManager.enqueueUniqueWork("download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request)
         .result.get(10, TimeUnit.SECONDS)
      releaseBackoff(request.id)
      responses.get(15, TimeUnit.SECONDS)
      await { workInfo(request.id).state.isFinished }
      assertEquals(WorkInfo.State.FAILED, workInfo(request.id).state)
      assertEquals(DownloadStatus.Failed, manager.get(record.path)?.status)
      assertEquals(503, manager.get(record.path)?.error?.httpStatus)
   }

   @Test
   fun scheduled503RetriesAndCompletesWithCorrectContents() {
      val record = createActive()
      val expected = buildString { repeat(1024 * 1024) { append(('a'.code + it % 26).toChar()) } }
      val request = OneTimeWorkRequestBuilder<DownloadWorker>()
         .apply { pendingWork(record).tags.forEach { addTag(it) } }
         .setInputData(DownloadManager.inputDataFor(record, null, storeDirectory))
         .build()
      synchronized(manager) { manager.workOwnership.replace(record.path, request.id) }
      val retryReached = gate()
      val release = gate()
      val responses = executor.submit {
         repeat(4) { respond(503) }
         server.accept().use { socket ->
            readRequest(socket)
            retryReached.countDown()
            assertTrue(release.await(15, TimeUnit.SECONDS))
            writeResponse(socket, 200, expected)
         }
      }
      workManager.enqueueUniqueWork("download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request)
         .result.get(10, TimeUnit.SECONDS)
      await {
         workInfo(request.id).let {
            it.state == WorkInfo.State.ENQUEUED && it.runAttemptCount == 1
         }
      }
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertNull(manager.get(record.path)?.error)
      releaseBackoff(request.id)
      assertTrue(retryReached.await(15, TimeUnit.SECONDS))
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertTrue(workInfo(request.id).runAttemptCount >= 2)
      release.countDown()
      responses.get(15, TimeUnit.SECONDS)
      await { workInfo(request.id).state.isFinished }
      assertEquals(WorkInfo.State.SUCCEEDED, workInfo(request.id).state)
      assertEquals(expected.length.toLong(), File(record.path).length())
      val digest = MessageDigest.getInstance("SHA-256")
      assertArrayEquals(digest.digest(expected.toByteArray()), digest.digest(File(record.path).readBytes()))
      assertNull(manager.get(record.path))
   }

   @Test
   fun hostCancellationOfRunningWorkBecomesPaused() {
      val record = createActive()
      val request = OneTimeWorkRequestBuilder<DownloadWorker>()
         .apply { pendingWork(record).tags.forEach { addTag(it) } }
         .setInputData(DownloadManager.inputDataFor(record, null, storeDirectory))
         .build()
      synchronized(manager) { manager.workOwnership.replace(record.path, request.id) }
      val release = gate()
      val response = executor.submit {
         server.accept().use { socket ->
            readRequest(socket)
            socket.getOutputStream().apply {
               write("HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc".toByteArray())
               flush()
               assertTrue(release.await(15, TimeUnit.SECONDS))
               write("def".toByteArray())
               flush()
            }
         }
      }
      workManager.enqueueUniqueWork("download_manager:${record.path}", ExistingWorkPolicy.REPLACE, request)
         .result.get(10, TimeUnit.SECONDS)
      await { partial(record).length() == 3L }
      workManager.cancelWorkById(request.id).result.get(10, TimeUnit.SECONDS)
      release.countDown()
      response.get(15, TimeUnit.SECONDS)
      await { synchronized(manager) { manager.get(record.path)?.status == DownloadStatus.Paused } }
      assertEquals(WorkInfo.State.CANCELLED, workInfo(request.id).state)
      assertEquals(3L, manager.get(record.path)?.receivedBytes)
      assertEquals("abc", partial(record).readText())
      assertFalse(manager.workOwnership.owns(record.path, request.id))
      assertEquals(manager.store.findByPath(record.path), DownloadStore(storeDirectory).findByPath(record.path))
   }

   @Test
   fun hostCancellationDuringForegroundSetupBecomesIdle() {
      val record = createActive()
      val entered = gate()
      val foreground = SettableFuture.create<Void>()
      val worker = worker(record, foregroundUpdater = ForegroundUpdater { _, _, _ ->
         entered.countDown()
         foreground
      })
      val result = worker.startWork()
      assertTrue(entered.await(15, TimeUnit.SECONDS))
      worker.stop(WorkInfo.STOP_REASON_CANCELLED_BY_APP)
      // WorkerWrapper delivers the reason before canceling the startWork future.
      // Direct workers have no wrapper, so reproduce both parts of that signal.
      result.cancel(true)
      await { synchronized(manager) { manager.get(record.path)?.status == DownloadStatus.Idle } }
      assertTrue(result.isCancelled)
      assertEquals(0L, manager.get(record.path)?.receivedBytes)
      assertFalse(partial(record).exists())
      assertFalse(manager.workOwnership.owns(record.path, worker.id))
      assertNoRequest()
   }

   @Test
   fun restoredPausedWorkCannotTransferUntilExplicitResume() {
      val record = createActive()
      partial(record).writeText("abc")
      // Recreate the crash window: Paused is durable, but WM cancellation is not.
      manager.store.update(record.withBytes(3L).withStatus(DownloadStatus.Paused))
      singleton.set(null, null)
      manager = DownloadManager.getInstance(context, storeDirectory)
      val events = mutableListOf<DownloadItem>()
      val subscriber = CoroutineScope(Dispatchers.Unconfined).launch {
         manager.changed.collect { events.add(it) }
      }
      try {
         assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record).doWork() })
         assertEquals(DownloadStatus.Paused, manager.get(record.path)?.status)
         assertEquals("abc", partial(record).readText())
         assertTrue(events.isEmpty())
         assertNoRequest()
      } finally {
         subscriber.cancel()
      }
      manager.resume(record.path)
      val response = executor.submit { respond(206, "def", "Content-Range: bytes 3-5/6\r\n") }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals("abcdef", File(record.path).readText())
   }

   @Test
   fun createDiscardsOrphanedBytesBeforeDownloadingADifferentUrl() {
      val old = createActive()
      partial(old).writeText("old bytes")
      // Recreate process death after cancel persisted removal, before file cleanup.
      manager.store.remove(old)
      singleton.set(null, null)
      manager = DownloadManager.getInstance(context, storeDirectory)
      val url = "http://127.0.0.1:${server.localPort}/different"
      File(old.path).writeText("previous completed file")
      manager.create(old.path, url)
      assertFalse(partial(old).exists())
      assertEquals("previous completed file", File(old.path).readText())
      manager.start(old.path)
      val fresh = requireNotNull(manager.store.findByPath(old.path))
      val response = executor.submit {
         server.accept().use { socket ->
            val request = readRequest(socket)
            assertTrue(request.startsWith("GET /different "))
            assertFalse(request.contains("Range:", ignoreCase = true))
            writeResponse(socket, 200, "new contents")
         }
      }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(fresh).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals("new contents", File(old.path).readText())
   }

   @Test
   fun createPreservesThePartialOfAnExistingRecord() {
      val record = createActive()
      partial(record).writeText("abc")
      manager.create(record.path, "http://example.com/different")
      assertEquals(record, manager.store.findByPath(record.path))
      assertEquals("abc", partial(record).readText())
   }

   @Test
   fun createRejectsAnOrphanThatCannotBeDeleted() {
      val path = File(directory, "file").path
      val orphan = File("$path${DownloadWorker.DOWNLOAD_SUFFIX}")
      assertTrue(orphan.mkdirs())
      File(orphan, "child").writeText("cannot delete a nonempty directory")
      val error = assertThrows(IOException::class.java) { manager.create(path, "http://example.com/new") }
      assertEquals("file", commandErrorCode(error))
      assertNull(manager.get(path))
      assertNull(DownloadStore(storeDirectory).findByPath(path))
      assertTrue(orphan.exists())
   }

   @Test
   fun schedulerCheckpointSurvivesStoreFailure() {
      checkpointWithBlockedStore(WorkInfo.STOP_REASON_CONSTRAINT_CONNECTIVITY, DownloadStatus.InProgress)
   }

   @Test
   fun hostCancellationCheckpointSurvivesStoreFailure() {
      checkpointWithBlockedStore(WorkInfo.STOP_REASON_CANCELLED_BY_APP, DownloadStatus.Paused)
   }

   private fun checkpointWithBlockedStore(reason: Int, status: DownloadStatus) {
      val events = java.util.concurrent.CopyOnWriteArrayList<DownloadItem>()
      val subscriber = CoroutineScope(Dispatchers.Unconfined).launch {
         manager.changed.collect { events.add(it) }
      }
      val record = try {
         interruptTransfer(reason, blockStore = true)
      } finally {
         subscriber.cancel()
      }
      val updated = requireNotNull(manager.store.findByPath(record.path))
      assertEquals(status, manager.get(record.path)?.status)
      assertEquals(3L, manager.list().single().receivedBytes)
      assertEquals(status, events.last().status)
      assertEquals(3L, events.last().receivedBytes)
      assertEquals("abc", partial(record).readText())
      assertEquals(0L, DownloadStore(storeDirectory).findByPath(record.path)?.receivedBytes)
      // The next successful store write must include the in-memory checkpoint.
      manager.create(File(directory, "other").path, record.url)
      assertEquals(updated, DownloadStore(storeDirectory).findByPath(record.path))
   }

   private fun assertNoRequest() {
      val timeout = server.soTimeout
      try {
         server.soTimeout = 200
         assertThrows(SocketTimeoutException::class.java) { server.accept().use { } }
      } finally {
         server.soTimeout = timeout
      }
   }

   @Test
   fun failedCancelPreservesTheRecordPartialAndWorkerOwnership() {
      val record = createActive()
      val request = worker(record).id
      partial(record).writeText("abc")
      withBlockedStore {
         assertThrows(DownloadException.Store::class.java) { manager.cancel(record.path) }
      }
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertTrue("Failed cancel revoked the pending worker", manager.workOwnership.owns(record.path, request))
      assertEquals("abc", partial(record).readText())
      assertEquals(WorkInfo.State.ENQUEUED, workInfo(request).state)
      assertEquals(record, DownloadStore(storeDirectory).findByPath(record.path))
   }

   @Test
   fun failedPauseAndResumePreserveTheirPreviousIntent() {
      val record = createActive()
      val request = worker(record).id
      withBlockedStore {
         assertThrows(DownloadException.Store::class.java) { manager.pause(record.path) }
      }
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertTrue(manager.workOwnership.owns(record.path, request))
      manager.pause(record.path)
      withBlockedStore {
         assertThrows(DownloadException.Store::class.java) { manager.resume(record.path) }
      }
      assertEquals(DownloadStatus.Paused, manager.get(record.path)?.status)
      assertFalse(manager.workOwnership.owns(record.path, request))
   }

   @Test
   fun restoredPendingRequestStillCompletesAfterManagerReinitialization() {
      val record = createActive()
      partial(record).writeText("abc")
      singleton.set(null, null)
      manager = DownloadManager.getInstance(context, storeDirectory)
      // Do not change or depend on the separate startup-reconciliation policy.
      val response = executor.submit { respond(206, "def", "Content-Range: bytes 3-5/6\r\n") }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals("abcdef", File(record.path).readText())
      assertNull(manager.get(record.path))
   }

   @Test
   fun serverIgnoringRangeStillRestartsFromZero() {
      successfulPartialResponse(200, "abcdef", "", "abcdef")
   }

   @Test
   fun alreadyCompletePartialStillFinishesOn416() {
      successfulPartialResponse(416, "", "Content-Range: bytes */3\r\n", "abc")
   }

   @Test
   fun unusablePartialIsDiscardedAnd416RemainsVisible() {
      val record = createActive()
      partial(record).writeText("abc")
      val response = executor.submit { respond(416, headers = "Content-Range: bytes */2\r\n") }
      assertEquals(ListenableWorker.Result.failure(), runBlocking { worker(record).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertFalse(partial(record).exists())
      assertEquals(DownloadStatus.Failed, manager.get(record.path)?.status)
      assertEquals(416, manager.get(record.path)?.error?.httpStatus)
      assertEquals(0L, manager.get(record.path)?.receivedBytes)
   }

   @Test
   fun coroutineCancellationDuringRequestRetryKeepsActiveIntent() {
      val record = createActive()
      partial(record).writeText("abc")
      val worker = worker(record)
      val response = executor.submit { respond(503) }
      val future = worker.startWork()
      response.get(15, TimeUnit.SECONDS)
      worker.stop(WorkInfo.STOP_REASON_CONSTRAINT_CONNECTIVITY)
      await { future.isDone && manager.get(record.path)?.receivedBytes == 3L }
      assertTrue(future.isCancelled)
      assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
      assertNull(manager.get(record.path)?.error)
      assertEquals("abc", partial(record).readText())
   }

   @Test
   fun completionListenerCanStartAReplacementAtTheSamePath() {
      val record = createActive()
      val subscriber = CoroutineScope(Dispatchers.Unconfined).launch {
         manager.changed.collect { item ->
            if (item.status == DownloadStatus.Completed) {
               manager.create(record.path, record.url)
               manager.start(record.path)
            }
         }
      }
      try {
         val response = executor.submit { respond(200, "abcdef") }
         assertEquals(ListenableWorker.Result.success(), worker(record).startWork().get(15, TimeUnit.SECONDS))
         response.get(15, TimeUnit.SECONDS)
         assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
         val replacement = pendingWork(record)
         assertTrue("Completion revoked its listener's replacement", manager.workOwnership.owns(record.path, replacement.id))
      } finally {
         subscriber.cancel()
      }
      val response = executor.submit { respond(200, "new contents") }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals("new contents", File(record.path).readText())
   }

   @Test
   fun pauseListenerCanImmediatelyResumeWithoutLosingTheReplacement() {
      val record = createActive()
      val oldRequest = pendingWork(record).id
      val subscriber = CoroutineScope(Dispatchers.Unconfined).launch {
         manager.changed.collect { item ->
            if (item.status == DownloadStatus.Paused) manager.resume(record.path)
         }
      }
      try {
         manager.pause(record.path)
         assertEquals(DownloadStatus.InProgress, manager.get(record.path)?.status)
         assertFalse(manager.workOwnership.owns(record.path, oldRequest))
         val replacement = workManager.getWorkInfosForUniqueWork("download_manager:${record.path}").get()
            .single { it.id != oldRequest }
         assertTrue("Pause revoked its listener's resumed work", manager.workOwnership.owns(record.path, replacement.id))
         assertEquals(WorkInfo.State.ENQUEUED, replacement.state)
      } finally {
         subscriber.cancel()
      }
   }

   @Test
   fun startListenerCanImmediatelyPauseWithoutLeavingRunnableWork() {
      val path = File(directory, "file").path
      manager.create(path, "http://127.0.0.1:${server.localPort}/file")
      val subscriber = CoroutineScope(Dispatchers.Unconfined).launch {
         manager.changed.collect { item ->
            if (item.status == DownloadStatus.InProgress) manager.pause(path)
         }
      }
      try {
         manager.start(path)
         assertEquals(DownloadStatus.Paused, manager.get(path)?.status)
         val request = workManager.getWorkInfosForUniqueWork("download_manager:$path").get().single()
         assertFalse("Start restored ownership after its listener paused", manager.workOwnership.owns(path, request.id))
         await { workInfo(request.id).state == WorkInfo.State.CANCELLED }
      } finally {
         subscriber.cancel()
      }
   }

   private fun successfulPartialResponse(status: Int, body: String, headers: String, expected: String) {
      val record = createActive()
      partial(record).writeText("abc")
      val response = executor.submit { respond(status, body, headers) }
      assertEquals(ListenableWorker.Result.success(), runBlocking { worker(record).doWork() })
      response.get(15, TimeUnit.SECONDS)
      assertEquals(expected, File(record.path).readText())
      assertFalse(partial(record).exists())
      assertNull(manager.get(record.path))
   }

   /** Blocks store writes without relying on AtomicFile's platform-specific filenames. */
   private fun withBlockedStore(action: () -> Unit) {
      val backup = File(directory, "store-backup")
      assertTrue(storeDirectory.renameTo(backup))
      try {
         storeDirectory.writeText("not a directory")
         action()
      } finally {
         assertTrue(storeDirectory.delete())
         assertTrue(backup.renameTo(storeDirectory))
      }
   }

   /** Stops a real worker after its first flushed chunk, then releases the blocked read. */
   private fun interruptTransfer(
      reason: Int = WorkInfo.STOP_REASON_CONSTRAINT_CONNECTIVITY, blockStore: Boolean = false,
   ): DownloadRecord {
      val record = createActive()
      val worker = worker(record)
      val firstChunk = gate()
      val release = gate()
      val response = executor.submit {
         server.accept().use { socket ->
            readRequest(socket)
            socket.getOutputStream().apply {
               write("HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc".toByteArray())
               flush()
               firstChunk.countDown()
               assertTrue(release.await(15, TimeUnit.SECONDS))
               write("def".toByteArray())
               flush()
            }
         }
      }
      val result = executor.submit<ListenableWorker.Result> { runBlocking { worker.doWork() } }
      assertTrue(firstChunk.await(15, TimeUnit.SECONDS))
      await { partial(record).length() == 3L }
      val stopAndJoin = {
         worker.stop(reason)
         release.countDown()
         response.get(15, TimeUnit.SECONDS)
         result.get(15, TimeUnit.SECONDS)
         Unit
      }
      if (blockStore) withBlockedStore(stopAndJoin) else stopAndJoin()
      return record
   }

   /** Completes replacement work before releasing an obsolete worker's blocked read. */
   private fun lateReadRace(replace: Boolean, cancel: Boolean = false) {
      val record = createActive()
      val oldWorker = worker(record)
      val release = gate()
      val oldResponse = executor.submit {
         server.accept().use { socket ->
            readRequest(socket)
            socket.getOutputStream().apply {
               write("HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc".toByteArray())
               flush()
               assertTrue(release.await(15, TimeUnit.SECONDS))
               write("def".toByteArray())
               flush()
            }
         }
      }
      val oldResult = executor.submit<ListenableWorker.Result> { runBlocking { oldWorker.doWork() } }
      await { partial(record).length() == 3L }
      when {
         cancel -> {
            manager.cancel(record.path)
            manager.create(record.path, record.url)
            manager.start(record.path)
         }
         replace -> {
            manager.pause(record.path)
            manager.resume(record.path)
         }
         else -> oldWorker.stop(WorkInfo.STOP_REASON_CONSTRAINT_CONNECTIVITY)
      }
      val resumed = worker(record, attempt = 1)
      assertEquals(!replace, oldWorker.id == resumed.id)
      val newResponse = executor.submit {
         if (cancel) respond(200, "replacement") else respond(206, "def", "Content-Range: bytes 3-5/6\r\n")
      }
      assertEquals(ListenableWorker.Result.success(), runBlocking { resumed.doWork() })
      newResponse.get(15, TimeUnit.SECONDS)
      release.countDown()
      oldResponse.get(15, TimeUnit.SECONDS)
      oldResult.get(15, TimeUnit.SECONDS)
      assertEquals(if (cancel) "replacement" else "abcdef", File(record.path).readText())
      assertFalse(partial(record).exists())
      assertNull(manager.get(record.path))
   }

   /** Releases an old response only after the command and replacement have committed. */
   private fun lateResponseRace(status: Int, cancel: Boolean, resume: Boolean, headers: String = "") {
      val record = createActive()
      partial(record).writeText("abc")
      val requested = gate()
      val release = gate()
      val oldWorker = worker(record)
      val response = executor.submit {
         server.accept().use { socket ->
            assertTrue(readRequest(socket).contains("Range: bytes=3-"))
            requested.countDown()
            assertTrue(release.await(15, TimeUnit.SECONDS))
            writeResponse(socket, status, if (status == 200) "abcdef" else "", headers)
         }
      }
      val result = executor.submit<ListenableWorker.Result> { runBlocking { oldWorker.doWork() } }
      assertTrue(requested.await(15, TimeUnit.SECONDS))
      if (cancel) {
         manager.cancel(record.path)
         assertFalse(partial(record).exists())
         manager.create(record.path, record.url)
         manager.start(record.path)
         partial(record).writeText("new")
      } else {
         manager.pause(record.path)
         if (resume) manager.resume(record.path)
      }
      // No stop() signal: this specifically exercises async cancellation's window.
      release.countDown()
      response.get(15, TimeUnit.SECONDS)
      result.get(15, TimeUnit.SECONDS)
      assertEquals(if (cancel) "new" else "abc", partial(record).readText())
      assertFalse(File(record.path).exists())
      assertEquals(if (resume) DownloadStatus.InProgress else DownloadStatus.Paused,
         manager.get(record.path)?.status)
      assertNull(manager.get(record.path)?.error)
   }

   /** Pending constrained requests are held by TestScheduler until explicitly released. */
   private fun createActive(): DownloadRecord {
      val path = File(directory, "file").path
      manager.create(path, "http://127.0.0.1:${server.localPort}/file")
      manager.start(path)
      workManager.getWorkInfosForUniqueWork("download_manager:$path").get(10, TimeUnit.SECONDS)
      return requireNotNull(manager.store.findByPath(path))
   }

   private fun worker(
      record: DownloadRecord, attempt: Int = 0, foregroundUpdater: ForegroundUpdater? = null,
   ): DownloadWorker {
      val request = pendingWork(record)
      return TestListenableWorkerBuilder.from(context, DownloadWorker::class.java)
         .setId(request.id)
         .setTags(request.tags.toList())
         .setRunAttemptCount(attempt)
         .apply { if (foregroundUpdater != null) setForegroundUpdater(foregroundUpdater) }
         .setInputData(DownloadManager.inputDataFor(record, null, storeDirectory))
         .build()
   }

   private fun pendingWork(record: DownloadRecord): WorkInfo =
      workManager.getWorkInfosForUniqueWork("download_manager:${record.path}")
         .get(10, TimeUnit.SECONDS).single { !it.state.isFinished }

   /** These assertions refer to requests already committed to WorkManager. */
   private fun workInfo(id: UUID): WorkInfo =
      requireNotNull(workManager.getWorkInfoById(id).get(10, TimeUnit.SECONDS)) {
         "Missing work request $id"
      }

   /** Advance only the test clock; production keeps WorkManager's default backoff. */
   private fun releaseBackoff(id: UUID) {
      clock.addAndGet(TimeUnit.DAYS.toMillis(2))
      requireNotNull(WorkManagerTestInitHelper.getTestDriver(context)).setInitialDelayMet(id)
   }

   private fun partial(record: DownloadRecord) = File("${record.path}${DownloadWorker.DOWNLOAD_SUFFIX}")
   private fun gate() = CountDownLatch(1).also { gates.add(it) }

   private fun respond(status: Int, body: String = "", headers: String = "") {
      server.accept().use { socket ->
         readRequest(socket)
         writeResponse(socket, status, body, headers)
      }
   }

   private fun readRequest(socket: Socket): String {
      socket.soTimeout = 15000
      val reader = socket.getInputStream().bufferedReader()
      return buildString {
         while (true) {
            val line = reader.readLine() ?: break
            if (line.isEmpty()) break
            appendLine(line)
         }
      }
   }

   private fun writeResponse(socket: Socket, status: Int, body: String, headers: String = "") {
      socket.getOutputStream().apply {
         write("HTTP/1.1 $status Test\r\nContent-Length: ${body.length}\r\n${headers}Connection: close\r\n\r\n$body".toByteArray())
         flush()
      }
   }

   private fun await(condition: () -> Boolean) {
      val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(15)
      while (!condition()) {
         assertTrue("Timed out waiting for worker", System.nanoTime() < deadline)
         Thread.sleep(20)
      }
   }
}
