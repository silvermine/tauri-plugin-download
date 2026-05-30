//
//  DownloadSessionDelegate.swift
//  DownloadManagerKit
//

import Foundation

/// URLSession delegate that forwards callbacks to the DownloadManager.
/// This class receives callbacks on a background serial queue and dispatches to async handler methods.
final class DownloadSessionDelegate: NSObject, URLSessionDownloadDelegate {
   weak var manager: DownloadManager?
   
   func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
      guard let path = downloadTask.taskDescription else { return }
      Task {
         await self.manager?.handleProgress(path: path, bytesWritten: bytesWritten, totalBytesWritten: totalBytesWritten, totalBytesExpectedToWrite: totalBytesExpectedToWrite)
      }
   }

   func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didResumeAtOffset fileOffset: Int64, expectedTotalBytes: Int64) {
      guard let path = downloadTask.taskDescription else { return }
      Task {
         await self.manager?.handleResumed(path: path, fileOffset: fileOffset, expectedTotalBytes: expectedTotalBytes)
      }
   }
   
   func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
      guard let path = downloadTask.taskDescription else { return }
      
      // File must be moved synchronously before this method returns - iOS deletes it after
      let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      do {
         try FileManager.default.moveItem(at: location, to: tempURL)
      } catch {
         return
      }
      
      Task {
         await self.manager?.handleFinished(path: path, location: tempURL)
      }
   }
   
   func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
      guard let path = task.taskDescription else { return }
      Task {
         await self.manager?.handleError(path: path, error: error)
      }
   }
   
   func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
      self.manager?.handleBackgroundSessionComplete()
   }
}
