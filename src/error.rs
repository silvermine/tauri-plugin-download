// Desktop, Android error types
#[cfg(any(desktop, target_os = "android"))]
#[allow(unused_imports)]
pub use download_manager::{Error, Result};

// iOS error types
#[cfg(target_os = "ios")]
mod ios_error {
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

#[cfg(target_os = "ios")]
pub use ios_error::{Error, Result};
