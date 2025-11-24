use crate::transformer::TransformConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// YAML config file structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlConfig {
    pub version: u32,
    pub source: SourceConfig,
    pub target: TargetConfig,
    pub options: Option<OptionsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub uri: String,
    #[serde(flatten)]
    pub extra_params: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub uri: String,
    #[serde(flatten)]
    pub extra_params: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OptionsConfig {
    pub batch_size: Option<usize>,
    pub infer_schema: Option<bool>,
    pub schema_file: Option<String>,
    pub preview: Option<usize>,
    pub dry_run: Option<bool>,
    pub log_level: Option<LogLevel>,
    pub skip_existing: Option<bool>,
    pub truncate: Option<bool>,
    pub transform: Option<String>,
    pub transform_file: Option<String>,
    pub source_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub source: String,
    pub target: String,
    pub infer_schema: bool,
    pub schema_file: Option<String>,
    pub batch_size: usize,
    pub preview: Option<usize>,
    pub dry_run: bool,
    pub log_level: LogLevel,
    pub skip_existing: bool,
    pub truncate: bool,
    #[serde(skip)] // Skip serialization as TransformConfig doesn't implement Serialize
    pub transform: TransformConfig,
    pub source_type: Option<String>,
    pub source_secret_id: Option<String>,
    pub dest_secret_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            infer_schema: true,
            schema_file: None,
            batch_size: 1_000, // Reduced from 10k to 1k for better memory usage with transactions
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::None,
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
        }
    }
}

impl YamlConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: YamlConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn into_config(self) -> Result<Config, Box<dyn std::error::Error>> {
        // Process environment variable substitution in URIs and other fields
        let source_uri = Self::substitute_env_vars(&self.source.uri)?;
        let target_uri = Self::substitute_env_vars(&self.target.uri)?;

        // Handle transform config with env var substitution
        let transform_config = if let Some(options) = &self.options {
            match (&options.transform_file, &options.transform) {
                (Some(file), None) => {
                    let processed_file = Self::substitute_env_vars(file)?;
                    TransformConfig::File(processed_file)
                }
                (None, Some(script)) => {
                    let processed_script = Self::substitute_env_vars(script)?;
                    TransformConfig::Script(processed_script)
                }
                (Some(file), Some(_)) => {
                    eprintln!("Warning: Both transform_file and transform specified. Using transform_file.");
                    let processed_file = Self::substitute_env_vars(file)?;
                    TransformConfig::File(processed_file)
                }
                (None, None) => TransformConfig::None,
            }
        } else {
            TransformConfig::None
        };

        let options = self.options.unwrap_or_default();

        // Process schema_file path if present
        let schema_file = if let Some(ref file) = options.schema_file {
            Some(Self::substitute_env_vars(file)?)
        } else {
            None
        };

        // Process source_type with env vars if present
        let source_type = if let Some(ref stype) = options.source_type {
            Some(Self::substitute_env_vars(stype)?)
        } else {
            None
        };

        Ok(Config {
            source: source_uri,
            target: target_uri,
            infer_schema: options.infer_schema.unwrap_or(true),
            schema_file,
            batch_size: options.batch_size.unwrap_or(10_000),
            preview: options.preview,
            dry_run: options.dry_run.unwrap_or(false),
            log_level: options.log_level.unwrap_or(LogLevel::Info),
            skip_existing: options.skip_existing.unwrap_or(false),
            truncate: options.truncate.unwrap_or(false),
            transform: transform_config,
            source_type,
            source_secret_id: None, // Not used with config files - env vars are substituted directly
            dest_secret_id: None, // Not used with config files - env vars are substituted directly
        })
    }

    /// Substitute environment variable patterns like ${VAR_NAME} in strings
    fn substitute_env_vars(input: &str) -> Result<String, Box<dyn std::error::Error>> {
        let env_var_pattern = Regex::new(r"\$\{([^}]+)\}")?;
        let mut result = input.to_string();

        for caps in env_var_pattern.captures_iter(input) {
            if let Some(var_name) = caps.get(1) {
                let var_name_str = var_name.as_str();
                let env_value = std::env::var(var_name_str)
                    .map_err(|_| format!("Environment variable '{}' not found", var_name_str))?;

                let pattern = format!("${{{}}}", var_name_str);
                result = result.replace(&pattern, &env_value);
            }
        }

        Ok(result)
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(LogLevel::Info),
            "warn" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err("Invalid log level. Valid values: info, warn, error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.batch_size, 1_000);
        assert!(config.infer_schema);
        assert!(!config.dry_run);
        assert!(!config.skip_existing);
        assert!(!config.truncate);
        assert!(matches!(config.log_level, LogLevel::Info));
    }

    #[test]
    fn test_log_level_parsing() {
        assert!(matches!("info".parse::<LogLevel>(), Ok(LogLevel::Info)));
        assert!(matches!("WARN".parse::<LogLevel>(), Ok(LogLevel::Warn)));
        assert!(matches!("Error".parse::<LogLevel>(), Ok(LogLevel::Error)));
        assert!("invalid".parse::<LogLevel>().is_err());
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(LogLevel::Info.to_string(), "info");
        assert_eq!(LogLevel::Warn.to_string(), "warn");
        assert_eq!(LogLevel::Error.to_string(), "error");
    }

    #[test]
    fn test_env_var_substitution() {
        // Set test environment variable
        std::env::set_var("TEST_VAR", "test_value");
        std::env::set_var("DB_PASSWORD", "secret123");

        // Test simple substitution
        let result = YamlConfig::substitute_env_vars("${TEST_VAR}").unwrap();
        assert_eq!(result, "test_value");

        // Test substitution within a string
        let result =
            YamlConfig::substitute_env_vars("mysql://user:${DB_PASSWORD}@localhost/db").unwrap();
        assert_eq!(result, "mysql://user:secret123@localhost/db");

        // Test multiple substitutions
        let result = YamlConfig::substitute_env_vars("${TEST_VAR}_${DB_PASSWORD}").unwrap();
        assert_eq!(result, "test_value_secret123");

        // Test no substitution needed
        let result = YamlConfig::substitute_env_vars("no_env_vars_here").unwrap();
        assert_eq!(result, "no_env_vars_here");

        // Clean up
        std::env::remove_var("TEST_VAR");
        std::env::remove_var("DB_PASSWORD");
    }

    #[test]
    fn test_env_var_substitution_missing_var() {
        let result = YamlConfig::substitute_env_vars("${MISSING_VAR}");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Environment variable 'MISSING_VAR' not found"));
    }
}
