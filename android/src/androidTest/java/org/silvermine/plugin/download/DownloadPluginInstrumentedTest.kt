package org.silvermine.plugin.download

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Intent
import android.webkit.WebView
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Configuration
import androidx.work.NetworkType
import androidx.work.WorkManager
import androidx.work.impl.WorkManagerImpl
import androidx.work.testing.WorkManagerTestInitHelper
import app.tauri.annotation.Command
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.fasterxml.jackson.annotation.JsonAutoDetect
import com.fasterxml.jackson.annotation.PropertyAccessor
import com.fasterxml.jackson.core.JsonParser
import com.fasterxml.jackson.databind.DeserializationContext
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.databind.JsonDeserializer
import com.fasterxml.jackson.databind.ObjectMapper
import com.fasterxml.jackson.databind.module.SimpleModule
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.silvermine.downloadmanager.DownloadManager
import java.io.File
import java.lang.reflect.InvocationTargetException
import java.util.UUID
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class BridgeTestActivity : Activity()

/** Real Tauri Invoke parsing and plugin handlers, with the scheduler held by work-testing. */
@RunWith(AndroidJUnit4::class)
@SuppressLint("RestrictedApi")
class DownloadPluginInstrumentedTest {
   private val instrumentation = InstrumentationRegistry.getInstrumentation()
   private val context = instrumentation.targetContext
   private val singleton = DownloadManager::class.java.getDeclaredField("instance").apply { isAccessible = true }
   private var previousManager: Any? = null
   private lateinit var previousWorkManager: WorkManagerImpl
   private lateinit var workManager: WorkManager
   private lateinit var directory: File
   private lateinit var activity: Activity
   private lateinit var plugin: DownloadPlugin
   private var webView: WebView? = null
   private val executor = Executors.newCachedThreadPool()
   private val events = CopyOnWriteArrayList<JSONObject>()
   private val mapper = ObjectMapper()
      .disable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES)
      .enable(DeserializationFeature.FAIL_ON_NULL_FOR_PRIMITIVES)
      .setVisibility(PropertyAccessor.FIELD, JsonAutoDetect.Visibility.ANY)

   @Before
   fun setUp() {
      previousManager = singleton.get(null)
      previousWorkManager = WorkManagerImpl.getInstance(context)
      singleton.set(null, null)
      directory = File(context.filesDir, "bridge-${UUID.randomUUID()}")
      WorkManagerTestInitHelper.initializeTestWorkManager(context,
         Configuration.Builder().setExecutor(executor).build())
      workManager = WorkManager.getInstance(context)
      // Only channel delivery is replaced; arguments and command results use Tauri Invoke.
      mapper.registerModule(SimpleModule().addDeserializer(Channel::class.java, object : JsonDeserializer<Channel>() {
         override fun deserialize(parser: JsonParser, context: DeserializationContext): Channel {
            val id = parser.valueAsString.removePrefix("__CHANNEL__:").toLong()
            return Channel(id, { payload -> events.add(JSONObject(payload)) }, mapper)
         }
      }))
      activity = instrumentation.startActivitySync(Intent(instrumentation.context, BridgeTestActivity::class.java)
         .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      instrumentation.runOnMainSync { plugin = DownloadPlugin(activity) }
      assertEquals("null", invoke("configure", mapOf("storeDir" to directory.path, "userAgent" to "bridge-test/1.0")))
   }

   @After
   fun tearDown() {
      try {
         if (::plugin.isInitialized) instrumentation.runOnMainSync {
            plugin.onDestroy()
            webView?.destroy()
            activity.finish()
         }
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

   private fun invoke(command: String, args: Map<String, Any?> = emptyMap()): String {
      val result = CompletableFuture<String>()
      val invocation = Invoke(1, command, 1, 2, { callback, payload ->
         if (callback == 1L) result.complete(payload)
         else result.completeExceptionally(AssertionError("Unexpected transport rejection: $payload"))
      }, mapper.writeValueAsString(args), mapper)
      val method = plugin.javaClass.getMethod(command, Invoke::class.java)
      assertNotNull("Missing Tauri command annotation: $command", method.getAnnotation(Command::class.java))
      instrumentation.runOnMainSync {
         try {
            method.invoke(plugin, invocation)
         } catch (error: Exception) {
            result.completeExceptionally((error as? InvocationTargetException)?.targetException ?: error)
         }
      }
      return result.get(10, TimeUnit.SECONDS)
   }

   /** Inspect raw JSON values so coercing JSONObject getters cannot hide wrong wire types. */
   private fun assertIdleItem(item: JSONObject, path: String, allowed: Boolean) {
      assertEquals("https://example.com/file", item.get("url"))
      assertEquals(path, item.get("path"))
      assertEquals("idle", item.get("status"))
      assertEquals(allowed, item.getJSONObject("options").get("allowMetered"))
      val receivedBytes = item.get("receivedBytes")
      assertTrue(receivedBytes is Int || receivedBytes is Long)
      assertEquals(0L, (receivedBytes as Number).toLong())
      assertSame(JSONObject.NULL, item.get("totalBytes"))
      val progress = item.get("progress")
      assertTrue(progress is Number)
      assertEquals(0.0, (progress as Number).toDouble(), 0.0)
   }

   @Test
   fun createStartAndResumeKeepPolicyThroughPlugin() {
      for (allowed in listOf(false, true)) {
         val path = File(directory, "policy-$allowed").path
         val created = JSONObject(invoke("create", mapOf("path" to path, "url" to "https://example.com/file",
            "options" to mapOf("allowMetered" to allowed))))
         assertIdleItem(created.getJSONObject("download"), path, allowed)
         assertEquals("idle", created.get("expectedStatus"))
         assertEquals(true, created.get("isExpectedStatus"))
         val manager = DownloadManager.getInstance(context, directory)
         assertEquals(allowed, manager.get(path)!!.options.allowMetered)
         assertEquals("bridge-test/1.0", manager.userAgent)
         assertTrue(File(directory, "downloads.json").exists())
         val started = JSONObject(invoke("start", mapOf("path" to path)))
         assertEquals("inProgress", started.getJSONObject("download").getString("status"))
         val first = workManager.getWorkInfosForUniqueWork("download_manager:$path").get(5, TimeUnit.SECONDS).single()
         val expected = if (allowed) NetworkType.CONNECTED else NetworkType.UNMETERED
         assertEquals(expected, first.constraints.requiredNetworkType)
         val paused = JSONObject(invoke("pause", mapOf("path" to path)))
         assertEquals("paused", paused.getJSONObject("download").getString("status"))
         workManager.cancelUniqueWork("download_manager:$path").result.get(5, TimeUnit.SECONDS)
         val resumed = JSONObject(invoke("resume", mapOf("path" to path)))
         assertTrue(resumed.getBoolean("isExpectedStatus"))
         val next = workManager.getWorkInfosForUniqueWork("download_manager:$path").get(5, TimeUnit.SECONDS)
            .single { it.id != first.id && !it.state.isFinished }
         assertEquals(expected, next.constraints.requiredNetworkType)
         val canceled = JSONObject(invoke("cancel", mapOf("path" to path)))
         assertEquals("canceled", canceled.getJSONObject("download").getString("status"))
      }
   }

   @Test
   fun listGetAndArgumentErrorsUseNativeWireShapes() {
      val path = File(directory, "file").path
      assertEquals("null", invoke("get", mapOf("path" to path)))
      val created = JSONObject(invoke("create", mapOf("path" to path, "url" to "https://example.com/file",
         "options" to mapOf("allowMetered" to false))))
      val item = created.getJSONObject("download")
      assertIdleItem(item, path, false)
      val listed = JSONObject(invoke("list")).getJSONArray("value")
      assertEquals(1, listed.length())
      assertIdleItem(listed.getJSONObject(0), path, false)
      assertIdleItem(JSONObject(invoke("get", mapOf("path" to path))), path, false)
      val paused = JSONObject(invoke("pause", mapOf("path" to path)))
      assertEquals("paused", paused.get("expectedStatus"))
      assertEquals(false, paused.get("isExpectedStatus"))
      assertIdleItem(paused.getJSONObject("download"), path, false)
      for (args in listOf(emptyMap(), mapOf("path" to path),
         mapOf("path" to path, "url" to "https://example.com/file"),
         mapOf("path" to path, "url" to "https://example.com/file", "options" to emptyMap<String, Any>()),
         mapOf("path" to path, "url" to "bad", "options" to mapOf("allowMetered" to false)))) {
         val error = JSONObject(invoke("create", args)).getJSONObject("__downloadError")
         assertEquals("invalid input", error.getString("code"))
         assertTrue(error.has("message"))
      }
      for (command in listOf("start", "pause", "resume", "cancel")) {
         val error = JSONObject(invoke(command, mapOf("path" to File(directory, "missing").path))).getJSONObject("__downloadError")
         assertEquals("download not found", error.getString("code"))
      }
   }

   @Test
   fun managerChangesReachRegisteredListenerAndRemovalStopsDelivery() {
      invoke("registerListener", mapOf("event" to "changed", "handler" to "__CHANNEL__:79"))
      instrumentation.runOnMainSync {
         webView = WebView(activity)
         plugin.load(webView!!)
      }
      instrumentation.waitForIdleSync()
      val path = File(directory, "event").path
      invoke("create", mapOf("path" to path, "url" to "https://example.com/file", "options" to mapOf("allowMetered" to false)))
      instrumentation.waitForIdleSync()
      assertTrue(events.any { it.getString("path") == path && it.getString("status") == "idle" })
      assertFalse(events.first().getJSONObject("options").getBoolean("allowMetered"))
      invoke("removeListener", mapOf("event" to "changed", "channelId" to 79))
      val count = events.size
      instrumentation.runOnMainSync { plugin.trigger("changed", JSObject().put("probe", true) as JSObject) }
      invoke("cancel", mapOf("path" to path))
      instrumentation.waitForIdleSync()
      assertEquals(count, events.size)
   }
}
