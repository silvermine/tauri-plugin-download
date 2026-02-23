//
//  UrlParser.swift
//  DownloadManagerKit
//

import Foundation

/// Parses and validates a download path string.
/// Checks that the path is not empty, is an absolute path and contains a filename.
public func parsePath(_ pathString: String) throws -> URL {
   if pathString.isEmpty {
       throw DownloadError.invalidPath("Path cannot be empty")
   }

   let url: URL
   if pathString.hasPrefix("file://") {
       guard let fileUrl = URL(string: pathString), fileUrl.isFileURL else {
           throw DownloadError.invalidPath("Invalid file URL: \(pathString)")
       }
       url = fileUrl
   } else if pathString.hasPrefix("/") {
       url = URL(fileURLWithPath: pathString)
   } else {
       throw DownloadError.invalidPath("Path must be absolute")
   }

   let fileName = url.lastPathComponent
   if fileName.isEmpty || fileName == "/" {
       throw DownloadError.invalidPath("Path must have a filename")
   }
   
   return url
}

/// Parses and validates a download URL string.
/// Checks that the URL is valid, has a valid scheme (http or https) and has a valid host.
public func parseUrl(_ urlString: String) throws -> URL {
   guard let url = URL(string: urlString) else {
      throw DownloadError.invalidUrl("Invalid URL: \(urlString)")
   }
   
   let scheme = url.scheme?.lowercased()
   guard scheme == "http" || scheme == "https" else {
      throw DownloadError.invalidUrl("Invalid URL scheme '\(scheme ?? "none")': must be http or https")
   }
   
   guard let host = url.host, !host.isEmpty else {
      throw DownloadError.invalidUrl("URL must have a host")
   }
   
   return url
}
