use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::{Config, LogLevel};
use crate::transformer::TransformConfig;

// YAML config file structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlConfig {
    pub version: u32,
    pub source: SourceOrTargetConfig,
    pub target: SourceOrTargetConfig,
    pub options: Option<OptionsConfig>,
}

// YAML config for source or target
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SourceOrTargetConfig {
    pub uri: String,
}

/// YAML config for options
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
    pub transform: Option<TransformConfig>,
    pub source_type: Option<String>,
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

        // Use default options if none provided
        let options = self.options.unwrap_or_default();

        // Execute env var substitution on transform config if present
        let transform_config = match options.transform {
            Some(TransformConfig::File(path)) => {
                TransformConfig::File(Self::substitute_env_vars(&path)?)
            }
            Some(TransformConfig::Script(script)) => {
                TransformConfig::Script(Self::substitute_env_vars(&script)?)
            }
            Some(TransformConfig::Inline(expr)) => {
                TransformConfig::Inline(Self::substitute_env_vars(&expr)?)
            }
            Some(TransformConfig::None) | None => TransformConfig::None,
        };

        // Execute env var substitution on schema_file if present
        let schema_file = if let Some(ref file) = options.schema_file {
            Some(Self::substitute_env_vars(file)?)
        } else {
            None
        };

        // Execute env var substitution on source_type if present
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_source_or_target_config() {
        let config = SourceOrTargetConfig::default();
        assert_eq!(config.uri, String::new());
    }

    #[test]
    fn test_serialize_source_or_target_config() {
        let config = SourceOrTargetConfig {
            uri: "file://tmp/file.txt".to_string(),
        };

        let serialized = serde_yaml::to_string(&config).unwrap();
        assert!(serialized.contains("uri: file://tmp/file.txt"));

        let deserialized: SourceOrTargetConfig = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.uri, "file://tmp/file.txt");
    }

    #[test]
    fn test_yaml_config_serialization() {
        let yaml_config = YamlConfig {
            version: 1,
            source: SourceOrTargetConfig {
                uri: "file://source_uri".to_string(),
            },
            target: SourceOrTargetConfig {
                uri: "file://target_uri".to_string(),
            },
            options: Some(OptionsConfig {
                batch_size: Some(5000),
                infer_schema: Some(true),
                schema_file: Some("schema.yaml".to_string()),
                preview: Some(10),
                dry_run: Some(false),
                log_level: Some(LogLevel::Warn),
                skip_existing: Some(true),
                truncate: Some(false),
                transform: Some(TransformConfig::Script("transform_script".to_string())),
                source_type: Some("csv".to_string()),
            }),
        };
        let expected_yaml = r#"version: 1
source:
  uri: file://source_uri
target:
  uri: file://target_uri
options:
  batch_size: 5000
  infer_schema: true
  schema_file: schema.yaml
  preview: 10
  dry_run: false
  log_level: warn
  skip_existing: true
  truncate: false
  transform:
    type: script
    value: transform_script
  source_type: csv
"#;
        let serialized = serde_yaml::to_string(&yaml_config).unwrap();

        assert_eq!(serialized, expected_yaml);
    }

    #[test]
    fn test_yaml_deserialization() {
        let yaml_str = r##"version: 1

source:
  uri: "employees.csv"          # or database connection string

target:
  uri: "employees_output.json"  # or database connection string

options:
  batch_size: 10000               # Number of rows per batch
  infer_schema: true              # Auto-detect column types
  schema_file: "schema path.yaml" # Override with external schema
  preview: 10                     # Show N rows without transfer
  dry_run: false                  # Validate without transferring
  log_level: info                 # info, warn, error (lowercase in YAML)
  skip_existing: false            # Skip if target exists
  source_type: "csv"              # Force source file type
  truncate: false                 # Truncate target before writing
  transform:                      # Inline Lua script transformation
    type: script
    value: |
      -- Calculate derived fields
      full_name = row.first_name .. " " .. row.last_name
      annual_salary = row.monthly_salary * 12
      hire_year = tonumber(string.sub(row.hire_date, 1, 4))
"##;

        let yaml_config: YamlConfig = serde_yaml::from_str(yaml_str).unwrap();
        let options = yaml_config.options.unwrap();

        // test all the fields
        assert_eq!(yaml_config.version, 1);
        assert_eq!(yaml_config.source.uri, "employees.csv");
        assert_eq!(yaml_config.target.uri, "employees_output.json");
        assert_eq!(options.batch_size.unwrap(), 10000);
        assert!(options.infer_schema.unwrap());
        assert_eq!(options.schema_file.unwrap(), "schema path.yaml");
        assert_eq!(options.preview.unwrap(), 10);
        assert!(!options.dry_run.unwrap());
        assert_eq!(options.log_level.unwrap(), LogLevel::Info);
        assert!(!options.skip_existing.unwrap());
        assert_eq!(options.source_type.unwrap(), "csv");
        assert!(!options.truncate.unwrap());
        assert!(options.transform.is_some());

        let expected_transform_config = TransformConfig::Script(
            r##"-- Calculate derived fields
full_name = row.first_name .. " " .. row.last_name
annual_salary = row.monthly_salary * 12
hire_year = tonumber(string.sub(row.hire_date, 1, 4))
"##
            .to_string(),
        );
        assert_eq!(options.transform.unwrap(), expected_transform_config);
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
