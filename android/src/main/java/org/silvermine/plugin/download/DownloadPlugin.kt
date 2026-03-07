package org.silvermine.plugin.download

import android.app.Activity
import android.util.Log
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.silvermine.downloadmanager.DownloadManager
import org.silvermine.downloadmanager.parsePath
import org.silvermine.downloadmanager.parseUrl
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.json.JSONArray

@InvokeArg
class PathArgs {
    var path: String? = null
}

@InvokeArg
class CreateArgs {
    var path: String? = null
    var url: String? = null
}

@TauriPlugin
class DownloadPlugin(activity: Activity) : Plugin(activity) {
    private val json = Json { encodeDefaults = true }
    private val downloadManager by lazy { DownloadManager.getInstance(activity.applicationContext) }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun load(webView: WebView) {
        scope.launch {
            downloadManager.changed.collect { item ->
                trigger("changed", JSObject(json.encodeToString(item)))
                Log.d(TAG, "[${item.path}] ${item.status} - ${"%.0f".format(item.progress)}%")
            }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        scope.cancel()
    }

    @Command
    fun list(invoke: Invoke) {
        scope.launch {
            val items = withContext(Dispatchers.IO) { downloadManager.list() }
            val result = JSObject().apply {
                put("value", JSONArray(json.encodeToString(items)))
            }
            invoke.resolve(result)
        }
    }

    @Command
    fun get(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            val response = withContext(Dispatchers.IO) { downloadManager.get(path) }
            invoke.resolve(JSObject(json.encodeToString(response)))
        }
    }

    @Command
    fun create(invoke: Invoke) {
        val args = invoke.parseArgs(CreateArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        val url = try {
            parseUrl(args.url ?: throw IllegalArgumentException("Missing required argument: url"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            val response = withContext(Dispatchers.IO) { downloadManager.create(path, url) }
            invoke.resolve(JSObject(json.encodeToString(response)))
        }
    }

    @Command
    fun start(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            try {
                val response = withContext(Dispatchers.IO) { downloadManager.start(path) }
                invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
                invoke.reject(e.message)
            }
        }
    }

    @Command
    fun cancel(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            try {
                val response = withContext(Dispatchers.IO) { downloadManager.cancel(path) }
                invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
                invoke.reject(e.message)
            }
        }
    }

    @Command
    fun pause(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            try {
                val response = withContext(Dispatchers.IO) { downloadManager.pause(path) }
                invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
                invoke.reject(e.message)
            }
        }
    }

    @Command
    fun resume(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        val path = try {
            parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
        } catch (e: Exception) {
            return invoke.reject(e.message)
        }
        scope.launch {
            try {
                val response = withContext(Dispatchers.IO) { downloadManager.resume(path) }
                invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
                invoke.reject(e.message)
            }
        }
    }

    companion object {
        private const val TAG = "DownloadPlugin"
    }
}
