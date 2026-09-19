import XCTest
@testable import DownloadManagerKit

final class DownloadStoreTests: XCTestCase {

   private var savePath: URL!

   override func setUp() {
      super.setUp()
      savePath = FileManager.default.temporaryDirectory
         .appendingPathComponent(UUID().uuidString)
         .appendingPathComponent("downloads.json")
      try? FileManager.default.createDirectory(
         at: savePath.deletingLastPathComponent(),
         withIntermediateDirectories: true
      )
   }

   override func tearDown() {
      try? FileManager.default.removeItem(at: savePath.deletingLastPathComponent())
      savePath = nil
      super.tearDown()
   }

   private func write(_ json: String) {
      try? Data(json.utf8).write(to: savePath)
   }

   private func record(path: String, received: UInt64 = 0, total: UInt64? = 1000) -> DownloadRecord {
      return DownloadRecord(
         url: URL(string: "http://example.com/\(path)")!,
         path: "/tmp/\(path)",
         receivedBytes: received,
         totalBytes: total,
         status: .paused
      )
   }

   // MARK: - Loading

   func testLoadsPersistedRecords() {
      write("""
      {"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":500,"totalBytes":1000,"status":"paused"}]}
      """)

      let records = DownloadStore.load(from: savePath)

      XCTAssertEqual(records.count, 1)
      XCTAssertEqual(records.first?.receivedBytes, 500)
      XCTAssertEqual(records.first?.totalBytes, 1000)
      XCTAssertEqual(records.first?.status, .paused)
      XCTAssertEqual(records.first?.options.allowMetered, true)
   }


   func testMissingFileLoadsEmpty() {
      XCTAssertEqual(DownloadStore.load(from: savePath).count, 0)
   }

   func testOneUnreadableRecordDiscardsTheWholeStore() {
      // Records are accepted as a whole; #64 will preserve unreadable files,
      // rather than salvage individual records.
      write("""
      {"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":1,"status":"paused"},
       {"url":"http://example.com/b.mp4","status":"paused"},
       {"url":"http://example.com/c.mp4","path":"/tmp/c.mp4","options":{"allowMetered":true},"receivedBytes":3,"status":"idle"}]}
      """)

      XCTAssertEqual(DownloadStore.load(from: savePath).count, 0)
   }

   func testMalformedFileLoadsEmpty() {
      write("this is not json")

      XCTAssertEqual(DownloadStore.load(from: savePath).count, 0)
   }

   func testUnknownKeysAreIgnored() {
      write("""
      {"version":1,"downloads":[{"url":"http://example.com/a.mp4","path":"/tmp/a.mp4","options":{"allowMetered":true},"receivedBytes":7,"status":"idle","somethingNew":42}]}
      """)

      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 7)
   }

   // MARK: - Persistence

   func testAppendPersists() async {
      let store = DownloadStore(savePath: savePath)
      await store.append(record(path: "a.mp4", received: 42))

      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 42)
   }

   func testAppendPersistsIntoADirectoryThatDoesNotExistYet() async {
      // The configured-store-location case, and the one `setUp`'s pre-created
      // directory hides: `Data.write` does not create intermediate directories, so
      // without `save` creating them every write fails into a log line while the
      // in-memory array keeps serving `list` — the store simply never appears.
      let directory = FileManager.default.temporaryDirectory
         .appendingPathComponent(UUID().uuidString)
         .appendingPathComponent("nested")
      let path = directory.appendingPathComponent("downloads.json")

      addTeardownBlock {
         try? FileManager.default.removeItem(at: directory.deletingLastPathComponent())
      }

      let store = DownloadStore(savePath: path)
      await store.append(record(path: "a.mp4", received: 42))

      XCTAssertEqual(DownloadStore.load(from: path).first?.receivedBytes, 42)
   }

   func testUpdateWithoutPersistLeavesTheFileUntouched() async throws {
      let store = DownloadStore(savePath: savePath)
      await store.append(record(path: "a.mp4", received: 42))
      let before = try Data(contentsOf: savePath)

      var updated = record(path: "a.mp4")
      updated.setBytes(received: 999)
      await store.update(updated, persist: false)

      // In memory only: this runs on every progress tick.
      let inMemory = await store.findByPath(updated.path)?.receivedBytes
      XCTAssertEqual(inMemory, 999)
      XCTAssertEqual(try Data(contentsOf: savePath), before)
      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 42)
   }

   func testUpdatePersistsWhenAsked() async {
      let store = DownloadStore(savePath: savePath)
      await store.append(record(path: "a.mp4", received: 42))

      var updated = record(path: "a.mp4")
      updated.setBytes(received: 999)
      await store.update(updated)

      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 999)
   }

   func testBatchUpdateAppliesEveryRecord() async {
      let store = DownloadStore(savePath: savePath)
      await store.append(record(path: "a.mp4", received: 1))
      await store.append(record(path: "b.mp4", received: 2))

      var first = record(path: "a.mp4")
      first.setBytes(received: 10)
      var second = record(path: "b.mp4")
      second.setBytes(received: 20)
      await store.update([first, second])

      let persisted = DownloadStore.load(from: savePath).sorted { $0.receivedBytes < $1.receivedBytes }
      XCTAssertEqual(persisted.map { $0.receivedBytes }, [10, 20])
   }

   func testBatchUpdateIgnoresUnknownPaths() async {
      let store = DownloadStore(savePath: savePath)
      await store.append(record(path: "a.mp4", received: 1))

      await store.update([record(path: "never-added.mp4", received: 5)])

      let count = await store.list().count
      XCTAssertEqual(count, 1)
      XCTAssertEqual(DownloadStore.load(from: savePath).first?.receivedBytes, 1)
   }

   // MARK: - Total compare-and-set

   func testSetTotalIfChangedReportsOnlyRealChanges() async {
      let store = DownloadStore(savePath: savePath)
      let stored = record(path: "a.mp4", total: nil)
      await store.append(stored)
      XCTAssertNil(stored.totalBytes)

      let changed = await store.setTotalIfChanged(path: stored.path, total: 2000)
      XCTAssertEqual(changed?.totalBytes, 2000)

      // Second callback reporting the same total must not report a change.
      let unchanged = await store.setTotalIfChanged(path: stored.path, total: 2000)
      XCTAssertNil(unchanged)
   }

   func testSetTotalIfChangedDoesNotPersist() async throws {
      let store = DownloadStore(savePath: savePath)
      let stored = record(path: "a.mp4", total: nil)
      await store.append(stored)
      let before = try Data(contentsOf: savePath)

      _ = await store.setTotalIfChanged(path: stored.path, total: 2000)

      // It runs on the hottest callback in the system; pause, cancel and
      // completion are what write the record.
      XCTAssertEqual(try Data(contentsOf: savePath), before)
   }

   // MARK: - Atomic mutation

   func testConcurrentMutationsEachObserveThePreviousOne() async {
      // Each body runs inside the actor, so every increment sees the one before it.
      // Composed from findByPath and update, they would land on the same snapshot.
      let store = DownloadStore(savePath: savePath)
      let path = "/tmp/a.mp4"
      let mutations = 200

      await store.append(record(path: "a.mp4"))

      await withTaskGroup(of: Void.self) { group in
         for _ in 0..<mutations {
            group.addTask {
               _ = await store.mutate(path: path, persist: false) { current in
                  current.setBytes(received: current.receivedBytes + 1, total: current.totalBytes)
               }
            }
         }
      }

      let updated = await store.findByPath(path)

      XCTAssertEqual(updated?.receivedBytes, UInt64(mutations))
   }

   func testMutateReturnsTheStoredRecordRatherThanTheCallersSnapshot() async {
      // What a caller emits must be what the store holds: a pause committed between
      // two progress callbacks must not be reported as still in progress.
      let store = DownloadStore(savePath: savePath)
      let path = "/tmp/a.mp4"

      await store.append(record(path: "a.mp4"))
      _ = await store.mutate(path: path, persist: false) { $0.setStatus(.inProgress) }

      let paused = await store.mutate(path: path, persist: false) { $0.setStatus(.paused) }
      let afterProgress = await store.mutate(path: path, persist: false) { current in
         current.setBytes(received: 500, total: current.totalBytes)
      }

      let unknown = await store.mutate(path: "/tmp/gone.mp4", persist: false) {
         $0.setStatus(.canceled)
      }

      XCTAssertEqual(paused?.status, .paused)
      // The byte-only mutation must carry the status the pause left behind.
      XCTAssertEqual(afterProgress?.status, .paused)
      XCTAssertEqual(afterProgress?.receivedBytes, 500)
      XCTAssertNil(unknown)
   }
}
