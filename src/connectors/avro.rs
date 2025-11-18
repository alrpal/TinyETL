use std::path::PathBuf;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::collections::HashMap;
use async_trait::async_trait;
use apache_avro::{
    Reader, Writer, Schema as AvroSchema, from_value, to_value,
    types::Value as AvroValue, to_avro_datum,
};
use serde_json::{Value as JsonValue, json};
use rust_decimal::Decimal;

use crate::{
    Result, TinyEtlError,
    schema::{Schema, Row, Value, Column, DataType, SchemaInferer},
    connectors::{Source, Target},
};

pub struct AvroSource {
    file_path: PathBuf,
    current_position: usize,
    total_records: Option<usize>,
    has_more: bool,
}

impl AvroSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            current_position: 0,
            total_records: None,
            has_more: true,
        })
    }

    fn avro_type_to_schema_type(avro_type: &JsonValue) -> DataType {
        match avro_type {
            JsonValue::String(type_name) => match type_name.as_str() {
                "string" => DataType::String,
                "int" | "long" => DataType::Integer,
                "float" | "double" => DataType::Decimal,
                "boolean" => DataType::Boolean,
                "date" => DataType::Date,
                "timestamp-millis" | "timestamp-micros" => DataType::DateTime,
                _ => DataType::String, // Default to string
            },
            JsonValue::Array(union_types) => {
                // Handle union types (e.g., ["null", "string"])
                for union_type in union_types {
                    if let JsonValue::String(type_str) = union_type {
                        if type_str != "null" {
                            return Self::avro_type_to_schema_type(union_type);
                        }
                    }
                }
                DataType::String // Default
            },
            JsonValue::Object(obj) => {
                if let Some(logical_type) = obj.get("logicalType") {
                    match logical_type.as_str().unwrap_or("") {
                        "date" => DataType::Date,
                        "timestamp-millis" | "timestamp-micros" => DataType::DateTime,
                        _ => DataType::String,
                    }
                } else if let Some(type_val) = obj.get("type") {
                    Self::avro_type_to_schema_type(type_val)
                } else {
                    DataType::String
                }
            },
            _ => DataType::String,
        }
    }

    fn is_nullable(avro_type: &JsonValue) -> bool {
        match avro_type {
            JsonValue::Array(union_types) => {
                union_types.iter().any(|t| {
                    if let JsonValue::String(type_str) = t {
                        type_str == "null"
                    } else {
                        false
                    }
                })
            },
            _ => false,
        }
    }

    fn avro_value_to_value(avro_value: &AvroValue) -> Result<Value> {
        match avro_value {
            AvroValue::Null => Ok(Value::Null),
            AvroValue::Boolean(b) => Ok(Value::Boolean(*b)),
            AvroValue::Int(i) => Ok(Value::Integer(*i as i64)),
            AvroValue::Long(l) => Ok(Value::Integer(*l)),
            AvroValue::Float(f) => {
                // Convert f32 to Decimal
                match Decimal::try_from(*f as f64) {
                    Ok(d) => Ok(Value::Decimal(d)),
                    Err(_) => Ok(Value::String(f.to_string())),
                }
            },
            AvroValue::Double(d) => {
                // Convert f64 to Decimal
                match Decimal::try_from(*d) {
                    Ok(decimal) => Ok(Value::Decimal(decimal)),
                    Err(_) => Ok(Value::String(d.to_string())),
                }
            },
            AvroValue::Bytes(b) => Ok(Value::String(format!("{:?}", b))),
            AvroValue::String(s) => Ok(Value::String(s.clone())),
            AvroValue::Fixed(_, bytes) => Ok(Value::String(format!("{:?}", bytes))),
            AvroValue::Enum(_, symbol) => Ok(Value::String(symbol.clone())),
            AvroValue::Union(_, boxed_value) => Self::avro_value_to_value(boxed_value),
            AvroValue::Array(values) => {
                // Convert array to JSON string for now
                let json_values: std::result::Result<Vec<JsonValue>, _> = values.iter()
                    .map(|v| Self::avro_value_to_json_value(v))
                    .collect();
                match json_values {
                    Ok(vals) => Ok(Value::String(serde_json::to_string(&vals).unwrap_or_default())),
                    Err(_) => Ok(Value::String("[]".to_string())),
                }
            },
            AvroValue::Map(map) => {
                // Convert map to JSON string
                let mut json_map = serde_json::Map::new();
                for (k, v) in map {
                    if let Ok(json_val) = Self::avro_value_to_json_value(v) {
                        json_map.insert(k.clone(), json_val);
                    }
                }
                Ok(Value::String(serde_json::to_string(&json_map).unwrap_or_default()))
            },
            AvroValue::Record(fields) => {
                // Convert record to JSON string
                let mut json_map = serde_json::Map::new();
                for (field_name, field_value) in fields {
                    if let Ok(json_val) = Self::avro_value_to_json_value(field_value) {
                        json_map.insert(field_name.clone(), json_val);
                    }
                }
                Ok(Value::String(serde_json::to_string(&json_map).unwrap_or_default()))
            },
            AvroValue::Date(days) => {
                // Convert days since epoch to date
                let base_date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid base date".to_string()))?;
                let target_date = base_date + chrono::Duration::days(*days as i64);
                let datetime = target_date.and_hms_opt(0, 0, 0)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid time conversion".to_string()))?
                    .and_utc();
                Ok(Value::Date(datetime))
            },
            AvroValue::TimeMillis(millis) => {
                // Time in milliseconds since midnight
                Ok(Value::String(format!("{}ms", millis)))
            },
            AvroValue::TimeMicros(micros) => {
                // Time in microseconds since midnight
                Ok(Value::String(format!("{}μs", micros)))
            },
            AvroValue::TimestampMillis(millis) => {
                // Timestamp in milliseconds since epoch
                let datetime = chrono::DateTime::from_timestamp(*millis / 1000, ((*millis % 1000) * 1_000_000) as u32)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid timestamp".to_string()))?;
                Ok(Value::Date(datetime))
            },
            AvroValue::TimestampMicros(micros) => {
                // Timestamp in microseconds since epoch
                let datetime = chrono::DateTime::from_timestamp(*micros / 1_000_000, ((*micros % 1_000_000) * 1000) as u32)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid timestamp".to_string()))?;
                Ok(Value::Date(datetime))
            },
            AvroValue::LocalTimestampMillis(millis) => {
                // Local timestamp in milliseconds since epoch
                let datetime = chrono::DateTime::from_timestamp(*millis / 1000, ((*millis % 1000) * 1_000_000) as u32)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid local timestamp".to_string()))?;
                Ok(Value::Date(datetime))
            },
            AvroValue::LocalTimestampMicros(micros) => {
                // Local timestamp in microseconds since epoch
                let datetime = chrono::DateTime::from_timestamp(*micros / 1_000_000, ((*micros % 1_000_000) * 1000) as u32)
                    .ok_or_else(|| TinyEtlError::DataTransfer("Invalid local timestamp".to_string()))?;
                Ok(Value::Date(datetime))
            },
            AvroValue::Decimal(decimal) => {
                // Convert decimal to string representation
                Ok(Value::String(format!("{:?}", decimal)))
            },
            AvroValue::Uuid(uuid) => {
                Ok(Value::String(uuid.to_string()))
            },
            AvroValue::Duration(duration) => {
                // Convert duration to string representation
                Ok(Value::String(format!("{:?}", duration)))
            },
        }
    }

    fn avro_value_to_json_value(avro_value: &AvroValue) -> Result<JsonValue> {
        match avro_value {
            AvroValue::Null => Ok(JsonValue::Null),
            AvroValue::Boolean(b) => Ok(JsonValue::Bool(*b)),
            AvroValue::Int(i) => Ok(JsonValue::Number(serde_json::Number::from(*i))),
            AvroValue::Long(l) => Ok(JsonValue::Number(serde_json::Number::from(*l))),
            AvroValue::Float(f) => Ok(JsonValue::Number(
                serde_json::Number::from_f64(*f as f64).unwrap_or(serde_json::Number::from(0))
            )),
            AvroValue::Double(d) => Ok(JsonValue::Number(
                serde_json::Number::from_f64(*d).unwrap_or(serde_json::Number::from(0))
            )),
            AvroValue::String(s) => Ok(JsonValue::String(s.clone())),
            AvroValue::Union(_, boxed_value) => Self::avro_value_to_json_value(boxed_value),
            _ => Ok(JsonValue::String(format!("{:?}", avro_value))),
        }
    }
}

#[async_trait]
impl Source for AvroSource {
    async fn connect(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(
                format!("Avro file not found: {}", self.file_path.display())
            ));
        }
        
        // Just verify the file can be opened and is a valid Avro file
        let file = File::open(&self.file_path)?;
        let buf_reader = BufReader::new(file);
        
        let _reader = Reader::new(buf_reader)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Invalid Avro file: {}", e)))?;
        
        self.current_position = 0;
        self.has_more = true;
        
        Ok(())
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(
                format!("Avro file not found: {}", self.file_path.display())
            ));
        }

        // Create a new reader for schema inference
        let file = File::open(&self.file_path)?;
        let buf_reader = BufReader::new(file);
        
        let reader = Reader::new(buf_reader)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create Avro reader: {}", e)))?;

        let avro_schema = reader.writer_schema();
        
        // Parse the Avro schema to extract field information
        let schema_json: JsonValue = serde_json::from_str(&avro_schema.canonical_form())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to parse Avro schema: {}", e)))?;

        let mut columns = Vec::new();
        
        if let JsonValue::Object(obj) = &schema_json {
            if let Some(JsonValue::Array(fields)) = obj.get("fields") {
                for field in fields {
                    if let JsonValue::Object(field_obj) = field {
                        let name = field_obj.get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        
                        let default_type = JsonValue::String("string".to_string());
                        let field_type = field_obj.get("type").unwrap_or(&default_type);
                        let data_type = Self::avro_type_to_schema_type(field_type);
                        let nullable = Self::is_nullable(field_type);
                        
                        columns.push(Column {
                            name,
                            data_type,
                            nullable,
                        });
                    }
                }
            }
        }

        Ok(Schema {
            columns,
            estimated_rows: self.total_records,
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if !self.has_more {
            return Ok(vec![]);
        }

        // Create a new reader each time (not ideal for performance, but works around lifetime issues)
        let file = File::open(&self.file_path)?;
        let buf_reader = BufReader::new(file);
        
        let reader = Reader::new(buf_reader)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create Avro reader: {}", e)))?;

        let mut rows = Vec::new();
        let mut count = 0;
        let mut current_index = 0;
        let mut reached_end = false;

        for value_result in reader {
            // Skip records we've already read
            if current_index < self.current_position {
                current_index += 1;
                continue;
            }

            if count >= batch_size {
                break;
            }

            match value_result {
                Ok(avro_value) => {
                    if let AvroValue::Record(fields) = avro_value {
                        let mut row = HashMap::new();
                        for (field_name, field_value) in fields {
                            let converted_value = Self::avro_value_to_value(&field_value)?;
                            row.insert(field_name, converted_value);
                        }
                        rows.push(row);
                        count += 1;
                        self.current_position += 1;
                        current_index += 1;
                    }
                }
                Err(e) => {
                    return Err(TinyEtlError::DataTransfer(format!("Failed to read Avro record: {}", e)));
                }
            }
        }

        // Check if we've reached the end by seeing if we can read more
        if count < batch_size {
            reached_end = true;
        }

        if rows.is_empty() || reached_end {
            self.has_more = false;
        }

        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(self.total_records)
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_position = 0;
        self.has_more = true;
        self.connect().await
    }

    fn has_more(&self) -> bool {
        self.has_more
    }
}

pub struct AvroTarget {
    file_path: PathBuf,
    schema: Option<AvroSchema>,
    buffer: Vec<Row>,
}

impl AvroTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            schema: None,
            buffer: Vec::new(),
        })
    }

    fn schema_to_avro_schema(schema: &Schema) -> Result<AvroSchema> {
        let mut fields = Vec::new();
        
        for column in &schema.columns {
            let field_type = match column.data_type {
                DataType::String => {
                    if column.nullable {
                        json!(["null", "string"])
                    } else {
                        json!("string")
                    }
                },
                DataType::Integer => {
                    if column.nullable {
                        json!(["null", "long"])
                    } else {
                        json!("long")
                    }
                },
                DataType::Decimal => {
                    if column.nullable {
                        json!(["null", "double"])
                    } else {
                        json!("double")
                    }
                },
                DataType::Boolean => {
                    if column.nullable {
                        json!(["null", "boolean"])
                    } else {
                        json!("boolean")
                    }
                },
                DataType::Date => {
                    if column.nullable {
                        json!(["null", {"type": "int", "logicalType": "date"}])
                    } else {
                        json!({"type": "int", "logicalType": "date"})
                    }
                },
                DataType::DateTime => {
                    if column.nullable {
                        json!(["null", {"type": "long", "logicalType": "timestamp-millis"}])
                    } else {
                        json!({"type": "long", "logicalType": "timestamp-millis"})
                    }
                },
                DataType::Json => {
                    // Avro stores JSON as string
                    if column.nullable {
                        json!(["null", "string"])
                    } else {
                        json!("string")
                    }
                },
                DataType::Null => json!(["null", "string"]),
            };

            fields.push(json!({
                "name": column.name,
                "type": field_type
            }));
        }

        let avro_schema_json = json!({
            "type": "record",
            "name": "Record",
            "fields": fields
        });

        AvroSchema::parse(&avro_schema_json)
            .map_err(|e| TinyEtlError::Configuration(format!("Failed to create Avro schema: {}", e)))
    }

    fn value_to_avro_value(value: &Value, data_type: &DataType, nullable: bool) -> Result<AvroValue> {
        // Handle null values - always wrap in Union for nullable fields
        if matches!(value, Value::Null) {
            if nullable {
                return Ok(AvroValue::Union(0, Box::new(AvroValue::Null)));
            } else {
                return Err(TinyEtlError::DataTransfer(
                    "Cannot insert null value into non-nullable field".to_string()
                ));
            }
        }

        // Convert the value to the appropriate Avro type
        let avro_value = match (value, data_type) {
            (Value::String(s), DataType::String) => AvroValue::String(s.clone()),
            (Value::Integer(i), DataType::Integer) => AvroValue::Long(*i),
            (Value::Decimal(d), DataType::Decimal) => {
                // Convert Decimal to f64 for Avro
                let f: f64 = (*d).try_into().unwrap_or(0.0);
                AvroValue::Double(f)
            },
            (Value::Boolean(b), DataType::Boolean) => AvroValue::Boolean(*b),
            (Value::Date(dt), DataType::Date) => {
                let days_since_epoch = (dt.timestamp() / 86400) as i32;
                AvroValue::Date(days_since_epoch)
            },
            (Value::Date(dt), DataType::DateTime) => {
                AvroValue::TimestampMillis(dt.timestamp_millis())
            },
            // Type conversion fallbacks
            (Value::String(s), DataType::Integer) => {
                let parsed = s.parse::<i64>()
                    .map_err(|_| TinyEtlError::DataTransfer(format!("Cannot convert '{}' to integer", s)))?;
                AvroValue::Long(parsed)
            },
            (Value::String(s), DataType::Decimal) => {
                let parsed = s.parse::<Decimal>()
                    .map_err(|_| TinyEtlError::DataTransfer(format!("Cannot convert '{}' to decimal", s)))?;
                let f: f64 = parsed.try_into().unwrap_or(0.0);
                AvroValue::Double(f)
            },
            (Value::String(s), DataType::Boolean) => {
                let parsed = s.parse::<bool>()
                    .map_err(|_| TinyEtlError::DataTransfer(format!("Cannot convert '{}' to boolean", s)))?;
                AvroValue::Boolean(parsed)
            },
            (Value::Integer(i), DataType::String) => AvroValue::String(i.to_string()),
            (Value::Decimal(d), DataType::String) => AvroValue::String(d.to_string()),
            (Value::Boolean(b), DataType::String) => AvroValue::String(b.to_string()),
            _ => return Err(TinyEtlError::DataTransfer(format!(
                "Cannot convert value {:?} to Avro type {:?}", value, data_type
            ))),
        };

        // For nullable fields, wrap non-null values in a Union with index 1 (the non-null variant)
        if nullable {
            Ok(AvroValue::Union(1, Box::new(avro_value)))
        } else {
            Ok(avro_value)
        }
    }
}

#[async_trait]
impl Target for AvroTarget {
    async fn connect(&mut self) -> Result<()> {
        // Connection will be established when create_table is called
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        let avro_schema = Self::schema_to_avro_schema(schema)?;
        self.schema = Some(avro_schema);
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.schema.is_none() {
            return Err(TinyEtlError::Configuration("Schema not initialized. Call create_table() first.".to_string()));
        }

        // Buffer the rows for writing during finalization
        self.buffer.extend_from_slice(rows);
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        if self.schema.is_none() {
            return Ok(());
        }

        let schema = self.schema.as_ref().unwrap();
        
        // Create writer and write all buffered data
        let file = File::create(&self.file_path)?;
        let buf_writer = BufWriter::new(file);
        
        let mut writer = Writer::new(schema, buf_writer);

        for row in &self.buffer {
            let mut record_fields = Vec::new();
            
            // Extract fields from schema to maintain order
            let schema_json: JsonValue = serde_json::from_str(&schema.canonical_form())
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to parse schema: {}", e)))?;
            
            if let JsonValue::Object(obj) = &schema_json {
                if let Some(JsonValue::Array(fields)) = obj.get("fields") {
                    for field in fields {
                        if let JsonValue::Object(field_obj) = field {
                            let field_name = field_obj.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            
                            let default_type = JsonValue::String("string".to_string());
                            let field_type = field_obj.get("type").unwrap_or(&default_type);
                            let data_type = AvroSource::avro_type_to_schema_type(field_type);
                            let nullable = AvroSource::is_nullable(field_type);
                            
                            let value = row.get(field_name).unwrap_or(&Value::Null);
                            let avro_value = Self::value_to_avro_value(value, &data_type, nullable)?;
                            
                            record_fields.push((field_name.to_string(), avro_value));
                        }
                    }
                }
            }

            let record = AvroValue::Record(record_fields);
            writer.append(record)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to write Avro record: {}", e)))?;
        }

        writer.flush()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to flush Avro writer: {}", e)))?;
            
        self.buffer.clear();
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // For Avro files, truncation means clearing buffer
        self.buffer.clear();
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // Avro files don't easily support append - would require reading existing file and merging
        // For simplicity, we return false to force truncation
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;
    use chrono::{DateTime, Utc};
    
    fn create_test_avro_file() -> Result<NamedTempFile> {
        let temp_file = NamedTempFile::new()?;
        
        // Create a simple schema
        let schema_json = json!({
            "type": "record",
            "name": "TestRecord",
            "fields": [
                {"name": "id", "type": "long"},
                {"name": "name", "type": "string"},
                {"name": "email", "type": ["null", "string"]},
                {"name": "age", "type": "int"},
                {"name": "salary", "type": "double"},
                {"name": "active", "type": "boolean"},
                {"name": "created_at", "type": {"type": "long", "logicalType": "timestamp-millis"}}
            ]
        });
        
        let schema = AvroSchema::parse(&schema_json)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create test schema: {}", e)))?;
        let file = temp_file.reopen()?;
        let mut writer = Writer::new(&schema, file);
        
        // Write test records
        let test_records = vec![
            AvroValue::Record(vec![
                ("id".to_string(), AvroValue::Long(1)),
                ("name".to_string(), AvroValue::String("John Doe".to_string())),
                ("email".to_string(), AvroValue::Union(1, Box::new(AvroValue::String("john@example.com".to_string())))),
                ("age".to_string(), AvroValue::Int(30)),
                ("salary".to_string(), AvroValue::Double(50000.0)),
                ("active".to_string(), AvroValue::Boolean(true)),
                ("created_at".to_string(), AvroValue::TimestampMillis(1609459200000)), // 2021-01-01
            ]),
            AvroValue::Record(vec![
                ("id".to_string(), AvroValue::Long(2)),
                ("name".to_string(), AvroValue::String("Jane Smith".to_string())),
                ("email".to_string(), AvroValue::Union(0, Box::new(AvroValue::Null))),
                ("age".to_string(), AvroValue::Int(25)),
                ("salary".to_string(), AvroValue::Double(60000.0)),
                ("active".to_string(), AvroValue::Boolean(false)),
                ("created_at".to_string(), AvroValue::TimestampMillis(1609545600000)), // 2021-01-02
            ]),
        ];
        
        for record in test_records {
            writer.append(record)
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to write test record: {}", e)))?;
        }
        
        writer.flush()
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to flush test writer: {}", e)))?;
        Ok(temp_file)
    }
    
    #[tokio::test]
    async fn test_avro_source_new() {
        let source = AvroSource::new("test.avro");
        assert!(source.is_ok());
        
        let source = source.unwrap();
        assert_eq!(source.file_path.to_string_lossy(), "test.avro");
        assert_eq!(source.current_position, 0);
        assert_eq!(source.total_records, None);
        assert!(source.has_more);
    }
    
    #[tokio::test]
    async fn test_avro_source_connect_file_not_found() {
        let mut source = AvroSource::new("nonexistent.avro").unwrap();
        let result = source.connect().await;
        
        assert!(result.is_err());
        if let Err(TinyEtlError::Connection(msg)) = result {
            assert!(msg.contains("Avro file not found"));
        } else {
            panic!("Expected Connection error");
        }
    }
    
    #[tokio::test]
    async fn test_avro_source_connect_success() {
        let temp_file = create_test_avro_file().unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let result = source.connect().await;
        assert!(result.is_ok());
        assert_eq!(source.current_position, 0);
        assert!(source.has_more);
    }
    
    #[tokio::test]
    async fn test_avro_source_infer_schema() {
        let temp_file = create_test_avro_file().unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let schema = source.infer_schema(100).await.unwrap();
        
        assert_eq!(schema.columns.len(), 7);
        
        // Check specific columns
        let id_col = &schema.columns[0];
        assert_eq!(id_col.name, "id");
        assert_eq!(id_col.data_type, DataType::Integer);
        assert!(!id_col.nullable);
        
        let name_col = &schema.columns[1];
        assert_eq!(name_col.name, "name");
        assert_eq!(name_col.data_type, DataType::String);
        
        let email_col = &schema.columns[2];
        assert_eq!(email_col.name, "email");
        assert_eq!(email_col.data_type, DataType::String);
        assert!(email_col.nullable);
    }
    
    #[tokio::test]
    async fn test_avro_source_read_batch() {
        let temp_file = create_test_avro_file().unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        let rows = source.read_batch(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        
        // Check first row
        let first_row = &rows[0];
        assert_eq!(first_row.get("id"), Some(&Value::Integer(1)));
        assert_eq!(first_row.get("name"), Some(&Value::String("John Doe".to_string())));
        assert_eq!(first_row.get("age"), Some(&Value::Integer(30)));
        assert_eq!(first_row.get("salary"), Some(&Value::Decimal(Decimal::new(50000, 0))));
        assert_eq!(first_row.get("active"), Some(&Value::Boolean(true)));
        
        // Check second row
        let second_row = &rows[1];
        assert_eq!(second_row.get("id"), Some(&Value::Integer(2)));
        assert_eq!(second_row.get("name"), Some(&Value::String("Jane Smith".to_string())));
        assert_eq!(second_row.get("email"), Some(&Value::Null));
    }
    
    #[tokio::test]
    async fn test_avro_source_read_batch_pagination() {
        let temp_file = create_test_avro_file().unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        // Read first batch with size 1
        let rows1 = source.read_batch(1).await.unwrap();
        assert_eq!(rows1.len(), 1);
        assert_eq!(rows1[0].get("id"), Some(&Value::Integer(1)));
        
        // Read second batch
        let rows2 = source.read_batch(1).await.unwrap();
        assert_eq!(rows2.len(), 1);
        assert_eq!(rows2[0].get("id"), Some(&Value::Integer(2)));
        
        // Try to read more - should be empty
        let rows3 = source.read_batch(1).await.unwrap();
        assert!(rows3.is_empty());
        assert!(!source.has_more());
    }
    
    #[tokio::test]
    async fn test_avro_source_reset() {
        let temp_file = create_test_avro_file().unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        // Read all data
        source.read_batch(10).await.unwrap();
        assert!(!source.has_more());
        
        // Reset and read again
        source.reset().await.unwrap();
        assert!(source.has_more());
        assert_eq!(source.current_position, 0);
        
        let rows = source.read_batch(10).await.unwrap();
        assert_eq!(rows.len(), 2);
    }
    
    #[tokio::test]
    async fn test_avro_type_to_schema_type() {
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("string")), DataType::String);
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("int")), DataType::Integer);
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("long")), DataType::Integer);
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("float")), DataType::Decimal);
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("double")), DataType::Decimal);
        assert_eq!(AvroSource::avro_type_to_schema_type(&json!("boolean")), DataType::Boolean);
        
        // Test logical types
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!({"type": "int", "logicalType": "date"})),
            DataType::Date
        );
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!({"type": "long", "logicalType": "timestamp-millis"})),
            DataType::DateTime
        );
        
        // Test union types
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!(["null", "string"])),
            DataType::String
        );
    }
    
    #[tokio::test]
    async fn test_is_nullable() {
        assert!(!AvroSource::is_nullable(&json!("string")));
        assert!(AvroSource::is_nullable(&json!(["null", "string"])));
        assert!(AvroSource::is_nullable(&json!(["string", "null"])));
        assert!(!AvroSource::is_nullable(&json!(["string", "int"])));
    }
    
    #[tokio::test]
    async fn test_avro_value_to_value() {
        // Test basic types
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::Null).unwrap(),
            Value::Null
        );
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::Boolean(true)).unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::Int(42)).unwrap(),
            Value::Integer(42)
        );
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::Long(100)).unwrap(),
            Value::Integer(100)
        );
        // Test f32 to decimal conversion (with precision limitations)
        let float_result = AvroSource::avro_value_to_value(&AvroValue::Float(3.14)).unwrap();
        if let Value::Decimal(d) = float_result {
            // Just check that it's approximately 3.14 (f32 precision limitations)
            let f: f64 = d.try_into().unwrap();
            assert!((f - 3.14).abs() < 0.01, "Expected ~3.14, got {}", f);
        } else {
            panic!("Expected Decimal value");
        }
        
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::Double(2.718)).unwrap(),
            Value::Decimal(Decimal::new(2718, 3))
        );
        assert_eq!(
            AvroSource::avro_value_to_value(&AvroValue::String("test".to_string())).unwrap(),
            Value::String("test".to_string())
        );
        
        // Test date/time types
        let date_result = AvroSource::avro_value_to_value(&AvroValue::Date(0)).unwrap();
        if let Value::Date(_) = date_result {
            // Success - date conversion worked
        } else {
            panic!("Expected Date value");
        }
        
        let timestamp_result = AvroSource::avro_value_to_value(&AvroValue::TimestampMillis(1609459200000)).unwrap();
        if let Value::Date(_) = timestamp_result {
            // Success - timestamp conversion worked
        } else {
            panic!("Expected Date value");
        }
        
        // Test union
        let union_value = AvroValue::Union(1, Box::new(AvroValue::String("test".to_string())));
        assert_eq!(
            AvroSource::avro_value_to_value(&union_value).unwrap(),
            Value::String("test".to_string())
        );
    }
    
    #[tokio::test]
    async fn test_avro_target_new() {
        let target = AvroTarget::new("output.avro");
        assert!(target.is_ok());
        
        let target = target.unwrap();
        assert_eq!(target.file_path.to_string_lossy(), "output.avro");
        assert!(target.schema.is_none());
        assert!(target.buffer.is_empty());
    }
    
    #[tokio::test]
    async fn test_avro_target_connect() {
        let mut target = AvroTarget::new("output.avro").unwrap();
        let result = target.connect().await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_avro_target_create_table() {
        let mut target = AvroTarget::new("output.avro").unwrap();
        
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
                    name: "active".to_string(),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
                Column {
                    name: "score".to_string(),
                    data_type: DataType::Decimal,
                    nullable: true,
                },
                Column {
                    name: "created".to_string(),
                    data_type: DataType::DateTime,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        
        let result = target.create_table("test_table", &schema).await;
        assert!(result.is_ok());
        assert!(target.schema.is_some());
    }
    
    #[tokio::test]
    async fn test_schema_to_avro_schema() {
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
                    name: "active".to_string(),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        
        let avro_schema = AvroTarget::schema_to_avro_schema(&schema);
        assert!(avro_schema.is_ok());
        
        let schema_json: JsonValue = serde_json::from_str(&avro_schema.unwrap().canonical_form()).unwrap();
        
        if let JsonValue::Object(obj) = schema_json {
            if let Some(JsonValue::Array(fields)) = obj.get("fields") {
                assert_eq!(fields.len(), 3);
                
                // Check field types
                let id_field = &fields[0];
                if let JsonValue::Object(field_obj) = id_field {
                    assert_eq!(field_obj.get("name").unwrap(), "id");
                    assert_eq!(field_obj.get("type").unwrap(), "long");
                }
                
                let name_field = &fields[1];
                if let JsonValue::Object(field_obj) = name_field {
                    assert_eq!(field_obj.get("name").unwrap(), "name");
                    assert_eq!(field_obj.get("type").unwrap(), &json!(["null", "string"]));
                }
            }
        }
    }
    
    #[tokio::test]
    async fn test_value_to_avro_value() {
        // Test basic conversions with nullable=true
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Null, &DataType::String, true).unwrap(),
            AvroValue::Union(0, Box::new(AvroValue::Null))
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::String("test".to_string()), &DataType::String, true).unwrap(),
            AvroValue::Union(1, Box::new(AvroValue::String("test".to_string())))
        );
        
        // Test basic conversions with nullable=false
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::String("test".to_string()), &DataType::String, false).unwrap(),
            AvroValue::String("test".to_string())
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Integer(42), &DataType::Integer, false).unwrap(),
            AvroValue::Long(42)
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Decimal(Decimal::new(314, 2)), &DataType::Decimal, false).unwrap(),
            AvroValue::Double(3.14)
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::Boolean, false).unwrap(),
            AvroValue::Boolean(true)
        );
        
        // Test type conversions
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::String("42".to_string()), &DataType::Integer, false).unwrap(),
            AvroValue::Long(42)
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::String("3.14".to_string()), &DataType::Decimal, false).unwrap(),
            AvroValue::Double(3.14)
        );
        
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::String("true".to_string()), &DataType::Boolean, false).unwrap(),
            AvroValue::Boolean(true)
        );
        
        // Test date/time
        let datetime = DateTime::from_timestamp(1609459200, 0).unwrap();
        let date_value = Value::Date(datetime);
        
        let result = AvroTarget::value_to_avro_value(&date_value, &DataType::Date, false).unwrap();
        if let AvroValue::Date(_) = result {
            // Success
        } else {
            panic!("Expected Date value");
        }
        
        let result = AvroTarget::value_to_avro_value(&date_value, &DataType::DateTime, false).unwrap();
        if let AvroValue::TimestampMillis(_) = result {
            // Success
        } else {
            panic!("Expected TimestampMillis value");
        }
    }
    
    #[tokio::test]
    async fn test_value_to_avro_value_invalid_conversions() {
        // Test invalid string to int conversion
        let result = AvroTarget::value_to_avro_value(&Value::String("not_a_number".to_string()), &DataType::Integer, false);
        assert!(result.is_err());
        
        // Test invalid string to float conversion
        let result = AvroTarget::value_to_avro_value(&Value::String("not_a_float".to_string()), &DataType::Decimal, false);
        assert!(result.is_err());
        
        // Test invalid string to bool conversion
        let result = AvroTarget::value_to_avro_value(&Value::String("not_a_bool".to_string()), &DataType::Boolean, false);
        assert!(result.is_err());
        
        // Test null value in non-nullable field
        let result = AvroTarget::value_to_avro_value(&Value::Null, &DataType::String, false);
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_avro_target_write_batch_without_schema() {
        let mut target = AvroTarget::new("output.avro").unwrap();
        
        let row = std::collections::HashMap::from([
            ("id".to_string(), Value::Integer(1)),
            ("name".to_string(), Value::String("test".to_string())),
        ]);
        
        let result = target.write_batch(&[row]).await;
        assert!(result.is_err());
        
        if let Err(TinyEtlError::Configuration(msg)) = result {
            assert!(msg.contains("Schema not initialized"));
        } else {
            panic!("Expected Configuration error");
        }
    }
    
    #[tokio::test]
    async fn test_avro_target_full_workflow() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = AvroTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        // Create schema
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
                    nullable: false,
                },
                Column {
                    name: "active".to_string(),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        
        // Connect and create table
        target.connect().await.unwrap();
        target.create_table("test_table", &schema).await.unwrap();
        
        // Write some data
        let rows = vec![
            std::collections::HashMap::from([
                ("id".to_string(), Value::Integer(1)),
                ("name".to_string(), Value::String("John".to_string())),
                ("active".to_string(), Value::Boolean(true)),
            ]),
            std::collections::HashMap::from([
                ("id".to_string(), Value::Integer(2)),
                ("name".to_string(), Value::String("Jane".to_string())),
                ("active".to_string(), Value::Boolean(false)),
            ]),
        ];
        
        let write_result = target.write_batch(&rows).await.unwrap();
        assert_eq!(write_result, 2);
        
        // Finalize
        target.finalize().await.unwrap();
        
        // Verify file was created and can be read
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        
        let read_rows = source.read_batch(10).await.unwrap();
        assert_eq!(read_rows.len(), 2);
        
        assert_eq!(read_rows[0].get("id"), Some(&Value::Integer(1)));
        assert_eq!(read_rows[0].get("name"), Some(&Value::String("John".to_string())));
        assert_eq!(read_rows[0].get("active"), Some(&Value::Boolean(true)));
        
        assert_eq!(read_rows[1].get("id"), Some(&Value::Integer(2)));
        assert_eq!(read_rows[1].get("name"), Some(&Value::String("Jane".to_string())));
        assert_eq!(read_rows[1].get("active"), Some(&Value::Boolean(false)));
    }
    
    #[tokio::test]
    async fn test_avro_target_exists() {
        let temp_file = NamedTempFile::new().unwrap();
        let target = AvroTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        assert!(target.exists("test_table").await.unwrap());
        
        let target_nonexistent = AvroTarget::new("nonexistent.avro").unwrap();
        assert!(!target_nonexistent.exists("test_table").await.unwrap());
    }
    
    #[tokio::test]
    async fn test_avro_source_invalid_file_content() {
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(temp_file.path(), "invalid avro content").unwrap();
        
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        
        assert!(result.is_err());
        if let Err(TinyEtlError::DataTransfer(msg)) = result {
            assert!(msg.contains("Invalid Avro file"));
        } else {
            panic!("Expected DataTransfer error");
        }
    }
    
    #[test]
    fn test_avro_value_to_json_value() {
        // Test basic types
        assert_eq!(
            AvroSource::avro_value_to_json_value(&AvroValue::Null).unwrap(),
            JsonValue::Null
        );
        
        assert_eq!(
            AvroSource::avro_value_to_json_value(&AvroValue::Boolean(true)).unwrap(),
            JsonValue::Bool(true)
        );
        
        assert_eq!(
            AvroSource::avro_value_to_json_value(&AvroValue::Int(42)).unwrap(),
            JsonValue::Number(serde_json::Number::from(42))
        );
        
        assert_eq!(
            AvroSource::avro_value_to_json_value(&AvroValue::String("test".to_string())).unwrap(),
            JsonValue::String("test".to_string())
        );
        
        // Test union
        let union_value = AvroValue::Union(1, Box::new(AvroValue::String("test".to_string())));
        assert_eq!(
            AvroSource::avro_value_to_json_value(&union_value).unwrap(),
            JsonValue::String("test".to_string())
        );
    }
    
    #[test]
    fn test_avro_value_conversions_comprehensive() {
        // Test Bytes
        let bytes_value = AvroValue::Bytes(vec![1, 2, 3]);
        let result = AvroSource::avro_value_to_value(&bytes_value).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("1"));
        } else {
            panic!("Expected String value");
        }
        
        // Test Fixed
        let fixed_value = AvroValue::Fixed(4, vec![1, 2, 3, 4]);
        let result = AvroSource::avro_value_to_value(&fixed_value).unwrap();
        if let Value::String(_) = result {
            // Success
        } else {
            panic!("Expected String value");
        }
        
        // Test Enum
        let enum_value = AvroValue::Enum(0, "OPTION_A".to_string());
        let result = AvroSource::avro_value_to_value(&enum_value).unwrap();
        assert_eq!(result, Value::String("OPTION_A".to_string()));
        
        // Test Array
        let array_value = AvroValue::Array(vec![
            AvroValue::Int(1),
            AvroValue::Int(2),
            AvroValue::Int(3),
        ]);
        let result = AvroSource::avro_value_to_value(&array_value).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("1"));
            assert!(s.contains("2"));
            assert!(s.contains("3"));
        } else {
            panic!("Expected String value");
        }
        
        // Test Map
        let mut map = std::collections::HashMap::new();
        map.insert("key1".to_string(), AvroValue::String("value1".to_string()));
        map.insert("key2".to_string(), AvroValue::Int(42));
        let map_value = AvroValue::Map(map);
        let result = AvroSource::avro_value_to_value(&map_value).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("key1"));
            assert!(s.contains("value1"));
        } else {
            panic!("Expected String value");
        }
        
        // Test Record
        let record_value = AvroValue::Record(vec![
            ("field1".to_string(), AvroValue::String("test".to_string())),
            ("field2".to_string(), AvroValue::Int(123)),
        ]);
        let result = AvroSource::avro_value_to_value(&record_value).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("field1"));
            assert!(s.contains("test"));
        } else {
            panic!("Expected String value");
        }
        
        // Test TimeMillis
        let time_millis = AvroValue::TimeMillis(3600000); // 1 hour
        let result = AvroSource::avro_value_to_value(&time_millis).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("3600000"));
        } else {
            panic!("Expected String value");
        }
        
        // Test TimeMicros
        let time_micros = AvroValue::TimeMicros(3600000000); // 1 hour
        let result = AvroSource::avro_value_to_value(&time_micros).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("3600000000"));
        } else {
            panic!("Expected String value");
        }
        
        // Test TimestampMicros
        let timestamp_micros = AvroValue::TimestampMicros(1609459200000000); // 2021-01-01
        let result = AvroSource::avro_value_to_value(&timestamp_micros).unwrap();
        if let Value::Date(_) = result {
            // Success
        } else {
            panic!("Expected Date value");
        }
        
        // Test LocalTimestampMillis
        let local_timestamp_millis = AvroValue::LocalTimestampMillis(1609459200000);
        let result = AvroSource::avro_value_to_value(&local_timestamp_millis).unwrap();
        if let Value::Date(_) = result {
            // Success
        } else {
            panic!("Expected Date value");
        }
        
        // Test LocalTimestampMicros
        let local_timestamp_micros = AvroValue::LocalTimestampMicros(1609459200000000);
        let result = AvroSource::avro_value_to_value(&local_timestamp_micros).unwrap();
        if let Value::Date(_) = result {
            // Success
        } else {
            panic!("Expected Date value");
        }
        
        // Test Decimal
        let decimal_bytes = vec![1, 2, 3, 4];
        let decimal_value = AvroValue::Decimal(apache_avro::Decimal::from(decimal_bytes));
        let result = AvroSource::avro_value_to_value(&decimal_value).unwrap();
        if let Value::String(_) = result {
            // Success
        } else {
            panic!("Expected String value");
        }
        
        // Test Uuid
        let uuid = uuid::Uuid::new_v4();
        let uuid_value = AvroValue::Uuid(uuid);
        let result = AvroSource::avro_value_to_value(&uuid_value).unwrap();
        assert_eq!(result, Value::String(uuid.to_string()));
        
        // Test Duration
        let duration_value = AvroValue::Duration(apache_avro::Duration::new(
            apache_avro::Months::new(1),
            apache_avro::Days::new(2),
            apache_avro::Millis::new(3),
        ));
        let result = AvroSource::avro_value_to_value(&duration_value).unwrap();
        if let Value::String(_) = result {
            // Success
        } else {
            panic!("Expected String value");
        }
    }
    
    #[test]
    fn test_avro_type_to_schema_type_edge_cases() {
        // Test union with only null
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!(["null"])),
            DataType::String
        );
        
        // Test object without logicalType
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!({"type": "string"})),
            DataType::String
        );
        
        // Test object with unknown logicalType
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!({"type": "string", "logicalType": "unknown"})),
            DataType::String
        );
        
        // Test unknown type
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!("unknown_type")),
            DataType::String
        );
        
        // Test number (shouldn't happen but test default)
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!(123)),
            DataType::String
        );
        
        // Test timestamp-micros
        assert_eq!(
            AvroSource::avro_type_to_schema_type(&json!({"type": "long", "logicalType": "timestamp-micros"})),
            DataType::DateTime
        );
    }
    
    #[tokio::test]
    async fn test_avro_target_value_conversion_edge_cases() {
        // Test Integer to String
        let result = AvroTarget::value_to_avro_value(&Value::Integer(42), &DataType::String, false).unwrap();
        assert_eq!(result, AvroValue::String("42".to_string()));
        
        // Test Decimal to String
        let result = AvroTarget::value_to_avro_value(&Value::Decimal(Decimal::new(314, 2)), &DataType::String, false).unwrap();
        assert_eq!(result, AvroValue::String("3.14".to_string()));
        
        // Test Boolean to String
        let result = AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::String, false).unwrap();
        assert_eq!(result, AvroValue::String("true".to_string()));
        
        // Test invalid type combination
        let result = AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::Integer, false);
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_avro_target_buffer_size() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = AvroTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        
        let schema = Schema {
            columns: vec![
                Column {
                    name: "id".to_string(),
                    data_type: DataType::Integer,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        
        target.connect().await.unwrap();
        target.create_table("test", &schema).await.unwrap();
        
        // Write multiple small batches
        for i in 0..5 {
            let row = std::collections::HashMap::from([
                ("id".to_string(), Value::Integer(i)),
            ]);
            target.write_batch(&[row]).await.unwrap();
        }
        
        // Check buffer
        assert_eq!(target.buffer.len(), 5);
        
        // Finalize to flush buffer
        target.finalize().await.unwrap();
        assert_eq!(target.buffer.len(), 0);
    }
    
    #[test]
    fn test_avro_json_value_float_edge_cases() {
        // Test Float with NaN (should use fallback)
        let result = AvroSource::avro_value_to_json_value(&AvroValue::Float(f32::NAN)).unwrap();
        assert_eq!(result, JsonValue::Number(serde_json::Number::from(0)));
        
        // Test Double with NaN (should use fallback)
        let result = AvroSource::avro_value_to_json_value(&AvroValue::Double(f64::NAN)).unwrap();
        assert_eq!(result, JsonValue::Number(serde_json::Number::from(0)));
    }
    
    #[tokio::test]
    async fn test_avro_schema_all_types() {
        let schema = Schema {
            columns: vec![
                Column { name: "col_string".to_string(), data_type: DataType::String, nullable: false },
                Column { name: "col_string_null".to_string(), data_type: DataType::String, nullable: true },
                Column { name: "col_int".to_string(), data_type: DataType::Integer, nullable: false },
                Column { name: "col_int_null".to_string(), data_type: DataType::Integer, nullable: true },
                Column { name: "col_decimal".to_string(), data_type: DataType::Decimal, nullable: false },
                Column { name: "col_decimal_null".to_string(), data_type: DataType::Decimal, nullable: true },
                Column { name: "col_bool".to_string(), data_type: DataType::Boolean, nullable: false },
                Column { name: "col_bool_null".to_string(), data_type: DataType::Boolean, nullable: true },
                Column { name: "col_date".to_string(), data_type: DataType::Date, nullable: false },
                Column { name: "col_date_null".to_string(), data_type: DataType::Date, nullable: true },
                Column { name: "col_datetime".to_string(), data_type: DataType::DateTime, nullable: false },
                Column { name: "col_datetime_null".to_string(), data_type: DataType::DateTime, nullable: true },
                Column { name: "col_null".to_string(), data_type: DataType::Null, nullable: true },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        
        let avro_schema = AvroTarget::schema_to_avro_schema(&schema);
        assert!(avro_schema.is_ok());
        
        let schema_json: JsonValue = serde_json::from_str(&avro_schema.unwrap().canonical_form()).unwrap();
        
        if let JsonValue::Object(obj) = schema_json {
            if let Some(JsonValue::Array(fields)) = obj.get("fields") {
                assert_eq!(fields.len(), 13);
            }
        }
    }
    
    #[tokio::test]
    async fn test_avro_source_array_conversion_error() {
        // Create an array with values that can't convert to JSON
        let array_with_complex = AvroValue::Array(vec![
            AvroValue::String("test".to_string()),
        ]);
        let result = AvroSource::avro_value_to_value(&array_with_complex).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("test"));
        }
    }
}
