use serde::de::DeserializeOwned;
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{AppHandle, Runtime};

use crate::models::*;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "org.silvermine.plugin.download";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_download);

pub fn init<R: Runtime, C: DeserializeOwned>(
   _app: &AppHandle<R>,
   _api: PluginApi<R, C>,
   user_agent: Option<String>,
   store_dir: Option<String>,
) -> crate::Result<Download<R>> {
   #[cfg(target_os = "android")]
   let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "DownloadPlugin")?;
   #[cfg(target_os = "ios")]
   let handle = _api.register_ios_plugin(init_plugin_download)?;

   let download = Download(handle);

   // Pushed as a command because the plugin config Tauri forwards to `load()` comes
   // from `tauri.conf.json`, which a builder value cannot ride. What keeps `configure`
   // unreachable from JS is the ACL, not its absence from `generate_handler!` — that
   // absence is what routes it to native. The invariant is that it never enters COMMANDS.
   //
   // Invoked unconditionally, even with nothing to set: both natives build their
   // download manager on first touch, and this is what touches it first — before
   // `load(webView)`, which Tauri defers until the webview exists. Skipping it would
   // hand that first touch to another caller, and on iOS delay the background
   // `URLSession` that restores in-flight downloads.
   //
   // A transport failure aborts launch deliberately: under `release_max_level_off`,
   // "log and continue" would ship the wrong agent or the wrong store silently.
   download.configure(user_agent, store_dir)?;

   Ok(download)
}

/// Access to the download APIs.
pub struct Download<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Download<R> {
   ///
   /// Pushes the builder's settings to the native plugin.
   ///
   /// Deliberately not public: it is only correct to call once, before any download
   /// exists. A later change would reach Android only for work enqueued after it and
   /// iOS only for newly created tasks, which is not a contract worth offering. The
   /// store directory is stricter still — both natives read it when they construct
   /// their store, so a later call could not move an already-open file.
   ///
   /// # Arguments
   /// - `user_agent` - The user agent sent with every download request, or `None` to
   ///   leave the platform's own default.
   /// - `store_dir` - The directory holding `downloads.json`, or `None` to leave the
   ///   platform's own default.
   ///
   fn configure(&self, user_agent: Option<String>, store_dir: Option<String>) -> crate::Result<()> {
      // A bare `invoke.resolve()` sends the literal `null` on both platforms, which
      // is what `()` decodes from. Anything else here would fail to deserialize.
      self
         .0
         .run_mobile_plugin(
            "configure",
            ConfigArgs {
               user_agent,
               store_dir,
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Initializes the API.
   /// Updates the state of any download operations which are still marked as "In Progress". This can occur if the
   /// application was suspended or terminated before a download was completed.
   ///
   pub fn init(&self) {
      // Not implemented on mobile platforms since initialization is handled by the plugin.
   }

   ///
   /// Lists all download operations.
   ///
   /// # Returns
   /// The list of download operations.
   pub fn list(&self) -> crate::Result<Vec<DownloadItem>> {
      // iOS and Android handle list responses differently:
      // - iOS `invoke.resolve()` accepts any Encodable, so it can return a bare JSON array.
      // - Android `invoke.resolve()` only accepts JSObject, so the array must be wrapped
      //   in an object (e.g. `{ "value": [...] }`).
      #[cfg(target_os = "ios")]
      {
         self.0.run_mobile_plugin("list", ()).map_err(Into::into)
      }
      #[cfg(target_os = "android")]
      {
         use serde::Deserialize;
         #[derive(Deserialize)]
         struct ListResponse {
            value: Vec<DownloadItem>,
         }
         let response: ListResponse = self.0.run_mobile_plugin("list", ())?;
         Ok(response.value)
      }
   }

   ///
   /// Gets a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation, or `None` if no download exists for the path.
   pub fn get(&self, path: &str) -> crate::Result<Option<DownloadItem>> {
      self
         .0
         .run_mobile_plugin(
            "get",
            PathArgs {
               path: path.to_string(),
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Creates a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   /// - `url` - The download URL for the resource.
   /// - `options` - Network policy persisted with the download.
   ///
   /// # Returns
   /// The download operation.
   pub fn create(
      &self,
      path: &str,
      url: &str,
      options: CreateOptions,
   ) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin(
            "create",
            CreateArgs {
               path: path.to_string(),
               url: url.to_string(),
               options,
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Starts a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn start(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin(
            "start",
            PathArgs {
               path: path.to_string(),
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Resumes a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn resume(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin(
            "resume",
            PathArgs {
               path: path.to_string(),
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Pauses a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn pause(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin(
            "pause",
            PathArgs {
               path: path.to_string(),
            },
         )
         .map_err(Into::into)
   }

   ///
   /// Cancels a download operation.
   ///
   /// # Arguments
   /// - `path` - The download path.
   ///
   /// # Returns
   /// The download operation.
   pub fn cancel(&self, path: &str) -> crate::Result<DownloadActionResponse> {
      self
         .0
         .run_mobile_plugin(
            "cancel",
            PathArgs {
               path: path.to_string(),
            },
         )
         .map_err(Into::into)
   }
}
