import XCTest
@testable import DownloadManagerKit

final class URLParserTests: XCTestCase {

   // MARK: - parsePath tests

   func testValidPath() throws {
      XCTAssertNoThrow(try parsePath("/downloads/file.mp4"))
      XCTAssertNoThrow(try parsePath("/file.txt"))
   }

   func testEmptyPath() {
      XCTAssertThrowsError(try parsePath(""))
   }

   func testRelativePath() {
      XCTAssertThrowsError(try parsePath("relative/path.txt"))
      XCTAssertThrowsError(try parsePath("file.txt"))
   }

   func testPathWithoutFilename() {
      XCTAssertThrowsError(try parsePath("/"))
   }

   func testFileURLIsRejected() {
      XCTAssertThrowsError(try parsePath("file:///file.txt"))
   }

   func testPathIsReturnedAsGiven() throws {
      // A URL would percent-encode the space, handing back a different string from
      // the one JS keys its listeners by.
      XCTAssertEqual(try parsePath("/tmp/a b/x.zip"), "/tmp/a b/x.zip")
      XCTAssertEqual(try parsePath("/tmp//../x.zip"), "/tmp//../x.zip")
   }

   // MARK: - parseURL tests

   func testValidUrls() throws {
      XCTAssertNoThrow(try parseURL("https://example.com/file.mp4"))
      XCTAssertNoThrow(try parseURL("http://example.com/file.mp4"))
      XCTAssertNoThrow(try parseURL("https://example.com:8080/file.mp4"))
      XCTAssertNoThrow(try parseURL("https://example.com/file.mp4?token=abc"))
   }

   func testEmptyUrl() {
      XCTAssertThrowsError(try parseURL(""))
   }

   func testInvalidScheme() {
      XCTAssertThrowsError(try parseURL("ftp://example.com/file.mp4"))
      XCTAssertThrowsError(try parseURL("file:///path/to/file.mp4"))
   }

   func testMissingHost() {
      XCTAssertThrowsError(try parseURL("https://:8080/file.mp4"))
   }

   func testInvalidUrlFormat() {
      XCTAssertThrowsError(try parseURL("not a valid url"))
   }

   // MARK: - Messages

   func testPathMessagesMatchOtherPlatforms() {
      // Rust and Android send these exact strings for the same inputs.
      XCTAssertEqual(message { try parsePath("") }, "Path Error: path cannot be empty")
      XCTAssertEqual(message { try parsePath("file.txt") }, "Path Error: path must be absolute")
      XCTAssertEqual(message { try parsePath("/") }, "Path Error: path must have a filename")
   }

   func testURLMessagesMatchOtherPlatforms() {
      XCTAssertEqual(message { try parseURL("") }, "URL Error: URL cannot be empty")
      XCTAssertEqual(message { try parseURL("not a valid url") }, "URL Error: Invalid URL: not a valid url")
      XCTAssertEqual(message { try parseURL("example.com/file.mp4") }, "URL Error: Invalid URL: example.com/file.mp4")
      XCTAssertEqual(
         message { try parseURL("ftp://example.com/file.mp4") },
         "URL Error: Invalid URL scheme 'ftp': must be http or https"
      )
   }

   /// The message the plugin rejects with: `localizedDescription` of the thrown error.
   private func message(_ body: () throws -> Any) -> String? {
      do {
         _ = try body()
         return nil
      } catch {
         return error.localizedDescription
      }
   }
}
