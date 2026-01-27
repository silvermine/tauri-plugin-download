use serde::{Deserialize, Serialize};
use std::fmt;

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
   /// Status could not be determined.
   #[default]
   Unknown,
   /// Download has not yet been created/persisted.
   Pending,
   /// Download has been created and is ready to start.
   Idle,
   /// Download is in progress.
   InProgress,
   /// Download was in progress but has been paused.
   Paused,
   /// Download was cancelled by the user.
   Cancelled,
   /// Download completed.
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

impl DownloadItem {
   pub fn with_progress(&self, new_progress: f64) -> DownloadItem {
      DownloadItem {
         progress: new_progress,
         status: DownloadStatus::InProgress,
         ..self.clone()
      }
   }

   pub fn with_status(&self, new_status: DownloadStatus) -> DownloadItem {
      DownloadItem {
         progress: if new_status == DownloadStatus::Completed {
            100.0
         } else {
            self.progress
         },
         status: new_status,
         ..self.clone()
      }
   }
}

impl fmt::Display for DownloadStatus {
   fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
      let text = match self {
         DownloadStatus::Unknown => "Unknown",
         DownloadStatus::Pending => "Pending",
         DownloadStatus::Idle => "Idle",
         DownloadStatus::InProgress => "InProgress",
         DownloadStatus::Paused => "Paused",
         DownloadStatus::Cancelled => "Cancelled",
         DownloadStatus::Completed => "Completed",
      };
      write!(f, "{}", text)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn sample_item() -> DownloadItem {
      DownloadItem {
         url: "http://example.com/file.mp4".to_string(),
         path: "/tmp/file.mp4".to_string(),
         progress: 0.0,
         status: DownloadStatus::Idle,
      }
   }

   #[test]
   fn test_download_item_with_progress() {
      let item = sample_item();
      let updated = item.with_progress(50.0);
      assert_eq!(updated.progress, 50.0);
      assert_eq!(updated.status, DownloadStatus::InProgress);
      assert_eq!(updated.url, item.url);
      assert_eq!(updated.path, item.path);
   }

   #[test]
   fn test_download_item_with_status() {
      let mut item = sample_item();
      item.progress = 50.0;

      // Preserves progress for non-completed status
      let paused = item.with_status(DownloadStatus::Paused);
      assert_eq!(paused.progress, 50.0);
      assert_eq!(paused.status, DownloadStatus::Paused);

      // Sets progress to 100 for completed status
      let completed = item.with_status(DownloadStatus::Completed);
      assert_eq!(completed.progress, 100.0);
      assert_eq!(completed.status, DownloadStatus::Completed);
   }

   #[test]
   fn test_download_action_response() {
      let item = sample_item();

      // new() sets is_expected_status to true
      let response = DownloadActionResponse::new(item.clone());
      assert!(response.is_expected_status);
      assert_eq!(response.expected_status, DownloadStatus::Idle);

      // with_expected_status() - matching status
      let match_response =
         DownloadActionResponse::with_expected_status(item.clone(), DownloadStatus::Idle);
      assert!(match_response.is_expected_status);

      // with_expected_status() - mismatched status
      let mismatch_response =
         DownloadActionResponse::with_expected_status(item, DownloadStatus::InProgress);
      assert!(!mismatch_response.is_expected_status);
   }

   #[test]
   fn test_download_status() {
      // Default
      let status: DownloadStatus = Default::default();
      assert_eq!(status, DownloadStatus::Unknown);

      // Display
      assert_eq!(format!("{}", DownloadStatus::Unknown), "Unknown");
      assert_eq!(format!("{}", DownloadStatus::InProgress), "InProgress");
      assert_eq!(format!("{}", DownloadStatus::Completed), "Completed");
   }
}
