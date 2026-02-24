//
//  DownloadContinuation.swift
//  DownloadManagerKit
//

import Foundation

/// A continuation for sending download items to async streams
actor DownloadContinuation {
   private var continuations: [UUID: AsyncStream<DownloadItem>.Continuation] = [:]
   
   func add(_ continuation: AsyncStream<DownloadItem>.Continuation) -> UUID {
      let id = UUID()
      continuations[id] = continuation
      return id
   }
   
   func remove(_ id: UUID) {
      continuations.removeValue(forKey: id)
   }
   
   func yield(_ item: DownloadItem) {
      for (_, continuation) in continuations {
         continuation.yield(item)
      }
   }
   
   func finish() {
      for (_, continuation) in continuations {
         continuation.finish()
      }
      continuations.removeAll()
   }
}
