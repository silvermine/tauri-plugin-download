// Desktop error types
#[cfg(desktop)]
pub use download_manager::{Error, Result};

// Mobile error types (iOS, Android)
#[cfg(any(mobile, test))]
mod mobile_error {
   use serde::{Serialize, ser::Serializer};

   #[cfg(mobile)]
   pub type Result<T> = std::result::Result<T, Error>;

   #[cfg(mobile)]
   #[derive(Debug, thiserror::Error)]
   pub enum Error {
      #[error("{0}")]
      Transfer(download_manager::DownloadFailure),

      #[error(transparent)]
      Io(#[from] std::io::Error),

      #[error(transparent)]
      PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),

      #[error(transparent)]
      DownloadManager(#[from] download_manager::Error),
   }

   /// Borrowed error data lets host tests exercise the mobile wire contract.
   /// Tauri's invocation types remain mobile-only; no substitute native types are used.
   enum Rejection<'a> {
      Transfer(&'a download_manager::DownloadFailure),
      DownloadManager(&'a download_manager::Error),
      Io(&'a std::io::Error),
      InvokeRejected(Option<&'a str>),
      PluginInvoke(&'a dyn std::fmt::Display),
   }

   #[cfg(mobile)]
   impl Serialize for Error {
      fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
      where
         S: Serializer,
      {
         use tauri::plugin::mobile::PluginInvokeError;

         let rejection = match self {
            Self::Transfer(error) => Rejection::Transfer(error),
            Self::DownloadManager(error) => Rejection::DownloadManager(error),
            Self::Io(error) => Rejection::Io(error),
            Self::PluginInvoke(PluginInvokeError::InvokeRejected(error)) => {
               Rejection::InvokeRejected(error.message.as_deref())
            }
            Self::PluginInvoke(error) => Rejection::PluginInvoke(error),
         };
         rejection.serialize(serializer)
      }
   }

   impl Serialize for Rejection<'_> {
      fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
      where
         S: Serializer,
      {
         use download_manager::{DownloadFailure, ErrorCode};

         let failure = match self {
            Self::Transfer(failure) => (*failure).clone(),
            Self::DownloadManager(error) => error.failure(),
            Self::Io(error) => DownloadFailure::file(error),
            Self::InvokeRejected(message) => DownloadFailure::command(
               ErrorCode::Unknown,
               message.unwrap_or("Native command failed").into(),
            ),
            Self::PluginInvoke(error) => {
               DownloadFailure::command(ErrorCode::Unknown, error.to_string())
            }
         };
         failure.serialize(serializer)
      }
   }

   #[cfg(test)]
   mod tests {
      use super::Rejection;
      use rstest::rstest;
      use serde_json::{json, to_value};

      #[rstest]
      #[case::transfer_failure(
         Rejection::Transfer(&download_manager::DownloadFailure::http(429)),
         json!({ "code": "http", "message": "HTTP 429", "httpStatus": 429 })
      )]
      #[case::download_manager_error(
         Rejection::DownloadManager(&download_manager::Error::NotFound("/tmp/missing".into())),
         json!({ "code": "download not found", "message": "Not Found: /tmp/missing" })
      )]
      #[case::io_error(
         Rejection::Io(&std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "access denied",
         )),
         json!({ "code": "file", "message": "access denied" })
      )]
      fn rejection_preserves_its_classification(
         #[case] rejection: Rejection<'_>,
         #[case] expected: serde_json::Value,
      ) {
         assert_eq!(to_value(rejection).unwrap(), expected);
      }

      #[rstest]
      #[case::native_message(Some("native rejection"), "native rejection")]
      #[case::empty_native_message(Some(""), "")]
      #[case::absent_message(None, "Native command failed")]
      fn invocation_rejection_uses_fallback_only_when_message_is_absent(
         #[case] message: Option<&str>,
         #[case] expected: &str,
      ) {
         assert_eq!(
            to_value(Rejection::InvokeRejected(message)).unwrap(),
            json!({
               "code": "unknown", "message": expected
            })
         );
      }

      // The target-only adapter supplies Display for unreachable-webview,
      // serialization/deserialization, and Android JNI errors alike.
      #[rstest]
      #[case::unreachable_webview("the webview is unreachable")]
      #[case::deserialization_failure("failed to deserialize response: invalid type")]
      #[case::serialization_failure("failed to serialize payload: invalid value")]
      #[case::jni_error("jni error: unavailable")]
      #[case::message_naming_a_timeout_and_an_http_status("timeout HTTP 404")]
      fn other_invocation_errors_keep_diagnostics_without_guessing_codes(#[case] message: &str) {
         assert_eq!(
            to_value(Rejection::PluginInvoke(&message)).unwrap(),
            json!({
               "code": "unknown", "message": message
            })
         );
      }
   }
}

#[cfg(mobile)]
pub use mobile_error::{Error, Result};
