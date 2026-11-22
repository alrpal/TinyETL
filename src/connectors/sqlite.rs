use async_trait::async_trait;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use sqlx::{sqlite::SqliteConnectOptions, Column, Row as SqlxRow, SqlitePool};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{
    connectors::{row_stream::RowStream, Source, Target},
    schema::{Column as SchemaColumn, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

/// TinyETL type for a SQLite declared column type, following SQLite's own affinity rules
/// (substring matches, so `VARCHAR(255)` and `DECIMAL(10,2)` are recognised), plus the
/// conventional `JSON`, `BOOLEAN`, `DATE` and `DATETIME` names.
fn declared_type(sql_type: &str) -> DataType {
    let t = sql_type.to_ascii_uppercase();
    if t.contains("JSON") {
        DataType::Json
    } else if t.contains("BOOL") {
        DataType::Boolean
    } else if t.contains("DATE") && !t.contains("TIME") {
        DataType::Date
    } else if t.contains("DATETIME") || t.contains("TIMESTAMP") {
        DataType::DateTime
    } else if t.contains("INT") {
        DataType::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        DataType::String
    } else if ["REAL", "FLOA", "DOUB", "NUMERIC", "DECIMAL"]
        .iter()
        .any(|k| t.contains(k))
    {
        DataType::Decimal
    } else {
        DataType::String // BLOB, no declared type, or anything else
    }
}

pub struct SqliteSource {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
    /// One streaming `SELECT` for the whole read (no OFFSET paging)
    stream: Option<RowStream>,
    consumed: bool,
}

impl SqliteSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - could be "file.db" or "sqlite:file.db#table" or "file.db#table"
        let (db_path, table) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "SQLite connection string format: file.db#table".to_string(),
                ));
            }
            (parts[0].trim_start_matches("sqlite:"), parts[1])
        } else {
            return Err(TinyEtlError::Configuration(
                "SQLite source requires table specification: file.db#table".to_string(),
            ));
        };

        Ok(Self {
            connection_string: format!("sqlite:{}", db_path),
            pool: None,
            table_name: table.to_string(),
            stream: None,
            consumed: false,
        })
    }

    /// Run `SELECT *` once on its own connection and stream the rows.
    fn start_stream(&self) -> Result<RowStream> {
        let pool = self
            .pool
            .clone()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;
        let sql = format!("SELECT * FROM \"{}\"", self.table_name);
        Ok(RowStream::spawn(move |mut sink| async move {
            let mut conn = match pool.acquire().await {
                Ok(conn) => conn,
                Err(e) => return sink.fail(format!("Failed to connect: {}", e)).await,
            };
            let mut rows = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch(&mut *conn);
            while let Some(row) = rows.next().await {
                match row {
                    Ok(row) => {
                        if !sink.push(Self::row_values(&row)).await {
                            return;
                        }
                    }
                    Err(e) => return sink.fail(format!("Failed to fetch rows: {}", e)).await,
                }
            }
            drop(rows);
            sink.finish().await;
        }))
    }

    fn row_values(row: &sqlx::sqlite::SqliteRow) -> Row {
        row.columns()
            .iter()
            .enumerate()
            .map(|(i, column)| (column.name().to_string(), Self::value_at(row, i)))
            .collect()
    }

    /// A column value by its stored type (SQLite types each value, not each column).
    fn value_at(row: &sqlx::sqlite::SqliteRow, i: usize) -> Value {
        if let Ok(val) = row.try_get::<Option<String>, _>(i) {
            val.map_or(Value::Null, Value::String)
        } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
            val.map_or(Value::Null, Value::Integer)
        } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
            match val {
                Some(f) => match Decimal::try_from(f) {
                    Ok(d) => Value::Decimal(d),
                    Err(_) => Value::String(f.to_string()),
                },
                None => Value::Null,
            }
        } else if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(i) {
            // BLOB: text stored as bytes (such as JSON written by some drivers) is read as
            // text; other bytes as hex
            match val {
                Some(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Value::String(text),
                    Err(e) => Value::String(format!("0x{}", hex::encode(e.as_bytes()))),
                },
                None => Value::Null,
            }
        } else {
            Value::Null
        }
    }
}

#[async_trait]
impl Source for SqliteSource {
    async fn connect(&mut self) -> Result<()> {
        match SqlitePool::connect(&self.connection_string).await {
            Ok(pool) => {
                self.pool = Some(pool);
                Ok(())
            }
            Err(e) => {
                let db_path = self.connection_string.trim_start_matches("sqlite:");
                Err(TinyEtlError::Connection(format!(
                    "Failed to connect to SQLite database '{}': {}. Make sure the file exists and is readable.", 
                    db_path,
                    e
                )))
            }
        }
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.pool.is_none() {
            self.connect().await?;
        }

        let pool = self.pool.as_ref().unwrap();

        // Get table info for column definitions
        let table_info = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA table_info(\"{}\")",
            self.table_name
        )))
        .fetch_all(pool)
        .await?;

        let mut columns = Vec::new();
        for row in table_info {
            let name: String = row.get(1);
            let sql_type: String = row.get(2);
            let not_null: bool = row.get(3);

            let data_type = declared_type(&sql_type);

            columns.push(SchemaColumn {
                name,
                data_type,
                nullable: !not_null,
            });
        }

        // SQLite doesn't enforce JSON in a JSON-declared column: read a sample, and keep a
        // column as text if any sampled value isn't valid JSON
        let json_columns: Vec<String> = columns
            .iter()
            .filter(|c| c.data_type == DataType::Json)
            .map(|c| c.name.clone())
            .collect();
        if !json_columns.is_empty() {
            let select = json_columns
                .iter()
                .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(", ");
            let sample = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {} FROM \"{}\" LIMIT {}",
                select, self.table_name, sample_size
            )))
            .fetch_all(pool)
            .await?;
            for (i, name) in json_columns.iter().enumerate() {
                let all_json = sample.iter().all(|row| match Self::value_at(row, i) {
                    Value::Null => true,
                    Value::String(text) => serde_json::from_str::<serde_json::Value>(&text).is_ok(),
                    _ => true, // numbers are valid JSON scalars
                });
                if !all_json {
                    if let Some(column) = columns.iter_mut().find(|c| &c.name == name) {
                        column.data_type = DataType::String;
                    }
                }
            }
        }

        // Get estimated row count
        let count_result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) as count FROM \"{}\"",
            self.table_name
        )))
        .fetch_one(pool)
        .await?;
        let estimated_rows: i64 = count_result.get("count");

        Ok(Schema {
            columns,
            estimated_rows: Some(estimated_rows as usize),
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.pool.is_none() {
            self.connect().await?;
        }
        if self.stream.is_none() {
            self.stream = Some(self.start_stream()?);
        }
        let rows = self
            .stream
            .as_mut()
            .expect("started above")
            .next_batch(batch_size)
            .await?;
        self.consumed |= !rows.is_empty();
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        if let Some(pool) = &self.pool {
            let count_result = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) as count FROM {}",
                self.table_name
            )))
            .fetch_one(pool)
            .await?;
            let count: i64 = count_result.get("count");
            Ok(Some(count as usize))
        } else {
            Ok(None)
        }
    }

    async fn reset(&mut self) -> Result<()> {
        // The query runs again on the next read, but only if rows were already read
        if self.consumed {
            self.stream = None;
            self.consumed = false;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.stream.as_ref().is_none_or(RowStream::has_more)
    }
}

pub struct SqliteTarget {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
    /// The load's transaction, begun at the first write and committed in finalize. Dropped
    /// without a commit (the transfer failed), it rolls back. Mutex only for Sync.
    tx: Option<Mutex<sqlx::Transaction<'static, sqlx::Sqlite>>>,
    /// truncate() was called: empty the table inside the load's transaction
    truncate_table: Option<String>,
}

impl SqliteTarget {
    /// Open the load's transaction (once), emptying the table first if --truncate was given.
    async fn begin(&mut self) -> Result<()> {
        if self.tx.is_some() {
            return Ok(());
        }
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Pool not connected".to_string()))?;
        let mut tx = pool.begin().await.map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to start a transaction: {}", e))
        })?;
        if let Some(table) = self.truncate_table.take() {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM \"{}\"", table)))
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e))
                })?;
        }
        self.tx = Some(Mutex::new(tx));
        Ok(())
    }

    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - support multiple formats:
        // - "file.db" or "file.db#table" (legacy)
        // - "sqlite:file.db" or "sqlite:file.db#table"
        // - "sqlite://file.db" or "sqlite://file.db#table"
        let normalized = connection_string
            .trim_start_matches("sqlite://")
            .trim_start_matches("sqlite:");

        let (db_path, table) = if normalized.contains('#') {
            let parts: Vec<&str> = normalized.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "SQLite connection string format: file.db#table or sqlite://file.db#table"
                        .to_string(),
                ));
            }
            (parts[0], parts[1])
        } else {
            // Default table name if not specified
            (normalized, "data")
        };

        Ok(Self {
            connection_string: format!("sqlite:{}", db_path),
            pool: None,
            table_name: table.to_string(),
            tx: None,
            truncate_table: None,
        })
    }

    fn get_db_path(&self) -> Result<PathBuf> {
        let path_str = self.connection_string.trim_start_matches("sqlite:");
        Ok(PathBuf::from(path_str))
    }

    fn map_data_type_to_sqlite(&self, data_type: &DataType) -> &'static str {
        match data_type {
            DataType::Integer => "INTEGER",
            DataType::Decimal => "REAL",
            DataType::String => "TEXT",
            DataType::Boolean => "INTEGER", // SQLite uses INTEGER for boolean
            DataType::Date => "TEXT",
            DataType::DateTime => "TEXT",
            DataType::Json => "TEXT", // SQLite stores JSON as TEXT
            DataType::Null => "TEXT",
        }
    }
}

#[async_trait]
impl Target for SqliteTarget {
    async fn connect(&mut self) -> Result<()> {
        // Ensure parent directory exists for the database file
        let db_path = self.get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // SQLite will automatically create the database file if it doesn't exist
        // when we connect to it, so we don't need to create it manually
        let connect_options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);

        match SqlitePool::connect_with(connect_options).await {
            Ok(pool) => {
                self.pool = Some(pool);
                Ok(())
            }
            Err(e) => Err(TinyEtlError::Connection(format!(
                "Failed to connect to SQLite database '{}': {}. Check file path and permissions.",
                db_path.display(),
                e
            ))),
        }
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.pool.is_none() {
            self.connect().await?;
        }

        let pool = self.pool.as_ref().unwrap();

        // Determine the actual table name to use
        let actual_table_name = if table_name.is_empty() {
            self.table_name.clone()
        } else {
            table_name.to_string()
        };

        // Update our internal table name to match what we're actually creating
        self.table_name = actual_table_name.clone();

        // Build CREATE TABLE statement with IF NOT EXISTS (append-first philosophy)
        let column_definitions: Vec<String> = schema
            .columns
            .iter()
            .map(|col| {
                let sqlite_type = self.map_data_type_to_sqlite(&col.data_type);
                let nullable = if col.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", col.name, sqlite_type, nullable)
            })
            .collect();

        // Use CREATE TABLE IF NOT EXISTS to support append-first philosophy
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            actual_table_name,
            column_definitions.join(", ")
        );

        sqlx::query(sqlx::AssertSqlSafe(create_sql.as_str()))
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.pool.is_none() {
            return Err(TinyEtlError::Connection("Pool not connected".to_string()));
        }

        if rows.is_empty() {
            return Ok(0);
        }

        self.begin().await?;
        // Taken out while the batch is written; a failure drops it, which rolls back
        let mut tx = self
            .tx
            .take()
            .expect("begun above")
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());

        // Get column names from first row
        let columns: Vec<String> = rows[0].keys().cloned().collect();

        // Quote column names to handle reserved keywords
        let quoted_columns: Vec<String> =
            columns.iter().map(|col| format!("\"{}\"", col)).collect();

        // SQLite has a limit of ~999 variables, so we need to chunk our inserts
        // Calculate max rows per chunk based on column count
        let max_variables = 900; // Use 900 to be safe
        let max_rows_per_chunk = max_variables / columns.len();

        let mut total_written = 0;

        // Process rows in chunks
        for chunk in rows.chunks(max_rows_per_chunk) {
            // Create batch insert with multiple value groups for this chunk
            let placeholders_per_row = vec!["?"; columns.len()].join(", ");
            let value_groups: Vec<String> = (0..chunk.len())
                .map(|_| format!("({})", placeholders_per_row))
                .collect();

            let insert_sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES {}",
                self.table_name,
                quoted_columns.join(", "),
                value_groups.join(", ")
            );

            let mut query = sqlx::query(sqlx::AssertSqlSafe(insert_sql.as_str()));

            // Bind all values in row order for this chunk
            for row in chunk {
                for column in &columns {
                    let value = row.get(column).unwrap_or(&Value::Null);
                    query = match value {
                        Value::String(s) => query.bind(s),
                        Value::Integer(i) => query.bind(*i),
                        Value::Decimal(d) => {
                            // Convert Decimal to f64 for SQLite binding
                            let f: f64 = (*d).try_into().unwrap_or(0.0);
                            query.bind(f)
                        }
                        Value::Boolean(b) => query.bind(*b),
                        Value::Date(dt) => query.bind(dt.to_rfc3339()),
                        Value::Json(j) => query
                            .bind(serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string())),
                        Value::Null => query.bind(None::<String>),
                    };
                }
            }

            let result = query.execute(&mut *tx).await?;
            total_written += result.rows_affected() as usize;
        }
        self.tx = Some(Mutex::new(tx));

        Ok(total_written)
    }

    async fn finalize(&mut self) -> Result<()> {
        // A --truncate with no rows to load still has to empty the table
        if self.truncate_table.is_some() {
            self.begin().await?;
        }
        if let Some(tx) = self.tx.take() {
            let tx = tx.into_inner().unwrap_or_else(|e| e.into_inner());
            tx.commit().await.map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to commit the load: {}", e))
            })?;
        }
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        if let Some(pool) = &self.pool {
            let actual_table_name = if table_name.is_empty() {
                &self.table_name
            } else {
                table_name
            };

            let result =
                sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(actual_table_name)
                    .fetch_optional(pool)
                    .await?;

            Ok(result.is_some())
        } else {
            Ok(false)
        }
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        // Deferred into the load's transaction, so a failed load keeps the old rows
        let table = if table_name.is_empty() {
            &self.table_name
        } else {
            table_name
        };
        self.truncate_table = Some(table.to_string());
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // SQLite databases support appending new rows
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_declared_types() {
        let cases = [
            ("json", DataType::Json),
            ("JSON", DataType::Json),
            ("VARCHAR(255)", DataType::String),
            ("TEXT", DataType::String),
            ("DECIMAL(10,2)", DataType::Decimal),
            ("REAL", DataType::Decimal),
            ("INTEGER", DataType::Integer),
            ("BIGINT", DataType::Integer),
            ("BOOLEAN", DataType::Boolean),
            ("DATE", DataType::Date),
            ("DATETIME", DataType::DateTime),
            ("TIMESTAMP", DataType::DateTime),
            ("BLOB", DataType::String),
            ("", DataType::String),
        ];
        for (declared, expected) in cases {
            assert_eq!(declared_type(declared), expected, "{:?}", declared);
        }
    }

    async fn schema_of(sql: &[&str]) -> Schema {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        for statement in sql {
            sqlx::query(sqlx::AssertSqlSafe(*statement))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
        let mut source = SqliteSource::new(&format!("{}#t", path.display())).unwrap();
        source.connect().await.unwrap();
        source.infer_schema(1000).await.unwrap()
    }

    #[tokio::test]
    async fn test_json_columns() {
        // JSON stored as BLOB (issue #13) and as TEXT is read as a JSON column
        let schema = schema_of(&[
            "CREATE TABLE t (id TEXT, doc json)",
            r#"INSERT INTO t VALUES ('a', CAST('{"k": [1, 2]}' AS BLOB)), ('b', '{"k": 3}'), ('c', NULL)"#,
        ])
        .await;
        assert_eq!(schema.columns[1].data_type, DataType::Json);

        // A JSON-declared column holding text that isn't JSON stays text
        let schema = schema_of(&[
            "CREATE TABLE t (id TEXT, doc json)",
            r#"INSERT INTO t VALUES ('a', '{"k": 1}'), ('b', 'not json')"#,
        ])
        .await;
        assert_eq!(schema.columns[1].data_type, DataType::String);
    }

    #[tokio::test]
    async fn test_sqlite_source_new() {
        let source = SqliteSource::new("test.db#users");
        assert!(source.is_ok());
    }

    #[tokio::test]
    async fn test_sqlite_source_invalid_format() {
        let source = SqliteSource::new("test.db");
        assert!(source.is_err());
    }

    #[tokio::test]
    async fn test_sqlite_target_new() {
        let target = SqliteTarget::new("test.db#users");
        assert!(target.is_ok());

        let target2 = SqliteTarget::new("test.db");
        assert!(target2.is_ok());
    }
}
