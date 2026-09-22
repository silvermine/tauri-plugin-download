import XCTest
@testable import DownloadManagerKit

final class DownloadFailureTests: XCTestCase {
   func testHTTPClassificationMatchesSharedFixture() throws {
      var root = URL(fileURLWithPath: #filePath)
      for _ in 0..<5 { root.deleteLastPathComponent() }
      let data = try Data(contentsOf: root.appendingPathComponent("fixtures/http-errors.json"))
      let cases = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
      for testCase in cases {
         let status = try XCTUnwrap(testCase["status"] as? Int)
         let failure = DownloadFailure.http(status)
         XCTAssertEqual(failure.code, "http")
         XCTAssertEqual(failure.httpStatus, status)
         XCTAssertEqual(failure.retryability, testCase["retryability"] as? String)
      }
   }

   func testNativeErrorClassificationUsesDomainsAndCodes() {
      for (code, category, retryability) in [
         (NSURLErrorTimedOut, "timeout", "transient"),
         (NSURLErrorNetworkConnectionLost, "connection", "transient"),
         (NSURLErrorDNSLookupFailed, "connection", "unknown"),
         (NSURLErrorServerCertificateUntrusted, "tls", "permanent"),
         (NSURLErrorSecureConnectionFailed, "tls", "transient")
      ] {
         let error = NSError(domain: NSURLErrorDomain, code: code, userInfo: [NSLocalizedDescriptionKey: "HTTP 404"])
         let failure = DownloadFailure.classify(error)
         XCTAssertEqual(failure.code, category)
         XCTAssertEqual(failure.retryability, retryability)
      }
      for code in [NSFileWriteNoPermissionError, NSFileWriteOutOfSpaceError] {
         let error = NSError(domain: NSCocoaErrorDomain, code: code)
         XCTAssertEqual(DownloadFailure.classify(error).code, "file")
         XCTAssertEqual(DownloadFailure.classify(error).retryability, "permanent")
      }
      XCTAssertEqual(DownloadFailure.classify(NSError(domain: "future", code: 1)).retryability, "unknown")
   }

   func testFailureSurvivesReloadAndOnlyOneResumeClaimsTheRecord() async throws {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      defer { try? FileManager.default.removeItem(at: directory) }
      let path = directory.appendingPathComponent("downloads.json")
      let store = DownloadStore(savePath: path)
      let record = DownloadRecord(url: URL(string: "https://example.com/a")!, path: "/tmp/a", receivedBytes: 123, status: .inProgress)
      await store.append(record)
      let resumeData = directory.appendingPathComponent("resume.data")
      let error = DownloadFailure.http(503)
      _ = await store.fail(path: record.path, error: error, resumeDataPath: resumeData)
      let reloaded = DownloadStore(savePath: path)
      let restored = await reloaded.findByPath(record.path)
      XCTAssertEqual(restored?.status, .failed)
      XCTAssertEqual(restored?.error, error)
      XCTAssertEqual(restored?.resumeDataPath, resumeData)
      XCTAssertEqual(restored?.receivedBytes, 123)
      async let first = reloaded.beginTransfer(path: record.path, allowed: [.paused, .failed])
      async let second = reloaded.beginTransfer(path: record.path, allowed: [.paused, .failed])
      let attempts = try await [first, second]
      XCTAssertEqual(attempts.compactMap { $0 }.filter { $0.1 }.count, 1)
      let resumed = await reloaded.findByPath(record.path)
      XCTAssertNil(resumed?.error)
      XCTAssertEqual(resumed?.status, .inProgress)
   }

   func testRejectedResumePreservesFailure() async throws {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      defer { try? FileManager.default.removeItem(at: directory) }
      let path = directory.appendingPathComponent("downloads.json")
      let store = DownloadStore(savePath: path)
      let record = DownloadRecord(url: URL(string: "https://example.com/a")!, path: "/tmp/a", status: .inProgress)
      await store.append(record)
      _ = await store.fail(path: record.path, error: .http(503))
      try FileManager.default.removeItem(at: path)
      try FileManager.default.createDirectory(at: path, withIntermediateDirectories: true)
      do {
         _ = try await store.beginTransfer(path: record.path, allowed: [.failed])
         XCTFail("Resume must reject when its state cannot be persisted")
      } catch let error as DownloadFailure {
         XCTAssertEqual(error.code, "store")
      }
      let restored = await store.findByPath(record.path)
      XCTAssertEqual(restored?.status, .failed)
      XCTAssertEqual(restored?.error, .http(503))
   }

   func testLateFailureCannotOverwritePauseOrRestoreCanceledRecord() async {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      defer { try? FileManager.default.removeItem(at: directory) }
      let store = DownloadStore(savePath: directory.appendingPathComponent("downloads.json"))
      let record = DownloadRecord(url: URL(string: "https://example.com/a")!, path: "/tmp/a", status: .paused)
      await store.append(record)
      let paused = await store.fail(path: record.path, error: .http(404))
      XCTAssertNil(paused)
      await store.remove(record)
      let canceled = await store.fail(path: record.path, error: .http(404))
      XCTAssertNil(canceled)
   }
}
