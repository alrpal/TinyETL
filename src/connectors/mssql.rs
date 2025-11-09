use std::collections::HashMap;
use async_trait::async_trait;
use tiberius::{Client, Config, AuthMethod, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use futures_util::stream::TryStreamExt;
use url::Url;
use rust_decimal::Decimal;
use chrono::{DateTime, Utc, NaiveDate, NaiveDateTime};
use uuid::Uuid;

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, Column, DataType, SchemaInferer},
    connectors::{Source, Target},
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
                "MSSQL connection string must include table name: connection_string#table_name".to_string()
            ))
        }
    }

    async fn create_client(connection_string: &str) -> Result<MssqlClient> {
        let url = Url::parse(connection_string).map_err(|e| {
            TinyEtlError::Configuration(format!("Invalid MSSQL URL: {}", e))
        })?;

        let mut config = Config::new();
        
        // Set server address
        if let Some(host) = url.host_str() {
            config.host(host);
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

        // Trust server certificate (for development)
        config.trust_cert();

        let tcp = TcpStream::connect(config.get_addr())
            .await
            .map_err(|e| TinyEtlError::Connection(format!("Failed to connect to MSSQL server: {}", e)))?;

        let client = Client::connect(config, tcp.compat_write())
            .await
            .map_err(|e| TinyEtlError::Connection(format!("Failed to authenticate with MSSQL: {}", e)))?;

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

        let mut stream = client.query(query, &[])
            .await
            .map_err(|e| TinyEtlError::SchemaInference(format!("Failed to get table schema: {}", e)))?;

        let mut columns = Vec::new();

        while let Some(item) = stream.try_next().await
            .map_err(|e| TinyEtlError::SchemaInference(format!("Failed to fetch schema results: {}", e)))? {
            
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
                "FLOAT" | "REAL" | "DECIMAL" | "NUMERIC" | "MONEY" | "SMALLMONEY" => DataType::Decimal,
                "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "TEXT" | "NTEXT" => DataType::String,
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
                "Table '{}' not found or has no columns", self.table_name
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
            return Err(TinyEtlError::Connection("Not connected to MSSQL".to_string()));
        }

        // Ensure we have schema
        if self.schema.is_none() {
            self.infer_schema(1000).await?;
        }

        let client = self.client.as_mut().unwrap();
        let schema = self.schema.as_ref().unwrap();
        
        let query = if let Some(ref custom_query) = self.query {
            format!("{} ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", 
                    custom_query, self.current_offset, batch_size)
        } else {
            format!("SELECT * FROM {} ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", 
                    self.table_name, self.current_offset, batch_size)
        };

        let mut stream = client.query(query, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to execute query: {}", e)))?;

        let mut rows = Vec::new();

        while let Some(item) = stream.try_next().await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to fetch results: {}", e)))? {
            
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
        }        self.current_offset += rows.len();
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
                "MSSQL connection string must include table name: connection_string#table_name".to_string()
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
            DataType::Null => "NVARCHAR(MAX)", // Default to string for null type
        }
    }

    fn format_value_for_sql(value: &Value) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::String(s) => format!("N'{}'", s.replace("'", "''")),
            Value::Integer(i) => i.to_string(),
            Value::Decimal(d) => d.to_string(),
            Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Date(dt) => format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S%.3f")),
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

        // Build CREATE TABLE statement
        let mut columns_sql = Vec::new();
        for column in &schema.columns {
            let sql_type = Self::sql_type_from_data_type(&column.data_type);
            let nullable = if column.nullable { "NULL" } else { "NOT NULL" };
            columns_sql.push(format!("[{}] {} {}", column.name, sql_type, nullable));
        }

        let create_table_sql = format!(
            "CREATE TABLE [{}] ({})",
            table_name,
            columns_sql.join(", ")
        );

        client.execute(&create_table_sql, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create table: {}", e)))?;

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.client.is_none() {
            return Err(TinyEtlError::Connection("Not connected to MSSQL".to_string()));
        }

        if rows.is_empty() {
            return Ok(0);
        }

        let client = self.client.as_mut().unwrap();
        let schema = self.schema.as_ref()
            .ok_or_else(|| TinyEtlError::DataTransfer("Schema not set".to_string()))?;

        // Build column names
        let column_names: Vec<String> = schema.columns.iter()
            .map(|c| format!("[{}]", c.name))
            .collect();

        // Process rows in chunks
        let mut total_written = 0;
        for chunk in rows.chunks(self.max_batch_size) {
            let mut values_sql = Vec::new();
            
            for row in chunk {
                if row.len() != schema.columns.len() {
                    return Err(TinyEtlError::DataTransfer(format!(
                        "Row has {} values but schema expects {}",
                        row.len(),
                        schema.columns.len()
                    )));
                }

                let row_values: Vec<String> = schema.columns.iter()
                    .map(|col| {
                        let value = row.get(&col.name).unwrap_or(&Value::Null);
                        Self::format_value_for_sql(value)
                    })
                    .collect();
                values_sql.push(format!("({})", row_values.join(", ")));
            }

            let insert_sql = format!(
                "INSERT INTO [{}] ({}) VALUES {}",
                self.table_name,
                column_names.join(", "),
                values_sql.join(", ")
            );

            client.execute(&insert_sql, &[])
                .await
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to insert batch: {}", e)))?;

            total_written += chunk.len();
        }

        Ok(total_written)
    }

    async fn finalize(&mut self) -> Result<()> {
        // MSSQL doesn't require explicit finalization for basic operations
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        if self.client.is_none() {
            return Err(TinyEtlError::Connection("Not connected to MSSQL".to_string()));
        }

        // We need to create a temporary connection since the method is not mutable
        let (db_part, _) = Self::parse_connection_string(&self.connection_string)?;
        let mut client = Self::create_client(&db_part).await?;
        
        let query = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = @P1";
        let mut stream = client.query(query, &[&table_name])
            .await
            .map_err(|e| TinyEtlError::Connection(format!("Failed to check table existence: {}", e)))?;

        if let Some(item) = stream.try_next().await
            .map_err(|e| TinyEtlError::Connection(format!("Failed to fetch existence result: {}", e)))? {
            
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

        client.execute(&truncate_sql, &[])
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e)))?;

        Ok(())
    }

    fn supports_append(&self) -> bool {
        true // MSSQL supports INSERT operations on existing tables
    }
}
