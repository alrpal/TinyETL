use apache_avro::{types::Value as AvroValue, Reader, Schema as AvroSchema, Writer};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::iter::Peekable;
use std::path::PathBuf;

use crate::{
    connectors::{output_file::OutputFile, Source, Target},
    schema::{Column, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

type AvroRecords = Peekable<Reader<'static, BufReader<File>>>;

/// Read a zigzag-encoded variable-length long (Avro's `long` encoding).
fn read_long(reader: &mut impl std::io::Read) -> std::io::Result<i64> {
    let (mut value, mut shift) = (0u64, 0u32);
    loop {
        let mut byte = [0u8];
        reader.read_exact(&mut byte)?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok((value >> 1) as i64 ^ -((value & 1) as i64));
        }
        shift += 7;
        if shift > 63 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad Avro varint",
            ));
        }
    }
}

struct CountingReader<'a, R> {
    inner: &'a mut R,
    read: u64,
}

impl<R: std::io::Read> std::io::Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

/// Records in an Avro container file, summed from the block headers. Each block's data is
/// skipped with a seek, so this reads only a few bytes per block.
fn count_records(path: &std::path::Path) -> std::io::Result<usize> {
    use std::io::{Read, Seek};
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not an Avro file");
    let mut reader = BufReader::with_capacity(1 << 16, File::open(path)?);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"Obj\x01" {
        return Err(invalid());
    }
    // File metadata: a map of string -> bytes, written as blocks of entries
    loop {
        let mut entries = read_long(&mut reader)?;
        if entries == 0 {
            break;
        }
        if entries < 0 {
            entries = -entries;
            read_long(&mut reader)?; // block size in bytes
        }
        for _ in 0..entries {
            for _ in 0..2 {
                let len = read_long(&mut reader)?;
                reader.seek_relative(len)?;
            }
        }
    }
    reader.seek_relative(16)?; // sync marker
    let end = reader.get_ref().metadata()?.len();
    let mut position = reader.stream_position()?;
    let mut records = 0usize;
    // Positions are tracked here rather than asked of the file, which would be a system call
    let mut counted = CountingReader {
        inner: &mut reader,
        read: 0,
    };
    while position < end {
        counted.read = 0;
        let count = read_long(&mut counted)?;
        let size = read_long(&mut counted)?;
        if count < 0 || size < 0 {
            return Err(invalid());
        }
        records += count as usize;
        counted.inner.seek_relative(size + 16)?;
        position += counted.read + size as u64 + 16;
    }
    Ok(records)
}

/// Reads an Avro container file record by record, keeping one reader open.
pub struct AvroSource {
    file_path: PathBuf,
    records: Option<AvroRecords>,
    columns: Vec<Column>,
    /// Each field's schema, for decimal scales and nested values
    field_schemas: HashMap<String, AvroSchema>,
    /// Whether records have been read since the reader was opened
    consumed: bool,
    exhausted: bool,
}

impl AvroSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            records: None,
            columns: Vec::new(),
            field_schemas: HashMap::new(),
            consumed: false,
            exhausted: true,
        })
    }

    fn open(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(format!(
                "Avro file not found: {}",
                self.file_path.display()
            )));
        }
        let file = File::open(&self.file_path)?;
        let reader = Reader::new(BufReader::new(file))
            .map_err(|e| TinyEtlError::DataTransfer(format!("Invalid Avro file: {}", e)))?;
        self.columns = Self::columns_from_schema(reader.writer_schema());
        self.field_schemas = Self::field_schemas(reader.writer_schema());
        let mut records = reader.peekable();
        self.exhausted = records.peek().is_none();
        self.records = Some(records);
        self.consumed = false;
        Ok(())
    }

    /// TinyETL columns for the fields of a record schema. Read from the parsed schema, not
    /// its canonical form, which drops logical types such as `timestamp-millis`.
    fn columns_from_schema(schema: &AvroSchema) -> Vec<Column> {
        let AvroSchema::Record(record) = schema else {
            return Vec::new();
        };
        record
            .fields
            .iter()
            .map(|field| {
                let (data_type, nullable) = Self::field_type(&field.schema);
                Column {
                    name: field.name.clone(),
                    data_type,
                    nullable,
                }
            })
            .collect()
    }

    fn field_type(schema: &AvroSchema) -> (DataType, bool) {
        match schema {
            AvroSchema::Union(union) => {
                let nullable = union
                    .variants()
                    .iter()
                    .any(|v| matches!(v, AvroSchema::Null));
                let data_type = union
                    .variants()
                    .iter()
                    .find(|v| !matches!(v, AvroSchema::Null))
                    .map(|v| Self::field_type(v).0)
                    .unwrap_or(DataType::String);
                (data_type, nullable)
            }
            AvroSchema::Int | AvroSchema::Long => (DataType::Integer, false),
            AvroSchema::Float | AvroSchema::Double => (DataType::Decimal, false),
            AvroSchema::Boolean => (DataType::Boolean, false),
            AvroSchema::Decimal(_) => (DataType::Decimal, false),
            AvroSchema::Array(_) | AvroSchema::Map(_) | AvroSchema::Record(_) => {
                (DataType::Json, false)
            }
            AvroSchema::Date => (DataType::Date, false),
            AvroSchema::TimestampMillis
            | AvroSchema::TimestampMicros
            | AvroSchema::LocalTimestampMillis
            | AvroSchema::LocalTimestampMicros => (DataType::DateTime, false),
            AvroSchema::Null => (DataType::String, true),
            _ => (DataType::String, false),
        }
    }

    #[cfg(test)]
    fn avro_value_to_value(avro_value: &AvroValue) -> Result<Value> {
        Ok(avro_to_value(avro_value, None))
    }

    #[cfg(test)]
    fn avro_value_to_json_value(avro_value: &AvroValue) -> Result<JsonValue> {
        Ok(avro_to_json(avro_value, None))
    }

    /// The schema of each top-level field, for converting its values
    fn field_schemas(schema: &AvroSchema) -> HashMap<String, AvroSchema> {
        match schema {
            AvroSchema::Record(record) => record
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.schema.clone()))
                .collect(),
            _ => HashMap::new(),
        }
    }
}

/// The schema of the value a union holds, when the union's schema is known
fn union_variant(schema: Option<&AvroSchema>, index: u32) -> Option<&AvroSchema> {
    match schema {
        Some(AvroSchema::Union(union)) => union.variants().get(index as usize),
        _ => None,
    }
}

/// Binary data as text: `0x` and lowercase hex, as every connector writes it
fn hex_text(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// Seconds and nanoseconds since midnight as `HH:MM:SS[.fff]`
fn time_text(seconds: i64, nanos: u32) -> Option<String> {
    let seconds = u32::try_from(seconds).ok()?;
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos)
        .map(|t| t.format("%H:%M:%S%.f").to_string())
}

/// An Avro decimal (a big-endian two's complement unscaled value) with its schema's scale
fn decimal_value(decimal: &apache_avro::Decimal, schema: Option<&AvroSchema>) -> Value {
    let bytes = <Vec<u8>>::try_from(decimal).unwrap_or_default();
    let scale = match schema {
        Some(AvroSchema::Decimal(d)) => d.scale,
        _ => 0,
    };
    if bytes.is_empty() || bytes.len() > 16 {
        return Value::String(hex_text(&bytes));
    }
    let negative = bytes[0] & 0x80 != 0;
    let mut padded = [if negative { 0xff } else { 0 }; 16];
    padded[16 - bytes.len()..].copy_from_slice(&bytes);
    let unscaled = i128::from_be_bytes(padded);
    match u32::try_from(scale)
        .ok()
        .and_then(|scale| Decimal::try_from_i128_with_scale(unscaled, scale).ok())
    {
        Some(d) => Value::Decimal(d),
        // Too wide for a Decimal: the exact digits as text
        None => {
            let digits = format!("{:0>width$}", unscaled.unsigned_abs(), width = scale + 1);
            let (whole, fraction) = digits.split_at(digits.len() - scale);
            let sign = if unscaled < 0 { "-" } else { "" };
            Value::String(if scale == 0 {
                format!("{}{}", sign, whole)
            } else {
                format!("{}{}.{}", sign, whole, fraction)
            })
        }
    }
}

fn millis_timestamp(millis: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(
        millis.div_euclid(1000),
        (millis.rem_euclid(1000) * 1_000_000) as u32,
    )
}

fn micros_timestamp(micros: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(
        micros.div_euclid(1_000_000),
        (micros.rem_euclid(1_000_000) * 1000) as u32,
    )
}

/// A float as a decimal, from the shortest text that reads back as it (0.1f32 is 0.1)
fn float_value(f: f64, text: String) -> Value {
    text.parse::<Decimal>()
        .ok()
        .or_else(|| Decimal::try_from(f).ok())
        .map(Value::Decimal)
        .unwrap_or(Value::String(text))
}

/// An Avro value as a TinyETL value. `schema` is the value's schema, when known: it carries a
/// decimal's scale. Nested values (arrays, maps, records) become JSON.
fn avro_to_value(value: &AvroValue, schema: Option<&AvroSchema>) -> Value {
    match value {
        AvroValue::Null => Value::Null,
        AvroValue::Boolean(b) => Value::Boolean(*b),
        AvroValue::Int(i) => Value::Integer(*i as i64),
        AvroValue::Long(l) => Value::Integer(*l),
        AvroValue::Float(f) => float_value(*f as f64, f.to_string()),
        AvroValue::Double(d) => float_value(*d, d.to_string()),
        AvroValue::Bytes(b) | AvroValue::Fixed(_, b) => Value::String(hex_text(b)),
        AvroValue::String(s) => Value::String(s.clone()),
        AvroValue::Enum(_, symbol) => Value::String(symbol.clone()),
        AvroValue::Union(index, inner) => avro_to_value(inner, union_variant(schema, *index)),
        AvroValue::Array(_) | AvroValue::Map(_) | AvroValue::Record(_) => {
            Value::Json(avro_to_json(value, schema))
        }
        AvroValue::Date(days) => chrono::DateTime::from_timestamp(*days as i64 * 86_400, 0)
            .map(Value::Date)
            .unwrap_or(Value::Integer(*days as i64)),
        AvroValue::TimeMillis(ms) => {
            let ms = *ms as i64;
            time_text(
                ms.div_euclid(1000),
                (ms.rem_euclid(1000) * 1_000_000) as u32,
            )
            .map(Value::String)
            .unwrap_or(Value::Integer(ms))
        }
        AvroValue::TimeMicros(us) => time_text(
            us.div_euclid(1_000_000),
            (us.rem_euclid(1_000_000) * 1000) as u32,
        )
        .map(Value::String)
        .unwrap_or(Value::Integer(*us)),
        AvroValue::TimestampMillis(ms) | AvroValue::LocalTimestampMillis(ms) => {
            millis_timestamp(*ms)
                .map(Value::Date)
                .unwrap_or(Value::Integer(*ms))
        }
        AvroValue::TimestampMicros(us) | AvroValue::LocalTimestampMicros(us) => {
            micros_timestamp(*us)
                .map(Value::Date)
                .unwrap_or(Value::Integer(*us))
        }
        AvroValue::Decimal(decimal) => decimal_value(decimal, schema),
        AvroValue::Uuid(uuid) => Value::String(uuid.to_string()),
        AvroValue::Duration(duration) => Value::String(format!(
            "{} months {} days {} ms",
            u32::from(duration.months()),
            u32::from(duration.days()),
            u32::from(duration.millis())
        )),
    }
}

/// An Avro value as JSON, for nested values
fn avro_to_json(value: &AvroValue, schema: Option<&AvroSchema>) -> JsonValue {
    match value {
        AvroValue::Array(items) => {
            let schema = match schema {
                Some(AvroSchema::Array(items)) => Some(items.as_ref()),
                _ => None,
            };
            JsonValue::Array(items.iter().map(|v| avro_to_json(v, schema)).collect())
        }
        AvroValue::Map(map) => {
            let schema = match schema {
                Some(AvroSchema::Map(values)) => Some(values.as_ref()),
                _ => None,
            };
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            JsonValue::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), avro_to_json(&map[k], schema)))
                    .collect(),
            )
        }
        AvroValue::Record(fields) => {
            let field_schema = |name: &str| match schema {
                Some(AvroSchema::Record(record)) => record
                    .fields
                    .iter()
                    .find(|f| f.name == name)
                    .map(|f| &f.schema),
                _ => None,
            };
            JsonValue::Object(
                fields
                    .iter()
                    .map(|(name, v)| (name.clone(), avro_to_json(v, field_schema(name))))
                    .collect(),
            )
        }
        AvroValue::Union(index, inner) => avro_to_json(inner, union_variant(schema, *index)),
        other => match avro_to_value(other, schema) {
            Value::Null => JsonValue::Null,
            Value::Boolean(b) => JsonValue::Bool(b),
            Value::Integer(i) => JsonValue::from(i),
            Value::Decimal(d) => serde_json::from_str(&d.to_string())
                .unwrap_or_else(|_| JsonValue::String(d.to_string())),
            Value::String(s) => JsonValue::String(s),
            Value::Date(dt) => JsonValue::String(dt.to_rfc3339()),
            Value::Json(j) => j,
        },
    }
}

#[async_trait]
impl Source for AvroSource {
    async fn connect(&mut self) -> Result<()> {
        self.open()
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if self.records.is_none() {
            self.open()?;
        }
        Ok(Schema {
            columns: self.columns.clone(),
            // The container format has no record count without reading every block
            estimated_rows: None,
            primary_key_candidate: None,
        })
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let Some(records) = self.records.as_mut() else {
            return Ok(Vec::new());
        };
        let mut rows = Vec::with_capacity(batch_size.min(64 * 1024));
        for value in records.by_ref().take(batch_size) {
            let value = value.map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to read Avro record: {}", e))
            })?;
            if let AvroValue::Record(fields) = value {
                let mut row = HashMap::with_capacity(fields.len());
                for (name, field_value) in fields {
                    let value = avro_to_value(&field_value, self.field_schemas.get(&name));
                    row.insert(name, value);
                }
                rows.push(row);
            }
        }
        self.consumed |= !rows.is_empty();
        self.exhausted = records.peek().is_none();
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        // Exact, from the record count in each block header; the data itself is skipped
        Ok(count_records(&self.file_path).ok())
    }

    async fn reset(&mut self) -> Result<()> {
        if self.consumed || self.records.is_none() {
            self.open()?;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        !self.exhausted
    }
}

/// Writes an Avro container file, appending each batch as it arrives.
pub struct AvroTarget {
    file_path: PathBuf,
    /// TinyETL's columns, in the Avro schema's field order
    columns: Vec<crate::schema::Column>,
    writer: Option<Writer<'static, BufWriter<File>>>,
    output: Option<OutputFile>,
}

impl AvroTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Ok(Self {
            file_path: PathBuf::from(file_path),
            columns: Vec::new(),
            writer: None,
            output: None,
        })
    }

    fn record(&self, row: &Row) -> Result<AvroValue> {
        let mut fields = Vec::with_capacity(self.columns.len());
        for column in &self.columns {
            let value = row.get(&column.name).unwrap_or(&Value::Null);
            // A Null-typed column is written as a nullable string (see schema_to_avro_schema)
            let avro_value = match column.data_type {
                DataType::Null => Self::value_to_avro_value(value, &DataType::String, true)?,
                ref data_type => Self::value_to_avro_value(value, data_type, column.nullable)?,
            };
            fields.push((column.name.clone(), avro_value));
        }
        Ok(AvroValue::Record(fields))
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
                }
                DataType::Integer => {
                    if column.nullable {
                        json!(["null", "long"])
                    } else {
                        json!("long")
                    }
                }
                DataType::Decimal => {
                    if column.nullable {
                        json!(["null", "double"])
                    } else {
                        json!("double")
                    }
                }
                DataType::Boolean => {
                    if column.nullable {
                        json!(["null", "boolean"])
                    } else {
                        json!("boolean")
                    }
                }
                DataType::Date => {
                    if column.nullable {
                        json!(["null", {"type": "int", "logicalType": "date"}])
                    } else {
                        json!({"type": "int", "logicalType": "date"})
                    }
                }
                DataType::DateTime => {
                    if column.nullable {
                        json!(["null", {"type": "long", "logicalType": "timestamp-millis"}])
                    } else {
                        json!({"type": "long", "logicalType": "timestamp-millis"})
                    }
                }
                DataType::Json => {
                    // Avro stores JSON as string
                    if column.nullable {
                        json!(["null", "string"])
                    } else {
                        json!("string")
                    }
                }
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

        AvroSchema::parse(&avro_schema_json).map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to create Avro schema: {}", e))
        })
    }

    fn value_to_avro_value(
        value: &Value,
        data_type: &DataType,
        nullable: bool,
    ) -> Result<AvroValue> {
        // Handle null values - always wrap in Union for nullable fields
        if matches!(value, Value::Null) {
            if nullable {
                return Ok(AvroValue::Union(0, Box::new(AvroValue::Null)));
            } else {
                return Err(TinyEtlError::DataTransfer(
                    "Cannot insert null value into non-nullable field".to_string(),
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
            }
            (Value::Boolean(b), DataType::Boolean) => AvroValue::Boolean(*b),
            (Value::Date(dt), DataType::Date) => {
                let days_since_epoch = (dt.timestamp() / 86400) as i32;
                AvroValue::Date(days_since_epoch)
            }
            (Value::Date(dt), DataType::DateTime) => {
                AvroValue::TimestampMillis(dt.timestamp_millis())
            }
            // Type conversion fallbacks
            (Value::String(s), DataType::Integer) => {
                let parsed = s.parse::<i64>().map_err(|_| {
                    TinyEtlError::DataTransfer(format!("Cannot convert '{}' to integer", s))
                })?;
                AvroValue::Long(parsed)
            }
            (Value::String(s), DataType::Decimal) => {
                let parsed = s.parse::<Decimal>().map_err(|_| {
                    TinyEtlError::DataTransfer(format!("Cannot convert '{}' to decimal", s))
                })?;
                let f: f64 = parsed.try_into().unwrap_or(0.0);
                AvroValue::Double(f)
            }
            (Value::String(s), DataType::Boolean) => {
                let parsed = s.parse::<bool>().map_err(|_| {
                    TinyEtlError::DataTransfer(format!("Cannot convert '{}' to boolean", s))
                })?;
                AvroValue::Boolean(parsed)
            }
            (Value::Integer(i), DataType::String) => AvroValue::String(i.to_string()),
            (Value::Decimal(d), DataType::String) => AvroValue::String(d.to_string()),
            (Value::Boolean(b), DataType::String) => AvroValue::String(b.to_string()),
            (Value::Date(dt), DataType::String) => AvroValue::String(dt.to_rfc3339()),
            // JSON is stored as text
            (Value::Json(j), DataType::Json | DataType::String) => AvroValue::String(j.to_string()),
            (Value::String(s), DataType::Json) => AvroValue::String(s.clone()),
            _ => {
                return Err(TinyEtlError::DataTransfer(format!(
                    "Cannot convert value {:?} to Avro type {:?}",
                    value, data_type
                )))
            }
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
        // The writer borrows its schema for its whole life; one schema per run is kept
        // allocated (a few KB) rather than adding a self-referencing struct
        let avro_schema: &'static AvroSchema =
            Box::leak(Box::new(Self::schema_to_avro_schema(schema)?));
        let (output, file) = OutputFile::create(&self.file_path)?;
        self.writer = Some(Writer::new(avro_schema, BufWriter::new(file)));
        self.output = Some(output);
        self.columns = schema.columns.clone();
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        let records = rows
            .iter()
            .map(|row| self.record(row))
            .collect::<Result<Vec<_>>>()?;
        let writer = self.writer.as_mut().ok_or_else(|| {
            TinyEtlError::Configuration(
                "Schema not initialized. Call create_table() first.".to_string(),
            )
        })?;
        writer.extend(records).map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to write Avro record: {}", e))
        })?;
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        let (Some(writer), Some(output)) = (self.writer.take(), self.output.take()) else {
            return Ok(());
        };
        let mut file = writer.into_inner().map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to flush Avro writer: {}", e))
        })?;
        file.flush()?;
        drop(file);
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
        // Avro files don't easily support append - would require reading existing file and merging
        // For simplicity, we return false to force truncation
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use tempfile::NamedTempFile;

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

        let schema = AvroSchema::parse(&schema_json).map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to create test schema: {}", e))
        })?;
        let file = temp_file.reopen()?;
        let mut writer = Writer::new(&schema, file);

        // Write test records
        let test_records = vec![
            AvroValue::Record(vec![
                ("id".to_string(), AvroValue::Long(1)),
                (
                    "name".to_string(),
                    AvroValue::String("John Doe".to_string()),
                ),
                (
                    "email".to_string(),
                    AvroValue::Union(
                        1,
                        Box::new(AvroValue::String("john@example.com".to_string())),
                    ),
                ),
                ("age".to_string(), AvroValue::Int(30)),
                ("salary".to_string(), AvroValue::Double(50000.0)),
                ("active".to_string(), AvroValue::Boolean(true)),
                (
                    "created_at".to_string(),
                    AvroValue::TimestampMillis(1609459200000),
                ), // 2021-01-01
            ]),
            AvroValue::Record(vec![
                ("id".to_string(), AvroValue::Long(2)),
                (
                    "name".to_string(),
                    AvroValue::String("Jane Smith".to_string()),
                ),
                (
                    "email".to_string(),
                    AvroValue::Union(0, Box::new(AvroValue::Null)),
                ),
                ("age".to_string(), AvroValue::Int(25)),
                ("salary".to_string(), AvroValue::Double(60000.0)),
                ("active".to_string(), AvroValue::Boolean(false)),
                (
                    "created_at".to_string(),
                    AvroValue::TimestampMillis(1609545600000),
                ), // 2021-01-02
            ]),
        ];

        for record in test_records {
            writer.append(record).map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to write test record: {}", e))
            })?;
        }

        writer.flush().map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to flush test writer: {}", e))
        })?;
        Ok(temp_file)
    }

    #[tokio::test]
    async fn test_avro_source_new() {
        let source = AvroSource::new("test.avro");
        assert!(source.is_ok());

        let source = source.unwrap();
        assert_eq!(source.file_path.to_string_lossy(), "test.avro");
        assert!(source.records.is_none());
        assert!(!source.has_more());
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
        assert!(source.has_more());
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
        assert_eq!(
            first_row.get("name"),
            Some(&Value::String("John Doe".to_string()))
        );
        assert_eq!(first_row.get("age"), Some(&Value::Integer(30)));
        assert_eq!(
            first_row.get("salary"),
            Some(&Value::Decimal(Decimal::new(50000, 0)))
        );
        assert_eq!(first_row.get("active"), Some(&Value::Boolean(true)));

        // Check second row
        let second_row = &rows[1];
        assert_eq!(second_row.get("id"), Some(&Value::Integer(2)));
        assert_eq!(
            second_row.get("name"),
            Some(&Value::String("Jane Smith".to_string()))
        );
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

        let rows = source.read_batch(10).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn test_record_count_from_block_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("count.avro");
        let schema = Schema {
            columns: vec![Column {
                name: "id".to_string(),
                data_type: DataType::Integer,
                nullable: false,
            }],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        let mut target = AvroTarget::new(path.to_str().unwrap()).unwrap();
        target.create_table("t", &schema).await.unwrap();
        for start in (0..50_000).step_by(10_000) {
            let rows: Vec<Row> = (start..start + 10_000)
                .map(|i| HashMap::from([("id".to_string(), Value::Integer(i))]))
                .collect();
            target.write_batch(&rows).await.unwrap();
        }
        target.finalize().await.unwrap();
        let source = AvroSource::new(path.to_str().unwrap()).unwrap();
        assert_eq!(source.estimated_row_count().await.unwrap(), Some(50_000));

        // Not an Avro file: no estimate rather than an error
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(source.estimated_row_count().await.unwrap(), None);
    }

    #[test]
    fn test_field_types_keep_logical_types() {
        let schema = AvroSchema::parse(&json!({
            "type": "record",
            "name": "R",
            "fields": [
                {"name": "s", "type": "string"},
                {"name": "n", "type": ["null", "long"]},
                {"name": "f", "type": "double"},
                {"name": "b", "type": "boolean"},
                {"name": "d", "type": {"type": "int", "logicalType": "date"}},
                {"name": "t", "type": ["null", {"type": "long", "logicalType": "timestamp-millis"}]},
                {"name": "u", "type": {"type": "long", "logicalType": "timestamp-micros"}}
            ]
        }))
        .unwrap();
        let columns = AvroSource::columns_from_schema(&schema);
        let types: Vec<(&str, DataType, bool)> = columns
            .iter()
            .map(|c| (c.name.as_str(), c.data_type.clone(), c.nullable))
            .collect();
        assert_eq!(
            types,
            vec![
                ("s", DataType::String, false),
                ("n", DataType::Integer, true),
                ("f", DataType::Decimal, false),
                ("b", DataType::Boolean, false),
                ("d", DataType::Date, false),
                ("t", DataType::DateTime, true),
                ("u", DataType::DateTime, false),
            ]
        );
    }

    #[tokio::test]
    #[allow(clippy::approx_constant)]
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

        let timestamp_result =
            AvroSource::avro_value_to_value(&AvroValue::TimestampMillis(1609459200000)).unwrap();
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
        assert!(target.writer.is_none());
    }

    #[tokio::test]
    async fn test_avro_writes_timestamps_dates_and_json() {
        use chrono::{TimeZone, Utc};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("typed.avro");
        let path = path.to_str().unwrap();
        let column = |name: &str, data_type| Column {
            name: name.to_string(),
            data_type,
            nullable: true,
        };
        let schema = Schema {
            columns: vec![
                column("at", DataType::DateTime),
                column("day", DataType::Date),
                column("doc", DataType::Json),
                column("empty", DataType::Null),
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        let at = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        let row: Row = HashMap::from([
            ("at".to_string(), Value::Date(at)),
            ("day".to_string(), Value::Date(at)),
            ("doc".to_string(), Value::Json(serde_json::json!({"a": 1}))),
            ("empty".to_string(), Value::Null),
        ]);

        let mut target = AvroTarget::new(path).unwrap();
        target.create_table("t", &schema).await.unwrap();
        target.write_batch(&[row]).await.unwrap();
        target.finalize().await.unwrap();

        let mut source = AvroSource::new(path).unwrap();
        source.connect().await.unwrap();
        let rows = source.read_batch(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["at"], Value::Date(at));
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
        assert!(target.writer.is_some());
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

        let schema_json: JsonValue =
            serde_json::from_str(&avro_schema.unwrap().canonical_form()).unwrap();

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
    #[allow(clippy::approx_constant)]
    async fn test_value_to_avro_value() {
        // Test basic conversions with nullable=true
        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Null, &DataType::String, true).unwrap(),
            AvroValue::Union(0, Box::new(AvroValue::Null))
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::String("test".to_string()),
                &DataType::String,
                true
            )
            .unwrap(),
            AvroValue::Union(1, Box::new(AvroValue::String("test".to_string())))
        );

        // Test basic conversions with nullable=false
        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::String("test".to_string()),
                &DataType::String,
                false
            )
            .unwrap(),
            AvroValue::String("test".to_string())
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Integer(42), &DataType::Integer, false)
                .unwrap(),
            AvroValue::Long(42)
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::Decimal(Decimal::new(314, 2)),
                &DataType::Decimal,
                false
            )
            .unwrap(),
            AvroValue::Double(3.14)
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::Boolean, false)
                .unwrap(),
            AvroValue::Boolean(true)
        );

        // Test type conversions
        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::String("42".to_string()),
                &DataType::Integer,
                false
            )
            .unwrap(),
            AvroValue::Long(42)
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::String("3.14".to_string()),
                &DataType::Decimal,
                false
            )
            .unwrap(),
            AvroValue::Double(3.14)
        );

        assert_eq!(
            AvroTarget::value_to_avro_value(
                &Value::String("true".to_string()),
                &DataType::Boolean,
                false
            )
            .unwrap(),
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

        let result =
            AvroTarget::value_to_avro_value(&date_value, &DataType::DateTime, false).unwrap();
        if let AvroValue::TimestampMillis(_) = result {
            // Success
        } else {
            panic!("Expected TimestampMillis value");
        }
    }

    #[tokio::test]
    async fn test_value_to_avro_value_invalid_conversions() {
        // Test invalid string to int conversion
        let result = AvroTarget::value_to_avro_value(
            &Value::String("not_a_number".to_string()),
            &DataType::Integer,
            false,
        );
        assert!(result.is_err());

        // Test invalid string to float conversion
        let result = AvroTarget::value_to_avro_value(
            &Value::String("not_a_float".to_string()),
            &DataType::Decimal,
            false,
        );
        assert!(result.is_err());

        // Test invalid string to bool conversion
        let result = AvroTarget::value_to_avro_value(
            &Value::String("not_a_bool".to_string()),
            &DataType::Boolean,
            false,
        );
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
        assert_eq!(
            read_rows[0].get("name"),
            Some(&Value::String("John".to_string()))
        );
        assert_eq!(read_rows[0].get("active"), Some(&Value::Boolean(true)));

        assert_eq!(read_rows[1].get("id"), Some(&Value::Integer(2)));
        assert_eq!(
            read_rows[1].get("name"),
            Some(&Value::String("Jane".to_string()))
        );
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
        assert_eq!(result, Value::String("0x010203".to_string()));

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
        assert_eq!(result, Value::Json(json!([1, 2, 3])));

        // Test Map
        let mut map = std::collections::HashMap::new();
        map.insert("key1".to_string(), AvroValue::String("value1".to_string()));
        map.insert("key2".to_string(), AvroValue::Int(42));
        let map_value = AvroValue::Map(map);
        let result = AvroSource::avro_value_to_value(&map_value).unwrap();
        assert_eq!(result, Value::Json(json!({"key1": "value1", "key2": 42})));

        // Test Record
        let record_value = AvroValue::Record(vec![
            ("field1".to_string(), AvroValue::String("test".to_string())),
            ("field2".to_string(), AvroValue::Int(123)),
        ]);
        let result = AvroSource::avro_value_to_value(&record_value).unwrap();
        assert_eq!(
            result,
            Value::Json(json!({"field1": "test", "field2": 123}))
        );

        // Test TimeMillis
        let time_millis = AvroValue::TimeMillis(3600000); // 1 hour
        let result = AvroSource::avro_value_to_value(&time_millis).unwrap();
        assert_eq!(result, Value::String("01:00:00".to_string()));

        // Test TimeMicros
        let time_micros = AvroValue::TimeMicros(3600000000); // 1 hour
        let result = AvroSource::avro_value_to_value(&time_micros).unwrap();
        assert_eq!(result, Value::String("01:00:00".to_string()));

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
        // Without the field's schema there's no scale: the unscaled value, 0x01020304
        let result = AvroSource::avro_value_to_value(&decimal_value).unwrap();
        assert_eq!(result, Value::Decimal(Decimal::from(0x01020304)));

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

    #[tokio::test]
    async fn test_avro_target_value_conversion_edge_cases() {
        // Test Integer to String
        let result =
            AvroTarget::value_to_avro_value(&Value::Integer(42), &DataType::String, false).unwrap();
        assert_eq!(result, AvroValue::String("42".to_string()));

        // Test Decimal to String
        let result = AvroTarget::value_to_avro_value(
            &Value::Decimal(Decimal::new(314, 2)),
            &DataType::String,
            false,
        )
        .unwrap();
        assert_eq!(result, AvroValue::String("3.14".to_string()));

        // Test Boolean to String
        let result =
            AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::String, false)
                .unwrap();
        assert_eq!(result, AvroValue::String("true".to_string()));

        // Test invalid type combination
        let result =
            AvroTarget::value_to_avro_value(&Value::Boolean(true), &DataType::Integer, false);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_avro_target_many_small_batches() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = AvroTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let schema = Schema {
            columns: vec![Column {
                name: "id".to_string(),
                data_type: DataType::Integer,
                nullable: false,
            }],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        target.connect().await.unwrap();
        target.create_table("test", &schema).await.unwrap();

        // Write multiple small batches
        for i in 0..5 {
            let row = std::collections::HashMap::from([("id".to_string(), Value::Integer(i))]);
            target.write_batch(&[row]).await.unwrap();
        }

        // Rows go to the writer as they arrive; the file appears on finalize
        target.finalize().await.unwrap();
        let mut source = AvroSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        assert_eq!(source.read_batch(10).await.unwrap().len(), 5);
    }

    #[test]
    fn test_avro_json_value_float_edge_cases() {
        // JSON has no NaN: it's kept as text rather than becoming a different number
        let result = AvroSource::avro_value_to_json_value(&AvroValue::Float(f32::NAN)).unwrap();
        assert_eq!(result, JsonValue::String("NaN".to_string()));
        let result = AvroSource::avro_value_to_json_value(&AvroValue::Double(f64::NAN)).unwrap();
        assert_eq!(result, JsonValue::String("NaN".to_string()));
    }

    #[tokio::test]
    async fn test_avro_schema_all_types() {
        let schema = Schema {
            columns: vec![
                Column {
                    name: "col_string".to_string(),
                    data_type: DataType::String,
                    nullable: false,
                },
                Column {
                    name: "col_string_null".to_string(),
                    data_type: DataType::String,
                    nullable: true,
                },
                Column {
                    name: "col_int".to_string(),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: "col_int_null".to_string(),
                    data_type: DataType::Integer,
                    nullable: true,
                },
                Column {
                    name: "col_decimal".to_string(),
                    data_type: DataType::Decimal,
                    nullable: false,
                },
                Column {
                    name: "col_decimal_null".to_string(),
                    data_type: DataType::Decimal,
                    nullable: true,
                },
                Column {
                    name: "col_bool".to_string(),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
                Column {
                    name: "col_bool_null".to_string(),
                    data_type: DataType::Boolean,
                    nullable: true,
                },
                Column {
                    name: "col_date".to_string(),
                    data_type: DataType::Date,
                    nullable: false,
                },
                Column {
                    name: "col_date_null".to_string(),
                    data_type: DataType::Date,
                    nullable: true,
                },
                Column {
                    name: "col_datetime".to_string(),
                    data_type: DataType::DateTime,
                    nullable: false,
                },
                Column {
                    name: "col_datetime_null".to_string(),
                    data_type: DataType::DateTime,
                    nullable: true,
                },
                Column {
                    name: "col_null".to_string(),
                    data_type: DataType::Null,
                    nullable: true,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        let avro_schema = AvroTarget::schema_to_avro_schema(&schema);
        assert!(avro_schema.is_ok());

        let schema_json: JsonValue =
            serde_json::from_str(&avro_schema.unwrap().canonical_form()).unwrap();

        if let JsonValue::Object(obj) = schema_json {
            if let Some(JsonValue::Array(fields)) = obj.get("fields") {
                assert_eq!(fields.len(), 13);
            }
        }
    }

    #[tokio::test]
    async fn test_avro_source_array_conversion_error() {
        // Create an array with values that can't convert to JSON
        let array_with_complex = AvroValue::Array(vec![AvroValue::String("test".to_string())]);
        let result = AvroSource::avro_value_to_value(&array_with_complex).unwrap();
        if let Value::String(s) = result {
            assert!(s.contains("test"));
        }
    }
}
