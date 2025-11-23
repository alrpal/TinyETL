pub mod file;
pub mod http;
pub mod snowflake;
pub mod ssh;

use crate::{
    connectors::{Source, Target},
    Result, TinyEtlError,
};
use async_trait::async_trait;
use url::Url;

/// Protocol trait that abstracts the transport layer from the data format layer.
/// Protocols handle how to connect to and authenticate with different systems,
/// while connectors handle the actual data format reading/writing.
#[async_trait]
pub trait Protocol: Send + Sync {
    /// Create a source connector for reading data from this protocol
    async fn create_source(&self, url: &Url) -> Result<Box<dyn Source>>;

    /// Create a source connector with a type hint (useful for HTTP/remote sources)
    async fn create_source_with_type(
        &self,
        url: &Url,
        _source_type: Option<&str>,
    ) -> Result<Box<dyn Source>> {
        // Default implementation ignores the type hint
        self.create_source(url).await
    }

    /// Create a target connector for writing data to this protocol  
    async fn create_target(&self, url: &Url) -> Result<Box<dyn Target>>;

    /// Validate that the URL is properly formatted for this protocol
    fn validate_url(&self, url: &Url) -> Result<()>;

    /// Get the protocol name (e.g., "snowflake", "file", "databricks")
    fn name(&self) -> &'static str;
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
        "snowflake" => Ok(Box::new(snowflake::SnowflakeProtocol::new())),
        "http" | "https" => Ok(Box::new(http::HttpProtocol::new())),
        "ssh" => Ok(Box::new(ssh::SshProtocol::new())),
        scheme => {
            Err(TinyEtlError::Configuration(
                format!("Unsupported protocol: {}. Supported protocols: file://, snowflake://, http://, https://, ssh://", scheme)
            ))
        }
    }
}

/// Helper function to create source using protocol abstraction
pub async fn create_source_from_url(url: &str) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type(url, None).await
}

/// Helper function to create source using protocol abstraction with optional type hint
pub async fn create_source_from_url_with_type(
    url: &str,
    source_type: Option<&str>,
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
        .create_source_with_type(&parsed_url, source_type)
        .await
}

/// Helper function to create target using protocol abstraction
pub async fn create_target_from_url(url: &str) -> Result<Box<dyn Target>> {
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
    protocol.create_target(&parsed_url).await
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
    fn test_create_snowflake_protocol() {
        let protocol = create_protocol(
            "snowflake://user:pass@account.region.cloud/db/schema?warehouse=WH&table=table",
        );
        assert!(protocol.is_ok());
        assert_eq!(protocol.unwrap().name(), "snowflake");
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
