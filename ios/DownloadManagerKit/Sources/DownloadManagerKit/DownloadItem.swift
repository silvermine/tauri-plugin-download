//
//  DownloadItem.swift
//  DownloadManagerKit
//

import Foundation

/// A value type that represents an item to be downloaded.
/// Used to track the status and progress of a download operation.
public struct DownloadItem: Identifiable, Codable, Sendable {
   public var id: URL { path }
   
   public let url: URL
   public let path: URL
   public private(set) var progress: Double
   public private(set) var transferredBytes: Int64
   public private(set) var totalBytes: Int64?
   public private(set) var status: DownloadStatus
   public var resumeDataPath: URL?

   enum CodingKeys: String, CodingKey {
      case url
      case path
      case progress
      case transferredBytes
      case totalBytes
      case status
      case resumeDataPath
   }
   
   init(
      url: URL,
      path: URL,
      progress: Double = 0.0,
      transferredBytes: Int64 = 0,
      totalBytes: Int64? = nil,
      status: DownloadStatus = .idle,
      resumeDataPath: URL? = nil
   ) {
      self.url = url
      self.path = path
      self.progress = progress
      self.transferredBytes = transferredBytes
      self.totalBytes = totalBytes
      self.status = status
      self.resumeDataPath = resumeDataPath
   }

   public init(from decoder: Decoder) throws {
      let container = try decoder.container(keyedBy: CodingKeys.self)

      url = try container.decode(URL.self, forKey: .url)
      path = try container.decode(URL.self, forKey: .path)
      progress = try container.decode(Double.self, forKey: .progress)
      transferredBytes = try container.decodeIfPresent(Int64.self, forKey: .transferredBytes) ?? 0
      totalBytes = try container.decodeIfPresent(Int64.self, forKey: .totalBytes)
      status = try container.decode(DownloadStatus.self, forKey: .status)
      resumeDataPath = try container.decodeIfPresent(URL.self, forKey: .resumeDataPath)
   }

   public func encode(to encoder: Encoder) throws {
      var container = encoder.container(keyedBy: CodingKeys.self)

      try container.encode(url, forKey: .url)
      try container.encode(path, forKey: .path)
      try container.encode(progress, forKey: .progress)
      try container.encode(transferredBytes, forKey: .transferredBytes)
      try container.encode(totalBytes, forKey: .totalBytes)
      try container.encode(status, forKey: .status)
      try container.encode(resumeDataPath, forKey: .resumeDataPath)
   }
   
   public mutating func setTransfer(_ transferredBytes: Int64, _ totalBytes: Int64?) {
      if let totalBytes, totalBytes > 0 {
         progress = Double(transferredBytes) / Double(totalBytes) * 100
      } else {
         progress = 0.0
      }
      self.transferredBytes = transferredBytes
      self.totalBytes = totalBytes
   }
   
   public mutating func setResumeDataPath(_ resumeDataPath: URL?) {
      self.resumeDataPath = resumeDataPath
   }
   
   public mutating func setStatus(_ status: DownloadStatus) {
      if status == .completed {
         progress = 100.0
         totalBytes = totalBytes ?? transferredBytes
      }
      self.status = status
   }
}
