//
//  URLParser.swift
//  DownloadManagerKit
//

import Foundation

/// Parses and validates a download path string.
/// Checks that the path is not empty, is an absolute path and contains a filename.
/// Returns it unchanged, as it is the download's identity.
public func parsePath(_ pathString: String) throws -> String {
   if pathString.isEmpty {
       throw DownloadError.invalidPath("path cannot be empty")
   }

   guard pathString.hasPrefix("/") else {
       throw DownloadError.invalidPath("path must be absolute")
   }

   let filename = URL(fileURLWithPath: pathString).lastPathComponent
   if filename.isEmpty || filename == "/" {
       throw DownloadError.invalidPath("path must have a filename")
   }
   
   return pathString
}

/// Parses and validates a download URL string.
/// Checks that the URL is valid, has a valid scheme (http or https) and has a valid host.
public func parseURL(_ urlString: String) throws -> URL {
   if urlString.isEmpty {
      throw DownloadError.invalidURL("URL cannot be empty")
   }

   guard let url = URL(string: urlString) else {
      throw DownloadError.invalidURL("Invalid URL: \(urlString)")
   }
   
   // Without a scheme the string is not an absolute URL, which is where Rust's parser
   // stops too, so it gets the same message rather than a scheme error.
   guard let scheme = url.scheme?.lowercased() else {
      throw DownloadError.invalidURL("Invalid URL: \(urlString)")
   }

   guard scheme == "http" || scheme == "https" else {
      throw DownloadError.invalidURL("Invalid URL scheme '\(scheme)': must be http or https")
   }
   
   guard let host = url.host, !host.isEmpty else {
      throw DownloadError.invalidURL("URL must have a host")
   }
   
   return url
}
