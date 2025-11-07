use std::collections::HashMap;
use async_trait::async_trait;
use sqlx::{MySqlPool, Row as SqlxRow, Column as SqlxColumn};
use url::Url;

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, Column, DataType, SchemaInferer},
    connectors::Target,
};

pub struct MysqlTarget {
    connection_string: String,
    database_url: String,
    table_name: String,
    pool: Option<MySqlPool>,
    max_batch_size: usize,
}

impl MysqlTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (db_url, table_name) = Self::parse_connection_string(connection_string)?;
        
        Ok(Self {
            connection_string: connection_string.to_string(),
            database_url: db_url,
            table_name,
            pool: None,
            max_batch_size: 1000, // Default to 1000 rows per batch
        })
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.max_batch_size = batch_size.max(1); // Ensure at least 1
        self
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        if let Some((db_part, table_part)) = connection_string.split_once('#') {
            Ok((db_part.to_string(), table_part.to_string()))
        } else {
            // Extract database name from mysql://user:pass@host:port/dbname
            let url = Url::parse(connection_string).map_err(|e| {
                TinyEtlError::Configuration(format!("Invalid MySQL URL: {}", e))
            })?;
            let db_name = url.path().trim_start_matches('/');
            let default_table = if db_name.is_empty() { "data" } else { "data" };
            Ok((connection_string.to_string(), default_table.to_string()))
        }
    }

    async fn get_pool(&self) -> Result<&MySqlPool> {
        self.pool.as_ref().ok_or_else(|| {
            TinyEtlError::Connection("MySQL connection not established".to_string())
        })
    }

    async fn verify_database_exists(&self) -> Result<()> {
        // Extract database name from the URL
        let url = Url::parse(&self.database_url).map_err(|e| {
            TinyEtlError::Configuration(format!("Invalid MySQL URL: {}", e))
        })?;
        
        let db_name = url.path().trim_start_matches('/');
        if db_name.is_empty() {
            return Err(TinyEtlError::Configuration(
                "No database name specified in MySQL connection URL".to_string()
            ));
        }

        // Create a connection to MySQL without specifying a database
        let mut base_url = url.clone();
        base_url.set_path("");
        
        let base_connection_string = base_url.as_str();
        let pool = MySqlPool::connect(base_connection_string)
            .await
            .map_err(|e| TinyEtlError::Connection(format!(
                "Failed to connect to MySQL server: {}", e
            )))?;

        // Check if the database exists
        let result = sqlx::query("SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?")
            .bind(db_name)
            .fetch_one(&pool)
            .await
            .map_err(|e| TinyEtlError::Connection(format!(
                "Failed to check database existence: {}", e
            )))?;

        let count: i64 = result.get(0);
        if count == 0 {
            return Err(TinyEtlError::Connection(format!(
                "Database '{}' does not exist", db_name
            )));
        }

        pool.close().await;
        Ok(())
    }

    fn map_data_type_to_mysql(&self, data_type: &DataType) -> &'static str {
        match data_type {
            DataType::Integer => "BIGINT",
            DataType::Float => "DOUBLE",
            DataType::String => "TEXT",
            DataType::Boolean => "BOOLEAN",
            DataType::Date => "DATE",
            DataType::DateTime => "DATETIME",
            DataType::Null => "TEXT",
        }
    }

    async fn write_chunk(&self, pool: &MySqlPool, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        // Get column names from the first row
        let columns: Vec<String> = rows[0].keys().cloned().collect();
        let num_columns = columns.len();
        
        // Build the base INSERT statement with multiple VALUES clauses
        let column_names = columns.iter()
            .map(|c| format!("`{}`", c))
            .collect::<Vec<_>>()
            .join(", ");
        
        // Create placeholders for all rows: (?, ?, ?), (?, ?, ?), ...
        let values_placeholders = rows.iter()
            .map(|_| {
                let row_placeholders = (0..num_columns)
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", row_placeholders)
            })
            .collect::<Vec<_>>()
            .join(", ");
        
        let insert_sql = format!(
            "INSERT INTO `{}` ({}) VALUES {}",
            self.table_name,
            column_names,
            values_placeholders
        );

        // Build the query with all parameter bindings
        let mut query = sqlx::query(&insert_sql);
        let default_value = Value::String("".to_string());
        
        // Bind all values for all rows in the correct order
        for row in rows {
            for column in &columns {
                let value = row.get(column).unwrap_or(&default_value);
                query = match value {
                    Value::Integer(i) => query.bind(i),
                    Value::Float(f) => query.bind(f),
                    Value::String(s) => query.bind(s),
                    Value::Boolean(b) => query.bind(b),
                    Value::Date(d) => query.bind(d.to_rfc3339()),
                    Value::Null => query.bind(None::<String>),
                };
            }
        }
        
        // Execute the batch insert
        let result = query.execute(pool).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to batch insert {} rows into MySQL: {}", rows.len(), e))
        })?;
        
        Ok(result.rows_affected() as usize)
    }
}

#[async_trait]
impl Target for MysqlTarget {
    async fn connect(&mut self) -> Result<()> {
        // First verify that the database exists
        self.verify_database_exists().await?;
        
        let pool = MySqlPool::connect(&self.database_url)
            .await
            .map_err(|e| TinyEtlError::Connection(format!(
                "Failed to connect to MySQL database: {}", e
            )))?;
        
        self.pool = Some(pool);
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        // Determine the actual table name to use
        let actual_table_name = if table_name.is_empty() {
            self.table_name.clone()
        } else {
            table_name.to_string()
        };

        // Update internal table name
        self.table_name = actual_table_name.clone();
        
        // Get pool after updating table name to avoid borrowing conflicts
        let pool = self.get_pool().await?;
        
        let mut columns = Vec::new();
        for column in &schema.columns {
            let mysql_type = self.map_data_type_to_mysql(&column.data_type);
            let nullable = if column.nullable { "" } else { " NOT NULL" };
            columns.push(format!("`{}` {}{}", column.name, mysql_type, nullable));
        }
        
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS `{}` ({})",
            actual_table_name,
            columns.join(", ")
        );
        
        sqlx::query(&create_sql)
            .execute(pool)
            .await
            .map_err(|e| TinyEtlError::Connection(format!(
                "Failed to create MySQL table '{}': {}", actual_table_name, e
            )))?;
        
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        let pool = self.get_pool().await?;
        let mut total_affected = 0;
        
        // Process rows in chunks to avoid hitting MySQL limits
        for chunk in rows.chunks(self.max_batch_size) {
            total_affected += self.write_chunk(pool, chunk).await?;
        }
        
        Ok(total_affected)
    }

    async fn finalize(&mut self) -> Result<()> {
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        let pool = self.pool.as_ref().ok_or_else(|| {
            TinyEtlError::Connection("MySQL connection not established".to_string())
        })?;
        
        let actual_table_name = if table_name.is_empty() {
            &self.table_name
        } else {
            table_name
        };
        
        let result = sqlx::query("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = ? AND table_schema = DATABASE()")
            .bind(actual_table_name)
            .fetch_one(pool)
            .await;
            
        match result {
            Ok(row) => Ok(row.get::<i64, _>(0) > 0),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_connection_string_with_table() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb#employees");
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(target.database_url, "mysql://user:pass@localhost:3306/testdb");
        assert_eq!(target.table_name, "employees");
    }

    #[test]
    fn test_parse_connection_string_without_table() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb");
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(target.database_url, "mysql://user:pass@localhost:3306/testdb");
        assert_eq!(target.table_name, "data");
    }

    #[test]
    fn test_invalid_connection_string() {
        let target = MysqlTarget::new("invalid-url");
        assert!(target.is_err());
    }

    #[test]
    fn test_database_name_extraction() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/mydb#table").unwrap();
        let url = url::Url::parse(&target.database_url).unwrap();
        let db_name = url.path().trim_start_matches('/');
        assert_eq!(db_name, "mydb");
    }

    #[test]
    fn test_empty_database_name() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/").unwrap();
        let url = url::Url::parse(&target.database_url).unwrap();
        let db_name = url.path().trim_start_matches('/');
        assert_eq!(db_name, "");
    }

    #[test]
    fn test_batch_insert_sql_generation() {
        // Test that we can generate proper batch INSERT SQL
        let columns = vec!["id".to_string(), "name".to_string(), "age".to_string()];
        let num_rows = 3;
        let num_columns = columns.len();
        
        let column_names = columns.iter()
            .map(|c| format!("`{}`", c))
            .collect::<Vec<_>>()
            .join(", ");
        
        let values_placeholders = (0..num_rows)
            .map(|_| {
                let row_placeholders = (0..num_columns)
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", row_placeholders)
            })
            .collect::<Vec<_>>()
            .join(", ");
        
        let insert_sql = format!(
            "INSERT INTO `{}` ({}) VALUES {}",
            "test_table",
            column_names,
            values_placeholders
        );
        
        let expected = "INSERT INTO `test_table` (`id`, `name`, `age`) VALUES (?, ?, ?), (?, ?, ?), (?, ?, ?)";
        assert_eq!(insert_sql, expected);
    }

    #[test]
    fn test_batch_size_configuration() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb")
            .unwrap()
            .with_batch_size(500);
        assert_eq!(target.max_batch_size, 500);
        
        // Test minimum batch size of 1
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb")
            .unwrap()
            .with_batch_size(0);
        assert_eq!(target.max_batch_size, 1);
    }
}
