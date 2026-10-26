use std::path::PathBuf;

use tauri::{
   AppHandle, Manager, RunEvent, Runtime,
   plugin::{self, TauriPlugin},
};

#[cfg(desktop)]
use tauri::Emitter;
#[cfg(desktop)]
use tracing::warn;

mod commands;
mod error;
mod models;
mod scope;

/// The models a Rust caller sees: the return types of the [`DownloadExt::download`]
/// methods, and, on desktop, the payload of the `tauri-plugin-download:changed` event.
///
/// [`DownloadStatus`] is deliberately not `#[non_exhaustive]`: a new variant should
/// fail a caller's `match` rather than fall into a `_` arm.
pub use models::{CreateOptions, DownloadActionResponse, DownloadItem, DownloadStatus};

/// The error half of every [`DownloadExt::download`] method's return type, so a caller
/// can name `Result` in their own signatures rather than boxing. The desktop and
/// mobile `Error` enums have largely different variant sets — only `Io` is shared —
/// so `Result<T>` and `?`-propagation are portable, but an exhaustive `match` on
/// `Error` is not and has to be written under `#[cfg(desktop)]`/`#[cfg(mobile)]`.
pub use error::{Error, Result};

/// The concrete type [`DownloadExt::download`] hands back, so a caller can name it in
/// a signature or a struct field rather than only call methods on it.
#[cfg(desktop)]
pub use download_manager::DownloadManager;

#[cfg(desktop)]
use download_manager::DownloadManagerConfig;

#[cfg(mobile)]
mod mobile;

/// The mobile counterpart of [`DownloadManager`]: a handle to the native plugin,
/// carrying the runtime generic Tauri's mobile bridge requires.
#[cfg(mobile)]
pub use mobile::Download;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the download APIs.
///
/// The trait is split by platform because the return type differs:
/// - Desktop uses the Tauri-agnostic `DownloadManager` (Rust implementation).
/// - Mobile delegates to the native plugin via a `PluginHandle`, so the return type
///   carries the `R: Runtime` generic required by Tauri's mobile plugin bridge.
#[cfg(desktop)]
pub trait DownloadExt<R: Runtime> {
   fn download(&self) -> &DownloadManager;
}

#[cfg(mobile)]
pub trait DownloadExt<R: Runtime> {
   fn download(&self) -> &Download<R>;
}

/// Blanket impl over any `T: Manager<R>` (i.e. `App`, `AppHandle`, `Window`) so callers
/// can use `app.download()` without explicitly referencing the managed state.
#[cfg(desktop)]
impl<R: Runtime, T: Manager<R>> crate::DownloadExt<R> for T {
   fn download(&self) -> &DownloadManager {
      self.state::<DownloadManager>().inner()
   }
}

#[cfg(mobile)]
impl<R: Runtime, T: Manager<R>> crate::DownloadExt<R> for T {
   fn download(&self) -> &Download<R> {
      self.state::<Download<R>>().inner()
   }
}

/// Closure type for the deferred [`Builder::on_setup`] hook.
///
/// Boxed error rather than [`crate::Result`]: this crate's `Error` is a different type
/// on desktop and mobile, so neither can appear in a cross-platform signature. It is
/// also Tauri's own `setup` return type, so `app.path().app_data_dir()?` just works.
type OnSetupHook<R> = Box<
   dyn FnOnce(&AppHandle<R>, &mut SetupConfig) -> std::result::Result<(), Box<dyn std::error::Error>>
      + Send,
>;

/// Collects the settings that can only be resolved once the `app` instance exists.
///
/// Passed to the [`Builder::on_setup`] hook during plugin setup. Named for the payload
/// rather than the setting, as [`ConfigArgs`](crate::models) is, so a second
/// runtime-resolved option is a new method here rather than a second hook.
#[derive(Debug, Default)]
pub struct SetupConfig {
   store_dir: Option<PathBuf>,
   download_dirs: Vec<PathBuf>,
}

impl SetupConfig {
   /// Sets the directory holding the store's `downloads.json`.
   ///
   /// Must be absolute, and on mobile inside the app sandbox — which only
   /// `app.path()` can name, and the reason this is a hook rather than a setter.
   ///
   /// Required. Changing it leaves existing records where they are, invisible to the
   /// plugin.
   pub fn store_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
      self.store_dir = Some(dir.into());
      self
   }

   /// Sets the directories a download may be written to.
   ///
   /// Every path the webview passes to `create`, `start` or `resume` must name a
   /// location inside one of them, once `.` and `..` are resolved. Each must be
   /// absolute and not a filesystem root, and on mobile inside the app sandbox,
   /// which only `app.path()` can name. Several rather than one
   /// because destinations need not share a root, as `Documents` and `Library` do
   /// not on iOS.
   ///
   /// At least one is required. None may admit the store file, as a download named
   /// `downloads.json` would overwrite it: a directory that is, or contains,
   /// [`store_dir`](Self::store_dir), or that names the store file itself, fails
   /// plugin initialization. A `store_dir` above them all is fine.
   pub fn download_dirs<I, P>(&mut self, dirs: I) -> &mut Self
   where
      I: IntoIterator<Item = P>,
      P: Into<PathBuf>,
   {
      self.download_dirs = dirs.into_iter().map(Into::into).collect();
      self
   }
}

/// Plugin builder for configuring the download manager before initialization.
///
/// # Examples
///
/// ```no_run
/// use tauri::Manager;
///
/// tauri::Builder::default()
///    .plugin(
///       tauri_plugin_download::Builder::new()
///          .user_agent("my-app/1.0")
///          .on_setup(|app, config| {
///             config.store_dir(app.path().app_data_dir()?.join("store"));
///             config.download_dirs([app.path().app_data_dir()?.join("downloads")]);
///             Ok(())
///          })
///          .build(),
///    );
/// ```
pub struct Builder<R: Runtime> {
   user_agent: Option<String>,
   on_setup: Option<OnSetupHook<R>>,
}

/// Hand-written rather than derived: a boxed closure is not [`Debug`], and deriving
/// would additionally require `R: Debug`, which no runtime satisfies.
impl<R: Runtime> std::fmt::Debug for Builder<R> {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      f.debug_struct("Builder")
         .field("user_agent", &self.user_agent)
         .field("on_setup", &self.on_setup.is_some())
         .finish()
   }
}

/// Hand-written for the same reason `Debug` is: `#[derive(Default)]` would require
/// `R: Default`.
impl<R: Runtime> Default for Builder<R> {
   fn default() -> Self {
      Self {
         user_agent: None,
         on_setup: None,
      }
   }
}

impl<R: Runtime> Builder<R> {
   /// Creates a new builder, leaving every platform's own defaults in place.
   pub fn new() -> Self {
      Self::default()
   }

   /// Registers a hook that runs during plugin setup, once the `app` instance exists.
   ///
   /// The only way to set a store directory: legitimate directories come from
   /// `app.path()`, and the builder runs before `tauri::Builder::run` creates the app.
   /// On mobile it is also the only way to name a writable sandbox directory.
   ///
   /// Returning `Err` aborts startup, as an invalid [`user_agent`](Self::user_agent) does.
   pub fn on_setup(
      mut self,
      f: impl FnOnce(
         &AppHandle<R>,
         &mut SetupConfig,
      ) -> std::result::Result<(), Box<dyn std::error::Error>>
      + Send
      + 'static,
   ) -> Self {
      self.on_setup = Some(Box::new(f));
      self
   }

   /// Sets the `User-Agent` header sent with every download request.
   ///
   /// Applies on desktop, Android and iOS; unset, each keeps what its own HTTP stack
   /// sends. Must be printable ASCII or horizontal tab — the rule all three accept.
   /// Anything else fails plugin initialization rather than surfacing later as a
   /// failed download on one platform.
   pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
      self.user_agent = Some(user_agent.into());
      self
   }

   /// Builds the Tauri plugin with the configured settings.
   pub fn build(self) -> TauriPlugin<R> {
      let user_agent = self.user_agent;
      let on_setup = self.on_setup;

      plugin::Builder::new("download")
         .invoke_handler(tauri::generate_handler![
            commands::create,
            commands::list,
            commands::get,
            commands::start,
            commands::cancel,
            commands::pause,
            commands::resume,
            commands::is_native,
         ])
         .setup(move |app, _api| {
            // Runs first so the settings it resolves are validated below alongside the
            // ones set on the builder, rather than reaching a platform unchecked.
            let mut config = SetupConfig::default();
            if let Some(on_setup) = on_setup {
               on_setup(app, &mut config)?;
            }

            // Validated before either platform branch so an invalid value fails the
            // same way everywhere, rather than only where the transport rejects it.
            if let Some(ref user_agent) = user_agent {
               download_manager::validate_user_agent(user_agent)?;
            }

            // Both required, with no default: a directory the app did not choose is
            // not one it agreed to write into. Reported together, so one launch names
            // every missing setting rather than one per rebuild.
            let (store_dir, download_dirs) = match (config.store_dir, config.download_dirs) {
               (Some(store_dir), dirs) if !dirs.is_empty() => (store_dir, dirs),
               (store_dir, dirs) => {
                  let mut missing = Vec::new();

                  if store_dir.is_none() {
                     missing.push("`store_dir`");
                  }
                  if dirs.is_empty() {
                     missing.push("`download_dirs`");
                  }

                  return Err(format!("Set {} in `on_setup`", missing.join(" and ")).into());
               }
            };

            download_manager::validate_store_dir(&store_dir)?;

            app.manage(scope::DownloadScope::new(download_dirs, &store_dir)?);

            #[cfg(desktop)]
            {
               // Wire Tauri event emission as the on_changed callback.
               let app_handle = app.app_handle().clone();
               let manager = DownloadManager::new(
                  store_dir,
                  std::sync::Arc::new(move |item| {
                     if let Err(e) = app_handle.emit("tauri-plugin-download:changed", &item) {
                        warn!("Failed to emit change event: {}", e);
                     }
                  }),
                  DownloadManagerConfig { user_agent },
               );
               app.manage(manager);
            }

            #[cfg(mobile)]
            {
               // The bridge to native is JSON, so the directory has to be UTF-8. A path
               // that is not fails here rather than being dropped, which would leave the
               // store at the platform default with nothing having failed.
               let store_dir = store_dir
                  .to_str()
                  .ok_or_else(|| format!("Store directory is not valid UTF-8: {:?}", store_dir))?
                  .to_string();

               // Mobile download management is handled natively by the platform plugin.
               let download = mobile::init(app, _api, user_agent, Some(store_dir))?;
               app.manage(download);
            }

            Ok(())
         })
         .on_event(|_app_handle, event| {
            if let RunEvent::Ready = event {
               // Initialize the download plugin.
               #[cfg(desktop)]
               _app_handle.state::<DownloadManager>().init();
            }
         })
         .build()
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use rstest::rstest;

   /// A builder with both required directories set, for the cases that are about
   /// something else. A test needing its own hook sets both itself, since `on_setup`
   /// holds one closure.
   fn configured() -> Builder<tauri::test::MockRuntime> {
      Builder::new().on_setup(|app, config| {
         config.store_dir(app.path().app_data_dir()?.join("store"));
         config.download_dirs([app.path().app_data_dir()?.join("downloads")]);
         Ok(())
      })
   }

   // The hooks of the unusable configuration cases. Named functions rather than
   // closures inside the case attributes, whose arguments rustfmt leaves unformatted.
   fn with_a_relative_download_dir(
      app: &AppHandle<tauri::test::MockRuntime>,
      config: &mut SetupConfig,
   ) -> std::result::Result<(), Box<dyn std::error::Error>> {
      config.store_dir(app.path().app_data_dir()?.join("store"));
      config.download_dirs(["downloads"]);
      Ok(())
   }

   fn without_a_store_dir(
      app: &AppHandle<tauri::test::MockRuntime>,
      config: &mut SetupConfig,
   ) -> std::result::Result<(), Box<dyn std::error::Error>> {
      config.download_dirs([app.path().app_data_dir()?.join("downloads")]);
      Ok(())
   }

   fn without_download_dirs(
      app: &AppHandle<tauri::test::MockRuntime>,
      config: &mut SetupConfig,
   ) -> std::result::Result<(), Box<dyn std::error::Error>> {
      config.store_dir(app.path().app_data_dir()?.join("store"));
      Ok(())
   }

   fn with_a_relative_store_dir(
      app: &AppHandle<tauri::test::MockRuntime>,
      config: &mut SetupConfig,
   ) -> std::result::Result<(), Box<dyn std::error::Error>> {
      config.store_dir("downloads");
      config.download_dirs([app.path().app_data_dir()?.join("downloads")]);
      Ok(())
   }

   #[test]
   fn test_user_agent_setter_stores_the_value() {
      // Not tautological: the field is read once, in `build`, and a setter that
      // dropped its argument would leave every download on the platform default with
      // nothing failing.
      let builder = Builder::<tauri::test::MockRuntime>::new().user_agent("my-app/1.0");

      assert_eq!(builder.user_agent, Some("my-app/1.0".to_string()));
   }

   #[rstest]
   // The promise the README makes. `validate_user_agent` runs ahead of the
   // desktop/mobile split, so this resolves on any host without a mobile target.
   #[case::non_ascii_user_agent(configured().user_agent("Caf\u{e9}/1.0"))]
   // A relative bound follows the working directory, so it bounds nothing stable.
   // It fails startup rather than silently permitting a moving set of paths.
   #[case::relative_download_dir(Builder::new().on_setup(with_a_relative_download_dir))]
   // Neither directory has a default. Without this, an app that named only one
   // would start with the other resolved to somewhere it never chose.
   #[case::missing_store_dir(Builder::new().on_setup(without_a_store_dir))]
   #[case::missing_download_dir(Builder::new().on_setup(without_download_dirs))]
   // The mirror of the `non_ascii_user_agent` case. `validate_store_dir` runs ahead of
   // the desktop/mobile split, so this resolves on any host without a mobile target.
   #[case::relative_store_dir(Builder::new().on_setup(with_a_relative_store_dir))]
   fn test_an_unusable_configuration_fails_plugin_initialization(
      #[case] builder: Builder<tauri::test::MockRuntime>,
   ) {
      let app = tauri::test::mock_builder()
         .plugin(builder.build())
         .build(tauri::test::mock_context(tauri::test::noop_assets()));

      assert!(matches!(app, Err(tauri::Error::PluginInitialization(_, _))));
   }

   #[test]
   fn test_a_valid_user_agent_initializes() {
      // Pairs with the `non_ascii_user_agent` case above: without it, any unrelated setup
      // failure would satisfy that assertion.
      let app = tauri::test::mock_builder()
         .plugin(configured().user_agent("my-app/1.0").build())
         .build(tauri::test::mock_context(tauri::test::noop_assets()));

      assert!(app.is_ok());
   }

   #[test]
   fn test_store_dir_setter_stores_the_value() {
      // Same reasoning as the user agent setter above: the field is read once, in
      // `build`, so a setter that dropped its argument would silently leave the store
      // at the platform default.
      let mut config = SetupConfig::default();

      config.store_dir("/var/lib/myapp");

      assert_eq!(config.store_dir, Some(PathBuf::from("/var/lib/myapp")));
   }

   #[test]
   fn test_the_setup_hook_runs_during_plugin_initialization() {
      // The hook exists to be run with an app handle that only exists at setup time.
      // Without this, `on_setup` could store a closure that is never called and every
      // other test here would still pass.
      let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
      let flag = ran.clone();

      let app = tauri::test::mock_builder()
         .plugin(
            Builder::new()
               .on_setup(move |app, config| {
                  flag.store(true, std::sync::atomic::Ordering::SeqCst);
                  config.store_dir(app.path().app_data_dir()?.join("store"));
                  config.download_dirs([app.path().app_data_dir()?.join("downloads")]);
                  Ok(())
               })
               .build(),
         )
         .build(tauri::test::mock_context(tauri::test::noop_assets()));

      assert!(app.is_ok());
      assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
   }

   #[test]
   fn test_the_configured_download_dirs_bound_the_commands() {
      // The setter and the validation both passing still leaves the scope unmanaged,
      // in which case every command would panic on its missing state. This is the
      // test that the configured directory actually reaches the commands.
      let app = tauri::test::mock_builder()
         .plugin(
            Builder::new()
               .on_setup(|app, config| {
                  config.store_dir(app.path().app_data_dir()?.join("store"));
                  config.download_dirs(["/data/downloads", "/media/library"]);
                  Ok(())
               })
               .build(),
         )
         .build(tauri::test::mock_context(tauri::test::noop_assets()))
         .unwrap();

      let scope = app.state::<scope::DownloadScope>();

      assert!(scope.check("/data/downloads/file.zip").is_ok());
      assert!(scope.check("/media/library/file.mp4").is_ok());
      assert!(scope.check("/data/downloads/../../etc/passwd").is_err());
      assert!(scope.check("/etc/passwd").is_err());
   }

   #[test]
   fn test_neither_directory_set_names_both_in_one_error() {
      // The plain `Builder::new().build()` case, which used to start on defaults.
      // Both are named at once, so configuring the plugin takes one launch rather
      // than one per missing setting.
      let app = tauri::test::mock_builder()
         .plugin(Builder::new().build())
         .build(tauri::test::mock_context(tauri::test::noop_assets()));

      let error = app.unwrap_err().to_string();

      assert!(
         error.contains("Set `store_dir` and `download_dirs` in `on_setup`"),
         "unexpected error: {}",
         error
      );
   }

   #[test]
   fn test_an_error_from_the_setup_hook_fails_plugin_initialization() {
      // The documented contract: a caller whose `app.path()` lookup fails aborts
      // startup rather than silently falling back to a directory they did not choose.
      let app = tauri::test::mock_builder()
         .plugin(
            Builder::new()
               .on_setup(|_app, _config| Err("no directory for you".into()))
               .build(),
         )
         .build(tauri::test::mock_context(tauri::test::noop_assets()));

      let error = app.unwrap_err().to_string();

      assert!(
         error.contains("no directory for you"),
         "unexpected error: {}",
         error
      );
   }

   #[cfg(desktop)]
   #[test]
   fn test_the_store_is_persisted_in_the_configured_directory() {
      // The feature itself, end to end through the plugin: creating a download has to
      // write `downloads.json` into the configured directory. Asserting on the setter
      // alone would pass even if `build` ignored the value.
      let dir = tempfile::tempdir().unwrap();
      let store_dir = dir.path().join("configured");
      let expected = store_dir.join("downloads.json");

      let configured = store_dir.clone();
      let downloads = dir.path().join("downloads");
      let app = tauri::test::mock_builder()
         .plugin(
            Builder::new()
               .on_setup(move |_app, config| {
                  config.store_dir(configured.clone());
                  config.download_dirs([downloads.clone()]);
                  Ok(())
               })
               .build(),
         )
         .build(tauri::test::mock_context(tauri::test::noop_assets()))
         .unwrap();

      let download: DownloadItem = app
         .download()
         .create(
            dir.path().join("downloads/file.mp4").to_str().unwrap(),
            "https://example.com/file.mp4",
         )
         .unwrap()
         .download;

      assert_eq!(download.status, DownloadStatus::Idle);
      assert!(expected.exists(), "store not written to {:?}", expected);
   }

   #[test]
   fn test_a_changed_event_payload_decodes_into_the_exported_models() {
      // What issue #42 is about: a Rust caller decodes the event payload into
      // `DownloadItem` and matches on `DownloadStatus`, instead of comparing the
      // status against a string whose casing depends on which side produced it.
      let payload = r#"{"url":"https://example.com/f.mp4","path":"/tmp/f.mp4","options":{"allowMetered":true},"receivedBytes":500,"totalBytes":1000,"progress":50.0,"status":"inProgress"}"#;

      let item: DownloadItem = serde_json::from_str(payload).unwrap();

      assert!(matches!(item.status, DownloadStatus::InProgress));
   }
}
