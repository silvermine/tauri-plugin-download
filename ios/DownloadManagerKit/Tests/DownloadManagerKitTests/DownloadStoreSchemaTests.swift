import XCTest
@testable import DownloadManagerKit

final class DownloadStoreSchemaTests: XCTestCase {
   private var savePath: URL!

   override func setUp() {
      super.setUp()
      savePath = FileManager.default.temporaryDirectory
         .appendingPathComponent(UUID().uuidString)
         .appendingPathComponent("nested/downloads.json")
   }

   override func tearDown() {
      try? FileManager.default.removeItem(at: savePath.deletingLastPathComponent().deletingLastPathComponent())
      savePath = nil
      super.tearDown()
   }

   private func write(_ text: String) throws {
      try FileManager.default.createDirectory(
         at: savePath.deletingLastPathComponent(), withIntermediateDirectories: true
      )
      try Data(text.utf8).write(to: savePath)
   }

   private func assertDecodeError(
      _ text: String, _ message: String, file: StaticString = #filePath, line: UInt = #line
   ) {
      XCTAssertThrowsError(try DownloadStore.decodeRecords(from: Data(text.utf8)), file: file, line: line) {
         XCTAssertEqual($0.localizedDescription, message, file: file, line: line)
      }
   }

   func testRejectsLegacyArraysAndMalformedEnvelopes() {
      for text in [
         "[]", "[1, []]", "[{}]", "null", "true", "1", "{}", "not json",
         #"{"version":1}"#,
         #"{"downloads":[]}"#,
         #"{"version":1,"downloads":null}"#,
         #"{"version":1,"downloads":{}}"#,
         #"{"version":1,"downloads":"private input"}"#
      ] {
         assertDecodeError(text, "Malformed store envelope")
      }
   }

   func testRejectsInvalidVersionTypesAndValues() {
      for version in [
         #""1""#, "true", "false", "null", "-1", "1.5", "1.0", "1e0", "1E0", "10e-1", "0.1e1",
         "+1", "01", "[]", "{}"
      ] {
         assertDecodeError(
            "{\"version\":\(version),\"downloads\":[]}", "Malformed store envelope"
         )
      }
   }

   func testVersionTokenValidationIgnoresNestedFieldsAndStringContents() throws {
      let text = #"{"extra":{"version":1.0,"items":[{"version":1e0}]},"text":"\"version\":1.0, } ] \\ \"","downloads":[],"version" : 1 }"#
      XCTAssertTrue(try DownloadStore.decodeRecords(from: Data(text.utf8)).isEmpty)

      for version in ["1.0", "1e0"] {
         let invalid = #"{"extra":{"version":1},"text":"\"version\":1","downloads":[],"version":\#(version)}"#
         assertDecodeError(invalid, "Malformed store envelope")
      }
   }

   func testVersionTokenValidationDecodesEscapedKeys() throws {
      let text = #"{"\u0076ersion":1,"downloads":[]}"#
      XCTAssertTrue(try DownloadStore.decodeRecords(from: Data(text.utf8)).isEmpty)
      for version in ["1.0", "1e0"] {
         assertDecodeError(
            #"{"\u0076ersion":\#(version),"downloads":[]}"#, "Malformed store envelope"
         )
      }
   }

   func testVersionTokenValidationStillRejectsMalformedJSON() {
      for text in [
         #"{"version":1,"downloads":[],"text":"unterminated"#,
         #"{"version":1,"downloads":[],"text":"escaped final quote\""#,
         #"{"version":1,"downloads":[],"extra":[}"#
      ] {
         assertDecodeError(text, "Malformed store envelope")
      }
   }

   func testVersionTokenValidationAcrossEncodings() throws {
      for encoding in [
         String.Encoding.utf8, .utf16LittleEndian, .utf16BigEndian, .utf32LittleEndian, .utf32BigEndian
      ] {
         let valid = try XCTUnwrap(#"{"version":1,"downloads":[]}"#.data(using: encoding))
         XCTAssertTrue(try DownloadStore.decodeRecords(from: valid).isEmpty)
         for version in ["1.0", "1e0", "1E0", "10e-1", "0.1e1", "true", "false"] {
            let bytes = try XCTUnwrap(
               #"{"version":\#(version),"downloads":[]}"#.data(using: encoding)
            )
            XCTAssertThrowsError(try DownloadStore.decodeRecords(from: bytes)) {
               XCTAssertEqual($0.localizedDescription, "Malformed store envelope")
            }
         }
      }
   }

   func testChecksUnsupportedVersionBeforeDecodingRecords() {
      for version in [UInt32(0), UInt32(2), UInt32.max] {
         assertDecodeError(
            "{\"version\":\(version),\"downloads\":[{\"future\":\"record\"}]}",
            "Unsupported store version: \(version) (expected 1)"
         )
      }
   }

   func testMissingFileStaysMissingOnLoad() async {
      let store = DownloadStore(savePath: savePath)
      let records = await store.list()
      XCTAssertTrue(records.isEmpty)
      XCTAssertFalse(FileManager.default.fileExists(atPath: savePath.path))
   }

   func testUnknownFieldsAreIgnoredWithoutRewritingOnLoad() throws {
      let text = #"{"version":1,"downloads":[{"url":"https://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"paused","extraRecordField":true}],"extraEnvelopeField":{"ignored":true}}"#
      try write(text)
      XCTAssertEqual(try DownloadStore.decodeRecords(from: Data(text.utf8)).first?.receivedBytes, 7)
      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 7)
      XCTAssertEqual(try Data(contentsOf: savePath), Data(text.utf8))
   }

   func testInvalidRecordRejectsWholeStoreWithoutExposingInput() throws {
      let text = #"{"version":1,"downloads":[{"url":"https://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"paused"},{"url":"https://example.com/b.mp4","path":"/tmp/b.mp4","options":{"allowMetered":true},"receivedBytes":8,"status":"private input"}]}"#
      assertDecodeError(text, "Invalid store records")
      try write(text)
      XCTAssertTrue(DownloadStore.load(from: savePath).isEmpty)
      XCTAssertEqual(try Data(contentsOf: savePath), Data(text.utf8))
   }

   func testRejectedDocumentsStayUntouchedUntilALaterSave() async throws {
      let original = #"{"version":2,"downloads":[]}"#
      try write(original)
      let store = DownloadStore(savePath: savePath)
      let records = await store.list()
      XCTAssertTrue(records.isEmpty)
      XCTAssertEqual(try Data(contentsOf: savePath), Data(original.utf8))

      let record = DownloadRecord(
         url: URL(string: "https://example.com/a.mp4")!, path: "/tmp/a.mp4"
      )
      // Preserving rejected files on a later save remains separate work under #64.
      await store.append(record)
      XCTAssertEqual(DownloadStore.load(from: savePath).first?.path, record.path)
   }

   func testWritesV1AndRoundTripsAllFieldsIncludingResumeData() async throws {
      let record = DownloadRecord(
         url: URL(string: "https://example.com/a.mp4")!,
         path: "/tmp/a.mp4",
         options: CreateOptions(allowMetered: false),
         receivedBytes: 123,
         totalBytes: 456,
         status: .paused,
         resumeDataPath: URL(fileURLWithPath: "/tmp/a.resume")
      )
      let store = DownloadStore(savePath: savePath)
      await store.append(record)

      let bytes = try Data(contentsOf: savePath)
      let document = try XCTUnwrap(try JSONSerialization.jsonObject(with: bytes) as? [String: Any])
      XCTAssertEqual(document["version"] as? Int, 1)
      XCTAssertEqual((document["downloads"] as? [Any])?.count, 1)
      let reloaded = try XCTUnwrap(try DownloadStore.decodeRecords(from: bytes).first)
      XCTAssertEqual(reloaded.url, record.url)
      XCTAssertEqual(reloaded.path, record.path)
      XCTAssertEqual(reloaded.options.allowMetered, false)
      XCTAssertEqual(reloaded.receivedBytes, 123)
      XCTAssertEqual(reloaded.totalBytes, 456)
      XCTAssertEqual(reloaded.status, .paused)
      XCTAssertEqual(reloaded.resumeDataPath, record.resumeDataPath)

      await store.remove(record)
      let emptyBytes = try Data(contentsOf: savePath)
      let empty = try XCTUnwrap(try JSONSerialization.jsonObject(with: emptyBytes) as? [String: Any])
      XCTAssertEqual(empty["version"] as? Int, 1)
      XCTAssertEqual((empty["downloads"] as? [Any])?.count, 0)
      XCTAssertTrue(try DownloadStore.decodeRecords(from: emptyBytes).isEmpty)
   }
}
