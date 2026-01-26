use serde::{Serialize, ser::Serializer};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
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

   #[error(transparent)]
   Io(#[from] std::io::Error),
}

impl Serialize for Error {
   fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
   where
      S: Serializer,
   {
      serializer.serialize_str(self.to_string().as_ref())
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
   }

   #[test]
   fn test_error_serialize() {
      let e = Error::Http("connection failed".to_string());
      let json = serde_json::to_string(&e).unwrap();
      assert_eq!(json, "\"HTTP Error: connection failed\"");
   }

   #[test]
   fn test_error_io_from() {
      let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
      let e: Error = io_err.into();
      assert!(e.to_string().contains("file not found"));
   }
}
