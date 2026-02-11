//
//  BackgroundSessionHandler.swift
//  DownloadManagerKit
//

/// Actor that synchronizes access to the background session completion handler.
/// Handles the race condition where the URLSession delegate may fire before the handler is set.
actor BackgroundSessionHandler {
   private var completionHandler: (() -> Void)?
   private var pendingComplete = false
   
   func set(_ handler: @escaping () -> Void) {
      completionHandler = handler
      if pendingComplete {
         pendingComplete = false
         handler()
         completionHandler = nil
      }
   }
   
   func handleComplete() {
      if let handler = completionHandler {
         handler()
         completionHandler = nil
      } else {
         pendingComplete = true
      }
   }
}
