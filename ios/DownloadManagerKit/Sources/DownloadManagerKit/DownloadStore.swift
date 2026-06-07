//
//  DownloadStore.swift
//  DownloadManagerKit
//

import Foundation
import os.log

struct PersistedDownloadItem: Codable {
   let url: URL
   let path: URL
   let legacyProgress: Double?
   let transferredBytes: Int64
   let totalBytes: Int64?
   let status: DownloadStatus
   let resumeDataPath: URL?

   enum CodingKeys: String, CodingKey {
      case url
      case path
      case progress
      case transferredBytes
      case totalBytes
      case status
      case resumeDataPath
   }

   init(item: DownloadItem) {
      url = item.url
      path = item.path
      legacyProgress = nil
      transferredBytes = item.transferredBytes
      totalBytes = item.totalBytes
      status = item.status
      resumeDataPath = item.resumeDataPath
   }

   init(from decoder: Decoder) throws {
      let container = try decoder.container(keyedBy: CodingKeys.self)
      url = try container.decode(URL.self, forKey: .url)
      path = try container.decode(URL.self, forKey: .path)
      legacyProgress = try container.decodeIfPresent(Double.self, forKey: .progress)
      transferredBytes = try container.decodeIfPresent(Int64.self, forKey: .transferredBytes) ?? 0
      totalBytes = try container.decodeIfPresent(Int64.self, forKey: .totalBytes)
      status = try container.decode(DownloadStatus.self, forKey: .status)
      resumeDataPath = try container.decodeIfPresent(URL.self, forKey: .resumeDataPath)
   }

   func encode(to encoder: Encoder) throws {
      var container = encoder.container(keyedBy: CodingKeys.self)
      try container.encode(url, forKey: .url)
      try container.encode(path, forKey: .path)
      try container.encode(transferredBytes, forKey: .transferredBytes)
      try container.encode(totalBytes, forKey: .totalBytes)
      try container.encode(status, forKey: .status)
      try container.encode(resumeDataPath, forKey: .resumeDataPath)
   }

   var downloadItem: DownloadItem {
      let progress: Double
      if transferredBytes == 0, totalBytes == nil, let legacyProgress {
         progress = legacyProgress
      } else {
         progress = DownloadItem.progress(for: transferredBytes, totalBytes: totalBytes, status: status)
      }

      return DownloadItem(
         url: url,
         path: path,
         progress: progress,
         transferredBytes: transferredBytes,
         totalBytes: totalBytes,
         status: status,
         resumeDataPath: resumeDataPath
      )
   }
}

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
         return try JSONDecoder().decode([PersistedDownloadItem].self, from: data).map(\.downloadItem)
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to load download store: %{public}@", error.localizedDescription)
         return []
      }
   }
   
   private func save() {
      let encoder = JSONEncoder()
      do {
         let data = try encoder.encode(downloads.map(PersistedDownloadItem.init))
         try data.write(to: DownloadStore.savePath, options: .atomic)
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to save download store: %{public}@", error.localizedDescription)
      }
   }
}
