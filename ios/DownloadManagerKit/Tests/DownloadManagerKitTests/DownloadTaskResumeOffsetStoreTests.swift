import Foundation
import XCTest
@testable import DownloadManagerKit

final class DownloadTaskResumeOffsetStoreTests: XCTestCase {

   func testTransferredBytesAddsOffsetWhenProgressStartsFromResumedChunk() async {
      let store = DownloadTaskResumeOffsetStore()
      let path = "/tmp/file.bin"

      await store.setOffset(2 * 1024 * 1024, for: path)

      let transferredBytes = await store.transferredBytes(
         bytesWritten: 512 * 1024,
         totalBytesWritten: 512 * 1024,
         for: path
      )

      XCTAssertEqual(transferredBytes, 2_621_440)
   }

   func testTransferredBytesUsesCumulativeTotalWhenProgressAlreadyIncludesOffset() async {
      let store = DownloadTaskResumeOffsetStore()
      let path = "/tmp/file.bin"

      await store.setOffset(2 * 1024 * 1024, for: path)

      let transferredBytes = await store.transferredBytes(
         bytesWritten: 512 * 1024,
         totalBytesWritten: 2_621_440,
         for: path
      )

      XCTAssertEqual(transferredBytes, 2_621_440)
   }

   func testTransferredBytesKeepsDetectedAccountingModeForLaterCallbacks() async {
      let store = DownloadTaskResumeOffsetStore()
      let path = "/tmp/file.bin"

      await store.setOffset(2 * 1024 * 1024, for: path)
      _ = await store.transferredBytes(
         bytesWritten: 512 * 1024,
         totalBytesWritten: 512 * 1024,
         for: path
      )

      let transferredBytes = await store.transferredBytes(
         bytesWritten: 256 * 1024,
         totalBytesWritten: 768 * 1024,
         for: path
      )

      XCTAssertEqual(transferredBytes, 2_883_584)
   }

   func testTransferredBytesFallsBackToTotalBytesWrittenWithoutResumeState() async {
      let store = DownloadTaskResumeOffsetStore()

      let transferredBytes = await store.transferredBytes(
         bytesWritten: 512 * 1024,
         totalBytesWritten: 768 * 1024,
         for: "/tmp/file.bin"
      )

      XCTAssertEqual(transferredBytes, 768 * 1024)
   }
}
