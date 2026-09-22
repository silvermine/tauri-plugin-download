// Desktop error types
#[cfg(desktop)]
pub use download_manager::{Error, Result};

// Mobile error types (iOS, Android)
#[cfg(mobile)]
mod mobile_error {
   use serde::{Serialize, ser::Serializer};

   pub type Result<T> = std::result::Result<T, Error>;

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

   impl Serialize for Error {
      fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
      where
         S: Serializer,
      {
         use download_manager::{DownloadFailure, ErrorCode};
         use tauri::plugin::mobile::PluginInvokeError;

         let failure = match self {
            Self::Transfer(failure) => failure.clone(),
            Self::DownloadManager(error) => error.failure(),
            Self::Io(error) => DownloadFailure::file(error),
            Self::PluginInvoke(PluginInvokeError::InvokeRejected(error)) => {
               DownloadFailure::native_command(
                  error.code.as_deref(),
                  error
                     .message
                     .clone()
                     .unwrap_or_else(|| "Native command failed".into()),
               )
            }
            Self::PluginInvoke(error) => {
               DownloadFailure::command(ErrorCode::Unknown, error.to_string())
            }
         };
         failure.serialize(serializer)
      }
   }
}

#[cfg(mobile)]
pub use mobile_error::{Error, Result};
