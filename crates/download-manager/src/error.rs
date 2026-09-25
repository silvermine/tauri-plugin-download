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
         reqwest_middleware::Error::Middleware(error) => {
            match error.downcast::<reqwest_retry::RetryError>() {
               Ok(reqwest_retry::RetryError::WithRetries { err, .. })
               | Ok(reqwest_retry::RetryError::Error(err)) => Self::from(err),
               Err(_) => Self::Transfer(DownloadFailure::command(
                  ErrorCode::Unknown,
                  "Download request middleware failed".into(),
               )),
            }
         }
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

   /// Uses the same retry middleware as download workers, including its error wrapper.
   fn manager_client(directory: &tempfile::TempDir) -> reqwest_middleware::ClientWithMiddleware {
      crate::DownloadManager::new(
         directory.path().to_path_buf(),
         std::sync::Arc::new(|_| {}),
         crate::DownloadManagerConfig::default(),
      )
      .http_client
   }

   #[tokio::test]
   async fn middleware_preserves_timeout_classification() {
      let directory = tempfile::TempDir::new().unwrap();
      let server = wiremock::MockServer::start().await;
      wiremock::Mock::given(wiremock::matchers::method("GET"))
         .respond_with(
            wiremock::ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(100)),
         )
         .expect(4)
         .mount(&server)
         .await;
      let error = manager_client(&directory)
         .get(format!("{}/?token=private-token", server.uri()))
         .timeout(std::time::Duration::from_millis(20))
         .send()
         .await
         .unwrap_err();
      let failure = Error::from(error).failure();
      assert_eq!(failure.code, ErrorCode::Timeout);
      assert_eq!(failure.retryability, crate::Retryability::Transient);
      assert!(!failure.message.contains("private-token"));
      server.verify().await;
   }

   #[tokio::test]
   async fn middleware_preserves_refused_connection_classification() {
      let directory = tempfile::TempDir::new().unwrap();
      let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
      let address = listener.local_addr().unwrap();
      drop(listener);
      let error = manager_client(&directory)
         .get(format!("http://{address}/"))
         .send()
         .await
         .unwrap_err();
      let failure = Error::from(error).failure();
      assert_eq!(failure.code, ErrorCode::Connection);
      assert_eq!(failure.retryability, crate::Retryability::Unknown);
   }

   #[tokio::test]
   async fn middleware_removes_url_from_fatal_redirect_error() {
      let directory = tempfile::TempDir::new().unwrap();
      let server = wiremock::MockServer::start().await;
      let url = format!("{}/?signature=private-token", server.uri());
      wiremock::Mock::given(wiremock::matchers::method("GET"))
         .respond_with(wiremock::ResponseTemplate::new(302).insert_header("Location", url.as_str()))
         .mount(&server)
         .await;
      let error = manager_client(&directory)
         .get(&url)
         .send()
         .await
         .unwrap_err();
      let failure = Error::from(error).failure();
      assert!(!failure.message.contains("private-token"));
      assert!(!failure.message.contains(&server.uri()));
      assert_eq!(failure.code, ErrorCode::Unknown);
   }

   #[tokio::test]
   async fn middleware_preserves_untrusted_certificate_classification() {
      use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
      let directory = tempfile::TempDir::new().unwrap();
      // Public, test-only self-signed certificate and private key. Never trusted by the client.
      let certificate =
         CertificateDer::from(include_bytes!("../tests/fixtures/untrusted-cert.der").to_vec());
      let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
         include_bytes!("../tests/fixtures/untrusted-key.der").to_vec(),
      ));
      let config = rustls::ServerConfig::builder()
         .with_no_client_auth()
         .with_single_cert(vec![certificate], key)
         .unwrap();
      let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let address = listener.local_addr().unwrap();
      let server = tokio::spawn(async move {
         loop {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = acceptor.accept(stream).await;
         }
      });
      let error = manager_client(&directory)
         .get(format!("https://{address}/"))
         .send()
         .await
         .unwrap_err();
      server.abort();
      let failure = Error::from(error).failure();
      assert_eq!(failure.code, ErrorCode::Tls);
      assert_eq!(failure.retryability, crate::Retryability::Permanent);
   }

   #[test]
   fn unknown_middleware_messages_are_not_exposed() {
      let error = reqwest_middleware::Error::Middleware(
         std::io::Error::other("https://example.com/?signature=private-token").into(),
      );
      let failure = Error::from(error).failure();
      assert_eq!(failure.code, ErrorCode::Unknown);
      assert_eq!(failure.message, "Download request middleware failed");
   }
}
