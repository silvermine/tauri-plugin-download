//
//  DownloadItem.swift
//  DownloadManagerKit
//

import Foundation

/// Emit-only payload sent to the frontend. Built from a [`DownloadRecord`] with
/// `progress` derived from `receivedBytes` / `totalBytes`; never persisted and
/// never decoded, so internal state such as the resume-data path stays out of
/// the frontend contract. `totalBytes` is nil when the server supplied no
/// content length, and encodes as an explicit JSON `null`.
public struct DownloadItem: Encodable, Sendable {
   public let url: URL
   public let path: String
   public let options: CreateOptions
   public let receivedBytes: UInt64
   public let totalBytes: UInt64?
   public let progress: Double
   public let status: DownloadStatus

   init(
      url: URL,
      path: String,
      options: CreateOptions,
      receivedBytes: UInt64,
      totalBytes: UInt64?,
      progress: Double,
      status: DownloadStatus
   ) {
      self.url = url
      self.path = path
      self.options = options
      self.receivedBytes = receivedBytes
      self.totalBytes = totalBytes
      self.progress = progress
      self.status = status
   }
   
   enum CodingKeys: String, CodingKey {
      case url, path, options, receivedBytes, totalBytes, progress, status
   }
   
   public func encode(to encoder: Encoder) throws {
      var container = encoder.container(keyedBy: CodingKeys.self)

      try container.encode(url, forKey: .url)
      try container.encode(path, forKey: .path)
      try container.encode(options, forKey: .options)
      try container.encode(receivedBytes, forKey: .receivedBytes)
      try container.encode(progress, forKey: .progress)
      try container.encode(status, forKey: .status)

      // The synthesized encoding would use encodeIfPresent and omit the key. The
      // TypeScript layer coalesces a missing key to null anyway (attachDownload in
      // guest-js/actions.ts), so this is not load-bearing for the plugin — it keeps
      // the payload self-describing for anything reading DownloadManagerKit directly.
      if let totalBytes = totalBytes {
         try container.encode(totalBytes, forKey: .totalBytes)
      } else {
         try container.encodeNil(forKey: .totalBytes)
      }
   }
}
