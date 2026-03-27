//
//  DownloadStatus.swift
//  DownloadManagerKit
//

/// Represents the various states of a download item.
public enum DownloadStatus: String, Codable, Sendable {
   /// Status could not be determined.
   case unknown
   /// Download has not yet been created/persisted.
   case pending
   /// Download has been created and is ready to start.
   case idle
   /// Download is in progress.
   case inProgress
   /// Download was in progress but has been paused.
   case paused
   /// Download was canceled by the user.
   case canceled
   /// Download completed.
   case completed
}
