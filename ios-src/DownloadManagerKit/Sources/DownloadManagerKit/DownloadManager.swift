//
//  DownloadManager.swift
//  DownloadManagerKit
//

import Combine
import Foundation

/// A manager class responsible for handling download operations.
/// Used to provide functionality for downloading files, tracking download progress and handling completion events.
public final class DownloadManager: NSObject, ObservableObject, URLSessionDownloadDelegate {
   public static let shared = DownloadManager()
   @Published public private(set) var downloads: [DownloadItem] = []
   
   public var changed: AsyncStream<DownloadItem> {
       AsyncStream { continuation in
           var id: UUID?
           Task {
               id = await downloadContinuation.add(continuation)
           }
           
           continuation.onTermination = { @Sendable _ in
              if let id = id {
                 Task {
                    await self.downloadContinuation.remove(id)
                 }
              }
           }
       }
   }
   
   let savePath = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("downloads.json")
   let queue = DispatchQueue(label: Bundle.main.bundleIdentifier!, attributes: .concurrent)
   let downloadContinuation = DownloadContinuation()
   
   var session: URLSession?
   public var backgroundCompletionHandler: (() -> Void)?

   override init() {
      super.init()
      let config = URLSessionConfiguration.background(withIdentifier: Bundle.main.bundleIdentifier!)
      session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
      loadState()
   }
   
   deinit {
      Task {
         await downloadContinuation.finish()
      }
   }
   
   public func list() -> [DownloadItem] {
       return downloads
   }
    
   public func get(path: URL) -> DownloadItem {
       if let item = downloads.first(where: { $0.path == path }) {
           return item
       }

       return DownloadItem(url: URL(fileURLWithPath: ""), path: path, status: .pending)
   }
   
   public func create(path: URL, url: URL) -> DownloadActionResponse {
      if downloads.contains(where: { $0.path == path }) {
         let existing = downloads.first(where: { $0.path == path })!
         return DownloadActionResponse(download: existing, expectedStatus: .idle)
      }

      let item = DownloadItem(url: url, path: path)
      downloads.append(item)
      saveState()
      emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }
   
   public func start(path: URL) throws -> DownloadActionResponse {
      guard let item = downloads.first(where: { $0.path == path }) else {
         throw DownloadError.notFound(path.path)
      }

      guard let session = session, item.status == .idle else {
         return DownloadActionResponse(download: item, expectedStatus: .inProgress)
      }
      
      let task = session.downloadTask(with: item.url)
      task.taskDescription = path.path
      task.resume()
      
      item.setStatus(.inProgress)
      if let index = downloads.firstIndex(where: {$0.path == path}) {
         downloads[index] = item
         saveState()
         emitChanged(item)
      }
      
      return DownloadActionResponse(download: item)
   }
   
   public func resume(path: URL) throws -> DownloadActionResponse {
      guard let item = downloads.first(where: { $0.path == path }) else {
         throw DownloadError.notFound(path.path)
      }
      
      guard item.status == .paused,
            let session = session,
            let data = loadResumeData(for: item) else {
         return DownloadActionResponse(download: item, expectedStatus: .inProgress)
      }
      
      let task = session.downloadTask(withResumeData: data)
      task.taskDescription = path.path
      task.resume()
      deleteResumeData(for: item)
      
      item.setStatus(.inProgress)
      if let index = self.downloads.firstIndex(where: {$0.path == path}) {
         downloads[index] = item
         saveState()
         emitChanged(item)
      }
      
      return DownloadActionResponse(download: item)
   }
   
   public func pause(path: URL) throws -> DownloadActionResponse {
      guard let item = downloads.first(where: { $0.path == path }) else {
         throw DownloadError.notFound(path.path)
      }

      guard item.status == .inProgress, let task = getDownloadTask(path.path) else {
         return DownloadActionResponse(download: item, expectedStatus: .paused)
      }
      
      task.cancel(byProducingResumeData: { data in
         if let data = data {
            self.saveResumeData(data, for: item)
         }
      })
      
      item.setStatus(.paused)
      if let index = self.downloads.firstIndex(where: {$0.path == path}) {
         downloads[index] = item
         saveState()
         emitChanged(item)
      }
      
      return DownloadActionResponse(download: item)
   }
   
   public func cancel(path: URL) throws -> DownloadActionResponse {
      guard let item = downloads.first(where: { $0.path == path }) else {
         throw DownloadError.notFound(path.path)
      }

      guard item.status == .idle || item.status == .inProgress || item.status == .paused else {
         return DownloadActionResponse(download: item, expectedStatus: .cancelled)
      }
      
      if let task = getDownloadTask(path.path) {
         task.cancel()
      }
      
      if let _ = loadResumeData(for: item) {
         deleteResumeData(for: item)
      }
      
      item.setStatus(.cancelled)
      if let index = self.downloads.firstIndex(where: {$0.path == path}) {
         downloads.remove(at: index)
         saveState()
         emitChanged(item)
      }
      
      return DownloadActionResponse(download: item)
   }

   /**
    URLSession delegate method called periodically to inform about download progress.
    This method is called periodically during a download operation to provide information about the amount of data that has been downloaded.

    - Parameters:
      - session: The URL session containing the download task.
      - downloadTask: The download task that provided data.
      - bytesWritten: The number of bytes that were written in the latest write operation.
      - totalBytesWritten: The total number of bytes transferred so far.
      - totalBytesExpectedToWrite: The expected length of the file, as provided by the Content-Length header. If this header was not provided, the value is NSURLSessionTransferSizeUnknown.
    */
   public func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
      guard let url = downloadTask.originalRequest?.url,
            let item = downloads.first(where: { $0.url == url }) else { return }

      item.setProgress(Double(totalBytesWritten) / Double(totalBytesExpectedToWrite) * 100)
      if let index = self.downloads.firstIndex(where: {$0.path == item.path}) {
         downloads[index] = item
         emitChanged(item)
      }
   }

   /**
    URLSession delegate method called when the download task has finished downloading.
    This method is called when the download task has completed successfully and the downloaded file is available at the specified location.

    - Parameters:
      - session: The URL session containing the download task.
      - downloadTask: The download task that finished downloading.
      - location: The temporary location of the downloaded file.
    */
   public func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
      guard let url = downloadTask.originalRequest?.url,
            let item = downloads.first(where: { $0.url == url }) else { return }

      // Ensure parent directory exists.
      let parentDirectory = item.path.deletingLastPathComponent()
      if !FileManager.default.fileExists(atPath: parentDirectory.path) {
         try? FileManager.default.createDirectory(at: parentDirectory, withIntermediateDirectories: true)
      }

      // Remove existing item (if found) and move downloaded item to destination path.
      try? FileManager.default.removeItem(at: item.path)
      try? FileManager.default.moveItem(at: location, to: item.path)

      item.setStatus(.completed)
      if let index = self.downloads.firstIndex(where: {$0.path == item.path}) {
         downloads.remove(at: index)
         saveState()
         emitChanged(item)
      }
   }
   
   /**
    URLSession delegate method called when a task completes with or without an error.
    This handles download failures, network errors, and cancellations.

    - Parameters:
      - session: The URL session containing the task.
      - task: The task that completed.
      - error: An error object indicating how the transfer failed, or nil if the transfer was successful.
    */
   public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
      guard let error = error,
            let url = task.originalRequest?.url,
            let item = downloads.first(where: { $0.url == url }) else { return }
      
      // Check if this is a cancellation with resume data (i.e., a pause)
      let userInfo = (error as NSError).userInfo
      if let resumeData = userInfo[NSURLSessionDownloadTaskResumeData] as? Data {
         saveResumeData(resumeData, for: item)
         return
      }
      
      // Download failed - update status and clean up
      item.setStatus(.cancelled)
      if let index = self.downloads.firstIndex(where: { $0.path == item.path }) {
         downloads.remove(at: index)
         saveState()
         emitChanged(item)
      }
      
      deleteResumeData(for: item)
   }
   
   /**
    URLSession delegate method called when all events for a background session have been delivered.
    This is called when the app is relaunched to handle background download completion.
    The completion handler must be called to let the system know we're done processing.
    */
   public func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
      DispatchQueue.main.async {
         self.backgroundCompletionHandler?()
         self.backgroundCompletionHandler = nil
      }
   }

   func loadResumeData(for item: DownloadItem) -> Data? {
      guard let url = item.resumeDataPath else { return nil }
      return try? Data(contentsOf: url)
   }
   
   func saveResumeData(_ data: Data, for item: DownloadItem) {
      let filename = UUID().uuidString + ".resumedata"
      let url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent(filename)
      try? data.write(to: url)
      item.resumeDataPath = url
      saveState()
   }
   
   func deleteResumeData(for item: DownloadItem) {
      guard let url = item.resumeDataPath else { return }
      try? FileManager.default.removeItem(at: url)
      item.resumeDataPath = nil
   }
   
   func loadState() {
      queue.sync {
         let decoder = JSONDecoder()
         if let data = try? Data(contentsOf: savePath),
            let saved = try? decoder.decode([DownloadItem].self, from: data) {
            downloads = saved
         }
      }
   }

   func saveState() {
      queue.sync(flags: .barrier) {
         let encoder = JSONEncoder()
         if let data = try? encoder.encode(downloads) {
            try? data.write(to: savePath)
         }
      }
   }
   
   func getDownloadTask(_ path: String) -> URLSessionDownloadTask? {
      var task: URLSessionDownloadTask? = nil
      let semaphore = DispatchSemaphore(value: 0)
      session?.getAllTasks { tasks in
         task = tasks.compactMap { $0 as? URLSessionDownloadTask }.first { $0.taskDescription == path }
         semaphore.signal()
      }
      
      semaphore.wait()
      return task
   }

   func emitChanged(_ item: DownloadItem) {
      Task {
         await downloadContinuation.yield(item)
      }
   }
}
