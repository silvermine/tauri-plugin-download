//
//  DownloadStore.swift
//  DownloadManagerKit
//

import Foundation

/// Thread-safe store for the downloads array.
actor DownloadStore {
   private var downloads: [DownloadItem] = []
   private let savePath = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("downloads.json")
   
   func list() -> [DownloadItem] { downloads }
   
   func findByPath(_ path: URL) -> DownloadItem? {
      downloads.first(where: { $0.path == path })
   }
   
   func findByUrl(_ url: URL) -> DownloadItem? {
      downloads.first(where: { $0.url == url })
   }
   
   func append(_ item: DownloadItem) {
      downloads.append(item)
   }
   
   func update(_ item: DownloadItem) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads[index] = item
      }
   }
   
   func remove(_ item: DownloadItem) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads.remove(at: index)
      }
   }
   
   func load() {
      let decoder = JSONDecoder()
      if let data = try? Data(contentsOf: savePath),
         let saved = try? decoder.decode([DownloadItem].self, from: data) {
         downloads = saved
      }
   }
   
   func save() {
      let encoder = JSONEncoder()
      if let data = try? encoder.encode(downloads) {
         try? data.write(to: savePath)
      }
   }
}
