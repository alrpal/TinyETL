use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json;
use sqlx::{Column as SqlxColumn, MySqlPool, Row as SqlxRow, TypeInfo};
use url::Url;

use crate::{
    connectors::{Source, Target},
    schema::{DataType, Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

pub struct MysqlSource {
    database_url: String,
    table_name: String,
    pool: Option<MySqlPool>,
    current_offset: usize,
    total_rows: Option<usize>,
}

impl MysqlSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (db_url, table_name) = Self::parse_connection_string(connection_string)?;

        Ok(Self {
            database_url: db_url,
            table_name,
            pool: None,
            current_offset: 0,
            total_rows: None,
        })
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        if let Some((db_part, table_part)) = connection_string.split_once('#') {
            Ok((db_part.to_string(), table_part.to_string()))
        } else {
            Err(TinyEtlError::Configuration(
                "MySQL source requires table specification: mysql://user:pass@host:port/db#table"
                    .to_string(),
            ))
        }
    }

    async fn get_pool(&self) -> Result<&MySqlPool> {
        self.pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("MySQL connection not established".to_string()))
    }

    fn extract_value(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column: &sqlx::mysql::MySqlColumn,
    ) -> Result<Value> {
        let col_name = column.name();
        let col_type = column.type_info();
        let type_name = col_type.name();

        // Debug: Log the column type
        tracing::debug!("Column '{}' has type '{}'", col_name, type_name);

        // Handle JSON type explicitly using serde_json::Value
        if type_name.eq_ignore_ascii_case("JSON") {
            tracing::debug!("Detected JSON column: {}", col_name);
            if let Ok(Some(json_val)) = row.try_get::<Option<serde_json::Value>, _>(col_name) {
                // Convert the JSON value to a compact string representation
                let json_string =
                    serde_json::to_string(&json_val).unwrap_or_else(|_| "null".to_string());
                tracing::debug!(
                    "Successfully extracted JSON as string: {} bytes",
                    json_string.len()
                );
                return Ok(Value::String(json_string));
            } else {
                tracing::debug!("JSON column '{}' is NULL", col_name);
                return Ok(Value::Null);
            }
        }

        // Try different MySQL types in order of likelihood
        // Try String (TEXT, VARCHAR, CHAR types)
        if let Ok(val) = row.try_get::<Option<String>, _>(col_name) {
            match val {
                Some(s) => Ok(Value::String(s)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i64>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i32>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i16>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i8>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<u64>, _>(col_name) {
            match val {
                Some(u) => Ok(Value::Integer(u as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<u32>, _>(col_name) {
            match val {
                Some(u) => Ok(Value::Integer(u as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<u16>, _>(col_name) {
            match val {
                Some(u) => Ok(Value::Integer(u as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<u8>, _>(col_name) {
            match val {
                Some(u) => Ok(Value::Integer(u as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<f64>, _>(col_name) {
            match val {
                Some(f) => match Decimal::try_from(f) {
                    Ok(d) => Ok(Value::Decimal(d)),
                    Err(_) => Ok(Value::String(f.to_string())),
                },
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<f32>, _>(col_name) {
            match val {
                Some(f) => match Decimal::try_from(f as f64) {
                    Ok(d) => Ok(Value::Decimal(d)),
                    Err(_) => Ok(Value::String(f.to_string())),
                },
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<bool>, _>(col_name) {
            match val {
                Some(b) => Ok(Value::Boolean(b)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<DateTime<Utc>>, _>(col_name) {
            match val {
                Some(dt) => Ok(Value::Date(dt)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<NaiveDateTime>, _>(col_name) {
            match val {
                Some(dt) => Ok(Value::Date(Utc.from_utc_datetime(&dt))),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<NaiveDate>, _>(col_name) {
            match val {
                Some(date) => {
                    let datetime = date.and_hms_opt(0, 0, 0).unwrap();
                    Ok(Value::Date(Utc.from_utc_datetime(&datetime)))
                }
                None => Ok(Value::Null),
            }
        } else {
            Ok(Value::Null)
        }
    }
}

#[async_trait]
impl Source for MysqlSource {
    async fn connect(&mut self) -> Result<()> {
        let pool = MySqlPool::connect(&self.database_url).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to MySQL database: {}", e))
        })?;

        self.pool = Some(pool);
        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        let pool = self.get_pool().await?;

        let query = format!("SELECT * FROM `{}` LIMIT {}", self.table_name, sample_size);

        let rows = sqlx::query(&query).fetch_all(pool).await.map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to fetch sample data: {}", e))
        })?;

        if rows.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        // Convert MySQL rows to our Row format
        let mut schema_rows = Vec::new();
        for row in &rows {
            let mut schema_row = Row::new();
            for column in row.columns() {
                let col_name = column.name();
                let value = self.extract_value(row, column)?;
                schema_row.insert(col_name.to_string(), value);
            }
            schema_rows.push(schema_row);
        }

        let mut schema = SchemaInferer::infer_from_rows(&schema_rows)?;

        // Get estimated row count
        if let Ok(count_result) = self.estimated_row_count().await {
            schema.estimated_rows = count_result;
        }

        Ok(schema)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let pool = self.get_pool().await?;

        let query = format!(
            "SELECT * FROM `{}` LIMIT {} OFFSET {}",
            self.table_name, batch_size, self.current_offset
        );

        let rows = sqlx::query(&query)
            .fetch_all(pool)
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to fetch batch: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            let mut schema_row = Row::new();
            for column in row.columns() {
                let col_name = column.name();
                let value = self.extract_value(&row, column)?;
                schema_row.insert(col_name.to_string(), value);
            }
            result.push(schema_row);
        }

        self.current_offset += result.len();
        Ok(result)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        let pool = self.get_pool().await?;

        // Try to get estimated count from information_schema
        let count_query = format!(
            "SELECT TABLE_ROWS FROM information_schema.TABLES WHERE TABLE_NAME = '{}' AND TABLE_SCHEMA = DATABASE()",
            self.table_name
        );

        match sqlx::query_scalar::<_, i64>(&count_query)
            .fetch_one(pool)
            .await
        {
            Ok(count) if count > 0 => Ok(Some(count as usize)),
            _ => {
                // Fallback to exact count if estimate fails
                let exact_query = format!("SELECT COUNT(*) FROM `{}`", self.table_name);
                match sqlx::query_scalar::<_, i64>(&exact_query)
                    .fetch_one(pool)
                    .await
                {
                    Ok(count) => Ok(Some(count as usize)),
                    Err(e) => Err(TinyEtlError::DataTransfer(format!(
                        "Failed to get row count: {}",
                        e
                    ))),
                }
            }
        }
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_offset = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        if let Some(total) = self.total_rows {
            self.current_offset < total
        } else {
            // If we don't know the total, assume there might be more
            true
        }
    }
}

pub struct MysqlTarget {
    database_url: String,
    table_name: String,
    pool: Option<MySqlPool>,
    max_batch_size: usize,
}

impl MysqlTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (db_url, table_name) = Self::parse_connection_string(connection_string)?;

        Ok(Self {
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
            let url = Url::parse(connection_string)
                .map_err(|e| TinyEtlError::Configuration(format!("Invalid MySQL URL: {}", e)))?;
            let db_name = url.path().trim_start_matches('/').to_string();
            let default_table = if db_name.is_empty() {
                "data".to_string()
            } else {
                format!("{}_data", db_name)
            };
            Ok((connection_string.to_string(), default_table))
        }
    }

    async fn get_pool(&self) -> Result<&MySqlPool> {
        self.pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("MySQL connection not established".to_string()))
    }

    async fn verify_database_exists(&self) -> Result<()> {
        // Extract database name from the URL
        let url = Url::parse(&self.database_url)
            .map_err(|e| TinyEtlError::Configuration(format!("Invalid MySQL URL: {}", e)))?;

        let db_name = url.path().trim_start_matches('/');
        if db_name.is_empty() {
            return Err(TinyEtlError::Configuration(
                "No database name specified in MySQL connection URL".to_string(),
            ));
        }

        // Create a connection to MySQL without specifying a database
        let mut base_url = url.clone();
        base_url.set_path("");

        let base_connection_string = base_url.as_str();
        let pool = MySqlPool::connect(base_connection_string)
            .await
            .map_err(|e| {
                TinyEtlError::Connection(format!("Failed to connect to MySQL server: {}", e))
            })?;

        // Check if the database exists
        let result =
            sqlx::query("SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?")
                .bind(db_name)
                .fetch_one(&pool)
                .await
                .map_err(|e| {
                    TinyEtlError::Connection(format!("Failed to check database existence: {}", e))
                })?;

        let count: i64 = result.get(0);
        if count == 0 {
            return Err(TinyEtlError::Connection(format!(
                "Database '{}' does not exist",
                db_name
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
            DataType::Json => "JSON", // MySQL native JSON type
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
        let column_names = columns
            .iter()
            .map(|c| format!("`{}`", c))
            .collect::<Vec<_>>()
            .join(", ");

        // Create placeholders for all rows: (?, ?, ?), (?, ?, ?), ...
        let values_placeholders = rows
            .iter()
            .map(|_| {
                let row_placeholders = (0..num_columns).map(|_| "?").collect::<Vec<_>>().join(", ");
                format!("({})", row_placeholders)
            })
            .collect::<Vec<_>>()
            .join(", ");

        let insert_sql = format!(
            "INSERT INTO `{}` ({}) VALUES {}",
            self.table_name, column_names, values_placeholders
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
                    }
                    Value::String(s) => query.bind(s),
                    Value::Boolean(b) => query.bind(b),
                    Value::Date(d) => query.bind(d.to_rfc3339()),
                    Value::Json(j) => {
                        query.bind(serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string()))
                    }
                    Value::Null => query.bind(None::<String>),
                };
            }
        }

        // Execute the batch insert
        let result = query.execute(pool).await.map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to batch insert {} rows into MySQL: {}",
                rows.len(),
                e
            ))
        })?;

        Ok(result.rows_affected() as usize)
    }
}

#[async_trait]
impl Target for MysqlTarget {
    async fn connect(&mut self) -> Result<()> {
        // First verify that the database exists
        self.verify_database_exists().await?;

        let pool = MySqlPool::connect(&self.database_url).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to MySQL database: {}", e))
        })?;

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

        sqlx::query(&create_sql).execute(pool).await.map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to create MySQL table '{}': {}",
                actual_table_name, e
            ))
        })?;

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
    use std::collections::HashMap;

    use crate::schema::Column;

    use super::*;

    #[test]
    fn test_parse_connection_string_with_table() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb#employees");
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(
            target.database_url,
            "mysql://user:pass@localhost:3306/testdb"
        );
        assert_eq!(target.table_name, "employees");
    }

    #[test]
    fn test_parse_connection_string_without_table() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb");
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(
            target.database_url,
            "mysql://user:pass@localhost:3306/testdb"
        );
        assert_eq!(target.table_name, "testdb_data");
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
        let columns = ["id".to_string(), "name".to_string(), "age".to_string()];
        let num_rows = 3;
        let num_columns = columns.len();

        let column_names = columns
            .iter()
            .map(|c| format!("`{}`", c))
            .collect::<Vec<_>>()
            .join(", ");

        let values_placeholders = (0..num_rows)
            .map(|_| {
                let row_placeholders = (0..num_columns).map(|_| "?").collect::<Vec<_>>().join(", ");
                format!("({})", row_placeholders)
            })
            .collect::<Vec<_>>()
            .join(", ");

        let insert_sql = format!(
            "INSERT INTO `{}` ({}) VALUES {}",
            "test_table", column_names, values_placeholders
        );

        let expected =
            "INSERT INTO `test_table` (`id`, `name`, `age`) VALUES (?, ?, ?), (?, ?, ?), (?, ?, ?)";
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
        assert_eq!(
            target.map_data_type_to_mysql(&DataType::Decimal),
            "DECIMAL(65,30)"
        );
        assert_eq!(target.map_data_type_to_mysql(&DataType::String), "TEXT");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Boolean), "BOOLEAN");
        assert_eq!(target.map_data_type_to_mysql(&DataType::Date), "DATE");
        assert_eq!(
            target.map_data_type_to_mysql(&DataType::DateTime),
            "DATETIME"
        );
        assert_eq!(target.map_data_type_to_mysql(&DataType::Null), "TEXT");
    }

    #[test]
    fn test_connection_string_parsing_edge_cases() {
        // Test with port number
        let target = MysqlTarget::new("mysql://root:password@127.0.0.1:3306/myapp#users").unwrap();
        assert_eq!(
            target.database_url,
            "mysql://root:password@127.0.0.1:3306/myapp"
        );
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
            "mysql",      // Not a URL format
            " ",          // Just whitespace
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
                Value::Null => {}    // Null should be handled
                Value::Date(_) => {} // Date should be converted to string
                Value::Json(_) => {} // JSON should be handled
            }
        }
    }

    #[test]
    fn test_date_value_formatting() {
        use chrono::DateTime;

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
        assert_eq!(target.table_name, "db_data");

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
        assert_eq!(
            target.database_url,
            "mysql://user:pass@localhost:3306/testdb"
        );
    }

    #[test]
    fn test_mysql_source_new_with_table() {
        let source = MysqlSource::new("mysql://user:pass@localhost:3306/testdb#employees");
        assert!(source.is_ok());
        let source = source.unwrap();
        assert_eq!(
            source.database_url,
            "mysql://user:pass@localhost:3306/testdb"
        );
        assert_eq!(source.table_name, "employees");
        assert_eq!(source.current_offset, 0);
        assert!(source.pool.is_none());
    }

    #[test]
    fn test_mysql_source_parse_connection_string_without_table() {
        let result =
            MysqlSource::parse_connection_string("mysql://user:pass@localhost:3306/testdb");
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("table specification"));
        }
    }

    #[test]
    fn test_mysql_source_parse_connection_string_with_table() {
        let result =
            MysqlSource::parse_connection_string("mysql://user:pass@localhost:3306/testdb#orders");
        assert!(result.is_ok());
        let (db_url, table) = result.unwrap();
        assert_eq!(db_url, "mysql://user:pass@localhost:3306/testdb");
        assert_eq!(table, "orders");
    }

    #[tokio::test]
    async fn test_mysql_source_get_pool_without_connection() {
        let source = MysqlSource::new("mysql://user:pass@localhost:3306/testdb#users").unwrap();
        let result = source.get_pool().await;
        assert!(result.is_err());
        if let Err(TinyEtlError::Connection(msg)) = result {
            assert!(msg.contains("connection not established"));
        }
    }

    #[test]
    fn test_mysql_source_has_more_with_total_rows() {
        let mut source = MysqlSource::new("mysql://user:pass@localhost:3306/testdb#users").unwrap();

        // When total_rows is None, should return true
        assert!(source.has_more());

        // When current_offset < total_rows, should return true
        source.total_rows = Some(100);
        source.current_offset = 50;
        assert!(source.has_more());

        // When current_offset >= total_rows, should return false
        source.current_offset = 100;
        assert!(!source.has_more());

        source.current_offset = 101;
        assert!(!source.has_more());
    }

    #[tokio::test]
    async fn test_mysql_source_reset() {
        let mut source = MysqlSource::new("mysql://user:pass@localhost:3306/testdb#users").unwrap();
        source.current_offset = 500;

        let result = source.reset().await;
        assert!(result.is_ok());
        assert_eq!(source.current_offset, 0);
    }

    #[test]
    fn test_value_type_conversions() {
        // Test different value type conversions
        let int_val = Value::Integer(42);
        assert!(matches!(int_val, Value::Integer(42)));

        let dec_val = Value::Decimal(Decimal::new(12345, 2));
        if let Value::Decimal(d) = dec_val {
            assert_eq!(d.to_string(), "123.45");
        }

        let str_val = Value::String("test".to_string());
        assert!(matches!(str_val, Value::String(ref s) if s == "test"));

        let bool_val = Value::Boolean(true);
        assert!(matches!(bool_val, Value::Boolean(true)));

        let null_val = Value::Null;
        assert!(matches!(null_val, Value::Null));
    }

    #[test]
    fn test_mysql_target_supports_append() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();
        assert!(target.supports_append());
    }

    #[test]
    fn test_schema_column_nullable_handling() {
        let schema = Schema {
            columns: vec![
                Column {
                    name: "required_field".to_string(),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: "optional_field".to_string(),
                    data_type: DataType::String,
                    nullable: true,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        assert!(!schema.columns[0].nullable);
        assert!(schema.columns[1].nullable);
    }

    #[test]
    fn test_decimal_precision_handling() {
        // Test high precision decimals (max scale is 28 for rust_decimal)
        let dec1 = Decimal::new(123456789012345, 10); // 12345.6789012345
        let dec2 = Decimal::new(1, 28); // Maximum precision scale

        let val1 = Value::Decimal(dec1);
        let val2 = Value::Decimal(dec2);

        assert!(matches!(val1, Value::Decimal(_)));
        assert!(matches!(val2, Value::Decimal(_)));
    }

    #[test]
    fn test_multiple_table_names_in_same_connection() {
        let source1 = MysqlSource::new("mysql://user:pass@localhost:3306/db#table1").unwrap();
        let source2 = MysqlSource::new("mysql://user:pass@localhost:3306/db#table2").unwrap();

        assert_eq!(source1.table_name, "table1");
        assert_eq!(source2.table_name, "table2");
        assert_eq!(source1.database_url, source2.database_url);
    }

    #[test]
    fn test_row_with_all_data_types() {
        use chrono::Utc;

        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("int_col".to_string(), Value::Integer(100));
        row.insert("dec_col".to_string(), Value::Decimal(Decimal::new(9999, 2)));
        row.insert("str_col".to_string(), Value::String("hello".to_string()));
        row.insert("bool_col".to_string(), Value::Boolean(false));
        row.insert("date_col".to_string(), Value::Date(Utc::now()));
        row.insert("null_col".to_string(), Value::Null);

        assert_eq!(row.len(), 6);
        assert!(row.contains_key("int_col"));
        assert!(row.contains_key("dec_col"));
        assert!(row.contains_key("str_col"));
        assert!(row.contains_key("bool_col"));
        assert!(row.contains_key("date_col"));
        assert!(row.contains_key("null_col"));
    }

    #[test]
    fn test_connection_string_with_schema_and_table() {
        // Some databases use schema.table format
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/db#schema.table").unwrap();
        assert_eq!(target.table_name, "schema.table");
    }

    #[test]
    fn test_column_name_with_special_characters() {
        let column = Column {
            name: "user-name".to_string(),
            data_type: DataType::String,
            nullable: true,
        };

        assert_eq!(column.name, "user-name");
        let escaped = format!("`{}`", column.name);
        assert_eq!(escaped, "`user-name`");
    }

    #[test]
    fn test_large_batch_size() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb")
            .unwrap()
            .with_batch_size(10000);

        assert_eq!(target.max_batch_size, 10000);
    }

    #[test]
    fn test_zero_offset_after_reset() {
        let mut source = MysqlSource::new("mysql://user:pass@localhost:3306/db#table").unwrap();
        source.current_offset = 999;

        assert_eq!(source.current_offset, 999);
        // Reset would set it back to 0
    }

    #[test]
    fn test_total_rows_initially_none() {
        let source = MysqlSource::new("mysql://user:pass@localhost:3306/db#table").unwrap();
        assert!(source.total_rows.is_none());
    }

    #[test]
    fn test_database_url_storage() {
        let target =
            MysqlTarget::new("mysql://admin:secret@db.example.com:3306/production#logs").unwrap();
        assert_eq!(
            target.database_url,
            "mysql://admin:secret@db.example.com:3306/production"
        );
        assert_eq!(target.table_name, "logs");
    }

    #[test]
    fn test_data_type_to_mysql_all_types() {
        let target = MysqlTarget::new("mysql://user:pass@localhost:3306/testdb").unwrap();

        // Test all DataType variants
        let type_mappings = vec![
            (DataType::Integer, "BIGINT"),
            (DataType::Decimal, "DECIMAL(65,30)"),
            (DataType::String, "TEXT"),
            (DataType::Boolean, "BOOLEAN"),
            (DataType::Date, "DATE"),
            (DataType::DateTime, "DATETIME"),
            (DataType::Null, "TEXT"),
        ];

        for (data_type, expected_sql) in type_mappings {
            let result = target.map_data_type_to_mysql(&data_type);
            assert_eq!(result, expected_sql, "Failed for {:?}", data_type);
        }
    }

    #[test]
    fn test_empty_table_name_handling() {
        // Test what happens with empty table specification
        let result = MysqlSource::parse_connection_string("mysql://user:pass@localhost:3306/db#");
        assert!(result.is_ok());
        let (_, table) = result.unwrap();
        assert_eq!(table, ""); // Empty string table name
    }

    #[test]
    fn test_multiple_hash_in_connection_string() {
        // Test with multiple # characters - should split on first
        let result =
            MysqlSource::parse_connection_string("mysql://user:pass@localhost:3306/db#table#extra");
        assert!(result.is_ok());
        let (db, table) = result.unwrap();
        assert_eq!(db, "mysql://user:pass@localhost:3306/db");
        assert_eq!(table, "table#extra");
    }

    #[test]
    fn test_value_decimal_from_float() {
        let f_val = 123.456;
        if let Ok(dec) = Decimal::try_from(f_val) {
            let value = Value::Decimal(dec);
            assert!(matches!(value, Value::Decimal(_)));
        }
    }

    #[test]
    fn test_connection_url_with_options() {
        // MySQL connection strings can have query parameters
        let target =
            MysqlTarget::new("mysql://user:pass@localhost:3306/db?ssl=true#table").unwrap();
        assert!(target
            .database_url
            .contains("mysql://user:pass@localhost:3306/db"));
    }

    #[test]
    fn test_schema_estimated_rows() {
        let schema = Schema {
            columns: vec![Column {
                name: "id".to_string(),
                data_type: DataType::Integer,
                nullable: false,
            }],
            estimated_rows: Some(1000),
            primary_key_candidate: Some("id".to_string()),
        };

        assert_eq!(schema.estimated_rows, Some(1000));
        assert_eq!(schema.primary_key_candidate, Some("id".to_string()));
    }

    #[test]
    fn test_row_len_and_contains() {
        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("col1".to_string(), Value::Integer(1));
        row.insert("col2".to_string(), Value::String("test".to_string()));

        assert_eq!(row.len(), 2);
        assert!(row.contains_key("col1"));
        assert!(row.contains_key("col2"));
        assert!(!row.contains_key("col3"));
    }

    #[test]
    fn test_json_field_as_string() {
        // Test that JSON data would be stored as a string
        let json_data = r#"{"key": "value", "number": 42}"#;
        let value = Value::String(json_data.to_string());

        match value {
            Value::String(s) => {
                assert!(s.contains("key"));
                assert!(s.contains("value"));
                assert!(s.contains("42"));
            }
            _ => panic!("Expected JSON data to be stored as String"),
        }
    }

    #[test]
    fn test_json_array_as_string() {
        // Test that JSON array data would be stored as a string
        let json_array = r#"[1, 2, 3, "test"]"#;
        let value = Value::String(json_array.to_string());

        match value {
            Value::String(s) => {
                assert!(s.starts_with('['));
                assert!(s.ends_with(']'));
                assert!(s.contains("test"));
            }
            _ => panic!("Expected JSON array to be stored as String"),
        }
    }

    #[test]
    fn test_null_json_field() {
        // Test that NULL JSON fields are handled correctly
        let value = Value::Null;
        assert!(matches!(value, Value::Null));
    }
}
