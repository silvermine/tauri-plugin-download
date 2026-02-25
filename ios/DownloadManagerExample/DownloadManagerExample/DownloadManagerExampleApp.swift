//
//  DownloadManagerExampleApp.swift
//  DownloadManagerExample
//

import SwiftUI

@main
struct DownloadManagerExampleApp: App {
   @UIApplicationDelegateAdaptor(AppDelegate.self) var appDelegate
   var body: some Scene {
      WindowGroup {
         DownloadsView()
      }
   }
}
