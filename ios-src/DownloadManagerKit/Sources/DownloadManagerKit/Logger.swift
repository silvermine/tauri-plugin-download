//
//  Logger.swift
//  DownloadManagerKit
//

import Foundation
import os.log

enum Log {
   static let downloadStore = OSLog(subsystem: "DownloadManagerKit", category: "DownloadStore")
}
