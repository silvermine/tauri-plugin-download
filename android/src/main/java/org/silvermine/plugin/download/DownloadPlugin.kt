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
import org.silvermine.downloadmanager.commandErrorCode
import org.silvermine.downloadmanager.CreateOptions
import org.silvermine.downloadmanager.DownloadManager
import org.silvermine.downloadmanager.parsePath
import org.silvermine.downloadmanager.parseURI
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.json.JSONArray
import java.io.File

@InvokeArg
class PathArgs {
   var path: String? = null
}

/**
 * Bridge-local mirror of [CreateOptions], kept separate so Tauri's `@InvokeArg`
 * reflection and kotlinx.serialization never share a type.
 *
 * Nullable so an absent value is rejected rather than defaulted. The Rust bridge
 * resolves the API default before invoking, so the policy always arrives stated.
 */
@InvokeArg
class CreateOptionsArgs {
   var allowMetered: Boolean? = null
}

/**
 * Settings pushed by the Rust plugin at startup.
 *
 * Named for the payload rather than the setting so a second builder option is a new
 * field here, not a second command.
 */
@InvokeArg
class ConfigArgs {
   var userAgent: String? = null
   var storeDir: String? = null
}

@InvokeArg
class CreateArgs {
   var path: String? = null
   var url: String? = null
   var options: CreateOptionsArgs? = null
}

@TauriPlugin
class DownloadPlugin(activity: Activity) : Plugin(activity) {
   private val json = Json { encodeDefaults = true }

   /**
    * Held because `Plugin.activity` is `private`, so a subclass can reach it from an
    * initializer — where the constructor parameter is still in scope — but not from a
    * member. The application context rather than the activity, which must not outlive
    * its own lifecycle.
    */
   private val appContext = activity.applicationContext

   /**
    * The instance [configure] built, `null` until it has. Volatile because it is written
    * there and read from every command.
    */
   @Volatile
   private var configuredManager: DownloadManager? = null

   /**
    * Read rather than built: [configure] has to construct the instance itself, to pass
    * the store directory into a constructor that reads it. Calling [DownloadManager
    * .getInstance] here would instead open the default directory for whichever caller
    * arrived first, which is the failure a configured store directory exists to
    * prevent — so a read before [configure] is a programming error and says so.
    */
   private val downloadManager: DownloadManager
      get() = checkNotNull(configuredManager) {
         "Download manager read before configure built it"
      }

   private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

   override fun load(webView: WebView) {
      scope.launch {
         downloadManager.changed.collect { item ->
            try {
               trigger("changed", JSObject(json.encodeToString(item)))
               Log.d(
                  TAG,
                  "[${File(item.path).name}] ${item.status} - ${"%.0f".format(item.progress)}%" +
                     " (${item.receivedBytes}/${item.totalBytes ?: "unknown"} bytes)",
               )
            } catch (e: Exception) {
               Log.e(TAG, "Failed to emit changed event: ${e.message}")
            }
         }
      }
   }

   override fun onDestroy() {
      super.onDestroy()
      scope.cancel()
   }

   /**
    * Applies the settings from the Rust plugin's builder.
    *
    * Invoked by the Rust plugin during setup rather than from the webview: Tauri
    * fills the config it hands to [load] from `tauri.conf.json`, so a value set on
    * the Rust builder can only arrive as a command.
    *
    * This is where the download manager is built, and that is load-bearing rather than
    * incidental. The store directory is read by [DownloadStore]'s constructor, so it
    * has to reach [DownloadManager.getInstance] on the call that creates the singleton.
    * Tauri defers [load] until the webview exists, which is after plugin registration,
    * so this command runs first — and the Rust side invokes it unconditionally, with
    * both fields null when nothing is configured, to keep that true.
    *
    * Every argument is optional: absent means "keep the platform default", not a
    * malformed call.
    */
   @Command
   fun configure(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, ConfigArgs::class.java)
         scope.launch {
            try {
               val manager = withContext(Dispatchers.IO) {
                  DownloadManager.getInstance(appContext, args.storeDir?.let { File(it) })
               }

               configuredManager = manager
               args.userAgent?.let { manager.userAgent = it }
               invoke.resolve()
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun list(invoke: Invoke) {
      try {
         scope.launch {
            try {
               val items = withContext(Dispatchers.IO) { downloadManager.list() }
               val result = JSObject().apply {
                  put("value", JSONArray(json.encodeToString(items)))
               }
               invoke.resolve(result)
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun get(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, PathArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         scope.launch {
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.get(path) }
               if (response == null) {
                  invoke.resolve()
               } else {
                  invoke.resolve(JSObject(json.encodeToString(response)))
               }
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun create(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, CreateArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         val url = parseURI(args.url ?: throw IllegalArgumentException("Missing required argument: url"))
         val options = CreateOptions(
            allowMetered = args.options?.allowMetered
               ?: throw IllegalArgumentException("Missing required argument: options.allowMetered"),
         )

         scope.launch {
            // Guarded like every sibling command. Without this the store's own write
            // failures — an unwritable configured directory reaches `AtomicFile.startWrite`
            // as an IOException — escape to a scope with no handler and take the app down,
            // where desktop returns the error to the caller.
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.create(path, url, options) }
               invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun start(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, PathArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         scope.launch {
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.start(path) }
               invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun cancel(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, PathArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         scope.launch {
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.cancel(path) }
               invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun pause(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, PathArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         scope.launch {
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.pause(path) }
               invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   @Command
   fun resume(invoke: Invoke) {
      try {
         val args = parseCommandArgs(invoke, PathArgs::class.java)
         val path = parsePath(args.path ?: throw IllegalArgumentException("Missing required argument: path"))
         scope.launch {
            try {
               val response = withContext(Dispatchers.IO) { downloadManager.resume(path) }
               invoke.resolve(JSObject(json.encodeToString(response)))
            } catch (e: Exception) {
               rejectCommand(invoke, e)
            }
         }
      } catch (e: Exception) {
         rejectCommand(invoke, e)
      }
   }

   /** Argument decoding is input validation even when Jackson throws an IOException. */
   private fun <T> parseCommandArgs(invoke: Invoke, type: Class<T>): T = try {
      invoke.parseArgs(type)
   } catch (error: Exception) {
      throw IllegalArgumentException(error.message, error)
   }

   /** Keeps the machine-readable code; Rust supplies the public rejection shape. */
   private fun rejectCommand(invoke: Invoke, error: Exception) {
      invoke.reject(error.message ?: "Native command failed", commandErrorCode(error))
   }

   companion object {
      private const val TAG = "DownloadPlugin"
   }
}
