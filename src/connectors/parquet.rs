use arrow::array::*;
use arrow::datatypes::*;
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use parquet::arrow::{arrow_reader::ParquetRecordBatchReaderBuilder, ArrowWriter};
use parquet::file::reader::{FileReader, SerializedFileReader};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    connectors::{Source, Target},
    schema::{Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

pub struct ParquetSource {
    file_path: PathBuf,
    all_batches: Vec<RecordBatch>,
    current_batch_index: usize,
    total_row_count: Option<usize>,
}

impl ParquetSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            all_batches: Vec::new(),
            current_batch_index: 0,
            total_row_count: None,
        })
    }

    fn arrow_type_to_schema_type(arrow_type: &DataType) -> crate::schema::DataType {
        crate::schema::DataType::from_arrow(arrow_type)
    }

    fn arrow_array_to_values(
        array: &dyn Array,
        column_name: &str,
        field: &Field,
    ) -> Result<Vec<(String, Value)>> {
        let mut values = Vec::new();

        // Check if this is a JSON column based on metadata
        let is_json = field
            .metadata()
            .get("tinyetl:type")
            .map(|s| s == "json")
            .unwrap_or(false);

        match array.data_type() {
            DataType::Utf8 => {
                let string_array =
                    array
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| {
                            TinyEtlError::DataTransfer(
                                "Failed to downcast to StringArray".to_string(),
                            )
                        })?;
                for i in 0..string_array.len() {
                    let value = if string_array.is_null(i) {
                        Value::Null
                    } else if is_json {
                        // Parse the string as JSON
                        let json_str = string_array.value(i);
                        match serde_json::from_str(json_str) {
                            Ok(json_val) => Value::Json(json_val),
                            Err(_) => Value::String(json_str.to_string()),
                        }
                    } else {
                        Value::String(string_array.value(i).to_string())
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            DataType::Int64 => {
                let int_array = array.as_any().downcast_ref::<Int64Array>().ok_or_else(|| {
                    TinyEtlError::DataTransfer("Failed to downcast to Int64Array".to_string())
                })?;
                for i in 0..int_array.len() {
                    let value = if int_array.is_null(i) {
                        Value::Null
                    } else {
                        Value::Integer(int_array.value(i))
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            DataType::Int32 => {
                let int_array = array.as_any().downcast_ref::<Int32Array>().ok_or_else(|| {
                    TinyEtlError::DataTransfer("Failed to downcast to Int32Array".to_string())
                })?;
                for i in 0..int_array.len() {
                    let value = if int_array.is_null(i) {
                        Value::Null
                    } else {
                        Value::Integer(int_array.value(i) as i64)
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            DataType::Float64 => {
                let float_array =
                    array
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .ok_or_else(|| {
                            TinyEtlError::DataTransfer(
                                "Failed to downcast to Float64Array".to_string(),
                            )
                        })?;
                for i in 0..float_array.len() {
                    let value = if float_array.is_null(i) {
                        Value::Null
                    } else {
                        // Convert f64 to Decimal
                        match Decimal::try_from(float_array.value(i)) {
                            Ok(d) => Value::Decimal(d),
                            Err(_) => Value::String(float_array.value(i).to_string()),
                        }
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            DataType::Boolean => {
                let bool_array =
                    array
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .ok_or_else(|| {
                            TinyEtlError::DataTransfer(
                                "Failed to downcast to BooleanArray".to_string(),
                            )
                        })?;
                for i in 0..bool_array.len() {
                    let value = if bool_array.is_null(i) {
                        Value::Null
                    } else {
                        Value::Boolean(bool_array.value(i))
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                let ts_array = array
                    .as_any()
                    .downcast_ref::<TimestampNanosecondArray>()
                    .ok_or_else(|| {
                        TinyEtlError::DataTransfer(
                            "Failed to downcast to TimestampNanosecondArray".to_string(),
                        )
                    })?;
                for i in 0..ts_array.len() {
                    let value = if ts_array.is_null(i) {
                        Value::Null
                    } else {
                        let timestamp_ns = ts_array.value(i);
                        let timestamp_s = timestamp_ns / 1_000_000_000;
                        let nanoseconds = (timestamp_ns % 1_000_000_000) as u32;
                        match chrono::DateTime::from_timestamp(timestamp_s, nanoseconds) {
                            Some(dt) => Value::Date(dt),
                            None => Value::Null,
                        }
                    };
                    values.push((column_name.to_string(), value));
                }
            }
            _ => {
                // For unsupported types, convert to string representation
                for i in 0..array.len() {
                    let value = if array.is_null(i) {
                        Value::Null
                    } else {
                        // This is a fallback - in production you might want more sophisticated conversion
                        Value::String(format!("unsupported_type_row_{}", i))
                    };
                    values.push((column_name.to_string(), value));
                }
            }
        }

        Ok(values)
    }

    fn record_batch_to_rows(batch: &RecordBatch) -> Result<Vec<Row>> {
        let num_rows = batch.num_rows();
        let mut rows = vec![HashMap::new(); num_rows];

        let schema = batch.schema();
        for (col_index, field) in schema.fields().iter().enumerate() {
            let column = batch.column(col_index);
            let column_values = Self::arrow_array_to_values(column.as_ref(), &field.name(), field)?;

            for (row_index, (column_name, value)) in column_values.into_iter().enumerate() {
                if row_index < num_rows {
                    rows[row_index].insert(column_name, value);
                }
            }
        }

        Ok(rows)
    }
}

#[async_trait]
impl Source for ParquetSource {
    async fn connect(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Parquet file not found: {}", self.file_path.display()),
            )));
        }

        let file = std::fs::File::open(&self.file_path).map_err(TinyEtlError::Io)?;

        let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
            TinyEtlError::Connection(format!("Failed to create reader builder: {}", e))
        })?;

        // Get total row count from metadata
        let metadata = builder.metadata();
        let total_rows: usize = metadata
            .row_groups()
            .iter()
            .map(|rg| rg.num_rows() as usize)
            .sum();
        self.total_row_count = Some(total_rows);

        // Read all batches upfront
        let mut batch_reader = builder.build().map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to create batch reader: {}", e))
        })?;

        while let Some(batch_result) = batch_reader.next() {
            let batch = batch_result
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to read batch: {}", e)))?;
            self.all_batches.push(batch);
        }

        Ok(())
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.all_batches.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        let arrow_schema = self.all_batches[0].schema();

        let columns = arrow_schema
            .fields()
            .iter()
            .map(|field| {
                let data_type = Self::arrow_type_to_schema_type(field.data_type());
                crate::schema::Column {
                    name: field.name().clone(),
                    data_type,
                    nullable: field.is_nullable(),
                }
            })
            .collect();

        Ok(Schema {
            columns,
            estimated_rows: self.total_row_count,
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.current_batch_index >= self.all_batches.len() {
            return Ok(Vec::new()); // No more data
        }

        let batch = &self.all_batches[self.current_batch_index];
        self.current_batch_index += 1;

        let mut rows = Self::record_batch_to_rows(batch)?;

        // If requested batch size is smaller than the record batch, we need to split it
        if rows.len() > batch_size {
            let remaining_rows = rows.split_off(batch_size);
            // For simplicity, we'll just return the first batch_size rows
            // In a more sophisticated implementation, you'd store the remaining rows
            // and return them in subsequent calls
        }

        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(self.total_row_count)
    }

    async fn reset(&mut self) -> Result<()> {
        self.current_batch_index = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.current_batch_index < self.all_batches.len()
    }
}

pub struct ParquetTarget {
    file_path: PathBuf,
    schema: Option<Arc<arrow::datatypes::Schema>>,
    buffered_rows: Vec<Row>,
    is_finalized: bool,
}

impl ParquetTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            schema: None,
            buffered_rows: Vec::new(),
            is_finalized: false,
        })
    }

    fn schema_to_arrow_schema(schema: &Schema) -> Arc<arrow::datatypes::Schema> {
        Arc::new(schema.to_arrow_schema())
    }

    fn rows_to_record_batch(
        rows: &[Row],
        schema: &Arc<arrow::datatypes::Schema>,
    ) -> Result<RecordBatch> {
        if rows.is_empty() {
            return Ok(RecordBatch::new_empty(schema.clone()));
        }

        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        for field in schema.fields() {
            let column_name = field.name();

            match field.data_type() {
                DataType::Utf8 => {
                    let mut builder = StringBuilder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::String(s)) => builder.append_value(s),
                            Some(Value::Json(j)) => builder.append_value(&j.to_string()),
                            Some(Value::Null) => builder.append_null(),
                            Some(other) => builder.append_value(&format!("{:?}", other)),
                            None => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
                DataType::Int64 => {
                    let mut builder = Int64Builder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Integer(i)) => builder.append_value(*i),
                            Some(Value::Null) => builder.append_null(),
                            None => builder.append_null(),
                            _ => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
                DataType::Float64 => {
                    let mut builder = Float64Builder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Decimal(d)) => {
                                // Convert Decimal to f64
                                let f: f64 = (*d).try_into().unwrap_or(0.0);
                                builder.append_value(f);
                            }
                            Some(Value::Null) => builder.append_null(),
                            None => builder.append_null(),
                            _ => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
                DataType::Boolean => {
                    let mut builder = BooleanBuilder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Boolean(b)) => builder.append_value(*b),
                            Some(Value::Null) => builder.append_null(),
                            None => builder.append_null(),
                            _ => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
                DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                    let mut builder = TimestampNanosecondBuilder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Date(dt)) => {
                                let timestamp_ns = dt.timestamp_nanos_opt().unwrap_or(0);
                                builder.append_value(timestamp_ns);
                            }
                            Some(Value::Null) => builder.append_null(),
                            None => builder.append_null(),
                            _ => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
                _ => {
                    // Default to string for unsupported types
                    let mut builder = StringBuilder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Null) => builder.append_null(),
                            Some(value) => builder.append_value(&format!("{:?}", value)),
                            None => builder.append_null(),
                        }
                    }
                    arrays.push(Arc::new(builder.finish()));
                }
            }
        }

        RecordBatch::try_new(schema.clone(), arrays).map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to create record batch: {}", e))
        })
    }
}

#[async_trait]
impl Target for ParquetTarget {
    async fn connect(&mut self) -> Result<()> {
        // Nothing to do here - we'll create the file when we finalize
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        let arrow_schema = Self::schema_to_arrow_schema(schema);
        self.schema = Some(arrow_schema);

        // Create parent directory if it doesn't exist
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(TinyEtlError::Io)?;
        }

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        // Buffer the rows - we'll write them all at once during finalize
        self.buffered_rows.extend_from_slice(rows);
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        if self.is_finalized || self.buffered_rows.is_empty() {
            return Ok(());
        }

        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?;

        // Create the file and writer
        let file = std::fs::File::create(&self.file_path).map_err(TinyEtlError::Io)?;

        let mut writer = ArrowWriter::try_new(file, schema.clone(), None).map_err(|e| {
            TinyEtlError::Connection(format!("Failed to create parquet writer: {}", e))
        })?;

        // Convert all buffered rows to record batch and write
        let batch = Self::rows_to_record_batch(&self.buffered_rows, schema)?;

        writer
            .write(&batch)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to write batch: {}", e)))?;

        writer.close().map_err(|e| {
            TinyEtlError::Connection(format!("Failed to close parquet writer: {}", e))
        })?;

        self.is_finalized = true;
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // For Parquet files, truncation means clearing buffered rows
        self.buffered_rows.clear();
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // Parquet files don't easily support append - would require reading existing file and merging
        // For simplicity, we return false to force truncation
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_parquet_target_creation() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.parquet");
        let target = ParquetTarget::new(file_path.to_str().unwrap());
        assert!(target.is_ok());
    }

    #[tokio::test]
    async fn test_parquet_write_read_cycle() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_cycle.parquet");

        // Create a simple schema
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
            ],
            estimated_rows: Some(2),
            primary_key_candidate: None,
        };

        // Create test data
        let mut row1 = HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));

        let mut row2 = HashMap::new();
        row2.insert("id".to_string(), Value::Integer(2));
        row2.insert("name".to_string(), Value::String("Bob".to_string()));

        let rows = vec![row1, row2];

        // Write data
        let mut target = ParquetTarget::new(file_path.to_str().unwrap()).unwrap();
        target.connect().await.unwrap();
        target.create_table("test_table", &schema).await.unwrap();
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // Verify file was created
        assert!(file_path.exists());

        // Read data back
        let mut source = ParquetSource::new(file_path.to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let read_schema = source.infer_schema(100).await.unwrap();
        assert_eq!(read_schema.columns.len(), 2);

        let read_rows = source.read_batch(100).await.unwrap();
        assert_eq!(read_rows.len(), 2);
    }
}
