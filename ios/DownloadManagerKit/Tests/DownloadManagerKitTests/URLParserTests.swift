import XCTest
@testable import DownloadManagerKit

final class URLParserTests: XCTestCase {

   // MARK: - parsePath tests

   func testValidPath() throws {
      XCTAssertNoThrow(try parsePath("/downloads/file.mp4"))
      XCTAssertNoThrow(try parsePath("/file.txt"))
      XCTAssertNoThrow(try parsePath("file:///downloads/file.mp4"))
      XCTAssertNoThrow(try parsePath("file:///file.txt"))
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
}
