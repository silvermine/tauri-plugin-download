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
   
   private var sessionDelegate: DownloadSessionDelegate!
   private var session: URLSession!
   
   /// Thread-safe store for the downloads array.
   private actor DownloadStore {
      var downloads: [DownloadItem] = []
      
      func getDownloads() -> [DownloadItem] { downloads }
      
      func setDownloads(_ downloads: [DownloadItem]) {
         self.downloads = downloads
      }
      
      func findByPath(_ path: URL) -> DownloadItem? {
         downloads.first(where: { $0.path == path })
      }
      
      func findByUrl(_ url: URL) -> DownloadItem? {
         downloads.first(where: { $0.url == url })
      }
      
      func append(_ item: DownloadItem) {
         downloads.append(item)
      }
      
      func update(_ item: DownloadItem) {
         if let index = downloads.firstIndex(where: { $0.path == item.path }) {
            downloads[index] = item
         }
      }
      
      func remove(_ item: DownloadItem) {
         if let index = downloads.firstIndex(where: { $0.path == item.path }) {
            downloads.remove(at: index)
         }
      }
   }
   
   private let store = DownloadStore()
   private var backgroundCompletionHandler: (() -> Void)?
   private var pendingBackgroundComplete = false

   override init() {
      super.init()
      sessionDelegate = DownloadSessionDelegate()
      sessionDelegate.manager = self
      
      // delegateQueue: nil creates a serial operation queue for delegate callbacks by default
      let config = URLSessionConfiguration.background(withIdentifier: Bundle.main.bundleIdentifier!)
      session = URLSession(configuration: config, delegate: sessionDelegate, delegateQueue: nil)
      Task {
         await loadState()
      }
   }
   
   deinit {
      Task {
         await downloadContinuation.finish()
      }
   }
   
   public func setBackgroundCompletionHandler(_ handler: @escaping () -> Void) {
      backgroundCompletionHandler = handler
      if pendingBackgroundComplete {
         pendingBackgroundComplete = false
         handler()
         backgroundCompletionHandler = nil
      }
   }
   
   /**
    Lists all download operations.

    - Returns: The list of download operations.
    */
   public func list() async -> [DownloadItem] {
      return await store.getDownloads()
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
      await saveState()
      emitChanged(item)
      
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
      await saveState()
      emitChanged(item)
      
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
      deleteResumeData(for: &item)
      
      item.setStatus(.inProgress)
      await store.update(item)
      await saveState()
      emitChanged(item)
      
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
               await self.saveResumeDataAsync(data, for: item)
            }
         }
      })
      
      item.setStatus(.paused)
      await store.update(item)
      await saveState()
      emitChanged(item)
      
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
         deleteResumeData(for: &item)
      }
      
      item.setStatus(.cancelled)
      await store.remove(item)
      await saveState()
      emitChanged(item)
      
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
      await store.update(item)
      emitChanged(item)
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
      await saveState()
      emitChanged(item)
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
      
      // Check if this is a cancellation with resume data (i.e., a pause)
      let userInfo = (error as NSError).userInfo
      if let resumeData = userInfo[NSURLSessionDownloadTaskResumeData] as? Data {
         await saveResumeDataAsync(resumeData, for: item)
         return
      }
      
      // Download failed - update status and clean up
      item.setStatus(.cancelled)
      await store.remove(item)
      await saveState()
      emitChanged(item)
      deleteResumeData(for: &item)
   }
   
   /**
    Handler for background session completion. Called by DownloadSessionDelegate.
    The completion handler must be called to let the system know we're done processing.
    If the handler hasn't been set yet (race condition), defers until it is set.
    */
   func handleBackgroundSessionComplete() {
      if let handler = backgroundCompletionHandler {
         handler()
         backgroundCompletionHandler = nil
      } else {
         pendingBackgroundComplete = true
      }
   }

   func loadResumeData(for item: DownloadItem) -> Data? {
      guard let url = item.resumeDataPath else { return nil }
      return try? Data(contentsOf: url)
   }
   
   func saveResumeData(_ data: Data, for item: inout DownloadItem) {
      let filename = UUID().uuidString + ".resumedata"
      let url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent(filename)
      try? data.write(to: url)
      item.resumeDataPath = url
   }
   
   private func saveResumeDataAsync(_ data: Data, for item: DownloadItem) async {
      var item = item
      saveResumeData(data, for: &item)
      await store.update(item)
      await saveState()
   }
   
   func deleteResumeData(for item: inout DownloadItem) {
      guard let url = item.resumeDataPath else { return }
      try? FileManager.default.removeItem(at: url)
      item.resumeDataPath = nil
   }
   
   func loadState() async {
      let saved = queue.sync { () -> [DownloadItem]? in
         let decoder = JSONDecoder()
         guard let data = try? Data(contentsOf: savePath),
               let items = try? decoder.decode([DownloadItem].self, from: data) else { return nil }
         return items
      }
      if let saved = saved {
         await store.setDownloads(saved)
      }
   }

   func saveState() async {
      let currentDownloads = await store.getDownloads()
      queue.sync(flags: .barrier) {
         let encoder = JSONEncoder()
         if let data = try? encoder.encode(currentDownloads) {
            try? data.write(to: savePath)
         }
      }
   }
   
   func getDownloadTask(_ path: String) async -> URLSessionDownloadTask? {
      await withCheckedContinuation { continuation in
         session.getAllTasks { tasks in
            let task = tasks.compactMap { $0 as? URLSessionDownloadTask }
               .first { $0.taskDescription == path }
            continuation.resume(returning: task)
         }
      }
   }

   func emitChanged(_ item: DownloadItem) {
      Task {
         await downloadContinuation.yield(item)
      }
   }
}
