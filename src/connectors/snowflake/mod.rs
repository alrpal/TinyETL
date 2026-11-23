//! Snowflake source and target.
//!
//! - **Reading** streams one `SELECT * FROM <table>` as Arrow batches.
//! - **Writing** stages rows as zstd-compressed CSV files in the table's own stage (`@%table`) and loads them
//!   with one `COPY INTO` when the transfer finishes. The stage upload only works for accounts on
//!   AWS (a limit of the client library); elsewhere rows are inserted in batches with bound
//!   parameters instead.
//! - Loads run in a transaction, together with `--truncate`'s `DELETE`, so a failed load leaves
//!   the table as it was.
//!
//! Talking to Snowflake goes through the [`client::SnowflakeClient`] trait; `firn_client.rs` is
//! the only code that knows which library implements it.

pub mod client;
#[cfg(test)]
mod fake_client;
mod firn_client;
mod staging;

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use std::collections::{HashMap, VecDeque};
use tracing::{debug, info, warn};

use self::client::{query_rows, QueryStream, SnowflakeClient};
use self::staging::{value_text, ChunkWriter, StagedFile};
use crate::{
    connectors::{arrow_values::record_batch_to_rows, Source, Target},
    schema::{Column, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

/// Rows per staged file, unless the `stage_file_rows` option says otherwise.
const DEFAULT_STAGE_FILE_ROWS: usize = 500_000;
/// Rows per INSERT statement on the insert path.
const INSERT_ROWS: usize = 1_000;

/// How the target loads rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMethod {
    /// Stage + COPY INTO on AWS accounts, INSERT elsewhere.
    Auto,
    Stage,
    Insert,
}

/// Everything needed to reach one Snowflake table.
#[derive(Clone)]
pub struct SnowflakeConfig {
    pub account: String,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub schema: String,
    pub warehouse: Option<String>,
    pub role: Option<String>,
    pub table: String,
    pub private_key_path: Option<String>,
    pub private_key_passphrase: Option<String>,
    pub load_method: LoadMethod,
    pub stage_file_rows: usize,
}

impl std::fmt::Debug for SnowflakeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnowflakeConfig")
            .field("account", &self.account)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("database", &self.database)
            .field("schema", &self.schema)
            .field("warehouse", &self.warehouse)
            .field("role", &self.role)
            .field("table", &self.table)
            .field("private_key_path", &self.private_key_path)
            .field(
                "private_key_passphrase",
                &self.private_key_passphrase.as_ref().map(|_| "***"),
            )
            .field("load_method", &self.load_method)
            .field("stage_file_rows", &self.stage_file_rows)
            .finish()
    }
}

/// Endpoint options read by Snowflake sources and targets (see `crate::options`)
pub const OPTIONS: &[&str] = &[
    "private_key_path",
    "private_key_passphrase",
    "warehouse",
    "role",
    "load_method",
    "stage_file_rows",
];

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

impl SnowflakeConfig {
    /// Parse `snowflake://user[:password]@account/database/schema?warehouse=WH&role=R#table`
    /// plus the endpoint options from a YAML config (and `SNOWFLAKE_PRIVATE_KEY_PATH` /
    /// `SNOWFLAKE_PRIVATE_KEY_PASSPHRASE` from the environment).
    pub fn parse(connection_string: &str, options: &HashMap<String, String>) -> Result<Self> {
        Self::parse_with_env(connection_string, options, |name| std::env::var(name).ok())
    }

    /// [`SnowflakeConfig::parse`] for a source query: the URL has no table
    fn parse_for_query(connection_string: &str, options: &HashMap<String, String>) -> Result<Self> {
        // The table isn't used; a placeholder keeps one parser for both forms
        Self::parse(&format!("{}#query", connection_string), options)
    }

    fn parse_with_env(
        connection_string: &str,
        options: &HashMap<String, String>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let invalid = |msg: &str| {
            TinyEtlError::Configuration(format!(
                "Invalid Snowflake URL ({}). Expected \
                 snowflake://user@account/database/schema?warehouse=WH#table",
                msg
            ))
        };
        let url = url::Url::parse(connection_string).map_err(|e| invalid(&e.to_string()))?;
        if url.scheme() != "snowflake" {
            return Err(invalid("scheme must be snowflake"));
        }
        let account = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| invalid("missing account"))?;
        // Accept the full host name too
        let account = account
            .strip_suffix(".snowflakecomputing.com")
            .unwrap_or(account)
            .to_string();
        let user = decode(url.username());
        if user.is_empty() {
            return Err(invalid("missing user"));
        }
        let password = url.password().map(decode).filter(|p| !p.is_empty());

        let path: Vec<String> = url
            .path_segments()
            .map(|segments| segments.filter(|s| !s.is_empty()).map(decode).collect())
            .unwrap_or_default();
        let [database, schema] = path.as_slice() else {
            return Err(invalid("the path must be /database/schema"));
        };

        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        for key in query.keys() {
            if !matches!(key.as_str(), "warehouse" | "role" | "table") {
                return Err(invalid(&format!("unknown parameter '{}'", key)));
            }
        }
        for key in options.keys() {
            if !OPTIONS.contains(&key.as_str()) {
                return Err(TinyEtlError::Configuration(format!(
                    "Unknown Snowflake option '{}'. Valid options: {}",
                    key,
                    OPTIONS.join(", ")
                )));
            }
        }
        let option = |key: &str| options.get(key).filter(|v| !v.is_empty()).cloned();

        let table = url
            .fragment()
            .map(decode)
            .or_else(|| query.get("table").cloned())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| invalid("missing table; add #table"))?;

        let load_method = match option("load_method").as_deref() {
            None | Some("auto") => LoadMethod::Auto,
            Some("stage") => LoadMethod::Stage,
            Some("insert") => LoadMethod::Insert,
            Some(other) => {
                return Err(TinyEtlError::Configuration(format!(
                    "Invalid Snowflake load_method '{}': use auto, stage or insert",
                    other
                )))
            }
        };
        let stage_file_rows = match option("stage_file_rows") {
            None => DEFAULT_STAGE_FILE_ROWS,
            Some(v) => v.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                TinyEtlError::Configuration(format!(
                    "Invalid Snowflake stage_file_rows '{}': must be a positive number",
                    v
                ))
            })?,
        };

        Ok(Self {
            account,
            user,
            password,
            database: database.clone(),
            schema: schema.clone(),
            warehouse: option("warehouse").or_else(|| query.get("warehouse").cloned()),
            role: option("role").or_else(|| query.get("role").cloned()),
            table,
            private_key_path: option("private_key_path")
                .or_else(|| env("SNOWFLAKE_PRIVATE_KEY_PATH").filter(|v| !v.is_empty())),
            private_key_passphrase: option("private_key_passphrase")
                .or_else(|| env("SNOWFLAKE_PRIVATE_KEY_PASSPHRASE").filter(|v| !v.is_empty())),
            load_method,
            stage_file_rows,
        })
    }
}

/// Snowflake's reserved keywords, which can't be used as unquoted identifiers.
const RESERVED: [&str; 98] = [
    "ACCOUNT",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "BETWEEN",
    "BY",
    "CASE",
    "CAST",
    "CHECK",
    "COLUMN",
    "CONNECT",
    "CONNECTION",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DATABASE",
    "DELETE",
    "DISTINCT",
    "DROP",
    "ELSE",
    "EXISTS",
    "FALSE",
    "FOLLOWING",
    "FOR",
    "FROM",
    "FULL",
    "GRANT",
    "GROUP",
    "GSCLUSTER",
    "HAVING",
    "ILIKE",
    "IN",
    "INCREMENT",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "ISSUE",
    "JOIN",
    "LATERAL",
    "LEFT",
    "LIKE",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "MINUS",
    "NATURAL",
    "NOT",
    "NULL",
    "OF",
    "ON",
    "OR",
    "ORDER",
    "ORGANIZATION",
    "QUALIFY",
    "REGEXP",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROW",
    "ROWS",
    "SAMPLE",
    "SCHEMA",
    "SELECT",
    "SET",
    "SOME",
    "START",
    "TABLE",
    "TABLESAMPLE",
    "THEN",
    "TO",
    "TRIGGER",
    "TRUE",
    "TRY_CAST",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHENEVER",
    "WHERE",
    "WINDOW",
    "WITH",
    "ASOF",
    "MATCH_CONDITION",
    "INCREMENTAL",
    "LIMIT",
    "OFFSET",
    "PROCEDURE",
];

/// SQL for one identifier.
///
/// Plain names (letters, digits, `_`, `$`, not starting with a digit) are left unquoted, so
/// Snowflake stores them upper-case and they can be queried without quotes, the way Snowflake
/// users expect. Reserved words are quoted upper-case for the same effect. Any other name
/// (spaces, punctuation) is quoted exactly as given.
fn ident(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if plain {
        let upper = name.to_ascii_uppercase();
        if RESERVED.contains(&upper.as_str()) {
            format!("\"{}\"", upper)
        } else {
            name.to_string()
        }
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// SQL for a possibly qualified table name (`table`, `schema.table`, `db.schema.table`).
fn qualified(name: &str) -> String {
    name.split('.').map(ident).collect::<Vec<_>>().join(".")
}

/// The table's own stage, `@%table` (or `@db.schema.%table`), plus a path inside it.
fn table_stage(table: &str, path: &str) -> String {
    let parts: Vec<String> = table.split('.').map(ident).collect();
    let (name, namespace) = parts.split_last().expect("split always yields a part");
    let prefix = if namespace.is_empty() {
        String::new()
    } else {
        format!("{}.", namespace.join("."))
    };
    format!("@{}%{}/{}", prefix, name, path)
}

/// Column type for `CREATE TABLE`.
fn column_type(data_type: &DataType) -> &'static str {
    match data_type {
        DataType::String | DataType::Null => "VARCHAR",
        DataType::Integer => "NUMBER(38,0)",
        DataType::Decimal => "NUMBER(38,10)",
        DataType::Boolean => "BOOLEAN",
        DataType::Date => "DATE",
        // TinyETL's timestamps are UTC; NTZ stores them as given, with no session time zone
        DataType::DateTime => "TIMESTAMP_NTZ",
        DataType::Json => "VARIANT",
    }
}

fn create_table_sql(table: &str, schema: &Schema) -> String {
    let columns = schema
        .columns
        .iter()
        .map(|c| {
            format!(
                "{} {}{}",
                ident(&c.name),
                column_type(&c.data_type),
                if c.nullable { "" } else { " NOT NULL" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        qualified(table),
        columns
    )
}

fn column_list(schema: &Schema) -> String {
    schema
        .columns
        .iter()
        .map(|c| ident(&c.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `COPY INTO` for every file staged under `stage_location`. JSON columns go through
/// `PARSE_JSON` so they land as VARIANT values, not strings.
fn copy_sql(table: &str, schema: &Schema, stage_location: &str) -> String {
    let selects = schema
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| match c.data_type {
            DataType::Json => format!("PARSE_JSON(${})", i + 1),
            _ => format!("${}", i + 1),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "COPY INTO {} ({}) FROM (SELECT {} FROM {}) FILE_FORMAT = ({}) \
         ON_ERROR = ABORT_STATEMENT PURGE = TRUE",
        qualified(table),
        column_list(schema),
        selects,
        stage_location,
        staging::FILE_FORMAT
    )
}

/// `INSERT ... SELECT ... FROM VALUES` for `rows` rows of bound parameters.
fn insert_sql(table: &str, schema: &Schema, rows: usize) -> String {
    let selects = schema
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| match c.data_type {
            DataType::Json => format!("PARSE_JSON(column{})", i + 1),
            _ => format!("column{}", i + 1),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let row = format!("({})", vec!["?"; schema.columns.len()].join(", "));
    format!(
        "INSERT INTO {} ({}) SELECT {} FROM VALUES {}",
        qualified(table),
        column_list(schema),
        selects,
        vec![row; rows].join(", ")
    )
}

/// Whether `CURRENT_REGION()` (e.g. `AWS_US_EAST_1`, `PUBLIC.AZURE_EASTUS2`) is an AWS region.
fn region_is_aws(region: &str) -> bool {
    region.to_ascii_uppercase().contains("AWS_")
}

/// Total of the `rows_loaded` column of a `COPY INTO` result.
fn rows_loaded(result: &[Row]) -> usize {
    result
        .iter()
        .filter_map(|row| {
            row.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("rows_loaded"))
                .map(|(_, v)| v)
        })
        .map(|v| match v {
            Value::Integer(n) => *n as usize,
            Value::Decimal(d) => d.to_string().parse().unwrap_or(0),
            Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// `TinyEtlError` isn't `Send` (it can hold a Lua error), so it can't be held across an
/// `.await`. This keeps its variant and message while a ROLLBACK runs, and rebuilds it after.
enum HeldError {
    Connection(String),
    Configuration(String),
    DataTransfer(String),
}

impl From<TinyEtlError> for HeldError {
    fn from(e: TinyEtlError) -> Self {
        match e {
            TinyEtlError::Connection(m) => HeldError::Connection(m),
            TinyEtlError::Configuration(m) => HeldError::Configuration(m),
            TinyEtlError::DataTransfer(m) => HeldError::DataTransfer(m),
            other => HeldError::DataTransfer(other.to_string()),
        }
    }
}

impl From<HeldError> for TinyEtlError {
    fn from(e: HeldError) -> Self {
        match e {
            HeldError::Connection(m) => TinyEtlError::Connection(m),
            HeldError::Configuration(m) => TinyEtlError::Configuration(m),
            HeldError::DataTransfer(m) => TinyEtlError::DataTransfer(m),
        }
    }
}

async fn open_client(config: &SnowflakeConfig) -> Result<Box<dyn SnowflakeClient>> {
    Ok(Box::new(firn_client::connect(config).await?))
}

// ---------------------------------------------------------------------------------------------
// Source

/// Snowflake source: streams every row of one table.
pub struct SnowflakeSource {
    config: SnowflakeConfig,
    /// `--source-query`: read this query's result instead of the table
    query: Option<String>,
    client: Option<Box<dyn SnowflakeClient>>,
    /// Behind a `Mutex` only so the source is `Sync`; it's always used through `&mut self`.
    result: std::sync::Mutex<Option<QueryStream>>,
    schema: Option<Schema>,
    total_rows: Option<u64>,
    pending: VecDeque<Row>,
    /// No row has been taken from the open result yet.
    fresh: bool,
    exhausted: bool,
}

impl SnowflakeSource {
    pub fn new(connection_string: &str, options: &HashMap<String, String>) -> Result<Self> {
        Ok(Self::with_config(SnowflakeConfig::parse(
            connection_string,
            options,
        )?))
    }

    /// Read the result of `query`; `connection_string` has no `#table`
    pub fn with_query(
        connection_string: &str,
        query: &str,
        options: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut source = Self::with_config(SnowflakeConfig::parse_for_query(
            connection_string,
            options,
        )?);
        source.query = Some(query.to_string());
        Ok(source)
    }

    fn with_config(config: SnowflakeConfig) -> Self {
        Self {
            config,
            query: None,
            client: None,
            result: std::sync::Mutex::new(None),
            schema: None,
            total_rows: None,
            pending: VecDeque::new(),
            fresh: false,
            exhausted: false,
        }
    }

    async fn open_query(&mut self) -> Result<()> {
        if self.client.is_none() {
            self.client = Some(open_client(&self.config).await?);
        }
        let sql = match &self.query {
            Some(query) => query.clone(),
            None => format!("SELECT * FROM {}", qualified(&self.config.table)),
        };
        debug!("Snowflake source query: {}", sql);
        let result = self.client.as_ref().expect("connected").query(&sql).await?;
        self.schema = Some(Schema {
            columns: result
                .columns
                .iter()
                .map(|c| Column {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                    nullable: c.nullable,
                })
                .collect(),
            estimated_rows: result.total_rows.map(|n| n as usize),
            primary_key_candidate: None,
        });
        self.total_rows = result.total_rows;
        *self.result.get_mut().unwrap() = Some(result);
        self.pending.clear();
        self.fresh = true;
        self.exhausted = false;
        Ok(())
    }
}

#[async_trait]
impl Source for SnowflakeSource {
    async fn connect(&mut self) -> Result<()> {
        self.open_query().await
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.schema.is_none() {
            self.open_query().await?;
        }
        Ok(self.schema.clone().expect("set by open_query"))
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.result.get_mut().unwrap().is_none() {
            self.open_query().await?;
        }
        self.fresh = false;
        while self.pending.len() < batch_size && !self.exhausted {
            let result = self.result.get_mut().unwrap().as_mut().expect("open");
            match result.batches.next().await {
                Some(batch) => self.pending.extend(record_batch_to_rows(&batch?)?),
                None => self.exhausted = true,
            }
        }
        let take = batch_size.min(self.pending.len());
        Ok(self.pending.drain(..take).collect())
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(self.total_rows.map(|n| n as usize))
    }

    async fn reset(&mut self) -> Result<()> {
        if !self.fresh {
            // Snowflake results only stream forward, so run the query again
            self.open_query().await?;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        !self.exhausted || !self.pending.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// Target

/// Snowflake target: creates the table if needed and loads rows by stage + COPY INTO, or by
/// batched INSERTs where staging isn't available.
pub struct SnowflakeTarget {
    config: SnowflakeConfig,
    client: Option<Box<dyn SnowflakeClient>>,
    /// Decided at connect time from the account's cloud.
    use_stage: bool,
    schema: Option<Schema>,
    /// `--truncate` was requested; the DELETE runs inside the load's transaction.
    truncate_pending: bool,
    in_transaction: bool,
    /// Stage path for this run: `tinyetl/<uuid>/`.
    run_path: String,
    chunks: Option<ChunkWriter>,
    rows_staged: usize,
    files_staged: usize,
}

impl SnowflakeTarget {
    pub fn new(connection_string: &str, options: &HashMap<String, String>) -> Result<Self> {
        Ok(Self::with_config(SnowflakeConfig::parse(
            connection_string,
            options,
        )?))
    }

    fn with_config(config: SnowflakeConfig) -> Self {
        Self {
            config,
            client: None,
            use_stage: true,
            schema: None,
            truncate_pending: false,
            in_transaction: false,
            run_path: format!("tinyetl/{}/", uuid::Uuid::new_v4().simple()),
            chunks: None,
            rows_staged: 0,
            files_staged: 0,
        }
    }

    fn client(&self) -> Result<&dyn SnowflakeClient> {
        self.client
            .as_deref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected to Snowflake".to_string()))
    }

    fn stage_location(&self) -> String {
        table_stage(&self.config.table, &self.run_path)
    }

    /// Choose stage or insert loading. Called once the client is connected.
    async fn choose_load_method(&mut self) -> Result<()> {
        self.use_stage = match self.config.load_method {
            LoadMethod::Stage => true,
            LoadMethod::Insert => false,
            LoadMethod::Auto => {
                let client = self.client()?;
                let rows = query_rows(client, "SELECT CURRENT_REGION() AS REGION").await?;
                let region = rows
                    .first()
                    .and_then(|row| row.values().next())
                    .map(|v| value_text(v).unwrap_or_default())
                    .unwrap_or_default();
                let aws = region_is_aws(&region);
                if !aws {
                    warn!(
                        "Snowflake account region {} is not on AWS; TinyETL can only stage files \
                         on AWS, so rows will be inserted in batches (slower). Set the option \
                         load_method = insert to silence this warning",
                        region
                    );
                }
                aws
            }
        };
        info!(
            "Snowflake: loading via {}",
            if self.use_stage {
                "stage + COPY INTO"
            } else {
                "batched INSERT"
            }
        );
        Ok(())
    }

    /// Start the load's transaction, running a pending truncate inside it.
    async fn begin(&mut self) -> Result<()> {
        if self.in_transaction {
            return Ok(());
        }
        let client = self.client()?;
        client.execute("BEGIN", vec![]).await?;
        self.in_transaction = true;
        if self.truncate_pending {
            let sql = format!("DELETE FROM {}", qualified(&self.config.table));
            let client = self.client()?;
            if let Err(e) = client.execute(&sql, vec![]).await.map_err(HeldError::from) {
                self.rollback().await;
                return Err(e.into());
            }
        }
        Ok(())
    }

    async fn rollback(&mut self) {
        if self.in_transaction {
            if let Some(client) = self.client.as_deref() {
                if let Err(e) = client.execute("ROLLBACK", vec![]).await {
                    warn!("Snowflake: ROLLBACK failed: {}", e);
                }
            }
            self.in_transaction = false;
        }
    }

    async fn upload(&mut self, file: StagedFile) -> Result<()> {
        let location = self.stage_location();
        debug!(
            "Snowflake: uploading {} ({} rows) to {}",
            file.path.display(),
            file.rows,
            location
        );
        let client = self.client()?;
        client.put(&file.path, &location).await?;
        let _ = std::fs::remove_file(&file.path);
        self.rows_staged += file.rows;
        self.files_staged += 1;
        Ok(())
    }

    async fn insert_rows(&mut self, rows: &[Row]) -> Result<()> {
        self.begin().await?;
        let schema = self.schema.clone().ok_or_else(|| {
            TinyEtlError::Configuration("Schema not set for Snowflake target".to_string())
        })?;
        for chunk in rows.chunks(INSERT_ROWS) {
            let sql = insert_sql(&self.config.table, &schema, chunk.len());
            let binds = chunk
                .iter()
                .flat_map(|row| {
                    schema
                        .columns
                        .iter()
                        .map(|c| value_text(row.get(&c.name).unwrap_or(&Value::Null)))
                })
                .collect();
            let client = self.client()?;
            if let Err(e) = client.execute(&sql, binds).await.map_err(HeldError::from) {
                self.rollback().await;
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// Remove this run's staged files after a failed load.
    async fn remove_staged_files(&self) {
        if self.files_staged > 0 {
            if let Some(client) = self.client.as_deref() {
                let sql = format!("REMOVE {}", self.stage_location());
                if let Err(e) = client.execute(&sql, vec![]).await {
                    warn!("Snowflake: could not remove staged files: {}", e);
                }
            }
        }
    }

    async fn copy_staged_files(&mut self) -> Result<()> {
        let schema = self.schema.clone().ok_or_else(|| {
            TinyEtlError::Configuration("Schema not set for Snowflake target".to_string())
        })?;
        self.begin().await?;
        let sql = copy_sql(&self.config.table, &schema, &self.stage_location());
        debug!("Snowflake: {}", sql);
        let client = self.client()?;
        let loaded = match query_rows(client, &sql).await.map_err(HeldError::from) {
            Ok(result) => rows_loaded(&result),
            Err(e) => {
                self.rollback().await;
                self.remove_staged_files().await;
                return Err(e.into());
            }
        };
        if loaded != self.rows_staged {
            self.rollback().await;
            self.remove_staged_files().await;
            return Err(TinyEtlError::DataTransfer(format!(
                "Snowflake COPY INTO loaded {} rows, but {} were staged; nothing was committed",
                loaded, self.rows_staged
            )));
        }
        info!(
            "Snowflake: loaded {} rows from {} staged file(s)",
            loaded, self.files_staged
        );
        Ok(())
    }
}

#[async_trait]
impl Target for SnowflakeTarget {
    async fn connect(&mut self) -> Result<()> {
        if self.client.is_none() {
            self.client = Some(open_client(&self.config).await?);
            self.choose_load_method().await?;
        }
        Ok(())
    }

    // The table always comes from the parsed URL: the engine derives its name from the text
    // after `#`, which the older `?table=` form doesn't have.
    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        let sql = create_table_sql(&self.config.table, schema);
        debug!("Snowflake: {}", sql);
        let client = self.client()?;
        client.execute(&sql, vec![]).await?;
        self.schema = Some(schema.clone());
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        if !self.use_stage {
            self.insert_rows(rows).await?;
            return Ok(rows.len());
        }
        let schema = self.schema.clone().ok_or_else(|| {
            TinyEtlError::Configuration("Schema not set for Snowflake target".to_string())
        })?;
        if self.chunks.is_none() {
            self.chunks = Some(ChunkWriter::new(self.config.stage_file_rows)?);
        }
        for row in rows {
            let texts: Vec<Option<String>> = schema
                .columns
                .iter()
                .map(|c| value_text(row.get(&c.name).unwrap_or(&Value::Null)))
                .collect();
            let finished = self
                .chunks
                .as_mut()
                .expect("created above")
                .write_row(texts.iter().map(|t| t.as_deref()))?;
            if let Some(file) = finished {
                self.upload(file).await?;
            }
        }
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        if self.client.is_none() {
            return Ok(());
        }
        if self.use_stage {
            let last_file = match self.chunks.as_mut() {
                Some(chunks) => chunks.finish()?,
                None => None,
            };
            if let Some(file) = last_file {
                self.upload(file).await?;
            }
            if self.rows_staged > 0 {
                self.copy_staged_files().await?;
            }
        }
        // A truncate with no rows to load still has to happen
        if self.truncate_pending && !self.in_transaction {
            self.begin().await?;
        }
        if self.in_transaction {
            let client = self.client()?;
            if let Err(e) = client
                .execute("COMMIT", vec![])
                .await
                .map_err(HeldError::from)
            {
                self.rollback().await;
                return Err(e.into());
            }
            self.in_transaction = false;
        }
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        let Some(client) = self.client.as_deref() else {
            return Ok(false);
        };
        let sql = format!("SELECT 1 FROM {} LIMIT 0", qualified(&self.config.table));
        Ok(client.execute(&sql, vec![]).await.is_ok())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        self.truncate_pending = true;
        Ok(())
    }

    fn supports_append(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
