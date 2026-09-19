use serde::{Deserialize, Serialize};
use std::fmt;

/// Options that control when a download is allowed to use the network.
///
/// Options are fixed when the download is created and remain unchanged for the
/// lifetime of its persisted record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOptions {
   /// Whether the download may start on metered or constrained connections.
   pub allow_metered: bool,
}

impl Default for CreateOptions {
   fn default() -> Self {
      Self {
         allow_metered: true,
      }
   }
}

/// Persisted download record. Stored in `downloads.json`.
///
/// Does not contain `progress` — that is a derived value only present in
/// [`DownloadItem`], which is what gets sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadRecord {
   pub url: String,
   pub path: String,
   pub options: CreateOptions,
   pub received_bytes: u64,
   pub total_bytes: Option<u64>,
   pub status: DownloadStatus,
}

/// Public payload sent to the frontend. Built from a [`DownloadRecord`] with
/// `progress` computed from `received_bytes` / `total_bytes`.
///
/// `Deserialize` must stay: on mobile, this is what a native command response
/// decodes into, in addition to a change event payload on desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
   pub url: String,
   pub path: String,
   /// Network policy fixed when the download was created.
   pub options: CreateOptions,
   pub received_bytes: u64,
   pub total_bytes: Option<u64>,
   pub progress: f64,
   pub status: DownloadStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
   /// Download has been created and is ready to start.
   Idle,
   /// Download is in progress.
   InProgress,
   /// Download was in progress but has been paused.
   Paused,
   /// Download was canceled by the user.
   Canceled,
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
   pub fn new(item: DownloadItem) -> Self {
      let expected_status = item.status.clone();
      Self {
         download: item,
         expected_status,
         is_expected_status: true,
      }
   }

   pub fn with_expected_status(item: DownloadItem, expected_status: DownloadStatus) -> Self {
      let is_expected_status = item.status == expected_status;
      Self {
         download: item,
         expected_status,
         is_expected_status,
      }
   }
}

impl DownloadRecord {
   pub fn with_bytes(&self, received_bytes: u64, total_bytes: Option<u64>) -> DownloadRecord {
      DownloadRecord {
         received_bytes,
         total_bytes,
         ..self.clone()
      }
   }

   pub fn with_status(&self, new_status: DownloadStatus) -> DownloadRecord {
      DownloadRecord {
         status: new_status,
         ..self.clone()
      }
   }

   pub fn to_item(&self) -> DownloadItem {
      let progress = match (&self.status, self.total_bytes) {
         (DownloadStatus::Completed, _) => 100.0,
         (_, Some(total)) if total > 0 => {
            ((self.received_bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
         }
         _ => 0.0,
      };
      DownloadItem {
         url: self.url.clone(),
         path: self.path.clone(),
         options: self.options,
         received_bytes: self.received_bytes,
         total_bytes: self.total_bytes,
         progress,
         status: self.status.clone(),
      }
   }
}

/// Renders the string serde does, so a status reads the same in a log line as in a
/// payload. `test_display_matches_the_serialized_form` holds the two together.
impl fmt::Display for DownloadStatus {
   fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
      let text = match self {
         DownloadStatus::Idle => "idle",
         DownloadStatus::InProgress => "inProgress",
         DownloadStatus::Paused => "paused",
         DownloadStatus::Canceled => "canceled",
         DownloadStatus::Completed => "completed",
      };
      write!(f, "{}", text)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn sample_record() -> DownloadRecord {
      DownloadRecord {
         url: "http://example.com/file.mp4".to_string(),
         path: "/tmp/file.mp4".to_string(),
         options: CreateOptions::default(),
         received_bytes: 0,
         total_bytes: None,
         status: DownloadStatus::Idle,
      }
   }

   #[test]
   fn test_download_record_with_bytes() {
      let record = sample_record();
      let updated = record.with_bytes(500, Some(1000));
      assert_eq!(updated.received_bytes, 500);
      assert_eq!(updated.total_bytes, Some(1000));
      assert_eq!(updated.status, DownloadStatus::Idle);
      assert_eq!(updated.url, record.url);
      assert_eq!(updated.path, record.path);
   }

   #[test]
   fn test_download_record_with_status() {
      let mut record = sample_record();
      record.received_bytes = 500;
      record.total_bytes = Some(1000);

      // Preserves bytes for non-completed status
      let paused = record.with_status(DownloadStatus::Paused);
      assert_eq!(paused.received_bytes, 500);
      assert_eq!(paused.total_bytes, Some(1000));
      assert_eq!(paused.status, DownloadStatus::Paused);

      // Status transitions preserve the factual byte counts, including completion.
      let completed = record.with_status(DownloadStatus::Completed);
      assert_eq!(completed.received_bytes, 500);
      assert_eq!(completed.total_bytes, Some(1000));
      assert_eq!(completed.status, DownloadStatus::Completed);
      assert_eq!(completed.to_item().progress, 100.0);
   }

   #[test]
   fn test_with_status_completed_unknown_size() {
      // Completed with unknown size preserves received_bytes
      let mut record = sample_record();
      record.received_bytes = 5000;
      let completed = record.with_status(DownloadStatus::Completed);
      assert_eq!(completed.received_bytes, 5000);
      assert_eq!(completed.total_bytes, None);
   }

   #[test]
   fn test_to_item_with_known_size() {
      let mut record = sample_record();
      record.options.allow_metered = false;
      record.received_bytes = 500;
      record.total_bytes = Some(1000);
      let item = record.to_item();
      assert!(!item.options.allow_metered);
      assert_eq!(item.progress, 50.0);
      assert_eq!(item.received_bytes, 500);
      assert_eq!(item.total_bytes, Some(1000));
      assert_eq!(
         serde_json::to_value(&item).unwrap()["options"]["allowMetered"],
         false
      );
   }

   #[test]
   fn test_to_item_clamps_progress_to_100_percent() {
      let mut record = sample_record();
      record.received_bytes = 1500;
      record.total_bytes = Some(1000);
      assert_eq!(record.to_item().progress, 100.0);
   }

   #[test]
   fn test_to_item_with_unknown_size() {
      let record = sample_record();
      let item = record.to_item();
      assert_eq!(item.progress, 0.0);

      // Completed with unknown size still reports 100%
      let completed = record.with_status(DownloadStatus::Completed);
      assert_eq!(completed.to_item().progress, 100.0);
   }

   #[test]
   fn test_deserialize_with_byte_fields() {
      let json = r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","options":{"allowMetered":true},"receivedBytes":500,"totalBytes":1000,"status":"paused"}"#;
      let record: DownloadRecord = serde_json::from_str(json).unwrap();
      assert_eq!(record.received_bytes, 500);
      assert_eq!(record.total_bytes, Some(1000));
      // progress is derived via to_item(), not stored
      assert_eq!(record.to_item().progress, 50.0);
   }

   #[test]
   fn test_deserialize_without_received_bytes_fails() {
      let json = r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","options":{"allowMetered":true},"status":"idle"}"#;

      assert!(serde_json::from_str::<DownloadRecord>(json).is_err());
   }

   #[test]
   fn test_deserialize_omitted_total_bytes_is_none() {
      // serde reads an absent Option as None on its own, so totalBytes stays
      // optional on the wire whatever attributes it carries. Nothing writes a
      // record without it: None serializes as an explicit null.
      let json = r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","options":{"allowMetered":true},"receivedBytes":500,"status":"idle"}"#;
      let record: DownloadRecord = serde_json::from_str(json).unwrap();

      assert_eq!(record.total_bytes, None);
      assert_eq!(record.received_bytes, 500);
   }

   #[test]
   fn test_deserialize_without_options_fails() {
      let json = r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","status":"idle"}"#;

      assert!(serde_json::from_str::<DownloadRecord>(json).is_err());
   }

   #[test]
   fn test_download_item_round_trip() {
      // The shape a Rust consumer decodes a `tauri-plugin-download:changed` payload
      // into. Asserting through the JSON keeps the camelCase field names — what the
      // frontend, both natives and the event payload all use — under test.
      let item = sample_record().with_bytes(500, Some(1000)).to_item();

      let json = serde_json::to_string(&item).unwrap();
      assert!(json.contains(r#""receivedBytes":500"#));
      assert!(json.contains(r#""totalBytes":1000"#));

      let decoded: DownloadItem = serde_json::from_str(&json).unwrap();
      assert_eq!(decoded.url, item.url);
      assert_eq!(decoded.path, item.path);
      assert_eq!(decoded.options, item.options);
      assert_eq!(decoded.received_bytes, 500);
      assert_eq!(decoded.total_bytes, Some(1000));
      assert_eq!(decoded.progress, 50.0);
      assert_eq!(decoded.status, DownloadStatus::Idle);
   }

   #[test]
   fn test_download_action_response_round_trip() {
      // The mobile bridge decodes every command response into this type, so it has
      // to survive the trip with the same camelCase keys the natives emit.
      let item = sample_record().to_item();
      let response = DownloadActionResponse::with_expected_status(item, DownloadStatus::InProgress);

      let json = serde_json::to_string(&response).unwrap();
      assert!(json.contains(r#""expectedStatus":"inProgress""#));
      assert!(json.contains(r#""isExpectedStatus":false"#));

      let decoded: DownloadActionResponse = serde_json::from_str(&json).unwrap();
      assert_eq!(decoded.expected_status, DownloadStatus::InProgress);
      assert!(!decoded.is_expected_status);
      assert_eq!(decoded.download.status, DownloadStatus::Idle);
   }

   #[test]
   fn test_create_options_default_allows_metered_connections() {
      assert!(CreateOptions::default().allow_metered);
   }

   #[test]
   fn test_create_options_without_allow_metered_fails() {
      assert!(serde_json::from_str::<CreateOptions>("{}").is_err());
   }

   #[test]
   fn test_create_options_round_trip_restriction() {
      let options = CreateOptions {
         allow_metered: false,
      };
      let json = serde_json::to_string(&options).unwrap();

      assert_eq!(json, r#"{"allowMetered":false}"#);
      assert_eq!(
         serde_json::from_str::<CreateOptions>(&json).unwrap(),
         options
      );
   }

   #[test]
   fn test_download_action_response() {
      let record = sample_record();
      let item = record.to_item();

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
      // Display
      assert_eq!(format!("{}", DownloadStatus::InProgress), "inProgress");
      assert_eq!(format!("{}", DownloadStatus::Completed), "completed");
   }

   #[test]
   fn test_display_matches_the_serialized_form() {
      // The invariant the `Display` impl exists under: one spelling of a status,
      // whether it reaches a reader through a log line or through JSON.
      let statuses = [
         DownloadStatus::Idle,
         DownloadStatus::InProgress,
         DownloadStatus::Paused,
         DownloadStatus::Canceled,
         DownloadStatus::Completed,
      ];

      for status in statuses {
         // The arms below make adding a variant a compile error here. Whoever
         // fixes that error also has to add the new variant to the `statuses`
         // array above, which these assertions iterate — the compiler cannot
         // enforce that second step.
         match &status {
            DownloadStatus::Idle
            | DownloadStatus::InProgress
            | DownloadStatus::Paused
            | DownloadStatus::Canceled
            | DownloadStatus::Completed => {}
         }

         let serialized = serde_json::to_string(&status).unwrap();

         assert_eq!(format!("\"{}\"", status), serialized);
      }
   }
}
