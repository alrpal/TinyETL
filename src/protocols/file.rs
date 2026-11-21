use crate::{
    connectors::{create_source, create_target, Source, Target},
    protocols::Protocol,
    Result, TinyEtlError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use url::Url;

/// File protocol for local file system operations.
/// This wraps existing connector implementations to maintain backward compatibility
/// while providing the new protocol abstraction.
pub struct FileProtocol;

impl Default for FileProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl FileProtocol {
    pub fn new() -> Self {
        Self
    }

    /// Convert a file:// URL back to a path string for backward compatibility
    /// with existing connector implementations
    fn url_to_path(&self, url: &Url) -> Result<String> {
        if url.scheme() != "file" {
            return Err(TinyEtlError::Configuration(format!(
                "Expected file:// scheme, got: {}",
                url.scheme()
            )));
        }

        // For file URLs, the path is what matters
        let path = if let Ok(path) = url.to_file_path() {
            path.to_string_lossy().to_string()
        } else {
            // Fallback for URLs that don't convert to file paths cleanly
            url.path().to_string()
        };

        // Handle fragment for database tables (e.g., file:///path/to/db.sqlite#table)
        let connection_string = if let Some(fragment) = url.fragment() {
            format!("{}#{}", path, fragment)
        } else {
            path
        };

        Ok(connection_string)
    }
}

#[async_trait]
impl Protocol for FileProtocol {
    async fn create_source(
        &self,
        url: &Url,
        _options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>> {
        // File protocol doesn't use options currently, but could be extended
        // for future features like compression, encoding, etc.
        let path = self.url_to_path(url)?;
        create_source(&path)
    }

    async fn create_target(
        &self,
        url: &Url,
        _options: &HashMap<String, String>,
    ) -> Result<Box<dyn Target>> {
        // File protocol doesn't use options currently, but could be extended
        // for future features like compression, encoding, etc.
        let path = self.url_to_path(url)?;
        create_target(&path)
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        if url.scheme() != "file" {
            return Err(TinyEtlError::Configuration(format!(
                "File protocol requires file:// scheme, got: {}",
                url.scheme()
            )));
        }

        let path = url.path();
        if path.is_empty() {
            return Err(TinyEtlError::Configuration(
                "File protocol requires a valid file path".to_string(),
            ));
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_file_url() {
        let protocol = FileProtocol::new();

        // Valid file URLs
        let url = Url::parse("file:///path/to/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        let url = Url::parse("file:///path/to/db.sqlite#table").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        // Invalid schemes
        let url = Url::parse("http://example.com/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());
    }

    #[test]
    fn test_url_to_path() {
        let protocol = FileProtocol::new();

        // Basic file path
        let url = Url::parse("file:///path/to/file.csv").unwrap();
        let path = protocol.url_to_path(&url).unwrap();
        assert!(path.ends_with("file.csv"));

        // Database with table
        let url = Url::parse("file:///path/to/db.sqlite#table").unwrap();
        let path = protocol.url_to_path(&url).unwrap();
        assert!(path.contains("db.sqlite#table"));
    }

    #[tokio::test]
    async fn test_create_csv_source() {
        let protocol = FileProtocol::new();
        let url = Url::parse("file:///test.csv").unwrap();
        let options = std::collections::HashMap::new();

        // This will fail because the file doesn't exist, but it tests the factory logic
        let result = protocol.create_source(&url, &options).await;
        // Should create a CSV source but fail to open the non-existent file
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_json_target() {
        let protocol = FileProtocol::new();
        let url = Url::parse("file:///output.json").unwrap();
        let options = std::collections::HashMap::new();

        let result = protocol.create_target(&url, &options).await;
        assert!(result.is_ok());
    }
}
