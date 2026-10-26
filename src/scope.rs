use std::path::{Component, Path, PathBuf};

/// The directories a download may be written to.
///
/// Held as managed state and checked in the command layer, so one rule in Rust bounds
/// desktop, Android and iOS alike. Several rather than one because destinations need
/// not share a root: on iOS the sandbox separates `Documents` from `Library`.
#[derive(Debug)]
pub(crate) struct DownloadScope {
   roots: Vec<PathBuf>,
}

impl DownloadScope {
   /// Creates a scope bounded by `roots`, each of which must be absolute and none of
   /// which may admit the store file `store_dir` holds.
   ///
   /// Roots are normalized, not canonicalized, to match what [`check`](Self::check)
   /// does to the caller's path. Resolving one side only would reject `/var` against
   /// a root resolved to `/private/var`.
   pub(crate) fn new<I, P>(roots: I, store_dir: &Path) -> crate::Result<Self>
   where
      I: IntoIterator<Item = P>,
      P: Into<PathBuf>,
   {
      let roots: Vec<PathBuf> = roots.into_iter().map(Into::into).collect();

      // No root admits nothing, which is unusable rather than merely narrow.
      if roots.is_empty() {
         return Err(reject("at least one download directory is required"));
      }

      // A relative root follows the working directory, and an empty one is not
      // absolute either, so one check covers both.
      for root in &roots {
         if !root.is_absolute() {
            return Err(reject("download directory must be absolute"));
         }
      }

      let roots: Vec<PathBuf> = roots.iter().map(|root| normalize(root)).collect();

      // A filesystem root has no parent and is a prefix of every absolute path, so
      // naming one leaves the plugin looking configured while bounding nothing.
      // Checked after normalizing, which is what turns `/data/..` into one.
      if roots.iter().any(|root| root.parent().is_none()) {
         return Err(reject("download directory cannot be a filesystem root"));
      }

      // A root that admits the store file puts the plugin's own history in reach of a
      // download named for it. Compared against the file, not the directory holding
      // it, so a root naming the file itself is caught too. A root below the store
      // directory is fine: the file then sits above it, where `check` refuses it.
      let store_file = normalize(store_dir).join(download_manager::STORE_FILE_NAME);

      if roots.iter().any(|root| store_file.starts_with(root)) {
         return Err(reject("download directory cannot contain the store"));
      }

      Ok(Self { roots })
   }

   /// Checks that `path`, as received from the webview, is inside the scope.
   ///
   /// Structural validation runs first so a malformed path keeps the message every
   /// platform already sends for it, rather than being reported as out of scope.
   pub(crate) fn check(&self, path: &str) -> crate::Result<()> {
      download_manager::validate_path(path)?;

      let normalized = normalize(Path::new(path));

      if !self.roots.iter().any(|root| normalized.starts_with(root)) {
         return Err(reject("path must be inside a download directory"));
      }

      Ok(())
   }
}

/// Builds the error every rejection here reports.
///
/// The conversion is an identity on desktop and a real one on mobile, so one
/// implementation reports identically on both.
#[allow(clippy::useless_conversion)]
fn reject(message: &str) -> crate::Error {
   download_manager::Error::Path(message.to_string()).into()
}

/// Resolves `.` and `..` in a path without touching the file system.
///
/// The target of a download does not exist yet, so [`std::fs::canonicalize`] cannot be
/// used on it. A `..` at the root is dropped, so no run of them escapes above it, and
/// a symbolic link pointing out of the scope is not resolved.
fn normalize(path: &Path) -> PathBuf {
   let mut normalized = PathBuf::new();

   for component in path.components() {
      match component {
         Component::CurDir => {}
         Component::ParentDir => {
            normalized.pop();
         }
         component => normalized.push(component),
      }
   }

   normalized
}

#[cfg(test)]
mod tests {
   use super::*;

   fn scope() -> DownloadScope {
      DownloadScope::new(["/data/downloads"], store_dir()).unwrap()
   }

   /// A store directory outside every root these tests use, so a case about something
   /// else is not decided by the overlap check.
   fn store_dir() -> &'static Path {
      Path::new("/state/store")
   }

   #[test]
   fn test_path_inside_the_scope_is_accepted() {
      assert!(scope().check("/data/downloads/file.zip").is_ok());
      assert!(scope().check("/data/downloads/nested/dir/file.zip").is_ok());
   }

   #[test]
   fn test_path_outside_the_scope_is_rejected() {
      assert!(scope().check("/etc/passwd").is_err());
      assert!(scope().check("/data/other/file.zip").is_err());
   }

   #[test]
   fn test_parent_traversal_out_of_the_scope_is_rejected() {
      // Resolved before the comparison, so a path is judged by where it lands.
      assert!(scope().check("/data/downloads/../../etc/passwd").is_err());
      assert!(
         scope()
            .check("/data/downloads/sub/../../other/f.zip")
            .is_err()
      );
   }

   #[test]
   fn test_parent_traversal_inside_the_scope_is_accepted() {
      // `..` is not itself suspicious if the path still lands inside the scope.
      assert!(scope().check("/data/downloads/sub/../file.zip").is_ok());
      assert!(scope().check("/data/downloads/./file.zip").is_ok());
   }

   #[test]
   fn test_sibling_directory_with_a_shared_prefix_is_rejected() {
      // A string prefix match, and a different directory. Comparing components
      // rather than characters is what separates them.
      assert!(scope().check("/data/downloads-evil/file.zip").is_err());
      assert!(scope().check("/data/downloadsevil/file.zip").is_err());
   }

   #[test]
   fn test_traversal_above_the_root_cannot_escape() {
      // Each `..` at the root is dropped rather than wrapping around to somewhere
      // the comparison would accept.
      assert!(scope().check("/../../../etc/passwd").is_err());
   }

   #[test]
   fn test_structural_errors_keep_their_own_message() {
      // A malformed path keeps the message every platform already sends for it.
      let message = |p: &str| scope().check(p).unwrap_err().to_string();

      assert_eq!(message(""), "Path Error: path cannot be empty");
      assert_eq!(message("file.txt"), "Path Error: path must be absolute");
      assert_eq!(message("/"), "Path Error: path must have a filename");
   }

   #[test]
   fn test_out_of_scope_message() {
      let error = scope().check("/etc/passwd").unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: path must be inside a download directory"
      );
   }

   #[test]
   fn test_a_root_that_bounds_nothing_is_rejected() {
      // Both fail here rather than at the first download.
      assert!(DownloadScope::new(["downloads"], store_dir()).is_err());
      assert!(DownloadScope::new([""], store_dir()).is_err());

      // One bad root among good ones fails too, rather than being dropped and
      // leaving the app bounded by fewer directories than it named.
      assert!(DownloadScope::new(["/data/downloads", "relative"], store_dir()).is_err());

      let error = DownloadScope::new(["downloads"], store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory must be absolute"
      );
   }

   #[test]
   fn test_a_filesystem_root_is_rejected() {
      // The configuration that would look set up and bound nothing: every absolute
      // path is inside the root, so the check would admit what it exists to refuse.
      let error = DownloadScope::new(["/"], store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot be a filesystem root"
      );

      // Reached by `..` as well as written directly, which is why the check runs on
      // the normalized root rather than the one the caller passed.
      let error = DownloadScope::new(["/data/.."], store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot be a filesystem root"
      );

      // One root short of the top is a narrow scope, not a missing one.
      assert!(DownloadScope::new(["/data"], store_dir()).is_ok());

      // A good root does not rescue a bad one.
      let error = DownloadScope::new(["/data/downloads", "/"], store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot be a filesystem root"
      );
   }

   #[test]
   fn test_no_roots_at_all_is_rejected() {
      let error = DownloadScope::new(Vec::<PathBuf>::new(), store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: at least one download directory is required"
      );
   }

   #[test]
   fn test_each_root_bounds_its_own_subtree() {
      // The iOS case: two roots the sandbox keeps apart.
      let scope =
         DownloadScope::new(["/app/Documents/data", "/app/Library/Media"], store_dir()).unwrap();

      assert!(scope.check("/app/Documents/data/pubs/f.epub").is_ok());
      assert!(scope.check("/app/Library/Media/Videos/f.mp4").is_ok());
      assert!(scope.check("/app/Library/Preferences/f.plist").is_err());
      assert!(scope.check("/app/f.zip").is_err());

      // A `..` cannot hop from one root into anything outside both.
      assert!(
         scope
            .check("/app/Documents/data/../../Library/f.plist")
            .is_err()
      );
   }

   #[test]
   fn test_root_is_normalized_before_it_is_compared() {
      // Both sides are normalized, so a root written with `.` or `..` bounds the
      // same paths as one written plainly.
      let scope = DownloadScope::new(["/data/./media/../downloads"], store_dir()).unwrap();

      assert!(scope.check("/data/downloads/file.zip").is_ok());
      assert!(scope.check("/data/media/file.zip").is_err());
   }

   #[test]
   fn test_a_root_holding_the_store_directory_is_rejected() {
      // `<store_dir>/downloads.json` would be in scope, so a download named for it
      // would overwrite the record of every other one. Fails at startup rather than
      // waiting for a webview to ask for that name.
      let error = DownloadScope::new(["/data"], Path::new("/data")).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot contain the store"
      );

      // Nested is the same exposure as equal, at any depth.
      assert!(DownloadScope::new(["/data"], Path::new("/data/store")).is_err());
      assert!(DownloadScope::new(["/data"], Path::new("/data/a/b/store")).is_err());

      // One offending root among good ones fails too.
      assert!(DownloadScope::new(["/media", "/data"], Path::new("/data/store")).is_err());
   }

   #[test]
   fn test_a_root_naming_the_store_file_is_rejected() {
      // The one way a root below the store directory still reaches the store: it names
      // the file itself, which `check` then admits because a path may equal a root.
      // Comparing against the directory alone would let this through.
      assert!(DownloadScope::new(["/data/downloads.json"], Path::new("/data")).is_err());
   }

   #[test]
   fn test_a_store_directory_outside_every_root_is_accepted() {
      assert!(DownloadScope::new(["/data/downloads"], Path::new("/state/store")).is_ok());

      // A shared prefix is not containment, for the same reason it is not in `check`.
      assert!(DownloadScope::new(["/data/downloads"], Path::new("/data/downloads-store")).is_ok());
   }

   #[test]
   fn test_a_root_inside_the_store_directory_is_accepted() {
      // The natural layout of `store_dir` at the app data directory with the downloads
      // beneath it. `downloads.json` sits above the root, where `check` refuses it.
      let scope = DownloadScope::new(["/data/downloads"], Path::new("/data")).unwrap();

      assert!(scope.check("/data/downloads/file.zip").is_ok());
      assert!(scope.check("/data/downloads.json").is_err());
   }

   #[test]
   fn test_the_store_directory_is_normalized_before_it_is_compared() {
      // Both sides are normalized, so a store directory written with `..` is judged by
      // where it lands rather than by how it reads.
      assert!(
         DownloadScope::new(["/data/downloads"], Path::new("/state/../data/downloads")).is_err()
      );
      assert!(
         DownloadScope::new(["/data/./downloads"], Path::new("/data/downloads/sub/..")).is_err()
      );
   }
}
