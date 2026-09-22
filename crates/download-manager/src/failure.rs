use serde::{Deserialize, Serialize};

/// Stable machine-readable categories shared by command and transfer errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorCode {
   #[serde(rename = "invalid input")]
   InvalidInput,
   #[serde(rename = "invalid state")]
   InvalidState,
   #[serde(rename = "download not found")]
   DownloadNotFound,
   #[serde(rename = "network unavailable")]
   NetworkUnavailable,
   #[serde(rename = "network restricted")]
   NetworkRestricted,
   Timeout,
   Connection,
   Tls,
   Http,
   File,
   Store,
   Unknown,
}

/// Advice about repeating the unchanged operation, independent of resume support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retryability {
   Transient,
   Permanent,
   Unknown,
}

/// Serializable public error. Messages are diagnostic; consumers branch on codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFailure {
   pub code: ErrorCode,
   pub message: String,
   pub retryability: Retryability,
   #[serde(skip_serializing_if = "Option::is_none")]
   pub http_status: Option<u16>,
}

impl DownloadFailure {
   /// Builds a command rejection without guessing from its diagnostic message.
   /// File, store and opaque transport errors need more context to advise a retry.
   pub fn command(code: ErrorCode, message: String) -> Self {
      let retryability = match code {
         ErrorCode::InvalidInput | ErrorCode::InvalidState | ErrorCode::DownloadNotFound => {
            Retryability::Permanent
         }
         ErrorCode::NetworkUnavailable
         | ErrorCode::NetworkRestricted
         | ErrorCode::Timeout
         | ErrorCode::Connection => Retryability::Transient,
         _ => Retryability::Unknown,
      };
      Self {
         code,
         message,
         retryability,
         http_status: None,
      }
   }

   /// Decodes native command codes preserved by Tauri's mobile rejection bridge.
   /// Unknown or absent codes stay unknown, including framework-generated errors.
   pub fn native_command(code: Option<&str>, message: String) -> Self {
      let code = match code {
         Some("invalid input") => ErrorCode::InvalidInput,
         Some("invalid state") => ErrorCode::InvalidState,
         Some("download not found") => ErrorCode::DownloadNotFound,
         Some("file") => ErrorCode::File,
         Some("store") => ErrorCode::Store,
         _ => ErrorCode::Unknown,
      };
      Self::command(code, message)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn native_codes_do_not_depend_on_messages() {
      for (code, expected) in [
         (Some("invalid input"), ErrorCode::InvalidInput),
         (Some("invalid state"), ErrorCode::InvalidState),
         (Some("download not found"), ErrorCode::DownloadNotFound),
         (Some("file"), ErrorCode::File),
         (Some("store"), ErrorCode::Store),
         (Some("future code"), ErrorCode::Unknown),
         (None, ErrorCode::Unknown),
      ] {
         let failure = DownloadFailure::native_command(code, "timeout HTTP 404".into());
         assert_eq!(failure.code, expected);
         assert_eq!(failure.http_status, None);
      }
   }
}
