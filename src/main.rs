use clap::Parser;
use tracing::{info, error};
use tracing_subscriber;

use tinyetl::{
    cli::Cli,
    config::Config,
    connectors::{create_source, create_target},
    transfer::TransferEngine,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();
    let config: Config = cli.into();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(match config.log_level {
            tinyetl::config::LogLevel::Info => tracing::Level::INFO,
            tinyetl::config::LogLevel::Warn => tracing::Level::WARN,
            tinyetl::config::LogLevel::Error => tracing::Level::ERROR,
        })
        .init();

    // Create source and target connectors
    let source = create_source(&config.source)?;
    let target = create_target(&config.target)?;

    // Execute the transfer
    match TransferEngine::execute(&config, source, target).await {
        Ok(stats) => {
            if !config.preview.is_some() && !config.dry_run {
                info!("✅ Transfer completed successfully!");
                info!("📊 Processed {} rows in {:.2}s ({:.0} rows/sec)", 
                    stats.total_rows, 
                    stats.total_time.as_secs_f64(),
                    stats.rows_per_second
                );
            }
        }
        Err(e) => {
            error!("❌ Transfer failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Integration tests would go here
    #[test]
    fn test_main_function_exists() {
        // Basic test to ensure main function compiles
        assert!(true);
    }
}
