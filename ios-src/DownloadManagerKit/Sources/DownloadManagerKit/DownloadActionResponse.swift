//
//  DownloadActionResponse.swift
//  DownloadManagerKit
//

import Foundation

/// Response from a download action containing the download item and status information.
public struct DownloadActionResponse: Codable {
   public let download: DownloadItem
   public let expectedStatus: DownloadStatus
   public let isExpectedStatus: Bool
   
   public init(download: DownloadItem) {
      self.download = download
      self.expectedStatus = download.status
      self.isExpectedStatus = true
   }
   
   public init(download: DownloadItem, expectedStatus: DownloadStatus) {
      self.download = download
      self.expectedStatus = expectedStatus
      self.isExpectedStatus = download.status == expectedStatus
   }
}
