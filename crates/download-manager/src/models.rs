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
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub error: Option<crate::DownloadFailure>,
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
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub error: Option<crate::DownloadFailure>,
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
   /// Download failed and can be resumed or canceled.
   Failed,
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
         error: if new_status == DownloadStatus::Failed {
            self.error.clone()
         } else {
            None
         },
         status: new_status,
         ..self.clone()
      }
   }

   /// Builds the public payload, computing `progress` from the byte counts.
   ///
   /// A completed download always reports 100%, even without a content length.
   /// In flight without one it reports 0%, since `received_bytes` is then the
   /// only meaningful signal.
   ///
   /// The clamp bounds the ratio; it does not assert the counts agree. Both
   /// reach the frontend unchanged, so read them for exact bytes.
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
         error: if self.status == DownloadStatus::Failed {
            Some(self.error.clone().unwrap_or_else(|| {
               crate::DownloadFailure::command(crate::ErrorCode::Unknown, "Download failed".into())
            }))
         } else {
            None
         },
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
         DownloadStatus::Failed => "failed",
         DownloadStatus::Canceled => "canceled",
         DownloadStatus::Completed => "completed",
      };
      write!(f, "{}", text)
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use rstest::rstest;

   fn sample_record() -> DownloadRecord {
      DownloadRecord {
         url: "http://example.com/file.mp4".to_string(),
         path: "/tmp/file.mp4".to_string(),
         options: CreateOptions::default(),
         received_bytes: 0,
         total_bytes: None,
         status: DownloadStatus::Idle,
         error: None,
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

   #[rstest]
   #[case::paused_known_size(500, Some(1000), DownloadStatus::Paused)]
   // Completion keeps the factual counts rather than snapping them to the total.
   #[case::completed_known_size(500, Some(1000), DownloadStatus::Completed)]
   #[case::completed_unknown_size(5000, None, DownloadStatus::Completed)]
   fn test_with_status_preserves_byte_counts(
      #[case] received_bytes: u64,
      #[case] total_bytes: Option<u64>,
      #[case] new_status: DownloadStatus,
   ) {
      let record = DownloadRecord {
         received_bytes,
         total_bytes,
         ..sample_record()
      };

      let updated = record.with_status(new_status.clone());

      assert_eq!(updated.received_bytes, received_bytes);
      assert_eq!(updated.total_bytes, total_bytes);
      assert_eq!(updated.status, new_status);
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

   #[rstest]
   #[case::received_beyond_total_clamps_to_100(DownloadStatus::Idle, 1500, Some(1000), 100.0)]
   #[case::idle_unknown_size(DownloadStatus::Idle, 0, None, 0.0)]
   // An empty body states a total of zero, which must not be divided by.
   #[case::in_progress_zero_total(DownloadStatus::InProgress, 0, Some(0), 0.0)]
   #[case::completed_unknown_size(DownloadStatus::Completed, 0, None, 100.0)]
   #[case::completed_short_of_known_total(DownloadStatus::Completed, 500, Some(1000), 100.0)]
   fn test_to_item_progress(
      #[case] status: DownloadStatus,
      #[case] received_bytes: u64,
      #[case] total_bytes: Option<u64>,
      #[case] expected: f64,
   ) {
      let record = DownloadRecord {
         status,
         received_bytes,
         total_bytes,
         ..sample_record()
      };

      assert_eq!(record.to_item().progress, expected);
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

   #[rstest]
   #[case::without_received_bytes(
      r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","options":{"allowMetered":true},"status":"idle"}"#,
      "receivedBytes"
   )]
   #[case::without_options(
      r#"{"url":"http://example.com/f.mp4","path":"/tmp/f.mp4","receivedBytes":0,"status":"idle"}"#,
      "options"
   )]
   fn test_deserialize_without_required_field_fails(
      #[case] json: &str,
      #[case] missing_field: &str,
   ) {
      let err = serde_json::from_str::<DownloadRecord>(json).unwrap_err();

      assert!(
         err.to_string()
            .contains(&format!("missing field `{missing_field}`")),
         "got: {err}"
      );
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
   fn test_download_action_response_new_expects_the_item_status() {
      // Not Idle, the status of sample_record, so a hardcoded Idle fails.
      let item = sample_record()
         .with_status(DownloadStatus::Paused)
         .to_item();

      let response = DownloadActionResponse::new(item);

      assert!(response.is_expected_status);
      assert_eq!(response.expected_status, DownloadStatus::Paused);
   }

   #[rstest]
   #[case::matching_status(DownloadStatus::Idle, true)]
   #[case::mismatched_status(DownloadStatus::InProgress, false)]
   fn test_with_expected_status_compares_the_item_status(
      #[case] expected_status: DownloadStatus,
      #[case] is_expected_status: bool,
   ) {
      let item = sample_record().to_item();

      let response = DownloadActionResponse::with_expected_status(item, expected_status);

      assert_eq!(response.is_expected_status, is_expected_status);
   }

   #[rstest]
   #[case::idle(DownloadStatus::Idle, "idle")]
   #[case::in_progress(DownloadStatus::InProgress, "inProgress")]
   #[case::paused(DownloadStatus::Paused, "paused")]
   #[case::failed(DownloadStatus::Failed, "failed")]
   #[case::canceled(DownloadStatus::Canceled, "canceled")]
   #[case::completed(DownloadStatus::Completed, "completed")]
   fn test_display_matches_the_serialized_form(
      #[case] status: DownloadStatus,
      #[case] spelling: &str,
   ) {
      // The invariant the `Display` impl exists under: one spelling of a status,
      // whether it reaches a reader through a log line or through JSON.
      //
      // The spelling is also the wire contract: Kotlin, Swift and TypeScript use these
      // strings, and the statuses mobile sends are decoded with them, so decoding is
      // checked as well as encoding.
      //
      // The arms below make adding a variant a compile error here. Whoever fixes
      // that error also has to add a #[case] for the new variant above — the
      // compiler cannot enforce that second step.
      match &status {
         DownloadStatus::Idle
         | DownloadStatus::InProgress
         | DownloadStatus::Failed
         | DownloadStatus::Paused
         | DownloadStatus::Canceled
         | DownloadStatus::Completed => {}
      }

      let json = format!("\"{spelling}\"");
      let decoded: DownloadStatus = serde_json::from_str(&json).unwrap();

      assert_eq!(status.to_string(), spelling);
      assert_eq!(serde_json::to_string(&status).unwrap(), json);
      assert_eq!(decoded, status);
   }
}
