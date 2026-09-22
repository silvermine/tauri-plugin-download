// Shared with `download-manager` on every platform rather than mirrored per platform:
// a second definition of these shapes could drift unnoticed.
pub use download_manager::{CreateOptions, DownloadActionResponse, DownloadItem, DownloadStatus};

/// Wire form of [`CreateOptions`] for the `create` command. TypeScript declares
/// `allowMetered` optional, so `create(url, {})` must mean "unstated" rather than
/// fail; holding that tolerance here lets the persisted [`CreateOptions`] require
/// the value. Unknown keys are ignored, not rejected.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOptionsArgs {
   pub allow_metered: Option<bool>,
}

impl From<CreateOptionsArgs> for CreateOptions {
   fn from(arg: CreateOptionsArgs) -> Self {
      Self {
         allow_metered: arg.allow_metered.unwrap_or(Self::default().allow_metered),
      }
   }
}

// Mobile model types (iOS, Android)
#[cfg(mobile)]
mod mobile_types {
   use serde::Serialize;

   use super::CreateOptions;

   #[derive(Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct PathArgs {
      pub path: String,
   }

   /// Settings pushed to the native plugin once at startup.
   ///
   /// Named for the payload rather than the setting, so a second builder option is a
   /// new field here rather than a second command mirrored across Kotlin and Swift.
   ///
   /// Every field is optional: the command is invoked unconditionally, so absent means
   /// "keep the platform default", not a malformed call.
   #[derive(Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct ConfigArgs {
      pub user_agent: Option<String>,
      pub store_dir: Option<String>,
   }

   #[derive(Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct CreateArgs {
      pub path: String,
      pub url: String,
      pub options: CreateOptions,
   }
}

#[cfg(mobile)]
pub use mobile_types::{ConfigArgs, CreateArgs, PathArgs};

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn test_unstated_policy_takes_the_api_default() {
      // `create(url, {})` from TypeScript decodes to this: serde reads an absent
      // Option as None, so the default is resolved here rather than by tolerating
      // a missing field on the persisted `CreateOptions`.
      let arg = CreateOptionsArgs {
         allow_metered: None,
      };

      assert_eq!(
         CreateOptions::from(arg).allow_metered,
         CreateOptions::default().allow_metered
      );
   }

   #[test]
   fn test_stated_policy_is_carried_through() {
      let arg = CreateOptionsArgs {
         allow_metered: Some(false),
      };

      assert!(!CreateOptions::from(arg).allow_metered);
   }

   #[test]
   fn test_empty_json_object_resolves_to_the_api_default() {
      // The `create(url, {})` case the struct's doc comment describes, decoded
      // through serde rather than constructed, so it holds at the wire.
      let arg: CreateOptionsArgs = serde_json::from_str("{}").unwrap();

      assert_eq!(arg.allow_metered, None);
      assert_eq!(
         CreateOptions::from(arg).allow_metered,
         CreateOptions::default().allow_metered
      );
   }

   #[test]
   fn test_json_false_is_carried_through_to_the_resolved_options() {
      let arg: CreateOptionsArgs = serde_json::from_str(r#"{"allowMetered":false}"#).unwrap();

      assert_eq!(arg.allow_metered, Some(false));
      assert!(!CreateOptions::from(arg).allow_metered);
   }

   #[test]
   fn test_json_true_is_carried_through_to_the_resolved_options() {
      let arg: CreateOptionsArgs = serde_json::from_str(r#"{"allowMetered":true}"#).unwrap();

      assert_eq!(arg.allow_metered, Some(true));
      assert!(CreateOptions::from(arg).allow_metered);
   }

   #[test]
   fn test_json_null_is_treated_the_same_as_the_field_being_absent() {
      // Null and absent are deliberately the same under "unstated" semantics.
      let arg: CreateOptionsArgs = serde_json::from_str(r#"{"allowMetered":null}"#).unwrap();

      assert_eq!(arg.allow_metered, None);
      assert_eq!(
         CreateOptions::from(arg).allow_metered,
         CreateOptions::default().allow_metered
      );
   }

   #[test]
   fn test_unrecognised_key_is_ignored_and_resolves_to_the_default() {
      // Characterisation test, not a design goal: `guest-js/actions.ts` forwards
      // the caller's options object verbatim, so `deny_unknown_fields` would break
      // a caller whose variable is typed wider than the options shape.
      let arg: CreateOptionsArgs = serde_json::from_str(r#"{"allowmetered":false}"#).unwrap();

      assert_eq!(arg.allow_metered, None);
      assert_eq!(
         CreateOptions::from(arg).allow_metered,
         CreateOptions::default().allow_metered
      );
   }
}

/// Internal native response marker. Tauri 2's rejection decoder drops custom fields,
/// so native errors travel as data until the Rust command rejects with the full value.
#[cfg(any(mobile, test))]
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum NativeResponse<T> {
   Failure {
      #[serde(rename = "__downloadError")]
      error: download_manager::DownloadFailure,
   },
   Value(T),
}

#[cfg(any(mobile, test))]
impl<T> NativeResponse<T> {
   /// Unwraps the internal transport without losing error metadata.
   pub(crate) fn into_result(self) -> Result<T, download_manager::DownloadFailure> {
      match self {
         Self::Failure { error } => Err(error),
         Self::Value(value) => Ok(value),
      }
   }
}

#[cfg(test)]
mod native_response_tests {
   use super::NativeResponse;

   #[test]
   fn preserves_native_error_metadata_and_successful_null() {
      let response: NativeResponse<()> = serde_json::from_value(serde_json::json!({
         "__downloadError": {
            "code": "http", "message": "HTTP 429", "retryability": "transient", "httpStatus": 429
         }
      }))
      .unwrap();
      let error = response.into_result().unwrap_err();
      assert_eq!(error.http_status, Some(429));
      assert_eq!(
         error.retryability,
         download_manager::Retryability::Transient
      );
      let success: NativeResponse<()> = serde_json::from_str("null").unwrap();
      assert!(success.into_result().is_ok());
   }
}
