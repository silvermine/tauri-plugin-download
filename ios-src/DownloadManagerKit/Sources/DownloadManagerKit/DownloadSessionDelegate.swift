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
      guard let url = downloadTask.originalRequest?.url else { return }
      Task {
         await self.manager?.handleProgress(url: url, totalBytesWritten: totalBytesWritten, totalBytesExpectedToWrite: totalBytesExpectedToWrite)
      }
   }
   
   func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
      guard let url = downloadTask.originalRequest?.url else { return }
      
      // File must be moved synchronously before this method returns - iOS deletes it after
      let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      do {
         try FileManager.default.moveItem(at: location, to: tempURL)
      } catch {
         return
      }
      
      Task {
         await self.manager?.handleFinished(url: url, location: tempURL)
      }
   }
   
   func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
      guard let url = task.originalRequest?.url else { return }
      Task {
         await self.manager?.handleError(url: url, error: error)
      }
   }
   
   func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
      self.manager?.handleBackgroundSessionComplete()
   }
}
