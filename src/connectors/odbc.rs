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
    // Performance optimization: Track last primary key value for cursor-based pagination
    last_pk_value: Option<String>,
    pk_column: Option<String>,
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
            last_pk_value: None,
            pk_column: None,
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
        let mut pk_candidate = None;
        
        for i in 1..=num_cols {
            let mut col_desc = ColumnDescription::default();
            cursor.describe_col(i as u16, &mut col_desc)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to describe column {}: {}", i, e)))?;
            
            let name = col_desc.name_to_string()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column name: {}", e)))?;
            
            let data_type = Self::map_odbc_type_to_datatype(&col_desc);
            let nullable = col_desc.nullability == odbc_api::Nullability::Nullable;
            
            // Try to identify a primary key candidate for optimized pagination
            // Look for common PK patterns: id, *_id, *Id columns that are integers and not nullable
            if pk_candidate.is_none() && !nullable && matches!(data_type, DataType::Integer) {
                let lower_name = name.to_lowercase();
                if lower_name == "id" || lower_name.ends_with("_id") || lower_name.ends_with("id") {
                    pk_candidate = Some(name.clone());
                }
            }
            
            columns.push(Column {
                name,
                data_type,
                nullable,
            });
        }
        
        // Store PK candidate for optimized reads
        self.pk_column = pk_candidate.clone();
        
        Ok(Schema { 
            columns,
            estimated_rows: None,
            primary_key_candidate: pk_candidate,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // PERFORMANCE OPTIMIZATION: Use cursor-based pagination with PK if available
        // This is MUCH faster than OFFSET for large tables (O(1) vs O(n))
        let query = if let Some(pk_col) = &self.pk_column {
            if let Some(last_val) = &self.last_pk_value {
                // Cursor-based: WHERE pk > last_value ORDER BY pk
                format!(
                    "SELECT * FROM [{}] WHERE [{}] > {} ORDER BY [{}] ASC FETCH NEXT {} ROWS ONLY",
                    self.table_name, pk_col, last_val, pk_col, batch_size
                )
            } else {
                // First batch with PK ordering
                format!(
                    "SELECT * FROM [{}] ORDER BY [{}] ASC FETCH NEXT {} ROWS ONLY",
                    self.table_name, pk_col, batch_size
                )
            }
        } else {
            // Fallback: OFFSET-based pagination (slower for large tables)
            // Note: This syntax works for SQL Server, PostgreSQL, and many modern databases
            format!(
                "SELECT * FROM [{}] ORDER BY (SELECT NULL) OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                self.table_name, self.current_offset, batch_size
            )
        };
        
        let mut cursor = match conn.execute(&query, ())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to execute query: {}", e)))? {
            Some(c) => c,
            None => return Ok(Vec::new()),
        };
        
        let num_cols = cursor.num_result_cols()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column count: {}", e)))?;
        
        // Get column names and types
        let mut col_info = Vec::new();
        let mut pk_col_idx = None;
        
        for i in 1..=num_cols {
            let mut col_desc = ColumnDescription::default();
            cursor.describe_col(i as u16, &mut col_desc)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to describe column {}: {}", i, e)))?;
            
            let name = col_desc.name_to_string()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column name: {}", e)))?;
            
            // Track PK column index for cursor updates
            if let Some(pk_col) = &self.pk_column {
                if &name == pk_col {
                    pk_col_idx = Some(i as u16);
                }
            }
            
            col_info.push((name, i as u16));
        }
        
        // PERFORMANCE OPTIMIZATION: Reuse a single buffer for all text reads
        // This eliminates allocating 4KB per column per row
        let mut text_buffer = vec![0u8; 8192]; // Larger buffer for better performance
        
        // Fetch rows one by one
        let mut rows = Vec::new();
        
        while let Some(mut row_cursor) = cursor.next_row()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to fetch row: {}", e)))? {
            
            if rows.len() >= batch_size {
                break;
            }
            
            let mut row = Row::new();
            let mut pk_value = None;
            
            for (col_name, col_idx) in &col_info {
                // Reuse buffer for text reading
                text_buffer.clear();
                text_buffer.resize(8192, 0);
                
                let is_non_null = row_cursor.get_text(*col_idx, &mut text_buffer)
                    .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to get column value: {}", e)))?;
                
                let value = if is_non_null {
                    // Convert buffer to string - find the null terminator
                    let end = text_buffer.iter().position(|&b| b == 0).unwrap_or(text_buffer.len());
                    let text = String::from_utf8_lossy(&text_buffer[..end]).to_string();
                    
                    // Track PK value for next cursor iteration
                    if Some(*col_idx) == pk_col_idx {
                        pk_value = Some(text.clone());
                    }
                    
                    Value::String(text)
                } else {
                    Value::Null
                };
                
                row.insert(col_name.clone(), value);
            }
            
            // Update last PK value for cursor-based pagination
            if let Some(val) = pk_value {
                self.last_pk_value = Some(val);
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
        self.last_pk_value = None;
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
    // Performance optimization: Track transaction state
    in_transaction: bool,
}

impl OdbcTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (conn_str, table_name) = Self::parse_connection_string(connection_string)?;
        
        Ok(Self {
            connection_string: conn_str,
            table_name,
            connection: None,
            schema: None,
            in_transaction: false,
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
            DataType::Json => "NVARCHAR(MAX)", // ODBC/SQL Server stores JSON as NVARCHAR
            DataType::Null => "VARCHAR(255)", // Default to VARCHAR for NULL type
        }
    }
}

#[async_trait]
impl Target for OdbcTarget {
    async fn connect(&mut self) -> Result<()> {
        let conn = OdbcConnection::new(&self.connection_string)?;
        self.connection = Some(Arc::new(conn));
        
        // PERFORMANCE: Start a single transaction for the entire transfer
        // This is MUCH faster than committing after each batch
        let conn_ref = self.connection.as_ref().unwrap();
        let conn = conn_ref.connection();
        
        // Set autocommit off and begin transaction
        conn.execute("SET IMPLICIT_TRANSACTIONS ON", ())
            .map_err(|e| TinyEtlError::Connection(format!("Failed to set implicit transactions: {}", e)))?;
        
        self.in_transaction = true;
        
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        if self.connection.is_none() {
            self.connect().await?;
        }

        let conn = self.connection.as_ref()
            .ok_or_else(|| TinyEtlError::Connection("ODBC connection not established".to_string()))?;
        
        let conn = conn.connection();
        
        // Check if table already exists
        let check_query = format!("SELECT 1 FROM [{}] WHERE 1=0", table_name);
        let table_exists = conn.execute(&check_query, ()).is_ok();
        
        if !table_exists {
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
        }
        
        // Always set the schema regardless of whether table was created
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
        
        // OPTIMIZATION: NO per-batch transaction!
        // Transaction is managed at connect() and finalize() level for max performance
        
        // Optimize chunk size - use larger chunks for better performance
        let max_params = 1900;
        let params_per_row = schema.columns.len();
        let chunk_size = (max_params / params_per_row).max(1).min(500);
        
        let mut total_inserted = 0;
        
        let column_names = schema.columns.iter()
            .map(|c| format!("[{}]", c.name))
            .collect::<Vec<_>>()
            .join(", ");
        
        for chunk in rows.chunks(chunk_size) {
            // Build all parameter strings first, then create parameter references
            let total_params = chunk.len() * schema.columns.len();
            let mut param_strings = Vec::with_capacity(total_params);
            
            // First pass: collect all string representations
            for row in chunk {
                for col in &schema.columns {
                    let value = row.get(&col.name).unwrap_or(&Value::Null);
                    
                    match value {
                        Value::Null => {
                            param_strings.push(None);
                        }
                        Value::String(s) => {
                            param_strings.push(Some(s.clone()));
                        }
                        Value::Integer(i) => {
                            param_strings.push(Some(i.to_string()));
                        }
                        Value::Decimal(d) => {
                            param_strings.push(Some(d.to_string()));
                        }
                        Value::Boolean(b) => {
                            param_strings.push(Some(if *b { "1" } else { "0" }.to_string()));
                        }
                        Value::Date(dt) => {
                            param_strings.push(Some(dt.format("%Y-%m-%d %H:%M:%S").to_string()));
                        }
                        Value::Json(j) => {
                            let json_str = serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string());
                            param_strings.push(Some(json_str));
                        }
                    }
                }
            }
            
            // Second pass: create parameter references
            let mut params = Vec::with_capacity(total_params);
            for param_str in &param_strings {
                match param_str {
                    Some(s) => params.push(Some(s.as_str()).into_parameter()),
                    None => params.push(None::<&str>.into_parameter()),
                }
            }
            
            // Build INSERT statement for this specific chunk size
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
            
            // Prepare and execute
            let mut prepared = conn.prepare(&insert_sql)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to prepare INSERT: {}", e)))?;
            
            prepared.execute(&params[..])
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to insert chunk: {}", e)))?;
            
            total_inserted += chunk.len();
        }
        
        Ok(total_inserted)
    }

    async fn finalize(&mut self) -> Result<()> {
        // PERFORMANCE: Commit the single transaction started in connect()
        // This commits ALL batches at once for maximum speed
        if self.in_transaction {
            if let Some(conn) = &self.connection {
                let conn = conn.connection();
                conn.commit()
                    .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to commit transaction: {}", e)))?;
                self.in_transaction = false;
            }
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
