mod error;
mod types;

#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod windows;

pub use error::{Error, Result};
pub use types::{ConnectionStatus, ConnectionType};

/// Queries the current connection status.
///
/// Returns a [`ConnectionStatus`] describing whether the connection is metered, constrained,
/// and what physical transport is in use.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] on platforms without an implementation, or
/// [`Error::Windows`] if a Windows API call fails.
pub fn connection_status() -> Result<ConnectionStatus> {
   #[cfg(windows)]
   {
      self::windows::connection_status()
   }
   #[cfg(not(windows))]
   {
      self::unsupported::connection_status()
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   // serialization

   #[test]
   fn serializes_connection_status() {
      let status = ConnectionStatus {
         metered: true,
         constrained: false,
         connection_type: ConnectionType::Cellular,
      };
      let json = serde_json::to_value(&status).unwrap();
      assert_eq!(json["metered"], true);
      assert_eq!(json["constrained"], false);
      assert_eq!(json["connectionType"], "cellular");
   }

   #[test]
   fn deserializes_connection_status() {
      let json = r#"{"metered":false,"constrained":false,"connectionType":"wifi"}"#;
      let status: ConnectionStatus = serde_json::from_str(json).unwrap();
      assert!(!status.metered);
      assert!(!status.constrained);
      assert_eq!(status.connection_type, ConnectionType::Wifi);
   }

   // error

   #[test]
   fn unsupported_error_displays_message() {
      let err = Error::Unsupported;
      assert_eq!(
         err.to_string(),
         "connection type detection is not supported on this platform"
      );
   }

   #[test]
   fn no_connection_error_displays_message() {
      let err = Error::NoConnection;
      assert_eq!(err.to_string(), "no active internet connection");
   }

   // stub

   #[test]
   #[cfg(not(windows))]
   fn non_windows_returns_unsupported_error() {
      let result = connection_status();
      assert!(result.is_err());
      assert!(matches!(result.unwrap_err(), Error::Unsupported));
   }
}
