use std::path::PathBuf;
use std::collections::HashMap;
use async_trait::async_trait;
use csv::{ReaderBuilder, WriterBuilder};
use serde_json;
use rust_decimal::Decimal;

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, SchemaInferer},
    connectors::{Source, Target},
    date_parser::DateParser,
};

pub struct CsvSource {
    file_path: PathBuf,
    reader: Option<csv::Reader<std::fs::File>>,
    headers: Vec<String>,
    current_position: u64,
    has_more_data: bool,
}

impl CsvSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            reader: None,
            headers: Vec::new(),
            current_position: 0,
            has_more_data: true,
        })
    }
    
    fn infer_schema_with_order(&self, rows: &[Row]) -> Result<Schema> {
        if rows.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        let mut column_types: HashMap<String, Vec<crate::schema::DataType>> = HashMap::new();
        
        // Use the CSV headers order instead of HashMap iteration order
        for col_name in &self.headers {
            let mut types = Vec::new();
            for row in rows {
                let data_type = match row.get(col_name) {
                    Some(value) => crate::schema::SchemaInferer::infer_type(value),
                    None => crate::schema::DataType::Null,
                };
                types.push(data_type);
            }
            column_types.insert(col_name.clone(), types);
        }

        // Determine final type for each column, preserving header order
        let columns = self.headers
            .iter()
            .filter_map(|col_name| {
                column_types.get(col_name).map(|types| {
                    let (data_type, nullable) = crate::schema::SchemaInferer::resolve_column_type(types);
                    crate::schema::Column {
                        name: col_name.clone(),
                        data_type,
                        nullable,
                    }
                })
            })
            .collect();

        Ok(Schema {
            columns,
            estimated_rows: Some(rows.len()),
            primary_key_candidate: None,
        })
    }
    
    fn parse_value(value: &str) -> Value {
        // Try to parse as different types
        
        // Try integer first
        if let Ok(int_val) = value.parse::<i64>() {
            return Value::Integer(int_val);
        }
        
        // Try decimal
        if let Ok(decimal_val) = value.parse::<Decimal>() {
            return Value::Decimal(decimal_val);
        }
        
        // Try boolean
        if let Ok(bool_val) = value.parse::<bool>() {
            return Value::Boolean(bool_val);
        }
        
        // Try date/datetime using the shared parser
        if let Some(date_value) = DateParser::try_parse(value) {
            return date_value;
        }
        
        // Default to string
        if value.is_empty() {
            Value::Null
        } else {
            Value::String(value.to_string())
        }
    }
}

#[async_trait]
impl Source for CsvSource {
    async fn connect(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(
                format!("CSV file not found: {}", self.file_path.display())
            ));
        }

        let file = std::fs::File::open(&self.file_path)?;
        let mut reader = ReaderBuilder::new()
            .has_headers(true)
            .from_reader(file);

        // Read and store headers
        self.headers = reader.headers()?.iter().map(|h| h.to_string()).collect();
        
        // Reset file for actual reading
        let file = std::fs::File::open(&self.file_path)?;
        self.reader = Some(ReaderBuilder::new()
            .has_headers(true)
            .from_reader(file));

        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.reader.is_none() {
            self.connect().await?;
        }

        let mut sample_rows = Vec::new();
        let mut count = 0;

        if let Some(ref mut reader) = self.reader {
            for result in reader.records() {
                if count >= sample_size {
                    break;
                }

                let record = result?;
                let mut row = Row::new();
                
                for (i, field) in record.iter().enumerate() {
                    if let Some(header) = self.headers.get(i) {
                        let value = Self::parse_value(field);
                        row.insert(header.clone(), value);
                    }
                }
                
                sample_rows.push(row);
                count += 1;
            }
        }

        // Reset for future reading
        self.reset().await?;

        // Custom schema inference that preserves column order from CSV headers
        self.infer_schema_with_order(&sample_rows)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.reader.is_none() {
            self.connect().await?;
        }

        let mut rows = Vec::new();
        let mut count = 0;

        if let Some(ref mut reader) = self.reader {
            let mut records_iter = reader.records();
            
            while count < batch_size {
                match records_iter.next() {
                    Some(Ok(record)) => {
                        let mut row = Row::new();
                        
                        for (i, field) in record.iter().enumerate() {
                            if let Some(header) = self.headers.get(i) {
                                let value = Self::parse_value(field);
                                row.insert(header.clone(), value);
                            }
                        }
                        
                        rows.push(row);
                        count += 1;
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => {
                        // No more records available
                        self.has_more_data = false;
                        break;
                    }
                }
            }
        }

        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        // Simple estimation by counting lines (not perfect but fast)
        let content = std::fs::read_to_string(&self.file_path)?;
        let line_count = content.lines().count();
        // Subtract 1 for header if present
        Ok(Some(line_count.saturating_sub(1)))
    }

    async fn reset(&mut self) -> Result<()> {
        let file = std::fs::File::open(&self.file_path)?;
        self.reader = Some(ReaderBuilder::new()
            .has_headers(true)
            .from_reader(file));
        self.current_position = 0;
        self.has_more_data = true;
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.has_more_data && self.reader.is_some()
    }
}

pub struct CsvTarget {
    file_path: PathBuf,
    writer: Option<csv::Writer<std::fs::File>>,
    headers_written: bool,
    column_order: Vec<String>,
}

impl CsvTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            writer: None,
            headers_written: false,
            column_order: Vec::new(),
        })
    }
    
    fn value_to_string(&self, value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Decimal(d) => d.to_string(),
            Value::Boolean(b) => b.to_string(),
            Value::Date(dt) => dt.to_rfc3339(),
            Value::Null => String::new(),
        }
    }
}

#[async_trait]
impl Target for CsvTarget {
    async fn connect(&mut self) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::File::create(&self.file_path)?;
        self.writer = Some(WriterBuilder::new().from_writer(file));
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        if self.writer.is_none() {
            self.connect().await?;
        }

        // Store column order from schema
        self.column_order = schema.columns.iter().map(|c| c.name.clone()).collect();

        // Write headers
        if let Some(ref mut writer) = self.writer {
            writer.write_record(&self.column_order)?;
            self.headers_written = true;
        }

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.writer.is_none() {
            return Err(TinyEtlError::Connection("Writer not connected".to_string()));
        }

        let mut written_count = 0;

        // Helper function to convert Value to String
        let value_to_string = |value: &Value| -> String {
            match value {
                Value::String(s) => s.clone(),
                Value::Integer(i) => i.to_string(),
                Value::Decimal(d) => d.to_string(),
                Value::Boolean(b) => b.to_string(),
                Value::Date(dt) => dt.to_rfc3339(),
                Value::Null => String::new(),
            }
        };

        if let Some(ref mut writer) = self.writer {
            for row in rows {
                // Use the stored column order from the schema
                let record: Vec<String> = if !self.column_order.is_empty() {
                    // Use schema-defined column order
                    self.column_order.iter()
                        .map(|key| {
                            row.get(key)
                                .map(|v| value_to_string(v))
                                .unwrap_or_default()
                        })
                        .collect()
                } else {
                    // Fallback for when schema wasn't used (maintain old behavior)
                    let keys: Vec<_> = row.keys().cloned().collect();
                    keys.iter()
                        .map(|key| {
                            row.get(key)
                                .map(|v| value_to_string(v))
                                .unwrap_or_default()
                        })
                        .collect()
                };

                writer.write_record(&record)?;
                written_count += 1;
            }
        }

        Ok(written_count)
    }

    async fn finalize(&mut self) -> Result<()> {
        if let Some(ref mut writer) = self.writer {
            writer.flush()?;
        }
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[tokio::test]
    async fn test_csv_source_connection() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name,age").unwrap();
        writeln!(temp_file, "1,Alice,25").unwrap();
        writeln!(temp_file, "2,Bob,30").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_csv_source_schema_inference() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name,age,active").unwrap();
        writeln!(temp_file, "1,Alice,25,true").unwrap();
        writeln!(temp_file, "2,Bob,30,false").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        let schema = source.infer_schema(10).await.unwrap();
        assert_eq!(schema.columns.len(), 4);
        
        let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, crate::schema::DataType::Integer);
    }

    #[tokio::test]
    async fn test_csv_source_read_batch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap();
        writeln!(temp_file, "2,Bob").unwrap();
        writeln!(temp_file, "3,Charlie").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        let rows = source.read_batch(2).await.unwrap();
        assert_eq!(rows.len(), 2);
        
        if let Some(Value::String(name)) = rows[0].get("name") {
            assert_eq!(name, "Alice");
        } else {
            panic!("Expected string value for name");
        }
    }

    #[tokio::test]
    async fn test_csv_target_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let result = target.connect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_value_types() {
        // Test integer parsing
        assert!(matches!(CsvSource::parse_value("42"), Value::Integer(42)));
        
        // Test decimal parsing
        let decimal_result = CsvSource::parse_value("3.14");
        assert!(matches!(decimal_result, Value::Decimal(d) if d == Decimal::new(314, 2)));
        
        // Test boolean parsing
        assert!(matches!(CsvSource::parse_value("true"), Value::Boolean(true)));
        assert!(matches!(CsvSource::parse_value("false"), Value::Boolean(false)));
        
        // Test datetime parsing
        assert!(matches!(CsvSource::parse_value("2023-01-01T12:00:00Z"), Value::Date(_)));
        
        // Test empty string to null
        assert!(matches!(CsvSource::parse_value(""), Value::Null));
        
        // Test string fallback
        assert!(matches!(CsvSource::parse_value("hello"), Value::String(s) if s == "hello"));
    }

    #[tokio::test]
    async fn test_csv_source_file_not_found() {
        let mut source = CsvSource::new("/nonexistent/file.csv").unwrap();
        let result = source.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test] 
    async fn test_csv_source_estimated_row_count() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap(); 
        writeln!(temp_file, "2,Bob").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        let count = source.estimated_row_count().await.unwrap();
        assert_eq!(count, Some(2)); // 2 data rows (excluding header)
    }

    #[tokio::test]
    async fn test_csv_source_has_more() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        assert!(source.has_more());
    }

    #[tokio::test]
    async fn test_csv_target_write_batch_without_connection() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let row = std::collections::HashMap::new();
        let result = target.write_batch(&[row]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_csv_target_create_table_and_write() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        // Create schema
        let schema = crate::schema::Schema {
            columns: vec![
                crate::schema::Column {
                    name: "id".to_string(),
                    data_type: crate::schema::DataType::Integer,
                    nullable: false,
                },
                crate::schema::Column {
                    name: "name".to_string(),
                    data_type: crate::schema::DataType::String,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        target.create_table("test", &schema).await.unwrap();
        
        // Write some data
        let mut row = std::collections::HashMap::new();
        row.insert("id".to_string(), Value::Integer(1));
        row.insert("name".to_string(), Value::String("Alice".to_string()));
        
        let written = target.write_batch(&[row]).await.unwrap();
        assert_eq!(written, 1);
        
        target.finalize().await.unwrap();
    }

    #[tokio::test]
    async fn test_csv_target_value_to_string() {
        let target = CsvTarget::new("/tmp/test.csv").unwrap();
        
        assert_eq!(target.value_to_string(&Value::String("test".to_string())), "test");
        assert_eq!(target.value_to_string(&Value::Integer(42)), "42");
        assert_eq!(target.value_to_string(&Value::Decimal(Decimal::new(314, 2))), "3.14");
        assert_eq!(target.value_to_string(&Value::Boolean(true)), "true");
        assert_eq!(target.value_to_string(&Value::Null), "");
        
        let dt = chrono::Utc::now();
        assert!(target.value_to_string(&Value::Date(dt)).contains("T"));
    }

    #[tokio::test]
    async fn test_csv_target_exists() {
        let temp_file = NamedTempFile::new().unwrap();
        let target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let exists = target.exists("test").await.unwrap();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_csv_target_write_batch_with_all_value_types() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        target.connect().await.unwrap();
        
        let mut row = std::collections::HashMap::new();
        row.insert("string".to_string(), Value::String("test".to_string()));
        row.insert("integer".to_string(), Value::Integer(42));
        row.insert("decimal".to_string(), Value::Decimal(Decimal::new(314, 2)));
        row.insert("boolean".to_string(), Value::Boolean(true));
        row.insert("date".to_string(), Value::Date(chrono::Utc::now()));
        row.insert("null".to_string(), Value::Null);
        
        let written = target.write_batch(&[row]).await.unwrap();
        assert_eq!(written, 1);
    }

    #[tokio::test]
    async fn test_csv_column_order_debug() {
        // Create source CSV with specific header order
        let mut source_file = NamedTempFile::new().unwrap();
        writeln!(source_file, "one,two,three").unwrap();
        writeln!(source_file, "1,blah,bell").unwrap();

        // Set up source
        let mut source = CsvSource::new(source_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        // Debug: print headers from source
        println!("Source headers: {:?}", source.headers);
        
        // Infer schema
        let schema = source.infer_schema(10).await.unwrap();
        
        // Debug: print schema column order
        let schema_columns: Vec<_> = schema.columns.iter().map(|c| c.name.clone()).collect();
        println!("Schema columns: {:?}", schema_columns);
        
        // Reset source and read data
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();
        
        // Debug: print first row data
        if let Some(first_row) = rows.first() {
            println!("First row keys: {:?}", first_row.keys().collect::<Vec<_>>());
            for (key, value) in first_row {
                println!("  {}: {:?}", key, value);
            }
        }
        
        // Set up target
        let target_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();
        
        // Debug: print target column order
        println!("Target column order: {:?}", target.column_order);
        
        // Write data to target
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();
        
        // Read and print the output
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        println!("Output content:\n{}", output_content);
    }

    #[tokio::test]
    async fn test_csv_to_csv_full_roundtrip() {
        // Create source CSV with specific header order
        let mut source_file = NamedTempFile::new().unwrap();
        writeln!(source_file, "one,two,three").unwrap();
        writeln!(source_file, "1,blah,bell").unwrap();
        writeln!(source_file, "2,blo,aff").unwrap();

        // Set up source
        let mut source = CsvSource::new(source_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        // Infer schema
        let schema = source.infer_schema(10).await.unwrap();
        
        // Verify schema preserves original column order
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.columns[0].name, "one");
        assert_eq!(schema.columns[1].name, "two");
        assert_eq!(schema.columns[2].name, "three");
        
        // Set up target
        let target_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();
        
        // Reset source and read all data
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();
        
        // Write data to target
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();
        
        // Read the output file and verify header order is preserved
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        let lines: Vec<&str> = output_content.lines().collect();
        
        // Check that headers are in the correct order (same as input)
        let header_line = lines[0];
        assert_eq!(header_line, "one,two,three", "Headers should maintain original CSV order");
        
        // Verify data integrity and order
        assert_eq!(lines.len(), 3); // header + 2 data rows
        assert_eq!(lines[1], "1,blah,bell", "First data row should maintain column order");
        assert_eq!(lines[2], "2,blo,aff", "Second data row should maintain column order");
    }
}
