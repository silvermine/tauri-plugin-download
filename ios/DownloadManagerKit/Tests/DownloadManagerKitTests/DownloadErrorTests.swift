import XCTest
@testable import DownloadManagerKit

final class DownloadErrorTests: XCTestCase {

   func testCommandCodesUseTypesRatherThanMessages() {
      XCTAssertEqual(commandErrorCode(DownloadError.notFound("/tmp/file")), "download not found")
      XCTAssertEqual(commandErrorCode(DownloadError.invalidPath("timeout")), "invalid input")
      XCTAssertEqual(commandErrorCode(DownloadError.invalidURL("HTTP 404")), "invalid input")
      XCTAssertEqual(commandErrorCode(DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "bad input"))), "invalid input")
      XCTAssertEqual(commandErrorCode(NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError)), "file")
      XCTAssertEqual(commandErrorCode(NSError(domain: "custom", code: 1, userInfo: [NSLocalizedDescriptionKey: "timeout"])), "unknown")
   }

   func testNotFoundMessageMatchesOtherPlatforms() {
      let error: Error = DownloadError.notFound("/tmp/file.mp4")

      XCTAssertEqual(error.localizedDescription, "Not Found: /tmp/file.mp4")
      XCTAssertEqual("\(error)", "Not Found: /tmp/file.mp4")
   }

   func testValidationErrorsCarryTheirMessage() {
      XCTAssertEqual(DownloadError.invalidPath("path must be absolute").localizedDescription, "Path Error: path must be absolute")
      XCTAssertEqual(DownloadError.invalidURL("URL must have a host").localizedDescription, "URL Error: URL must have a host")

      // Validation throws synchronously from the plugin's `@objc` handlers, where Tauri
      // rejects with `"\(error)"` rather than `localizedDescription`.
      XCTAssertEqual("\(DownloadError.invalidPath("path must be absolute") as Error)", "Path Error: path must be absolute")
      XCTAssertEqual("\(DownloadError.invalidURL("URL must have a host") as Error)", "URL Error: URL must have a host")
   }
}
