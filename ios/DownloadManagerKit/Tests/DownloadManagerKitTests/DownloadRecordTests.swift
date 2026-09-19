import XCTest
@testable import DownloadManagerKit

final class DownloadRecordTests: XCTestCase {

   private func sampleRecord() -> DownloadRecord {
      return DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/file.mp4",
         receivedBytes: 0,
         totalBytes: nil,
         status: .idle
      )
   }

   // MARK: - Mutation

   func testSetBytes() {
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)

      XCTAssertEqual(record.receivedBytes, 500)
      XCTAssertEqual(record.totalBytes, 1000)
      XCTAssertEqual(record.status, .idle)
      XCTAssertEqual(record.url, sampleRecord().url)
      XCTAssertEqual(record.path, sampleRecord().path)
   }

   func testSetBytesWithNilTotalPreservesAKnownTotal() {
      // A resume answered without a content length reports no total — "none given",
      // not "there is none" — so it must not erase an established one.
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)
      record.setBytes(received: 600)

      XCTAssertEqual(record.receivedBytes, 600)
      XCTAssertEqual(record.totalBytes, 1000)
   }

   func testSetStatusPreservesBytes() {
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)

      var paused = record
      paused.setStatus(.paused)
      XCTAssertEqual(paused.receivedBytes, 500)
      XCTAssertEqual(paused.totalBytes, 1000)
      XCTAssertEqual(paused.status, .paused)

      // Status transitions preserve the factual byte counts, including completion.
      var completed = record
      completed.setStatus(.completed)
      XCTAssertEqual(completed.receivedBytes, 500)
      XCTAssertEqual(completed.totalBytes, 1000)
      XCTAssertEqual(completed.status, .completed)
      XCTAssertEqual(completed.toItem().progress, 100.0)
   }

   func testSetStatusCompletedWithUnknownSize() {
      // Completed with unknown size preserves receivedBytes.
      var record = sampleRecord()
      record.setBytes(received: 5000, total: nil)
      record.setStatus(.completed)

      XCTAssertEqual(record.receivedBytes, 5000)
      XCTAssertNil(record.totalBytes)
   }

   // MARK: - Progress derivation

   func testToItemWithKnownSize() {
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)

      let item = record.toItem()
      XCTAssertEqual(item.progress, 50.0)
      XCTAssertEqual(item.receivedBytes, 500)
      XCTAssertEqual(item.totalBytes, 1000)
   }

   func testToItemClampsProgressTo100Percent() {
      var record = sampleRecord()
      record.setBytes(received: 1500, total: 1000)

      XCTAssertEqual(record.toItem().progress, 100.0)
   }

   func testToItemWithUnknownSize() {
      let record = sampleRecord()
      XCTAssertEqual(record.toItem().progress, 0.0)

      // Completed with unknown size still reports 100%.
      var completed = record
      completed.setStatus(.completed)
      XCTAssertEqual(completed.toItem().progress, 100.0)
   }

   func testToItemWithZeroTotal() {
      // A zero content length must not divide by zero.
      var record = sampleRecord()
      record.setBytes(received: 0, total: 0)

      XCTAssertEqual(record.toItem().progress, 0.0)
   }

   // MARK: - Encoding

   func testItemEncodesTotalBytesAsExplicitNull() throws {
      // The key is always present, so the payload matches the frontend contract
      // without relying on the TypeScript layer to coalesce a missing key.
      let record = sampleRecord()
      let json = try JSONSerialization.jsonObject(
         with: try JSONEncoder().encode(record.toItem())
      ) as? [String: Any]

      XCTAssertTrue(try XCTUnwrap(json).keys.contains("totalBytes"))
      XCTAssertTrue(try XCTUnwrap(json)["totalBytes"] is NSNull)
   }

   func testItemEncodesByteFields() throws {
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)

      let json = try XCTUnwrap(
         try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(record.toItem())
         ) as? [String: Any]
      )

      XCTAssertEqual(json["receivedBytes"] as? UInt64, 500)
      XCTAssertEqual(json["totalBytes"] as? UInt64, 1000)
      XCTAssertEqual(json["progress"] as? Double, 50.0)
      XCTAssertEqual(json["status"] as? String, "idle")
   }

   func testItemEncodesThePathAsGiven() throws {
      // JS keys listeners by the string it passed, so the path must not come back
      // as a percent-encoded file URL.
      let record = DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/Application Support//x.zip",
         status: .idle
      )

      let json = try XCTUnwrap(
         try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(record.toItem())
         ) as? [String: Any]
      )

      XCTAssertEqual(json["path"] as? String, "/tmp/Application Support//x.zip")
   }

   func testItemEncodesExactlyTheContractKeys() throws {
      // DownloadItem hand-writes encode(to:) so a nil total becomes an explicit
      // null. That means a property added without a matching encode line still
      // compiles and silently vanishes from the payload — on iOS only, since Rust
      // and Kotlin derive their encoders. Pin the whole key set, not one key.
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)
      record.setResumeDataPath(URL(fileURLWithPath: "/tmp/abc.resumedata"))

      let json = try XCTUnwrap(
         try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(record.toItem())
         ) as? [String: Any]
      )

      XCTAssertEqual(
         Set(json.keys),
         ["url", "path", "options", "receivedBytes", "totalBytes", "progress", "status"]
      )
   }

   func testItemDoesNotEncodeResumeDataPath() throws {
      // resumeDataPath is internal to URLSession and must not reach the frontend.
      var record = sampleRecord()
      record.setResumeDataPath(URL(fileURLWithPath: "/tmp/abc.resumedata"))

      let json = try XCTUnwrap(
         try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(record.toItem())
         ) as? [String: Any]
      )

      XCTAssertFalse(json.keys.contains("resumeDataPath"))
   }

   func testRecordDecodesPersistedWireFormat() throws {
      // A literal payload, not a round trip, which would pass even if the
      // field names drifted on both sides together.
      let json = """
      {"url":"http://example.com/f.mp4","path":"/tmp/f.mp4",\
      "options":{"allowMetered":true},\
      "receivedBytes":500,"totalBytes":1000,"status":"paused"}
      """

      let record = try JSONDecoder().decode(DownloadRecord.self, from: Data(json.utf8))

      XCTAssertEqual(record.receivedBytes, 500)
      XCTAssertEqual(record.totalBytes, 1000)
      XCTAssertEqual(record.status, .paused)
      // progress is derived via toItem(), not stored.
      XCTAssertEqual(record.toItem().progress, 50.0)
   }


   func testRecordRoundTripsThroughJSON() throws {
      var record = sampleRecord()
      record.setBytes(received: 500, total: 1000)
      record.setStatus(.paused)

      let decoded = try JSONDecoder().decode(
         DownloadRecord.self,
         from: try JSONEncoder().encode(record)
      )

      XCTAssertEqual(decoded.receivedBytes, 500)
      XCTAssertEqual(decoded.totalBytes, 1000)
      XCTAssertEqual(decoded.status, .paused)
      // progress is derived via toItem(), not stored.
      XCTAssertEqual(decoded.toItem().progress, 50.0)
   }


   // MARK: - Network policy

   func testUnstatedPolicyAllowsMeteredConnections() {
      // The API default, applied when a caller states no policy. It is not a
      // decoding fallback — a persisted record must spell the value out.
      XCTAssertTrue(sampleRecord().options.allowMetered)
      XCTAssertTrue(CreateOptions().allowMetered)
   }

   func testRecordWithoutOptionsFailsToDecode() {
      // The policy is decided once, at creation. A record that has lost it cannot
      // be read back as anything trustworthy, so decoding refuses rather than
      // assuming the permissive default.
      let json = """
      {"url":"http://example.com/f.mp4","path":"/tmp/f.mp4",\
      "receivedBytes":0,"totalBytes":null,"status":"idle"}
      """

      XCTAssertThrowsError(
         try JSONDecoder().decode(DownloadRecord.self, from: Data(json.utf8))
      )
   }

   func testRecordWithEmptyOptionsFailsToDecode() {
      let json = """
      {"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","options":{},\
      "receivedBytes":0,"totalBytes":null,"status":"idle"}
      """

      XCTAssertThrowsError(
         try JSONDecoder().decode(DownloadRecord.self, from: Data(json.utf8))
      )
   }

   func testRecordDecodesARestrictedPolicy() throws {
      let json = """
      {"url":"http://example.com/f.mp4","path":"/tmp/f.mp4",\
      "options":{"allowMetered":false},\
      "receivedBytes":0,"totalBytes":null,"status":"idle"}
      """

      let record = try JSONDecoder().decode(DownloadRecord.self, from: Data(json.utf8))

      XCTAssertFalse(record.options.allowMetered)
   }

   func testRecordRoundTripsARestrictedPolicy() throws {
      let record = DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/file.mp4",
         options: CreateOptions(allowMetered: false)
      )

      let decoded = try JSONDecoder().decode(
         DownloadRecord.self,
         from: try JSONEncoder().encode(record)
      )

      XCTAssertFalse(decoded.options.allowMetered)
   }

   func testItemEncodesTheResolvedPolicy() throws {
      let record = DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/file.mp4",
         options: CreateOptions(allowMetered: false)
      )

      let json = try XCTUnwrap(
         try JSONSerialization.jsonObject(
            with: try JSONEncoder().encode(record.toItem())
         ) as? [String: Any]
      )
      let options = try XCTUnwrap(json["options"] as? [String: Any])

      XCTAssertEqual(options["allowMetered"] as? Bool, false)
   }

   // MARK: - Action response

   func testActionResponse() {
      let item = sampleRecord().toItem()

      // The single-argument initializer reports the status as expected.
      let response = DownloadActionResponse(download: item)
      XCTAssertTrue(response.isExpectedStatus)
      XCTAssertEqual(response.expectedStatus, .idle)

      // A matching expected status is still expected.
      let matching = DownloadActionResponse(download: item, expectedStatus: .idle)
      XCTAssertTrue(matching.isExpectedStatus)

      // A mismatched expected status is not.
      let mismatched = DownloadActionResponse(download: item, expectedStatus: .inProgress)
      XCTAssertFalse(mismatched.isExpectedStatus)
      XCTAssertEqual(mismatched.expectedStatus, .inProgress)
   }
}
