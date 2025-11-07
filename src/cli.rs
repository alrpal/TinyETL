use clap::Parser;
use crate::config::{Config, LogLevel};

#[derive(Parser)]
#[command(name = "tinyetl")]
#[command(about = "A tiny ETL tool for moving data between sources")]
#[command(version)]
pub struct Cli {
    /// Source connection string (file path or connection string)
    pub source: String,

    /// Target connection string (file path or connection string)
    pub target: String,

    /// Auto-detect columns and types
    #[arg(long, default_value = "true")]
    pub infer_schema: bool,

    /// Number of rows per batch
    #[arg(long, default_value = "10000")]
    pub batch_size: usize,

    /// Show first N rows and inferred schema without copying
    #[arg(long, value_name = "N")]
    pub preview: Option<usize>,

    /// Validate source/target without transferring data
    #[arg(long)]
    pub dry_run: bool,

    /// Log level: info, warn, error
    #[arg(long, default_value = "info")]
    pub log_level: LogLevel,

    /// Skip rows already in target if primary key detected
    #[arg(long)]
    pub skip_existing: bool,
}

impl From<Cli> for Config {
    fn from(cli: Cli) -> Self {
        Config {
            source: cli.source,
            target: cli.target,
            infer_schema: cli.infer_schema,
            batch_size: cli.batch_size,
            preview: cli.preview,
            dry_run: cli.dry_run,
            log_level: cli.log_level,
            skip_existing: cli.skip_existing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_basic_cli_parsing() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "source.csv",
            "target.db#table"
        ]).unwrap();

        assert_eq!(cli.source, "source.csv");
        assert_eq!(cli.target, "target.db#table");
        assert_eq!(cli.batch_size, 10000);
        assert!(cli.infer_schema);
        assert!(!cli.dry_run);
        assert!(!cli.skip_existing);
    }

    #[test]
    fn test_cli_with_options() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "source.json",
            "target.csv",
            "--batch-size", "5000",
            "--preview", "10",
            "--dry-run",
            "--log-level", "warn",
            "--skip-existing"
        ]).unwrap();

        assert_eq!(cli.source, "source.json");
        assert_eq!(cli.target, "target.csv");
        assert_eq!(cli.batch_size, 5000);
        assert_eq!(cli.preview, Some(10));
        assert!(cli.dry_run);
        assert!(cli.skip_existing);
        assert!(matches!(cli.log_level, LogLevel::Warn));
    }

    #[test]
    fn test_cli_to_config_conversion() {
        let cli = Cli::try_parse_from(&[
            "tinyetl",
            "input.csv",
            "output.db#data",
            "--batch-size", "2000",
            "--preview", "5"
        ]).unwrap();

        let config: Config = cli.into();
        assert_eq!(config.source, "input.csv");
        assert_eq!(config.target, "output.db#data");
        assert_eq!(config.batch_size, 2000);
        assert_eq!(config.preview, Some(5));
    }

    #[test]
    fn test_missing_arguments() {
        let result = Cli::try_parse_from(&["tinyetl"]);
        assert!(result.is_err());
        
        let result = Cli::try_parse_from(&["tinyetl", "source.csv"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_log_level() {
        let result = Cli::try_parse_from(&[
            "tinyetl",
            "source.csv",
            "target.db",
            "--log-level", "invalid"
        ]);
        assert!(result.is_err());
    }
}
