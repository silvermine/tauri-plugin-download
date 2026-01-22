use crate::Error;

/// Validates a download URL.
///
/// Checks that the URL:
/// - Is not empty
/// - Has a valid scheme (http or https)
/// - Has a valid host
pub fn validate(url: &str) -> crate::Result<()> {
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
   fn test_valid_urls() {
      assert!(validate("https://example.com/file.mp4").is_ok());
      assert!(validate("http://example.com/file.mp4").is_ok());
      assert!(validate("https://example.com:8080/file.mp4").is_ok());
      assert!(validate("https://example.com/file.mp4?token=abc").is_ok());
   }

   #[test]
   fn test_empty_url() {
      let result = validate("");
      assert!(result.is_err());
      assert!(result.unwrap_err().to_string().contains("empty"));
   }

   #[test]
   fn test_invalid_scheme() {
      assert!(validate("ftp://example.com/file.mp4").is_err());
      assert!(validate("file:///path/to/file.mp4").is_err());
   }

   #[test]
   fn test_missing_host() {
      assert!(validate("https://:8080/file.mp4").is_err());
   }

   #[test]
   fn test_invalid_url_format() {
      assert!(validate("not a valid url").is_err());
   }
}
