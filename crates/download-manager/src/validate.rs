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
   let parsed = url::Url::parse(url).map_err(|e| Error::Url(format!("Invalid URL: {}", e)))?;

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
}
