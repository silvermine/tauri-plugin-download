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
   #[serde(other)]
   Unknown,
}

/// Public diagnostic data shared by command and transfer errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFailure {
   pub code: ErrorCode,
   #[serde(default = "default_failure_message")]
   pub message: String,
   #[serde(skip_serializing_if = "Option::is_none")]
   pub http_status: Option<u16>,
}

/// Older or future stores may omit the diagnostic message.
fn default_failure_message() -> String {
   "Download failed".into()
}

impl DownloadFailure {
   /// Builds a command rejection without guessing from its diagnostic message.
   pub fn command(code: ErrorCode, message: String) -> Self {
      // Without a response status there is no HTTP failure to expose.
      let code = if code == ErrorCode::Http {
         ErrorCode::Unknown
      } else {
         code
      };
      Self {
         code,
         message,
         http_status: None,
      }
   }

   /// Classifies HTTP responses consistently with the native implementations.
   pub fn http(status: u16) -> Self {
      Self {
         code: ErrorCode::Http,
         message: format!("HTTP {status}"),
         http_status: Some(status),
      }
   }

   /// Preserves the filesystem cause instead of parsing platform-dependent wording.
   pub fn file(error: &std::io::Error) -> Self {
      Self::command(ErrorCode::File, error.to_string())
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
         return failure;
      }
      let mut source = std::error::Error::source(&error);
      while let Some(cause) = source {
         if let Some(code) = Self::network_cause(cause) {
            failure.code = code;
            return failure;
         }
         source = cause.source();
      }
      if error.is_connect() || error.is_body() || error.is_request() {
         failure.code = ErrorCode::Connection;
      }
      failure
   }

   /// Extracts stable categories exposed by the transport's typed error chain.
   fn network_cause(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorCode> {
      if cause.downcast_ref::<rustls::Error>().is_some() {
         return Some(ErrorCode::Tls);
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
            ErrorKind::TimedOut => Some(ErrorCode::Timeout),
            ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof => Some(ErrorCode::Connection),
            ErrorKind::ConnectionRefused => Some(ErrorCode::Connection),
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
   use rstest::rstest;
   use std::io::{Error, ErrorKind};

   #[test]
   fn http_classification_matches_shared_fixture() {
      let cases: Vec<serde_json::Value> =
         serde_json::from_str(include_str!("../../../fixtures/http-errors.json")).unwrap();
      for case in cases {
         let status = case["status"].as_u64().unwrap() as u16;
         let failure = serde_json::to_value(DownloadFailure::http(status)).unwrap();
         assert_eq!(failure["code"], "http");
         assert_eq!(failure["httpStatus"], status);
      }
   }

   #[rstest]
   #[case::timed_out(&Error::from(ErrorKind::TimedOut), ErrorCode::Timeout)]
   #[case::connection_reset(&Error::from(ErrorKind::ConnectionReset), ErrorCode::Connection)]
   #[case::untrusted_certificate(
      &rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
      ErrorCode::Tls
   )]
   fn classifies_typed_network_causes(
      #[case] cause: &(dyn std::error::Error + 'static),
      #[case] expected: ErrorCode,
   ) {
      assert_eq!(DownloadFailure::network_cause(cause), Some(expected));
   }

   #[rstest]
   #[case::storage_full_worded_as_timeout(Error::new(ErrorKind::StorageFull, "timeout"))]
   #[case::permission_denied_worded_as_timeout(Error::new(ErrorKind::PermissionDenied, "timeout"))]
   #[case::other_worded_as_permission_denied(Error::other("permission denied"))]
   fn classifies_file_causes_without_parsing_text(#[case] error: Error) {
      assert_eq!(DownloadFailure::file(&error).code, ErrorCode::File);
   }

   #[tokio::test]
   async fn truncated_response_body_is_a_connection_failure() {
      use futures::StreamExt;
      use tokio::io::{AsyncReadExt, AsyncWriteExt};
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let address = listener.local_addr().unwrap();
      let server = tokio::spawn(async move {
         let (mut socket, _) = listener.accept().await.unwrap();
         let mut request = Vec::new();
         let mut buffer = [0; 1024];
         while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
         }
         socket
            .write_all(
               b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n0123456789",
            )
            .await
            .unwrap();
         socket.shutdown().await.unwrap();
      });
      let response = reqwest::Client::builder()
         .no_proxy()
         .timeout(std::time::Duration::from_secs(3))
         .build()
         .unwrap()
         .get(format!("http://{address}/file"))
         .send()
         .await
         .unwrap();
      let mut body = response.bytes_stream();
      let error = loop {
         match body.next().await {
            Some(Err(error)) => break error,
            Some(Ok(_)) => {}
            None => panic!("truncated body was accepted"),
         }
      };
      assert_eq!(DownloadFailure::request(error).code, ErrorCode::Connection);
      server.await.unwrap();
   }

   #[rstest]
   #[case::invalid_input(ErrorCode::InvalidInput)]
   #[case::invalid_state(ErrorCode::InvalidState)]
   #[case::download_not_found(ErrorCode::DownloadNotFound)]
   #[case::network_unavailable(ErrorCode::NetworkUnavailable)]
   #[case::network_restricted(ErrorCode::NetworkRestricted)]
   #[case::timeout(ErrorCode::Timeout)]
   #[case::connection(ErrorCode::Connection)]
   #[case::tls(ErrorCode::Tls)]
   #[case::http(ErrorCode::Http)]
   #[case::file(ErrorCode::File)]
   #[case::store(ErrorCode::Store)]
   #[case::unknown(ErrorCode::Unknown)]
   fn error_codes_round_trip(#[case] code: ErrorCode) {
      let value = serde_json::to_value(code).unwrap();
      assert_eq!(serde_json::from_value::<ErrorCode>(value).unwrap(), code);
   }

   #[test]
   fn future_error_codes_use_unknown() {
      assert_eq!(
         serde_json::from_str::<ErrorCode>(r#""future error""#).unwrap(),
         ErrorCode::Unknown
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
      assert!(!failure.message.contains(&server.uri()));
   }
}
