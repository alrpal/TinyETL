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
    // Integration tests would go here
    #[test]
    fn test_main_function_exists() {
        // Basic test to ensure main function compiles
        assert!(true);
    }
}
