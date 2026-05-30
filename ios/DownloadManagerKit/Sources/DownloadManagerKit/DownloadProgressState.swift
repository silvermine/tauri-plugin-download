import Foundation

enum DownloadProgressState {
   private static let progressThreshold = 1.0
   private static let bytesThreshold: Int64 = 1024 * 1024

   static func transferredBytes(totalBytesWritten: Int64, resumeOffset: Int64) -> Int64 {
      totalBytesWritten + resumeOffset
   }

   static func totalBytes(expectedTotalBytes: Int64, currentTotalBytes: Int64?) -> Int64? {
      expectedTotalBytes > 0 ? expectedTotalBytes : currentTotalBytes
   }

   static func shouldThrottle(item: DownloadItem, transferredBytes: Int64, totalBytes: Int64?) -> Bool {
      if let totalBytes, totalBytes > 0 {
         let progress = Double(transferredBytes) / Double(totalBytes) * 100
         return progress < 100.0 && progress - item.progress < progressThreshold
      }

      return transferredBytes - item.transferredBytes < bytesThreshold
   }
}

 private enum DownloadTaskResumeByteAccounting {
   case unknown
   case bytesSinceResume
   case bytesIncludingResumeOffset
 }

 private struct DownloadTaskResumeProgress {
   let offset: Int64
   var accounting: DownloadTaskResumeByteAccounting = .unknown

   mutating func transferredBytes(bytesWritten: Int64, totalBytesWritten: Int64) -> Int64 {
      switch accounting {
      case .unknown:
         accounting = accountingMode(bytesWritten: bytesWritten, totalBytesWritten: totalBytesWritten)
      case .bytesSinceResume, .bytesIncludingResumeOffset:
         break
      }

      switch accounting {
      case .bytesIncludingResumeOffset:
         return totalBytesWritten
      case .unknown, .bytesSinceResume:
         return offset + totalBytesWritten
      }
   }

   private func accountingMode(bytesWritten: Int64, totalBytesWritten: Int64) -> DownloadTaskResumeByteAccounting {
      if offset <= 0 {
         return .bytesIncludingResumeOffset
      }

      if totalBytesWritten == bytesWritten {
         return .bytesSinceResume
      }

      if totalBytesWritten >= offset + bytesWritten {
         return .bytesIncludingResumeOffset
      }

      return .bytesSinceResume
   }
 }

actor DownloadTaskResumeOffsetStore {
   private var progressStates: [String: DownloadTaskResumeProgress] = [:]

   func offset(for path: String) -> Int64 {
      progressStates[path]?.offset ?? 0
   }

   func setOffset(_ offset: Int64, for path: String) {
      progressStates[path] = DownloadTaskResumeProgress(offset: offset)
   }

   func transferredBytes(bytesWritten: Int64, totalBytesWritten: Int64, for path: String) -> Int64 {
      guard var progressState = progressStates[path] else {
         return totalBytesWritten
      }

      let transferredBytes = progressState.transferredBytes(bytesWritten: bytesWritten, totalBytesWritten: totalBytesWritten)
      progressStates[path] = progressState

      return transferredBytes
   }

   func removeOffset(for path: String) {
      progressStates.removeValue(forKey: path)
   }
}
