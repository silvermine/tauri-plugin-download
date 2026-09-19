import XCTest
@testable import DownloadManagerKit

final class DownloadManagerTests: XCTestCase {

   private func inProgressRecord(resumeDataPath: URL? = nil) -> DownloadRecord {
      return DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/file.mp4",
         receivedBytes: 500,
         totalBytes: 1000,
         status: .inProgress,
         resumeDataPath: resumeDataPath
      )
   }

   private func resumeDataURL() -> URL {
      return URL(fileURLWithPath: "/tmp/abc.resumedata")
   }

   // MARK: - Reconciliation

   func testLiveTaskIsLeftAlone() {
      // A background-session task outlives the process, so an InProgress record
      // with a task still behind it is genuinely running.
      let record = inProgressRecord()

      XCTAssertNil(DownloadManager.reconciledRecord(record, hasLiveTask: true))
   }

   func testStrandedRecordWithResumeDataBecomesPaused() {
      let record = inProgressRecord(resumeDataPath: resumeDataURL())

      let reconciled = DownloadManager.reconciledRecord(record, hasLiveTask: false)

      XCTAssertEqual(reconciled?.status, .paused)
      // The resume data represents these bytes, so the count stands.
      XCTAssertEqual(reconciled?.receivedBytes, 500)
      XCTAssertEqual(reconciled?.totalBytes, 1000)
      XCTAssertEqual(reconciled?.resumeDataPath, resumeDataURL())
   }

   func testStrandedRecordWithoutResumeDataBecomesIdleAtZero() {
      let record = inProgressRecord()

      let reconciled = DownloadManager.reconciledRecord(record, hasLiveTask: false)

      XCTAssertEqual(reconciled?.status, .idle)
      // Nothing to resume from, so the download restarts from scratch.
      XCTAssertEqual(reconciled?.receivedBytes, 0)
      // The total came from headers and is still true of the remote file.
      XCTAssertEqual(reconciled?.totalBytes, 1000)
   }

   func testNonInProgressRecordsAreLeftAlone() {
      // Derived from the enum so a status added later is covered automatically.
      for status in DownloadStatus.allCases where status != .inProgress {
         var record = inProgressRecord(resumeDataPath: resumeDataURL())
         record.setStatus(status)

         XCTAssertNil(
            DownloadManager.reconciledRecord(record, hasLiveTask: false),
            "\(status) should not be reconciled"
         )
      }
   }

   // MARK: - Placing the downloaded file

   func testPlacingDownloadedFileCreatesMissingParentDirectory() throws {
      let source = try makeTemporaryFile(contents: "payload")
      let destination = makeTemporaryDirectoryURL()
         .appendingPathComponent("nested", isDirectory: true)
         .appendingPathComponent("file.mp4")

      try DownloadManager.placeDownloadedFile(from: source, to: destination)

      XCTAssertEqual(try String(contentsOf: destination, encoding: .utf8), "payload")
      // The move must consume the temporary file, not copy it.
      XCTAssertFalse(FileManager.default.fileExists(atPath: source.path))
   }

   func testPlacingDownloadedFileReplacesAnExistingFile() throws {
      let source = try makeTemporaryFile(contents: "new")
      let destination = try makeTemporaryFile(contents: "stale")

      try DownloadManager.placeDownloadedFile(from: source, to: destination)

      XCTAssertEqual(try String(contentsOf: destination, encoding: .utf8), "new")
   }

   func testPlacingDownloadedFileThrowsWhenTheDestinationIsUnreachable() throws {
      let source = try makeTemporaryFile(contents: "payload")

      // A regular file occupies the destination's parent, so the move cannot land.
      // Unthrown, this reports a completed download for a file that is not there.
      let blockingFile = try makeTemporaryFile(contents: "not a directory")
      let destination = blockingFile.appendingPathComponent("file.mp4")

      XCTAssertThrowsError(try DownloadManager.placeDownloadedFile(from: source, to: destination))
   }

   func testPlacingDownloadedFileThrowsWhenTheSourceIsMissing() {
      let source = makeTemporaryDirectoryURL().appendingPathComponent("gone.tmp")
      let destination = makeTemporaryDirectoryURL().appendingPathComponent("file.mp4")

      XCTAssertThrowsError(try DownloadManager.placeDownloadedFile(from: source, to: destination))
   }

   // MARK: - Helpers

   /// A directory unique to the calling test, removed when the test finishes.
   private func makeTemporaryDirectoryURL() -> URL {
      let directory = FileManager.default.temporaryDirectory
         .appendingPathComponent(UUID().uuidString, isDirectory: true)

      try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

      addTeardownBlock {
         try? FileManager.default.removeItem(at: directory)
      }

      return directory
   }

   private func makeTemporaryFile(contents: String) throws -> URL {
      let url = makeTemporaryDirectoryURL().appendingPathComponent(UUID().uuidString)

      try contents.write(to: url, atomically: true, encoding: .utf8)

      return url
   }

   // MARK: - Network policy

   private func idleRecord(allowMetered: Bool) -> DownloadRecord {
      return DownloadRecord(
         url: URL(string: "http://example.com/file.mp4")!,
         path: "/tmp/file.mp4",
         options: CreateOptions(allowMetered: allowMetered)
      )
   }

   func testUnrestrictedRequestLeavesEveryPathAllowed() {
      let request = DownloadManager.request(for: idleRecord(allowMetered: true), userAgent: nil)

      XCTAssertEqual(request.url, URL(string: "http://example.com/file.mp4"))
      XCTAssertTrue(request.allowsCellularAccess)
      XCTAssertTrue(request.allowsExpensiveNetworkAccess)
      XCTAssertTrue(request.allowsConstrainedNetworkAccess)
   }

   func testRestrictedRequestRefusesMeteredAndConstrainedPaths() {
      // All three, not just allowsCellularAccess: expensive covers personal
      // hotspots and constrained covers Low Data Mode, which together match the
      // desktop check that rejects a metered *or* constrained connection.
      let request = DownloadManager.request(for: idleRecord(allowMetered: false), userAgent: nil)

      XCTAssertEqual(request.url, URL(string: "http://example.com/file.mp4"))
      XCTAssertFalse(request.allowsCellularAccess)
      XCTAssertFalse(request.allowsExpensiveNetworkAccess)
      XCTAssertFalse(request.allowsConstrainedNetworkAccess)
   }

   func testConfiguredUserAgentIsSetOnTheRequest() {
      let request = DownloadManager.request(
         for: idleRecord(allowMetered: true),
         userAgent: "my-app/1.0"
      )

      XCTAssertEqual(request.value(forHTTPHeaderField: "User-Agent"), "my-app/1.0")
   }

   func testNoUserAgentLeavesTheHeaderUnset() {
      // The setting is opt-in: unset must leave URLSession's own default in place
      // rather than start sending a header the app did not ask for.
      let request = DownloadManager.request(
         for: idleRecord(allowMetered: true),
         userAgent: nil
      )

      XCTAssertNil(request.value(forHTTPHeaderField: "User-Agent"))
   }

   // -- User agent holder --

   func testUserAgentHolderStoresTheValue() async {
      let holder = UserAgentHolder()

      await holder.set("my-app/1.0")

      let value = await holder.value
      XCTAssertEqual(value, "my-app/1.0")
   }

   func testUserAgentHolderOverwritesAnEarlierValue() async {
      // A holder that latched its first value would pass the test above and still
      // send a stale user agent for the rest of the process.
      let holder = UserAgentHolder()

      await holder.set("my-app/1.0")
      await holder.set(nil)

      let value = await holder.value
      XCTAssertNil(value)
   }
}
