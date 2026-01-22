//
//  UrlParser.swift
//  DownloadManagerKit
//

import Foundation

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
   
   guard url.host != nil && !url.host!.isEmpty else {
      throw DownloadError.invalidUrl("URL must have a host")
   }
   
   return url
}
