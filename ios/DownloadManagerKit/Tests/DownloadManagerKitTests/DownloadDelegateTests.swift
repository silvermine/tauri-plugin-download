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
      delegate.urlSession(.shared, downloadTask: task, didWriteData: 7, totalBytesWritten: 7, totalBytesExpectedToWrite: 7)
      // The rejection happens synchronously in the delegate; no manager task is scheduled.
      let record = await store.findByPath(task.taskDescription!)
      XCTAssertEqual(record?.receivedBytes, 0)
      XCTAssertNil(record?.totalBytes)
      _ = manager
   }

   func testBackgroundCompletionBeforeAndAfterRegistration() async throws {
      for completeFirst in [false, true] {
         let (manager, _, _, delegate, _) = try await fixture()
         let done = expectation(description: "Background completion delivered once")
         done.assertForOverFulfill = true
         if completeFirst { delegate.urlSessionDidFinishEvents(forBackgroundURLSession: .shared) }
         manager.setBackgroundCompletionHandler {
            XCTAssertTrue(Thread.isMainThread)
            done.fulfill()
         }
         if !completeFirst { delegate.urlSessionDidFinishEvents(forBackgroundURLSession: .shared) }
         await fulfillment(of: [done], timeout: 3)
      }
   }
}

private final class DelegateTask: URLSessionDownloadTask, @unchecked Sendable {
   var httpResponse: HTTPURLResponse?
   override var response: URLResponse? { httpResponse }
}
