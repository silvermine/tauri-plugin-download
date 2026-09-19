use std::path::Path;

use crate::Error;

/// Validates a download path.
///
/// Checks that the path:
/// - Is not empty
/// - Is an absolute path
/// - Has a filename component
pub fn path(path: &str) -> crate::Result<()> {
   if path.is_empty() {
      return Err(Error::Path("path cannot be empty".to_string()));
   }

   let p = Path::new(path);

   if !p.is_absolute() {
      return Err(Error::Path("path must be absolute".to_string()));
   }

   if p.file_name().is_none() {
      return Err(Error::Path("path must have a filename".to_string()));
   }

   Ok(())
}

/// Validates a download store directory.
///
/// Checks that the directory:
/// - Is not empty
/// - Is an absolute path
///
/// Structural only, like [`path`]: no existence or writability check. The store creates
/// its parent on save, and a directory writable now can stop being writable before the
/// first write — a check here would report a state it cannot promise still holds.
///
/// No filename check, unlike [`path`]: every platform appends its own `downloads.json`,
/// so a root directory is legal.
pub fn store_dir(dir: &Path) -> crate::Result<()> {
   if dir.as_os_str().is_empty() {
      return Err(Error::Path("store directory cannot be empty".to_string()));
   }

   if !dir.is_absolute() {
      return Err(Error::Path("store directory must be absolute".to_string()));
   }

   Ok(())
}

/// Validates a download URL.
///
/// Checks that the URL:
/// - Is not empty
/// - Has a valid scheme (http or https)
/// - Has a valid host
pub fn url(url: &str) -> crate::Result<()> {
   if url.is_empty() {
      return Err(Error::Url("URL cannot be empty".to_string()));
   }

   // Parse and validate URL structure
   let parsed = url::Url::parse(url).map_err(|_| Error::Url(format!("Invalid URL: {}", url)))?;

   // Check scheme
   match parsed.scheme() {
      "http" | "https" => {}
      scheme => {
         return Err(Error::Url(format!(
            "Invalid URL scheme '{}': must be http or https",
            scheme
         )));
      }
   }

   // Check host
   if parsed.host().is_none() {
      return Err(Error::Url("URL must have a host".to_string()));
   }

   Ok(())
}

/// Validates an HTTP user agent.
///
/// Accepts printable ASCII and horizontal tab — what all three transports accept, and
/// narrower than reqwest's own `HeaderValue` rule, which admits the UTF-8 bytes of a
/// non-ASCII character only for OkHttp to reject them on every Android download.
///
/// No emptiness check, unlike [`path`] and [`url`]: an empty user agent is legal HTTP,
/// and refusing it would be this crate's policy, not the specification's.
pub fn user_agent(user_agent: &str) -> crate::Result<()> {
   let invalid = user_agent
      .chars()
      .find(|&c| c != '\t' && c != ' ' && !c.is_ascii_graphic());

   if let Some(c) = invalid {
      return Err(Error::UserAgent(format!(
         "Invalid user agent: {:?} is not printable ASCII",
         c
      )));
   }

   Ok(())
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn test_valid_path() {
      assert!(path("/downloads/file.mp4").is_ok());
      assert!(path("/file.txt").is_ok());
   }

   #[test]
   fn test_empty_path() {
      let result = path("");
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[test]
   fn test_relative_path() {
      assert!(path("relative/path.txt").is_err());
      assert!(path("file.txt").is_err());
   }

   #[test]
   fn test_path_without_filename() {
      // Root path has no filename component.
      assert!(path("/").is_err());
   }

   #[test]
   fn test_path_messages_match_other_platforms() {
      // iOS and Android send these exact strings for the same inputs.
      let message = |p: &str| path(p).unwrap_err().to_string();

      assert_eq!(message(""), "Path Error: path cannot be empty");
      assert_eq!(message("file.txt"), "Path Error: path must be absolute");
      assert_eq!(
         message("file:///tmp/file.txt"),
         "Path Error: path must be absolute"
      );
      assert_eq!(message("/"), "Path Error: path must have a filename");
   }

   #[test]
   fn test_valid_store_dirs() {
      assert!(store_dir(Path::new("/var/lib/myapp")).is_ok());
      assert!(store_dir(Path::new("/downloads")).is_ok());
   }

   #[test]
   fn test_empty_store_dir() {
      let result = store_dir(Path::new(""));

      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[test]
   fn test_relative_store_dir() {
      assert!(store_dir(Path::new("downloads")).is_err());
      assert!(store_dir(Path::new("./downloads")).is_err());
      assert!(store_dir(Path::new("../downloads")).is_err());
   }

   #[test]
   fn test_root_store_dir_is_accepted() {
      // Unlike `path`, which requires a filename component: the store appends its own
      // `downloads.json`, so a directory needs no trailing component to be usable.
      assert!(store_dir(Path::new("/")).is_ok());
   }

   #[test]
   fn test_store_dir_is_not_required_to_exist() {
      // The store creates its parent when it saves. Naming a directory that does not
      // exist yet is the normal case on a first run, not a configuration error.
      assert!(store_dir(Path::new("/nonexistent/directory/for/tests")).is_ok());
   }

   #[test]
   fn test_valid_urls() {
      assert!(url("https://example.com/file.mp4").is_ok());
      assert!(url("http://example.com/file.mp4").is_ok());
      assert!(url("https://example.com:8080/file.mp4").is_ok());
      assert!(url("https://example.com/file.mp4?token=abc").is_ok());
      // No path component is valid.
      assert!(url("https://example.com").is_ok());
   }

   #[test]
   fn test_empty_url() {
      let result = url("");
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[test]
   fn test_invalid_scheme() {
      assert!(url("ftp://example.com/file.mp4").is_err());
      assert!(url("file:///path/to/file.mp4").is_err());
      assert!(url("ws://example.com/socket").is_err());
      assert!(url("data:text/plain,hello").is_err());
   }

   #[test]
   fn test_missing_host() {
      assert!(url("https://:8080/file.mp4").is_err());
   }

   #[test]
   fn test_invalid_url_format() {
      assert!(url("not a valid url").is_err());
      // Protocol-relative URL with no scheme.
      assert!(url("//example.com/file.mp4").is_err());
   }

   #[test]
   fn test_url_messages_match_other_platforms() {
      // iOS and Android send these exact strings for the same inputs.
      let message = |u: &str| url(u).unwrap_err().to_string();

      assert_eq!(message(""), "URL Error: URL cannot be empty");
      assert_eq!(
         message("not a valid url"),
         "URL Error: Invalid URL: not a valid url"
      );
      assert_eq!(
         message("example.com/file.mp4"),
         "URL Error: Invalid URL: example.com/file.mp4"
      );
      assert_eq!(
         message("ftp://example.com/file.mp4"),
         "URL Error: Invalid URL scheme 'ftp': must be http or https"
      );
   }

   #[test]
   fn test_valid_user_agents() {
      assert!(user_agent("my-app/1.0").is_ok());
      assert!(user_agent("MyApp/2.1 (Linux; x86_64) build/1234").is_ok());
      // A single token with no version is unusual but legal.
      assert!(user_agent("curl").is_ok());
   }

   #[test]
   fn test_user_agent_with_an_internal_tab_is_accepted() {
      // Tab is the one non-graphic character OkHttp's `checkValue` admits, so the
      // accepted set matches it exactly. Without this case, dropping the tab arm of
      // the filter changes no test outcome.
      assert!(user_agent("my-app/1.0 (build\t1)").is_ok());
   }

   #[test]
   fn test_empty_user_agent_is_accepted() {
      // Characterisation test, not a design goal: an empty header value is legal
      // HTTP, so the rule delegated to `HeaderValue` admits it. Rejecting it would
      // be a policy this crate invented.
      assert!(user_agent("").is_ok());
   }

   #[test]
   fn test_user_agent_with_non_ascii_characters() {
      // Accepted by `HeaderValue`, rejected by OkHttp: without this rule the value
      // passes plugin init and then fails every Android download.
      assert!(user_agent("Caf\u{e9}/1.0").is_err());
      assert!(user_agent("\u{fc}ber/2.0").is_err());
      assert!(user_agent("app/1.0 \u{2122}").is_err());

      let result = user_agent("Caf\u{e9}/1.0");
      assert!(result.unwrap_err().to_string().contains("printable ASCII"));
   }

   #[test]
   fn test_user_agent_with_control_characters() {
      // A newline would let a caller inject a second header, so the header value
      // rule rejects it before the client is ever built.
      assert!(user_agent("bad\nvalue").is_err());
      assert!(user_agent("bad\rvalue").is_err());
      assert!(user_agent("bad\0value").is_err());
   }
}
