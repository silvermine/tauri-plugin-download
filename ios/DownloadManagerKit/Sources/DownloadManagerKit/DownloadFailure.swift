import Foundation

/// Stable error data shared by command rejections, events and persisted records.
public struct DownloadFailure: Codable, Sendable, Error, Equatable {
   public let code: String
   public let message: String
   public let retryability: String
   public let httpStatus: Int?

   public init(code: String, message: String, retryability: String, httpStatus: Int? = nil) {
      self.code = code
      self.message = message
      self.retryability = retryability
      self.httpStatus = httpStatus
   }

   /// Uses the same HTTP classification as Rust and Android.
   public static func http(_ status: Int) -> DownloadFailure {
      let transient = status == 408 || status == 429 || (500...599).contains(status) && status != 501 && status != 505
      return DownloadFailure(code: "http", message: "HTTP \(status)",
         retryability: transient ? "transient" : "permanent", httpStatus: status)
   }

   /// Native domains/codes determine classification; message text is diagnostic only.
   public static func classify(_ error: Error) -> DownloadFailure {
      if let failure = error as? DownloadFailure { return failure }
      let native = error as NSError
      var code = commandErrorCode(error)
      var retryability = ["invalid input", "invalid state", "download not found"].contains(code) ? "permanent" : "unknown"
      if native.domain == NSURLErrorDomain {
         switch native.code {
         case NSURLErrorTimedOut:
            code = "timeout"; retryability = "transient"
         case NSURLErrorNetworkConnectionLost, NSURLErrorNotConnectedToInternet:
            code = "connection"; retryability = "transient"
         case NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed, NSURLErrorCannotConnectToHost:
            code = "connection"; retryability = "unknown"
         case NSURLErrorServerCertificateHasBadDate, NSURLErrorServerCertificateUntrusted,
              NSURLErrorServerCertificateHasUnknownRoot, NSURLErrorServerCertificateNotYetValid,
              NSURLErrorClientCertificateRejected, NSURLErrorClientCertificateRequired:
            code = "tls"; retryability = "permanent"
         case NSURLErrorCannotCreateFile, NSURLErrorCannotOpenFile, NSURLErrorCannotCloseFile,
              NSURLErrorCannotWriteToFile, NSURLErrorCannotRemoveFile, NSURLErrorCannotMoveFile:
            code = "file"
            if let underlying = native.userInfo[NSUnderlyingErrorKey] as? Error {
               retryability = classify(underlying).retryability
            }
         case NSURLErrorNoPermissionsToReadFile:
            code = "file"; retryability = "permanent"
         case NSURLErrorSecureConnectionFailed:
            code = "tls"; retryability = "transient"
         default: break
         }
      } else if native.domain == NSCocoaErrorDomain {
         code = "file"
         if [NSFileReadNoPermissionError, NSFileWriteNoPermissionError, NSFileWriteOutOfSpaceError,
             NSFileReadNoSuchFileError, NSFileNoSuchFileError, NSFileWriteVolumeReadOnlyError,
             NSFileWriteInvalidFileNameError].contains(native.code) {
            retryability = "permanent"
         }
      } else if native.domain == NSPOSIXErrorDomain {
         code = "file"
         if [Int(EACCES), Int(EPERM), Int(ENOSPC), Int(EROFS), Int(ENOENT), Int(ENOTDIR), Int(EISDIR), Int(EINVAL)].contains(native.code) {
            retryability = "permanent"
         }
      }
      return DownloadFailure(code: code, message: error.localizedDescription, retryability: retryability)
   }
}
