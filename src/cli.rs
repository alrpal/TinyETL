use crate::config::{Config, LogLevel};
use crate::transformer::TransformConfig;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tinyetl")]
#[command(about = "A tiny ETL tool for moving data between sources")]
#[command(version)]
#[command(args_conflicts_with_subcommands = true)]
#[command(subcommand_precedence_over_arg = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Source connection string (file path or connection string)
    pub source: Option<String>,

    /// Target connection string (file path or connection string)
    pub target: Option<String>,

    /// Auto-detect columns and types
    #[arg(long, default_value = "true")]
    pub infer_schema: bool,

    /// Path to schema file (YAML) to override auto-detection
    #[arg(long, value_name = "FILE")]
    pub schema_file: Option<String>,

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

    /// Not implemented yet; has no effect
    #[arg(long)]
    pub skip_existing: bool,

    /// Truncate target before writing (overrides append-first behavior)
    #[arg(long)]
    pub truncate: bool,

    /// Path to Lua file containing a 'transform' function
    #[arg(long, value_name = "FILE")]
    pub transform_file: Option<String>,

    /// Inline transformation expressions (semicolon-separated, e.g., "new_col=row.old_col * 2; name=row.first .. ' ' .. row.last")
    #[arg(long, value_name = "EXPRESSIONS")]
    pub transform: Option<String>,

    /// Force source file type (csv, json, parquet, avro, xlsx, ...) - useful for URLs without clear extensions
    #[arg(long, value_name = "TYPE")]
    pub source_type: Option<String>,

    /// Secret ID for source password (resolves to TINYETL_SECRET_{id})
    #[arg(long, value_name = "ID")]
    pub source_secret_id: Option<String>,

    /// Secret ID for destination password (resolves to TINYETL_SECRET_{id})
    #[arg(long, value_name = "ID")]
    pub dest_secret_id: Option<String>,
    /// Source option, repeatable (e.g. delimiter=tab); the same keys as `options:` in YAML
    #[arg(long = "source-option", value_name = "KEY=VALUE", value_parser = parse_key_value)]
    pub source_options: Vec<(String, String)>,

    /// Target option, repeatable (e.g. delimiter=|); the same keys as `options:` in YAML
    #[arg(long = "target-option", value_name = "KEY=VALUE", value_parser = parse_key_value)]
    pub target_options: Vec<(String, String)>,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once at startup
pub enum Commands {
    /// Run a job from a YAML configuration file
    Run {
        /// Path to the YAML configuration file
        config_file: String,
    },
    /// Generate a default YAML configuration example and output to STDOUT
    GenerateDefaultConfig,
    /// Generate a YAML configuration file from CLI arguments and output to STDOUT
    #[command(override_usage = "tinyetl generate-config [OPTIONS] <SOURCE> <TARGET>")]
    GenerateConfig {
        /// Source connection string (file path or connection string)
        source: String,

        /// Target connection string (file path or connection string)
        target: String,

        /// Auto-detect columns and types
        #[arg(long, default_value = "true")]
        infer_schema: bool,

        /// Path to schema file (YAML) to override auto-detection
        #[arg(long, value_name = "FILE")]
        schema_file: Option<String>,

        /// Number of rows per batch
        #[arg(long, default_value = "10000")]
        batch_size: usize,

        /// Show first N rows and inferred schema without copying
        #[arg(long, value_name = "N")]
        preview: Option<usize>,

        /// Validate source/target without transferring data
        #[arg(long)]
        dry_run: bool,

        /// Log level: info, warn, error
        #[arg(long, default_value = "info")]
        log_level: LogLevel,

        /// Not implemented yet; has no effect
        #[arg(long)]
        skip_existing: bool,

        /// Truncate target before writing (overrides append-first behavior)
        #[arg(long)]
        truncate: bool,

        /// Path to Lua file containing a 'transform' function
        #[arg(long, value_name = "FILE")]
        transform_file: Option<String>,

        /// Inline transformation expressions (semicolon-separated)
        #[arg(long, value_name = "EXPRESSIONS")]
        transform: Option<String>,

        /// Force source file type (csv, json, parquet, avro, xlsx, ...)
        #[arg(long, value_name = "TYPE")]
        source_type: Option<String>,

        /// Secret ID for source password (resolves to TINYETL_SECRET_{id})
        #[arg(long, value_name = "ID")]
        source_secret_id: Option<String>,

        /// Secret ID for destination password (resolves to TINYETL_SECRET_{id})
        #[arg(long, value_name = "ID")]
        dest_secret_id: Option<String>,

        /// Source option, repeatable (e.g. delimiter=tab)
        #[arg(long = "source-option", value_name = "KEY=VALUE", value_parser = parse_key_value)]
        source_options: Vec<(String, String)>,

        /// Target option, repeatable (e.g. delimiter=|)
        #[arg(long = "target-option", value_name = "KEY=VALUE", value_parser = parse_key_value)]
        target_options: Vec<(String, String)>,
    },
}

/// Parse a `KEY=VALUE` option; the value may itself contain `=`
pub fn parse_key_value(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_string(), value.to_string()))
        }
        _ => Err(format!("expected KEY=VALUE, got {:?}", s)),
    }
}

impl Cli {
    /// Check if this CLI call is for running a config file
    pub fn is_config_mode(&self) -> bool {
        matches!(self.command, Some(Commands::Run { .. }))
    }

    /// Check if this CLI call is for generating a config file
    pub fn is_generate_config_mode(&self) -> bool {
        matches!(self.command, Some(Commands::GenerateConfig { .. }))
    }

    /// Check if this CLI call is for generating a default config file
    pub fn is_generate_default_config_mode(&self) -> bool {
        matches!(self.command, Some(Commands::GenerateDefaultConfig))
    }

    /// Get the config file path if in config mode
    pub fn get_config_file(&self) -> Option<&str> {
        match &self.command {
            Some(Commands::Run { config_file }) => Some(config_file),
            _ => None,
        }
    }

    /// Check if we have both source and target for direct mode
    pub fn has_direct_params(&self) -> bool {
        self.source.is_some() && self.target.is_some()
    }
}

impl From<Cli> for Config {
    fn from(cli: Cli) -> Self {
        // Ensure we have both source and target for direct CLI usage
        let source = cli.source.expect("Source is required for direct CLI usage");
        let target = cli.target.expect("Target is required for direct CLI usage");

        // Determine transformation config
        let transform_config = match (&cli.transform_file, &cli.transform) {
            (Some(file), None) => TransformConfig::File(file.clone()),
            (None, Some(expressions)) => TransformConfig::Inline(expressions.clone()),
            (Some(file), Some(_)) => {
                eprintln!("Warning: Both --transform-file and --transform specified. Using --transform-file.");
                TransformConfig::File(file.clone())
            }
            (None, None) => TransformConfig::None,
        };

        Config {
            source,
            target,
            infer_schema: cli.infer_schema,
            schema_file: cli.schema_file,
            batch_size: cli.batch_size,
            preview: cli.preview,
            dry_run: cli.dry_run,
            log_level: cli.log_level,
            skip_existing: cli.skip_existing,
            truncate: cli.truncate,
            transform: transform_config,
            source_type: cli.source_type,
            source_secret_id: cli.source_secret_id,
            dest_secret_id: cli.dest_secret_id,
            source_options: cli.source_options.into_iter().collect(),
            target_options: cli.target_options.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_basic_cli_parsing() {
        let cli = Cli::try_parse_from(["tinyetl", "source.csv", "target.db#table"]).unwrap();

        assert_eq!(cli.source, Some("source.csv".to_string()));
        assert_eq!(cli.target, Some("target.db#table".to_string()));
        assert_eq!(cli.batch_size, 10000);
        assert!(cli.infer_schema);
        assert!(!cli.dry_run);
        assert!(!cli.skip_existing);
        assert!(cli.has_direct_params());
        assert!(!cli.is_config_mode());
    }

    #[test]
    fn test_config_file_parsing() {
        let cli = Cli::try_parse_from(["tinyetl", "run", "my_job.yaml"]).unwrap();

        assert!(cli.is_config_mode());
        assert_eq!(cli.get_config_file(), Some("my_job.yaml"));
        assert!(!cli.has_direct_params());
    }

    #[test]
    fn test_cli_with_options() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "source.json",
            "target.csv",
            "--batch-size",
            "5000",
            "--preview",
            "10",
            "--dry-run",
            "--log-level",
            "warn",
            "--skip-existing",
            "--source-type",
            "json",
        ])
        .unwrap();

        assert_eq!(cli.source, Some("source.json".to_string()));
        assert_eq!(cli.target, Some("target.csv".to_string()));
        assert_eq!(cli.batch_size, 5000);
        assert_eq!(cli.preview, Some(10));
        assert!(cli.dry_run);
        assert!(cli.skip_existing);
        assert!(matches!(cli.log_level, LogLevel::Warn));
        assert_eq!(cli.source_type, Some("json".to_string()));
    }

    #[test]
    fn test_cli_to_config_conversion() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "input.csv",
            "output.db#data",
            "--batch-size",
            "2000",
            "--preview",
            "5",
        ])
        .unwrap();

        let config: Config = cli.into();
        assert_eq!(config.source, "input.csv");
        assert_eq!(config.target, "output.db#data");
        assert_eq!(config.batch_size, 2000);
        assert_eq!(config.preview, Some(5));
    }

    #[test]
    fn test_missing_arguments() {
        // Should still work without source/target for subcommands
        let result = Cli::try_parse_from(["tinyetl"]);
        assert!(result.is_ok());

        // But should fail when trying to convert to Config without source/target
        // This would panic when converting to Config, but that's expected behavior
    }

    #[test]
    fn test_invalid_log_level() {
        let result = Cli::try_parse_from([
            "tinyetl",
            "source.csv",
            "target.db",
            "--log-level",
            "invalid",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_http_source_with_type() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "https://example.com/api/data",
            "output.csv",
            "--source-type",
            "json",
        ])
        .unwrap();

        assert_eq!(cli.source, Some("https://example.com/api/data".to_string()));
        assert_eq!(cli.target, Some("output.csv".to_string()));
        assert_eq!(cli.source_type, Some("json".to_string()));

        let config: Config = cli.into();
        assert_eq!(config.source_type, Some("json".to_string()));
    }

    #[test]
    fn test_generate_default_config_parsing() {
        let cli = Cli::try_parse_from(["tinyetl", "generate-default-config"]).unwrap();

        assert!(cli.is_generate_default_config_mode());
        assert!(!cli.is_config_mode());
        assert!(!cli.is_generate_config_mode());
        assert!(!cli.has_direct_params());
    }

    #[test]
    fn test_endpoint_options() {
        let cli = Cli::try_parse_from([
            "tinyetl",
            "in.csv",
            "out.csv",
            "--source-option",
            "delimiter=;",
            "--source-option",
            "header.X-Token=a=b",
            "--target-option",
            "delimiter=tab",
        ])
        .unwrap();
        let config: Config = cli.into();
        assert_eq!(config.source_options["delimiter"], ";");
        assert_eq!(config.source_options["header.X-Token"], "a=b");
        assert_eq!(config.target_options["delimiter"], "tab");

        assert!(
            Cli::try_parse_from(["tinyetl", "a.csv", "b.csv", "--source-option", "nokey"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["tinyetl", "a.csv", "b.csv", "--source-option", "=x"]).is_err()
        );
    }
}
