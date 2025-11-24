use crate::{
    connectors::{create_source, Source, Target},
    protocols::Protocol,
    Result, TinyEtlError,
};
use async_trait::async_trait;
use std::io::Write;
use tempfile::NamedTempFile;
use tracing::info;
use url::Url;

/// HTTP/HTTPS protocol for downloading files from web servers.
/// Downloads files to temporary locations and then uses existing connectors.
pub struct HttpProtocol;

impl Default for HttpProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpProtocol {
    pub fn new() -> Self {
        Self
    }

    /// Download a file from an HTTP/HTTPS URL to a temporary file
    #[allow(dead_code)]
    async fn download_to_temp(&self, url: &Url) -> Result<NamedTempFile> {
        self.download_to_temp_with_type(url, None).await
    }

    /// Download a file from an HTTP/HTTPS URL to a temporary file with optional type hint
    async fn download_to_temp_with_type(
        &self,
        url: &Url,
        source_type: Option<&str>,
    ) -> Result<NamedTempFile> {
        let client = reqwest::Client::new();

        info!("Downloading from HTTP URL: {}", url);

        let response =
            client.get(url.as_str()).send().await.map_err(|e| {
                TinyEtlError::Connection(format!("Failed to fetch URL {}: {}", url, e))
            })?;

        if !response.status().is_success() {
            return Err(TinyEtlError::Connection(format!(
                "HTTP request failed with status {}: {}",
                response.status(),
                url
            )));
        }

        // Log the content size if available
        if let Some(size) = response.content_length() {
            info!("Downloading {} bytes", size);
        } else {
            info!("Downloading file (size unknown)");
        }

        let content = response.bytes().await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to read response body: {}", e))
        })?;

        info!("Download completed, {} bytes received", content.len());

        // Create a temporary file with an appropriate extension based on the URL or source type
        let extension = self.get_file_extension(url, source_type);
        let mut temp_file = if let Some(ext) = extension {
            tempfile::Builder::new()
                .suffix(&format!(".{}", ext))
                .tempfile()
                .map_err(TinyEtlError::Io)?
        } else {
            tempfile::NamedTempFile::new().map_err(TinyEtlError::Io)?
        };

        temp_file.write_all(&content).map_err(TinyEtlError::Io)?;

        temp_file.flush().map_err(TinyEtlError::Io)?;

        Ok(temp_file)
    }

    /// Extract file extension from URL path for proper temporary file naming
    fn extract_extension_from_url(&self, url: &Url) -> Option<String> {
        let path = url.path();
        if let Some(filename) = path.split('/').next_back() {
            if let Some(extension) = filename.split('.').next_back() {
                if !extension.is_empty() && extension.len() <= 10 && extension != filename {
                    return Some(extension.to_lowercase());
                }
            }
        }
        None
    }

    /// Extract file extension from URL with optional type override
    fn get_file_extension(&self, url: &Url, source_type: Option<&str>) -> Option<String> {
        if let Some(forced_type) = source_type {
            Some(forced_type.to_lowercase())
        } else {
            self.extract_extension_from_url(url)
        }
    }
}

#[async_trait]
impl Protocol for HttpProtocol {
    async fn create_source(&self, url: &Url) -> Result<Box<dyn Source>> {
        self.create_source_with_type(url, None).await
    }

    async fn create_source_with_type(
        &self,
        url: &Url,
        source_type: Option<&str>,
    ) -> Result<Box<dyn Source>> {
        // Download the file to a temporary location
        let temp_file = self.download_to_temp_with_type(url, source_type).await?;

        // Create a persistent temporary file in the system temp directory
        let extension = self.get_file_extension(url, source_type);
        let temp_dir = std::env::temp_dir();
        let file_name = format!(
            "tinyetl_download_{}.{}",
            std::process::id(),
            extension.unwrap_or_else(|| "tmp".to_string())
        );
        let persistent_path = temp_dir.join(file_name);

        // Copy the content to the persistent path
        std::fs::copy(temp_file.path(), &persistent_path).map_err(TinyEtlError::Io)?;

        let final_path = persistent_path.to_string_lossy().to_string();

        // Create source using the persistent temporary file path
        // Note: This file will not be automatically cleaned up
        // In a production implementation, we'd want better lifecycle management
        create_source(&final_path)
    }

    async fn create_target(&self, _url: &Url) -> Result<Box<dyn Target>> {
        // HTTP protocol doesn't support writing/uploading files in this implementation
        // This would require additional authentication and upload mechanisms
        Err(TinyEtlError::Configuration(
            "HTTP protocol does not support target/write operations. Use file:// protocol for local output.".to_string()
        ))
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        match url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(TinyEtlError::Configuration(format!(
                    "HTTP protocol requires http:// or https:// scheme, got: {}",
                    url.scheme()
                )))
            }
        }

        if url.host().is_none() {
            return Err(TinyEtlError::Configuration(
                "HTTP protocol requires a valid host".to_string(),
            ));
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_http_url() {
        let protocol = HttpProtocol::new();

        // Valid HTTP URLs
        let url = Url::parse("http://example.com/data.csv").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        let url = Url::parse("https://example.com/data.json").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        // Invalid schemes
        let url = Url::parse("file:///path/to/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());

        let url = Url::parse("ftp://example.com/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());
    }

    #[test]
    fn test_extract_extension_from_url() {
        let protocol = HttpProtocol::new();

        // URLs with extensions
        let url = Url::parse("https://example.com/data.csv").unwrap();
        assert_eq!(
            protocol.extract_extension_from_url(&url),
            Some("csv".to_string())
        );

        let url = Url::parse("https://example.com/path/to/file.json").unwrap();
        assert_eq!(
            protocol.extract_extension_from_url(&url),
            Some("json".to_string())
        );

        let url = Url::parse("https://example.com/data.parquet").unwrap();
        assert_eq!(
            protocol.extract_extension_from_url(&url),
            Some("parquet".to_string())
        );

        // URLs without extensions
        let url = Url::parse("https://example.com/api/data").unwrap();
        assert_eq!(protocol.extract_extension_from_url(&url), None);

        // URLs with query parameters
        let url = Url::parse("https://example.com/data.csv?format=csv&limit=1000").unwrap();
        assert_eq!(
            protocol.extract_extension_from_url(&url),
            Some("csv".to_string())
        );
    }

    #[test]
    fn test_target_not_supported() {
        let protocol = HttpProtocol::new();
        let url = Url::parse("https://example.com/upload").unwrap();

        // HTTP protocol should not support target operations
        tokio_test::block_on(async {
            let result = protocol.create_target(&url).await;
            assert!(result.is_err());
        });
    }
}
