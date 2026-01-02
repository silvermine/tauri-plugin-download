//
//  DownloadItem.swift
//  DownloadManagerKit
//

import Foundation

/// A class that represents an item to be downloaded.
/// Used to track the status and progress of a download operation.
public final class DownloadItem: ObservableObject, Identifiable, Codable {
   enum CodingKeys: CodingKey {
      case url, path, progress, status, resumeDataPath
   }
   
   public let url: URL
   public let path: URL
   @Published public private(set) var progress: Double
   @Published public private(set) var status: DownloadStatus
   public var resumeDataPath: URL?
   
   init(url: URL, path: URL, progress: Double = 0.0, status: DownloadStatus = .idle, resumeDataPath: URL? = nil) {
      self.url = url
      self.path = path
      self.progress = progress
      self.status = status;
      self.resumeDataPath = resumeDataPath
   }
   
   public required init(from decoder: Decoder) throws {
      let container = try decoder.container(keyedBy: CodingKeys.self)
      url = try container.decode(URL.self, forKey: .url)
      path = try container.decode(URL.self, forKey: .path)
      progress = try container.decode(Double.self, forKey: .progress)
      status = try container.decode(DownloadStatus.self, forKey: .status)
      resumeDataPath = try container.decodeIfPresent(URL.self, forKey: .resumeDataPath)
   }
   
   public func setProgress(_ progress: Double) {
      self.progress = progress
   }
   
   public func setStatus(_ status: DownloadStatus) {
      self.status = status
   }   
   
   public func encode(to encoder: Encoder) throws {
      var container = encoder.container(keyedBy: CodingKeys.self)
      try container.encode(url, forKey: .url)
      try container.encode(path, forKey: .path)
      try container.encode(progress, forKey: .progress)
      try container.encode(status, forKey: .status)
      try container.encode(resumeDataPath, forKey: .resumeDataPath)
   }
}
