//
//  AppDelegate.swift
//  DownloadManagerExample
//

import UIKit
import DownloadManagerKit

class AppDelegate: NSObject, UIApplicationDelegate {
   func application(_ application: UIApplication, handleEventsForBackgroundURLSession identifier: String, completionHandler: @escaping () -> Void) {
      DownloadManager.shared.setBackgroundCompletionHandler(completionHandler)
   }
}
