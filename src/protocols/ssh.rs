use async_trait::async_trait;
use url::Url;
use tempfile::NamedTempFile;
use std::process::Command;
use std::io::Write;
use tracing::info;
use crate::{
    Result, TinyEtlError,
    connectors::{Source, Target, create_source, create_target},
    protocols::Protocol,
};

/// SSH protocol for downloading files via SCP/SFTP.
/// Uses system SSH client for file transfers to temporary locations.
pub struct SshProtocol;

impl SshProtocol {
    pub fn new() -> Self {
        Self
    }
    
    /// Download a file via SCP to a temporary file with progress
    async fn download_via_scp(&self, url: &Url) -> Result<NamedTempFile> {
        // Parse SSH URL: ssh://user@host:port/path/to/file
        let host = url.host_str()
            .ok_or_else(|| TinyEtlError::Configuration("SSH URL must specify a host".to_string()))?;
        
        let username = if !url.username().is_empty() {
            url.username()
        } else {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a username (ssh://user@host/path)".to_string()
            ));
        };
        
        let port = url.port().unwrap_or(22);
        let remote_path = url.path();
        
        if remote_path.is_empty() || remote_path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a file path".to_string()
            ));
        }
        
        // Create temporary file with appropriate extension
        let extension = self.extract_extension_from_path(remote_path);
        let temp_file = if let Some(ext) = extension {
            tempfile::Builder::new()
                .suffix(&format!(".{}", ext))
                .tempfile()
                .map_err(|e| TinyEtlError::Io(e))?
        } else {
            tempfile::NamedTempFile::new()
                .map_err(|e| TinyEtlError::Io(e))?
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
            .map_err(|e| TinyEtlError::Connection(format!("Failed to execute scp command: {}", e)))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TinyEtlError::Connection(format!(
                "SCP failed to download file from {}: {}", 
                scp_source, 
                stderr
            )));
        }
        
        info!("SSH download completed");
        
        Ok(temp_file)
    }
    
    /// Upload a file via SCP (for target operations)
    async fn upload_via_scp(&self, url: &Url, local_path: &str) -> Result<()> {
        let host = url.host_str()
            .ok_or_else(|| TinyEtlError::Configuration("SSH URL must specify a host".to_string()))?;
        
        let username = if !url.username().is_empty() {
            url.username()
        } else {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a username (ssh://user@host/path)".to_string()
            ));
        };
        
        let port = url.port().unwrap_or(22);
        let remote_path = url.path();
        
        if remote_path.is_empty() || remote_path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH URL must specify a file path".to_string()
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
            .map_err(|e| TinyEtlError::Connection(format!("Failed to execute scp command: {}", e)))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TinyEtlError::Connection(format!(
                "SCP failed to upload file to {}: {}", 
                scp_dest, 
                stderr
            )));
        }
        
        info!("SSH upload completed");
        
        Ok(())
    }
    
    /// Extract file extension from remote path
    fn extract_extension_from_path(&self, path: &str) -> Option<String> {
        if let Some(filename) = path.split('/').last() {
            if let Some(extension) = filename.split('.').last() {
                if extension.len() > 0 && extension.len() <= 10 && extension != filename {
                    return Some(extension.to_lowercase());
                }
            }
        }
        None
    }
}

#[async_trait]
impl Protocol for SshProtocol {
    async fn create_source(&self, url: &Url) -> Result<Box<dyn Source>> {
        // Download the file via SCP to a temporary location
        let temp_file = self.download_via_scp(url).await?;
        let temp_path = temp_file.path().to_string_lossy().to_string();
        
        // Create source using the temporary file path
        // Note: Similar limitation as HTTP - the temp file lifetime management
        // could be improved
        create_source(&temp_path)
    }
    
    async fn create_target(&self, url: &Url) -> Result<Box<dyn Target>> {
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
            return Err(TinyEtlError::Configuration(
                format!("SSH protocol requires ssh:// scheme, got: {}", url.scheme())
            ));
        }
        
        if url.host().is_none() {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a valid host".to_string()
            ));
        }
        
        if url.username().is_empty() {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a username in the URL (ssh://user@host/path)".to_string()
            ));
        }
        
        let path = url.path();
        if path.is_empty() || path == "/" {
            return Err(TinyEtlError::Configuration(
                "SSH protocol requires a file path".to_string()
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
        assert_eq!(protocol.extract_extension_from_path("/path/to/data.csv"), Some("csv".to_string()));
        assert_eq!(protocol.extract_extension_from_path("/data/file.json"), Some("json".to_string()));
        assert_eq!(protocol.extract_extension_from_path("file.parquet"), Some("parquet".to_string()));
        
        // Paths without extensions
        assert_eq!(protocol.extract_extension_from_path("/path/to/data"), None);
        assert_eq!(protocol.extract_extension_from_path("/api/endpoint"), None);
    }
    
    #[test]
    fn test_target_not_fully_supported() {
        let protocol = SshProtocol::new();
        let url = Url::parse("ssh://user@example.com/upload/file.csv").unwrap();
        
        // SSH target operations are not fully implemented yet
        tokio_test::block_on(async {
            let result = protocol.create_target(&url).await;
            assert!(result.is_err());
        });
    }
}
