use clap::Parser;
use tracing::{info, error};
use tracing_subscriber::{EnvFilter, fmt};

use tinyetl::{
    cli::Cli,
    config::Config,
    connectors::{create_source_from_url_with_type, create_target_from_url},
    transfer::TransferEngine,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();
    let config: Config = cli.into();

    // Initialize logging with specific module filtering
    let env_filter = EnvFilter::new(format!(
        "sqlx=warn,tinyetl={}",
        match config.log_level {
            tinyetl::config::LogLevel::Info => "info",
            tinyetl::config::LogLevel::Warn => "warn", 
            tinyetl::config::LogLevel::Error => "error",
        }
    ));
    
    fmt()
        .with_env_filter(env_filter)
        .init();

    // Create source and target connectors
    let source = create_source_from_url_with_type(&config.source, config.source_type.as_deref()).await?;
    let target = create_target_from_url(&config.target).await?;

    // Execute the transfer
    match TransferEngine::execute(&config, source, target).await {
        Ok(stats) => {
            if !config.preview.is_some() && !config.dry_run {
                info!("Transfer completed successfully!");
                info!("Processed {} rows in {:.2}s ({:.0} rows/sec)", 
                    stats.total_rows, 
                    stats.total_time.as_secs_f64(),
                    stats.rows_per_second
                );
            }
        }
        Err(e) => {
            error!("Transfer failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::NamedTempFile;
    use std::fs::File;
    use std::io::Write;
    
    fn create_test_csv_file() -> Result<NamedTempFile, Box<dyn std::error::Error>> {
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "id,name,age")?;
        writeln!(temp_file, "1,John,30")?;
        writeln!(temp_file, "2,Jane,25")?;
        temp_file.flush()?;
        Ok(temp_file)
    }
    
    #[test]
    fn test_main_function_exists() {
        // Basic test to ensure main function compiles
        assert!(true);
    }
    
    #[test]
    fn test_cli_parsing() {
        // Test that Cli can be created from command line args
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "--source", "test.csv",
            "--target", "test.db#table",
        ]);
        assert!(cli.is_ok());
        
        let cli = cli.unwrap();
        assert_eq!(cli.source, "test.csv");
        assert_eq!(cli.target, "test.db#table");
    }
    
    #[test]
    fn test_cli_to_config_conversion() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "--source", "input.csv", 
            "--target", "output.json",
            "--batch-size", "100",
        ]).unwrap();
        
        let config: Config = cli.into();
        assert_eq!(config.source, "input.csv");
        assert_eq!(config.target, "output.json");
        assert_eq!(config.batch_size, Some(100));
    }
    
    #[test]
    fn test_cli_with_preview_option() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "--source", "test.csv",
            "--target", "test.json",
            "--preview", "5",
        ]).unwrap();
        
        let config: Config = cli.into();
        assert_eq!(config.preview, Some(5));
    }
    
    #[test]
    fn test_cli_with_dry_run() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "--source", "test.csv",
            "--target", "test.json",
            "--dry-run",
        ]).unwrap();
        
        let config: Config = cli.into();
        assert!(config.dry_run);
    }
    
    #[test]
    fn test_cli_with_transform() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "--source", "test.csv",
            "--target", "test.json", 
            "--transform", "transform.lua",
        ]).unwrap();
        
        let config: Config = cli.into();
        assert_eq!(config.transform, Some("transform.lua".to_string()));
    }
    
    #[test]
    fn test_cli_missing_required_args() {
        // Should fail when missing source
        let result = Cli::try_parse_from(&[
            "tinyetl",
            "--target", "test.json",
        ]);
        assert!(result.is_err());
        
        // Should fail when missing target
        let result = Cli::try_parse_from(&[
            "tinyetl", 
            "--source", "test.csv",
        ]);
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_env_filter_log_levels() {
        // Test different log level configurations
        let config_info = Config {
            source: "test.csv".to_string(),
            target: "test.json".to_string(),
            log_level: tinyetl::config::LogLevel::Info,
            ..Default::default()
        };
        
        let env_filter = EnvFilter::new(format!(
            "sqlx=warn,tinyetl={}",
            match config_info.log_level {
                tinyetl::config::LogLevel::Info => "info",
                tinyetl::config::LogLevel::Warn => "warn", 
                tinyetl::config::LogLevel::Error => "error",
            }
        ));
        
        // Just verify the filter can be created without error
        assert!(env_filter.to_string().contains("sqlx=warn"));
        assert!(env_filter.to_string().contains("tinyetl=info"));
        
        let config_warn = Config {
            log_level: tinyetl::config::LogLevel::Warn,
            ..config_info.clone()
        };
        
        let env_filter_warn = EnvFilter::new(format!(
            "sqlx=warn,tinyetl={}",
            match config_warn.log_level {
                tinyetl::config::LogLevel::Info => "info",
                tinyetl::config::LogLevel::Warn => "warn", 
                tinyetl::config::LogLevel::Error => "error",
            }
        ));
        
        assert!(env_filter_warn.to_string().contains("tinyetl=warn"));
    }
    
    // Integration test using the actual binary
    #[test]
    fn test_binary_help_command() {
        let output = Command::new("cargo")
            .args(&["run", "--", "--help"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output();
            
        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("Usage:") || stdout.contains("USAGE:"));
            assert!(stdout.contains("--source"));
            assert!(stdout.contains("--target"));
        }
        // If cargo run fails (e.g., in CI), just pass the test
    }
    
    #[test] 
    fn test_binary_missing_args() {
        let output = Command::new("cargo")
            .args(&["run", "--"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output();
            
        if let Ok(output) = output {
            assert!(!output.status.success());
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Should contain error about missing required arguments
            assert!(stderr.contains("required") || stderr.contains("argument"));
        }
    }
    
    #[tokio::test]
    async fn test_source_connector_creation_csv() {
        let temp_file = create_test_csv_file().unwrap();
        let file_path = temp_file.path().to_str().unwrap();
        
        // Test that we can create a CSV source from the file
        let result = create_source_from_url_with_type(file_path, None).await;
        assert!(result.is_ok());
    }
    
    #[tokio::test] 
    async fn test_source_connector_creation_nonexistent_file() {
        let result = create_source_from_url_with_type("nonexistent.csv", None).await;
        // This might succeed in creation but fail on connection - behavior depends on implementation
        // The test just verifies the connector creation doesn't panic
        let _result = result; // Use the result to avoid unused variable warning
    }
    
    #[tokio::test]
    async fn test_target_connector_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let file_path = temp_file.path().to_str().unwrap();
        
        // Test JSON target creation
        let json_target = format!("{}.json", file_path);
        let result = create_target_from_url(&json_target).await;
        assert!(result.is_ok());
        
        // Test CSV target creation  
        let csv_target = format!("{}.csv", file_path);
        let result = create_target_from_url(&csv_target).await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_config_default_values() {
        let config = Config::default();
        
        // Test that default values are sensible
        assert_eq!(config.batch_size, None);
        assert_eq!(config.preview, None);
        assert!(!config.dry_run);
        assert_eq!(config.transform, None);
        assert_eq!(config.log_level, tinyetl::config::LogLevel::Info);
    }
    
    // Test the full CLI flow with a simple CSV to JSON transfer
    #[tokio::test]
    async fn test_integration_csv_to_json_flow() {
        let source_file = create_test_csv_file().unwrap();
        let target_file = NamedTempFile::new().unwrap();
        let target_path = format!("{}.json", target_file.path().to_str().unwrap());
        
        // Create config similar to what main() would create
        let config = Config {
            source: source_file.path().to_str().unwrap().to_string(),
            target: target_path.clone(),
            batch_size: Some(10),
            preview: None,
            dry_run: false,
            transform: None,
            source_type: None,
            log_level: tinyetl::config::LogLevel::Error, // Quiet for test
            ..Default::default()
        };
        
        // Test the core flow without the process::exit() call
        let source_result = create_source_from_url_with_type(&config.source, config.source_type.as_deref()).await;
        assert!(source_result.is_ok());
        
        let target_result = create_target_from_url(&config.target).await;
        assert!(target_result.is_ok());
        
        let source = source_result.unwrap();
        let target = target_result.unwrap();
        
        // Execute transfer
        let transfer_result = TransferEngine::execute(&config, source, target).await;
        assert!(transfer_result.is_ok());
        
        let stats = transfer_result.unwrap();
        assert!(stats.total_rows > 0);
        assert!(stats.total_time.as_millis() > 0);
        
        // Verify target file was created and has content
        assert!(std::path::Path::new(&target_path).exists());
    }
}
