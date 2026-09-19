import DownloadManagerKit
import SwiftRs
import Tauri
import WebKit

class PathArgs: Decodable {
   let path: String
}

/// Settings pushed by the Rust plugin at startup.
///
/// Named for the payload rather than the setting so a second builder option is a new
/// field here, not a second command.
///
/// Every field is optional because the command is invoked unconditionally: an absent
/// value means "keep the platform default", not a malformed call.
class ConfigArgs: Decodable {
   let userAgent: String?
   let storeDir: String?
}

class CreateArgs: Decodable {
   let path: String
   let url: String
   let options: CreateOptions
}

class DownloadPlugin: Plugin {
   /// Computed rather than stored, so constructing the plugin does not construct the
   /// manager. `configure` has to set the store directory before anything reaches
   /// `shared`, because the manager's store opens its file in its own initializer.
   ///
   /// Thread-safe lazy construction still comes from `static let shared` rather than
   /// from a `lazy var`, which is not thread-safe on a class.
   var downloadManager: DownloadManager { DownloadManager.shared }

   /// Subscribed here rather than in `init`, mirroring Android's `load(webView)`.
   ///
   /// Moving it off `init` is what keeps the manager unconstructed until `configure`
   /// runs. Nothing is missed by waiting: `trigger` delivers to listeners registered
   /// from JS, so before a webview exists there are none and it is already a no-op.
   override func load(webview: WKWebView) {
      Task {
          for await download in self.downloadManager.changed {
             try? self.trigger("changed", data: download);
#if DEBUG
             Logger.debug("[\(URL(fileURLWithPath: download.path).lastPathComponent)] \(download.status) - \(String(format: "%.0f", download.progress))% (\(download.receivedBytes)/\(download.totalBytes.map { String($0) } ?? "unknown") bytes)")
#endif
          }
      }
   }

   /// Applies the settings from the Rust plugin's builder.
   ///
   /// Invoked by the Rust plugin during setup rather than from the webview: Tauri
   /// fills the config it hands to `load(webview:)` from `tauri.conf.json`, so a
   /// value set on the Rust builder can only arrive as a command.
   ///
   /// This is also where the download manager is first built, which is load-bearing
   /// rather than incidental. The store directory has to be set before `shared` is
   /// touched, and Tauri defers `load(webview:)` until the webview exists — after
   /// plugin registration — so this command runs first. The Rust side invokes it
   /// unconditionally, with both fields nil when nothing is configured, to keep that
   /// true and to leave the background `URLSession` restoring as early as it does now.
   @objc public func configure(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(ConfigArgs.self)

      // Before the `Task`, and before any use of `downloadManager`: the manager builds
      // its store — reading the file — during its own initialization.
      //
      // Thrown rather than logged, so a directory the app cannot write to aborts launch
      // through Rust's `download.configure(...)?` instead of leaving the store where
      // `save` will silently fail to write it. Matches how desktop treats an invalid
      // store directory, and how Android reaches the same outcome via `AtomicFile`.
      if let storeDir = args.storeDir {
         try DownloadManager.setStoreDirectory(URL(fileURLWithPath: storeDir, isDirectory: true))
      }

      Task {
         await self.downloadManager.setUserAgent(args.userAgent)
         // No response payload anchors this call inside the Task the way every other
         // handler's does, so hoisting it out would still compile — and would let
         // Rust's blocking `run_mobile_plugin` return before the actor write lands.
         invoke.resolve()
      }
   }

   @objc public func list(_ invoke: Invoke) {
      Task {
         let response = await self.downloadManager.list()
         invoke.resolve(response)
      }
   }

   @objc public func get(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let path = try parsePath(args.path)
      Task {
         let response = await self.downloadManager.get(path: path)
         invoke.resolve(response)
      }
   }
   
   @objc public func create(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(CreateArgs.self)
      let path = try parsePath(args.path)
      let url = try parseURL(args.url)
      Task {
         let response = await self.downloadManager.create(
            path: path,
            url: url,
            options: args.options
         )
         invoke.resolve(response)
      }
   }
   
   @objc public func start(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let path = try parsePath(args.path)
      Task {
         do {
            let response = try await self.downloadManager.start(path: path)
            invoke.resolve(response)
         } catch {
            invoke.reject(error.localizedDescription)
         }
      }
   }
   
   @objc public func cancel(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let path = try parsePath(args.path)
      Task {
         do {
            let response = try await self.downloadManager.cancel(path: path)
            invoke.resolve(response)
         } catch {
            invoke.reject(error.localizedDescription)
         }
      }
   }
   
   @objc public func pause(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let path = try parsePath(args.path)
      Task {
         do {
            let response = try await self.downloadManager.pause(path: path)
            invoke.resolve(response)
         } catch {
            invoke.reject(error.localizedDescription)
         }
      }
   }
   
   @objc public func resume(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let path = try parsePath(args.path)
      Task {
         do {
            let response = try await self.downloadManager.resume(path: path)
            invoke.resolve(response)
         } catch {
            invoke.reject(error.localizedDescription)
         }
      }
   }
}

@_cdecl("init_plugin_download")
func initPlugin() -> Plugin {
   return DownloadPlugin()
}
