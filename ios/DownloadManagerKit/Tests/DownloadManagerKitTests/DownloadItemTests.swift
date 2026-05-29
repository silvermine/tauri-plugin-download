import Foundation
import XCTest
@testable import DownloadManagerKit

final class DownloadItemTests: XCTestCase {

   func testDecodeOlderPersistedItemDefaultsByteTrackingFields() throws {
      let data = try XCTUnwrap(
         """
         [
            {
               "url": "https://example.com/file.bin",
               "path": "file:///tmp/file.bin",
               "progress": 42.5,
               "status": "paused"
            }
         ]
         """.data(using: .utf8)
      )

      let items = try JSONDecoder().decode([DownloadItem].self, from: data)
      let item = try XCTUnwrap(items.first)

      XCTAssertEqual(item.transferredBytes, 0)
      XCTAssertNil(item.totalBytes)
      XCTAssertEqual(item.progress, 42.5)
      XCTAssertEqual(item.status, .paused)
   }

   func testSetTransferWithUnknownSizeTracksBytesAndResetsProgress() {
      var item = DownloadItem(
         url: URL(string: "https://example.com/file.bin")!,
         path: URL(fileURLWithPath: "/tmp/file.bin")
      )

      item.setTransfer(1_024, nil)

      XCTAssertEqual(item.transferredBytes, 1_024)
      XCTAssertNil(item.totalBytes)
      XCTAssertEqual(item.progress, 0.0)
   }

   func testCompletedStatusInfersTotalBytesFromTransferredBytesWhenUnknown() {
      var item = DownloadItem(
         url: URL(string: "https://example.com/file.bin")!,
         path: URL(fileURLWithPath: "/tmp/file.bin")
      )

      item.setTransfer(2_048, nil)
      item.setStatus(.completed)

      XCTAssertEqual(item.transferredBytes, 2_048)
      XCTAssertEqual(item.totalBytes, 2_048)
      XCTAssertEqual(item.progress, 100.0)
      XCTAssertEqual(item.status, .completed)
   }
}
