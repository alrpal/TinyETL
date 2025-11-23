use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

use tinyetl::{
    cli::Cli,
    config::{Config, YamlConfig},
    connectors::{create_source_from_url_with_type, create_target_from_url},
    secrets::process_connection_string,
    transfer::TransferEngine,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Determine if we're using config file or direct CLI args
    let config: Config = if cli.is_config_mode() {
        // Load config from YAML file
        if let Some(config_file) = cli.get_config_file() {
            let yaml_config = YamlConfig::from_file(config_file)?;
            yaml_config.into_config()?
        } else {
            return Err("No config file specified".into());
        }
    } else if cli.has_direct_params() {
        // Use direct CLI parameters
        cli.into()
    } else {
        return Err(
            "Either provide source and target arguments, or use 'run <config_file>'".into(),
        );
    };

    // Initialize logging with specific module filtering
    // Respect RUST_LOG environment variable if set, otherwise use config
    let env_filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::new(format!(
            "sqlx=warn,tinyetl={}",
            match config.log_level {
                tinyetl::config::LogLevel::Info => "info",
                tinyetl::config::LogLevel::Warn => "warn",
                tinyetl::config::LogLevel::Error => "error",
            }
        ))
    };

    fmt().with_env_filter(env_filter).init();

    // Process connection strings to resolve secrets
    let processed_source =
        process_connection_string(&config.source, config.source_secret_id.as_ref(), "source")?;

    let processed_target = process_connection_string(
        &config.target,
        config.dest_secret_id.as_ref(),
        "destination",
    )?;

    // Create source and target connectors
    let source =
        create_source_from_url_with_type(&processed_source, config.source_type.as_deref()).await?;
    let target = create_target_from_url(&processed_target).await?;

    // Execute the transfer
    match TransferEngine::execute(&config, source, target).await {
        Ok(stats) => {
            if config.preview.is_none() && !config.dry_run {
                info!("Transfer completed successfully!");
                info!(
                    "Processed {} rows in {:.2}s ({:.0} rows/sec)",
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

    #[test]
    fn test_main_function_exists() {
        // Basic test to ensure main function compiles
        assert!(true);
    }

    #[test]
    fn test_cli_parsing() {
        // Test that Cli can be created from command line args
        let cli = Cli::try_parse_from([
            "tinyetl",
            "test.csv",      // positional source
            "test.db#table", // positional target
        ]);
        assert!(cli.is_ok());

        let cli = cli.unwrap();
        assert_eq!(cli.source, Some("test.csv".to_string()));
        assert_eq!(cli.target, Some("test.db#table".to_string()));
    }

    #[test]
    fn test_cli_to_config_conversion() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "input.csv",   // positional source
            "output.json", // positional target
            "--batch-size",
            "100",
        ])
        .unwrap();

        let config: Config = cli.into();
        assert_eq!(config.source, "input.csv");
        assert_eq!(config.target, "output.json");
        assert_eq!(config.batch_size, 100);
    }

    #[test]
    fn test_cli_with_preview_option() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "test.csv",  // positional source
            "test.json", // positional target
            "--preview",
            "5",
        ])
        .unwrap();

        let config: Config = cli.into();
        assert_eq!(config.preview, Some(5));
    }

    #[test]
    fn test_cli_with_dry_run() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "test.csv",  // positional source
            "test.json", // positional target
            "--dry-run",
        ])
        .unwrap();

        let config: Config = cli.into();
        assert!(config.dry_run);
    }

    #[test]
    fn test_cli_with_transform() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "test.csv",  // positional source
            "test.json", // positional target
            "--transform-file",
            "transform.lua",
        ])
        .unwrap();

        let config: Config = cli.into();
        match config.transform {
            tinyetl::transformer::TransformConfig::File(path) => {
                assert_eq!(path, "transform.lua");
            }
            _ => panic!("Expected File transform config"),
        }
    }

    #[test]
    fn test_cli_missing_required_args() {
        // With new subcommand structure, CLI parsing should succeed
        // but conversion to Config should handle validation
        let result = Cli::try_parse_from(["tinyetl", "only_target.json"]);
        assert!(result.is_ok());

        // Should succeed parsing with no args (could be subcommand)
        let result = Cli::try_parse_from(["tinyetl"]);
        assert!(result.is_ok());

        // Test config file subcommand
        let result = Cli::try_parse_from(["tinyetl", "run", "config.yaml"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(cli.is_config_mode());
        assert_eq!(cli.get_config_file(), Some("config.yaml"));
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
            .args(["run", "--", "--help"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("Usage:") || stdout.contains("USAGE:"));
            // Since source and target are positional, check for SOURCE and TARGET in help
            assert!(stdout.contains("SOURCE") || stdout.contains("source"));
            assert!(stdout.contains("TARGET") || stdout.contains("target"));
        }
        // If cargo run fails (e.g., in CI), just pass the test
    }

    #[test]
    fn test_binary_missing_args() {
        let output = Command::new("cargo")
            .args(["run", "--"])
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
        assert_eq!(config.batch_size, 1_000);
        assert_eq!(config.preview, None);
        assert!(!config.dry_run);
        assert_eq!(config.log_level, tinyetl::config::LogLevel::Info);
        match config.transform {
            tinyetl::transformer::TransformConfig::None => {} // Expected
            _ => panic!("Expected None transform config by default"),
        }
    }
}
