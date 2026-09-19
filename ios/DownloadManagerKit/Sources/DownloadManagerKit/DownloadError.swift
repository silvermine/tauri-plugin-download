//
//  DownloadError.swift
//  DownloadManagerKit
//

import Foundation

/// Represents possible errors that can occur during download operations.
public enum DownloadError: Error {
   case notFound(String)
   case invalidPath(String)
   case invalidURL(String)
}

/// Gives each error the message Rust and Android send for the same case. The plugin
/// rejects with `localizedDescription`, or `"\(error)"` when a command throws; without
/// these, both render the enum case instead of a message.
extension DownloadError: LocalizedError, CustomStringConvertible {
   public var errorDescription: String? {
      return description
   }

   public var description: String {
      switch self {
      case .notFound(let path):
         return "Not Found: \(path)"
      case .invalidPath(let message):
         return "Path Error: \(message)"
      case .invalidURL(let message):
         return "URL Error: \(message)"
      }
   }
}
