// Desktop error types
#[cfg(desktop)]
#[allow(unused_imports)]
pub use download_manager::{Error, Result};

// Mobile error types (iOS, Android)
#[cfg(mobile)]
mod mobile_error {
   use serde::{Serialize, ser::Serializer};

   pub type Result<T> = std::result::Result<T, Error>;

   #[derive(Debug, thiserror::Error)]
   pub enum Error {
      #[error(transparent)]
      Io(#[from] std::io::Error),

      #[error(transparent)]
      PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
   }

   impl Serialize for Error {
      fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
      where
         S: Serializer,
      {
         serializer.serialize_str(self.to_string().as_ref())
      }
   }
}

#[cfg(mobile)]
pub use mobile_error::Result;
