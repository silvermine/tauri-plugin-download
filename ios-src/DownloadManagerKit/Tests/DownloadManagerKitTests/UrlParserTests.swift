import XCTest
@testable import DownloadManagerKit

final class UrlParserTests: XCTestCase {

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

    // MARK: - parseUrl tests

    func testValidUrls() throws {
        XCTAssertNoThrow(try parseUrl("https://example.com/file.mp4"))
        XCTAssertNoThrow(try parseUrl("http://example.com/file.mp4"))
        XCTAssertNoThrow(try parseUrl("https://example.com:8080/file.mp4"))
        XCTAssertNoThrow(try parseUrl("https://example.com/file.mp4?token=abc"))
    }

    func testEmptyUrl() {
        XCTAssertThrowsError(try parseUrl(""))
    }

    func testInvalidScheme() {
        XCTAssertThrowsError(try parseUrl("ftp://example.com/file.mp4"))
        XCTAssertThrowsError(try parseUrl("file:///path/to/file.mp4"))
    }

    func testMissingHost() {
        XCTAssertThrowsError(try parseUrl("https://:8080/file.mp4"))
    }

    func testInvalidUrlFormat() {
        XCTAssertThrowsError(try parseUrl("not a valid url"))
    }
}
