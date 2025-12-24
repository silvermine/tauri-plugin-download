import Combine
import DownloadManagerKit
import SwiftRs
import Tauri
import WebKit

class KeyArgs: Decodable {
   let key: String
}

class CreateArgs: Decodable {
   let key: String
   let url: String
   let path: String
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
             Logger.debug("[\(download.key)] \(download.status) - \(String(format: "%.0f", download.progress))%")
#endif
          }
      }
   }

   @objc public func list(_ invoke: Invoke) throws {
      let response = downloadManager.list()
      invoke.resolve(response)
   }

   @objc public func get(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(KeyArgs.self)
      let response = downloadManager.get(key: args.key)
      invoke.resolve(response)
   }
   
   @objc public func create(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(CreateArgs.self)
      let response = downloadManager.create(key: args.key, url: URL(string: args.url)!, path: URL(string: args.path)!)
      invoke.resolve(response)
   }
   
   @objc public func start(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(KeyArgs.self)
      let response = try downloadManager.start(key: args.key)
      invoke.resolve(response)
   }
   
   @objc public func cancel(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(KeyArgs.self)
      let response = try downloadManager.cancel(key: args.key)
      invoke.resolve(response)
   }
   
   @objc public func pause(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(KeyArgs.self)
      let response = try downloadManager.pause(key: args.key)
      invoke.resolve(response)
   }
   
   @objc public func resume(_ invoke: Invoke) throws {
      let args = try invoke.parseArgs(KeyArgs.self)
      let response = try downloadManager.resume(key: args.key)
      invoke.resolve(response)
   }
}

@_cdecl("init_plugin_download")
func initPlugin() -> Plugin {
   return DownloadPlugin()
}
