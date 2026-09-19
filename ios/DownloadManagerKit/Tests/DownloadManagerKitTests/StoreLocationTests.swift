import XCTest
@testable import DownloadManagerKit

/// Covers where the store decides to persist itself.
///
/// `StoreLocation` is process-wide, so each test resets it on both sides. `tearDown`
/// alone protects the tests that follow but not this class from what precedes it:
/// XCTest orders classes arbitrarily within one process, so anything that constructs a
/// `DownloadStore` with its default path first would leave `isResolved` set and trap
/// the whole bundle here rather than failing one test.
final class StoreLocationTests: XCTestCase {

   override func setUp() {
      super.setUp()
      StoreLocation.resetForTesting()
   }

   override func tearDown() {
      StoreLocation.resetForTesting()
      super.tearDown()
   }

   func testTheDefaultPathIsInTheApplicationSupportDirectory() {
      // The behaviour an app that configures nothing keeps. Asserting the filename
      // alongside the directory: both platforms and the Rust store agree on it, and a
      // change here would silently orphan every existing store.
      //
      // Against the `FileManager` call rather than `StoreLocation.defaultDirectory`,
      // which is what this test exists to pin down. Comparing the two would hold for
      // whatever `defaultDirectory` returned — swapping it to `.cachesDirectory` would
      // move every unconfigured store into a purgeable directory and still pass.
      let path = StoreLocation.savePath()

      XCTAssertEqual(path.lastPathComponent, "downloads.json")
      XCTAssertEqual(
         path.deletingLastPathComponent(),
         FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
      )
   }

   func testAConfiguredDirectoryHoldsTheStore() throws {
      let directory = temporaryDirectoryURL()

      try StoreLocation.set(directory)

      XCTAssertEqual(StoreLocation.savePath(), directory.appendingPathComponent("downloads.json"))
   }

   func testAConfiguredDirectoryReplacesTheDefault() throws {
      // Pairs with the case above: without it, a `set` that appended to the default
      // rather than replacing it would still produce a path containing the directory.
      let directory = temporaryDirectoryURL()

      try StoreLocation.set(directory)

      XCTAssertNotEqual(
         StoreLocation.savePath().deletingLastPathComponent(),
         StoreLocation.defaultDirectory
      )
   }

   func testTheLastDirectorySetBeforeTheStoreOpensWins() throws {
      // The plugin sets once, but nothing in the type forbids two calls before the
      // store opens, and latching the first would be the wrong half of the contract:
      // it is setting *after* the open that is refused.
      let first = temporaryDirectoryURL()
      let second = temporaryDirectoryURL()

      try StoreLocation.set(first)
      try StoreLocation.set(second)

      XCTAssertEqual(StoreLocation.savePath(), second.appendingPathComponent("downloads.json"))
   }

   func testTheDirectoryIsCreated() throws {
      // `set` is handed a path that does not exist yet — the normal case on a first
      // launch. Creating it here is what lets an unusable location fail on this call
      // rather than in `save`, which can only log.
      let directory = temporaryDirectoryURL()

      try StoreLocation.set(directory)

      XCTAssertTrue(FileManager.default.fileExists(atPath: directory.path))
   }

   func testADirectoryThatCannotBeCreatedIsRefused() throws {
      // The misconfiguration this guard exists for. `DownloadStore.save` can only log a
      // write failure, so a directory the app cannot create has to fail here or every
      // record is lost with nothing reported to anyone. A regular file stands in for an
      // unwritable location: it makes `createDirectory` fail on a path component
      // without depending on sandbox permissions the test host does not have.
      let blocker = temporaryDirectoryURL()
      try Data().write(to: blocker)

      XCTAssertThrowsError(try StoreLocation.set(blocker.appendingPathComponent("store")))

      // The refusal leaves the store where it was rather than pointing it at a
      // directory nothing can be written to.
      XCTAssertEqual(
         StoreLocation.savePath().deletingLastPathComponent(),
         StoreLocation.defaultDirectory
      )
   }

   func testSettingTheDirectoryAfterTheStoreOpensIsRefused() throws {
      // What `set`'s throwing signature promises: it either applied the directory or
      // said so. This path used to return normally after an `assertionFailure` the
      // shipping build compiles out.
      _ = StoreLocation.savePath()

      let directory = temporaryDirectoryURL()

      XCTAssertThrowsError(try StoreLocation.set(directory)) { error in
         guard case StoreLocationError.alreadyResolved = error else {
            return XCTFail("expected alreadyResolved, got \(error)")
         }
      }

      XCTAssertEqual(
         StoreLocation.savePath().deletingLastPathComponent(),
         StoreLocation.defaultDirectory
      )
   }

   func testTheStoreDefaultsThroughStoreLocation() async throws {
      // The seam the whole iOS feature hangs on: a configured directory reaches the
      // store only through `DownloadStore.init`'s `savePath` default argument. Every
      // other test constructs the store with an explicit path, so nothing else in the
      // bundle ever evaluates that default — hardcode a path here instead of reading
      // `StoreLocation` and the rest of the suite would still pass green.
      let directory = temporaryDirectoryURL()
      try StoreLocation.set(directory)

      let store = DownloadStore()
      await store.append(
         DownloadRecord(
            url: URL(string: "http://example.com/a.mp4")!,
            path: "/tmp/a.mp4",
            receivedBytes: 42,
            totalBytes: 1000,
            status: .paused
         )
      )

      let persisted = DownloadStore.load(from: directory.appendingPathComponent("downloads.json"))

      XCTAssertEqual(persisted.first?.receivedBytes, 42)
   }

   /// A unique path under the temporary directory, removed when the test ends.
   ///
   /// Deliberately not created: `set` is what creates it, and two of these tests exist
   /// to prove that.
   ///
   /// - Returns: A path that does not exist yet.
   private func temporaryDirectoryURL() -> URL {
      let url = FileManager.default.temporaryDirectory
         .appendingPathComponent(UUID().uuidString)

      addTeardownBlock {
         try? FileManager.default.removeItem(at: url)
      }

      return url
   }
}
