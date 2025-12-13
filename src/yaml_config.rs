use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[serde(default)]
    pub options: HashMap<String, String>,
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

    /// Convert a Config struct to a YamlConfig struct
    pub fn from_config(config: Config) -> Self {
        YamlConfig {
            version: 1,
            source: SourceOrTargetConfig {
                uri: config.source,
                options: config.source_options,
            },
            target: SourceOrTargetConfig {
                uri: config.target,
                options: config.target_options,
            },
            options: Some(OptionsConfig {
                batch_size: Some(config.batch_size),
                infer_schema: Some(config.infer_schema),
                schema_file: config.schema_file,
                preview: config.preview,
                dry_run: Some(config.dry_run),
                log_level: Some(config.log_level),
                skip_existing: Some(config.skip_existing),
                truncate: Some(config.truncate),
                transform: match config.transform {
                    TransformConfig::None => None,
                    other => Some(other),
                },
                source_type: config.source_type,
            }),
        }
    }

    /// Serialize this YamlConfig to a YAML string
    pub fn to_yaml_string(&self) -> Result<String, Box<dyn std::error::Error>> {
        let yaml = serde_yaml::to_string(self)?;
        Ok(yaml)
    }

    pub fn into_config(self) -> Result<Config, Box<dyn std::error::Error>> {
        // Process environment variable substitution in URIs and other fields
        let source_uri = Self::substitute_env_vars(&self.source.uri)?;
        let target_uri = Self::substitute_env_vars(&self.target.uri)?;

        // Process environment variable substitution in source and target options
        let source_options = Self::substitute_env_vars_in_map(&self.source.options)?;
        let target_options = Self::substitute_env_vars_in_map(&self.target.options)?;

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
            source_options,
            target_options,
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

    /// Substitute environment variables in all values of a HashMap
    fn substitute_env_vars_in_map(
        input: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
        let mut result = HashMap::new();
        for (key, value) in input {
            result.insert(key.clone(), Self::substitute_env_vars(value)?);
        }
        Ok(result)
    }
}

impl From<Config> for YamlConfig {
    fn from(config: Config) -> Self {
        YamlConfig::from_config(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_source_or_target_config() {
        let config = SourceOrTargetConfig::default();
        assert_eq!(config.uri, String::new());
        assert!(config.options.is_empty());
    }

    #[test]
    fn test_serialize_source_or_target_config() {
        let config = SourceOrTargetConfig {
            uri: "file://tmp/file.txt".to_string(),
            options: HashMap::new(),
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
                options: HashMap::new(),
            },
            target: SourceOrTargetConfig {
                uri: "file://target_uri".to_string(),
                options: HashMap::new(),
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
  options: {}
target:
  uri: file://target_uri
  options: {}
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

    // Round-trip tests: CLI args → generate-config → YAML → run config → verify results match direct CLI

    #[test]
    fn test_round_trip_basic_config() {
        // Step 1: Create a Config as if it came from CLI args
        let original_config = Config {
            source: "input.csv".to_string(),
            target: "output.json".to_string(),
            infer_schema: true,
            schema_file: None,
            batch_size: 5000,
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::None,
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        // Step 2: Convert to YamlConfig (simulate generate-config)
        let yaml_config = YamlConfig::from_config(original_config.clone());

        // Step 3: Serialize to YAML string
        let yaml_string = yaml_config.to_yaml_string().unwrap();

        // Verify YAML contains expected content
        assert!(yaml_string.contains("version: 1"));
        assert!(yaml_string.contains("uri: input.csv"));
        assert!(yaml_string.contains("uri: output.json"));
        assert!(yaml_string.contains("batch_size: 5000"));

        // Step 4: Deserialize from YAML string (simulate run config)
        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();

        // Step 5: Convert back to Config
        let restored_config = deserialized_yaml.into_config().unwrap();

        // Step 6: Verify all fields match the original
        assert_eq!(restored_config.source, original_config.source);
        assert_eq!(restored_config.target, original_config.target);
        assert_eq!(restored_config.infer_schema, original_config.infer_schema);
        assert_eq!(restored_config.schema_file, original_config.schema_file);
        assert_eq!(restored_config.batch_size, original_config.batch_size);
        assert_eq!(restored_config.preview, original_config.preview);
        assert_eq!(restored_config.dry_run, original_config.dry_run);
        assert_eq!(restored_config.log_level, original_config.log_level);
        assert_eq!(restored_config.skip_existing, original_config.skip_existing);
        assert_eq!(restored_config.truncate, original_config.truncate);
        assert_eq!(restored_config.transform, original_config.transform);
        assert_eq!(restored_config.source_type, original_config.source_type);
        // Note: source_secret_id and dest_secret_id are not preserved through YAML (by design)
    }

    #[test]
    fn test_round_trip_config_with_all_options() {
        // Step 1: Create a Config with all options set
        let original_config = Config {
            source: "https://example.com/data.csv".to_string(),
            target: "sqlite://output.db#my_table".to_string(),
            infer_schema: false,
            schema_file: Some("schema.yaml".to_string()),
            batch_size: 2000,
            preview: Some(10),
            dry_run: true,
            log_level: LogLevel::Warn,
            skip_existing: true,
            truncate: true,
            transform: TransformConfig::Inline("result = row.value * 2".to_string()),
            source_type: Some("csv".to_string()),
            source_secret_id: None, // Not preserved through YAML
            dest_secret_id: None,   // Not preserved through YAML
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        // Step 2: Convert to YamlConfig
        let yaml_config = YamlConfig::from_config(original_config.clone());

        // Step 3: Serialize to YAML string
        let yaml_string = yaml_config.to_yaml_string().unwrap();

        // Verify specific options in YAML
        assert!(yaml_string.contains("infer_schema: false"));
        assert!(yaml_string.contains("schema_file: schema.yaml"));
        assert!(yaml_string.contains("preview: 10"));
        assert!(yaml_string.contains("dry_run: true"));
        assert!(yaml_string.contains("log_level: warn"));
        assert!(yaml_string.contains("skip_existing: true"));
        assert!(yaml_string.contains("truncate: true"));
        assert!(yaml_string.contains("source_type: csv"));
        assert!(yaml_string.contains("type: inline"));

        // Step 4: Deserialize from YAML string
        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();

        // Step 5: Convert back to Config
        let restored_config = deserialized_yaml.into_config().unwrap();

        // Step 6: Verify all fields match
        assert_eq!(restored_config.source, original_config.source);
        assert_eq!(restored_config.target, original_config.target);
        assert_eq!(restored_config.infer_schema, original_config.infer_schema);
        assert_eq!(restored_config.schema_file, original_config.schema_file);
        assert_eq!(restored_config.batch_size, original_config.batch_size);
        assert_eq!(restored_config.preview, original_config.preview);
        assert_eq!(restored_config.dry_run, original_config.dry_run);
        assert_eq!(restored_config.log_level, original_config.log_level);
        assert_eq!(restored_config.skip_existing, original_config.skip_existing);
        assert_eq!(restored_config.truncate, original_config.truncate);
        assert_eq!(restored_config.transform, original_config.transform);
        assert_eq!(restored_config.source_type, original_config.source_type);
    }

    #[test]
    fn test_round_trip_with_transform_file() {
        // Test with File transform
        let original_config = Config {
            source: "data.csv".to_string(),
            target: "output.db#table".to_string(),
            infer_schema: true,
            schema_file: None,
            batch_size: 10000,
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::File("transform.lua".to_string()),
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        let yaml_config = YamlConfig::from_config(original_config.clone());
        let yaml_string = yaml_config.to_yaml_string().unwrap();

        assert!(yaml_string.contains("type: file"));
        assert!(yaml_string.contains("value: transform.lua"));

        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
        let restored_config = deserialized_yaml.into_config().unwrap();

        assert_eq!(restored_config.transform, original_config.transform);
    }

    #[test]
    fn test_round_trip_with_transform_script() {
        // Test with Script transform (multi-line Lua)
        let script = r#"-- Calculate values
total = row.price * row.quantity
discount = total * 0.1"#;

        let original_config = Config {
            source: "sales.csv".to_string(),
            target: "processed.json".to_string(),
            infer_schema: true,
            schema_file: None,
            batch_size: 10000,
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::Script(script.to_string()),
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        let yaml_config = YamlConfig::from_config(original_config.clone());
        let yaml_string = yaml_config.to_yaml_string().unwrap();

        assert!(yaml_string.contains("type: script"));
        assert!(yaml_string.contains("Calculate values"));

        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
        let restored_config = deserialized_yaml.into_config().unwrap();

        assert_eq!(restored_config.transform, original_config.transform);
    }

    #[test]
    fn test_round_trip_with_default_values() {
        // Test that default values are properly restored
        let original_config = Config {
            source: "in.csv".to_string(),
            target: "out.json".to_string(),
            ..Config::default()
        };

        let yaml_config = YamlConfig::from_config(original_config.clone());
        let yaml_string = yaml_config.to_yaml_string().unwrap();
        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
        let restored_config = deserialized_yaml.into_config().unwrap();

        // Verify defaults are preserved
        assert_eq!(restored_config.infer_schema, true);
        assert_eq!(restored_config.batch_size, Config::default().batch_size);
        assert_eq!(restored_config.dry_run, false);
        assert_eq!(restored_config.log_level, LogLevel::Info);
        assert_eq!(restored_config.skip_existing, false);
        assert_eq!(restored_config.truncate, false);
        assert_eq!(restored_config.transform, TransformConfig::None);
    }

    #[test]
    fn test_round_trip_multiple_cycles() {
        // Test that multiple round-trips don't degrade the data
        let mut config = Config {
            source: "input.csv".to_string(),
            target: "output.db#data".to_string(),
            infer_schema: false,
            schema_file: Some("my_schema.yaml".to_string()),
            batch_size: 7500,
            preview: Some(25),
            dry_run: false,
            log_level: LogLevel::Error,
            skip_existing: true,
            truncate: false,
            transform: TransformConfig::Inline("x = row.a + row.b".to_string()),
            source_type: Some("json".to_string()),
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        // Perform 3 round-trips
        for _ in 0..3 {
            let yaml_config = YamlConfig::from_config(config.clone());
            let yaml_string = yaml_config.to_yaml_string().unwrap();
            let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
            config = deserialized_yaml.into_config().unwrap();
        }

        // Verify data is still intact after multiple cycles
        assert_eq!(config.source, "input.csv");
        assert_eq!(config.target, "output.db#data");
        assert_eq!(config.infer_schema, false);
        assert_eq!(config.schema_file, Some("my_schema.yaml".to_string()));
        assert_eq!(config.batch_size, 7500);
        assert_eq!(config.preview, Some(25));
        assert_eq!(config.log_level, LogLevel::Error);
        assert_eq!(config.skip_existing, true);
        assert_eq!(
            config.transform,
            TransformConfig::Inline("x = row.a + row.b".to_string())
        );
        assert_eq!(config.source_type, Some("json".to_string()));
    }

    #[test]
    fn test_round_trip_preserves_special_characters() {
        // Test that special characters in URIs and other fields are preserved
        let original_config = Config {
            source: "postgresql://user:p@ss!w0rd@localhost:5432/db?sslmode=require".to_string(),
            target: "mysql://admin:s3cr3t#123@remote:3306/database".to_string(),
            infer_schema: true,
            schema_file: Some("path/to/my schema file.yaml".to_string()),
            batch_size: 1000,
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::Script(
                "name = row['first-name'] .. ' ' .. row['last-name']".to_string(),
            ),
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        let yaml_config = YamlConfig::from_config(original_config.clone());
        let yaml_string = yaml_config.to_yaml_string().unwrap();
        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
        let restored_config = deserialized_yaml.into_config().unwrap();

        assert_eq!(restored_config.source, original_config.source);
        assert_eq!(restored_config.target, original_config.target);
        assert_eq!(restored_config.schema_file, original_config.schema_file);
        assert_eq!(restored_config.transform, original_config.transform);
    }

    #[test]
    fn test_round_trip_none_transform_handling() {
        // Verify that TransformConfig::None properly round-trips
        let original_config = Config {
            source: "in.csv".to_string(),
            target: "out.json".to_string(),
            infer_schema: true,
            schema_file: None,
            batch_size: 10000,
            preview: None,
            dry_run: false,
            log_level: LogLevel::Info,
            skip_existing: false,
            truncate: false,
            transform: TransformConfig::None,
            source_type: None,
            source_secret_id: None,
            dest_secret_id: None,
            source_options: HashMap::new(),
            target_options: HashMap::new(),
        };

        let yaml_config = YamlConfig::from_config(original_config.clone());
        let yaml_string = yaml_config.to_yaml_string().unwrap();

        // Deserialize and convert back
        let deserialized_yaml: YamlConfig = serde_yaml::from_str(&yaml_string).unwrap();
        let restored_config = deserialized_yaml.into_config().unwrap();

        // Should restore as TransformConfig::None
        assert_eq!(restored_config.transform, TransformConfig::None);

        // Verify all other fields match as well
        assert_eq!(restored_config.source, original_config.source);
        assert_eq!(restored_config.target, original_config.target);
        assert_eq!(restored_config.batch_size, original_config.batch_size);
    }
}
