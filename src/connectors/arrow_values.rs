//! Convert Arrow record batches into TinyETL rows.
//!
//! Used by every source that reads Arrow data (Parquet files, Snowflake results). Each Arrow type
//! maps to the closest [`Value`]; types without one are kept as their Arrow text form, so no
//! data is lost.

use arrow::array::*;
use arrow::datatypes::{DataType, Field, TimeUnit};
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::{
    schema::{Row, Value},
    Result, TinyEtlError,
};

/// Whether a column holds JSON text that should become [`Value::Json`]: TinyETL's own marker
/// (`tinyetl:type = json`, written by the Parquet target) or Snowflake's semi-structured types
/// (`logicalType` = VARIANT / OBJECT / ARRAY).
pub(crate) fn is_json_field(field: &Field) -> bool {
    let metadata = field.metadata();
    metadata.get("tinyetl:type").is_some_and(|t| t == "json")
        || metadata
            .get("logicalType")
            .is_some_and(|t| matches!(t.as_str(), "VARIANT" | "OBJECT" | "ARRAY"))
}

/// Convert every row of `batch`.
pub(crate) fn record_batch_to_rows(batch: &RecordBatch) -> Result<Vec<Row>> {
    let schema = batch.schema();
    let mut rows = vec![Row::new(); batch.num_rows()];
    for (field, column) in schema.fields().iter().zip(batch.columns()) {
        let json = is_json_field(field);
        let formatter = ArrayFormatter::try_new(column.as_ref(), &FormatOptions::default())
            .map_err(|e| TinyEtlError::DataTransfer(format!("Unsupported Arrow column: {}", e)))?;
        for (index, row) in rows.iter_mut().enumerate() {
            let value = value_at(column.as_ref(), index, json, &formatter);
            row.insert(field.name().clone(), value);
        }
    }
    Ok(rows)
}

/// The value at `index` of `array`.
fn value_at(array: &dyn Array, index: usize, json: bool, formatter: &ArrayFormatter) -> Value {
    if array.is_null(index) {
        return Value::Null;
    }
    let as_text = || formatter.value(index).to_string();

    match array.data_type() {
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
            let text = as_text();
            if json {
                serde_json::from_str(&text)
                    .map(Value::Json)
                    .unwrap_or(Value::String(text))
            } else {
                Value::String(text)
            }
        }
        DataType::Boolean => Value::Boolean(array.as_boolean().value(index)),
        DataType::Int8 => {
            Value::Integer(array.as_primitive::<types::Int8Type>().value(index) as i64)
        }
        DataType::Int16 => {
            Value::Integer(array.as_primitive::<types::Int16Type>().value(index) as i64)
        }
        DataType::Int32 => {
            Value::Integer(array.as_primitive::<types::Int32Type>().value(index) as i64)
        }
        DataType::Int64 => Value::Integer(array.as_primitive::<types::Int64Type>().value(index)),
        DataType::UInt8 => {
            Value::Integer(array.as_primitive::<types::UInt8Type>().value(index) as i64)
        }
        DataType::UInt16 => {
            Value::Integer(array.as_primitive::<types::UInt16Type>().value(index) as i64)
        }
        DataType::UInt32 => {
            Value::Integer(array.as_primitive::<types::UInt32Type>().value(index) as i64)
        }
        // A Decimal column (past i64::MAX), so every value is one
        DataType::UInt64 => Value::Decimal(Decimal::from(
            array.as_primitive::<types::UInt64Type>().value(index),
        )),
        DataType::Float16 | DataType::Float32 | DataType::Float64 => {
            let v = match array.data_type() {
                DataType::Float16 => array
                    .as_primitive::<types::Float16Type>()
                    .value(index)
                    .to_f64(),
                DataType::Float32 => array.as_primitive::<types::Float32Type>().value(index) as f64,
                _ => array.as_primitive::<types::Float64Type>().value(index),
            };
            Decimal::try_from(v)
                .map(Value::Decimal)
                .unwrap_or_else(|_| Value::String(v.to_string()))
        }
        DataType::Decimal128(_, scale) => {
            let raw = array.as_primitive::<types::Decimal128Type>().value(index);
            u32::try_from(*scale)
                .ok()
                .and_then(|scale| Decimal::try_from_i128_with_scale(raw, scale).ok())
                .map(Value::Decimal)
                .unwrap_or_else(|| Value::String(as_text()))
        }
        DataType::Date32 => {
            let days = array.as_primitive::<types::Date32Type>().value(index) as i64;
            timestamp(days * 86_400, 0).unwrap_or_else(|| Value::String(as_text()))
        }
        DataType::Date64 => {
            let ms = array.as_primitive::<types::Date64Type>().value(index);
            timestamp(
                ms.div_euclid(1000),
                (ms.rem_euclid(1000) * 1_000_000) as u32,
            )
            .unwrap_or_else(|| Value::String(as_text()))
        }
        DataType::Timestamp(unit, _) => {
            let (raw, per_second) = match unit {
                TimeUnit::Second => (
                    array
                        .as_primitive::<types::TimestampSecondType>()
                        .value(index),
                    1,
                ),
                TimeUnit::Millisecond => (
                    array
                        .as_primitive::<types::TimestampMillisecondType>()
                        .value(index),
                    1_000,
                ),
                TimeUnit::Microsecond => (
                    array
                        .as_primitive::<types::TimestampMicrosecondType>()
                        .value(index),
                    1_000_000,
                ),
                TimeUnit::Nanosecond => (
                    array
                        .as_primitive::<types::TimestampNanosecondType>()
                        .value(index),
                    1_000_000_000,
                ),
            };
            let nanos_per_tick = 1_000_000_000 / per_second;
            timestamp(
                raw.div_euclid(per_second),
                (raw.rem_euclid(per_second) * nanos_per_tick) as u32,
            )
            .unwrap_or_else(|| Value::String(as_text()))
        }
        DataType::Binary => Value::String(hex_text(array.as_binary::<i32>().value(index))),
        DataType::LargeBinary => Value::String(hex_text(array.as_binary::<i64>().value(index))),
        DataType::BinaryView => Value::String(hex_text(array.as_binary_view().value(index))),
        DataType::FixedSizeBinary(_) => {
            Value::String(hex_text(array.as_fixed_size_binary().value(index)))
        }
        DataType::List(_)
        | DataType::LargeList(_)
        | DataType::FixedSizeList(_, _)
        | DataType::Struct(_)
        | DataType::Map(_, _) => match json_at(array, index) {
            Ok(json) => Value::Json(json),
            Err(_) => Value::String(as_text()),
        },
        // Times, durations, Decimal256, dictionaries...: keep Arrow's text form
        _ => Value::String(as_text()),
    }
}

/// Binary data as text: `0x` and lowercase hex, as every connector writes it
fn hex_text(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// The value at `index` as JSON, for nested values (lists, structs, maps)
fn json_at(array: &dyn Array, index: usize) -> std::result::Result<serde_json::Value, String> {
    use serde_json::Value as Json;
    if array.is_null(index) {
        return Ok(Json::Null);
    }
    let elements = |values: ArrayRef| -> std::result::Result<Json, String> {
        (0..values.len())
            .map(|i| json_at(values.as_ref(), i))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(Json::Array)
    };
    match array.data_type() {
        DataType::List(_) => elements(array.as_list::<i32>().value(index)),
        DataType::LargeList(_) => elements(array.as_list::<i64>().value(index)),
        DataType::FixedSizeList(_, _) => elements(array.as_fixed_size_list().value(index)),
        DataType::Struct(fields) => {
            let array = array.as_struct();
            let mut object = serde_json::Map::new();
            for (field, column) in fields.iter().zip(array.columns()) {
                object.insert(field.name().clone(), json_at(column.as_ref(), index)?);
            }
            Ok(Json::Object(object))
        }
        DataType::Map(_, _) => {
            let entries = array.as_map().value(index);
            let (keys, values) = (entries.column(0), entries.column(1));
            let mut object = serde_json::Map::new();
            for i in 0..entries.len() {
                let key = match json_at(keys.as_ref(), i)? {
                    Json::String(s) => s,
                    other => other.to_string(),
                };
                object.insert(key, json_at(values.as_ref(), i)?);
            }
            Ok(Json::Object(object))
        }
        _ => {
            let formatter = ArrayFormatter::try_new(array, &FormatOptions::default())
                .map_err(|e| e.to_string())?;
            Ok(match value_at(array, index, false, &formatter) {
                Value::Null => Json::Null,
                Value::Boolean(b) => Json::Bool(b),
                Value::Integer(i) => Json::from(i),
                Value::Decimal(d) => {
                    serde_json::from_str(&d.to_string()).unwrap_or(Json::String(d.to_string()))
                }
                Value::String(s) => Json::String(s),
                Value::Date(dt) => Json::String(dt.to_rfc3339()),
                Value::Json(j) => j,
            })
        }
    }
}

fn timestamp(seconds: i64, nanos: u32) -> Option<Value> {
    DateTime::<Utc>::from_timestamp(seconds, nanos).map(Value::Date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Schema;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;

    fn one_column(field: Field, array: ArrayRef) -> Vec<Row> {
        let batch = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![array]).unwrap();
        record_batch_to_rows(&batch).unwrap()
    }

    #[test]
    fn test_numbers_and_decimals() {
        let rows = one_column(
            Field::new("n", DataType::Int16, true),
            Arc::new(Int16Array::from(vec![Some(7), None])),
        );
        assert_eq!(rows[0]["n"], Value::Integer(7));
        assert_eq!(rows[1]["n"], Value::Null);

        let rows = one_column(
            Field::new("d", DataType::Decimal128(10, 3), true),
            Arc::new(
                Decimal128Array::from(vec![12345, -5])
                    .with_precision_and_scale(10, 3)
                    .unwrap(),
            ),
        );
        assert_eq!(
            rows[0]["d"],
            Value::Decimal(Decimal::from_str("12.345").unwrap())
        );
        assert_eq!(
            rows[1]["d"],
            Value::Decimal(Decimal::from_str("-0.005").unwrap())
        );

        let rows = one_column(
            Field::new("f", DataType::Float32, true),
            Arc::new(Float32Array::from(vec![1.5])),
        );
        assert_eq!(
            rows[0]["f"],
            Value::Decimal(Decimal::from_str("1.5").unwrap())
        );
    }

    #[test]
    fn test_dates_and_timestamps() {
        let expected = Value::Date(
            DateTime::parse_from_rfc3339("2024-01-15T10:30:00.25Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let ms = 1_705_314_600_250i64;
        let rows = one_column(
            Field::new("t", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Arc::new(TimestampMillisecondArray::from(vec![ms])),
        );
        assert_eq!(rows[0]["t"], expected);
        let rows = one_column(
            Field::new(
                "t",
                DataType::Timestamp(TimeUnit::Microsecond, Some("+00:00".into())),
                true,
            ),
            Arc::new(TimestampMicrosecondArray::from(vec![ms * 1000]).with_timezone("+00:00")),
        );
        assert_eq!(rows[0]["t"], expected);

        // 2024-01-15 as days since the epoch
        let rows = one_column(
            Field::new("d", DataType::Date32, true),
            Arc::new(Date32Array::from(vec![19737])),
        );
        assert_eq!(
            rows[0]["d"],
            Value::Date(
                DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn test_json_columns() {
        let snowflake_variant = Field::new("v", DataType::Utf8, true).with_metadata(HashMap::from(
            [("logicalType".to_string(), "VARIANT".to_string())],
        ));
        let rows = one_column(
            snowflake_variant,
            Arc::new(StringArray::from(vec![r#"{"a": [1, 2]}"#, "not json"])),
        );
        assert_eq!(rows[0]["v"], Value::Json(serde_json::json!({"a": [1, 2]})));
        assert_eq!(rows[1]["v"], Value::String("not json".into()));

        let plain = one_column(
            Field::new("s", DataType::Utf8, true),
            Arc::new(StringArray::from(vec![r#"{"a": 1}"#])),
        );
        assert_eq!(plain[0]["s"], Value::String(r#"{"a": 1}"#.into()));
    }

    #[test]
    fn test_other_types_keep_their_text() {
        let rows = one_column(
            Field::new("t", DataType::Time64(TimeUnit::Microsecond), true),
            Arc::new(Time64MicrosecondArray::from(vec![3_723_000_000])),
        );
        assert_eq!(rows[0]["t"], Value::String("01:02:03".into()));
    }
}
