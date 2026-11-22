use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use chrono::{DateTime, Utc};
use duckdb::{types::ValueRef, Connection};
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    connectors::{row_stream::RowStream, Source, Target},
    schema::{Column as SchemaColumn, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

/// Efficient batch insert using DuckDB's Appender API
fn insert_with_appender(
    conn: &Connection,
    table_name: &str,
    rows: &[Row],
    schema: &Schema,
) -> Result<usize> {
    let mut appender = conn
        .appender(table_name)
        .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create appender: {}", e)))?;

    // Insert each row using the appender
    for row in rows {
        // Collect values in schema order as owned values
        let mut row_values: Vec<duckdb::types::Value> = Vec::new();

        for col in &schema.columns {
            let value = row.get(&col.name).unwrap_or(&Value::Null);

            let duckdb_value = match value {
                Value::String(s) => duckdb::types::Value::Text(s.clone()),
                Value::Integer(i) => duckdb::types::Value::BigInt(*i),
                Value::Decimal(d) => {
                    // Convert decimal to f64 for DuckDB
                    let f: f64 = (*d).try_into().unwrap_or(0.0);
                    duckdb::types::Value::Double(f)
                }
                Value::Boolean(b) => duckdb::types::Value::Boolean(*b),
                Value::Date(dt) => {
                    // Convert datetime to string for DuckDB
                    let timestamp_str = dt.to_rfc3339();
                    duckdb::types::Value::Text(timestamp_str)
                }
                Value::Json(j) => {
                    // DuckDB can store JSON as text or use JSON type
                    let json_str = serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string());
                    duckdb::types::Value::Text(json_str)
                }
                Value::Null => duckdb::types::Value::Null,
            };

            row_values.push(duckdb_value);
        }

        // Convert to refs for appender
        let params_refs: Vec<&dyn duckdb::types::ToSql> = row_values
            .iter()
            .map(|v| v as &dyn duckdb::types::ToSql)
            .collect();

        appender
            .append_row(params_refs.as_slice())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to append row: {}", e)))?;
    }

    // Flush the appender to commit the data
    appender
        .flush()
        .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to flush appender: {}", e)))?;

    Ok(rows.len())
}

/// DuckDB source connector for reading data from DuckDB databases
pub struct DuckdbSource {
    connection_string: String,
    connection: Option<Arc<Mutex<Connection>>>,
    table_name: String,
    /// One streaming `SELECT` for the whole read (no OFFSET paging), on its own thread
    stream: Option<RowStream>,
    consumed: bool,
}

impl DuckdbSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - could be "file.duckdb" or "duckdb:file.duckdb#table" or "file.duckdb#table"
        let (db_path, table) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "DuckDB connection string format: file.duckdb#table".to_string(),
                ));
            }
            (parts[0].trim_start_matches("duckdb:"), parts[1])
        } else {
            return Err(TinyEtlError::Configuration(
                "DuckDB source requires table specification: file.duckdb#table".to_string(),
            ));
        };

        Ok(Self {
            connection_string: db_path.to_string(),
            connection: None,
            table_name: table.to_string(),
            stream: None,
            consumed: false,
        })
    }

    /// Run `SELECT *` once on a cloned connection, on its own thread, and stream the rows.
    fn start_stream(&self) -> Result<RowStream> {
        let conn = self
            .connection
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_clone()
            .map_err(|e| TinyEtlError::Connection(format!("Failed to open a connection: {}", e)))?;
        let sql = format!("SELECT * FROM \"{}\"", self.table_name);
        RowStream::spawn_blocking(move |mut sink| {
            let result = (|| -> std::result::Result<(), String> {
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare query: {}", e))?;
                let mut rows = stmt
                    .query([])
                    .map_err(|e| format!("Failed to execute query: {}", e))?;
                let mut names: Vec<String> = Vec::new();
                while let Some(row) = rows
                    .next()
                    .map_err(|e| format!("Failed to fetch row: {}", e))?
                {
                    if names.is_empty() {
                        let count = row.as_ref().column_count();
                        names = (0..count)
                            .map(|i| {
                                row.as_ref()
                                    .column_name(i)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|_| format!("column_{}", i))
                            })
                            .collect();
                    }
                    let values: Row = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| (name.clone(), value_at(row, i)))
                        .collect();
                    if !sink.blocking_push(values) {
                        return Ok(()); // the reader is gone
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => sink.blocking_finish(),
                Err(message) => sink.blocking_fail(message),
            }
        })
    }
}

/// A DuckDB value as a TinyETL value, keeping numeric and date/time types.
fn value_at(row: &duckdb::Row, i: usize) -> Value {
    let Ok(value_ref) = row.get_ref(i) else {
        return Value::Null;
    };
    match value_ref {
        ValueRef::Null => Value::Null,
        ValueRef::Boolean(b) => Value::Boolean(b),
        ValueRef::TinyInt(n) => Value::Integer(n as i64),
        ValueRef::SmallInt(n) => Value::Integer(n as i64),
        ValueRef::Int(n) => Value::Integer(n as i64),
        ValueRef::BigInt(n) => Value::Integer(n),
        ValueRef::HugeInt(n) => match i64::try_from(n) {
            Ok(n) => Value::Integer(n),
            Err(_) => Value::String(n.to_string()),
        },
        ValueRef::UTinyInt(n) => Value::Integer(n as i64),
        ValueRef::USmallInt(n) => Value::Integer(n as i64),
        ValueRef::UInt(n) => Value::Integer(n as i64),
        ValueRef::UBigInt(n) => match i64::try_from(n) {
            Ok(n) => Value::Integer(n),
            Err(_) => Value::String(n.to_string()),
        },
        ValueRef::Float(f) => match Decimal::try_from(f) {
            Ok(d) => Value::Decimal(d),
            Err(_) => Value::String(f.to_string()),
        },
        ValueRef::Double(f) => match Decimal::try_from(f) {
            Ok(d) => Value::Decimal(d),
            Err(_) => Value::String(f.to_string()),
        },
        ValueRef::Decimal(d) => Value::Decimal(d),
        ValueRef::Timestamp(unit, value) => {
            let micros = unit.to_micros(value);
            match DateTime::<Utc>::from_timestamp_micros(micros) {
                Some(dt) => Value::Date(dt),
                None => Value::Null,
            }
        }
        ValueRef::Date32(days) => DateTime::<Utc>::from_timestamp(days as i64 * 86_400, 0)
            .map_or(Value::Null, Value::Date),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => Value::String(s.to_string()),
            Err(_) => Value::Null,
        },
        ValueRef::Blob(bytes) => Value::String(general_purpose::STANDARD.encode(bytes)),
        _ => row.get::<_, String>(i).map_or(Value::Null, Value::String),
    }
}

#[async_trait]
impl Source for DuckdbSource {
    async fn connect(&mut self) -> Result<()> {
        // DuckDB connection is synchronous, but we wrap it for async compatibility
        let conn = Connection::open(&self.connection_string)
            .map_err(|e| TinyEtlError::Connection(format!(
                "Failed to connect to DuckDB database '{}': {}. Make sure the file exists and is readable.",
                self.connection_string,
                e
            )))?;

        self.connection = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref().unwrap();
        let conn = conn.lock().unwrap();

        // Get table schema using PRAGMA or DESCRIBE
        let query = format!("DESCRIBE \"{}\"", self.table_name);
        let mut stmt = conn.prepare(&query).map_err(|e| {
            TinyEtlError::DataTransfer(format!(
                "Failed to describe table '{}': {}",
                self.table_name, e
            ))
        })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?, // column_name
                    row.get::<_, String>(1)?, // column_type
                    row.get::<_, String>(2)?, // null
                ))
            })
            .map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to query table schema: {}", e))
            })?;

        let mut columns = Vec::new();
        for row_result in rows {
            let (name, duckdb_type, null_str) = row_result.map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to read schema row: {}", e))
            })?;

            let data_type = match duckdb_type.to_uppercase().as_str() {
                t if t.contains("INT") || t.contains("INTEGER") => DataType::Integer,
                t if t.contains("DOUBLE")
                    || t.contains("FLOAT")
                    || t.contains("REAL")
                    || t.contains("DECIMAL")
                    || t.contains("NUMERIC") =>
                {
                    DataType::Decimal
                }
                t if t.contains("VARCHAR") || t.contains("TEXT") || t.contains("STRING") => {
                    DataType::String
                }
                t if t.contains("BOOL") => DataType::Boolean,
                t if t.contains("DATE") && !t.contains("TIME") => DataType::Date,
                t if t.contains("TIMESTAMP") || t.contains("DATETIME") => DataType::DateTime,
                _ => DataType::String,
            };

            let nullable = null_str.to_uppercase() == "YES";

            columns.push(SchemaColumn {
                name,
                data_type,
                nullable,
            });
        }

        // Get estimated row count
        let count_query = format!("SELECT COUNT(*) FROM \"{}\"", self.table_name);
        let count: i64 = conn
            .query_row(&count_query, [], |row| row.get(0))
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to count rows: {}", e)))?;

        Ok(Schema {
            columns,
            estimated_rows: Some(count as usize),
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.connection.is_none() {
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
        if let Some(conn) = &self.connection {
            let conn = conn.lock().unwrap();
            let count_query = format!("SELECT COUNT(*) FROM \"{}\"", self.table_name);
            let count: i64 = conn
                .query_row(&count_query, [], |row| row.get(0))
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to count rows: {}", e)))?;
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

/// DuckDB target connector for writing data to DuckDB databases
pub struct DuckdbTarget {
    connection_string: String,
    connection: Option<Arc<Mutex<Connection>>>,
    table_name: String,
    schema: Option<Schema>, // Cache the schema to avoid re-inference
    /// The load runs in one transaction, begun at the first write and committed in
    /// finalize; dropping the target without finalize rolls it back
    in_transaction: bool,
    /// truncate() was called: empty the table inside the load's transaction
    truncate_table: Option<String>,
}

impl DuckdbTarget {
    /// Open the load's transaction (once), emptying the table first if --truncate was given.
    fn begin(&mut self) -> Result<()> {
        if self.in_transaction {
            return Ok(());
        }
        let conn = self
            .connection
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Connection not established".to_string()))?
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        conn.execute_batch("BEGIN TRANSACTION").map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to start a transaction: {}", e))
        })?;
        if let Some(table) = self.truncate_table.take() {
            conn.execute(&format!("DELETE FROM \"{}\"", table), [])
                .map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e))
                })?;
        }
        drop(conn);
        self.in_transaction = true;
        Ok(())
    }
}

impl Drop for DuckdbTarget {
    fn drop(&mut self) {
        // The transfer failed before finalize: undo the partial load
        if self.in_transaction {
            if let Some(conn) = &self.connection {
                let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }
}

impl DuckdbTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - support multiple formats:
        // - "file.duckdb" or "file.duckdb#table" (legacy)
        // - "duckdb:file.duckdb" or "duckdb:file.duckdb#table"
        let normalized = connection_string.trim_start_matches("duckdb:");

        let (db_path, table) = if normalized.contains('#') {
            let parts: Vec<&str> = normalized.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "DuckDB connection string format: file.duckdb#table or duckdb:file.duckdb#table".to_string()
                ));
            }
            (parts[0], parts[1])
        } else {
            // Default table name if not specified
            (normalized, "data")
        };

        Ok(Self {
            connection_string: db_path.to_string(),
            connection: None,
            table_name: table.to_string(),
            schema: None,
            in_transaction: false,
            truncate_table: None,
        })
    }

    fn get_db_path(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(&self.connection_string))
    }

    fn map_data_type_to_duckdb(&self, data_type: &DataType) -> &'static str {
        match data_type {
            DataType::Integer => "BIGINT",
            DataType::Decimal => "DOUBLE",
            DataType::String => "VARCHAR",
            DataType::Boolean => "BOOLEAN",
            DataType::Date => "DATE",
            DataType::DateTime => "TIMESTAMP",
            DataType::Json => "JSON", // DuckDB has native JSON type
            DataType::Null => "VARCHAR",
        }
    }
}

#[async_trait]
impl Target for DuckdbTarget {
    async fn connect(&mut self) -> Result<()> {
        // Ensure parent directory exists for the database file
        let db_path = self.get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // DuckDB will automatically create the database file if it doesn't exist
        let conn = Connection::open(&self.connection_string).map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to connect to DuckDB database '{}': {}. Check file path and permissions.",
                db_path.display(),
                e
            ))
        })?;

        self.connection = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref().unwrap();
        let conn = conn.lock().unwrap();

        // Determine the actual table name to use
        let actual_table_name = if table_name.is_empty() {
            self.table_name.clone()
        } else {
            table_name.to_string()
        };

        // Update our internal table name to match what we're actually creating
        self.table_name = actual_table_name.clone();

        // Store the schema for efficient Arrow-based writes
        self.schema = Some(schema.clone());

        // Build CREATE TABLE statement with IF NOT EXISTS (append-first philosophy)
        let column_definitions: Vec<String> = schema
            .columns
            .iter()
            .map(|col| {
                let duckdb_type = self.map_data_type_to_duckdb(&col.data_type);
                let nullable = if col.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", col.name, duckdb_type, nullable)
            })
            .collect();

        // Use CREATE TABLE IF NOT EXISTS to support append-first philosophy
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            actual_table_name,
            column_definitions.join(", ")
        );

        conn.execute(&create_sql, [])
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create table: {}", e)))?;

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.connection.is_none() {
            return Err(TinyEtlError::Connection(
                "Connection not established".to_string(),
            ));
        }

        if rows.is_empty() {
            return Ok(0);
        }
        self.begin()?;

        let conn = self.connection.as_ref().unwrap();
        let conn = conn.lock().unwrap();

        // Use cached schema if available, otherwise infer from rows
        let schema = match &self.schema {
            Some(s) => s.clone(),
            None => {
                // Fallback: infer schema from rows (less efficient)
                crate::schema::SchemaInferer::infer_from_rows(rows)?
            }
        };

        // Use DuckDB's high-performance Appender API for batch insertion
        let affected = insert_with_appender(&conn, &self.table_name, rows, &schema)?;

        Ok(affected)
    }

    async fn finalize(&mut self) -> Result<()> {
        // A --truncate with no rows to load still has to empty the table
        if self.truncate_table.is_some() {
            self.begin()?;
        }
        if self.in_transaction {
            let conn = self.connection.as_ref().unwrap();
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch("COMMIT").map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to commit the load: {}", e))
            })?;
            drop(conn);
            self.in_transaction = false;
        }
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        if let Some(conn) = &self.connection {
            let conn = conn.lock().unwrap();
            let actual_table_name = if table_name.is_empty() {
                &self.table_name
            } else {
                table_name
            };

            let query = "SELECT table_name FROM information_schema.tables WHERE table_name = ?";
            let result = conn.query_row(query, [actual_table_name], |_| Ok(true));

            Ok(result.unwrap_or(false))
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
        // DuckDB databases support appending new rows
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duckdb_source_new() {
        let source = DuckdbSource::new("test.duckdb#users");
        assert!(source.is_ok());
    }

    #[test]
    fn test_duckdb_source_invalid_format() {
        let source = DuckdbSource::new("test.duckdb");
        assert!(source.is_err());
    }

    #[test]
    fn test_duckdb_target_new() {
        let target = DuckdbTarget::new("test.duckdb#users");
        assert!(target.is_ok());

        let target2 = DuckdbTarget::new("test.duckdb");
        assert!(target2.is_ok());
    }
}
