pub mod file;
pub mod http;
pub mod s3;
pub mod ssh;

use crate::{
    connectors::{Source, Target},
    Result, TinyEtlError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use url::Url;

/// Protocol trait that abstracts the transport layer from the data format layer.
/// Protocols handle how to connect to and authenticate with different systems,
/// while connectors handle the actual data format reading/writing.
#[async_trait]
pub trait Protocol: Send + Sync {
    /// Create a source connector for reading data from this protocol
    async fn create_source(
        &self,
        url: &Url,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>>;

    /// Create a source connector with a type hint (useful for HTTP/remote sources)
    async fn create_source_with_type(
        &self,
        url: &Url,
        _source_type: Option<&str>,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>> {
        // Default implementation ignores the type hint
        self.create_source(url, options).await
    }

    /// Create a target connector for writing data to this protocol  
    async fn create_target(
        &self,
        url: &Url,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Target>>;

    /// Validate that the URL is properly formatted for this protocol
    fn validate_url(&self, url: &Url) -> Result<()>;

    /// Get the protocol name (e.g., "snowflake", "file", "databricks")
    fn name(&self) -> &'static str;
}

/// Extract a lowercased file extension from the last segment of a URL or remote path.
/// Used by remote protocols to pick the right file connector for a downloaded file.
pub(crate) fn extension_from_path(path: &str) -> Option<String> {
    let filename = path.split('/').next_back()?;
    let (stem, extension) = filename.rsplit_once('.')?;
    if extension.is_empty() || extension.len() > 10 {
        return None;
    }
    let extension = extension.to_lowercase();
    // Keep the format of a compressed file: `data.csv.gz` → `csv.gz`
    if extension == "gz" || extension == "zst" {
        if let Some((_, inner)) = stem.rsplit_once('.') {
            if !inner.is_empty() && inner.len() <= 10 {
                return Some(format!("{}.{}", inner.to_lowercase(), extension));
            }
        }
    }
    Some(extension)
}

/// Factory function to create a protocol handler based on URL scheme
pub fn create_protocol(url: &str) -> Result<Box<dyn Protocol>> {
    // For backward compatibility, handle file paths that aren't valid URLs
    if !url.contains("://") && (url.contains('.') || url.starts_with('/')) {
        return Ok(Box::new(file::FileProtocol::new()));
    }

    let parsed_url = Url::parse(url)
        .map_err(|e| TinyEtlError::Configuration(format!("Invalid URL '{}': {}", url, e)))?;

    match parsed_url.scheme() {
        "file" => Ok(Box::new(file::FileProtocol::new())),
        "http" | "https" => Ok(Box::new(http::HttpProtocol::new())),
        "ssh" => Ok(Box::new(ssh::SshProtocol::new())),
        "s3" => Ok(Box::new(s3::S3Protocol::new())),
        scheme => {
            Err(TinyEtlError::Configuration(
                format!("Unsupported protocol: {}. Supported protocols: file://, http://, https://, ssh://, s3:// (databases such as snowflake:// are handled by connectors)", scheme)
            ))
        }
    }
}

/// Helper function to create source using protocol abstraction
pub async fn create_source_from_url(url: &str) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type(url, None, &HashMap::new()).await
}

/// Helper function to create source using protocol abstraction with optional type hint
pub async fn create_source_from_url_with_type(
    url: &str,
    source_type: Option<&str>,
    options: &HashMap<String, String>,
) -> Result<Box<dyn Source>> {
    let protocol = create_protocol(url)?;
    let parsed_url = if url.contains("://") {
        Url::parse(url)
            .map_err(|e| TinyEtlError::Configuration(format!("Invalid URL '{}': {}", url, e)))?
    } else {
        // For backward compatibility with simple file paths
        Url::parse(&format!("file://{}", url)).map_err(|e| {
            TinyEtlError::Configuration(format!("Invalid file path '{}': {}", url, e))
        })?
    };

    protocol.validate_url(&parsed_url)?;
    protocol
        .create_source_with_type(&parsed_url, source_type, options)
        .await
}

/// Helper function to create target using protocol abstraction
pub async fn create_target_from_url(url: &str) -> Result<Box<dyn Target>> {
    create_target_from_url_with_options(url, &HashMap::new()).await
}

/// Helper function to create target using protocol abstraction with options
pub async fn create_target_from_url_with_options(
    url: &str,
    options: &HashMap<String, String>,
) -> Result<Box<dyn Target>> {
    let protocol = create_protocol(url)?;
    let parsed_url = if url.contains("://") {
        Url::parse(url)
            .map_err(|e| TinyEtlError::Configuration(format!("Invalid URL '{}': {}", url, e)))?
    } else {
        // For backward compatibility with simple file paths
        Url::parse(&format!("file://{}", url)).map_err(|e| {
            TinyEtlError::Configuration(format!("Invalid file path '{}': {}", url, e))
        })?
    };

    protocol.validate_url(&parsed_url)?;
    protocol.create_target(&parsed_url, options).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_file_protocol() {
        let protocol = create_protocol("file:///path/to/file.csv");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "file");
    }

    #[test]
    fn test_create_http_protocol() {
        let protocol = create_protocol("http://example.com/data.csv");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "http");
    }

    #[test]
    fn test_create_https_protocol() {
        let protocol = create_protocol("https://example.com/data.csv");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "http");
    }

    #[test]
    fn test_create_ssh_protocol() {
        let protocol = create_protocol("ssh://user@example.com/path/to/data.csv");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "ssh");
    }

    #[test]
    fn test_create_s3_protocol() {
        let protocol = create_protocol("s3://my-bucket/path/to/data.parquet");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "s3");
    }

    #[test]
    fn test_extension_from_path() {
        assert_eq!(
            extension_from_path("/a/b/data.CSV"),
            Some("csv".to_string())
        );
        assert_eq!(extension_from_path("/a.b/data"), None);
        assert_eq!(extension_from_path("/a/b/"), None);
        assert_eq!(
            extension_from_path("/d/worldcities.csv.gz"),
            Some("csv.gz".to_string())
        );
        assert_eq!(
            extension_from_path("/d/t.Parquet.ZST"),
            Some("parquet.zst".to_string())
        );
        assert_eq!(extension_from_path("/d/archive.gz"), Some("gz".to_string()));
    }

    #[test]
    fn test_backward_compatibility() {
        // Test that old file paths still work
        let protocol = create_protocol("test.csv");
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "file");
    }

    #[test]
    fn test_unsupported_protocol() {
        let result = create_protocol("ftp://example.com/file.csv");
        assert!(result.is_err());

        let result = create_protocol("unknown://example.com/file.csv");
        assert!(result.is_err());
    }
}
