//
//  DownloadState.swift
//  DownloadManagerKit
//

/// Represents the various states of a download item.
public enum DownloadStatus: String, Codable, Sendable {
   case pending, idle, inProgress, paused, cancelled, completed, unknown
}
