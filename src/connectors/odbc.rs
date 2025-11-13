use async_trait::async_trait;
use odbc_api::{
    Environment, Connection, Cursor, ConnectionOptions, ResultSetMetadata, IntoParameter,
    handles::ColumnDescription,
};
use rust_decimal::Decimal;
use chrono::{DateTime, Utc, NaiveDateTime};
use std::sync::Arc;

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, Column, DataType},
    connectors::{Source, Target},
};

/// Wrapper that keeps Environment and Connection together with proper lifetimes
struct OdbcConnection {
    _env: Arc<Environment>,
    // Connection is stored as a raw pointer to avoid lifetime issues
    // We manually manage the lifetime to ensure env outlives connection
    connection: *mut Connection<'static>,
}

impl OdbcConnection {
    fn new(connection_string: &str) -> Result<Self> {
        let env = Environment::new()
            .map_err(|e| TinyEtlError::Connection(format!("Failed to create ODBC environment: {}", e)))?;
        
        let env = Arc::new(env);
        
        // Create connection with proper lifetime management
        let connection = unsafe {
            let env_ref: &'static Environment = std::mem::transmute(env.as_ref());
            let conn = env_ref.connect_with_connection_string(connection_string, ConnectionOptions::default())
                .map_err(|e| TinyEtlError::Connection(format!("Failed to connect to ODBC data source: {}", e)))?;
            Box::into_raw(Box::new(conn))
        };
        
        Ok(Self {
            _env: env,
            connection,
        })
    }
    
    fn connection(&self) -> &Connection<'static> {
        unsafe { &*self.connection }
    }
}

impl Drop for OdbcConnection {
    fn drop(&mut self) {
        // Manually drop the connection before the environment
        unsafe {
            if !self.connection.is_null() {
                let _ = Box::from_raw(self.connection);
            }
        }
        // env will be dropped after this
    }
}

// Safety: OdbcConnection can be sent across threads
unsafe impl Send for OdbcConnection {}
unsafe impl Sync for OdbcConnection {}

/// ODBC source connector for reading data from ODBC-compatible databases
pub struct OdbcSource {
    connection_string: String,
    table_name: String,
    connection: Option<Arc<OdbcConnection>>,
    current_offset: usize,
    total_rows: Option<usize>,
}

impl OdbcSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (conn_str, table_name) = Self::parse_connection_string(connection_string)?;
        
        Ok(Self {
            connection_string: conn_str,
            table_name,
            connection: None,
            current_offset: 0,
            total_rows: None,
        })
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        // Format: odbc://DSN=MyDataSource;UID=user;PWD=pass#table
        // or: odbc://Driver={SQL Server};Server=localhost;Database=mydb;UID=user;PWD=pass#table
        
        let connection_string = connection_string.trim_start_matches("odbc://");
        
        if let Some((conn_part, table_part)) = connection_string.split_once('#') {
            Ok((conn_part.to_string(), table_part.to_string()))
        } else {
            Err(TinyEtlError::Configuration(
                "ODBC source requires table specification: odbc://connection_string#table".to_string()
            ))
        }
    }

    fn map_odbc_type_to_datatype(col_desc: &ColumnDescription) -> DataType {
        use odbc_api::DataType as OdbcDataType;
        
        match &col_desc.data_type {
            OdbcDataType::Unknown => DataType::String,
            OdbcDataType::Char { .. } | OdbcDataType::Varchar { .. } | 
            OdbcDataType::WChar { .. } | OdbcDataType::WVarchar { .. } | 
            OdbcDataType::LongVarchar { .. } => DataType::String,
            OdbcDataType::Decimal { .. } | OdbcDataType::Numeric { .. } => DataType::Decimal,
            OdbcDataType::SmallInt | OdbcDataType::Integer | OdbcDataType::TinyInt |
            OdbcDataType::BigInt => DataType::Integer,
            OdbcDataType::Real | OdbcDataType::Float { .. } | OdbcDataType::Double => DataType::Decimal,
            OdbcDataType::Bit => DataType::Boolean,
            OdbcDataType::Date => DataType::Date,
            OdbcDataType::Time { .. } | OdbcDataType::Timestamp { .. } => DataType::DateTime,
            OdbcDataType::Binary { .. } | OdbcDataType::Varbinary { .. } | 
            OdbcDataType::LongVarbinary { .. } => DataType::String,
            OdbcDataType::Other { .. } => DataType::String,
        }
    }
}

#[async_trait]
impl Source for OdbcSource {
    async fn connect(&mut self) -> Result<()> {
        let conn = OdbcConnection::new(&self.connection_string)?;
        self.connection = Some(Arc::new(conn));
        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Query to get column metadata - fetch a single row to infer schema
        // Use quoted identifier for table name
        let query = format!("SELECT * FROM [{}] WHERE 1=0", self.table_name);
        
        let cursor = conn.execute(&query, ())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to query table metadata: {}", e)))?
            .ok_or_else(|| TinyEtlError::DataTransfer("No cursor returned for metadata query".to_string()))?;
        
        let mut cursor = cursor;
        let num_cols = cursor.num_result_cols()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column count: {}", e)))?;
        
        let mut columns = Vec::new();
        
        for i in 1..=num_cols {
            let mut col_desc = ColumnDescription::default();
            cursor.describe_col(i as u16, &mut col_desc)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to describe column {}: {}", i, e)))?;
            
            let name = col_desc.name_to_string()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column name: {}", e)))?;
            
            let data_type = Self::map_odbc_type_to_datatype(&col_desc);
            let nullable = col_desc.nullability == odbc_api::Nullability::Nullable;
            
            columns.push(Column {
                name,
                data_type,
                nullable,
            });
        }
        
        Ok(Schema { 
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Query with OFFSET and FETCH for pagination
        // Note: This syntax works for SQL Server, PostgreSQL, and many modern databases
        // For older databases, you may need to adjust this
        // Use quoted identifier for table name
        let query = format!(
            "SELECT * FROM [{}] ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
            self.table_name, self.current_offset, batch_size
        );
        
        let mut cursor = match conn.execute(&query, ())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to execute query: {}", e)))? {
            Some(c) => c,
            None => return Ok(Vec::new()),
        };
        
        let num_cols = cursor.num_result_cols()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column count: {}", e)))?;
        
        // Get column names and types
        let mut col_info = Vec::new();
        for i in 1..=num_cols {
            let mut col_desc = ColumnDescription::default();
            cursor.describe_col(i as u16, &mut col_desc)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to describe column {}: {}", i, e)))?;
            
            let name = col_desc.name_to_string()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column name: {}", e)))?;
            
            col_info.push((name, i as u16));
        }
        
        // Fetch rows one by one
        let mut rows = Vec::new();
        
        while let Some(mut row_cursor) = cursor.next_row()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to fetch row: {}", e)))? {
            
            if rows.len() >= batch_size {
                break;
            }
            
            let mut row = Row::new();
            
            for (col_name, col_idx) in &col_info {
                // Get data as text - simplest and most compatible approach
                let mut buf = vec![0u8; 4096];
                let is_non_null = row_cursor.get_text(*col_idx, &mut buf)
                    .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column value: {}", e)))?;
                
                let value = if is_non_null {
                    // Convert buffer to string
                    let text = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
                    Value::String(text)
                } else {
                    Value::Null
                };
                
                row.insert(col_name.clone(), value);
            }
            
            rows.push(row);
        }
        
        self.current_offset += rows.len();
        
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(self.total_rows)
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_offset = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        // We don't know ahead of time, so return true until we get an empty batch
        true
    }
}

/// ODBC target connector for writing data to ODBC-compatible databases
pub struct OdbcTarget {
    connection_string: String,
    table_name: String,
    connection: Option<Arc<OdbcConnection>>,
    schema: Option<Schema>,
}

impl OdbcTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (conn_str, table_name) = Self::parse_connection_string(connection_string)?;
        
        Ok(Self {
            connection_string: conn_str,
            table_name,
            connection: None,
            schema: None,
        })
    }

    fn parse_connection_string(connection_string: &str) -> Result<(String, String)> {
        let connection_string = connection_string.trim_start_matches("odbc://");
        
        if let Some((conn_part, table_part)) = connection_string.split_once('#') {
            Ok((conn_part.to_string(), table_part.to_string()))
        } else {
            Err(TinyEtlError::Configuration(
                "ODBC target requires table specification: odbc://connection_string#table".to_string()
            ))
        }
    }

    fn map_datatype_to_sql(&self, data_type: &DataType) -> &str {
        match data_type {
            DataType::String => "VARCHAR(255)",
            DataType::Integer => "BIGINT",
            DataType::Decimal => "DECIMAL(18,4)",
            DataType::Boolean => "BIT",
            DataType::Date => "DATE",
            DataType::DateTime => "DATETIME2",  // Use DATETIME2 instead of TIMESTAMP for SQL Server
            DataType::Null => "VARCHAR(255)", // Default to VARCHAR for NULL type
        }
    }
}

#[async_trait]
impl Target for OdbcTarget {
    async fn connect(&mut self) -> Result<()> {
        let conn = OdbcConnection::new(&self.connection_string)?;
        self.connection = Some(Arc::new(conn));
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Build CREATE TABLE statement with proper identifier quoting for SQL Server
        let mut create_sql = format!("CREATE TABLE [{}] (", table_name);
        
        for (i, col) in schema.columns.iter().enumerate() {
            if i > 0 {
                create_sql.push_str(", ");
            }
            
            let sql_type = self.map_datatype_to_sql(&col.data_type);
            let nullable = if col.nullable { "" } else { " NOT NULL" };
            
            // Quote column names with square brackets for SQL Server
            create_sql.push_str(&format!("[{}] {}{}", col.name, sql_type, nullable));
        }
        
        create_sql.push(')');
        
        // Execute CREATE TABLE
        conn.execute(&create_sql, ())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create table: {}", e)))?;
        
        self.schema = Some(schema.clone());
        
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let schema = self.schema.as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set for ODBC target".to_string()))?;
        
        let conn = conn.connection();
        
        // ODBC has limits on parameter counts. SQL Server typically supports up to ~2100 parameters.
        // To be safe, we'll chunk inserts to stay well under this limit.
        // With 12 columns, we can do ~150 rows per insert (12 * 150 = 1800 parameters)
        let max_params = 2000;
        let params_per_row = schema.columns.len();
        let chunk_size = (max_params / params_per_row).max(1);
        
        let mut total_inserted = 0;
        
        // Process rows in chunks
        for chunk in rows.chunks(chunk_size) {
            // Build column names list (quoted for SQL Server compatibility)
            let column_names = schema.columns.iter()
                .map(|c| format!("[{}]", c.name))
                .collect::<Vec<_>>()
                .join(", ");
            
            // Build bulk INSERT statement with multiple value sets for this chunk
            // Format: INSERT INTO table (col1, col2) VALUES (?,?),(?,?),(?,?)
            let single_row_placeholders = schema.columns.iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            
            let all_value_sets = (0..chunk.len())
                .map(|_| format!("({})", single_row_placeholders))
                .collect::<Vec<_>>()
                .join(", ");
            
            let insert_sql = format!(
                "INSERT INTO [{}] ({}) VALUES {}",
                self.table_name, column_names, all_value_sets
            );
            
            // Prepare statement for this chunk
            let mut prepared = conn.prepare(&insert_sql)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to prepare bulk INSERT statement: {}", e)))?;
            
            // Build parameters for this chunk
            let total_params_chunk = chunk.len() * schema.columns.len();
            let mut param_strings = Vec::with_capacity(total_params_chunk);
            
            // Convert all values to strings upfront
            for row in chunk {
                for col in &schema.columns {
                    let value = row.get(&col.name).unwrap_or(&Value::Null);
                    let string_value = match value {
                        Value::String(s) => s.clone(),
                        Value::Integer(i) => i.to_string(),
                        Value::Decimal(d) => d.to_string(),
                        Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
                        Value::Date(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                        Value::Null => String::new(),
                    };
                    param_strings.push(string_value);
                }
            }
            
            // Build parameter references for ODBC
            let mut param_idx = 0;
            let mut param_values = Vec::with_capacity(total_params_chunk);
            
            for row in chunk {
                for col in &schema.columns {
                    let value = row.get(&col.name).unwrap_or(&Value::Null);
                    let opt_str = match value {
                        Value::Null => None,
                        _ => Some(param_strings[param_idx].as_str()),
                    };
                    param_values.push(opt_str);
                    param_idx += 1;
                }
            }
            
            // Convert to ODBC parameters
            let params: Vec<_> = param_values.iter()
                .map(|opt_str| opt_str.as_ref().map(|s| *s).into_parameter())
                .collect();
            
            // Execute bulk insert for this chunk
            prepared.execute(&params[..])
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to insert batch chunk: {}", e)))?;
            
            total_inserted += chunk.len();
        }
        
        Ok(total_inserted)
    }

    async fn finalize(&mut self) -> Result<()> {
        // Commit any pending transactions
        if let Some(conn) = &self.connection {
            let conn = conn.connection();
            conn.commit()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to commit transaction: {}", e)))?;
        }
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        if self.connection.is_none() {
            return Ok(false);
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Try to query the table - if it fails, it doesn't exist
        let query = format!("SELECT 1 FROM [{}] WHERE 1=0", table_name);
        let result = conn.execute(&query, ());
        
        match result {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Use DELETE instead of TRUNCATE for better compatibility
        let truncate_sql = format!("DELETE FROM [{}]", table_name);
        conn.execute(&truncate_sql, ())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e)))?;
        
        Ok(())
    }

    fn supports_append(&self) -> bool {
        true
    }
}
