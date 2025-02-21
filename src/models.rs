use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEvent {
  pub key: String,
  pub progress: Option<f64>, 
  pub state: DownloadState, 
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
  pub key: String,
  pub url: String,
  pub path: String,
  pub progress: f64,
  pub state: DownloadState,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DownloadState {
#[default]
  Unknown,
  Created,
  InProgress,
  Paused,
  Cancelled,
  Completed
}

pub trait DownloadRecordExt {
  fn with_progress(&self, new_progress: f64) -> DownloadRecord;
  fn with_state(&self, new_state: DownloadState) -> DownloadRecord;
}

impl DownloadRecordExt for DownloadRecord {
  fn with_progress(&self, new_progress: f64) -> DownloadRecord {
      DownloadRecord {
          progress: new_progress,
          state: DownloadState::InProgress,
          ..self.clone() // Clone the rest of the fields
      }
  }

  fn with_state(&self, new_state: DownloadState) -> DownloadRecord {
      DownloadRecord {
          state: new_state,
          ..self.clone() // Clone the rest of the fields
      }
  }
}

impl fmt::Display for DownloadState {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
      let text = match self {
        DownloadState::Unknown => "Unknown",
        DownloadState::Created => "Created",
        DownloadState::InProgress => "InProgress",
        DownloadState::Paused => "Paused",
        DownloadState::Cancelled => "Cancelled",
        DownloadState::Completed => "Completed",
      };
      write!(f, "{}", text)
  }
}