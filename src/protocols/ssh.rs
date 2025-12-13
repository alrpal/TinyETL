use crate::{
    connectors::{create_source, Source, Target},
    protocols::Protocol,
    Result, TinyEtlError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Command;
use tempfile::NamedTempFile;
use tracing::info;
use url::Url;

/// SSH protocol for downloading files via SCP/SFTP.
/// Uses system SSH client for file transfers to temporary locations.
pub struct SshProtocol;

impl Default for SshProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl SshProtocol {
    pub fn new() -> Self {
        Self
    }

    /// Download a file via SCP to a temporary file with progress
    async fn download_via_scp(&self, url: &Url) -> Result<NamedTempFile> {
        // Parse SSH URL: ssh://user@host:port/path/to/file
        let host = url.host_str().ok_or_else(|| {
            TinyEtlError::Configuration("SSH URL must specify a host".to_string())
        })?;

        let username = if !url.username().is_empty() {
            url.username()
        } else {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a username (ssh://user@host/path)".to_string(),
            ));
        };

        let port = url.port().unwrap_or(22);
        let remote_path = url.path();

        if remote_path.is_empty() || remote_path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a file path".to_string(),
            ));
        }

        // Create temporary file with appropriate extension
        let extension = self.extract_extension_from_path(remote_path);
        let temp_file = if let Some(ext) = extension {
            tempfile::Builder::new()
                .suffix(&format!(".{}", ext))
                .tempfile()
                .map_err(TinyEtlError::Io)?
        } else {
            tempfile::NamedTempFile::new().map_err(TinyEtlError::Io)?
        };

        let temp_path = temp_file.path().to_string_lossy().to_string();

        // Build SCP command: scp -P port user@host:remote_path local_path
        let scp_source = format!("{}@{}:{}", username, host, remote_path);

        info!("Downloading via SSH: {}", scp_source);

        let output = Command::new("scp")
            .arg("-P")
            .arg(port.to_string())
            .arg("-o")
            .arg("StrictHostKeyChecking=no") // Allow connecting to new hosts
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null") // Don't save host keys
            .arg("-q") // Quiet mode
            .arg(&scp_source)
            .arg(&temp_path)
            .output()
            .map_err(|e| {
                TinyEtlError::Connection(format!("Failed to execute scp command: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TinyEtlError::Connection(format!(
                "SCP failed to download file from {}: {}",
                scp_source, stderr
            )));
        }

        info!("SSH download completed");

        Ok(temp_file)
    }

    /// Upload a file via SCP (for target operations)
    #[allow(dead_code)]
    async fn upload_via_scp(&self, url: &Url, local_path: &str) -> Result<()> {
        let host = url.host_str().ok_or_else(|| {
            TinyEtlError::Configuration("SSH URL must specify a host".to_string())
        })?;

        let username = if !url.username().is_empty() {
            url.username()
        } else {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a username (ssh://user@host/path)".to_string(),
            ));
        };

        let port = url.port().unwrap_or(22);
        let remote_path = url.path();

        if remote_path.is_empty() || remote_path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a file path".to_string(),
            ));
        }

        // Build SCP command: scp -P port local_path user@host:remote_path
        let scp_dest = format!("{}@{}:{}", username, host, remote_path);

        info!("Uploading via SSH to: {}", scp_dest);

        let output = Command::new("scp")
            .arg("-P")
            .arg(port.to_string())
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-q")
            .arg(local_path)
            .arg(&scp_dest)
            .output()
            .map_err(|e| {
                TinyEtlError::Connection(format!("Failed to execute scp command: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TinyEtlError::Connection(format!(
                "SCP failed to upload file to {}: {}",
                scp_dest, stderr
            )));
        }

        info!("SSH upload completed");

        Ok(())
    }

    /// Extract file extension from remote path
    fn extract_extension_from_path(&self, path: &str) -> Option<String> {
        if let Some(filename) = path.split('/').next_back() {
            if let Some(extension) = filename.split('.').next_back() {
                if !extension.is_empty() && extension.len() <= 10 && extension != filename {
                    return Some(extension.to_lowercase());
                }
            }
        }
        None
    }
}

#[async_trait]
impl Protocol for SshProtocol {
    async fn create_source(
        &self,
        url: &Url,
        _options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>> {
        // SSH protocol could use options for:
        // - identity_file: path to SSH key
        // - known_hosts_file: path to known_hosts
        // - ssh_options: additional SSH options
        // Download the file via SCP to a temporary location
        let temp_file = self.download_via_scp(url).await?;
        let temp_path = temp_file.path().to_string_lossy().to_string();

        // Create source using the temporary file path
        // Note: Similar limitation as HTTP - the temp file lifetime management
        // could be improved
        create_source(&temp_path)
    }

    async fn create_target(
        &self,
        _url: &Url,
        _options: &HashMap<String, String>,
    ) -> Result<Box<dyn Target>> {
        // For SSH targets, we'll create a local temporary file target
        // and then upload it after writing is complete
        // This is a simplified implementation - a full implementation would
        // need better integration with the Target trait lifecycle
        Err(TinyEtlError::Configuration(
            "SSH target implementation requires additional coordination with the ETL pipeline. Use file:// for local output and manually upload via SSH.".to_string()
        ))
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        if url.scheme() != "ssh" {
            return Err(TinyEtlError::Configuration(format!(
                "SSH protocol requires ssh:// scheme, got: {}",
                url.scheme()
            )));
        }

        if url.host().is_none() {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a valid host".to_string(),
            ));
        }

        if url.username().is_empty() {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a username in the URL (ssh://user@host/path)".to_string(),
            ));
        }

        let path = url.path();
        if path.is_empty() || path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a file path".to_string(),
            ));
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "ssh"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ssh_url() {
        let protocol = SshProtocol::new();

        // Valid SSH URLs
        let url = Url::parse("ssh://user@example.com/path/to/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        let url = Url::parse("ssh://user@example.com:2222/data/file.json").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        // Invalid - missing username
        let url = Url::parse("ssh://example.com/path/to/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());

        // Invalid - no path
        let url = Url::parse("ssh://user@example.com/").unwrap();
        assert!(protocol.validate_url(&url).is_err());

        // Invalid scheme
        let url = Url::parse("http://example.com/file.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());
    }

    #[test]
    fn test_extract_extension_from_path() {
        let protocol = SshProtocol::new();

        // Paths with extensions
        assert_eq!(
            protocol.extract_extension_from_path("/path/to/data.csv"),
            Some("csv".to_string())
        );
        assert_eq!(
            protocol.extract_extension_from_path("/data/file.json"),
            Some("json".to_string())
        );
        assert_eq!(
            protocol.extract_extension_from_path("file.parquet"),
            Some("parquet".to_string())
        );

        // Paths without extensions
        assert_eq!(protocol.extract_extension_from_path("/path/to/data"), None);
        assert_eq!(protocol.extract_extension_from_path("/api/endpoint"), None);
    }

    #[test]
    fn test_target_not_fully_supported() {
        let protocol = SshProtocol::new();
        let url = Url::parse("ssh://user@example.com/upload/file.csv").unwrap();
        let options = std::collections::HashMap::new();

        // SSH target operations are not fully implemented yet
        tokio_test::block_on(async {
            let result = protocol.create_target(&url, &options).await;
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_ssh_protocol_name() {
        let protocol = SshProtocol::new();
        assert_eq!(protocol.name(), "ssh");
    }

    #[test]
    fn test_url_validation_error_messages() {
        let protocol = SshProtocol::new();

        // Test wrong scheme
        let url = Url::parse("ftp://user@example.com/file.csv").unwrap();
        let result = protocol.validate_url(&url);
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("SSH protocol requires ssh:// scheme"));
            assert!(msg.contains("ftp"));
        } else {
            panic!("Expected Configuration error");
        }

        // Test missing username
        let url = Url::parse("ssh://example.com/file.csv").unwrap();
        let result = protocol.validate_url(&url);
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("requires a username"));
        } else {
            panic!("Expected Configuration error");
        }

        // Test missing path
        let url = Url::parse("ssh://user@example.com").unwrap();
        let result = protocol.validate_url(&url);
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("requires a file path"));
        } else {
            panic!("Expected Configuration error");
        }

        // Test empty path
        let url = Url::parse("ssh://user@example.com/").unwrap();
        let result = protocol.validate_url(&url);
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("requires a file path"));
        } else {
            panic!("Expected Configuration error");
        }
    }

    #[test]
    fn test_extract_extension_edge_cases() {
        let protocol = SshProtocol::new();

        // Test various edge cases for extension extraction
        assert_eq!(protocol.extract_extension_from_path(""), None);
        assert_eq!(protocol.extract_extension_from_path("/"), None);
        assert_eq!(
            protocol.extract_extension_from_path("/.hidden"),
            Some("hidden".to_string())
        ); // Current implementation treats this as extension
        assert_eq!(protocol.extract_extension_from_path("/path/to/."), None);
        assert_eq!(protocol.extract_extension_from_path("/path/to/.."), None);

        // Test file names with dots but no extension
        assert_eq!(protocol.extract_extension_from_path("/path/to/file."), None);
        assert_eq!(
            protocol.extract_extension_from_path("/path/file.toolongextension"),
            None
        );

        // Test various valid extensions
        assert_eq!(
            protocol.extract_extension_from_path("/data.CSV"),
            Some("csv".to_string())
        );
        assert_eq!(
            protocol.extract_extension_from_path("/file.JSON"),
            Some("json".to_string())
        );
        assert_eq!(
            protocol.extract_extension_from_path("/archive.tar.gz"),
            Some("gz".to_string())
        );

        // Test nested paths
        assert_eq!(
            protocol.extract_extension_from_path("/home/user/documents/data.xlsx"),
            Some("xlsx".to_string())
        );
        assert_eq!(
            protocol.extract_extension_from_path("./relative/path/file.txt"),
            Some("txt".to_string())
        );

        // Test Windows-style paths (shouldn't matter for SSH but good to test)
        assert_eq!(
            protocol.extract_extension_from_path("C:\\data\\file.csv"),
            Some("csv".to_string())
        );
    }

    #[test]
    fn test_url_parsing_components() {
        // Test URL component extraction logic that would be used in SCP operations

        // Basic URL
        let url = Url::parse("ssh://user@example.com/path/to/file.csv").unwrap();
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.username(), "user");
        assert_eq!(url.port(), None); // Should default to 22
        assert_eq!(url.path(), "/path/to/file.csv");

        // URL with port
        let url = Url::parse("ssh://admin@server.local:2222/data/export.json").unwrap();
        assert_eq!(url.host_str(), Some("server.local"));
        assert_eq!(url.username(), "admin");
        assert_eq!(url.port(), Some(2222));
        assert_eq!(url.path(), "/data/export.json");

        // URL with special characters in username
        let url = Url::parse("ssh://user%2Bname@example.com/file.csv").unwrap();
        assert_eq!(url.username(), "user%2Bname"); // URL encoded

        // URL with IP address
        let url = Url::parse("ssh://root@192.168.1.100:22/tmp/data.parquet").unwrap();
        assert_eq!(url.host_str(), Some("192.168.1.100"));
        assert_eq!(url.port(), Some(22));
    }

    #[test]
    fn test_scp_command_building() {
        // Test the logic for building SCP commands (without actually executing them)
        let url = Url::parse("ssh://user@example.com:2222/path/to/file.csv").unwrap();

        let host = url.host_str().unwrap();
        let username = url.username();
        let port = url.port().unwrap_or(22);
        let remote_path = url.path();

        // Test source construction for download
        let scp_source = format!("{}@{}:{}", username, host, remote_path);
        assert_eq!(scp_source, "user@example.com:/path/to/file.csv");

        // Test port handling
        assert_eq!(port, 2222);

        // Test destination construction for upload
        let scp_dest = format!("{}@{}:{}", username, host, "/upload/target.csv");
        assert_eq!(scp_dest, "user@example.com:/upload/target.csv");
    }

    #[test]
    fn test_default_port_handling() {
        let url = Url::parse("ssh://user@example.com/file.csv").unwrap();
        let port = url.port().unwrap_or(22);
        assert_eq!(port, 22);

        let url_with_port = Url::parse("ssh://user@example.com:443/file.csv").unwrap();
        let port_explicit = url_with_port.port().unwrap_or(22);
        assert_eq!(port_explicit, 443);
    }

    #[test]
    fn test_path_validation_edge_cases() {
        let protocol = SshProtocol::new();

        // Test various path scenarios that should be invalid
        let invalid_paths = vec![
            "ssh://user@host",  // No path
            "ssh://user@host/", // Root path only
        ];

        for url_str in invalid_paths {
            let url = Url::parse(url_str).unwrap();
            assert!(
                protocol.validate_url(&url).is_err(),
                "Should reject URL: {}",
                url_str
            );
        }

        // Test valid paths
        let valid_paths = vec![
            "ssh://user@host/file",
            "ssh://user@host/path/to/file.csv",
            "ssh://user@host/data/export.json",
            "ssh://user@host/home/user/.bashrc",
        ];

        for url_str in valid_paths {
            let url = Url::parse(url_str).unwrap();
            assert!(
                protocol.validate_url(&url).is_ok(),
                "Should accept URL: {}",
                url_str
            );
        }
    }

    #[tokio::test]
    async fn test_create_source_url_validation() {
        let protocol = SshProtocol::new();
        let options = std::collections::HashMap::new();

        // Test that create_source validates the URL before attempting download
        let invalid_url = Url::parse("ssh://example.com/file.csv").unwrap(); // Missing username
        let result = protocol.create_source(&invalid_url, &options).await;

        // Should fail at validation stage, not at SCP execution
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_target_error_handling() {
        let protocol = SshProtocol::new();
        let options = std::collections::HashMap::new();

        let url = Url::parse("ssh://user@example.com/upload/file.csv").unwrap();
        let result = protocol.create_target(&url, &options).await;

        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("SSH target implementation requires additional coordination"));
        } else {
            panic!("Expected specific Configuration error message");
        }
    }

    #[test]
    fn test_error_types_and_messages() {
        // Test that appropriate error types are returned for different failure modes
        let protocol = SshProtocol::new();

        // Configuration errors
        let url = Url::parse("http://example.com/file.csv").unwrap();
        match protocol.validate_url(&url) {
            Err(TinyEtlError::Configuration(_)) => {} // Expected
            _ => panic!("Expected Configuration error"),
        }

        // Test error message content
        let url = Url::parse("ssh://host/file.csv").unwrap(); // Missing username
        if let Err(TinyEtlError::Configuration(msg)) = protocol.validate_url(&url) {
            assert!(msg.contains("username"));
            assert!(msg.contains("ssh://user@host/path"));
        } else {
            panic!("Expected Configuration error with helpful message");
        }
    }

    #[test]
    fn test_temp_file_extension_preservation() {
        let protocol = SshProtocol::new();

        // Test that extensions are preserved for temporary files
        let paths_and_extensions = vec![
            ("/data/file.csv", Some("csv".to_string())),
            ("/exports/data.json", Some("json".to_string())),
            ("/files/archive.parquet", Some("parquet".to_string())),
            ("/backup/dump.sql", Some("sql".to_string())),
            ("/logs/access.log", Some("log".to_string())),
            ("/data/noextension", None),
        ];

        for (path, expected_ext) in paths_and_extensions {
            let actual_ext = protocol.extract_extension_from_path(path);
            assert_eq!(actual_ext, expected_ext, "Failed for path: {}", path);
        }
    }

    #[test]
    fn test_ssh_protocol_instantiation() {
        let protocol = SshProtocol::new();

        // Test that the protocol can be created and has expected properties
        assert_eq!(protocol.name(), "ssh");

        // Test that it implements the expected traits
        let _: &dyn Protocol = &protocol;
    }
}
