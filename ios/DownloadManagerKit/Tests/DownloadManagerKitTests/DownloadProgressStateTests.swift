import Foundation
import XCTest
@testable import DownloadManagerKit

final class DownloadProgressStateTests: XCTestCase {

   func testTransferredBytesAddsResumeOffset() {
      let transferredBytes = DownloadProgressState.transferredBytes(
         totalBytesWritten: 512 * 1024,
         resumeOffset: 2 * 1024 * 1024
      )

      XCTAssertEqual(transferredBytes, 2_621_440)
   }

   func testTotalBytesPreservesKnownValueWhenCallbackReportsUnknownSize() {
      let totalBytes = DownloadProgressState.totalBytes(
         expectedTotalBytes: NSURLSessionTransferSizeUnknown,
         currentTotalBytes: 2_048
      )

      XCTAssertEqual(totalBytes, 2_048)
   }

   func testShouldNotThrottleUnknownSizeWhenEffectiveTransferredBytesAdvancePastThreshold() {
      let item = DownloadItem(
         url: URL(string: "https://example.com/file.bin")!,
         path: URL(fileURLWithPath: "/tmp/file.bin"),
         transferredBytes: 5 * 1024 * 1024,
         status: .inProgress
      )
      let transferredBytes = DownloadProgressState.transferredBytes(
         totalBytesWritten: 2 * 1024 * 1024,
         resumeOffset: 5 * 1024 * 1024
      )

      XCTAssertFalse(
         DownloadProgressState.shouldThrottle(
            item: item,
            transferredBytes: transferredBytes,
            totalBytes: nil
         )
      )
   }

   func testShouldNotThrottleKnownSizeWhenResumeOffsetProducesLargeProgressJump() {
      let item = DownloadItem(
         url: URL(string: "https://example.com/file.bin")!,
         path: URL(fileURLWithPath: "/tmp/file.bin"),
         progress: 50.0,
         transferredBytes: 1_000,
         totalBytes: 2_000,
         status: .inProgress
      )
      let transferredBytes = DownloadProgressState.transferredBytes(
         totalBytesWritten: 500,
         resumeOffset: 1_000
      )

      XCTAssertFalse(
         DownloadProgressState.shouldThrottle(
            item: item,
            transferredBytes: transferredBytes,
            totalBytes: 2_000
         )
      )
   }

   func testResumeOffsetStoreSeparatesEntriesByPath() async {
      let store = DownloadTaskResumeOffsetStore()

      await store.setOffset(1_024, for: "/tmp/file-a.bin")
      await store.setOffset(2_048, for: "/tmp/file-b.bin")

      let firstOffset = await store.offset(for: "/tmp/file-a.bin")
      let secondOffset = await store.offset(for: "/tmp/file-b.bin")

      XCTAssertEqual(firstOffset, 1_024)
      XCTAssertEqual(secondOffset, 2_048)
   }
}
