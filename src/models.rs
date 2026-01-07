// Desktop, Android model types
#[cfg(any(desktop, target_os = "android"))]
pub use download_manager::{DownloadActionResponse, DownloadItem};

// iOS model types
#[cfg(target_os = "ios")]
mod ios_types {
   use serde::{Deserialize, Serialize};

   #[derive(Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct PathArgs {
      pub path: String,
   }

   #[derive(Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct CreateArgs {
      pub path: String,
      pub url: String,
   }

   #[derive(Debug, Clone, Default, Deserialize, Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct DownloadItem {
      pub url: String,
      pub path: String,
      pub progress: f64,
      pub status: DownloadStatus,
   }

   #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub enum DownloadStatus {
      #[default]
      Unknown,
      Pending,
      Idle,
      InProgress,
      Paused,
      Cancelled,
      Completed,
   }

   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub struct DownloadActionResponse {
      pub download: DownloadItem,
      pub expected_status: DownloadStatus,
      pub is_expected_status: bool,
   }

   impl DownloadActionResponse {
      pub fn new(download: DownloadItem) -> Self {
         let expected_status = download.status.clone();
         Self {
            download,
            expected_status,
            is_expected_status: true,
         }
      }

      pub fn with_expected_status(download: DownloadItem, expected_status: DownloadStatus) -> Self {
         let is_expected_status = download.status == expected_status;
         Self {
            download,
            expected_status,
            is_expected_status,
         }
      }
   }
}

#[cfg(target_os = "ios")]
pub use ios_types::{CreateArgs, DownloadActionResponse, DownloadItem, DownloadStatus, PathArgs};
