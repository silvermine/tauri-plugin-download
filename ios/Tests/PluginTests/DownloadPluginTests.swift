import CoreFoundation
import Foundation
import WebKit
import XCTest
@testable import Tauri
@testable import tauri_plugin_download

final class DownloadPluginTests: XCTestCase {
   // The native singleton opens this store once for the entire test process.
   private static let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
   private static let plugin = DownloadPlugin()
   private static var configured = false
   private static var loaded = false
   private var paths: [String] = []

   override func setUp() async throws {
      if !Self.configured {
         PluginManager.shared.load(name: "download", plugin: Self.plugin, config: "{}", webview: nil)
         // First access to the singleton initializes URLSession's system service.
         // A freshly booted CI simulator can take longer than an ordinary command.
         let result = try await invoke("configure", ["storeDir": Self.directory.path, "userAgent": "plugin-test/1.0"], timeout: 60)
         XCTAssertTrue(result is NSNull)
         Self.configured = true
      }
   }

   override func tearDown() async throws {
      for path in paths { _ = try await invoke("cancel", ["path": path]) }
   }

   override class func tearDown() {
      try? FileManager.default.removeItem(at: directory)
      super.tearDown()
   }

   private func newPath() -> String {
      let path = Self.directory.appendingPathComponent(UUID().uuidString).path
      paths.append(path)
      return path
   }

   func testCreateGetListAndCancelPreserveOptionsAndResponseShapes() async throws {
      for allowed in [false, true] {
         let path = newPath()
         let created = try await invoke("create", ["path": path, "url": "https://example.com/file", "options": ["allowMetered": allowed]])
         let action = try XCTUnwrap(created as? [String: Any])
         XCTAssertEqual(action["expectedStatus"] as? String, "idle")
         XCTAssertEqual(action["isExpectedStatus"] as? Bool, true)
         let item = try XCTUnwrap(action["download"] as? [String: Any])
         XCTAssertEqual(item["url"] as? String, "https://example.com/file")
         XCTAssertEqual(item["path"] as? String, path)
         XCTAssertEqual(item["status"] as? String, "idle")
         XCTAssertEqual((item["options"] as? [String: Any])?["allowMetered"] as? Bool, allowed)
         let receivedBytes = try XCTUnwrap(item["receivedBytes"] as? NSNumber)
         XCTAssertNotEqual(CFGetTypeID(receivedBytes), CFBooleanGetTypeID())
         XCTAssertEqual(receivedBytes.doubleValue, 0)
         let progress = try XCTUnwrap(item["progress"] as? NSNumber)
         XCTAssertNotEqual(CFGetTypeID(progress), CFBooleanGetTypeID())
         XCTAssertEqual(progress.doubleValue, 0)
         XCTAssertTrue(item["totalBytes"] is NSNull)
         XCTAssertNil(item["resumeDataPath"])
         let got = try await invoke("get", ["path": path]) as? [String: Any]
         XCTAssertEqual((got?["options"] as? [String: Any])?["allowMetered"] as? Bool, allowed)
         let listed = try await invoke("list") as? [[String: Any]]
         XCTAssertNotNil(listed, "iOS returns a bare array")
         XCTAssertTrue(listed?.contains { $0["path"] as? String == path } == true)
         XCTAssertTrue(FileManager.default.fileExists(atPath: Self.directory.appendingPathComponent("downloads.json").path))
         let paused = try await invoke("pause", ["path": path]) as? [String: Any]
         XCTAssertEqual(paused?["expectedStatus"] as? String, "paused")
         XCTAssertEqual(paused?["isExpectedStatus"] as? Bool, false)
         let canceled = try await invoke("cancel", ["path": path]) as? [String: Any]
         XCTAssertEqual(canceled?["expectedStatus"] as? String, "canceled")
         XCTAssertEqual(canceled?["isExpectedStatus"] as? Bool, true)
         let missing = try await invoke("get", ["path": path])
         XCTAssertTrue(missing is NSNull)
      }
   }

   func testMalformedArgumentsAndMissingDownloadsUseErrorMarker() async throws {
      let path = newPath()
      for (command, args) in [
         ("get", [:]), ("get", ["path": 42]), ("get", ["path": "relative"]),
         ("create", ["path": path, "url": "https://example.com/file"]),
         ("create", ["path": path, "url": "bad", "options": ["allowMetered": false]]),
      ] as [(String, [String: Any])] {
         let value = try await invoke(command, args) as? [String: Any]
         let error = try XCTUnwrap(value?["__downloadError"] as? [String: Any])
         XCTAssertEqual(error["code"] as? String, "invalid input")
         XCTAssertNotNil(error["message"] as? String)
      }
      for command in ["pause", "cancel"] {
         let value = try await invoke(command, ["path": path]) as? [String: Any]
         XCTAssertEqual((value?["__downloadError"] as? [String: Any])?["code"] as? String, "download not found")
      }
   }

   func testChangedEventsUseRealListenerChannel() async throws {
      let events = EventLog()
      _ = try await invoke("registerListener", ["event": "changed", "handler": "__CHANNEL__:79"], channel: { id, payload in
         XCTAssertEqual(id, 79)
         events.append(payload)
      })
      if !Self.loaded {
         await MainActor.run { Self.plugin.load(webview: WKWebView()) }
         Self.loaded = true
      }
      // load starts an async subscription. Probe through real create/cancel commands
      // until the channel observes it; no timing assumption about Task scheduling.
      let deadline = Date().addingTimeInterval(5)
      while events.values.isEmpty && Date() < deadline {
         let path = newPath()
         _ = try await invoke("create", ["path": path, "url": "https://example.com/event", "options": ["allowMetered": false]])
         _ = try await invoke("cancel", ["path": path])
      }
      XCTAssertFalse(events.values.isEmpty, "Manager changes must reach the registered Tauri channel")
      for payload in events.values {
         let item = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(payload.utf8)) as? [String: Any])
         XCTAssertTrue(paths.contains(item["path"] as? String ?? ""))
         XCTAssertEqual((item["options"] as? [String: Any])?["allowMetered"] as? Bool, false)
      }
      _ = try await invoke("removeListener", ["event": "changed", "channelId": 79])
      let count = events.values.count
      Self.plugin.trigger("changed", data: ["probe": true])
      XCTAssertEqual(events.values.count, count)
   }

   /// Use Tauri's dispatcher so the Objective-C command selectors are exercised too.
   private func invoke(_ command: String, _ args: [String: Any] = [:], timeout: TimeInterval = 10,
      channel: @escaping (UInt64, String) -> Void = { _, _ in XCTFail("Unexpected channel") }) async throws -> Any {
      let completed = expectation(description: command)
      completed.assertForOverFulfill = true
      let result = EventLog()
      let data = try JSONSerialization.data(withJSONObject: args)
      let invocation = Invoke(command: command, callback: 1, error: 2, sendResponse: { callback, value in
         XCTAssertEqual(callback, 1, "Unexpected transport rejection: \(value ?? "null")")
         result.append(value ?? "null")
         completed.fulfill()
      }, sendChannelData: channel, data: String(decoding: data, as: UTF8.self))
      PluginManager.shared.invoke(name: "download", invoke: invocation)
      await fulfillment(of: [completed], timeout: timeout)
      let payload = try XCTUnwrap(result.values.first)
      return try JSONSerialization.jsonObject(with: Data(payload.utf8), options: [.fragmentsAllowed])
   }
}

private final class EventLog {
   private let lock = NSLock()
   private var storage: [String] = []
   var values: [String] {
      lock.lock()
      defer { lock.unlock() }
      return storage
   }

   func append(_ value: String) {
      lock.lock()
      defer { lock.unlock() }
      storage.append(value)
   }
}
