import Foundation
import XCTest
@testable import DownloadManagerKit

final class DownloadRequestPolicyTests: XCTestCase {
   func testStartAppliesStoredPolicyAndUserAgent() async throws {
      for allowed in [false, true] {
         try await checkRequest(allowMetered: allowed, resume: false)
      }
   }

   func testResumeWithoutDataAppliesStoredPolicyAndUserAgent() async throws {
      for allowed in [false, true] {
         try await checkRequest(allowMetered: allowed, resume: true)
      }
   }

   /// Observe the request received by URLSession, including the manager call site.
   private func checkRequest(allowMetered: Bool, resume: Bool) async throws {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      defer { try? FileManager.default.removeItem(at: directory) }
      let store = DownloadStore(savePath: directory.appendingPathComponent("downloads.json"))
      let configuration = URLSessionConfiguration.ephemeral
      configuration.protocolClasses = [RecordingDownloadProtocol.self]
      let manager = DownloadManager(store: store, configuration: configuration)
      let url = URL(string: "https://policy.invalid/\(UUID().uuidString)")!
      let path = directory.appendingPathComponent("payload").path
      let received = expectation(description: "URLSession received the request")
      RecordingDownloadProtocol.register(url) { request in
         XCTAssertEqual(request.url, url)
         XCTAssertEqual(request.allowsCellularAccess, allowMetered)
         XCTAssertEqual(request.allowsExpensiveNetworkAccess, allowMetered)
         XCTAssertEqual(request.allowsConstrainedNetworkAccess, allowMetered)
         XCTAssertEqual(request.value(forHTTPHeaderField: "User-Agent"), "policy-test/1.0")
         received.fulfill()
      }
      defer { RecordingDownloadProtocol.remove(url) }
      await manager.setUserAgent("policy-test/1.0")
      _ = await manager.create(path: path, url: url, options: CreateOptions(allowMetered: allowMetered))
      if resume {
         let stored = await store.findByPath(path)
         var record = try XCTUnwrap(stored)
         record.setStatus(.paused)
         XCTAssertNil(record.resumeDataPath)
         await store.update(record)
         _ = try await manager.resume(path: path)
      } else {
         _ = try await manager.start(path: path)
      }
      await fulfillment(of: [received], timeout: 5)
      _ = try await manager.cancel(path: path)
   }
}

/// Intercepts every fixture request and holds it until the test cancels the task.
private final class RecordingDownloadProtocol: URLProtocol {
   private static let lock = NSLock()
   private static var handlers: [URL: (URLRequest) -> Void] = [:]

   static func register(_ url: URL, handler: @escaping (URLRequest) -> Void) {
      lock.lock()
      defer { lock.unlock() }
      handlers[url] = handler
   }

   static func remove(_ url: URL) {
      lock.lock()
      defer { lock.unlock() }
      handlers.removeValue(forKey: url)
   }

   override class func canInit(with request: URLRequest) -> Bool { true }

   override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

   override func startLoading() {
      Self.lock.lock()
      let handler = request.url.flatMap { Self.handlers[$0] }
      Self.lock.unlock()
      guard let handler else {
         XCTFail("Unexpected request: \(request)")
         client?.urlProtocol(self, didFailWithError: URLError(.unsupportedURL))
         return
      }
      handler(request)
   }

   override func stopLoading() {}
}
