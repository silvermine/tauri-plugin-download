//
//  DownloadManager.swift
//  DownloadManagerKit
//

import Foundation
import os.log

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

   private let userAgentHolder = UserAgentHolder()
   
   private var sessionDelegate: DownloadSessionDelegate!
   private var session: URLSession!
   private let store: DownloadStore
   private let backgroundSessionHandler = BackgroundSessionHandler()

   /// Reconciliation runs once, at init. Assigned before any caller can reach the
   /// instance and never written again; every public entry point awaits it so a
   /// caller cannot observe or mutate a record it is about to rewrite.
   ///
   /// `var` and optional rather than `let`: as an NSObject subclass every stored
   /// property must be initialized before `super.init()`, but the task needs the
   /// `self` that only exists after it.
   private var reconcileTask: Task<Void, Never>?

   override convenience init() {
      self.init(store: DownloadStore(), configuration: .background(withIdentifier: Bundle.main.bundleIdentifier!))
   }

   /// Allows manager callbacks to be exercised with an isolated store and session.
   init(store: DownloadStore, configuration: URLSessionConfiguration) {
      self.store = store
      super.init()
      sessionDelegate = DownloadSessionDelegate()
      sessionDelegate.manager = self
      session = URLSession(configuration: configuration, delegate: sessionDelegate, delegateQueue: nil)
      reconcileTask = Task { [weak self] in
         await self?.reconcileStore()
      }
   }

   deinit {
      let continuation = downloadContinuation
      Task { await continuation.finish() }
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
      await ensureReconciled()

      return await store.list().map { $0.toItem() }
   }
    
   /**
    Gets a download operation.

    - Parameter path: The download path.
    - Returns: The download operation, or `nil` if no download exists for the path.
    */
   public func get(path: String) async -> DownloadItem? {
      await ensureReconciled()

      return await store.findByPath(path)?.toItem()
   }
   
   /**
    Creates a download operation.

    Options are fixed on creation: an existing record is returned unchanged,
    keeping its original options, as on desktop.

    - Parameters:
      - path: The download path.
      - url: The download URL for the resource.
      - options: Network policy persisted with the download.
    - Returns: The download operation.
    */
   public func create(
      path: String,
      url: URL,
      options: CreateOptions = CreateOptions()
   ) async -> DownloadActionResponse {
      await ensureReconciled()

      if let existing = await store.findByPath(path) {
         return DownloadActionResponse(download: existing.toItem(), expectedStatus: .idle)
      }

      let record = DownloadRecord(url: url, path: path, options: options)
      await store.append(record)
      let item = await emitChanged(record)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Sets the user agent sent with every download request.

    Applied per request, not on the session configuration: the background session is
    built in `init()` before any value can arrive, and is not one to recreate at
    runtime. Same reason the network policy is per request.

    The holder is tested; that `start(path:)` and `resume(path:)` read it is not, and
    cannot be until this package can intercept the session's requests.

    - Parameter userAgent: The user agent, or `nil` to leave URLSession's default.
    */
   public func setUserAgent(_ userAgent: String?) async {
      await userAgentHolder.set(userAgent)
   }

   /**
    Sets the directory holding the download store.

    Static, and unlike `setUserAgent` must be called *before* `shared` is first touched:
    the store opens its file as the manager initializes, so there is nothing left to move
    afterwards. A late call throws rather than reporting a success it did not deliver.

    Creates the directory, so a location the app cannot write to fails here rather than
    silently costing every record later — `save` can only log a write failure.

    Unset, the store stays in the app's Application Support directory.

    - Parameter url: The directory to persist the store in.
    - Throws: `StoreLocationError.alreadyResolved` if the store has already opened its
      file, or the underlying error if the directory cannot be created.
    */
   public static func setStoreDirectory(_ url: URL) throws {
      try StoreLocation.set(url)
   }

   /**
    Starts a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func start(path: String) async throws -> DownloadActionResponse {
      await ensureReconciled()

      guard let (claimed, accepted) = try await store.beginTransfer(path: path, allowed: [.idle]) else {
         throw DownloadError.notFound(path)
      }
      guard accepted else {
         return DownloadActionResponse(download: claimed.toItem(), expectedStatus: .inProgress)
      }
      let record = claimed

      let request = Self.request(for: record, userAgent: await userAgentHolder.value)
      let task = session.downloadTask(with: request)
      task.taskDescription = path
      let item = await emitChanged(record)
      task.resume()
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Resumes a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func resume(path: String) async throws -> DownloadActionResponse {
      await ensureReconciled()

      guard let (claimed, accepted) = try await store.beginTransfer(path: path, allowed: [.paused, .failed]) else {
         throw DownloadError.notFound(path)
      }
      guard accepted else {
         return DownloadActionResponse(download: claimed.toItem(), expectedStatus: .inProgress)
      }
      var record = claimed

      if let staged = record.stagedFilePath, FileManager.default.fileExists(atPath: staged.path) {
         let item = await emitChanged(record)
         Task { await self.handleFinished(path: path, location: staged) }
         return DownloadActionResponse(download: item)
      }

      // Absence is not a refusal. pause() sets .paused whether or not URLSession
      // produced resume data, so a server without byte-range support leaves a paused
      // record with none — and requiring it here left that record stuck for good.
      // Rust and Android also restart from zero when their partial file is missing.
      let resumeData = loadResumeData(for: record)
      let task: URLSessionDownloadTask

      if let resumeData, !resumeData.isEmpty {
         // The one task this manager creates without building the request: resume
         // data carries the original NSURLRequest, so the policy and user agent set
         // by request(for:) at start() survive. Confirmed on device, but Apple does
         // not document resume data as preserving request properties — re-check on
         // new iOS majors.
         task = session.downloadTask(withResumeData: resumeData)
      } else {
         os_log(.info, log: Log.downloadManager,
                "No usable resume data for %{public}@, restarting from zero",
                record.fileURL.lastPathComponent)

         // Reset before the task starts, so the first callback is not overwritten.
         deleteResumeData(for: record)
         record.setResumeDataPath(nil)
         record.stagedFilePath = nil
         record.setBytes(received: 0, total: record.totalBytes)
         await store.update(record)

         let request = Self.request(for: record, userAgent: await userAgentHolder.value)
         task = session.downloadTask(with: request)
      }

      task.taskDescription = path
      task.resume()

      // Only now that the task owns the data can its file go. Deleting it any
      // earlier means a process death in this window loses the partial download
      // outright, rather than leaving it resumable. A no-op on the restart path,
      // which cleared its own file already.
      deleteResumeData(for: record)

      // In the actor: the task is already running, so a progress callback landing
      // between a read and a write here would lose its byte count.
      let updated = await mutateRecord(path: path) { $0.setResumeDataPath(nil) }

      let item = await emitChanged(updated ?? record)
      
      return DownloadActionResponse(download: item)
   }
   
   /**
    Pauses a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func pause(path: String) async throws -> DownloadActionResponse {
      await ensureReconciled()

      guard let record = await store.findByPath(path) else {
         throw DownloadError.notFound(path)
      }

      guard record.status == .inProgress,
            let task = await getDownloadTask(path) else {
         return DownloadActionResponse(download: record.toItem(), expectedStatus: .paused)
      }
      
      // Three writers can be in flight at once — this callback, the status mutation
      // below, and handleError. mutateRecord applies each inside the actor, so the
      // first to persist a path wins and the duplicate file is cleaned up.
      task.cancel(byProducingResumeData: { [weak self] data in
         guard let self, let data else { return }
         let savedURL = self.saveResumeData(data)
         Task {
            let updated = await self.mutateRecord(path: path) { current in
               if current.resumeDataPath == nil {
                  current.setResumeDataPath(savedURL)
               }
            }
            if let updated, updated.resumeDataPath != savedURL {
               try? FileManager.default.removeItem(at: savedURL)
            }
         }
      })

      // Persisting here is what carries the last progress tick's byte count
      // across the pause.
      let updated = await mutateRecord(path: path) { $0.setStatus(.paused) }
      let result = updated ?? record
      let item = await emitChanged(result)

      return DownloadActionResponse(download: item)
   }
   
   /**
    Cancels a download operation.

    - Parameter path: The download path.
    - Returns: The download operation.
    */
   public func cancel(path: String) async throws -> DownloadActionResponse {
      await ensureReconciled()

      guard var record = await store.findByPath(path) else {
         throw DownloadError.notFound(path)
      }

      guard record.status == .idle || record.status == .inProgress || record.status == .paused || record.status == .failed else {
         return DownloadActionResponse(download: record.toItem(), expectedStatus: .canceled)
      }
      
      let task = await getDownloadTask(path)
      
      if let _ = loadResumeData(for: record) {
         deleteResumeData(for: record)
         record.setResumeDataPath(nil)
      }
      
      if let staged = record.stagedFilePath { try? FileManager.default.removeItem(at: staged) }
      record.setStatus(.canceled)
      await store.remove(record)

      // Only once the record is gone: the task's own cancel callback would otherwise
      // find it still inProgress and revert it.
      task?.cancel()

      let item = await emitChanged(record)
      
      return DownloadActionResponse(download: item)
   }

   /**
    Handler for download progress updates. Called by DownloadSessionDelegate.

    - Parameters:
      - path: The download path, which is the task's description.
      - totalBytesWritten: The total number of bytes transferred so far.
      - totalBytesExpectedToWrite: The expected length of the file, or a negative
        value when the server did not supply a content length.
    */
   func handleProgress(path: String, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) async {
      guard var record = await store.findByPath(path),
            record.status == .inProgress else { return }

      let receivedBytes = UInt64(max(totalBytesWritten, 0))
      let totalBytes = DownloadManager.statedTotal(totalBytesExpectedToWrite)

      // Record a known total independently of the throttle below. handleFinished
      // reads the total back off the record, so a download that completes without
      // ever passing the throttle — a small file whose first callback is already
      // at 100% — would otherwise report no total.
      if let totalBytes, let updated = await store.setTotalIfChanged(path: record.path, total: totalBytes) {
         record = updated
      }
      
      // This callback may report no total (a resume the server answers without a
      // content length) for a record that already knows one. setBytes() coalesces
      // on its own, but the tracker takes the total directly, and without it the
      // cadence would silently fall back to the byte threshold.
      let effectiveTotal = totalBytes ?? record.totalBytes

      // Every emission writes the byte count back to the store, so the stored
      // record is itself the last-emitted baseline. That keeps throttle state off
      // this class, which would otherwise need synchronizing across callbacks.
      let tracker = ProgressTracker(
         lastEmittedBytes: record.receivedBytes,
         receivedBytes: receivedBytes,
         totalBytes: effectiveTotal
      )

      // Completion is reported by handleFinished, which sizes the landed file.
      guard tracker.shouldEmit, !tracker.isComplete else { return }

      // Bytes only, in the actor: a pause landing between this callback's read and
      // its write would otherwise be overwritten by this stale InProgress. Emit what
      // the store returned, for the same reason.
      guard let updated = await mutateRecord(path: record.path, persist: false, {
         if $0.status == .inProgress { $0.setBytes(received: receivedBytes, total: effectiveTotal) }
      }) else {
         return
      }

      await emitChanged(updated)
   }

   /**
    Handler for download completion. Called by DownloadSessionDelegate.
    The file has already been moved to a temp location by the delegate.

    - Parameters:
      - path: The download path, which is the task's description.
      - location: The temporary location of the downloaded file.
      - expectedBytes: The response's stated length, or a negative value when it
        stated none.
    */
   func handleFinished(path: String, location: URL, expectedBytes: Int64 = NSURLSessionTransferSizeUnknown) async {
      guard var record = await store.findByPath(path), record.status == .inProgress else {
         try? FileManager.default.removeItem(at: location)
         return
      }

      do {
         try DownloadManager.placeDownloadedFile(from: location, to: record.fileURL)
      } catch {
         os_log(.error, log: Log.downloadManager, "Failed to place %{public}@: %{public}@",
                record.fileURL.lastPathComponent, error.localizedDescription)

         if let failed = await store.fail(path: record.path, error: DownloadFailure.classify(error), stagedFilePath: location) {
            await emitChanged(failed)
         }

         return
      }

      // Size the file that landed. URLSession's counters already include the
      // resume offset, so this agrees with them; it is here because the file on
      // disk is the authority on what was actually written, and the last progress
      // callback may have been throttled away before completion.
      let receivedBytes = DownloadManager.fileSize(at: record.fileURL) ?? record.receivedBytes

      // Falls back to the response's own figure: an empty body produces no progress
      // callback, so nothing has recorded the header's total by now.
      let totalBytes = record.totalBytes ?? DownloadManager.statedTotal(expectedBytes)

      record.setBytes(received: receivedBytes, total: totalBytes)
      record.setStatus(.completed)
      await store.remove(record)
      await emitChanged(record)
   }
   
   /// Places a finished download's temporary file at its destination, creating the
   /// parent directory and replacing whatever is already there. Static and throwing
   /// so the failure path can be tested; moveItem() fails rather than replacing, so
   /// the destination is removed first.
   static func placeDownloadedFile(from location: URL, to destination: URL) throws {
      let parentDirectory = destination.deletingLastPathComponent()

      if !FileManager.default.fileExists(atPath: parentDirectory.path) {
         try FileManager.default.createDirectory(at: parentDirectory, withIntermediateDirectories: true)
      }

      try? FileManager.default.removeItem(at: destination)
      try FileManager.default.moveItem(at: location, to: destination)
   }

   /**
    Handler for download errors. Called by DownloadSessionDelegate.

    - Parameters:
      - path: The download path, which is the task's description.
      - error: An error object indicating how the transfer failed, or nil if successful.
    */
   func handleError(path: String, error: Error?) async {
      guard let error, let record = await store.findByPath(path) else { return }
      let native = error as NSError
      let data = native.userInfo[NSURLSessionDownloadTaskResumeData] as? Data
      let savedURL = data.map { saveResumeData($0) }

      // A system cancellation can arrive after startup reconciled the task to Idle.
      // Keep its resume data and publish Paused, but never interrupt a replacement task.
      let task = await getDownloadTask(path)
      let hasLiveTask = task.map { $0.state != .completed && $0.state != .canceling } ?? false
      let canceled = native.domain == NSURLErrorDomain && native.code == NSURLErrorCancelled
      if let savedURL, !hasLiveTask, canceled || record.status == .paused {
         let updated = await mutateRecord(path: path) { current in
            guard [.idle, .inProgress, .paused].contains(current.status) else { return }
            current.setStatus(.paused)
            if current.resumeDataPath == nil { current.resumeDataPath = savedURL }
         }
         if updated?.resumeDataPath != savedURL { try? FileManager.default.removeItem(at: savedURL) }
         if let updated, updated.status == .paused { await emitChanged(updated) }
         return
      }
      if record.status == .paused || canceled && hasLiveTask {
         if let savedURL { try? FileManager.default.removeItem(at: savedURL) }
         return
      }
      // URLSession calls completion only after its task has ended. A background task
      // waiting for connectivity has not completed and never reaches this transition.
      if let failed = await store.fail(path: record.path, error: DownloadFailure.classify(error), resumeDataPath: savedURL) {
         await emitChanged(failed)
      } else if let savedURL {
         try? FileManager.default.removeItem(at: savedURL)
      }
   }

   /// Rejects an HTTP error body before it can be placed at the destination.
   func handleFailedResponse(path: String, statusCode: Int) async {
      await handleError(path: path, error: DownloadFailure.http(statusCode))
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

   private func ensureReconciled() async {
      await reconcileTask?.value
   }

   /// Builds the request a download's task runs on, applying the record's network
   /// policy.
   ///
   /// The policy is per-request rather than per-session because one background
   /// session is shared by every download. Its configuration stays permissive and
   /// the effective policy is the intersection of the two, so restricting here is
   /// what makes the option per-download.
   ///
   /// A restricted task is not rejected: a background session waits for a path
   /// that satisfies the request and starts transferring once one appears.
   /// Desktop, having no such scheduler, rejects `start()`/`resume()` instead.
   static func request(for record: DownloadRecord, userAgent: String?) -> URLRequest {
      var request = URLRequest(url: record.url)
      let allowMetered = record.options.allowMetered

      // allowsExpensiveNetworkAccess covers cellular and personal hotspots;
      // allowsConstrainedNetworkAccess covers Low Data Mode. Together with
      // allowsCellularAccess they match the desktop check, which rejects a
      // connection reported as either metered or constrained.
      request.allowsCellularAccess = allowMetered
      request.allowsExpensiveNetworkAccess = allowMetered
      request.allowsConstrainedNetworkAccess = allowMetered

      if let userAgent {
         request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
      }

      return request
   }

   /// Decides what a record left `inProgress` with no task behind it should become.
   /// Returns nil when the record needs no change.
   ///
   /// `start()` and `resume()` both commit `inProgress` to the store before calling
   /// `task.resume()`. A process death in that window strands the record: `start()`
   /// requires `idle` and `resume()` requires `paused`, so nothing but `cancel()`
   /// would clear it. A record with resume data can still be resumed, so it becomes
   /// `paused` and keeps its byte count; one without restarts from scratch, so it
   /// becomes `idle` at zero. The total is kept either way — headers established it.
   static func reconciledRecord(_ record: DownloadRecord, hasLiveTask: Bool) -> DownloadRecord? {
      guard record.status == .inProgress, !hasLiveTask else { return nil }

      var updated = record

      if record.resumeDataPath == nil {
         updated.setBytes(received: 0, total: record.totalBytes)
         updated.setStatus(.idle)
      } else {
         updated.setStatus(.paused)
      }

      return updated
   }

   /// Reconciles the store against the session's live tasks.
   ///
   /// Background-session tasks outlive the process and are restored alongside the
   /// session, so a record is reverted only when the session reports no task for its
   /// path — otherwise a download still running would be clobbered. Android needs no
   /// such check: its worker constructs the manager before transferring, so
   /// reconciliation always precedes any transfer — though a backoff or the unmetered
   /// constraint can leave a record reading Paused or Idle for a while first.
   ///
   /// That check is also why the delegate handlers — handleProgress, handleFinished,
   /// handleError — do not await ensureReconciled(), and must not start: gating them
   /// would serialize every progress callback behind reconciliation for no benefit.
   /// They are already safe because
   ///
   /// - this reverts only records with no live task, while a callback fires only for
   ///   a task that exists, so the two act on disjoint records;
   /// - a new task can only appear via start()/resume(), which do await the gate; and
   /// - if a restored task finishes mid-reconcile and handleFinished removes its
   ///   record, the batched update cannot resurrect it — DownloadStore's
   ///   update(_ records:) writes only paths that still exist.
   func reconcileStore() async {
      let livePaths = Set(await session.allTasks.compactMap { $0.taskDescription })

      var reconciled: [DownloadRecord] = []

      for record in await store.list() {
         guard let updated = DownloadManager.reconciledRecord(
            record,
            hasLiveTask: livePaths.contains(record.path)
         ) else {
            continue
         }

         reconciled.append(updated)
         os_log(.info, log: Log.downloadManager, "Reconciled %{public}@ to %{public}@",
                record.fileURL.lastPathComponent, String(describing: updated.status))
      }

      await store.update(reconciled)
   }

   func loadResumeData(for record: DownloadRecord) -> Data? {
      guard let url = record.resumeDataPath else { return nil }
      return try? Data(contentsOf: url)
   }
   
   func saveResumeData(_ data: Data) -> URL {
      let filename = UUID().uuidString + ".resumedata"
      let url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent(filename)
      try? data.write(to: url)
      return url
   }
   
   func deleteResumeData(for record: DownloadRecord) {
      guard let url = record.resumeDataPath else { return }
      try? FileManager.default.removeItem(at: url)
   }
   
   /// Applies `body` to the stored record at `path` and returns what actually landed,
   /// or nil when no record has that path. One await, so the read and the write cannot
   /// be interleaved; see DownloadStore.mutate().
   private func mutateRecord(
      path: String,
      persist: Bool = true,
      _ body: @Sendable (inout DownloadRecord) -> Void
   ) async -> DownloadRecord? {
      return await store.mutate(path: path, persist: persist, body)
   }
   
   func getDownloadTask(_ path: String) async -> URLSessionDownloadTask? {
      let tasks = await session.allTasks
      return tasks.compactMap { $0 as? URLSessionDownloadTask }
         .first { $0.taskDescription == path }
   }

   /// Emits the public payload derived from `record` and returns it for reuse in
   /// a `DownloadActionResponse`.
   @discardableResult
   func emitChanged(_ record: DownloadRecord) async -> DownloadItem {
      let item = record.toItem()
      await downloadContinuation.yield(item)
      return item
   }

   /// A response's stated length, or nil when it stated none.
   ///
   /// URLSession reports `NSURLSessionTransferSizeUnknown` (-1) for a body whose
   /// length the server withheld, such as a chunked response. A stated zero is a
   /// known total rather than an unknown one: an empty body is a complete download,
   /// and collapsing it to nil disagreed with desktop, which reports 0.
   static func statedTotal(_ expectedBytes: Int64) -> UInt64? {
      return expectedBytes >= 0 ? UInt64(expectedBytes) : nil
   }

   static func fileSize(at url: URL) -> UInt64? {
      guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
            let size = attributes[.size] as? NSNumber else {
         return nil
      }
      return size.uint64Value
   }
}
