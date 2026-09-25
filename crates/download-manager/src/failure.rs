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

   /// Classifies HTTP responses consistently with the native implementations.
   pub fn http(status: u16) -> Self {
      Self {
         code: ErrorCode::Http,
         message: format!("HTTP {status}"),
         retryability: if matches!(status, 408 | 429 | 500 | 502..=504 | 506..=599) {
            Retryability::Transient
         } else {
            Retryability::Permanent
         },
         http_status: Some(status),
      }
   }

   /// Preserves the filesystem cause instead of parsing platform-dependent wording.
   pub fn file(error: &std::io::Error) -> Self {
      use std::io::ErrorKind;
      let mut failure = Self::command(ErrorCode::File, error.to_string());
      if matches!(
         error.kind(),
         ErrorKind::PermissionDenied
            | ErrorKind::StorageFull
            | ErrorKind::ReadOnlyFilesystem
            | ErrorKind::NotFound
            | ErrorKind::NotADirectory
            | ErrorKind::IsADirectory
            | ErrorKind::InvalidInput
      ) {
         failure.retryability = Retryability::Permanent;
      }
      failure
   }

   /// Inspects typed causes; certificate failures must not look like connection loss.
   pub fn request(error: reqwest::Error) -> Self {
      let error = error.without_url();
      if let Some(status) = error.status() {
         return Self::http(status.as_u16());
      }
      let mut failure = Self::command(ErrorCode::Unknown, error.to_string());
      if error.is_timeout() {
         failure.code = ErrorCode::Timeout;
         failure.retryability = Retryability::Transient;
         return failure;
      }
      let mut source = std::error::Error::source(&error);
      while let Some(cause) = source {
         if let Some((code, retryability)) = Self::network_cause(cause) {
            failure.code = code;
            failure.retryability = retryability;
            return failure;
         }
         source = cause.source();
      }
      if error.is_connect() {
         // A connect failure can also be DNS. Reqwest does not expose a stable DNS
         // category, so preserve uncertainty rather than inspecting its text.
         failure.code = ErrorCode::Connection;
      } else if error.is_body() || error.is_request() {
         failure.code = ErrorCode::Connection;
         failure.retryability = Retryability::Transient;
      }
      failure
   }

   /// Extracts stable categories exposed by the transport's typed error chain.
   fn network_cause(
      cause: &(dyn std::error::Error + 'static),
   ) -> Option<(ErrorCode, Retryability)> {
      if let Some(tls) = cause.downcast_ref::<rustls::Error>() {
         return Some((
            ErrorCode::Tls,
            if matches!(tls, rustls::Error::InvalidCertificate(_)) {
               Retryability::Permanent
            } else {
               Retryability::Transient
            },
         ));
      }
      if let Some(io) = cause.downcast_ref::<std::io::Error>() {
         use std::io::ErrorKind;
         // io::Error::source skips the payload itself, which can be the TLS cause.
         if let Some(inner) = io.get_ref()
            && let Some(classification) = Self::network_cause(inner)
         {
            return Some(classification);
         }
         return match io.kind() {
            ErrorKind::TimedOut => Some((ErrorCode::Timeout, Retryability::Transient)),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe => {
               Some((ErrorCode::Connection, Retryability::Transient))
            }
            ErrorKind::ConnectionRefused => Some((ErrorCode::Connection, Retryability::Unknown)),
            _ => None,
         };
      }
      None
   }
}

impl std::fmt::Display for DownloadFailure {
   fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      formatter.write_str(&self.message)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn http_classification_matches_shared_fixture() {
      let cases: Vec<serde_json::Value> =
         serde_json::from_str(include_str!("../../../fixtures/http-errors.json")).unwrap();
      for case in cases {
         let status = case["status"].as_u64().unwrap() as u16;
         let failure = serde_json::to_value(DownloadFailure::http(status)).unwrap();
         assert_eq!(failure["code"], "http");
         assert_eq!(failure["retryability"], case["retryability"]);
         assert_eq!(failure["httpStatus"], status);
      }
   }

   #[test]
   fn classifies_typed_network_and_file_causes() {
      use std::io::{Error, ErrorKind};
      assert_eq!(
         DownloadFailure::network_cause(&Error::from(ErrorKind::TimedOut)),
         Some((ErrorCode::Timeout, Retryability::Transient))
      );
      assert_eq!(
         DownloadFailure::network_cause(&Error::from(ErrorKind::ConnectionReset)),
         Some((ErrorCode::Connection, Retryability::Transient))
      );
      assert_eq!(
         DownloadFailure::network_cause(&rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer
         )),
         Some((ErrorCode::Tls, Retryability::Permanent))
      );
      for kind in [ErrorKind::StorageFull, ErrorKind::PermissionDenied] {
         let failure = DownloadFailure::file(&Error::new(kind, "timeout"));
         assert_eq!(failure.code, ErrorCode::File);
         assert_eq!(failure.retryability, Retryability::Permanent);
      }
      assert_eq!(
         DownloadFailure::file(&Error::other("permission denied")).retryability,
         Retryability::Unknown
      );
   }

   #[tokio::test]
   async fn request_timeout_is_classified_without_parsing_text() {
      let server = wiremock::MockServer::start().await;
      wiremock::Mock::given(wiremock::matchers::method("GET"))
         .respond_with(
            wiremock::ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(100)),
         )
         .mount(&server)
         .await;
      let error = reqwest::Client::builder()
         .timeout(std::time::Duration::from_millis(10))
         .build()
         .unwrap()
         .get(server.uri())
         .send()
         .await
         .unwrap_err();
      let failure = DownloadFailure::request(error);
      assert_eq!(failure.code, ErrorCode::Timeout);
      assert_eq!(failure.retryability, Retryability::Transient);
      assert!(!failure.message.contains(&server.uri()));
   }
}
