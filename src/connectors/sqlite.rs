use async_trait::async_trait;
use rust_decimal::Decimal;
use sqlx::{sqlite::SqliteConnectOptions, Column, Row as SqlxRow, SqlitePool};
use std::path::PathBuf;

use crate::{
    connectors::{Source, Target},
    schema::{Column as SchemaColumn, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

pub struct SqliteSource {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
    query: Option<String>,
    current_offset: usize,
    total_rows: Option<usize>,
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
            query: None,
            current_offset: 0,
            total_rows: None,
        })
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

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.pool.is_none() {
            self.connect().await?;
        }

        let pool = self.pool.as_ref().unwrap();

        // Get table info for column definitions
        let table_info = sqlx::query(&format!("PRAGMA table_info(\"{}\")", self.table_name))
            .fetch_all(pool)
            .await?;

        let mut columns = Vec::new();
        for row in table_info {
            let name: String = row.get(1);
            let sql_type: String = row.get(2);
            let not_null: bool = row.get(3);

            let data_type = match sql_type.to_uppercase().as_str() {
                "INTEGER" | "INT" => DataType::Integer,
                "REAL" | "FLOAT" | "DOUBLE" | "NUMERIC" | "DECIMAL" => DataType::Decimal,
                "TEXT" | "VARCHAR" => DataType::String,
                "BOOLEAN" | "BOOL" => DataType::Boolean,
                "DATE" => DataType::Date,
                "DATETIME" | "TIMESTAMP" => DataType::DateTime,
                _ => DataType::String,
            };

            columns.push(SchemaColumn {
                name,
                data_type,
                nullable: !not_null,
            });
        }

        // Get estimated row count
        let count_result = sqlx::query(&format!(
            "SELECT COUNT(*) as count FROM \"{}\"",
            self.table_name
        ))
        .fetch_one(pool)
        .await?;
        let estimated_rows: i64 = count_result.get("count");

        // Store total rows for pagination
        self.total_rows = Some(estimated_rows as usize);

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

        let pool = self.pool.as_ref().unwrap();

        // Use LIMIT and OFFSET for proper pagination
        let query = format!(
            "SELECT * FROM \"{}\" LIMIT {} OFFSET {}",
            self.table_name, batch_size, self.current_offset
        );
        let rows = sqlx::query(&query).fetch_all(pool).await?;

        // Update offset for next batch
        self.current_offset += rows.len();

        let mut result_rows = Vec::new();
        for row in rows {
            let mut data_row = Row::new();

            // Get column info
            for (i, column) in row.columns().iter().enumerate() {
                let column_name = column.name();

                // This is a simplified value extraction - in practice we'd need proper type handling
                let value = if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                    match val {
                        Some(s) => Value::String(s),
                        None => Value::Null,
                    }
                } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                    match val {
                        Some(i) => Value::Integer(i),
                        None => Value::Null,
                    }
                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                    match val {
                        Some(f) => {
                            // Convert f64 to Decimal
                            match Decimal::try_from(f) {
                                Ok(d) => Value::Decimal(d),
                                Err(_) => Value::String(f.to_string()),
                            }
                        }
                        None => Value::Null,
                    }
                } else {
                    Value::Null
                };

                data_row.insert(column_name.to_string(), value);
            }

            result_rows.push(data_row);
        }

        Ok(result_rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        if let Some(pool) = &self.pool {
            let count_result = sqlx::query(&format!(
                "SELECT COUNT(*) as count FROM {}",
                self.table_name
            ))
            .fetch_one(pool)
            .await?;
            let count: i64 = count_result.get("count");
            Ok(Some(count as usize))
        } else {
            Ok(None)
        }
    }

    async fn reset(&mut self) -> Result<()> {
        // Reset pagination state
        self.current_offset = 0;
        self.query = None;
        Ok(())
    }

    fn has_more(&self) -> bool {
        // Check if there are more rows to read based on pagination state
        if let Some(total) = self.total_rows {
            self.current_offset < total
        } else {
            false
        }
    }
}

pub struct SqliteTarget {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
}

impl SqliteTarget {
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

        sqlx::query(&create_sql).execute(pool).await?;
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.pool.is_none() {
            return Err(TinyEtlError::Connection("Pool not connected".to_string()));
        }

        if rows.is_empty() {
            return Ok(0);
        }

        let pool = self.pool.as_ref().unwrap();

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

            let mut query = sqlx::query(&insert_sql);

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

            let result = query.execute(pool).await?;
            total_written += result.rows_affected() as usize;
        }

        Ok(total_written)
    }

    async fn finalize(&mut self) -> Result<()> {
        // SQLite doesn't require explicit finalization
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
        if let Some(pool) = &self.pool {
            let actual_table_name = if table_name.is_empty() {
                &self.table_name
            } else {
                table_name
            };

            sqlx::query(&format!("DELETE FROM \"{}\"", actual_table_name))
                .execute(pool)
                .await?;
        }
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
