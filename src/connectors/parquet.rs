use arrow::array::*;
use arrow::datatypes::*;
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use parquet::arrow::{
    arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder},
    ArrowWriter,
};
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::collections::VecDeque;
use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    connectors::{output_file::OutputFile, Source, Target},
    schema::{Row, Schema, Value},
    Result, TinyEtlError,
};

/// Rows decoded per Arrow batch when reading.
const READ_BATCH_ROWS: usize = 8192;

/// Rows per row group when writing. The writer buffers one row group (encoded) in memory.
const ROW_GROUP_ROWS: usize = 128 * 1024;

/// Reads a Parquet file batch by batch, keeping one reader open.
pub struct ParquetSource {
    file_path: PathBuf,
    // Mutex only to make the reader Sync (Source requires it); reached through get_mut
    reader: Option<Mutex<ParquetRecordBatchReader>>,
    arrow_schema: Option<SchemaRef>,
    /// Rows decoded but not yet returned (an Arrow batch rarely matches the requested size)
    pending: VecDeque<Row>,
    total_row_count: Option<usize>,
    exhausted: bool,
    /// Whether rows have been read since the file was opened
    consumed: bool,
}

impl ParquetSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            reader: None,
            arrow_schema: None,
            pending: VecDeque::new(),
            total_row_count: None,
            exhausted: true,
            consumed: false,
        })
    }

    fn arrow_type_to_schema_type(arrow_type: &DataType) -> crate::schema::DataType {
        crate::schema::DataType::from_arrow(arrow_type)
    }

    fn open(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Parquet file not found: {}", self.file_path.display()),
            )));
        }
        let file = File::open(&self.file_path).map_err(TinyEtlError::Io)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
            TinyEtlError::Connection(format!("Failed to create reader builder: {}", e))
        })?;
        self.total_row_count = Some(
            builder
                .metadata()
                .row_groups()
                .iter()
                .map(|rg| rg.num_rows() as usize)
                .sum(),
        );
        self.arrow_schema = Some(builder.schema().clone());
        let reader = builder
            .with_batch_size(READ_BATCH_ROWS)
            .build()
            .map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to create batch reader: {}", e))
            })?;
        self.reader = Some(Mutex::new(reader));
        self.pending.clear();
        self.exhausted = self.total_row_count == Some(0);
        self.consumed = false;
        Ok(())
    }
}

#[async_trait]
impl Source for ParquetSource {
    async fn connect(&mut self) -> Result<()> {
        self.open()
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.arrow_schema.is_none() {
            self.open()?;
        }
        let arrow_schema = self.arrow_schema.as_ref().expect("opened above");
        let columns = arrow_schema
            .fields()
            .iter()
            .map(|field| crate::schema::Column {
                name: field.name().clone(),
                data_type: Self::arrow_type_to_schema_type(field.data_type()),
                nullable: field.is_nullable(),
            })
            .collect();
        Ok(Schema {
            columns,
            estimated_rows: self.total_row_count,
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        while self.pending.len() < batch_size && !self.exhausted {
            let Some(reader) = self.reader.as_mut() else {
                break;
            };
            let reader = reader.get_mut().unwrap_or_else(|e| e.into_inner());
            match reader.next() {
                Some(batch) => {
                    let batch = batch.map_err(|e| {
                        TinyEtlError::DataTransfer(format!("Failed to read batch: {}", e))
                    })?;
                    self.pending
                        .extend(crate::connectors::arrow_values::record_batch_to_rows(
                            &batch,
                        )?);
                }
                None => self.exhausted = true,
            }
        }
        let n = batch_size.min(self.pending.len());
        self.consumed |= n > 0;
        Ok(self.pending.drain(..n).collect())
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(self.total_row_count)
    }

    async fn reset(&mut self) -> Result<()> {
        if self.consumed || self.reader.is_none() {
            self.open()?;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        !self.pending.is_empty() || !self.exhausted
    }
}

/// Writes a Parquet file, one record batch per incoming batch, Snappy-compressed.
pub struct ParquetTarget {
    file_path: PathBuf,
    schema: Option<Arc<arrow::datatypes::Schema>>,
    // Mutex only to make the writer Sync (Target requires it); reached through get_mut
    writer: Option<Mutex<ArrowWriter<File>>>,
    output: Option<OutputFile>,
}

impl ParquetTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            schema: None,
            writer: None,
            output: None,
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
                            Some(Value::Null) => builder.append_null(),
                            Some(other) => match other.to_string_for_arrow() {
                                Some(text) => builder.append_value(text),
                                None => builder.append_null(),
                            },
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
                DataType::Date64 => {
                    let mut builder = Date64Builder::new();
                    for row in rows {
                        match row.get(column_name) {
                            Some(Value::Date(dt)) => builder.append_value(dt.timestamp_millis()),
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
                            Some(value) => builder.append_value(format!("{:?}", value)),
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
        let (output, file) = OutputFile::create(&self.file_path)?;
        let properties = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .build();
        let writer =
            ArrowWriter::try_new(file, arrow_schema.clone(), Some(properties)).map_err(|e| {
                TinyEtlError::Connection(format!("Failed to create parquet writer: {}", e))
            })?;
        self.schema = Some(arrow_schema);
        self.writer = Some(Mutex::new(writer));
        self.output = Some(output);
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?;
        let batch = Self::rows_to_record_batch(rows, schema)?;
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?
            .get_mut()
            .unwrap_or_else(|e| e.into_inner());
        writer
            .write(&batch)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to write batch: {}", e)))?;
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        let (Some(writer), Some(output)) = (self.writer.take(), self.output.take()) else {
            return Ok(());
        };
        let writer = writer.into_inner().unwrap_or_else(|e| e.into_inner());
        writer.close().map_err(|e| {
            TinyEtlError::Connection(format!("Failed to close parquet writer: {}", e))
        })?;
        output.commit()
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // The file is replaced when the transfer finishes
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
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn column(name: &str, data_type: DataType) -> Column {
        Column {
            name: name.to_string(),
            data_type,
            nullable: true,
        }
    }

    fn one_column_schema() -> Schema {
        Schema {
            columns: vec![column("id", DataType::Integer)],
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }

    fn id_rows(range: std::ops::Range<i64>) -> Vec<Row> {
        range
            .map(|i| HashMap::from([("id".to_string(), Value::Integer(i))]))
            .collect()
    }

    async fn read_all(path: &str, batch_size: usize) -> Vec<Row> {
        let mut source = ParquetSource::new(path).unwrap();
        source.connect().await.unwrap();
        source.infer_schema(100).await.unwrap();
        source.reset().await.unwrap();
        let mut rows = Vec::new();
        while source.has_more() {
            let batch = source.read_batch(batch_size).await.unwrap();
            if batch.is_empty() {
                break;
            }
            assert!(batch.len() <= batch_size);
            rows.extend(batch);
        }
        rows
    }

    #[tokio::test]
    async fn test_streams_many_batches_without_losing_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("many.parquet");
        let path = path.to_str().unwrap();
        let mut target = ParquetTarget::new(path).unwrap();
        target
            .create_table("t", &one_column_schema())
            .await
            .unwrap();
        for start in (0..30_000).step_by(7_000) {
            let end = (start + 7_000).min(30_000);
            target.write_batch(&id_rows(start..end)).await.unwrap();
        }
        target.finalize().await.unwrap();

        // Batch sizes smaller and larger than the reader's Arrow batches (8192 rows)
        for batch_size in [500, 10_000] {
            let rows = read_all(path, batch_size).await;
            assert_eq!(rows.len(), 30_000, "batch size {}", batch_size);
            assert_eq!(rows[29_999]["id"], Value::Integer(29_999));
        }
    }

    #[tokio::test]
    async fn test_reset_reads_again() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("reset.parquet");
        let path = path.to_str().unwrap();
        let mut target = ParquetTarget::new(path).unwrap();
        target
            .create_table("t", &one_column_schema())
            .await
            .unwrap();
        target.write_batch(&id_rows(0..10)).await.unwrap();
        target.finalize().await.unwrap();

        let mut source = ParquetSource::new(path).unwrap();
        source.connect().await.unwrap();
        assert_eq!(source.read_batch(4).await.unwrap().len(), 4);
        source.reset().await.unwrap();
        assert_eq!(source.read_batch(100).await.unwrap().len(), 10);
        assert!(!source.has_more());
    }

    #[tokio::test]
    async fn test_empty_input_writes_a_valid_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.parquet");
        let path = path.to_str().unwrap();
        let mut target = ParquetTarget::new(path).unwrap();
        target
            .create_table("t", &one_column_schema())
            .await
            .unwrap();
        target.finalize().await.unwrap();

        let mut source = ParquetSource::new(path).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(100).await.unwrap();
        assert_eq!(schema.columns.len(), 1);
        assert_eq!(schema.columns[0].name, "id");
        assert!(!source.has_more());
    }

    #[tokio::test]
    async fn test_unfinished_write_keeps_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("keep.parquet");
        std::fs::write(&path, b"previous contents").unwrap();
        let mut target = ParquetTarget::new(path.to_str().unwrap()).unwrap();
        target
            .create_table("t", &one_column_schema())
            .await
            .unwrap();
        target.write_batch(&id_rows(0..10)).await.unwrap();
        drop(target); // the transfer failed before finalize
        assert_eq!(std::fs::read(&path).unwrap(), b"previous contents");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn test_date_columns() {
        use chrono::{TimeZone, Utc};
        let dir = tempdir().unwrap();
        let path = dir.path().join("dates.parquet");
        let path = path.to_str().unwrap();
        let schema = Schema {
            columns: vec![
                column("day", DataType::Date),
                column("at", DataType::DateTime),
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        let day = Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let at = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 5).unwrap();
        let row: Row = HashMap::from([
            ("day".to_string(), Value::Date(day)),
            ("at".to_string(), Value::Date(at)),
        ]);
        let mut target = ParquetTarget::new(path).unwrap();
        target.create_table("t", &schema).await.unwrap();
        target.write_batch(&[row]).await.unwrap();
        target.finalize().await.unwrap();

        let rows = read_all(path, 10).await;
        assert_eq!(rows[0]["day"], Value::Date(day));
        assert_eq!(rows[0]["at"], Value::Date(at));
    }

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
