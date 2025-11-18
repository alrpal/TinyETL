use async_trait::async_trait;
use url::Url;
use std::collections::HashMap;
use tempfile::NamedTempFile;
use std::io::Write;
use tracing::{info, debug, error, warn};

use crate::{
    Result, TinyEtlError,
    connectors::{Source, Target},
    protocols::Protocol,
    schema::{Schema, Row, Column, DataType},
};

/// Snowflake protocol that handles authentication and data transfer 
/// using temporary files as an intermediate format.
/// 
/// This is a simplified implementation that uses HTTP-based REST API
/// rather than the more complex Snowflake JDBC/ODBC drivers.
pub struct SnowflakeProtocol;

/// Parsed Snowflake connection parameters
#[derive(Debug, Clone)]
pub struct SnowflakeConnection {
    pub account: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub schema: String,
    pub warehouse: Option<String>,
    pub role: Option<String>,
    pub table: String,
}

impl SnowflakeProtocol {
    pub fn new() -> Self {
        Self
    }
    
    /// Parse a Snowflake URL into connection parameters
    /// Format: snowflake://user:pass@account.region.cloud/database/schema?warehouse=WH&role=ROLE&table=TABLE
    fn parse_url(&self, url: &Url) -> Result<SnowflakeConnection> {
        if url.scheme() != "snowflake" {
            return Err(TinyEtlError::Configuration(
                format!("Expected snowflake:// scheme, got: {}", url.scheme())
            ));
        }
        
        // Extract username and password
        let username = url.username();
        if username.is_empty() {
            return Err(TinyEtlError::Configuration(
                "Snowflake URL must include username".to_string()
            ));
        }
        
        let password = url.password().unwrap_or("");
        if password.is_empty() {
            return Err(TinyEtlError::Configuration(
                "Snowflake URL must include password".to_string()
            ));
        }
        
        // Extract account from host
        let account = url.host_str()
            .ok_or_else(|| TinyEtlError::Configuration(
                "Snowflake URL must include account in host".to_string()
            ))?
            .to_string();
        
        // Validate account is not empty
        if account.is_empty() {
            return Err(TinyEtlError::Configuration(
                "Snowflake URL must include account in host".to_string()
            ));
        }
        
        // Extract database and schema from path
        let path_segments: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();
        if path_segments.len() < 2 {
            return Err(TinyEtlError::Configuration(
                "Snowflake URL must include database and schema in path: /database/schema".to_string()
            ));
        }
        
        let database = path_segments[0].to_string();
        let schema = path_segments[1].to_string();
        
        // Parse query parameters
        let query_params: HashMap<String, String> = url.query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        
        // Table is required
        let table = query_params.get("table")
            .ok_or_else(|| TinyEtlError::Configuration(
                "Snowflake URL must include table parameter: ?table=TABLE_NAME".to_string()
            ))?
            .clone();
        
        let warehouse = query_params.get("warehouse").cloned();
        let role = query_params.get("role").cloned();
        
        Ok(SnowflakeConnection {
            account,
            username: username.to_string(),
            password: password.to_string(),
            database,
            schema,
            warehouse,
            role,
            table,
        })
    }
    
    /// Create a Snowflake connection - for now this is a placeholder
    /// In a real implementation, you'd use the snowflake-connector-rs or HTTP REST API
    async fn create_connection(&self, conn: &SnowflakeConnection) -> Result<()> {
        info!("Creating connection to Snowflake account: {}", conn.account);
        info!("Database: {}, Schema: {}, Table: {}", conn.database, conn.schema, conn.table);
        
        // For now, we'll simulate the connection
        // In a real implementation, you would:
        // 1. Authenticate with Snowflake using JWT tokens or username/password
        // 2. Establish a session
        // 3. Set the warehouse, database, schema context
        
        warn!("Snowflake protocol is currently a mock implementation");
        warn!("For production use, implement proper Snowflake REST API or JDBC connection");
        
        Ok(())
    }
}

#[async_trait]
impl Protocol for SnowflakeProtocol {
    async fn create_source(&self, url: &Url) -> Result<Box<dyn Source>> {
        let conn = self.parse_url(url)?;
        Ok(Box::new(SnowflakeSource::new(conn).await?))
    }
    
    async fn create_target(&self, url: &Url) -> Result<Box<dyn Target>> {
        let conn = self.parse_url(url)?;
        Ok(Box::new(SnowflakeTarget::new(conn).await?))
    }
    
    fn validate_url(&self, url: &Url) -> Result<()> {
        self.parse_url(url)?;
        Ok(())
    }
    
    fn name(&self) -> &'static str {
        "snowflake"
    }
}

/// Snowflake source that reads data by simulating export to temporary Parquet files
/// In production, this would use Snowflake's COPY INTO or UNLOAD commands
pub struct SnowflakeSource {
    connection: SnowflakeConnection,
    temp_file: Option<NamedTempFile>,
    parquet_source: Option<Box<dyn Source>>,
    schema: Option<Schema>,
}

impl SnowflakeSource {
    pub async fn new(connection: SnowflakeConnection) -> Result<Self> {
        Ok(Self {
            connection,
            temp_file: None,
            parquet_source: None,
            schema: None,
        })
    }
    
    async fn export_to_temp_file(&mut self) -> Result<()> {
        info!("Simulating export of Snowflake table {} to temporary Parquet file", self.connection.table);
        
        // Create temporary file
        let temp_file = NamedTempFile::new()
            .map_err(|e| TinyEtlError::Io(e))?;
        
        let temp_path = temp_file.path().to_string_lossy().to_string();
        
        // In a real implementation, you would:
        // 1. Execute a COPY INTO command: 
        //    COPY INTO '@~/temp_stage/data.parquet' FROM table_name FILE_FORMAT = (TYPE = PARQUET)
        // 2. Download the file from the Snowflake stage
        // 3. Or use UNLOAD to export directly to S3/Azure/GCS and then download
        
        warn!("Mock implementation: Creating sample Parquet file for testing");
        
        // Create a Parquet target to write sample data
        let mut parquet_target = crate::connectors::parquet::ParquetTarget::new(&temp_path)?;
        parquet_target.connect().await?;
        
        // Create a sample schema and write some mock data
        let sample_schema = Schema {
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
                    name: "created_at".to_string(),
                    data_type: DataType::DateTime,
                    nullable: true,
                },
            ],
            estimated_rows: Some(2),
            primary_key_candidate: Some("id".to_string()),
        };
        
        parquet_target.create_table("mock_table", &sample_schema).await?;
        
        // Write some sample rows
        use crate::schema::Value;
        use chrono::Utc;
        let sample_rows = vec![
            {
                let mut row = std::collections::HashMap::new();
                row.insert("id".to_string(), Value::Integer(1));
                row.insert("name".to_string(), Value::String("Sample User 1".to_string()));
                row.insert("created_at".to_string(), Value::Date(Utc::now()));
                row
            },
            {
                let mut row = std::collections::HashMap::new();
                row.insert("id".to_string(), Value::Integer(2));
                row.insert("name".to_string(), Value::String("Sample User 2".to_string()));
                row.insert("created_at".to_string(), Value::Date(Utc::now()));
                row
            }
        ];
        
        parquet_target.write_batch(&sample_rows).await?;
        parquet_target.finalize().await?;
        
        // Now create a Parquet source to read the data
        let parquet_source = crate::connectors::parquet::ParquetSource::new(&temp_path)?;
        
        self.temp_file = Some(temp_file);
        self.parquet_source = Some(Box::new(parquet_source));
        
        info!("Created mock Parquet file with sample data");
        Ok(())
    }
}

#[async_trait]
impl Source for SnowflakeSource {
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Snowflake account: {}", self.connection.account);
        
        let protocol = SnowflakeProtocol::new();
        protocol.create_connection(&self.connection).await?;
        
        info!("Successfully connected to Snowflake");
        Ok(())
    }
    
    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.schema.is_none() {
            // Export data to temp file if not already done
            if self.parquet_source.is_none() {
                self.export_to_temp_file().await?;
            }
            
            // Use the Parquet source to infer schema
            if let Some(ref mut source) = self.parquet_source {
                source.connect().await?;
                let schema = source.infer_schema(sample_size).await?;
                self.schema = Some(schema.clone());
                return Ok(schema);
            }
        }
        
        self.schema.clone()
            .ok_or_else(|| TinyEtlError::SchemaInference("Schema not available".to_string()))
    }
    
    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        // Ensure we have exported the data
        if self.parquet_source.is_none() {
            self.export_to_temp_file().await?;
        }
        
        if let Some(ref mut source) = self.parquet_source {
            // Connect if not already connected
            if self.schema.is_none() {
                source.connect().await?;
            }
            source.read_batch(batch_size).await
        } else {
            warn!("Mock implementation: No parquet source available, returning empty batch");
            Ok(vec![])
        }
    }
    
    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        if let Some(ref source) = self.parquet_source {
            source.estimated_row_count().await
        } else {
            // Mock implementation
            Ok(Some(2)) // We know we write 2 sample rows
        }
    }
    
    async fn reset(&mut self) -> Result<()> {
        if let Some(ref mut source) = self.parquet_source {
            source.reset().await
        } else {
            Ok(())
        }
    }
    
    fn has_more(&self) -> bool {
        self.parquet_source.as_ref()
            .map(|source| source.has_more())
            .unwrap_or(false)
    }
}

/// Snowflake target that simulates writing data via temporary files
/// In production, this would use Snowflake's COPY INTO or bulk loading capabilities
pub struct SnowflakeTarget {
    connection: SnowflakeConnection,
    temp_file: Option<NamedTempFile>,
    parquet_target: Option<Box<dyn Target>>,
}

impl SnowflakeTarget {
    pub async fn new(connection: SnowflakeConnection) -> Result<Self> {
        Ok(Self {
            connection,
            temp_file: None,
            parquet_target: None,
        })
    }
    
    async fn setup_temp_target(&mut self) -> Result<()> {
        if self.parquet_target.is_none() {
            // Create temporary Parquet file
            let temp_file = NamedTempFile::new()
                .map_err(|e| TinyEtlError::Io(e))?;
            
            let temp_path = temp_file.path().to_string_lossy().to_string();
            let parquet_target = crate::connectors::parquet::ParquetTarget::new(&temp_path)?;
            
            self.temp_file = Some(temp_file);
            self.parquet_target = Some(Box::new(parquet_target));
        }
        
        Ok(())
    }
    
    async fn import_from_temp_file(&mut self) -> Result<()> {
        if let Some(ref temp_file) = self.temp_file {
            let temp_path = temp_file.path().to_string_lossy().to_string();
            
            info!("Simulating import from temporary Parquet file to Snowflake table {}", self.connection.table);
            
            // In production, you would:
            // 1. Upload the Parquet file to a Snowflake stage
            // 2. Execute: COPY INTO table_name FROM @stage_name/file.parquet FILE_FORMAT = (TYPE = PARQUET)
            
            warn!("Mock implementation: Simulating data import to Snowflake");
            
            info!("Successfully simulated import to Snowflake");
        }
        
        Ok(())
    }
}

#[async_trait]
impl Target for SnowflakeTarget {
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Snowflake account: {}", self.connection.account);
        
        let protocol = SnowflakeProtocol::new();
        protocol.create_connection(&self.connection).await?;
        
        self.setup_temp_target().await?;
        
        if let Some(ref mut target) = self.parquet_target {
            target.connect().await?;
        }
        
        info!("Successfully connected to Snowflake");
        Ok(())
    }
    
    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        // Simulate creating table in Snowflake using DDL
        warn!("Mock implementation: Simulating table creation in Snowflake");
        
        let mut ddl = format!("CREATE TABLE IF NOT EXISTS {} (", self.connection.table);
        
        for (i, column) in schema.columns.iter().enumerate() {
            if i > 0 {
                ddl.push_str(", ");
            }
            
            let snowflake_type = match column.data_type {
                DataType::Integer => "INTEGER",
                DataType::Decimal => "NUMBER(38,18)", // Snowflake high precision decimal
                DataType::String => "VARCHAR(16777216)", // Snowflake max VARCHAR size
                DataType::Boolean => "BOOLEAN",
                DataType::DateTime => "TIMESTAMP",
                DataType::Date => "DATE",
                DataType::Json => "VARIANT", // Snowflake native semi-structured data type
                DataType::Null => "VARCHAR(16777216)", // Default to VARCHAR for null types
            };
            
            ddl.push_str(&format!("{} {}", column.name, snowflake_type));
        }
        ddl.push(')');
        
        info!("Simulating Snowflake table creation: {}", ddl);
        
        // In production, you would execute this DDL against Snowflake
        
        // Also create table in temporary Parquet target
        if let Some(ref mut target) = self.parquet_target {
            target.create_table(table_name, schema).await?;
        }
        
        Ok(())
    }
    
    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if let Some(ref mut target) = self.parquet_target {
            target.write_batch(rows).await
        } else {
            Ok(0)
        }
    }
    
    async fn finalize(&mut self) -> Result<()> {
        // First finalize the Parquet target
        if let Some(ref mut target) = self.parquet_target {
            target.finalize().await?;
        }
        
        // Then import the data to Snowflake
        self.import_from_temp_file().await?;
        
        Ok(())
    }
    
    async fn exists(&self, table_name: &str) -> Result<bool> {
        // Mock implementation
        // In production, you would query INFORMATION_SCHEMA.TABLES
        warn!("Mock implementation: Assuming table does not exist");
        Ok(false)
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        // Mock implementation
        // In production, you would execute TRUNCATE TABLE statement
        warn!("Mock implementation: Simulating table truncation in Snowflake");
        info!("Simulating Snowflake table truncation: TRUNCATE TABLE {}", table_name);
        
        // Also truncate the temporary Parquet target
        if let Some(ref mut target) = self.parquet_target {
            target.truncate(table_name).await?;
        }
        
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // Snowflake supports appending data to existing tables
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_snowflake_url() {
        let protocol = SnowflakeProtocol::new();
        
        // Valid Snowflake URL
        let url = Url::parse("snowflake://alex:password@xy12345.east-us.azure/mydb/public?warehouse=COMPUTE_WH&table=sales").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.username, "alex");
        assert_eq!(conn.password, "password");
        assert_eq!(conn.account, "xy12345.east-us.azure");
        assert_eq!(conn.database, "mydb");
        assert_eq!(conn.schema, "public");
        assert_eq!(conn.warehouse, Some("COMPUTE_WH".to_string()));
        assert_eq!(conn.table, "sales");
    }
    
    #[test]
    fn test_parse_snowflake_url_missing_table() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("snowflake://alex:password@xy12345.east-us.azure/mydb/public?warehouse=COMPUTE_WH").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("table parameter"));
    }
    
    #[test]
    fn test_parse_snowflake_url_missing_credentials() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("snowflake://xy12345.east-us.azure/mydb/public?table=sales").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("username"));
    }
    
    #[test]
    fn test_validate_url() {
        let protocol = SnowflakeProtocol::new();
        
        // Valid URL
        let url = Url::parse("snowflake://alex:password@xy12345.east-us.azure/mydb/public?warehouse=COMPUTE_WH&table=sales").unwrap();
        assert!(protocol.validate_url(&url).is_ok());
        
        // Invalid scheme
        let url = Url::parse("http://example.com").unwrap();
        assert!(protocol.validate_url(&url).is_err());
    }

    #[test]
    fn test_protocol_name() {
        let protocol = SnowflakeProtocol::new();
        assert_eq!(protocol.name(), "snowflake");
    }

    #[test]
    fn test_parse_url_missing_password() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with username but no password
        let url = Url::parse("snowflake://alex@xy12345.east-us.azure/mydb/public?table=sales").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("password"));
        }
    }

    #[test]
    fn test_parse_url_missing_database() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with no path segments
        let url = Url::parse("snowflake://alex:pass@xy12345.east-us.azure/?table=sales").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("database and schema"));
        }
    }

    #[test]
    fn test_parse_url_missing_schema() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with only database in path
        let url = Url::parse("snowflake://alex:pass@xy12345.east-us.azure/mydb?table=sales").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("database and schema"));
        }
    }

    #[test]
    fn test_parse_url_with_role() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("snowflake://user:pass@account.region.cloud/db/schema?warehouse=WH&role=ANALYST&table=data").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.role, Some("ANALYST".to_string()));
        assert_eq!(conn.warehouse, Some("WH".to_string()));
    }

    #[test]
    fn test_parse_url_without_optional_params() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("snowflake://user:pass@account.region.cloud/db/schema?table=data").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.username, "user");
        assert_eq!(conn.password, "pass");
        assert_eq!(conn.database, "db");
        assert_eq!(conn.schema, "schema");
        assert_eq!(conn.table, "data");
        assert_eq!(conn.warehouse, None);
        assert_eq!(conn.role, None);
    }

    #[test]
    fn test_parse_url_with_special_characters_in_password() {
        let protocol = SnowflakeProtocol::new();
        
        // URL-encoded special characters in password
        // Note: url.password() returns the percent-encoded form, not decoded
        let url = Url::parse("snowflake://user:p%40ssw0rd%21@account.region.cloud/db/schema?table=data").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.username, "user");
        // The password is stored as-is from URL (percent-encoded)
        assert_eq!(conn.password, "p%40ssw0rd%21");
    }

    #[test]
    fn test_parse_url_different_regions() {
        let protocol = SnowflakeProtocol::new();
        
        let regions = vec![
            "account.us-east-1.aws",
            "account.eu-west-1.aws",
            "account.ap-southeast-2.aws",
            "account.eastus2.azure",
            "account.westeurope.azure",
        ];
        
        for region in regions {
            let url_str = format!("snowflake://user:pass@{}/db/schema?table=test", region);
            let url = Url::parse(&url_str).unwrap();
            let conn = protocol.parse_url(&url).unwrap();
            
            assert_eq!(conn.account, region);
        }
    }

    #[test]
    fn test_parse_url_invalid_scheme() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("mysql://user:pass@host/db?table=test").unwrap();
        let result = protocol.parse_url(&url);
        
        assert!(result.is_err());
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("snowflake://"));
            assert!(msg.contains("mysql"));
        }
    }

    #[test]
    fn test_parse_url_case_sensitive_params() {
        let protocol = SnowflakeProtocol::new();
        
        // Snowflake parameters are typically case-insensitive in practice,
        // but our parser is case-sensitive for query params
        let url = Url::parse("snowflake://user:pass@account.region.cloud/DB/SCHEMA?table=TABLE&warehouse=WH").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.database, "DB");
        assert_eq!(conn.schema, "SCHEMA");
        assert_eq!(conn.table, "TABLE");
    }

    #[test]
    fn test_parse_url_with_extra_query_params() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with extra parameters that should be ignored
        let url = Url::parse("snowflake://user:pass@account.region.cloud/db/schema?table=data&warehouse=WH&custom=value&timeout=30").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.table, "data");
        assert_eq!(conn.warehouse, Some("WH".to_string()));
        // custom and timeout params are not parsed but shouldn't cause errors
    }

    #[test]
    fn test_snowflake_connection_clone() {
        let conn = SnowflakeConnection {
            account: "account".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            database: "db".to_string(),
            schema: "schema".to_string(),
            warehouse: Some("WH".to_string()),
            role: Some("ROLE".to_string()),
            table: "table".to_string(),
        };
        
        let cloned = conn.clone();
        
        assert_eq!(conn.account, cloned.account);
        assert_eq!(conn.username, cloned.username);
        assert_eq!(conn.password, cloned.password);
        assert_eq!(conn.table, cloned.table);
    }

    #[test]
    fn test_connection_debug_format() {
        let conn = SnowflakeConnection {
            account: "test_account".to_string(),
            username: "test_user".to_string(),
            password: "secret".to_string(),
            database: "test_db".to_string(),
            schema: "test_schema".to_string(),
            warehouse: None,
            role: None,
            table: "test_table".to_string(),
        };
        
        let debug_str = format!("{:?}", conn);
        assert!(debug_str.contains("test_account"));
        assert!(debug_str.contains("test_user"));
        assert!(debug_str.contains("test_table"));
    }

    #[test]
    fn test_parse_url_empty_table_param() {
        let protocol = SnowflakeProtocol::new();
        
        // Table parameter exists but is empty
        let url = Url::parse("snowflake://user:pass@account.region.cloud/db/schema?table=").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        // Empty table name is technically valid in URL parsing
        assert_eq!(conn.table, "");
    }

    #[test]
    fn test_parse_url_complex_path() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with additional path segments (should only use first two)
        let url = Url::parse("snowflake://user:pass@account.region.cloud/db/schema/extra/path?table=test").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.database, "db");
        assert_eq!(conn.schema, "schema");
    }

    #[test]
    fn test_parse_url_no_host() {
        let protocol = SnowflakeProtocol::new();
        
        // Try to parse URL with empty host - this will fail at URL parsing level
        match Url::parse("snowflake://user:pass@/db/schema?table=test") {
            Ok(url) => {
                // If it somehow parses, it should fail at protocol level
                let result = protocol.parse_url(&url);
                assert!(result.is_err());
            }
            Err(_) => {
                // Expected: URL parsing itself fails for empty host
                // This is the correct behavior
            }
        }
    }

    #[test]
    fn test_parse_url_localhost_host() {
        let protocol = SnowflakeProtocol::new();
        
        // URL with valid host but missing other required fields
        let url = Url::parse("snowflake://user:pass@localhost/db/schema").unwrap();
        let result = protocol.parse_url(&url);
        // Should fail because table parameter is missing
        assert!(result.is_err());
        match result {
            Err(TinyEtlError::Configuration(msg)) => {
                assert!(msg.contains("table"), "Error message was: {}", msg);
            }
            _ => panic!("Expected Configuration error about table"),
        }
    }

    #[test]
    fn test_validate_url_with_different_schemes() {
        let protocol = SnowflakeProtocol::new();
        
        let invalid_schemes = vec![
            "http://example.com",
            "https://example.com",
            "mysql://host/db",
            "postgres://host/db",
            "ftp://example.com",
        ];
        
        for scheme in invalid_schemes {
            let url = Url::parse(scheme).unwrap();
            assert!(protocol.validate_url(&url).is_err(), "Should reject scheme: {}", scheme);
        }
    }

    #[test]
    fn test_snowflake_url_with_port() {
        let protocol = SnowflakeProtocol::new();
        
        // Snowflake doesn't typically use custom ports, but test URL parsing
        let url = Url::parse("snowflake://user:pass@account.region.cloud:443/db/schema?table=test").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        // Port is part of the host in URL parsing
        assert!(conn.account.contains("account.region.cloud"));
    }

    #[test]
    fn test_parse_url_with_underscore_and_numbers() {
        let protocol = SnowflakeProtocol::new();
        
        let url = Url::parse("snowflake://user_123:pass@account_456.region.cloud/db_test/schema_v2?table=table_2024&warehouse=WH_PROD").unwrap();
        let conn = protocol.parse_url(&url).unwrap();
        
        assert_eq!(conn.username, "user_123");
        assert_eq!(conn.account, "account_456.region.cloud");
        assert_eq!(conn.database, "db_test");
        assert_eq!(conn.schema, "schema_v2");
        assert_eq!(conn.table, "table_2024");
        assert_eq!(conn.warehouse, Some("WH_PROD".to_string()));
    }
}
