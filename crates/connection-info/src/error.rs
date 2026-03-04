use serde::{Serialize, ser::Serializer};

/// Errors that can occur when detecting the connection type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
   /// The current platform does not support connection type detection.
   #[error("connection type detection is not supported on this platform")]
   Unsupported,

   /// A Windows API call failed.
   #[cfg(windows)]
   #[error("windows API error: {0}")]
   Windows(#[from] windows::core::Error),
}

impl Serialize for Error {
   fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
   where
      S: Serializer,
   {
      serializer.serialize_str(self.to_string().as_ref())
   }
}

/// A specialized [`Result`] type for connection info operations.
pub type Result<T> = std::result::Result<T, Error>;
