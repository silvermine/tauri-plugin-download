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
   use rstest::rstest;

   fn scope() -> DownloadScope {
      DownloadScope::new(["/data/downloads"], store_dir()).unwrap()
   }

   /// The iOS case: two roots the sandbox keeps apart.
   fn two_root_scope() -> DownloadScope {
      DownloadScope::new(["/app/Documents/data", "/app/Library/Media"], store_dir()).unwrap()
   }

   /// A store directory outside every root these tests use, so a case about something
   /// else is not decided by the overlap check.
   fn store_dir() -> &'static Path {
      Path::new("/state/store")
   }

   #[rstest]
   #[case::file_in_the_root("/data/downloads/file.zip")]
   #[case::file_in_a_nested_directory("/data/downloads/nested/dir/file.zip")]
   // `..` is not itself suspicious if the path still lands inside the scope.
   #[case::parent_traversal_landing_inside("/data/downloads/sub/../file.zip")]
   #[case::current_directory_component("/data/downloads/./file.zip")]
   // A `..` at the filesystem root is dropped, as the OS resolves it, rather than kept
   // in front of a path that is otherwise inside.
   #[case::parent_traversal_above_the_filesystem_root("/../data/downloads/file.zip")]
   fn test_path_inside_the_scope_is_accepted(#[case] path: &str) {
      assert!(scope().check(path).is_ok());
   }

   #[rstest]
   #[case::system_file("/etc/passwd")]
   #[case::sibling_directory("/data/other/file.zip")]
   // Resolved before the comparison, so a path is judged by where it lands.
   #[case::parent_traversal_from_the_root("/data/downloads/../../etc/passwd")]
   #[case::parent_traversal_from_a_subdirectory("/data/downloads/sub/../../other/f.zip")]
   // A string prefix match, and a different directory. Comparing components
   // rather than characters is what separates them.
   #[case::hyphenated_sibling_sharing_the_prefix("/data/downloads-evil/file.zip")]
   #[case::sibling_sharing_the_prefix("/data/downloadsevil/file.zip")]
   // Outside the root wherever a `..` above the filesystem root leads, so the accepted
   // parent_traversal_above_the_filesystem_root is the case that pins the drop.
   #[case::traversal_above_the_filesystem_root("/../../../etc/passwd")]
   fn test_path_outside_the_scope_is_rejected(#[case] path: &str) {
      let error = scope().check(path).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: path must be inside a download directory"
      );
   }

   // A malformed path keeps the message every platform already sends for it.
   #[rstest]
   #[case::empty("", "Path Error: path cannot be empty")]
   #[case::relative("file.txt", "Path Error: path must be absolute")]
   #[case::root_without_a_filename("/", "Path Error: path must have a filename")]
   fn test_structural_errors_keep_their_own_message(#[case] path: &str, #[case] expected: &str) {
      assert_eq!(scope().check(path).unwrap_err().to_string(), expected);
   }

   // Fails here rather than at the first download.
   #[rstest]
   #[case::relative_root(&["downloads"])]
   #[case::empty_root(&[""])]
   // One bad root among good ones fails too, rather than being dropped and
   // leaving the app bounded by fewer directories than it named.
   #[case::relative_root_beside_an_absolute_one(&["/data/downloads", "relative"])]
   fn test_a_root_that_bounds_nothing_is_rejected(#[case] roots: &[&str]) {
      let error = DownloadScope::new(roots, store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory must be absolute"
      );
   }

   // The configuration that would look set up and bound nothing: every absolute
   // path is inside the root, so the check would admit what it exists to refuse.
   #[rstest]
   #[case::filesystem_root(&["/"])]
   // Reached by `..` as well as written directly, which is why the check runs on
   // the normalized root rather than the one the caller passed.
   #[case::parent_traversal_to_the_filesystem_root(&["/data/.."])]
   // A good root does not rescue a bad one.
   #[case::filesystem_root_beside_a_good_one(&["/data/downloads", "/"])]
   fn test_a_filesystem_root_is_rejected(#[case] roots: &[&str]) {
      let error = DownloadScope::new(roots, store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot be a filesystem root"
      );
   }

   #[test]
   fn test_a_root_one_level_below_the_filesystem_root_is_accepted() {
      // One root short of the top is a narrow scope, not a missing one.
      assert!(DownloadScope::new(["/data"], store_dir()).is_ok());
   }

   #[test]
   fn test_no_roots_at_all_is_rejected() {
      let error = DownloadScope::new(Vec::<PathBuf>::new(), store_dir()).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: at least one download directory is required"
      );
   }

   #[rstest]
   #[case::file_under_the_documents_root("/app/Documents/data/pubs/f.epub")]
   #[case::file_under_the_library_root("/app/Library/Media/Videos/f.mp4")]
   fn test_each_root_admits_its_own_subtree(#[case] path: &str) {
      assert!(two_root_scope().check(path).is_ok());
   }

   #[rstest]
   #[case::sibling_of_a_root("/app/Library/Preferences/f.plist")]
   #[case::file_in_the_common_ancestor("/app/f.zip")]
   // A `..` cannot hop from one root into anything outside both.
   #[case::traversal_from_one_root_to_outside_both("/app/Documents/data/../../Library/f.plist")]
   fn test_a_path_outside_every_root_is_rejected(#[case] path: &str) {
      let error = two_root_scope().check(path).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: path must be inside a download directory"
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

   // `<store_dir>/downloads.json` would be in scope, so a download named for it
   // would overwrite the record of every other one. Fails at startup rather than
   // waiting for a webview to ask for that name.
   #[rstest]
   #[case::store_directory_equal_to_the_root(&["/data"], "/data")]
   // Nested is the same exposure as equal, at any depth.
   #[case::store_directory_nested_in_the_root(&["/data"], "/data/store")]
   #[case::store_directory_deeply_nested_in_the_root(&["/data"], "/data/a/b/store")]
   // One offending root among good ones fails too.
   #[case::offending_root_beside_a_good_one(&["/media", "/data"], "/data/store")]
   // The one way a root below the store directory still reaches the store: it names
   // the file itself, which `check` then admits because a path may equal a root.
   // Comparing against the directory alone would let this through.
   #[case::root_naming_the_store_file(&["/data/downloads.json"], "/data")]
   // Both sides are normalized, so a root or a store directory written with `..` is
   // judged by where it lands rather than by how it reads.
   #[case::root_written_with_parent_traversal(&["/data/media/.."], "/data/store")]
   #[case::store_directory_written_with_parent_traversal(
      &["/data/downloads"],
      "/state/../data/downloads"
   )]
   // Left unresolved, a trailing `..` still puts the file under every directory root
   // that holds where it lands, so in this table only a root naming the file tells
   // the two apart.
   #[case::store_directory_ending_in_parent_traversal(&["/data/downloads.json"], "/data/sub/..")]
   // Lands on the root, so its file is under the root whether or not the `..` is resolved.
   #[case::store_directory_ending_in_parent_traversal_onto_the_root(
      &["/data/./downloads"],
      "/data/downloads/sub/.."
   )]
   fn test_a_root_admitting_the_store_file_is_rejected(
      #[case] roots: &[&str],
      #[case] store: &str,
   ) {
      let error = DownloadScope::new(roots, Path::new(store)).unwrap_err();

      assert_eq!(
         error.to_string(),
         "Path Error: download directory cannot contain the store"
      );
   }

   #[rstest]
   #[case::unrelated_directory("/state/store")]
   // A shared prefix is not containment, for the same reason it is not in `check`.
   #[case::hyphenated_sibling_sharing_the_prefix("/data/downloads-store")]
   // The layout of `test_a_root_inside_the_store_directory_is_accepted`, written with
   // `..`. Left unresolved, its file would read as inside the root and fail a valid setup.
   #[case::parent_of_the_root_written_with_parent_traversal("/data/downloads/..")]
   fn test_a_store_directory_outside_every_root_is_accepted(#[case] store: &str) {
      assert!(DownloadScope::new(["/data/downloads"], Path::new(store)).is_ok());
   }

   #[test]
   fn test_a_root_inside_the_store_directory_is_accepted() {
      // The natural layout of `store_dir` at the app data directory with the downloads
      // beneath it. `downloads.json` sits above the root, where `check` refuses it.
      let scope = DownloadScope::new(["/data/downloads"], Path::new("/data")).unwrap();

      assert!(scope.check("/data/downloads/file.zip").is_ok());
      assert!(scope.check("/data/downloads.json").is_err());
   }
}
