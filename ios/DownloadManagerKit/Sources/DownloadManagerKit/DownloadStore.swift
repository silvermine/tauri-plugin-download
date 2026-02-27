//
//  DownloadStore.swift
//  DownloadManagerKit
//

import Foundation
import os.log

/// Thread-safe store for the downloads array.
actor DownloadStore {
   private var downloads: [DownloadItem]
   private static let savePath = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("downloads.json")

   init() {
      downloads = DownloadStore.load()
   }

   func list() -> [DownloadItem] { downloads }
   
   func findByPath(_ path: URL) -> DownloadItem? {
      downloads.first(where: { $0.path == path })
   }
   
   func findByUrl(_ url: URL) -> DownloadItem? {
      downloads.first(where: { $0.url == url })
   }
   
   func append(_ item: DownloadItem) {
      downloads.append(item)
      save()
   }
   
   func update(_ item: DownloadItem, persist: Bool = true) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads[index] = item
      }
      if persist {
         save()
      }
   }
   
   func remove(_ item: DownloadItem) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads.remove(at: index)
      }
      save()
   }
   
   private static func load() -> [DownloadItem] {
      do {
         let data = try Data(contentsOf: savePath)
         return try JSONDecoder().decode([DownloadItem].self, from: data)
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to load download store: %{public}@", error.localizedDescription)
         return []
      }
   }
   
   private func save() {
      let encoder = JSONEncoder()
      do {
         let data = try encoder.encode(downloads)
         try data.write(to: DownloadStore.savePath, options: .atomic)
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to save download store: %{public}@", error.localizedDescription)
      }
   }
}
