import XCTest
@testable import DownloadManagerKit

final class DownloadRecoveryTests: XCTestCase {
   private func fixture() async throws -> (DownloadManager, DownloadStore, URL) {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
      let store = DownloadStore(savePath: directory.appendingPathComponent("downloads.json"))
      let manager = DownloadManager(store: store, configuration: .ephemeral)
      _ = await manager.list() // Finish initialization before seeding the callback's record.
      return (manager, store, directory)
   }

   private func record(in directory: URL) -> DownloadRecord {
      DownloadRecord(url: URL(string: "https://example.com/file")!,
         path: directory.appendingPathComponent("output/file").path,
         receivedBytes: 7, totalBytes: 7, status: .inProgress)
   }

   private func resumeError(_ code: Int) -> NSError {
      NSError(domain: NSURLErrorDomain, code: code,
         userInfo: [NSURLSessionDownloadTaskResumeData: Data("partial".utf8)])
   }

   func testSystemCancellationPreservesResumeDataInEitherReconciliationOrder() async throws {
      for reconcileFirst in [false, true] {
         let (manager, store, directory) = try await fixture()
         let original = record(in: directory)
         await store.append(original)
         if reconcileFirst { await manager.reconcileStore() }
         await manager.handleError(path: original.path, error: resumeError(NSURLErrorCancelled))
         if !reconcileFirst { await manager.reconcileStore() }
         let paused = await store.findByPath(original.path)
         XCTAssertEqual(paused?.status, .paused)
         XCTAssertNil(paused?.error)
         let resumeURL = try XCTUnwrap(paused?.resumeDataPath)
         defer { try? FileManager.default.removeItem(at: resumeURL) }
         XCTAssertEqual(try Data(contentsOf: resumeURL), Data("partial".utf8))
         let reloaded = DownloadStore(savePath: directory.appendingPathComponent("downloads.json"))
         let persisted = await reloaded.findByPath(original.path)
         XCTAssertEqual(persisted?.resumeDataPath, resumeURL)
         XCTAssertEqual(persisted?.status, .paused)
      }
   }

   func testTransferFailureRetainsResumeDataAndReason() async throws {
      let (manager, store, directory) = try await fixture()
      let original = record(in: directory)
      await store.append(original)
      await manager.handleError(path: original.path, error: resumeError(NSURLErrorTimedOut))
      let failed = await store.findByPath(original.path)
      XCTAssertEqual(failed?.status, .failed)
      XCTAssertEqual(failed?.error?.code, "timeout")
      let resumeURL = try XCTUnwrap(failed?.resumeDataPath)
      defer { try? FileManager.default.removeItem(at: resumeURL) }
      XCTAssertEqual(try Data(contentsOf: resumeURL), Data("partial".utf8))
   }

   func testPauseKeepsExistingResumeDataAndCancelCannotBeRestored() async throws {
      let (manager, store, directory) = try await fixture()
      var original = record(in: directory)
      original.setStatus(.paused)
      let existing = directory.appendingPathComponent("existing.resume")
      try Data("existing".utf8).write(to: existing)
      original.resumeDataPath = existing
      await store.append(original)
      await manager.handleError(path: original.path, error: resumeError(NSURLErrorCancelled))
      let paused = await store.findByPath(original.path)
      XCTAssertEqual(paused?.resumeDataPath, existing)
      _ = try await manager.cancel(path: original.path)
      await manager.handleError(path: original.path, error: resumeError(NSURLErrorCancelled))
      let canceled = await store.findByPath(original.path)
      XCTAssertNil(canceled)
      XCTAssertFalse(FileManager.default.fileExists(atPath: existing.path))
   }

   /// Registers before the callback so observing a fast completion is deterministic.
   private func observe(_ manager: DownloadManager, status: DownloadStatus) async -> XCTestExpectation {
      let done = expectation(description: "Received \(status)")
      var continuation: AsyncStream<DownloadItem>.Continuation!
      let stream = AsyncStream<DownloadItem> { continuation = $0 }
      _ = await manager.downloadContinuation.add(continuation)
      let task = Task {
         for await item in stream where item.status == status {
            done.fulfill()
            break
         }
      }
      addTeardownBlock { task.cancel() }
      return done
   }

   func testStagedFileSurvivesFailureAndResumeRetriesPlacement() async throws {
      let (manager, store, directory) = try await fixture()
      let original = record(in: directory)
      let parent = original.fileURL.deletingLastPathComponent()
      try Data("blocked".utf8).write(to: parent)
      let staged = directory.appendingPathComponent("staged")
      try Data("payload".utf8).write(to: staged)
      await store.append(original)
      await manager.handleFinished(path: original.path, location: staged)
      let failed = await store.findByPath(original.path)
      XCTAssertEqual(failed?.status, .failed)
      XCTAssertEqual(failed?.error?.code, "file")
      XCTAssertEqual(failed?.stagedFilePath, staged)
      XCTAssertTrue(FileManager.default.fileExists(atPath: staged.path))
      try FileManager.default.removeItem(at: parent)
      let done = await observe(manager, status: .completed)
      let resumed = try await manager.resume(path: original.path)
      XCTAssertNil(resumed.download.error)
      await fulfillment(of: [done], timeout: 3)
      XCTAssertEqual(try Data(contentsOf: original.fileURL), Data("payload".utf8))
      XCTAssertFalse(FileManager.default.fileExists(atPath: staged.path))
      let finished = await store.findByPath(original.path)
      XCTAssertNil(finished)
   }

   func testCancelRemovesFailedStagedFile() async throws {
      let (manager, store, directory) = try await fixture()
      let original = record(in: directory)
      let staged = directory.appendingPathComponent("staged")
      try Data("payload".utf8).write(to: staged)
      await store.append(original)
      _ = await store.fail(path: original.path,
         error: DownloadFailure(code: "file", message: "Cannot place file", retryability: "unknown"), stagedFilePath: staged)
      _ = try await manager.cancel(path: original.path)
      XCTAssertFalse(FileManager.default.fileExists(atPath: staged.path))
      let canceled = await store.findByPath(original.path)
      XCTAssertNil(canceled)
   }

   func testDelegateRejectsHTTPErrorBodyBeforePlacement() async throws {
      let (manager, store, directory) = try await fixture()
      let original = record(in: directory)
      await store.append(original)
      let body = directory.appendingPathComponent("response")
      try Data("error body".utf8).write(to: body)
      let task = ResponseTask()
      task.taskDescription = original.path
      task.httpResponse = HTTPURLResponse(url: original.url, statusCode: 404, httpVersion: nil, headerFields: nil)
      let delegate = DownloadSessionDelegate()
      delegate.manager = manager
      let done = await observe(manager, status: .failed)
      delegate.urlSession(.shared, downloadTask: task, didFinishDownloadingTo: body)
      await fulfillment(of: [done], timeout: 3)
      let failed = await store.findByPath(original.path)
      XCTAssertEqual(failed?.error?.httpStatus, 404)
      XCTAssertEqual(failed?.status, .failed)
      XCTAssertFalse(FileManager.default.fileExists(atPath: original.path))
      XCTAssertNil(failed?.stagedFilePath)
   }
}

private final class ResponseTask: URLSessionDownloadTask, @unchecked Sendable {
   var httpResponse: HTTPURLResponse?
   override var response: URLResponse? { httpResponse }
}
