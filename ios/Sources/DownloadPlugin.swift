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
      let url = try parseUrl(args.url)
      Task {
         let response = await self.downloadManager.create(path: path, url: url)
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
