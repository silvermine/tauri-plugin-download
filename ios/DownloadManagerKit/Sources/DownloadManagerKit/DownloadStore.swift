//
//  DownloadStore.swift
//  DownloadManagerKit
//

import CoreFoundation
import Foundation
import os.log

private let currentSchemaVersion: UInt32 = 2

private enum StoreDecodingError: LocalizedError {
   case malformedEnvelope
   case unsupportedVersion(UInt32)
   case invalidRecords

   var errorDescription: String? {
      switch self {
      case .malformedEnvelope:
         return "Malformed store envelope"
      case .unsupportedVersion(let version):
         return "Unsupported store version: \(version) (expected \(currentSchemaVersion))"
      case .invalidRecords:
         return "Invalid store records"
      }
   }
}

/// Reject Boolean and floating-point versions before JSONDecoder converts
/// whole-valued decimals and exponents to UInt32. StoreDocument checks the range,
/// supported version, and record payload after this number-type check.
private func validateVersionToken(in data: Data) throws {
   guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
         let version = root["version"] as? NSNumber,
         CFGetTypeID(version) != CFBooleanGetTypeID(),
         !CFNumberIsFloatType(version)
   else { throw StoreDecodingError.malformedEnvelope }
}

/// v1 records decode with absent optional error/staged-file fields; all writes use v2.
private struct StoreDocument: Codable {
   let version: UInt32
   let downloads: [DownloadRecord]

   private enum CodingKeys: String, CodingKey {
      case version, downloads
   }

   init(downloads: [DownloadRecord]) {
      self.version = currentSchemaVersion
      self.downloads = downloads
   }

   init(from decoder: Decoder) throws {
      let container: KeyedDecodingContainer<CodingKeys>
      do {
         container = try decoder.container(keyedBy: CodingKeys.self)
         version = try container.decode(UInt32.self, forKey: .version)
         // Check the array shape without decoding records from a future schema.
         _ = try container.nestedUnkeyedContainer(forKey: .downloads)
      } catch {
         throw StoreDecodingError.malformedEnvelope
      }

      guard version == 1 || version == currentSchemaVersion else {
         throw StoreDecodingError.unsupportedVersion(version)
      }
      do {
         downloads = try container.decode([DownloadRecord].self, forKey: .downloads)
         guard !downloads.contains(where: { $0.status == .failed && $0.error == nil }) else {
            throw StoreDecodingError.invalidRecords
         }
      } catch {
         throw StoreDecodingError.invalidRecords
      }
   }
}

/// Thread-safe store for versioned download records, persisted to an atomically
/// written JSON file.
actor DownloadStore {
   private var downloads: [DownloadRecord]
   private let savePath: URL

   /// - Parameter savePath: Where the store is persisted. Injectable so tests can
   ///   work against a temporary file rather than the app's Application Support
   ///   directory.
   ///   Defaults through [`StoreLocation`], which is what a configured directory
   ///   reaches this class by — the path has to be known here, since the file is read
   ///   below rather than on first use.
   init(savePath: URL = StoreLocation.savePath()) {
      self.savePath = savePath
      self.downloads = DownloadStore.load(from: savePath)
   }

   func list() -> [DownloadRecord] { downloads }
   
   func findByPath(_ path: String) -> DownloadRecord? {
      downloads.first(where: { $0.path == path })
   }
   
   /// Claims an attempt in one actor hop, so concurrent resume calls cannot both start.
   func beginTransfer(path: String, allowed: [DownloadStatus]) throws -> (DownloadRecord, Bool)? {
      guard let index = downloads.firstIndex(where: { $0.path == path }) else { return nil }
      guard allowed.contains(downloads[index].status) else { return (downloads[index], false) }
      let previous = downloads[index]
      downloads[index].setStatus(.inProgress)
      do {
         try saveThrowing()
      } catch {
         downloads[index] = previous
         throw DownloadFailure(code: "store", message: error.localizedDescription, retryability: "unknown")
      }
      return (downloads[index], true)
   }

   /// Records only failures of active transfers, retaining usable resume or staged data.
   func fail(path: String, error: DownloadFailure, resumeDataPath: URL? = nil, stagedFilePath: URL? = nil) -> DownloadRecord? {
      guard let index = downloads.firstIndex(where: { $0.path == path }),
            downloads[index].status == .inProgress else { return nil }
      downloads[index].setStatus(.failed)
      downloads[index].error = error
      if let resumeDataPath { downloads[index].resumeDataPath = resumeDataPath }
      if let stagedFilePath { downloads[index].stagedFilePath = stagedFilePath }
      if downloads[index].resumeDataPath == nil && downloads[index].stagedFilePath == nil {
         downloads[index].setBytes(received: 0)
      }
      save()
      return downloads[index]
   }

   func append(_ item: DownloadRecord) {
      downloads.append(item)
      save()
   }
   
   func update(_ item: DownloadRecord, persist: Bool = true) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads[index] = item
      }
      if persist {
         save()
      }
   }
   
   /// Records a newly-learned total in a single actor hop, returning the updated
   /// record, or nil when the total was already known.
   ///
   /// Progress callbacks arrive as unordered tasks, so a compare-and-set composed
   /// from a separate `findByPath` and `update` lets several callbacks each read an
   /// unknown total and each act on it. Deliberately does not persist: pause,
   /// cancel and completion all write the record, and this runs on the hottest
   /// callback in the system.
   func setTotalIfChanged(path: String, total: UInt64) -> DownloadRecord? {
      guard let index = downloads.firstIndex(where: { $0.path == path }),
            downloads[index].totalBytes != total else {
         return nil
      }

      downloads[index].setBytes(received: downloads[index].receivedBytes, total: total)

      return downloads[index]
   }

   /// Applies `body` in one actor hop and returns the stored record, or nil when no
   /// record has that path. Composed from `findByPath` and `update` it would suspend
   /// between read and write and lose concurrent changes; `body` is synchronous for
   /// the same reason.
   func mutate(path: String, persist: Bool, _ body: @Sendable (inout DownloadRecord) -> Void) -> DownloadRecord? {
      guard let index = downloads.firstIndex(where: { $0.path == path }) else {
         return nil
      }

      body(&downloads[index])

      if persist {
         save()
      }

      return downloads[index]
   }

   /// Applies several record updates with a single write.
   ///
   /// Reconciliation can revert many records at once; one write per record would
   /// re-encode and rewrite the whole file that many times. Unknown paths are
   /// ignored.
   func update(_ records: [DownloadRecord]) {
      guard !records.isEmpty else { return }

      for record in records {
         if let index = downloads.firstIndex(where: { $0.path == record.path }) {
            downloads[index] = record
         }
      }

      save()
   }

   func remove(_ item: DownloadRecord) {
      if let index = downloads.firstIndex(where: { $0.path == item.path }) {
         downloads.remove(at: index)
      }
      save()
   }
   
   /// Decodes the persisted store.
   ///
   /// Rejects the whole document if any record is malformed. Preserving an
   /// unreadable file before continuing empty is tracked separately in #64.
   static func load(from savePath: URL) -> [DownloadRecord] {
      do {
         let data = try Data(contentsOf: savePath)
         return try decodeRecords(from: data)
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to load download store: %{public}@", error.localizedDescription)
         return []
      }
   }

   /// Separate decoding from the existing log-and-continue load path so tests can
   /// distinguish invalid envelopes, unsupported versions, and invalid records.
   static func decodeRecords(from data: Data) throws -> [DownloadRecord] {
      do {
         try validateVersionToken(in: data)
         return try JSONDecoder().decode(StoreDocument.self, from: data).downloads
      } catch let error as StoreDecodingError {
         throw error
      } catch {
         // Raw decoder errors can expose document contents in the store logger.
         throw StoreDecodingError.malformedEnvelope
      }
   }

   private func save() {
      do {
         try saveThrowing()
      } catch {
         os_log(.error, log: Log.downloadStore, "Failed to save download store: %{public}@", error.localizedDescription)
      }
   }

   private func saveThrowing() throws {
      let data = try JSONEncoder().encode(StoreDocument(downloads: downloads))
      try FileManager.default.createDirectory(
         at: savePath.deletingLastPathComponent(),
         withIntermediateDirectories: true
      )
      try data.write(to: savePath, options: .atomic)
   }
}
