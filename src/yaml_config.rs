//! `tinyetl run job.yaml`: YAML job files.
//!
//! A job names a `source` and a `target`, each a plain string or `{ uri, options }`. Every
//! other key has the name of a command-line flag (`batch_size` for `--batch-size`,
//! `transform`, `transform_file`, ...). Older files that put those keys under `options:` and
//! write `transform: { type, value }` still load. Unknown keys are an error, so a typo fails
//! instead of being ignored. `${VAR}` and `${VAR:-default}` read environment variables.

use regex::{Captures, Regex};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;

use crate::config::{Config, LogLevel};
use crate::transformer::TransformConfig;

const DEFAULT_BATCH_SIZE: usize = 10_000;

/// The annotated example printed by `tinyetl generate-default-config`; README.md shows the same
/// text (a test checks)
pub const EXAMPLE: &str = r#"# TinyETL job file. Run it with: tinyetl run job.yaml
#
# Only source and target are required. Every other key is optional and has the
# name of a command-line flag (batch_size is --batch-size). Unknown keys are an
# error. ${VAR} inserts an environment variable, ${VAR:-default} a default when
# it isn't set. Generate a job from a command line with:
#   tinyetl generate-config <SOURCE> <TARGET> [OPTIONS] > job.yaml

source: employees.csv                  # a file, URL or database connection string
target: postgres://etl:${DB_PASSWORD:-etl}@localhost/hr#employees

# A source or target with options (see "Endpoint Options" in the README):
# source:
#   uri: https://example.com/export.csv
#   options:
#     auth.bearer: ${API_TOKEN}
#     delimiter: ";"

# source_query: SELECT * FROM orders WHERE paid   # a database source's query, instead of #table
# truncate: true                       # replace the target's rows instead of appending
# batch_size: 10000                    # rows per batch
# schema_file: employees_schema.yaml   # column types and validation, instead of inference
# source_type: csv                     # when the source has no file extension
# preview: 10                          # show 10 rows and stop
# dry_run: true                        # check the source and schema, write nothing
# log_level: warn                      # info, warn or error

# Lua transform: one assignment per line (or separated by ";"). Every column
# is kept; each line adds or replaces one, and can use the ones before it.
transform: |
  full_name = row.first_name .. " " .. row.last_name
  annual_salary = row.monthly_salary * 12
  hire_year = tonumber(string.sub(row.hire_date, 1, 4))

# Or a Lua file with a transform(row) function, which can also drop rows:
# transform_file: transform.lua
"#;

/// A YAML job file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YamlConfig {
    /// Only version 1 exists; it can be left out
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    pub source: SourceOrTargetConfig,
    pub target: SourceOrTargetConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer_schema: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LogLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// SQL to run on a database source instead of reading a whole table
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_query: Option<String>,
    /// Lua assignments (a string), or the older `{ type, value }` form
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    /// A Lua file with a `transform(row)` function
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform_file: Option<String>,
    /// Accepted for older files; has no effect
    #[serde(default, skip_serializing)]
    pub skip_existing: Option<bool>,
    /// The older place for the settings above
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<OptionsConfig>,
}

/// A source or target: `data.csv`, or `{ uri: data.csv, options: { delimiter: ";" } }`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceOrTargetConfig {
    pub uri: String,
    pub options: HashMap<String, String>,
}

/// The `options:` block of older job files
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsConfig {
    pub batch_size: Option<usize>,
    pub infer_schema: Option<bool>,
    pub schema_file: Option<String>,
    pub preview: Option<usize>,
    pub dry_run: Option<bool>,
    pub log_level: Option<LogLevel>,
    pub skip_existing: Option<bool>,
    pub truncate: Option<bool>,
    pub transform: Option<Transform>,
    pub transform_file: Option<String>,
    pub source_type: Option<String>,
}

/// The `transform` key: Lua assignments as a string, or `{ type: script|inline|file|none, value }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transform(pub TransformConfig);

impl YamlConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Can't read config file {}: {}", path, e))?;
        Self::from_yaml(&content).map_err(|e| format!("Invalid config file {}: {}", path, e).into())
    }

    pub fn from_yaml(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config: YamlConfig = serde_yaml::from_str(content)?;
        if let Some(version) = config.version.filter(|v| *v != 1) {
            return Err(format!(
                "unsupported version {}; this TinyETL reads version 1",
                version
            )
            .into());
        }
        Ok(config)
    }

    /// The job file for a command line (`tinyetl generate-config`): only the settings that
    /// differ from the defaults
    pub fn from_config(config: Config) -> Self {
        let (transform, transform_file) = match config.transform {
            TransformConfig::None => (None, None),
            TransformConfig::File(path) => (None, Some(path)),
            other => (Some(Transform(other)), None),
        };
        YamlConfig {
            version: None,
            source: SourceOrTargetConfig {
                uri: config.source,
                options: config.source_options,
            },
            target: SourceOrTargetConfig {
                uri: config.target,
                options: config.target_options,
            },
            batch_size: Some(config.batch_size).filter(|b| *b != DEFAULT_BATCH_SIZE),
            infer_schema: Some(config.infer_schema).filter(|i| !i),
            schema_file: config.schema_file,
            preview: config.preview,
            dry_run: Some(config.dry_run).filter(|d| *d),
            log_level: Some(config.log_level).filter(|l| *l != LogLevel::Info),
            truncate: Some(config.truncate).filter(|t| *t),
            source_type: config.source_type,
            source_query: config.source_query,
            transform,
            transform_file,
            skip_existing: None,
            options: None,
        }
    }

    pub fn to_yaml_string(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_yaml::to_string(self)?)
    }

    pub fn into_config(self) -> Result<Config, Box<dyn std::error::Error>> {
        let old = self.options.unwrap_or_default();

        // A setting may be at the top level or under `options:`, not both
        fn pick<T>(name: &str, top: Option<T>, old: Option<T>) -> Result<Option<T>, String> {
            match (top, old) {
                (Some(_), Some(_)) => Err(format!(
                    "'{}' is set both at the top level and under 'options:'; keep one",
                    name
                )),
                (top, old) => Ok(top.or(old)),
            }
        }
        let batch_size = pick("batch_size", self.batch_size, old.batch_size)?;
        let infer_schema = pick("infer_schema", self.infer_schema, old.infer_schema)?;
        let schema_file = pick("schema_file", self.schema_file, old.schema_file)?;
        let preview = pick("preview", self.preview, old.preview)?;
        let dry_run = pick("dry_run", self.dry_run, old.dry_run)?;
        let log_level = pick("log_level", self.log_level, old.log_level)?;
        let skip_existing = pick("skip_existing", self.skip_existing, old.skip_existing)?;
        let truncate = pick("truncate", self.truncate, old.truncate)?;
        let source_type = pick("source_type", self.source_type, old.source_type)?;
        let transform = pick("transform", self.transform, old.transform)?;
        let transform_file = pick("transform_file", self.transform_file, old.transform_file)?;

        let transform = match (transform, transform_file) {
            (Some(_), Some(_)) => {
                return Err("set either 'transform' or 'transform_file', not both".into())
            }
            (Some(Transform(t)), None) => t,
            (None, Some(path)) => TransformConfig::File(path),
            (None, None) => TransformConfig::None,
        };
        let transform = match transform {
            TransformConfig::File(path) => TransformConfig::File(substitute_env_vars(&path)?),
            TransformConfig::Script(script) => {
                TransformConfig::Script(substitute_env_vars(&script)?)
            }
            TransformConfig::Inline(expr) => TransformConfig::Inline(substitute_env_vars(&expr)?),
            TransformConfig::None => TransformConfig::None,
        };

        Ok(Config {
            source: substitute_env_vars(&self.source.uri)?,
            target: substitute_env_vars(&self.target.uri)?,
            infer_schema: infer_schema.unwrap_or(true),
            schema_file: schema_file.map(|f| substitute_env_vars(&f)).transpose()?,
            batch_size: batch_size.unwrap_or(DEFAULT_BATCH_SIZE),
            preview,
            dry_run: dry_run.unwrap_or(false),
            log_level: log_level.unwrap_or(LogLevel::Info),
            skip_existing: skip_existing.unwrap_or(false),
            truncate: truncate.unwrap_or(false),
            transform,
            source_type: source_type.map(|t| substitute_env_vars(&t)).transpose()?,
            source_query: self
                .source_query
                .map(|q| substitute_env_vars(&q))
                .transpose()?,
            source_secret_id: None, // Not used with config files - env vars are substituted directly
            dest_secret_id: None,
            source_options: substitute_env_vars_in_map(&self.source.options)?,
            target_options: substitute_env_vars_in_map(&self.target.options)?,
        })
    }
}

impl From<Config> for YamlConfig {
    fn from(config: Config) -> Self {
        YamlConfig::from_config(config)
    }
}

/// Replace `${VAR}` with the environment variable's value, and `${VAR:-default}` with the
/// value or, when the variable is unset or empty, the default
fn substitute_env_vars(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    let pattern = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}")?;
    let mut missing = None;
    let result = pattern.replace_all(input, |caps: &Captures| {
        let name = &caps[1];
        match (
            std::env::var(name).ok().filter(|v| !v.is_empty()),
            caps.get(2),
        ) {
            (Some(value), _) => value,
            (None, Some(default)) => default.as_str().to_string(),
            (None, None) => {
                // An empty variable without a default is still a value
                std::env::var(name).unwrap_or_else(|_| {
                    missing.get_or_insert_with(|| name.to_string());
                    String::new()
                })
            }
        }
    });
    if let Some(name) = missing {
        return Err(format!(
            "Environment variable '{}' not found (use ${{{}:-default}} for a default)",
            name, name
        )
        .into());
    }
    Ok(result.into_owned())
}

fn substitute_env_vars_in_map(
    input: &HashMap<String, String>,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    input
        .iter()
        .map(|(key, value)| Ok((key.clone(), substitute_env_vars(value)?)))
        .collect()
}

impl Serialize for SourceOrTargetConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Full<'a> {
            uri: &'a str,
            options: std::collections::BTreeMap<&'a String, &'a String>,
        }
        if self.options.is_empty() {
            serializer.serialize_str(&self.uri)
        } else {
            Full {
                uri: &self.uri,
                options: self.options.iter().collect(),
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for SourceOrTargetConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Full {
            uri: String,
            #[serde(default)]
            options: HashMap<String, String>,
        }

        struct EndpointVisitor;
        impl<'de> Visitor<'de> for EndpointVisitor {
            type Value = SourceOrTargetConfig;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a location such as data.csv, or a map with uri and options")
            }

            fn visit_str<E: de::Error>(self, uri: &str) -> Result<Self::Value, E> {
                Ok(SourceOrTargetConfig {
                    uri: uri.to_string(),
                    options: HashMap::new(),
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let full = Full::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(SourceOrTargetConfig {
                    uri: full.uri,
                    options: full.options,
                })
            }
        }

        deserializer.deserialize_any(EndpointVisitor)
    }
}

impl Serialize for Transform {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            TransformConfig::Script(lua) | TransformConfig::Inline(lua) => {
                serializer.serialize_str(lua)
            }
            other => other.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Transform {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TransformVisitor;
        impl<'de> Visitor<'de> for TransformVisitor {
            type Value = Transform;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("Lua assignments such as \"total = row.qty * row.price\"")
            }

            fn visit_str<E: de::Error>(self, lua: &str) -> Result<Self::Value, E> {
                Ok(Transform(TransformConfig::Script(lua.to_string())))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                TransformConfig::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(Transform)
            }
        }

        deserializer.deserialize_any(TransformVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(yaml: &str) -> Result<Config, String> {
        YamlConfig::from_yaml(yaml)
            .and_then(|c| c.into_config())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn test_example_loads_and_matches_readme() {
        std::env::remove_var("DB_PASSWORD");
        let config = load(EXAMPLE).unwrap();
        assert_eq!(config.source, "employees.csv");
        assert_eq!(config.target, "postgres://etl:etl@localhost/hr#employees");
        assert!(matches!(config.transform, TransformConfig::Script(_)));
        assert!(
            include_str!("../README.md").contains(EXAMPLE),
            "README.md must show yaml_config::EXAMPLE unchanged"
        );
    }

    #[test]
    fn test_minimal_job() {
        let config = load("source: data.csv\ntarget: out.db\n").unwrap();
        assert_eq!(config.source, "data.csv");
        assert_eq!(config.target, "out.db");
        assert_eq!(config.batch_size, DEFAULT_BATCH_SIZE);
        assert!(config.infer_schema);
        assert_eq!(config.transform, TransformConfig::None);
    }

    #[test]
    fn test_flat_job() {
        let config = load(
            r#"
source:
  uri: https://example.com/a.csv
  options:
    auth.bearer: token
target: out.parquet
batch_size: 500
truncate: true
log_level: warn
transform: |
  total = row.qty * row.price
  profit = total * 0.3
"#,
        )
        .unwrap();
        assert_eq!(config.source_options["auth.bearer"], "token");
        assert_eq!(config.batch_size, 500);
        assert!(config.truncate);
        assert_eq!(config.log_level, LogLevel::Warn);
        assert_eq!(
            config.transform,
            TransformConfig::Script(
                "total = row.qty * row.price\nprofit = total * 0.3\n".to_string()
            )
        );

        let config = load(
            "source: postgres://u@h/db\ntarget: out.parquet\nsource_query: |\n  SELECT id\n  FROM orders\n",
        )
        .unwrap();
        assert_eq!(
            config.source_query.as_deref(),
            Some("SELECT id\nFROM orders\n")
        );

        let config = load("source: a.csv\ntarget: b.csv\ntransform_file: t.lua\n").unwrap();
        assert_eq!(config.transform, TransformConfig::File("t.lua".to_string()));
    }

    #[test]
    fn test_older_job_files_still_load() {
        let config = load(
            r#"
version: 1
source:
  uri: employees.csv
target:
  uri: out.json
  options: {}
options:
  batch_size: 100
  infer_schema: true
  schema_file: null
  preview: null
  skip_existing: false
  transform:
    type: inline
    value: "a=1; b=2"
"#,
        )
        .unwrap();
        assert_eq!(config.batch_size, 100);
        assert_eq!(
            config.transform,
            TransformConfig::Inline("a=1; b=2".to_string())
        );

        let config = load(
            "source: a.csv\ntarget: b.csv\noptions:\n  transform:\n    type: file\n    value: t.lua\n",
        )
        .unwrap();
        assert_eq!(config.transform, TransformConfig::File("t.lua".to_string()));
    }

    #[test]
    fn test_mistakes_are_errors() {
        // The typo in the old examples/postgres_config.yaml
        let e =
            load("source: a.csv\ntarget: b.csv\noptions:\n  transform_fil: t.lua\n").unwrap_err();
        assert!(e.contains("unknown field `transform_fil`"), "{}", e);

        let e = load("source: a.csv\ntarget: b.csv\nbatchsize: 5\n").unwrap_err();
        assert!(e.contains("unknown field `batchsize`"), "{}", e);

        let e = load("source:\n  url: a.csv\ntarget: b.csv\n").unwrap_err();
        assert!(e.contains("unknown field `url`"), "{}", e);

        let e = load("source: a.csv\ntarget: b.csv\ntruncate: true\noptions:\n  truncate: false\n")
            .unwrap_err();
        assert!(e.contains("'truncate' is set both"), "{}", e);

        let e = load("source: a.csv\ntarget: b.csv\ntransform: a=1\ntransform_file: t.lua\n")
            .unwrap_err();
        assert!(
            e.contains("either 'transform' or 'transform_file'"),
            "{}",
            e
        );

        let e = load("version: 2\nsource: a.csv\ntarget: b.csv\n").unwrap_err();
        assert!(e.contains("unsupported version 2"), "{}", e);

        assert!(load("target: b.csv\n")
            .unwrap_err()
            .contains("missing field `source`"));
    }

    #[test]
    fn test_env_var_substitution() {
        std::env::set_var("TINYETL_YAML_TEST_PASS", "secret123");
        std::env::remove_var("TINYETL_YAML_TEST_UNSET");
        std::env::set_var("TINYETL_YAML_TEST_EMPTY", "");

        let sub = |s: &str| substitute_env_vars(s).map_err(|e| e.to_string());
        assert_eq!(
            sub("mysql://user:${TINYETL_YAML_TEST_PASS}@localhost/db").unwrap(),
            "mysql://user:secret123@localhost/db"
        );
        assert_eq!(
            sub("${TINYETL_YAML_TEST_UNSET:-out.csv}").unwrap(),
            "out.csv"
        );
        assert_eq!(sub("${TINYETL_YAML_TEST_PASS:-x}").unwrap(), "secret123");
        assert_eq!(sub("${TINYETL_YAML_TEST_EMPTY:-dflt}").unwrap(), "dflt");
        assert_eq!(sub("a${TINYETL_YAML_TEST_EMPTY}b").unwrap(), "ab");
        assert_eq!(
            sub("no vars, $5 and ${ not one").unwrap(),
            "no vars, $5 and ${ not one"
        );

        let e = sub("${TINYETL_YAML_TEST_UNSET}").unwrap_err();
        assert!(e.contains("'TINYETL_YAML_TEST_UNSET' not found"), "{}", e);

        // In endpoint options, too
        let config = load(
            "source: a.csv\ntarget:\n  uri: s3://b/x.csv\n  options:\n    region: ${TINYETL_YAML_TEST_UNSET:-eu-west-1}\n",
        )
        .unwrap();
        assert_eq!(config.target_options["region"], "eu-west-1");
    }

    #[test]
    fn test_generated_config_is_minimal_and_round_trips() {
        let config = Config {
            source: "input.csv".to_string(),
            target: "output.json".to_string(),
            batch_size: DEFAULT_BATCH_SIZE,
            ..Config::default()
        };
        let yaml = YamlConfig::from_config(config).to_yaml_string().unwrap();
        assert_eq!(yaml, "source: input.csv\ntarget: output.json\n");

        let config = Config {
            source: "https://x/a.csv".to_string(),
            target: "out.db".to_string(),
            batch_size: 500,
            truncate: true,
            log_level: LogLevel::Warn,
            transform: TransformConfig::Inline("a=1; b=a+1".to_string()),
            source_options: HashMap::from([("auth.bearer".to_string(), "t".to_string())]),
            ..Config::default()
        };
        let yaml = YamlConfig::from_config(config.clone())
            .to_yaml_string()
            .unwrap();
        assert_eq!(
            yaml,
            "source:\n  uri: https://x/a.csv\n  options:\n    auth.bearer: t\ntarget: out.db\nbatch_size: 500\nlog_level: warn\ntruncate: true\ntransform: a=1; b=a+1\n"
        );
        let restored = load(&yaml).unwrap();
        assert_eq!(restored.source_options, config.source_options);
        assert_eq!(restored.batch_size, 500);
        assert!(restored.truncate);

        let config = Config {
            source: "a.csv".to_string(),
            target: "b.csv".to_string(),
            batch_size: DEFAULT_BATCH_SIZE,
            transform: TransformConfig::File("t.lua".to_string()),
            ..Config::default()
        };
        let yaml = YamlConfig::from_config(config).to_yaml_string().unwrap();
        assert!(yaml.ends_with("transform_file: t.lua\n"), "{}", yaml);
    }
}
