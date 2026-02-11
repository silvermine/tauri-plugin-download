//
//  DownloadManager.swift
//  DownloadManagerKit
//

import Foundation

/// A manager class responsible for handling download operations.
/// Used to provide functionality for downloading files, tracking download progress and handling completion events.
public final class DownloadManager: NSObject {
   public static let shared = DownloadManager()

   public var changed: AsyncStream<DownloadItem> {
       AsyncStream { continuation in
           Task {
               let id = await self.downloadContinuation.add(continuation)
               continuation.onTermination = { @Sendable _ in
                  Task {
                     await self.downloadContinuation.remove(id)
                  }
               }
           }
       }
   }
   
   let downloadContinuation = DownloadContinuation()
   
   private var sessionDelegate: DownloadSessionDelegate!
   private var session: URLSession!
   private let store = DownloadStore()
   private let backgroundSessionHandler = BackgroundSessionHandler()

   override init() {
      super.init()
      sessionDelegate = DownloadSessionDelegate()
      sessionDelegate.manager = self
      
      // delegateQueue: nil creates a serial operation queue for delegate callbacks by default
      let config = URLSessionConfiguration.background(withIdentifier: Bundle.main.bundleIdentifier!)
      session = URLSession(configuration: config, delegate: sessionDelegate, delegateQueue: nil)
      Task {
         await store.load()
      }
   }
   
   deinit {
      Task {
         await downloadContinuation.finish()
      }
   }
   
   public func setBackgroundCompletionHandler(_ handler: @escaping () -> Void) {
      Task {
         await backgroundSessionHandler.set(handler)
      }
   }
   
   /**
    Lists all download operations.

    - Returns: The list of download operations.
    */
   public func list() async -> [DownloadItem] {
      return await store.list()
   }
    
   /**
    Gets a download operation.

    If the download exists in the store, returns it. If not found, returns a download
    in `pending` state (not persisted to store). The caller can then call `create` to
    persist it and transition to `idle` state.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func get(path: URL) async -> DownloadItem {
      if let item = await store.findByPath(path) {
         return item
      }

      return DownloadItem(url: URL(fileURLWithPath: ""), path: path, status: .pending)
   }
   
   /**
    Creates a download operation.

    - Parameters:
      - path: The download path.
      - url: The download URL for the resource.
    - Returns: The download operation.
    */
   public func create(path: URL, url: URL) async -> DownloadActionResponse {
      if let existing = await store.findByPath(path) {
         return DownloadActionResponse(download: existing, expectedStatus: .idle)
      }

      let item = DownloadItem(url: url, path: path)
      await store.append(item)
      await emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Starts a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func start(path: URL) async throws -> DownloadActionResponse {
      guard var item = await store.findByPath(path) else {
         throw DownloadError.notFound(path.path)
      }

      guard item.status == .idle else {
         return DownloadActionResponse(download: item, expectedStatus: .inProgress)
      }
      
      let task = session.downloadTask(with: item.url)
      task.taskDescription = path.path
      task.resume()
      
      item.setStatus(.inProgress)
      await store.update(item)
      await emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Resumes a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func resume(path: URL) async throws -> DownloadActionResponse {
      guard var item = await store.findByPath(path) else {
         throw DownloadError.notFound(path.path)
      }
      
      guard item.status == .paused,
            let data = loadResumeData(for: item) else {
         return DownloadActionResponse(download: item, expectedStatus: .inProgress)
      }
      
      let task = session.downloadTask(withResumeData: data)
      task.taskDescription = path.path
      task.resume()
      deleteResumeData(for: item)

      item.setResumeDataPath(nil)
      item.setStatus(.inProgress)
      await store.update(item)
      await emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Pauses a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func pause(path: URL) async throws -> DownloadActionResponse {
      guard var item = await store.findByPath(path) else {
         throw DownloadError.notFound(path.path)
      }

      guard item.status == .inProgress,
            let task = await getDownloadTask(path.path) else {
         return DownloadActionResponse(download: item, expectedStatus: .paused)
      }
      
      task.cancel(byProducingResumeData: { data in
         if let data = data {
            Task {
               item.setResumeDataPath(self.saveResumeData(data))
               await self.store.update(item)
            }
         }
      })
      
      item.setStatus(.paused)
      await store.update(item)
      await emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Cancels a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func cancel(path: URL) async throws -> DownloadActionResponse {
      guard var item = await store.findByPath(path) else {
         throw DownloadError.notFound(path.path)
      }

      guard item.status == .idle || item.status == .inProgress || item.status == .paused else {
         return DownloadActionResponse(download: item, expectedStatus: .cancelled)
      }
      
      if let task = await getDownloadTask(path.path) {
         task.cancel()
      }
      
      if let _ = loadResumeData(for: item) {
         deleteResumeData(for: item)
         item.setResumeDataPath(nil)
      }
      
      item.setStatus(.cancelled)
      await store.remove(item)
      await emitChanged(item)
      
      return DownloadActionResponse(download: item)
   }

   /**
    Handler for download progress updates. Called by DownloadSessionDelegate.

    - Parameters:
      - url: The URL of the download.
      - totalBytesWritten: The total number of bytes transferred so far.
      - totalBytesExpectedToWrite: The expected length of the file.
    */
   func handleProgress(url: URL, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) async {
      guard var item = await store.findByUrl(url),
            totalBytesExpectedToWrite > 0 else { return }
      
      let progress = Double(totalBytesWritten) / Double(totalBytesExpectedToWrite) * 100
      
      // Throttle progress updates - only emit if progress increases by at least 1%
      let progressThreshold = 1.0
      if progress < 100.0 && progress - item.progress < progressThreshold {
         return
      }
      
      item.setProgress(progress)
      await store.update(item, persist: false)
      await emitChanged(item)
   }

   /**
    Handler for download completion. Called by DownloadSessionDelegate.
    The file has already been moved to a temp location by the delegate.

    - Parameters:
      - url: The URL of the download.
      - location: The temporary location of the downloaded file.
    */
   func handleFinished(url: URL, location: URL) async {
      guard var item = await store.findByUrl(url) else {
         try? FileManager.default.removeItem(at: location)
         return
      }

      // Ensure parent directory exists.
      let parentDirectory = item.path.deletingLastPathComponent()
      if !FileManager.default.fileExists(atPath: parentDirectory.path) {
         try? FileManager.default.createDirectory(at: parentDirectory, withIntermediateDirectories: true)
      }

      // Remove existing item (if found) and move downloaded item to destination path.
      try? FileManager.default.removeItem(at: item.path)
      try? FileManager.default.moveItem(at: location, to: item.path)

      item.setStatus(.completed)
      await store.remove(item)
      await emitChanged(item)
   }
   
   /**
    Handler for download errors. Called by DownloadSessionDelegate.

    - Parameters:
      - url: The URL of the download.
      - error: An error object indicating how the transfer failed, or nil if successful.
    */
   func handleError(url: URL, error: Error?) async {
      guard let error = error,
            var item = await store.findByUrl(url) else { return }
      
      // Cancellation with resume data. For user-invoked pauses, the pause() closure
      // persists resume data first so we skip here. For system-initiated cancellations
      // (e.g., network loss, app terminated), this is the only path that saves it.
      if let data = (error as NSError).userInfo[NSURLSessionDownloadTaskResumeData] as? Data {
         if item.resumeDataPath == nil {
            item.setResumeDataPath(saveResumeData(data))
            item.setStatus(.paused)
            await store.update(item)
            await emitChanged(item)
         }
         return
      }
      
      // Download failed - update status and clean up
      deleteResumeData(for: item)
      item.setStatus(.cancelled)
      await store.remove(item)
      await emitChanged(item)
   }
   
   /**
    Handler for background session completion. Called by DownloadSessionDelegate.
    The completion handler must be called to let the system know we're done processing.
    If the handler hasn't been set yet (race condition), defers until it is set.
    */
   func handleBackgroundSessionComplete() {
      Task {
         await backgroundSessionHandler.handleComplete()
      }
   }

   func loadResumeData(for item: DownloadItem) -> Data? {
      guard let url = item.resumeDataPath else { return nil }
      return try? Data(contentsOf: url)
   }
   
   func saveResumeData(_ data: Data) -> URL {
      let filename = UUID().uuidString + ".resumedata"
      let url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent(filename)
      try? data.write(to: url)
      return url
   }
   
   func deleteResumeData(for item: DownloadItem) {
      guard let url = item.resumeDataPath else { return }
      try? FileManager.default.removeItem(at: url)
   }
   
   func getDownloadTask(_ path: String) async -> URLSessionDownloadTask? {
      let tasks = await session.allTasks
      return tasks.compactMap { $0 as? URLSessionDownloadTask }
         .first { $0.taskDescription == path }
   }

   func emitChanged(_ item: DownloadItem) async {
      await downloadContinuation.yield(item)
   }
}
