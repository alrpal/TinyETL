use crate::transformer::TransformConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};

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
    pub transform: TransformConfig,
    pub source_type: Option<String>,
    pub source_secret_id: Option<String>,
    pub dest_secret_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
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
}
