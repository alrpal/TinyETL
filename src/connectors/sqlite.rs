use std::path::PathBuf;
use async_trait::async_trait;
use sqlx::{SqlitePool, Row as SqlxRow, Column};

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, Column as SchemaColumn, DataType, SchemaInferer},
    connectors::{Source, Target}
};

pub struct SqliteSource {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
    query: Option<String>,
}

impl SqliteSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - could be "file.db" or "sqlite:file.db#table" or "file.db#table"
        let (db_path, table) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "SQLite connection string format: file.db#table".to_string()
                ));
            }
            (parts[0].trim_start_matches("sqlite:"), parts[1])
        } else {
            return Err(TinyEtlError::Configuration(
                "SQLite source requires table specification: file.db#table".to_string()
            ));
        };

        Ok(Self {
            connection_string: format!("sqlite:{}", db_path),
            pool: None,
            table_name: table.to_string(),
            query: None,
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

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.pool.is_none() {
            self.connect().await?;
        }

        let pool = self.pool.as_ref().unwrap();
        
        // Get table info for column definitions
        let table_info = sqlx::query(&format!("PRAGMA table_info({})", self.table_name))
            .fetch_all(pool)
            .await?;

        let mut columns = Vec::new();
        for row in table_info {
            let name: String = row.get(1);
            let sql_type: String = row.get(2);
            let not_null: bool = row.get(3);
            
            let data_type = match sql_type.to_uppercase().as_str() {
                "INTEGER" | "INT" => DataType::Integer,
                "REAL" | "FLOAT" | "DOUBLE" => DataType::Float,
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
        let count_result = sqlx::query(&format!("SELECT COUNT(*) as count FROM {}", self.table_name))
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

        let pool = self.pool.as_ref().unwrap();
        
        // Simple implementation - in practice we'd need proper pagination
        let query = format!("SELECT * FROM {} LIMIT {}", self.table_name, batch_size);
        let rows = sqlx::query(&query).fetch_all(pool).await?;
        
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
                        Some(f) => Value::Float(f),
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
            let count_result = sqlx::query(&format!("SELECT COUNT(*) as count FROM {}", self.table_name))
                .fetch_one(pool)
                .await?;
            let count: i64 = count_result.get("count");
            Ok(Some(count as usize))
        } else {
            Ok(None)
        }
    }

    async fn reset(&mut self) -> Result<()> {
        // For SQLite sources, reset means preparing for a new query
        self.query = None;
        Ok(())
    }

    fn has_more(&self) -> bool {
        // Simplified - in practice we'd track pagination state
        true
    }
}

pub struct SqliteTarget {
    connection_string: String,
    pool: Option<SqlitePool>,
    table_name: String,
}

impl SqliteTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - could be "file.db" or "sqlite:file.db#table" or "file.db#table"
        let (db_path, table) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "SQLite connection string format: file.db#table".to_string()
                ));
            }
            (parts[0].trim_start_matches("sqlite:"), parts[1])
        } else {
            // Default table name if not specified
            (connection_string.trim_start_matches("sqlite:"), "data")
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
        match SqlitePool::connect(&self.connection_string).await {
            Ok(pool) => {
                self.pool = Some(pool);
                Ok(())
            }
            Err(e) => {
                Err(TinyEtlError::Connection(format!(
                    "Failed to connect to SQLite database '{}': {}. Check file path and permissions.", 
                    db_path.display(), 
                    e
                )))
            }
        }
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.pool.is_none() {
            self.connect().await?;
        }

        let pool = self.pool.as_ref().unwrap();
        
        // Override table name if provided
        let actual_table_name = if table_name.is_empty() {
            &self.table_name
        } else {
            table_name
        };

        // Build CREATE TABLE statement
        let column_definitions: Vec<String> = schema.columns.iter().map(|col| {
            let nullable = if col.nullable { "" } else { " NOT NULL" };
            format!("{} {}{}", col.name, col.data_type, nullable)
        }).collect();

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
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
        let placeholders = vec!["?"; columns.len()].join(", ");
        
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            self.table_name,
            columns.join(", "),
            placeholders
        );

        let mut written_count = 0;
        for row in rows {
            let mut query = sqlx::query(&insert_sql);
            
            for column in &columns {
                let value = row.get(column).unwrap_or(&Value::Null);
                query = match value {
                    Value::String(s) => query.bind(s),
                    Value::Integer(i) => query.bind(*i),
                    Value::Float(f) => query.bind(*f),
                    Value::Boolean(b) => query.bind(*b),
                    Value::Date(dt) => query.bind(dt.to_rfc3339()),
                    Value::Null => query.bind(None::<String>),
                };
            }
            
            query.execute(pool).await?;
            written_count += 1;
        }

        Ok(written_count)
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

            let result = sqlx::query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name=?"
            )
            .bind(actual_table_name)
            .fetch_optional(pool)
            .await?;

            Ok(result.is_some())
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

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
