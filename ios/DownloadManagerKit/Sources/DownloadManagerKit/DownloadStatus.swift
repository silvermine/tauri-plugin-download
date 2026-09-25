//
//  DownloadStatus.swift
//  DownloadManagerKit
//

/// Represents the various states of a download item.
public enum DownloadStatus: String, Codable, CaseIterable, Sendable {
   /// Download has been created and is ready to start.
   case idle
   /// Download is in progress.
   case inProgress
   /// Download was in progress but has been paused.
   case paused
   /// Transfer failed; resume retries it and cancel discards it.
   case failed
   /// Download was canceled by the user.
   case canceled
   /// Download completed.
   case completed
}
