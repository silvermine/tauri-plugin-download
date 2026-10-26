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

   // Refused, not forwarded: credentials would be persisted in the store and logged
   // with every progress line. After the host, so all three platforms agree on which
   // error a URL failing both gets.
   if !parsed.username().is_empty() || parsed.password().is_some() {
      return Err(Error::Url("URL must not contain credentials".to_string()));
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
   use rstest::rstest;

   #[rstest]
   #[case::nested_file("/downloads/file.mp4")]
   #[case::top_level_file("/file.txt")]
   fn test_valid_path(#[case] input: &str) {
      assert!(path(input).is_ok());
   }

   #[test]
   fn test_empty_path() {
      let result = path("");
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[rstest]
   #[case::nested_relative_file("relative/path.txt")]
   #[case::bare_filename("file.txt")]
   fn test_relative_path(#[case] input: &str) {
      assert!(path(input).is_err());
   }

   #[test]
   fn test_path_without_filename() {
      // Root path has no filename component.
      assert!(path("/").is_err());
   }

   // iOS and Android send these exact strings for the same inputs.
   #[rstest]
   #[case::empty("", "Path Error: path cannot be empty")]
   #[case::relative("file.txt", "Path Error: path must be absolute")]
   #[case::file_url("file:///tmp/file.txt", "Path Error: path must be absolute")]
   #[case::root("/", "Path Error: path must have a filename")]
   fn test_path_messages_match_other_platforms(#[case] input: &str, #[case] expected: &str) {
      assert_eq!(path(input).unwrap_err().to_string(), expected);
   }

   #[rstest]
   #[case::nested("/var/lib/myapp")]
   #[case::top_level("/downloads")]
   // Unlike `path`, which requires a filename component: the store appends its own
   // `downloads.json`, so a directory needs no trailing component to be usable.
   #[case::root("/")]
   // The store creates its parent when it saves. Naming a directory that does not
   // exist yet is the normal case on a first run, not a configuration error.
   #[case::nonexistent_directory("/nonexistent/directory/for/tests")]
   fn test_valid_store_dirs(#[case] dir: &str) {
      assert!(store_dir(Path::new(dir)).is_ok());
   }

   #[test]
   fn test_empty_store_dir() {
      let result = store_dir(Path::new(""));

      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[rstest]
   #[case::bare_name("downloads")]
   #[case::current_dir_prefix("./downloads")]
   #[case::parent_dir_prefix("../downloads")]
   fn test_relative_store_dir(#[case] dir: &str) {
      assert!(store_dir(Path::new(dir)).is_err());
   }

   #[rstest]
   #[case::https("https://example.com/file.mp4")]
   #[case::http("http://example.com/file.mp4")]
   #[case::explicit_port("https://example.com:8080/file.mp4")]
   #[case::query_string("https://example.com/file.mp4?token=abc")]
   #[case::no_path_component("https://example.com")]
   fn test_valid_urls(#[case] input: &str) {
      assert!(url(input).is_ok());
   }

   #[test]
   fn test_empty_url() {
      let result = url("");
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   // file: and data: URLs have no host, so the host check would reject them too.
   // Only the message shows that the scheme check rejected them.
   #[rstest]
   #[case::ftp("ftp://example.com/file.mp4", "ftp")]
   #[case::file("file:///path/to/file.mp4", "file")]
   #[case::ws("ws://example.com/socket", "ws")]
   #[case::data("data:text/plain,hello", "data")]
   fn test_invalid_scheme(#[case] input: &str, #[case] scheme: &str) {
      assert_eq!(
         url(input).unwrap_err().to_string(),
         format!("URL Error: Invalid URL scheme '{scheme}': must be http or https")
      );
   }

   #[test]
   fn test_missing_host() {
      assert!(url("https://:8080/file.mp4").is_err());
   }

   #[rstest]
   #[case::username_and_password("https://user:pass@example.com/file.mp4")]
   #[case::username_only("https://user@example.com/file.mp4")]
   // A password with no username is still credentials.
   #[case::password_only("https://:pass@example.com/file.mp4")]
   fn test_url_with_credentials(#[case] input: &str) {
      assert!(url(input).is_err());
   }

   #[rstest]
   #[case::free_text("not a valid url")]
   #[case::protocol_relative_without_scheme("//example.com/file.mp4")]
   fn test_invalid_url_format(#[case] input: &str) {
      assert!(url(input).is_err());
   }

   // iOS and Android send these exact strings for the same inputs.
   #[rstest]
   #[case::empty("", "URL Error: URL cannot be empty")]
   #[case::free_text("not a valid url", "URL Error: Invalid URL: not a valid url")]
   #[case::missing_scheme("example.com/file.mp4", "URL Error: Invalid URL: example.com/file.mp4")]
   #[case::unsupported_scheme(
      "ftp://example.com/file.mp4",
      "URL Error: Invalid URL scheme 'ftp': must be http or https"
   )]
   #[case::credentials(
      "https://user:pass@example.com/file.mp4",
      "URL Error: URL must not contain credentials"
   )]
   fn test_url_messages_match_other_platforms(#[case] input: &str, #[case] expected: &str) {
      assert_eq!(url(input).unwrap_err().to_string(), expected);
   }

   #[rstest]
   #[case::name_and_version("my-app/1.0")]
   #[case::with_platform_comment("MyApp/2.1 (Linux; x86_64) build/1234")]
   // A single token with no version is unusual but legal.
   #[case::single_token_without_version("curl")]
   // Tab is the one non-graphic character OkHttp's `checkValue` admits, so the
   // accepted set matches it exactly. Without this case, dropping the tab arm of
   // the filter changes no test outcome.
   #[case::internal_tab("my-app/1.0 (build\t1)")]
   fn test_valid_user_agents(#[case] value: &str) {
      assert!(user_agent(value).is_ok());
   }

   #[test]
   fn test_empty_user_agent_is_accepted() {
      // Characterisation test, not a design goal: an empty header value is legal
      // HTTP, so the rule delegated to `HeaderValue` admits it. Rejecting it would
      // be a policy this crate invented.
      assert!(user_agent("").is_ok());
   }

   // Accepted by `HeaderValue`, rejected by OkHttp: without this rule the value
   // passes plugin init and then fails every Android download.
   #[rstest]
   #[case::e_acute("Caf\u{e9}/1.0")]
   #[case::u_umlaut("\u{fc}ber/2.0")]
   #[case::trademark_sign("app/1.0 \u{2122}")]
   fn test_user_agent_with_non_ascii_characters(#[case] value: &str) {
      let result = user_agent(value);
      assert!(result.unwrap_err().to_string().contains("printable ASCII"));
   }

   // A newline would let a caller inject a second header, so the header value
   // rule rejects it before the client is ever built.
   #[rstest]
   #[case::newline("bad\nvalue")]
   #[case::carriage_return("bad\rvalue")]
   #[case::nul("bad\0value")]
   fn test_user_agent_with_control_characters(#[case] value: &str) {
      assert!(user_agent(value).is_err());
   }
}
