import Combine
import DownloadManagerKit
import SwiftRs
import Tauri
import WebKit

class PathArgs: Decodable {
   let path: String
}

class CreateArgs: Decodable {
   let path: String
   let url: String
}

class DownloadPlugin: Plugin {
   let downloadManager = DownloadManager.shared

   override init()
   {
      super.init()
      Task {
          for await download in DownloadManager.shared.changed {
             try? self.trigger("changed", data: download);
#if DEBUG
             Logger.debug("[\(download.path)] \(download.status) - \(String(format: "%.0f", download.progress))%")
#endif
          }
      }
   }

   @objc public func list(_ invoke: Invoke) throws {
      let response = downloadManager.list()
      invoke.resolve(response)
   }

   @objc public func get(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let response = downloadManager.get(path: pathToURL(args.path))
      invoke.resolve(response)
   }
   
   @objc public func create(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(CreateArgs.self)
      let response = downloadManager.create(path: pathToURL(args.path), url: URL(string: args.url)!)
      invoke.resolve(response)
   }
   
   @objc public func start(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let response = try downloadManager.start(path: pathToURL(args.path))
      invoke.resolve(response)
   }
   
   @objc public func cancel(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let response = try downloadManager.cancel(path: pathToURL(args.path))
      invoke.resolve(response)
   }
   
   @objc public func pause(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let response = try downloadManager.pause(path: pathToURL(args.path))
      invoke.resolve(response)
   }
   
   @objc public func resume(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(PathArgs.self)
      let response = try downloadManager.resume(path: pathToURL(args.path))
      invoke.resolve(response)
   }

   private func pathToURL(_ path: String) -> URL {
      // Converts a path string to a URL. Handles both file:// URLs and plain filesystem paths.
      return URL(string: path) ?? URL(fileURLWithPath: path)
   }
}

@_cdecl("init_plugin_download")
func initPlugin() -> Plugin {
   return DownloadPlugin()
}
