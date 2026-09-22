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

      // An error body is not the resource: counted, it would be reported as progress
      // and its length saved as the total.
      let statusCode = (downloadTask.response as? HTTPURLResponse)?.statusCode
      if let statusCode, !Self.isSuccessStatus(statusCode) { return }

      Task {
         await self.manager?.handleProgress(path: path, totalBytesWritten: totalBytesWritten, totalBytesExpectedToWrite: totalBytesExpectedToWrite)
      }
   }
   
   func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
      guard let path = downloadTask.taskDescription else { return }
      let statusCode = (downloadTask.response as? HTTPURLResponse)?.statusCode

      // URLSession does not treat a 4xx or 5xx as a transfer error: it reports a
      // finished download whose file is the server's error body. Read the status
      // before taking the file, or that body lands at the caller's destination and
      // is announced as Completed. A non-HTTP response reports no status.
      if let statusCode, !Self.isSuccessStatus(statusCode) {
         Task {
            await self.manager?.handleFailedResponse(path: path, statusCode: statusCode)
         }
         return
      }

      // File must be moved synchronously before this method returns - iOS deletes it after
      let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      do {
         try FileManager.default.moveItem(at: location, to: tempURL)
      } catch {
         Task { await self.manager?.handleError(path: path, error: error) }
         return
      }
      
      // An empty body produces no didWriteData callback at all, so the header's total
      // has never been seen. Read it here instead of losing it.
      let expectedBytes = downloadTask.response?.expectedContentLength ?? NSURLSessionTransferSizeUnknown

      Task {
         await self.manager?.handleFinished(path: path, location: tempURL, expectedBytes: expectedBytes)
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

   /// Whether a response status means the body is the requested resource. 206 belongs
   /// here: it answers the range request resume() normally sends.
   static func isSuccessStatus(_ statusCode: Int) -> Bool {
      return (200..<300).contains(statusCode)
   }
}
