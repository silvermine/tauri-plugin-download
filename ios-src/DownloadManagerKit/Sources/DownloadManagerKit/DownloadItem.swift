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
   public private(set) var status: DownloadStatus
   public var resumeDataPath: URL?
   
   init(url: URL, path: URL, progress: Double = 0.0, status: DownloadStatus = .idle, resumeDataPath: URL? = nil) {
      self.url = url
      self.path = path
      self.progress = progress
      self.status = status
      self.resumeDataPath = resumeDataPath
   }
   
   public mutating func setProgress(_ progress: Double) {
      self.progress = progress
   }
   
   public mutating func setStatus(_ status: DownloadStatus) {
      self.status = status
   }
}
