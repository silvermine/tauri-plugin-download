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

pub trait DownloadItemExt {
   fn with_progress(&self, new_progress: f64) -> DownloadItem;
   fn with_status(&self, new_status: DownloadStatus) -> DownloadItem;
}

impl DownloadItemExt for DownloadItem {
   fn with_progress(&self, new_progress: f64) -> DownloadItem {
      DownloadItem {
         progress: new_progress,
         status: DownloadStatus::InProgress,
         ..self.clone() // Clone the rest of the fields
      }
   }

   fn with_status(&self, new_status: DownloadStatus) -> DownloadItem {
      DownloadItem {
         progress: if new_status == DownloadStatus::Completed {
            100.0
         } else {
            self.progress
         },
         status: new_status,
         ..self.clone() // Clone the rest of the fields
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
