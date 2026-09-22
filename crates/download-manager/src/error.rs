use serde::{Serialize, ser::Serializer};

use crate::{DownloadFailure, ErrorCode};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
   #[error("{0}")]
   Transfer(DownloadFailure),

   #[error("Invalid State")]
   InvalidState,

   #[error("Not Found: {0}")]
   NotFound(String),

   #[error("Store Error: {0}")]
   Store(String),

   #[error("File Error: {0}")]
   File(String),

   #[error("HTTP Error: {0}")]
   Http(String),

   #[error("URL Error: {0}")]
   Url(String),

   #[error("Path Error: {0}")]
   Path(String),

   #[error("User Agent Error: {0}")]
   UserAgent(String),

   #[error("Network unavailable: no active connection")]
   NetworkUnavailable,

   #[error("Network restricted: metered or constrained connections are not allowed")]
   NetworkRestricted,

   #[error("Connectivity Error: {0}")]
   Connectivity(String),

   #[error("Internal Error: {0}")]
   Internal(String),

   #[error(transparent)]
   Io(#[from] std::io::Error),
}

impl Error {
   /// Builds the public rejection while retaining the native error for Rust callers.
   pub fn failure(&self) -> DownloadFailure {
      let code = match self {
         Self::Transfer(failure) => return failure.clone(),
         Self::Io(error) => return DownloadFailure::file(error),
         Self::InvalidState => ErrorCode::InvalidState,
         Self::NotFound(_) => ErrorCode::DownloadNotFound,
         Self::Store(_) => ErrorCode::Store,
         Self::File(_) => ErrorCode::File,
         Self::Http(_) => ErrorCode::Http,
         Self::Url(_) | Self::Path(_) | Self::UserAgent(_) => ErrorCode::InvalidInput,
         Self::NetworkUnavailable => ErrorCode::NetworkUnavailable,
         Self::NetworkRestricted => ErrorCode::NetworkRestricted,
         Self::Connectivity(_) | Self::Internal(_) => ErrorCode::Unknown,
      };
      DownloadFailure::command(code, self.to_string())
   }
}

impl From<reqwest::Error> for Error {
   fn from(error: reqwest::Error) -> Self {
      Self::Transfer(DownloadFailure::request(error))
   }
}

impl From<reqwest_middleware::Error> for Error {
   fn from(error: reqwest_middleware::Error) -> Self {
      match error {
         reqwest_middleware::Error::Reqwest(error) => error.into(),
         reqwest_middleware::Error::Middleware(error) => Self::Transfer(DownloadFailure::command(
            ErrorCode::Unknown,
            error.to_string(),
         )),
      }
   }
}

impl Serialize for Error {
   fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
   where
      S: Serializer,
   {
      self.failure().serialize(serializer)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn test_error_display() {
      assert_eq!(Error::InvalidState.to_string(), "Invalid State");
      assert_eq!(
         Error::NotFound("test.mp4".to_string()).to_string(),
         "Not Found: test.mp4"
      );
      assert_eq!(
         Error::Store("failed".to_string()).to_string(),
         "Store Error: failed"
      );
      assert_eq!(
         Error::File("denied".to_string()).to_string(),
         "File Error: denied"
      );
      assert_eq!(
         Error::Http("timeout".to_string()).to_string(),
         "HTTP Error: timeout"
      );
      assert_eq!(
         Error::NetworkUnavailable.to_string(),
         "Network unavailable: no active connection"
      );
      assert_eq!(
         Error::NetworkRestricted.to_string(),
         "Network restricted: metered or constrained connections are not allowed"
      );
      assert_eq!(
         Error::Connectivity("backend unavailable".to_string()).to_string(),
         "Connectivity Error: backend unavailable"
      );
      assert_eq!(
         Error::Internal("Connectivity worker failed".to_string()).to_string(),
         "Internal Error: Connectivity worker failed"
      );
   }

   #[test]
   fn test_error_serialize() {
      let e = Error::Http("connection failed".to_string());
      let json = serde_json::to_string(&e).unwrap();
      assert_eq!(
         serde_json::from_str::<serde_json::Value>(&json).unwrap(),
         serde_json::json!({
            "code": "http",
            "message": "HTTP Error: connection failed",
            "retryability": "unknown"
         })
      );
   }

   #[test]
   fn command_rejections_have_stable_codes_and_retry_advice() {
      use crate::Retryability::{Permanent, Transient, Unknown};
      for (error, code, retryability) in [
         (Error::InvalidState, "invalid state", Permanent),
         (
            Error::NotFound("/tmp/a".into()),
            "download not found",
            Permanent,
         ),
         (Error::Url("bad URL".into()), "invalid input", Permanent),
         (Error::Path("bad path".into()), "invalid input", Permanent),
         (
            Error::UserAgent("bad agent".into()),
            "invalid input",
            Permanent,
         ),
         (Error::NetworkUnavailable, "network unavailable", Transient),
         (Error::NetworkRestricted, "network restricted", Transient),
         (Error::File("timeout".into()), "file", Unknown),
         (Error::Store("HTTP 404".into()), "store", Unknown),
         (Error::Internal("timeout".into()), "unknown", Unknown),
         (Error::Connectivity("offline".into()), "unknown", Unknown),
      ] {
         let value = serde_json::to_value(&error).unwrap();
         assert_eq!(value["code"], code);
         assert_eq!(value["message"], error.to_string());
         assert_eq!(
            value["retryability"],
            serde_json::to_value(retryability).unwrap()
         );
         assert!(value.get("httpStatus").is_none());
      }
   }

   #[test]
   fn test_error_io_from() {
      let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
      let e: Error = io_err.into();
      assert!(e.to_string().contains("file not found"));
   }
}
