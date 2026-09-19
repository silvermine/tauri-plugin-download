//
//  DownloadRecord.swift
//  DownloadManagerKit
//

import Foundation

/// Persisted download record, stored in `downloads.json`. Mirrors the Rust
/// `DownloadRecord` in `crates/download-manager/src/models.rs`.
///
/// Carries no `progress` — that is derived, and lives only on [`DownloadItem`],
/// the type sent to the frontend. `resumeDataPath` is an internal URLSession
/// concern that deliberately never leaves this type.
struct DownloadRecord: Codable, Sendable {
   let url: URL
   let path: String
   let options: CreateOptions
   private(set) var receivedBytes: UInt64
   private(set) var totalBytes: UInt64?
   private(set) var status: DownloadStatus
   var resumeDataPath: URL?

   init(
      url: URL,
      path: String,
      options: CreateOptions = CreateOptions(),
      receivedBytes: UInt64 = 0,
      totalBytes: UInt64? = nil,
      status: DownloadStatus = .idle,
      resumeDataPath: URL? = nil
   ) {
      self.url = url
      self.path = path
      self.options = options
      self.receivedBytes = receivedBytes
      self.totalBytes = totalBytes
      self.status = status
      self.resumeDataPath = resumeDataPath
   }

   /// Sets the byte counts. A nil total means "this callback did not report one",
   /// not "there is none": a resume the server answers without a content length
   /// cannot know a total an earlier response already established, so nil never
   /// erases a total the record holds. Pass a value to widen or correct it.
   mutating func setBytes(received: UInt64, total: UInt64? = nil) {
      self.receivedBytes = received
      self.totalBytes = total ?? self.totalBytes
   }

   mutating func setResumeDataPath(_ resumeDataPath: URL?) {
      self.resumeDataPath = resumeDataPath
   }

   mutating func setStatus(_ status: DownloadStatus) {
      self.status = status
   }

   /// The file to write, for filesystem calls. `path` stays a `String` because it is
   /// the download's identity, which a `URL` would percent-encode.
   var fileURL: URL {
      URL(fileURLWithPath: path)
   }

   /// Builds the public payload, computing `progress` from the byte counts.
   ///
   /// A completed download always reports 100%, even without a content length.
   /// In flight without one it reports 0%, since `receivedBytes` is then the
   /// only meaningful signal.
   func toItem() -> DownloadItem {
      let progress: Double

      if status == .completed {
         progress = 100.0
      } else if let total = totalBytes, total > 0 {
         progress = min(max((Double(receivedBytes) / Double(total)) * 100.0, 0.0), 100.0)
      } else {
         progress = 0.0
      }

      return DownloadItem(
         url: url,
         path: path,
         options: options,
         receivedBytes: receivedBytes,
         totalBytes: totalBytes,
         progress: progress,
         status: status
      )
   }
}
