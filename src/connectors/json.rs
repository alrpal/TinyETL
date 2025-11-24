use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json;
use std::path::PathBuf;

use crate::{
    connectors::{Source, Target},
    date_parser::DateParser,
    schema::{Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

pub struct JsonSource {
    file_path: PathBuf,
    data: Vec<serde_json::Value>,
    current_index: usize,
}

impl JsonSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            data: Vec::new(),
            current_index: 0,
        })
    }

    fn json_value_to_value(&self, json_val: &serde_json::Value) -> Value {
        match json_val {
            serde_json::Value::String(s) => {
                // Try to parse string as date first, fall back to string
                if let Some(date_value) = DateParser::try_parse(s) {
                    date_value
                } else {
                    Value::String(s.clone())
                }
            }
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    // Convert f64 to Decimal
                    match Decimal::try_from(f) {
                        Ok(d) => Value::Decimal(d),
                        Err(_) => Value::String(n.to_string()),
                    }
                } else {
                    Value::String(n.to_string())
                }
            }
            serde_json::Value::Bool(b) => Value::Boolean(*b),
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Value::String(json_val.to_string())
            }
        }
    }
}

#[async_trait]
impl Source for JsonSource {
    async fn connect(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(format!(
                "JSON file not found: {}",
                self.file_path.display()
            )));
        }

        let content = std::fs::read_to_string(&self.file_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;

        // Expect an array of objects
        if let serde_json::Value::Array(array) = json {
            self.data = array;
        } else {
            return Err(TinyEtlError::Configuration(
                "JSON file must contain an array of objects".to_string(),
            ));
        }

        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.data.is_empty() {
            self.connect().await?;
        }

        let sample_data = self.data.iter().take(sample_size).collect::<Vec<_>>();

        let mut rows = Vec::new();
        for json_obj in sample_data {
            if let serde_json::Value::Object(obj) = json_obj {
                let mut row = Row::new();
                for (key, value) in obj {
                    row.insert(key.clone(), self.json_value_to_value(value));
                }
                rows.push(row);
            }
        }

        SchemaInferer::infer_from_rows(&rows)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        let end_index = std::cmp::min(self.current_index + batch_size, self.data.len());

        for i in self.current_index..end_index {
            if let serde_json::Value::Object(obj) = &self.data[i] {
                let mut row = Row::new();
                for (key, value) in obj {
                    row.insert(key.clone(), self.json_value_to_value(value));
                }
                rows.push(row);
            }
        }

        self.current_index = end_index;
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(Some(self.data.len()))
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_index = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.current_index < self.data.len()
    }
}

pub struct JsonTarget {
    file_path: PathBuf,
    accumulated_rows: Vec<Row>,
    schema: Option<Schema>,
}

impl JsonTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            accumulated_rows: Vec::new(),
            schema: None,
        })
    }

    fn value_to_json(&self, value: &Value) -> serde_json::Value {
        match value {
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Integer(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
            Value::Decimal(d) => {
                // Convert Decimal to f64 for JSON representation
                match (*d).try_into() {
                    Ok(f) => {
                        if let Some(n) = serde_json::Number::from_f64(f) {
                            serde_json::Value::Number(n)
                        } else {
                            serde_json::Value::String(d.to_string())
                        }
                    }
                    Err(_) => serde_json::Value::String(d.to_string()),
                }
            }
            Value::Boolean(b) => serde_json::Value::Bool(*b),
            Value::Date(dt) => serde_json::Value::String(dt.to_rfc3339()),
            Value::Json(j) => j.clone(), // Already a JSON value, just clone it
            Value::Null => serde_json::Value::Null,
        }
    }

    fn json_to_value(json_val: &serde_json::Value) -> Value {
        match json_val {
            serde_json::Value::String(s) => {
                // Try to parse as date first
                if let Some(date_value) = crate::date_parser::DateParser::try_parse(s) {
                    date_value
                } else {
                    Value::String(s.clone())
                }
            }
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    match rust_decimal::Decimal::try_from(f) {
                        Ok(d) => Value::Decimal(d),
                        Err(_) => Value::String(n.to_string()),
                    }
                } else {
                    Value::String(n.to_string())
                }
            }
            serde_json::Value::Bool(b) => Value::Boolean(*b),
            serde_json::Value::Null => Value::Null,
            _ => Value::String(json_val.to_string()), // Fallback for arrays/objects
        }
    }
}

#[async_trait]
impl Target for JsonTarget {
    async fn connect(&mut self) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        self.schema = Some(schema.clone());

        // If file exists and we support append, load existing data
        if self.file_path.exists() && self.supports_append() {
            if let Ok(content) = std::fs::read_to_string(&self.file_path) {
                if let Ok(existing_json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(array) = existing_json.as_array() {
                        // Convert existing JSON objects back to Row format
                        for json_obj in array {
                            if let Some(obj) = json_obj.as_object() {
                                let mut row = Row::new();
                                for (key, json_val) in obj {
                                    let value = Self::json_to_value(json_val);
                                    row.insert(key.clone(), value);
                                }
                                self.accumulated_rows.push(row);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        // Accumulate rows - we'll write them all at once in finalize()
        self.accumulated_rows.extend_from_slice(rows);
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        // Convert all accumulated rows to JSON and write to file
        let mut json_objects = Vec::new();

        for row in &self.accumulated_rows {
            let mut json_obj = serde_json::Map::new();

            // Use schema order if available, otherwise use row keys
            if let Some(schema) = &self.schema {
                for column in &schema.columns {
                    let json_value = row
                        .get(&column.name)
                        .map(|v| self.value_to_json(v))
                        .unwrap_or(serde_json::Value::Null);
                    json_obj.insert(column.name.clone(), json_value);
                }
            } else {
                // Fallback: iterate over row keys
                for (key, value) in row {
                    json_obj.insert(key.clone(), self.value_to_json(value));
                }
            }

            json_objects.push(serde_json::Value::Object(json_obj));
        }

        // Write JSON array to file
        let json_array = serde_json::Value::Array(json_objects);
        let json_string = serde_json::to_string_pretty(&json_array)?;
        std::fs::write(&self.file_path, json_string)?;

        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // For JSON files, truncation means clearing accumulated rows
        self.accumulated_rows.clear();
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // JSON arrays support append - we can merge existing data with new data
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_json_source_connection() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice", "age": 25}},
            {{"id": 2, "name": "Bob", "age": 30}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_ok());
        assert_eq!(source.data.len(), 2);
    }

    #[tokio::test]
    async fn test_json_source_schema_inference() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice", "active": true}},
            {{"id": 2, "name": "Bob", "active": false}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let schema = source.infer_schema(10).await.unwrap();
        assert_eq!(schema.columns.len(), 3);

        let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, crate::schema::DataType::Integer);
    }

    #[tokio::test]
    async fn test_json_source_read_batch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice"}},
            {{"id": 2, "name": "Bob"}},
            {{"id": 3, "name": "Charlie"}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
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
    async fn test_json_invalid_format() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"{{"not": "an array"}}"#).unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_json_target_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let result = target.connect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_json_target_write_with_schema() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        // Create a schema
        let schema = Schema {
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
                crate::schema::Column {
                    name: "active".to_string(),
                    data_type: crate::schema::DataType::Boolean,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        target.create_table("test", &schema).await.unwrap();

        // Create test data
        let mut row1 = std::collections::HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));
        row1.insert("active".to_string(), Value::Boolean(true));

        let mut row2 = std::collections::HashMap::new();
        row2.insert("id".to_string(), Value::Integer(2));
        row2.insert("name".to_string(), Value::String("Bob".to_string()));
        row2.insert("active".to_string(), Value::Boolean(false));

        let written = target.write_batch(&[row1, row2]).await.unwrap();
        assert_eq!(written, 2);

        target.finalize().await.unwrap();

        // Verify the output
        let output_content = std::fs::read_to_string(temp_file.path()).unwrap();
        let parsed_json: serde_json::Value = serde_json::from_str(&output_content).unwrap();

        assert!(parsed_json.is_array());
        let array = parsed_json.as_array().unwrap();
        assert_eq!(array.len(), 2);

        // Verify first object
        let first_obj = &array[0];
        assert_eq!(first_obj["id"], 1);
        assert_eq!(first_obj["name"], "Alice");
        assert_eq!(first_obj["active"], true);
    }

    #[tokio::test]
    async fn test_json_target_value_conversion() {
        let target = JsonTarget::new("/tmp/test.json").unwrap();

        assert_eq!(
            target.value_to_json(&Value::String("test".to_string())),
            serde_json::Value::String("test".to_string())
        );
        assert_eq!(
            target.value_to_json(&Value::Integer(42)),
            serde_json::Value::Number(serde_json::Number::from(42))
        );
        assert_eq!(
            target.value_to_json(&Value::Boolean(true)),
            serde_json::Value::Bool(true)
        );
        assert_eq!(target.value_to_json(&Value::Null), serde_json::Value::Null);

        // Test decimal conversion
        let decimal_val = target.value_to_json(&Value::Decimal(Decimal::new(314, 2)));
        assert!(decimal_val.is_number());
    }

    #[tokio::test]
    async fn test_json_roundtrip() {
        // Create source JSON
        let source_file = NamedTempFile::new().unwrap();
        let source_path = source_file.path().to_str().unwrap();
        std::fs::write(
            source_path,
            r#"[
            {"id": 1, "name": "Alice", "active": true},
            {"id": 2, "name": "Bob", "active": false}
        ]"#,
        )
        .unwrap();

        // Read with JsonSource
        let mut source = JsonSource::new(source_path).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(10).await.unwrap();
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();

        // Write with JsonTarget
        let target_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // Verify output
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        let parsed_json: serde_json::Value = serde_json::from_str(&output_content).unwrap();

        assert!(parsed_json.is_array());
        let array = parsed_json.as_array().unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array[0]["name"], "Alice");
        assert_eq!(array[1]["name"], "Bob");
    }
}
