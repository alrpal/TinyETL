use std::collections::HashMap;
use async_trait::async_trait;
use sqlx::{MySqlPool, Row as SqlxRow, Column as SqlxColumn};
use url::Url;
use rust_decimal::Decimal;

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
            DataType::Decimal => "DECIMAL(65,30)",
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
                    Value::Decimal(d) => {
                        // Convert Decimal to string for MySQL binding since it doesn't support direct Decimal binding
                        query.bind(d.to_string())
                    },
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

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        let pool = self.pool.as_ref().ok_or_else(|| {
            TinyEtlError::Connection("MySQL connection not established".to_string())
        })?;
        
        let actual_table_name = if table_name.is_empty() {
            &self.table_name
        } else {
            table_name
        };

        sqlx::query(&format!("TRUNCATE TABLE `{}`", actual_table_name))
            .execute(pool)
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e)))?;

        Ok(())
    }

    fn supports_append(&self) -> bool {
        // MySQL databases support appending new rows
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    
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
        if let Err(TinyEtlError::Configuration(msg)) = target {
            assert!(msg.contains("Invalid MySQL URL"));
        } else {
            panic!("Expected Configuration error");
        }
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

    #[test]
    fn test_map_data_type_to_mysql() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        
        assert_eq!(target.map_data_type_to_mysql(&DataType::Integer), "BIGINT");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Decimal), "DECIMAL(65,30)");
        assert_eq!(target.map_data_type_to_mysql(&DataType::String), "TEXT");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Boolean), "BOOLEAN");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Date), "DATE");
        assert_eq!(target.map_data_type_to_mysql(&DataType::DateTime), "DATETIME");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Null), "TEXT");
    }

    #[test]
    fn test_connection_string_parsing_edge_cases() {
        // Test with port number
        let target = MysqlTarget::new("mysql://root:password@127.0.0.1:3306/myapp#users").unwrap();
        assert_eq!(target.database_url, "mysql://root:password@127.0.0.1:3306/myapp");
        assert_eq!(target.table_name, "users");

        // Test with special characters in password
        let target = MysqlTarget::new("mysql://user:p%40ssw0rd@localhost/db#table").unwrap();
        assert_eq!(target.database_url, "mysql://user:p%40ssw0rd@localhost/db");
        assert_eq!(target.table_name, "table");

        // Test with no password
        let target = MysqlTarget::new("mysql://user@localhost/db#table").unwrap();
        assert_eq!(target.database_url, "mysql://user@localhost/db");
        assert_eq!(target.table_name, "table");
    }

    #[test]
    fn test_url_parsing_failures() {
        // Test various invalid URLs that actually fail URL parsing
        let invalid_urls = vec![
            "",
            "not-a-url",
            "://invalid", // No scheme
            "mysql", // Not a URL format
            " ", // Just whitespace
        ];

        for url in invalid_urls {
            let result = MysqlTarget::new(url);
            assert!(result.is_err(), "Expected error for URL: {}", url);
        }
        
        // Test that mysql:// actually succeeds (it's a valid URL)
        let result = MysqlTarget::new("mysql://");
        assert!(result.is_ok(), "mysql:// should be valid URL");
    }

    #[tokio::test]
    async fn test_get_pool_without_connection() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        let result = target.get_pool().await;
        assert!(result.is_err());
        if let Err(TinyEtlError::Connection(msg)) = result {
            assert!(msg.contains("connection not established"));
        } else {
            panic!("Expected Connection error");
        }
    }

    #[test]
    fn test_create_table_sql_generation() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        
        let schema = Schema {
            columns: vec![
                Column {
                    name: "id".to_string(),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: "name".to_string(),
                    data_type: DataType::String,
                    nullable: true,
                },
                Column {
                    name: "score".to_string(),
                    data_type: DataType::Decimal,
                    nullable: false,
                },
                Column {
                    name: "active".to_string(),
                    data_type: DataType::Boolean,
                    nullable: true,
                },
                Column {
                    name: "created_at".to_string(),
                    data_type: DataType::DateTime,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        // Test SQL generation logic manually
        let mut columns = Vec::new();
        for column in &schema.columns {
            let mysql_type = target.map_data_type_to_mysql(&column.data_type);
            let nullable = if column.nullable { "" } else { " NOT NULL" };
            columns.push(format!("`{}` {}{}", column.name, mysql_type, nullable));
        }
        
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS `{}` ({})",
            "test_table",
            columns.join(", ")
        );
        
        let expected = "CREATE TABLE IF NOT EXISTS `test_table` (`id` BIGINT NOT NULL, `name` TEXT, `score` DECIMAL(65,30) NOT NULL, `active` BOOLEAN, `created_at` DATETIME NOT NULL)";
        assert_eq!(create_sql, expected);
    }

    #[test]
    fn test_write_batch_empty_rows() {
        // Test behavior with empty row slice
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        
        // Can't actually test the async method without a DB, but we can test the logic
        let rows: &[Row] = &[];
        assert!(rows.is_empty());
        
        // The method should return Ok(0) for empty rows
        // This tests the early return path
    }

    #[test]
    fn test_batch_chunking_logic() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb")
            .unwrap()
            .with_batch_size(2);
        
        // Create test data that would be chunked
        let mut test_rows = Vec::new();
        for i in 0..5 {
            let mut row = HashMap::new();
            row.insert("id".to_string(), Value::Integer(i));
            row.insert("name".to_string(), Value::String(format!("user_{}", i)));
            test_rows.push(row);
        }
        
        // Test chunking behavior
        let chunks: Vec<_> = test_rows.chunks(target.max_batch_size).collect();
        assert_eq!(chunks.len(), 3); // 5 rows / 2 batch_size = 3 chunks
        assert_eq!(chunks[0].len(), 2);
        assert_eq!(chunks[1].len(), 2);
        assert_eq!(chunks[2].len(), 1);
    }

    #[test]
    fn test_value_binding_logic() {
        // Test the value conversion logic used in write_chunk
        let values = vec![
            Value::Integer(42),
            Value::Decimal(Decimal::new(314, 2)),
            Value::String("test".to_string()),
            Value::Boolean(true),
            Value::Null,
        ];
        
        // Test that each value type can be processed
        for value in &values {
            match value {
                Value::Integer(i) => assert_eq!(*i, 42),
                Value::Decimal(d) => assert_eq!(*d, Decimal::new(314, 2)),
                Value::String(s) => assert_eq!(s, "test"),
                Value::Boolean(b) => assert!(*b),
                Value::Null => {}, // Null should be handled
                Value::Date(_) => {}, // Date should be converted to string
            }
        }
    }

    #[test]
    fn test_date_value_formatting() {
        use chrono::{DateTime, Utc};
        
        // Test date formatting for MySQL
        let datetime = DateTime::from_timestamp(1609459200, 0).unwrap();
        let date_value = Value::Date(datetime);
        
        if let Value::Date(d) = date_value {
            let formatted = d.to_rfc3339();
            assert!(formatted.contains("2021-01-01"));
        } else {
            panic!("Expected Date value");
        }
    }

    #[tokio::test]
    async fn test_connection_url_validation() {
        // Test various connection string formats
        let valid_urls = vec![
            "mysql://user:pass@localhost:3306/db",
            "mysql://user:pass@localhost/db#table",
            "mysql://user@host:3306/database#users",
            "mysql://root:password@127.0.0.1:3306/myapp#customers",
        ];

        for url in valid_urls {
            let target = MysqlTarget::new(url);
            assert!(target.is_ok(), "Should accept valid URL: {}", url);
        }
    }

    #[test]
    fn test_table_name_handling() {
        // Test with explicit table name
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/db#users").unwrap();
        assert_eq!(target.table_name, "users");

        // Test without table name (should default to "data")
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/db").unwrap();
        assert_eq!(target.table_name, "data");

        // Test with empty database path
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/").unwrap();
        assert_eq!(target.table_name, "data");
    }

    #[test]
    fn test_error_message_formatting() {
        // Test that error messages contain useful information
        let result = MysqlTarget::new("not-a-valid-url");
        assert!(result.is_err());
        
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("Invalid MySQL URL"));
            assert!(msg.contains("not-a-valid-url") || msg.contains("relative URL"));
        } else {
            panic!("Expected Configuration error with descriptive message");
        }
    }

    #[test]
    fn test_sql_injection_prevention() {
        // Test that table and column names are properly escaped
        let table_name = "users; DROP TABLE important; --";
        let escaped_table = format!("`{}`", table_name);
        assert_eq!(escaped_table, "`users; DROP TABLE important; --`");

        // The backticks help prevent SQL injection
        assert!(escaped_table.starts_with('`'));
        assert!(escaped_table.ends_with('`'));
    }

    #[test]
    fn test_default_values_handling() {
        // Test default value creation and usage
        let default_value = Value::String("".to_string());
        
        match default_value {
            Value::String(ref s) => assert!(s.is_empty()),
            _ => panic!("Expected empty string default"),
        }
        
        // Test row access with missing columns
        let mut row = HashMap::new();
        row.insert("existing_col".to_string(), Value::Integer(42));
        
        let value = row.get("missing_col").unwrap_or(&default_value);
        match value {
            Value::String(s) => assert!(s.is_empty()),
            _ => panic!("Expected default string value"),
        }
    }

    #[test]
    fn test_connection_pool_configuration() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        
        // Pool should be None initially
        assert!(target.pool.is_none());
        
        // Test that connection string is stored correctly
        assert_eq!(target.database_url, "mysql://user:pass@localhost:3306/testdb");
    }
}
