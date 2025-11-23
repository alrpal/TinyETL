use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures_util::stream::TryStreamExt;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::time::Duration;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use url::Url;
use uuid::Uuid;

use crate::{
    connectors::{Source, Target},
    schema::{Column, DataType, Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

type MssqlClient = Client<tokio_util::compat::Compat<TcpStream>>;

pub struct MssqlSource {
    connection_string: String,
    client: Option<MssqlClient>,
    table_name: String,
    query: Option<String>,
    current_offset: usize,
    total_rows: Option<usize>,
    schema: Option<Schema>,
}

impl MssqlSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (_, table) = Self::parse_connection_string(connection_string)?;

        Ok(Self {
            connection_string: connection_string.to_string(),
            client: None,
            table_name: table,
            query: None,
            current_offset: 0,
            total_rows: None,
            schema: None,
        })
    }

    pub fn with_query(mut self, query: String) -> Self {
        self.query = Some(query);
        self
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        if let Some((db_part, table_part)) = connection_string.split_once('#') {
            Ok((db_part.to_string(), table_part.to_string()))
        } else {
            Err(TinyEtlError::Configuration(
                "MSSQL connection string must include table name: connection_string#table_name"
                    .to_string(),
            ))
        }
    }

    async fn create_client(connection_string: &str) -> Result<MssqlClient> {
        // Support both natural format (localhost\INSTANCE) and URL-encoded format (localhost%5CINSTANCE)
        // Replace backslashes with URL encoding before parsing
        let sanitized_url = connection_string.replace("\\", "%5C");

        let url = Url::parse(&sanitized_url)
            .map_err(|e| TinyEtlError::Configuration(format!("Invalid MSSQL URL: {}", e)))?;

        let mut config = Config::new();

        // Set server address - decode %5C back to backslash for SQL Server named instances
        if let Some(host) = url.host_str() {
            let decoded_host = host.replace("%5C", "\\").replace("%5c", "\\");
            config.host(&decoded_host);
        }

        // Set port
        if let Some(port) = url.port() {
            config.port(port);
        } else {
            config.port(1433); // Default MSSQL port
        }

        // Set database name
        let database = url.path().trim_start_matches('/');
        if !database.is_empty() {
            config.database(database);
        }

        // Set authentication
        if !url.username().is_empty() {
            let username = url.username();
            let password = url.password().unwrap_or("");
            config.authentication(AuthMethod::sql_server(username, password));
        } else {
            // For simplicity, we require username/password authentication
            return Err(TinyEtlError::Configuration(
                "MSSQL connection requires username and password in URL format: mssql://user:pass@host:port/database#table".to_string()
            ));
        }

        // Configure encryption - use NotSupported to avoid TLS issues with self-signed certs
        // This is equivalent to TrustServerCertificate=yes in ODBC
        config.encryption(EncryptionLevel::NotSupported);
        config.trust_cert();

        // Add connection timeout (10 seconds)
        let tcp = timeout(
            Duration::from_secs(10),
            TcpStream::connect(config.get_addr()),
        )
        .await
        .map_err(|_| {
            TinyEtlError::Connection(
                "Connection timeout: Failed to connect to MSSQL server within 10 seconds"
                    .to_string(),
            )
        })?
        .map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to MSSQL server: {}", e))
        })?;

        // Add authentication timeout (10 seconds)
        let client = timeout(
            Duration::from_secs(10),
            Client::connect(config, tcp.compat_write()),
        )
        .await
        .map_err(|_| {
            TinyEtlError::Connection(
                "Authentication timeout: Failed to authenticate with MSSQL within 10 seconds"
                    .to_string(),
            )
        })?
        .map_err(|e| {
            TinyEtlError::Connection(format!("Failed to authenticate with MSSQL: {}", e))
        })?;

        Ok(client)
    }
}

#[async_trait]
impl Source for MssqlSource {
    async fn connect(&mut self) -> Result<()> {
        let (db_part, _) = Self::parse_connection_string(&self.connection_string)?;
        self.client = Some(Self::create_client(&db_part).await?);
        Ok(())
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.client.is_none() {
            self.connect().await?;
        }

        let client = self.client.as_mut().unwrap();

        // Get column information from the table
        let query = format!(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = '{}' ORDER BY ORDINAL_POSITION",
            self.table_name
        );

        let mut stream = client.query(query, &[]).await.map_err(|e| {
            TinyEtlError::SchemaInference(format!("Failed to get table schema: {}", e))
        })?;

        let mut columns = Vec::new();

        while let Some(item) = stream.try_next().await.map_err(|e| {
            TinyEtlError::SchemaInference(format!("Failed to fetch schema results: {}", e))
        })? {
            if let tiberius::QueryItem::Row(row) = item {
                let column_name: &str = row.get(0).ok_or_else(|| {
                    TinyEtlError::SchemaInference("Missing column name".to_string())
                })?;
                let data_type: &str = row.get(1).ok_or_else(|| {
                    TinyEtlError::SchemaInference("Missing data type".to_string())
                })?;
                let is_nullable: &str = row.get(2).ok_or_else(|| {
                    TinyEtlError::SchemaInference("Missing nullable info".to_string())
                })?;

                let data_type = match data_type.to_uppercase().as_str() {
                    "INT" | "SMALLINT" | "TINYINT" | "BIGINT" => DataType::Integer,
                    "FLOAT" | "REAL" | "DECIMAL" | "NUMERIC" | "MONEY" | "SMALLMONEY" => {
                        DataType::Decimal
                    }
                    "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "TEXT" | "NTEXT" => {
                        DataType::String
                    }
                    "BIT" => DataType::Boolean,
                    "DATE" => DataType::Date,
                    "DATETIME" | "DATETIME2" | "SMALLDATETIME" | "TIMESTAMP" => DataType::DateTime,
                    "UNIQUEIDENTIFIER" => DataType::String,
                    _ => DataType::String,
                };

                columns.push(Column {
                    name: column_name.to_string(),
                    data_type,
                    nullable: is_nullable.eq_ignore_ascii_case("YES"),
                });
            }
        }

        if columns.is_empty() {
            return Err(TinyEtlError::SchemaInference(format!(
                "Table '{}' not found or has no columns",
                self.table_name
            )));
        }

        let schema = Schema {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        };

        self.schema = Some(schema.clone());
        Ok(schema)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.client.is_none() {
            return Err(TinyEtlError::Connection(
                "Not connected to MSSQL".to_string(),
            ));
        }

        // Ensure we have schema
        if self.schema.is_none() {
            self.infer_schema(1000).await?;
        }

        let client = self.client.as_mut().unwrap();
        let schema = self.schema.as_ref().unwrap();

        let query = if let Some(ref custom_query) = self.query {
            format!(
                "{} ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                custom_query, self.current_offset, batch_size
            )
        } else {
            format!(
                "SELECT * FROM {} ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                self.table_name, self.current_offset, batch_size
            )
        };

        let mut stream = client
            .query(query, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to execute query: {}", e)))?;

        let mut rows = Vec::new();

        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to fetch results: {}", e)))?
        {
            if let tiberius::QueryItem::Row(row) = item {
                let mut row_data = HashMap::new();
                for (i, column) in schema.columns.iter().enumerate() {
                    let value = match row.try_get::<&str, usize>(i) {
                        Ok(Some(s)) => Value::String(s.to_string()),
                        Ok(None) => Value::Null,
                        Err(_) => {
                            // Try different types
                            if let Ok(Some(v)) = row.try_get::<i64, usize>(i) {
                                Value::Integer(v)
                            } else if let Ok(Some(v)) = row.try_get::<f64, usize>(i) {
                                Value::Decimal(Decimal::from_f64_retain(v).unwrap_or_default())
                            } else if let Ok(Some(v)) = row.try_get::<bool, usize>(i) {
                                Value::Boolean(v)
                            } else if let Ok(Some(v)) = row.try_get::<NaiveDateTime, usize>(i) {
                                // Convert NaiveDateTime to DateTime<Utc>
                                Value::Date(DateTime::from_utc(v, Utc))
                            } else {
                                Value::Null
                            }
                        }
                    };
                    row_data.insert(column.name.clone(), value);
                }
                rows.push(row_data);
            }
        }
        self.current_offset += rows.len();
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        if self.total_rows.is_some() {
            return Ok(self.total_rows);
        }

        Ok(None) // For simplicity, we don't implement this optimization
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_offset = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        // This is a simple heuristic - in a real implementation, you might want to
        // track this more precisely
        true
    }
}

pub struct MssqlTarget {
    connection_string: String,
    table_name: String,
    client: Option<MssqlClient>,
    max_batch_size: usize,
    schema: Option<Schema>,
}

impl MssqlTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (_, table_name) = Self::parse_connection_string(connection_string)?;

        Ok(Self {
            connection_string: connection_string.to_string(),
            table_name,
            client: None,
            max_batch_size: 1000,
            schema: None,
        })
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.max_batch_size = batch_size.max(1);
        self
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        if let Some((db_part, table_part)) = connection_string.split_once('#') {
            Ok((db_part.to_string(), table_part.to_string()))
        } else {
            Err(TinyEtlError::Configuration(
                "MSSQL connection string must include table name: connection_string#table_name"
                    .to_string(),
            ))
        }
    }

    async fn create_client(connection_string: &str) -> Result<MssqlClient> {
        MssqlSource::create_client(connection_string).await
    }

    fn sql_type_from_data_type(data_type: &DataType) -> &'static str {
        match data_type {
            DataType::Integer => "BIGINT",
            DataType::Decimal => "DECIMAL(18,6)",
            DataType::String => "NVARCHAR(MAX)",
            DataType::Boolean => "BIT",
            DataType::Date => "DATE",
            DataType::DateTime => "DATETIME2",
            DataType::Json => "NVARCHAR(MAX)", // MSSQL stores JSON as NVARCHAR
            DataType::Null => "NVARCHAR(MAX)", // Default to string for null type
        }
    }

    fn format_value_for_insert(value: &Value, expected_type: &DataType) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::String(s) => {
                // Try to coerce string to expected type
                match expected_type {
                    DataType::Integer => {
                        if let Ok(i) = s.parse::<i64>() {
                            i.to_string()
                        } else {
                            "NULL".to_string()
                        }
                    }
                    DataType::Decimal => {
                        if let Ok(d) = s.parse::<f64>() {
                            d.to_string()
                        } else {
                            "NULL".to_string()
                        }
                    }
                    DataType::Boolean => match s.to_lowercase().as_str() {
                        "true" | "1" | "yes" => "1".to_string(),
                        "false" | "0" | "no" => "0".to_string(),
                        _ => "NULL".to_string(),
                    },
                    _ => format!("N'{}'", s.replace("'", "''")),
                }
            }
            Value::Integer(i) => match expected_type {
                DataType::String => format!("N'{}'", i),
                _ => i.to_string(),
            },
            Value::Decimal(d) => match expected_type {
                DataType::String => {
                    if let Some(f) = d.to_f64() {
                        format!("N'{}'", f)
                    } else {
                        "NULL".to_string()
                    }
                }
                _ => {
                    if let Some(f) = d.to_f64() {
                        f.to_string()
                    } else {
                        "NULL".to_string()
                    }
                }
            },
            Value::Boolean(b) => match expected_type {
                DataType::String => format!("N'{}'", if *b { "true" } else { "false" }),
                _ => if *b { "1" } else { "0" }.to_string(),
            },
            Value::Date(dt) => format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S%.3f")),
            Value::Json(j) => {
                let json_str = serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string());
                format!("N'{}'", json_str.replace("'", "''"))
            }
        }
    }

    // Write value directly to a string buffer for better performance
    fn write_value_to_buffer(buffer: &mut String, value: &Value, expected_type: &DataType) {
        match value {
            Value::Null => buffer.push_str("NULL"),
            Value::String(s) => {
                match expected_type {
                    DataType::Integer => {
                        if let Ok(i) = s.parse::<i64>() {
                            buffer.push_str(&i.to_string());
                        } else {
                            buffer.push_str("NULL");
                        }
                    }
                    DataType::Decimal => {
                        if let Ok(d) = s.parse::<f64>() {
                            buffer.push_str(&d.to_string());
                        } else {
                            buffer.push_str("NULL");
                        }
                    }
                    DataType::Boolean => {
                        buffer.push_str(match s.to_lowercase().as_str() {
                            "true" | "1" | "yes" => "1",
                            "false" | "0" | "no" => "0",
                            _ => "NULL",
                        });
                    }
                    _ => {
                        buffer.push_str("N'");
                        // Escape single quotes
                        for ch in s.chars() {
                            if ch == '\'' {
                                buffer.push_str("''");
                            } else {
                                buffer.push(ch);
                            }
                        }
                        buffer.push('\'');
                    }
                }
            }
            Value::Integer(i) => {
                if matches!(expected_type, DataType::String) {
                    buffer.push_str("N'");
                    buffer.push_str(&i.to_string());
                    buffer.push('\'');
                } else {
                    buffer.push_str(&i.to_string());
                }
            }
            Value::Decimal(d) => {
                if let Some(f) = d.to_f64() {
                    if matches!(expected_type, DataType::String) {
                        buffer.push_str("N'");
                        buffer.push_str(&f.to_string());
                        buffer.push('\'');
                    } else {
                        buffer.push_str(&f.to_string());
                    }
                } else {
                    buffer.push_str("NULL");
                }
            }
            Value::Boolean(b) => {
                if matches!(expected_type, DataType::String) {
                    buffer.push_str(if *b { "N'true'" } else { "N'false'" });
                } else {
                    buffer.push(if *b { '1' } else { '0' });
                }
            }
            Value::Date(dt) => {
                buffer.push('\'');
                buffer.push_str(&dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
                buffer.push('\'');
            }
            Value::Json(j) => {
                buffer.push_str("N'");
                let json_str = serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string());
                for ch in json_str.chars() {
                    if ch == '\'' {
                        buffer.push_str("''");
                    } else {
                        buffer.push(ch);
                    }
                }
                buffer.push('\'');
            }
        }
    }
}

#[async_trait]
impl Target for MssqlTarget {
    async fn connect(&mut self) -> Result<()> {
        let (db_part, _) = Self::parse_connection_string(&self.connection_string)?;
        self.client = Some(Self::create_client(&db_part).await?);
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.client.is_none() {
            self.connect().await?;
        }

        let client = self.client.as_mut().unwrap();
        self.schema = Some(schema.clone());

        // Build CREATE TABLE statement with IF NOT EXISTS logic
        let mut columns_sql = Vec::new();
        for column in &schema.columns {
            let sql_type = Self::sql_type_from_data_type(&column.data_type);
            let nullable = if column.nullable { "NULL" } else { "NOT NULL" };
            columns_sql.push(format!("[{}] {} {}", column.name, sql_type, nullable));
        }

        // SQL Server doesn't have CREATE TABLE IF NOT EXISTS, so we use IF NOT EXISTS wrapper
        let create_table_sql = format!(
            "IF NOT EXISTS (SELECT * FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = '{}') \
             BEGIN CREATE TABLE [{}] ({}) END",
            table_name.replace("'", "''"), // Escape single quotes
            table_name,
            columns_sql.join(", ")
        );

        client
            .execute(&create_table_sql, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create table: {}", e)))?;

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.client.is_none() {
            return Err(TinyEtlError::Connection(
                "Not connected to MSSQL".to_string(),
            ));
        }

        if rows.is_empty() {
            return Ok(0);
        }

        let client = self.client.as_mut().unwrap();
        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::DataTransfer("Schema not set".to_string()))?;

        // Build column names
        let column_names: Vec<String> = schema
            .columns
            .iter()
            .map(|c| format!("[{}]", c.name))
            .collect();

        let mut total_written = 0;

        // Process rows in larger chunks for better performance
        // SQL Server can handle 1000 rows per INSERT statement efficiently
        let chunk_size = self.max_batch_size.min(1000);

        // Process without explicit transaction management - let tiberius handle it
        // (tiberius uses autocommit by default which is fine for bulk inserts)
        let result = async {
            for chunk in rows.chunks(chunk_size) {
                // Build the INSERT statement directly to avoid intermediate allocations
                // Estimate size: table name + columns + (~100 bytes per row avg)
                let estimated_size = 200 + column_names.len() * 25 + chunk.len() * 150;
                let mut insert_sql = String::with_capacity(estimated_size);

                insert_sql.push_str("INSERT INTO [");
                insert_sql.push_str(&self.table_name);
                insert_sql.push_str("] (");
                insert_sql.push_str(&column_names.join(", "));
                insert_sql.push_str(") VALUES ");

                for (row_idx, row) in chunk.iter().enumerate() {
                    if row.len() != schema.columns.len() {
                        return Err(TinyEtlError::DataTransfer(format!(
                            "Row has {} values but schema expects {}",
                            row.len(),
                            schema.columns.len()
                        )));
                    }

                    if row_idx > 0 {
                        insert_sql.push_str(", ");
                    }
                    insert_sql.push('(');

                    // Write VALUES clause directly into the string buffer for maximum performance
                    for (col_idx, col) in schema.columns.iter().enumerate() {
                        if col_idx > 0 {
                            insert_sql.push_str(", ");
                        }

                        let value = row.get(&col.name).unwrap_or(&Value::Null);
                        Self::write_value_to_buffer(&mut insert_sql, value, &col.data_type);
                    }

                    insert_sql.push(')');
                }

                client.execute(&insert_sql, &[]).await.map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to insert batch: {}", e))
                })?;

                total_written += chunk.len();
            }
            Ok::<usize, TinyEtlError>(total_written)
        }
        .await;

        // Return result
        result
    }

    async fn finalize(&mut self) -> Result<()> {
        // MSSQL doesn't require explicit finalization for basic operations
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        if self.client.is_none() {
            return Err(TinyEtlError::Connection(
                "Not connected to MSSQL".to_string(),
            ));
        }

        // We need to create a temporary connection since the method is not mutable
        let (db_part, _) = Self::parse_connection_string(&self.connection_string)?;
        let mut client = Self::create_client(&db_part).await?;

        let query = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = @P1";
        let mut stream = client.query(query, &[&table_name]).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to check table existence: {}", e))
        })?;

        if let Some(item) = stream.try_next().await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to fetch existence result: {}", e))
        })? {
            if let tiberius::QueryItem::Row(row) = item {
                if let Ok(Some(count)) = row.try_get::<i32, usize>(0) {
                    return Ok(count > 0);
                }
            }
        }

        Ok(false)
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        if self.client.is_none() {
            self.connect().await?;
        }

        let client = self.client.as_mut().unwrap();
        let truncate_sql = format!("TRUNCATE TABLE [{}]", table_name);

        client
            .execute(&truncate_sql, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e)))?;

        Ok(())
    }

    fn supports_append(&self) -> bool {
        true // MSSQL supports INSERT operations on existing tables
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType, Schema, Value};
    use std::collections::HashMap;

    #[test]
    fn test_parse_connection_string_valid() {
        let result = MssqlSource::parse_connection_string(
            "mssql://user:pass@localhost:1433/testdb#testtable",
        );
        assert!(result.is_ok());
        let (db_part, table_name) = result.unwrap();
        assert_eq!(db_part, "mssql://user:pass@localhost:1433/testdb");
        assert_eq!(table_name, "testtable");
    }

    #[test]
    fn test_parse_connection_string_missing_table() {
        let result =
            MssqlSource::parse_connection_string("mssql://user:pass@localhost:1433/testdb");
        assert!(result.is_err());
        match result {
            Err(TinyEtlError::Configuration(msg)) => {
                assert!(msg.contains("must include table name"));
            }
            _ => panic!("Expected Configuration error"),
        }
    }

    #[test]
    fn test_parse_connection_string_with_complex_table() {
        let result = MssqlSource::parse_connection_string(
            "mssql://user:pass@localhost:1433/testdb#schema.table",
        );
        assert!(result.is_ok());
        let (_, table_name) = result.unwrap();
        assert_eq!(table_name, "schema.table");
    }

    #[test]
    fn test_mssql_source_new_valid() {
        let source = MssqlSource::new("mssql://user:pass@localhost:1433/testdb#testtable");
        assert!(source.is_ok());
        let source = source.unwrap();
        assert_eq!(source.table_name, "testtable");
        assert_eq!(source.current_offset, 0);
        assert!(source.client.is_none());
        assert!(source.schema.is_none());
    }

    #[test]
    fn test_mssql_source_new_invalid() {
        let source = MssqlSource::new("mssql://user:pass@localhost:1433/testdb");
        assert!(source.is_err());
    }

    #[test]
    fn test_mssql_source_with_query() {
        let source = MssqlSource::new("mssql://user:pass@localhost:1433/testdb#testtable")
            .unwrap()
            .with_query("SELECT * FROM testtable WHERE id > 100".to_string());

        assert_eq!(
            source.query,
            Some("SELECT * FROM testtable WHERE id > 100".to_string())
        );
    }

    #[test]
    fn test_mssql_target_new_valid() {
        let target = MssqlTarget::new("mssql://user:pass@localhost:1433/testdb#testtable");
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(target.table_name, "testtable");
        assert!(target.client.is_none());
        assert_eq!(target.max_batch_size, 1000);
    }

    #[test]
    fn test_mssql_target_new_invalid() {
        let target = MssqlTarget::new("mssql://user:pass@localhost:1433/testdb");
        assert!(target.is_err());
    }

    #[test]
    fn test_mssql_target_with_batch_size() {
        let target = MssqlTarget::new("mssql://user:pass@localhost:1433/testdb#testtable")
            .unwrap()
            .with_batch_size(500);

        assert_eq!(target.max_batch_size, 500);
    }

    #[test]
    fn test_mssql_target_with_batch_size_minimum() {
        let target = MssqlTarget::new("mssql://user:pass@localhost:1433/testdb#testtable")
            .unwrap()
            .with_batch_size(0); // Should be clamped to 1

        assert_eq!(target.max_batch_size, 1);
    }

    #[test]
    fn test_sql_type_from_data_type_integer() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::Integer),
            "BIGINT"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_decimal() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::Decimal),
            "DECIMAL(18,6)"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_string() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::String),
            "NVARCHAR(MAX)"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_boolean() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::Boolean),
            "BIT"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_date() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::Date),
            "DATE"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_datetime() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::DateTime),
            "DATETIME2"
        );
    }

    #[test]
    fn test_sql_type_from_data_type_null() {
        assert_eq!(
            MssqlTarget::sql_type_from_data_type(&DataType::Null),
            "NVARCHAR(MAX)"
        );
    }

    #[test]
    fn test_format_value_for_insert_null() {
        let result = MssqlTarget::format_value_for_insert(&Value::Null, &DataType::String);
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_format_value_for_insert_string_to_string() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("hello".to_string()),
            &DataType::String,
        );
        assert_eq!(result, "N'hello'");
    }

    #[test]
    fn test_format_value_for_insert_string_with_quotes() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("O'Brien".to_string()),
            &DataType::String,
        );
        assert_eq!(result, "N'O''Brien'");
    }

    #[test]
    fn test_format_value_for_insert_string_to_integer() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("42".to_string()),
            &DataType::Integer,
        );
        assert_eq!(result, "42");
    }

    #[test]
    fn test_format_value_for_insert_string_to_integer_invalid() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("not_a_number".to_string()),
            &DataType::Integer,
        );
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_format_value_for_insert_string_to_decimal() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("3.14".to_string()),
            &DataType::Decimal,
        );
        assert_eq!(result, "3.14");
    }

    #[test]
    fn test_format_value_for_insert_string_to_boolean_true() {
        let tests = vec!["true", "TRUE", "1", "yes", "YES"];
        for s in tests {
            let result = MssqlTarget::format_value_for_insert(
                &Value::String(s.to_string()),
                &DataType::Boolean,
            );
            assert_eq!(result, "1", "Failed for input: {}", s);
        }
    }

    #[test]
    fn test_format_value_for_insert_string_to_boolean_false() {
        let tests = vec!["false", "FALSE", "0", "no", "NO"];
        for s in tests {
            let result = MssqlTarget::format_value_for_insert(
                &Value::String(s.to_string()),
                &DataType::Boolean,
            );
            assert_eq!(result, "0", "Failed for input: {}", s);
        }
    }

    #[test]
    fn test_format_value_for_insert_integer() {
        let result = MssqlTarget::format_value_for_insert(&Value::Integer(42), &DataType::Integer);
        assert_eq!(result, "42");
    }

    #[test]
    fn test_format_value_for_insert_integer_to_string() {
        let result = MssqlTarget::format_value_for_insert(&Value::Integer(42), &DataType::String);
        assert_eq!(result, "N'42'");
    }

    #[test]
    fn test_format_value_for_insert_boolean_true() {
        let result =
            MssqlTarget::format_value_for_insert(&Value::Boolean(true), &DataType::Boolean);
        assert_eq!(result, "1");
    }

    #[test]
    fn test_format_value_for_insert_boolean_false() {
        let result =
            MssqlTarget::format_value_for_insert(&Value::Boolean(false), &DataType::Boolean);
        assert_eq!(result, "0");
    }

    #[test]
    fn test_format_value_for_insert_boolean_to_string() {
        let result_true =
            MssqlTarget::format_value_for_insert(&Value::Boolean(true), &DataType::String);
        assert_eq!(result_true, "N'true'");

        let result_false =
            MssqlTarget::format_value_for_insert(&Value::Boolean(false), &DataType::String);
        assert_eq!(result_false, "N'false'");
    }

    #[test]
    fn test_format_value_for_insert_decimal() {
        use rust_decimal::Decimal;
        let dec = Decimal::new(12345, 2); // 123.45
        let result = MssqlTarget::format_value_for_insert(&Value::Decimal(dec), &DataType::Decimal);
        assert_eq!(result, "123.45");
    }

    #[test]
    fn test_format_value_for_insert_date() {
        use chrono::{DateTime, Utc};
        let dt = DateTime::parse_from_rfc3339("2024-03-15T14:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let result = MssqlTarget::format_value_for_insert(&Value::Date(dt), &DataType::DateTime);
        assert!(result.starts_with("'2024-03-15"));
        assert!(result.ends_with("'"));
    }

    #[test]
    fn test_supports_append() {
        let target = MssqlTarget::new("mssql://user:pass@localhost:1433/testdb#testtable").unwrap();
        assert!(target.supports_append());
    }

    #[test]
    fn test_parse_target_connection_string() {
        let result = MssqlTarget::parse_connection_string(
            "mssql://user:pass@localhost:1433/testdb#testtable",
        );
        assert!(result.is_ok());
        let (db_part, table_name) = result.unwrap();
        assert_eq!(db_part, "mssql://user:pass@localhost:1433/testdb");
        assert_eq!(table_name, "testtable");
    }

    #[test]
    fn test_parse_target_connection_string_error() {
        let result = MssqlTarget::parse_connection_string("mssql://user:pass@localhost/db");
        assert!(result.is_err());
    }

    #[test]
    fn test_row_with_various_types() {
        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("id".to_string(), Value::Integer(1));
        row.insert("name".to_string(), Value::String("Test".to_string()));
        row.insert("active".to_string(), Value::Boolean(true));
        row.insert(
            "score".to_string(),
            Value::Decimal(rust_decimal::Decimal::new(955, 1)),
        );
        row.insert("deleted".to_string(), Value::Null);

        assert_eq!(row.get("id"), Some(&Value::Integer(1)));
        assert_eq!(row.get("name"), Some(&Value::String("Test".to_string())));
        assert_eq!(row.get("active"), Some(&Value::Boolean(true)));
        assert_eq!(row.get("deleted"), Some(&Value::Null));
    }

    #[test]
    fn test_write_value_to_buffer_null() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Null, &DataType::String);
        assert_eq!(buffer, "NULL");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_string() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("test".to_string()),
            &DataType::String,
        );
        assert_eq!(buffer, "N'test'");
    }

    #[test]
    fn test_write_value_to_buffer_string_with_quotes() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("O'Brien's".to_string()),
            &DataType::String,
        );
        assert_eq!(buffer, "N'O''Brien''s'");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_integer_valid() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("123".to_string()),
            &DataType::Integer,
        );
        assert_eq!(buffer, "123");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_integer_invalid() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("not_a_number".to_string()),
            &DataType::Integer,
        );
        assert_eq!(buffer, "NULL");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_decimal_valid() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("3.14159".to_string()),
            &DataType::Decimal,
        );
        assert_eq!(buffer, "3.14159");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_decimal_invalid() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("not_a_decimal".to_string()),
            &DataType::Decimal,
        );
        assert_eq!(buffer, "NULL");
    }

    #[test]
    fn test_write_value_to_buffer_string_to_boolean_true_values() {
        let true_values = vec!["true", "1", "yes"];
        for val in true_values {
            let mut buffer = String::new();
            MssqlTarget::write_value_to_buffer(
                &mut buffer,
                &Value::String(val.to_string()),
                &DataType::Boolean,
            );
            assert_eq!(buffer, "1", "Failed for: {}", val);
        }
    }

    #[test]
    fn test_write_value_to_buffer_string_to_boolean_false_values() {
        let false_values = vec!["false", "0", "no"];
        for val in false_values {
            let mut buffer = String::new();
            MssqlTarget::write_value_to_buffer(
                &mut buffer,
                &Value::String(val.to_string()),
                &DataType::Boolean,
            );
            assert_eq!(buffer, "0", "Failed for: {}", val);
        }
    }

    #[test]
    fn test_write_value_to_buffer_string_to_boolean_invalid() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("maybe".to_string()),
            &DataType::Boolean,
        );
        assert_eq!(buffer, "NULL");
    }

    #[test]
    fn test_write_value_to_buffer_integer_to_integer() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Integer(42), &DataType::Integer);
        assert_eq!(buffer, "42");
    }

    #[test]
    fn test_write_value_to_buffer_integer_to_string() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Integer(123), &DataType::String);
        assert_eq!(buffer, "N'123'");
    }

    #[test]
    fn test_write_value_to_buffer_decimal_to_decimal() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::Decimal(rust_decimal::Decimal::new(314159, 5)),
            &DataType::Decimal,
        );
        assert_eq!(buffer, "3.14159");
    }

    #[test]
    fn test_write_value_to_buffer_decimal_to_string() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::Decimal(rust_decimal::Decimal::new(271828, 5)),
            &DataType::String,
        );
        assert_eq!(buffer, "N'2.71828'");
    }

    #[test]
    fn test_write_value_to_buffer_boolean_true_to_boolean() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Boolean(true), &DataType::Boolean);
        assert_eq!(buffer, "1");
    }

    #[test]
    fn test_write_value_to_buffer_boolean_false_to_boolean() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Boolean(false), &DataType::Boolean);
        assert_eq!(buffer, "0");
    }

    #[test]
    fn test_write_value_to_buffer_boolean_to_string_true() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Boolean(true), &DataType::String);
        assert_eq!(buffer, "N'true'");
    }

    #[test]
    fn test_write_value_to_buffer_boolean_to_string_false() {
        let mut buffer = String::new();
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Boolean(false), &DataType::String);
        assert_eq!(buffer, "N'false'");
    }

    #[test]
    fn test_write_value_to_buffer_date() {
        use chrono::{DateTime, Utc};
        let mut buffer = String::new();
        let dt = DateTime::parse_from_rfc3339("2024-03-15T14:30:45.123Z")
            .unwrap()
            .with_timezone(&Utc);
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Date(dt), &DataType::DateTime);
        assert!(buffer.starts_with("'2024-03-15"));
        assert!(buffer.ends_with("'"));
    }

    #[test]
    fn test_format_value_decimal_to_string() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::Decimal(rust_decimal::Decimal::new(12345, 2)),
            &DataType::String,
        );
        assert_eq!(result, "N'123.45'");
    }

    #[test]
    fn test_format_value_string_to_boolean_invalid() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("maybe".to_string()),
            &DataType::Boolean,
        );
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_format_value_string_to_decimal_invalid() {
        let result = MssqlTarget::format_value_for_insert(
            &Value::String("not_a_decimal".to_string()),
            &DataType::Decimal,
        );
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_multiple_buffer_writes() {
        let mut buffer = String::new();

        buffer.push('(');
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Integer(1), &DataType::Integer);
        buffer.push_str(", ");
        MssqlTarget::write_value_to_buffer(
            &mut buffer,
            &Value::String("test".to_string()),
            &DataType::String,
        );
        buffer.push_str(", ");
        MssqlTarget::write_value_to_buffer(&mut buffer, &Value::Boolean(true), &DataType::Boolean);
        buffer.push(')');

        assert_eq!(buffer, "(1, N'test', 1)");
    }

    #[test]
    fn test_parse_source_connection_string() {
        let result = MssqlSource::parse_connection_string(
            "mssql://user:pass@localhost:1433/testdb#testtable",
        );
        assert!(result.is_ok());
        let (db_part, table_name) = result.unwrap();
        assert_eq!(db_part, "mssql://user:pass@localhost:1433/testdb");
        assert_eq!(table_name, "testtable");
    }

    #[test]
    fn test_parse_source_connection_string_missing_table() {
        let result =
            MssqlSource::parse_connection_string("mssql://user:pass@localhost:1433/testdb");
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("must include table name"));
        }
    }

    #[test]
    fn test_source_with_query() {
        let source = MssqlSource::new("mssql://user:pass@localhost/db#table")
            .unwrap()
            .with_query("SELECT * FROM custom_table WHERE active = 1".to_string());

        assert!(source.query.is_some());
        assert_eq!(
            source.query.unwrap(),
            "SELECT * FROM custom_table WHERE active = 1"
        );
    }
}
