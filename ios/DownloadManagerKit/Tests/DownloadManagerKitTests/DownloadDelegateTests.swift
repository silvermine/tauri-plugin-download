import Foundation
import XCTest
@testable import DownloadManagerKit

final class DownloadDelegateTests: XCTestCase {
   private func fixture() async throws -> (DownloadManager, DownloadStore, URL, DownloadSessionDelegate, DelegateTask) {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
      let store = DownloadStore(savePath: directory.appendingPathComponent("downloads.json"))
      let manager = DownloadManager(store: store, configuration: .ephemeral)
      _ = await manager.list()
      let path = directory.appendingPathComponent("output").path
      await store.append(DownloadRecord(url: URL(string: "https://example.com/file")!, path: path, status: .inProgress))
      let delegate = DownloadSessionDelegate()
      delegate.manager = manager
      let task = DelegateTask()
      task.taskDescription = path
      task.httpResponse = HTTPURLResponse(url: URL(string: "https://example.com/file")!, statusCode: 200,
         httpVersion: nil, headerFields: ["Content-Length": "7"])
      return (manager, store, directory, delegate, task)
   }

   private func observe(_ manager: DownloadManager, check: @escaping (DownloadItem) -> Bool) async -> XCTestExpectation {
      let received = expectation(description: "Delegate forwarded callback")
      var continuation: AsyncStream<DownloadItem>.Continuation!
      let stream = AsyncStream<DownloadItem> { continuation = $0 }
      let id = await manager.downloadContinuation.add(continuation)
      let observer = Task {
         for await item in stream where check(item) {
            received.fulfill()
            break
         }
      }
      addTeardownBlock {
         observer.cancel()
         await manager.downloadContinuation.remove(id)
      }
      return received
   }

   func testProgressCallbackUpdatesStoreAndEvent() async throws {
      let (manager, store, _, delegate, task) = try await fixture()
      let done = await observe(manager) { $0.receivedBytes == 3 && $0.totalBytes == 7 }
      delegate.urlSession(.shared, downloadTask: task, didWriteData: 3, totalBytesWritten: 3, totalBytesExpectedToWrite: 7)
      await fulfillment(of: [done], timeout: 3)
      let record = await store.findByPath(task.taskDescription!)
      XCTAssertEqual(record?.receivedBytes, 3)
      XCTAssertEqual(record?.totalBytes, 7)
   }

   func testCompletionCallbackPreservesAndPlacesTemporaryFile() async throws {
      let (manager, store, directory, delegate, task) = try await fixture()
      let source = directory.appendingPathComponent("session-temporary-file")
      try Data("payload".utf8).write(to: source)
      let done = await observe(manager) { $0.status == .completed && $0.receivedBytes == 7 && $0.totalBytes == 7 }
      delegate.urlSession(.shared, downloadTask: task, didFinishDownloadingTo: source)
      XCTAssertFalse(FileManager.default.fileExists(atPath: source.path), "Must take ownership before returning to URLSession")
      await fulfillment(of: [done], timeout: 3)
      XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: task.taskDescription!)), Data("payload".utf8))
      let record = await store.findByPath(task.taskDescription!)
      XCTAssertNil(record)
   }

   func testErrorCallbackPreservesResumeDataAndFailure() async throws {
      let (manager, store, _, delegate, task) = try await fixture()
      let done = await observe(manager) { $0.status == .failed && $0.error?.code == "timeout" }
      let error = NSError(domain: NSURLErrorDomain, code: NSURLErrorTimedOut,
         userInfo: [NSURLSessionDownloadTaskResumeData: Data("resume".utf8)])
      delegate.urlSession(.shared, task: task, didCompleteWithError: error)
      await fulfillment(of: [done], timeout: 3)
      let record = await store.findByPath(task.taskDescription!)
      let resume = try XCTUnwrap(record?.resumeDataPath)
      XCTAssertEqual(try Data(contentsOf: resume), Data("resume".utf8))
      _ = try await manager.cancel(path: task.taskDescription!)
   }

   func testHTTPErrorProgressIsIgnored() async throws {
      let (manager, store, _, delegate, task) = try await fixture()
      task.httpResponse = HTTPURLResponse(url: URL(string: "https://example.com/file")!, statusCode: 404,
         httpVersion: nil, headerFields: nil)
      let unexpected = await observe(manager) { $0.receivedBytes == 3 }
      unexpected.isInverted = true
      // A partial tick would emit progress if the delegate's HTTP guard were lost.
      delegate.urlSession(.shared, downloadTask: task, didWriteData: 3, totalBytesWritten: 3, totalBytesExpectedToWrite: 7)
      await fulfillment(of: [unexpected], timeout: 0.2)
      let record = await store.findByPath(task.taskDescription!)
      XCTAssertEqual(record?.receivedBytes, 0)
      XCTAssertNil(record?.totalBytes)
   }

   func testBackgroundCompletionIsForwardedToMainThread() async throws {
      let (manager, _, _, delegate, _) = try await fixture()
      let done = expectation(description: "Delegate forwards background completion")
      manager.setBackgroundCompletionHandler {
         XCTAssertTrue(Thread.isMainThread)
         done.fulfill()
      }
      delegate.urlSessionDidFinishEvents(forBackgroundURLSession: .shared)
      await fulfillment(of: [done], timeout: 3)
   }

   func testBackgroundHandlerSupportsBothArrivalOrders() async {
      for completeFirst in [false, true] {
         let handler = BackgroundSessionHandler()
         let calls = CompletionCount()
         let done = expectation(description: "First background completion delivered")
         // The manager wrappers launch independent Tasks, whose execution order
         // is unspecified. Await the actor directly to exercise both orders.
         if completeFirst { await handler.handleComplete() }
         await handler.set {
            XCTAssertTrue(Thread.isMainThread)
            if calls.increment() == 1 { done.fulfill() }
         }
         if !completeFirst { await handler.handleComplete() }
         await fulfillment(of: [done], timeout: 3)
         await handler.handleComplete()
         // handleComplete enqueues delivery on the serial main queue. Drain it
         // before checking the count so a second call cannot escape the assertion.
         let drained = expectation(description: "Main queue processed repeated completion")
         DispatchQueue.main.async { drained.fulfill() }
         await fulfillment(of: [drained], timeout: 3)
         XCTAssertEqual(calls.value, 1, "A registered handler must only be delivered once")
      }
   }
}

private final class CompletionCount {
   private let lock = NSLock()
   private var count = 0

   var value: Int {
      lock.lock()
      defer { lock.unlock() }
      return count
   }

   func increment() -> Int {
      lock.lock()
      defer { lock.unlock() }
      count += 1
      return count
   }
}

private final class DelegateTask: URLSessionDownloadTask, @unchecked Sendable {
   var httpResponse: HTTPURLResponse?
   override var response: URLResponse? { httpResponse }
}
