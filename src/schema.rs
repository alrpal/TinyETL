use crate::Result;
use arrow::datatypes::{DataType as ArrowDataType, Field, TimeUnit};
use chrono::{DateTime, Utc};
use regex::Regex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Wrapper around Arrow DataType for schema definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    String,
    Integer,
    Decimal,
    Boolean,
    Date,
    DateTime,
    Json,
    Null,
}

impl DataType {
    /// Convert to Arrow DataType
    pub fn to_arrow(&self) -> ArrowDataType {
        match self {
            DataType::String => ArrowDataType::Utf8,
            DataType::Integer => ArrowDataType::Int64,
            DataType::Decimal => ArrowDataType::Float64, // Could also use Decimal128
            DataType::Boolean => ArrowDataType::Boolean,
            DataType::Date => ArrowDataType::Date64,
            DataType::DateTime => ArrowDataType::Timestamp(TimeUnit::Nanosecond, None),
            DataType::Json => ArrowDataType::Utf8, // Store JSON as string in Arrow
            DataType::Null => ArrowDataType::Null,
        }
    }

    /// Convert from Arrow DataType
    pub fn from_arrow(arrow_type: &ArrowDataType) -> Self {
        match arrow_type {
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => DataType::String,
            ArrowDataType::Int8
            | ArrowDataType::Int16
            | ArrowDataType::Int32
            | ArrowDataType::Int64 => DataType::Integer,
            ArrowDataType::UInt8
            | ArrowDataType::UInt16
            | ArrowDataType::UInt32
            | ArrowDataType::UInt64 => DataType::Integer,
            ArrowDataType::Float16 | ArrowDataType::Float32 | ArrowDataType::Float64 => {
                DataType::Decimal
            }
            ArrowDataType::Decimal128(_, _) | ArrowDataType::Decimal256(_, _) => DataType::Decimal,
            ArrowDataType::Boolean => DataType::Boolean,
            ArrowDataType::Date32 | ArrowDataType::Date64 => DataType::Date,
            ArrowDataType::Timestamp(_, _) => DataType::DateTime,
            ArrowDataType::Null => DataType::Null,
            _ => DataType::String, // Default to string for complex types
        }
    }
}

fn default_nullable() -> bool {
    true
}

/// What a schema file does with a value that doesn't parse as its column's declared type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnMismatch {
    /// Stop the transfer, naming the column and the value
    #[default]
    Fail,
    /// Write NULL; the transfer reports how many values it replaced
    Null,
    /// Keep the source text; files and SQLite store it, typed databases reject it
    Raw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFileColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default = "default_nullable")]
    pub nullable: bool,
    pub pattern: Option<String>,
    pub default: Option<String>,
    /// Overrides the schema file's `on_mismatch` for this column
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_mismatch: Option<OnMismatch>,
}

impl SchemaFileColumn {
    fn declared_type(&self) -> DataType {
        match self.data_type.to_lowercase().as_str() {
            "integer" => DataType::Integer,
            "decimal" => DataType::Decimal,
            "boolean" => DataType::Boolean,
            "date" => DataType::Date,
            "datetime" => DataType::DateTime,
            "json" => DataType::Json,
            _ => DataType::String, // `validate` rejects unknown names
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFile {
    /// The default for every column: `fail` (the default), `null` or `raw`
    #[serde(default, skip_serializing_if = "is_default_policy")]
    pub on_mismatch: OnMismatch,
    pub columns: Vec<SchemaFileColumn>,
}

fn is_default_policy(policy: &OnMismatch) -> bool {
    *policy == OnMismatch::Fail
}

impl SchemaFile {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::TinyEtlError::Configuration(format!(
                "Can't read schema file {}: {}",
                path.display(),
                e
            ))
        })?;
        let schema_file: SchemaFile = serde_yaml::from_str(&content).map_err(|e| {
            crate::TinyEtlError::Configuration(format!(
                "Invalid schema file {}: {}",
                path.display(),
                e
            ))
        })?;

        // Validate the schema file
        schema_file.validate()?;

        Ok(schema_file)
    }

    pub fn validate(&self) -> Result<()> {
        for column in &self.columns {
            // Validate data type
            match column.data_type.to_lowercase().as_str() {
                "string" | "integer" | "decimal" | "boolean" | "date" | "datetime" | "json" => {}
                _ => {
                    return Err(crate::TinyEtlError::Configuration(format!(
                        "Invalid data type '{}' for column '{}'",
                        column.data_type, column.name
                    )))
                }
            }

            // Validate regex pattern if provided
            if let Some(pattern) = &column.pattern {
                Regex::new(pattern).map_err(|e| {
                    crate::TinyEtlError::Configuration(format!(
                        "Invalid regex pattern '{}' for column '{}': {}",
                        pattern, column.name, e
                    ))
                })?;
            }
        }
        Ok(())
    }

    /// The schema file's columns alone, in its order
    pub fn to_schema(&self) -> Result<Schema> {
        let columns = self
            .columns
            .iter()
            .map(|col| Column {
                name: col.name.clone(),
                data_type: col.declared_type(),
                nullable: col.nullable,
            })
            .collect();

        Ok(Schema {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        })
    }

    /// The transfer's schema: the source's columns in the source's order, with the declared
    /// type and nullability for the columns this file lists. Columns it doesn't list keep their
    /// inferred type; listed columns the source doesn't have come last.
    pub fn apply_to(&self, inferred: &Schema) -> Schema {
        let declared = |name: &str| self.columns.iter().find(|c| c.name == name);
        let mut columns: Vec<Column> = inferred
            .columns
            .iter()
            .map(|column| match declared(&column.name) {
                Some(col) => Column {
                    name: col.name.clone(),
                    data_type: col.declared_type(),
                    nullable: col.nullable,
                },
                None => column.clone(),
            })
            .collect();
        for col in &self.columns {
            if !columns.iter().any(|c| c.name == col.name) {
                columns.push(Column {
                    name: col.name.clone(),
                    data_type: col.declared_type(),
                    nullable: col.nullable,
                });
            }
        }
        Schema {
            columns,
            estimated_rows: inferred.estimated_rows,
            primary_key_candidate: None,
        }
    }

    /// Give each listed column its declared type: apply defaults, check `nullable` and
    /// `pattern` (against the source text), and parse the source text as the declared type.
    /// A String column keeps the source text exactly. Returns how many values were replaced by
    /// NULL under `on_mismatch: null`.
    pub fn validate_and_transform_row(&self, row: &mut Row) -> Result<usize> {
        let mut nulled = 0;
        for schema_col in &self.columns {
            let missing = matches!(row.get(&schema_col.name), None | Some(Value::Null));
            if missing {
                if !schema_col.nullable {
                    let Some(default_str) = &schema_col.default else {
                        return Err(crate::TinyEtlError::DataValidation(format!(
                            "Required column '{}' is missing or null",
                            schema_col.name
                        )));
                    };
                    let default_value =
                        self.parse_default_value(default_str, &schema_col.data_type)?;
                    row.insert(schema_col.name.clone(), default_value);
                }
                continue;
            }

            let Some(slot) = row.get_mut(&schema_col.name) else {
                continue;
            };
            let declared = schema_col.declared_type();
            let value = std::mem::replace(slot, Value::Null);

            if let Some(pattern) = &schema_col.pattern {
                // The source text if there is one, else the value as text in a String column
                let text = match &value {
                    Value::String(s) => Some(s.clone()),
                    other if declared == DataType::String => other.to_string_for_arrow(),
                    _ => None,
                };
                if let Some(text) = text {
                    let regex = Regex::new(pattern).unwrap(); // Already validated in validate()
                    if !regex.is_match(&text) {
                        return Err(crate::TinyEtlError::DataValidation(format!(
                            "Column '{}' value '{}' does not match pattern '{}'",
                            schema_col.name, text, pattern
                        )));
                    }
                }
            }

            let converted = value.clone().coerce_to(&declared);
            if converted.fits(&declared) {
                *slot = converted;
                continue;
            }
            match schema_col.on_mismatch.unwrap_or(self.on_mismatch) {
                OnMismatch::Fail => {
                    let shown = value.to_string_for_arrow().unwrap_or_default();
                    return Err(crate::TinyEtlError::DataValidation(format!(
                        "Column '{}' value '{}' is not a valid {} (set on_mismatch: null or raw in the schema file to load it anyway)",
                        schema_col.name, shown, schema_col.data_type
                    )));
                }
                OnMismatch::Null if !schema_col.nullable => {
                    return Err(crate::TinyEtlError::DataValidation(format!(
                        "Column '{}' value '{}' is not a valid {}, and the column isn't nullable",
                        schema_col.name,
                        value.to_string_for_arrow().unwrap_or_default(),
                        schema_col.data_type
                    )));
                }
                OnMismatch::Null => nulled += 1, // the slot already holds NULL
                OnMismatch::Raw => *slot = value,
            }
        }
        Ok(nulled)
    }

    fn parse_default_value(&self, default_str: &str, data_type: &str) -> Result<Value> {
        match data_type.to_lowercase().as_str() {
            "string" => Ok(Value::String(default_str.to_string())),
            "integer" => {
                let parsed = default_str.parse::<i64>().map_err(|_| {
                    crate::TinyEtlError::Configuration(format!(
                        "Invalid default integer value: '{}'",
                        default_str
                    ))
                })?;
                Ok(Value::Integer(parsed))
            }
            "decimal" => {
                let parsed = default_str.parse::<Decimal>().map_err(|_| {
                    crate::TinyEtlError::Configuration(format!(
                        "Invalid default decimal value: '{}'",
                        default_str
                    ))
                })?;
                Ok(Value::Decimal(parsed))
            }
            "boolean" => {
                let parsed = default_str.parse::<bool>().map_err(|_| {
                    crate::TinyEtlError::Configuration(format!(
                        "Invalid default boolean value: '{}'",
                        default_str
                    ))
                })?;
                Ok(Value::Boolean(parsed))
            }
            "date" | "datetime" => {
                // Try to parse as RFC3339 first, then as date
                let parsed = chrono::DateTime::parse_from_rfc3339(default_str)
                    .or_else(|_| {
                        // Try parsing as date only
                        chrono::NaiveDate::parse_from_str(default_str, "%Y-%m-%d")
                            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().fixed_offset())
                    })
                    .map_err(|_| {
                        crate::TinyEtlError::Configuration(format!(
                            "Invalid default date/datetime value: '{}'",
                            default_str
                        ))
                    })?;
                Ok(Value::Date(parsed.with_timezone(&Utc)))
            }
            "json" => {
                let parsed = serde_json::from_str(default_str).map_err(|e| {
                    crate::TinyEtlError::Configuration(format!(
                        "Invalid default JSON value: '{}' - {}",
                        default_str, e
                    ))
                })?;
                Ok(Value::Json(parsed))
            }
            _ => Err(crate::TinyEtlError::Configuration(format!(
                "Unsupported data type for default value: '{}'",
                data_type
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

impl Column {
    /// Convert to Arrow Field
    pub fn to_arrow_field(&self) -> Field {
        let mut field = Field::new(&self.name, self.data_type.to_arrow(), self.nullable);

        // Add metadata for JSON type so we can preserve it when reading back from Parquet
        if matches!(self.data_type, DataType::Json) {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("tinyetl:type".to_string(), "json".to_string());
            field = field.with_metadata(metadata);
        }

        field
    }

    /// Convert from Arrow Field
    pub fn from_arrow_field(field: &Field) -> Self {
        Self {
            name: field.name().clone(),
            data_type: DataType::from_arrow(field.data_type()),
            nullable: field.is_nullable(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub columns: Vec<Column>,
    pub estimated_rows: Option<usize>,
    pub primary_key_candidate: Option<String>,
}

impl Schema {
    /// Convert to Arrow Schema
    pub fn to_arrow_schema(&self) -> arrow::datatypes::Schema {
        let fields: Vec<Field> = self
            .columns
            .iter()
            .map(|col| col.to_arrow_field())
            .collect();
        arrow::datatypes::Schema::new(fields)
    }

    /// Convert from Arrow Schema
    pub fn from_arrow_schema(arrow_schema: &arrow::datatypes::Schema) -> Self {
        let columns: Vec<Column> = arrow_schema
            .fields()
            .iter()
            .map(|field| Column::from_arrow_field(field))
            .collect();

        Self {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }
}

/// Value type compatible with Arrow data representation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(String),
    Integer(i64),
    Decimal(Decimal),
    Boolean(bool),
    Date(DateTime<Utc>),
    Json(serde_json::Value),
    Null,
}

impl Value {
    /// Get the corresponding Arrow DataType for this value
    pub fn arrow_type(&self) -> ArrowDataType {
        match self {
            Value::String(_) => ArrowDataType::Utf8,
            Value::Integer(_) => ArrowDataType::Int64,
            Value::Decimal(_) => ArrowDataType::Float64,
            Value::Boolean(_) => ArrowDataType::Boolean,
            Value::Date(_) => ArrowDataType::Timestamp(TimeUnit::Nanosecond, None),
            Value::Json(_) => ArrowDataType::Utf8, // Store JSON as string in Arrow
            Value::Null => ArrowDataType::Null,
        }
    }

    /// Convert to string representation for Arrow array building
    pub fn to_string_for_arrow(&self) -> Option<String> {
        match self {
            Value::String(s) => Some(s.clone()),
            Value::Integer(i) => Some(i.to_string()),
            Value::Decimal(d) => Some(d.to_string()),
            Value::Boolean(b) => Some(b.to_string()),
            Value::Date(dt) => Some(dt.to_rfc3339()),
            Value::Json(j) => Some(serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string())),
            Value::Null => None,
        }
    }

    /// Convert to i64 for Arrow array building
    pub fn to_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Convert to f64 for Arrow array building
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Value::Decimal(d) => (*d).try_into().ok(),
            _ => None,
        }
    }

    /// Convert to bool for Arrow array building
    pub fn to_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Convert to timestamp nanoseconds for Arrow array building
    pub fn to_timestamp_nanos(&self) -> Option<i64> {
        match self {
            Value::Date(dt) => dt.timestamp_nanos_opt(),
            _ => None,
        }
    }

    /// Convert to JSON value
    pub fn to_json(&self) -> Option<&serde_json::Value> {
        match self {
            Value::Json(j) => Some(j),
            _ => None,
        }
    }

    /// Whether this value can be stored as-is in a column of type `data_type`.
    pub fn fits(&self, data_type: &DataType) -> bool {
        matches!(
            (self, data_type),
            (Value::Null, _)
                | (Value::String(_), DataType::String | DataType::Null)
                | (Value::Integer(_), DataType::Integer)
                | (Value::Decimal(_), DataType::Decimal)
                | (Value::Boolean(_), DataType::Boolean)
                | (Value::Date(_), DataType::Date | DataType::DateTime)
                | (Value::Json(_), DataType::Json)
        )
    }

    /// Convert this value to `data_type` without losing information, so every value in a
    /// column has the column's type. Schemas are inferred from a sample, so later rows can
    /// disagree (an integer in a decimal column, a number in a text column).
    ///
    /// - Anything converts to text.
    /// - Text converts when it parses as the column's type; blank text in a non-text
    ///   column becomes NULL.
    /// - Integers widen to decimals; decimals with no fractional part narrow to integers.
    /// - Numbers and booleans become JSON scalars.
    ///
    /// A value that can't convert is returned unchanged; targets that need exact types
    /// report it.
    pub fn coerce_to(self, data_type: &DataType) -> Value {
        if self.fits(data_type) {
            return self;
        }
        match (self, data_type) {
            (value, DataType::String | DataType::Null) => {
                Value::String(value.to_string_for_arrow().unwrap_or_default())
            }
            (Value::String(s), _) if s.trim().is_empty() => Value::Null,
            (Value::String(s), DataType::Integer | DataType::Decimal)
                if !number_keeps_text(s.trim()) =>
            {
                Value::String(s)
            }
            (Value::String(s), DataType::Integer) => match s.trim().parse::<i64>() {
                Ok(i) => Value::Integer(i),
                Err(_) => Value::String(s),
            },
            (Value::String(s), DataType::Decimal) => match s.trim().parse::<Decimal>() {
                Ok(d) => Value::Decimal(d),
                Err(_) => Value::String(s),
            },
            (Value::String(s), DataType::Boolean) => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Value::Boolean(true),
                "false" => Value::Boolean(false),
                _ => Value::String(s),
            },
            (Value::String(s), DataType::Date | DataType::DateTime) => {
                match crate::date_parser::DateParser::try_parse(&s) {
                    Some(date @ Value::Date(_)) => date,
                    _ => Value::String(s),
                }
            }
            (Value::String(s), DataType::Json) => match serde_json::from_str(&s) {
                Ok(json) => Value::Json(json),
                Err(_) => Value::String(s),
            },
            (Value::Integer(i), DataType::Decimal) => Value::Decimal(Decimal::from(i)),
            (Value::Decimal(d), DataType::Integer) if d.fract().is_zero() => {
                match i64::try_from(d) {
                    Ok(i) => Value::Integer(i),
                    Err(_) => Value::Decimal(d),
                }
            }
            (Value::Integer(i), DataType::Json) => Value::Json(i.into()),
            (Value::Decimal(d), DataType::Json) => {
                match f64::try_from(d).ok().and_then(serde_json::Number::from_f64) {
                    Some(n) => Value::Json(n.into()),
                    None => Value::Decimal(d),
                }
            }
            (Value::Boolean(b), DataType::Json) => Value::Json(b.into()),
            (value, _) => value,
        }
    }
}

/// Whether `text` can become a number without changing how it reads: false for a leading `+`
/// or a leading zero before another digit (`01826`, `+15551234`, `0012.50`), which are codes
/// rather than quantities. `0`, `-7` and `0.5` are fine.
pub fn number_keeps_text(text: &str) -> bool {
    if text.starts_with('+') {
        return false;
    }
    let digits = text.strip_prefix('-').unwrap_or(text).as_bytes();
    !(digits.len() > 1 && digits[0] == b'0' && digits[1].is_ascii_digit())
}

/// Convert each value in `row` to its column's type with [`Value::coerce_to`].
pub fn coerce_row(row: &mut Row, schema: &Schema) {
    for column in &schema.columns {
        if let Some(value) = row.get_mut(&column.name) {
            if !value.fits(&column.data_type) {
                let owned = std::mem::replace(value, Value::Null);
                *value = owned.coerce_to(&column.data_type);
            }
        }
    }
}

pub type Row = HashMap<String, Value>;

pub struct SchemaInferer;

impl SchemaInferer {
    /// Infers a schema from sample rows. Rows are hash maps and carry no column order, so the
    /// columns come out sorted by name; sources that know their column order should call
    /// [`SchemaInferer::infer_from_rows_ordered`] instead.
    pub fn infer_from_rows(rows: &[Row]) -> Result<Schema> {
        Self::infer_from_rows_ordered(rows, &[])
    }

    /// Infers a schema from sample rows, with columns in `column_order` (for example a CSV
    /// header or a query's result columns). Columns found in the rows but missing from
    /// `column_order` follow, sorted by name; names in `column_order` that appear in no row
    /// are skipped.
    pub fn infer_from_rows_ordered(rows: &[Row], column_order: &[String]) -> Result<Schema> {
        if rows.is_empty() {
            // No data to infer types from (e.g. a CSV with only a header): keep the known
            // columns as nullable text, so targets still get a usable table
            return Ok(Schema {
                columns: column_order
                    .iter()
                    .map(|name| Column {
                        name: name.clone(),
                        data_type: DataType::String,
                        nullable: true,
                    })
                    .collect(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        let present: std::collections::HashSet<&String> =
            rows.iter().flat_map(|row| row.keys()).collect();
        let mut seen = std::collections::HashSet::new();
        let mut names: Vec<&String> = column_order
            .iter()
            .filter(|name| present.contains(name) && seen.insert(*name))
            .collect();
        let mut rest: Vec<&String> = present
            .into_iter()
            .filter(|name| !seen.contains(name))
            .collect();
        rest.sort();
        names.extend(rest);

        let columns = names
            .into_iter()
            .map(|col_name| {
                let types: Vec<DataType> = rows
                    .iter()
                    .map(|row| match row.get(col_name) {
                        // Blank text says nothing about the column's type; it becomes NULL
                        // in a non-text column (see Value::coerce_to)
                        Some(Value::String(s)) if s.trim().is_empty() => DataType::Null,
                        Some(value) => Self::infer_type(value),
                        None => DataType::Null,
                    })
                    .collect();
                let (data_type, nullable) = Self::resolve_column_type(&types);
                Column {
                    name: col_name.clone(),
                    data_type,
                    nullable,
                }
            })
            .collect();

        Ok(Schema {
            columns,
            estimated_rows: Some(rows.len()),
            primary_key_candidate: None, // TODO: Implement PK detection
        })
    }

    pub fn infer_type(value: &Value) -> DataType {
        match value {
            Value::String(_) => DataType::String,
            Value::Integer(_) => DataType::Integer,
            Value::Decimal(_) => DataType::Decimal,
            Value::Boolean(_) => DataType::Boolean,
            Value::Date(_) => DataType::DateTime,
            Value::Json(_) => DataType::Json,
            Value::Null => DataType::Null,
        }
    }

    pub fn resolve_column_type(types: &[DataType]) -> (DataType, bool) {
        // Filter out nulls for type determination
        let non_null_types: Vec<&DataType> = types
            .iter()
            .filter(|t| !matches!(t, DataType::Null))
            .collect();

        if non_null_types.is_empty() {
            return (DataType::String, true);
        }

        // If all non-null values are the same type, use that type
        let first_type = non_null_types[0];
        let resolved_type = if non_null_types
            .iter()
            .all(|t| std::mem::discriminant(*t) == std::mem::discriminant(first_type))
        {
            first_type.clone()
        } else if non_null_types
            .iter()
            .all(|t| matches!(t, DataType::Integer | DataType::Decimal))
        {
            // Whole and fractional numbers in one column (e.g. 84 and 29.85)
            DataType::Decimal
        } else {
            // Mixed types default to String
            DataType::String
        };

        // IMPORTANT: Always return nullable=true when inferring from sample data.
        // We cannot be certain that unseen data doesn't contain NULLs, and it's safer
        // for ETL operations to allow NULLs than to create NOT NULL constraints that
        // might be violated by subsequent batches or data that wasn't in the sample.
        (resolved_type, true)
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::String => write!(f, "TEXT"),
            DataType::Integer => write!(f, "INTEGER"),
            DataType::Decimal => write!(f, "DECIMAL"),
            DataType::Boolean => write!(f, "BOOLEAN"),
            DataType::Date => write!(f, "DATE"),
            DataType::DateTime => write!(f, "TIMESTAMP"),
            DataType::Json => write!(f, "JSON"),
            DataType::Null => write!(f, "NULL"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_empty_schema_inference() {
        let rows = Vec::new();
        let schema = SchemaInferer::infer_from_rows(&rows).unwrap();
        assert!(schema.columns.is_empty());
        assert_eq!(schema.estimated_rows, Some(0));
    }

    #[test]
    fn test_single_row_schema_inference() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), Value::Integer(1));
        row.insert("name".to_string(), Value::String("test".to_string()));
        row.insert("active".to_string(), Value::Boolean(true));

        let schema = SchemaInferer::infer_from_rows(&[row]).unwrap();
        assert_eq!(schema.columns.len(), 3);

        let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, DataType::Integer);
        assert!(id_col.nullable); // Always nullable when inferred from sample data
    }

    fn column_names(schema: &Schema) -> Vec<&str> {
        schema.columns.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn test_infer_from_rows_ordered_keeps_given_order() {
        let mut row1 = HashMap::new();
        row1.insert("last_name".to_string(), Value::String("Smith".to_string()));
        row1.insert("customer_id".to_string(), Value::Integer(1));
        row1.insert("zeta".to_string(), Value::Integer(9));
        let mut row2 = row1.clone();
        // A column only some rows have, missing from the order: goes after the ordered ones
        row2.insert("extra".to_string(), Value::Boolean(true));

        let order: Vec<String> = ["zeta", "customer_id", "not_in_rows", "last_name", "zeta"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let schema = SchemaInferer::infer_from_rows_ordered(&[row1, row2], &order).unwrap();

        // "not_in_rows" is skipped and the duplicate "zeta" appears once
        assert_eq!(
            column_names(&schema),
            ["zeta", "customer_id", "last_name", "extra"]
        );
        let extra = schema.columns.iter().find(|c| c.name == "extra").unwrap();
        assert_eq!(extra.data_type, DataType::Boolean);
        assert!(extra.nullable);
    }

    #[test]
    fn test_infer_from_rows_is_sorted_by_name() {
        let mut row = HashMap::new();
        for name in ["delta", "alpha", "charlie", "bravo"] {
            row.insert(name.to_string(), Value::Integer(1));
        }
        let schema = SchemaInferer::infer_from_rows(&[row]).unwrap();
        assert_eq!(
            column_names(&schema),
            ["alpha", "bravo", "charlie", "delta"]
        );
    }

    #[test]
    fn test_mixed_types_schema_inference() {
        let mut row1 = HashMap::new();
        row1.insert("value".to_string(), Value::Integer(1));

        let mut row2 = HashMap::new();
        row2.insert("value".to_string(), Value::String("hello".to_string()));

        let schema = SchemaInferer::infer_from_rows(&[row1, row2]).unwrap();
        let value_col = schema.columns.iter().find(|c| c.name == "value").unwrap();
        assert_eq!(value_col.data_type, DataType::String);
    }

    #[test]
    fn test_nullable_column_inference() {
        let mut row1 = HashMap::new();
        row1.insert("optional".to_string(), Value::String("present".to_string()));

        let mut row2 = HashMap::new();
        row2.insert("optional".to_string(), Value::Null);

        let schema = SchemaInferer::infer_from_rows(&[row1, row2]).unwrap();
        let optional_col = schema
            .columns
            .iter()
            .find(|c| c.name == "optional")
            .unwrap();
        assert_eq!(optional_col.data_type, DataType::String);
        assert!(optional_col.nullable);
    }

    #[test]
    fn test_data_type_display() {
        assert_eq!(DataType::String.to_string(), "TEXT");
        assert_eq!(DataType::Integer.to_string(), "INTEGER");
        assert_eq!(DataType::Decimal.to_string(), "DECIMAL");
        assert_eq!(DataType::Boolean.to_string(), "BOOLEAN");
        assert_eq!(DataType::Date.to_string(), "DATE");
        assert_eq!(DataType::DateTime.to_string(), "TIMESTAMP");
        assert_eq!(DataType::Json.to_string(), "JSON");
        assert_eq!(DataType::Null.to_string(), "NULL");
    }

    #[test]
    fn test_value_type_inference() {
        assert_eq!(
            SchemaInferer::infer_type(&Value::String("test".to_string())),
            DataType::String
        );
        assert_eq!(
            SchemaInferer::infer_type(&Value::Integer(42)),
            DataType::Integer
        );
        assert_eq!(
            SchemaInferer::infer_type(&Value::Decimal(Decimal::new(314, 2))),
            DataType::Decimal
        );
        assert_eq!(
            SchemaInferer::infer_type(&Value::Boolean(true)),
            DataType::Boolean
        );
        assert_eq!(
            SchemaInferer::infer_type(&Value::Date(Utc::now())),
            DataType::DateTime
        );
        assert_eq!(
            SchemaInferer::infer_type(&Value::Json(serde_json::json!({"key": "value"}))),
            DataType::Json
        );
        assert_eq!(SchemaInferer::infer_type(&Value::Null), DataType::Null);
    }

    #[test]
    fn test_column_type_resolution() {
        // All same type - but still nullable when inferred
        let types = vec![DataType::Integer, DataType::Integer, DataType::Integer];
        let (resolved_type, nullable) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::Integer);
        assert!(nullable); // Always nullable when inferring from sample data

        // With nulls
        let types = vec![DataType::String, DataType::Null, DataType::String];
        let (resolved_type, nullable) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::String);
        assert!(nullable);

        // All nulls
        let types = vec![DataType::Null, DataType::Null];
        let (resolved_type, nullable) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::String);
        assert!(nullable);

        // Whole and fractional numbers make a decimal column
        let types = vec![DataType::Decimal, DataType::Integer, DataType::Null];
        let (resolved_type, _) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::Decimal);

        // Numbers mixed with anything else stay text
        let types = vec![DataType::Integer, DataType::Boolean];
        let (resolved_type, _) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::String);
    }

    #[test]
    fn test_coerce_to_column_type() {
        use std::str::FromStr;
        let dec = |s: &str| Value::Decimal(Decimal::from_str(s).unwrap());

        // Widen, narrow, and leave what doesn't convert
        assert_eq!(Value::Integer(84).coerce_to(&DataType::Decimal), dec("84"));
        assert_eq!(
            dec("12.00").coerce_to(&DataType::Integer),
            Value::Integer(12)
        );
        assert_eq!(dec("12.5").coerce_to(&DataType::Integer), dec("12.5"));

        // Anything becomes text
        assert_eq!(
            dec("29.85").coerce_to(&DataType::String),
            Value::String("29.85".into())
        );
        assert_eq!(
            Value::Boolean(true).coerce_to(&DataType::String),
            Value::String("true".into())
        );

        // Text parses into the column type; blank text is NULL
        assert_eq!(
            Value::String(" 1889.5 ".into()).coerce_to(&DataType::Decimal),
            dec("1889.5")
        );
        assert_eq!(
            Value::String(" ".into()).coerce_to(&DataType::Decimal),
            Value::Null
        );
        assert_eq!(
            Value::String("TRUE".into()).coerce_to(&DataType::Boolean),
            Value::Boolean(true)
        );
        assert_eq!(
            Value::String("n/a".into()).coerce_to(&DataType::Integer),
            Value::String("n/a".into())
        );
        assert!(matches!(
            Value::String("2024-01-15".into()).coerce_to(&DataType::Date),
            Value::Date(_)
        ));
        assert_eq!(
            Value::String(r#"{"a":1}"#.into()).coerce_to(&DataType::Json),
            Value::Json(serde_json::json!({"a": 1}))
        );
        assert_eq!(
            Value::Integer(5).coerce_to(&DataType::Json),
            Value::Json(serde_json::json!(5))
        );

        // NULL fits everywhere
        assert_eq!(Value::Null.coerce_to(&DataType::Integer), Value::Null);
    }

    #[test]
    fn test_header_only_keeps_columns() {
        let order = vec!["id".to_string(), "note".to_string()];
        let schema = SchemaInferer::infer_from_rows_ordered(&[], &order).unwrap();
        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "note"]);
        assert!(schema
            .columns
            .iter()
            .all(|c| c.data_type == DataType::String && c.nullable));
    }

    #[test]
    fn test_schema_file_nullable_defaults_to_true() {
        let file: SchemaFile =
            serde_yaml::from_str("columns:\n  - name: id\n    type: Integer\n").unwrap();
        file.validate().unwrap();
        assert!(file.columns[0].nullable);
    }

    #[test]
    fn test_blank_text_does_not_decide_column_type() {
        let rows: Vec<Row> = ["29.85", " ", "84"]
            .iter()
            .map(|v| {
                let value = match *v {
                    " " => Value::String(" ".into()),
                    "84" => Value::Integer(84),
                    _ => Value::Decimal(v.parse().unwrap()),
                };
                HashMap::from([("total".to_string(), value)])
            })
            .collect();
        let schema = SchemaInferer::infer_from_rows(&rows).unwrap();
        assert_eq!(schema.columns[0].data_type, DataType::Decimal);

        // A column of nothing but blanks stays text
        let rows = vec![HashMap::from([(
            "x".to_string(),
            Value::String(" ".into()),
        )])];
        let schema = SchemaInferer::infer_from_rows(&rows).unwrap();
        assert_eq!(schema.columns[0].data_type, DataType::String);
    }

    #[test]
    fn test_coerce_row() {
        let schema = Schema {
            columns: vec![
                Column {
                    name: "charge".into(),
                    data_type: DataType::Decimal,
                    nullable: true,
                },
                Column {
                    name: "note".into(),
                    data_type: DataType::String,
                    nullable: true,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        let mut row: Row = HashMap::new();
        row.insert("charge".into(), Value::Integer(20));
        row.insert("note".into(), Value::Integer(7));
        row.insert("extra".into(), Value::Integer(1)); // not in the schema: untouched
        coerce_row(&mut row, &schema);
        assert_eq!(row["charge"], Value::Decimal(Decimal::from(20)));
        assert_eq!(row["note"], Value::String("7".into()));
        assert_eq!(row["extra"], Value::Integer(1));
    }

    #[test]
    fn test_large_schema_inference() {
        let mut rows = Vec::new();
        for i in 0..1000 {
            let mut row = HashMap::new();
            row.insert("id".to_string(), Value::Integer(i));
            row.insert("name".to_string(), Value::String(format!("user_{}", i)));
            if i % 10 == 0 {
                row.insert("optional".to_string(), Value::Null);
            } else {
                row.insert("optional".to_string(), Value::String("present".to_string()));
            }
            rows.push(row);
        }

        let schema = SchemaInferer::infer_from_rows(&rows).unwrap();
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.estimated_rows, Some(1000));

        let optional_col = schema
            .columns
            .iter()
            .find(|c| c.name == "optional")
            .unwrap();
        assert!(optional_col.nullable);
    }

    #[test]
    fn test_missing_columns_in_rows() {
        // Test case where some rows don't have all columns
        let mut row1 = HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));
        row1.insert(
            "email".to_string(),
            Value::String("alice@example.com".to_string()),
        );

        let mut row2 = HashMap::new();
        row2.insert("id".to_string(), Value::Integer(2));
        row2.insert("name".to_string(), Value::String("Bob".to_string()));
        // Missing email column in this row

        let schema = SchemaInferer::infer_from_rows(&[row1, row2]).unwrap();
        assert_eq!(schema.columns.len(), 3);

        let email_col = schema.columns.iter().find(|c| c.name == "email").unwrap();
        assert_eq!(email_col.data_type, DataType::String);
        assert!(email_col.nullable); // Should be nullable because it's missing in row2
    }

    #[test]
    fn test_json_type_inference() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), Value::Integer(1));
        row.insert(
            "metadata".to_string(),
            Value::Json(serde_json::json!({
                "key": "value",
                "count": 42
            })),
        );

        let schema = SchemaInferer::infer_from_rows(&[row]).unwrap();
        assert_eq!(schema.columns.len(), 2);

        let metadata_col = schema
            .columns
            .iter()
            .find(|c| c.name == "metadata")
            .unwrap();
        assert_eq!(metadata_col.data_type, DataType::Json);
        assert!(metadata_col.nullable);
    }

    #[test]
    fn test_json_value_conversion() {
        let json_value = Value::Json(serde_json::json!({"name": "test", "count": 123}));

        // Test arrow_type
        assert_eq!(json_value.arrow_type(), ArrowDataType::Utf8);

        // Test to_string_for_arrow
        let json_str = json_value.to_string_for_arrow().unwrap();
        assert!(json_str.contains("\"name\""));
        assert!(json_str.contains("\"test\""));

        // Test to_json
        assert!(json_value.to_json().is_some());
    }

    #[test]
    fn test_json_default_value() {
        let schema_file = SchemaFile {
            on_mismatch: OnMismatch::Fail,
            columns: vec![SchemaFileColumn {
                name: "config".to_string(),
                data_type: "json".to_string(),
                nullable: false,
                pattern: None,
                default: Some(r#"{"enabled": true, "count": 0}"#.to_string()),
                on_mismatch: None,
            }],
        };

        let default = schema_file
            .parse_default_value(r#"{"enabled": true, "count": 0}"#, "json")
            .unwrap();

        match default {
            Value::Json(j) => {
                assert_eq!(j["enabled"], true);
                assert_eq!(j["count"], 0);
            }
            _ => panic!("Expected JSON value"),
        }
    }

    fn schema_file(yaml: &str) -> SchemaFile {
        let file: SchemaFile = serde_yaml::from_str(yaml).unwrap();
        file.validate().unwrap();
        file
    }

    fn text(s: &str) -> Value {
        Value::String(s.to_string())
    }

    #[test]
    fn test_number_keeps_text() {
        for ok in ["0", "7", "-7", "0.5", "-0.5", "10", "1.50", "0e1"] {
            assert!(number_keeps_text(ok), "{}", ok);
        }
        for code in ["01826", "007", "-01", "0012.50", "+15551234", "00"] {
            assert!(!number_keeps_text(code), "{}", code);
        }
        assert_eq!(text("007").coerce_to(&DataType::Integer), text("007"));
        assert_eq!(
            text("0012.50").coerce_to(&DataType::Decimal),
            text("0012.50")
        );
        assert_eq!(text("12").coerce_to(&DataType::Integer), Value::Integer(12));
    }

    #[test]
    fn test_string_columns_keep_the_source_text() {
        let file = schema_file(
            "columns:\n  - {name: zip, type: String}\n  - {name: day, type: String}\n  - {name: flag, type: String}\n  - {name: n, type: String}\n",
        );
        let mut row: Row = [
            ("zip", text("01826")),
            ("day", text("3/4/2021")),
            ("flag", text("true")),
            ("n", Value::Integer(12)), // already typed, e.g. a JSON number
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(file.validate_and_transform_row(&mut row).unwrap(), 0);
        assert_eq!(row["zip"], text("01826"));
        assert_eq!(row["day"], text("3/4/2021"));
        assert_eq!(row["flag"], text("true"));
        assert_eq!(row["n"], text("12"));
    }

    #[test]
    fn test_declared_types_parse_the_source_text() {
        let file = schema_file(
            "columns:\n  - {name: n, type: Integer}\n  - {name: d, type: Date, pattern: '^\\d{4}-'}\n  - {name: j, type: Json}\n",
        );
        let mut row: Row = [
            ("n", text(" 42")),
            ("d", text("2024-01-02")),
            ("j", text("[1]")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        file.validate_and_transform_row(&mut row).unwrap();
        assert_eq!(row["n"], Value::Integer(42));
        assert!(matches!(row["d"], Value::Date(_)));
        assert_eq!(row["j"], Value::Json(serde_json::json!([1])));
        // The pattern sees the text as written
        let mut row: Row = [("d".to_string(), text("01/02/2024"))]
            .into_iter()
            .collect();
        let err = file
            .validate_and_transform_row(&mut row)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match pattern"), "{}", err);
    }

    #[test]
    fn test_on_mismatch() {
        let row = || -> Row { [("n".to_string(), text("abc"))].into_iter().collect() };
        let fail = schema_file("columns:\n  - {name: n, type: Integer}\n");
        let err = fail
            .validate_and_transform_row(&mut row())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Column 'n' value 'abc' is not a valid Integer"),
            "{}",
            err
        );

        let null = schema_file("on_mismatch: null\ncolumns:\n  - {name: n, type: Integer}\n");
        let mut r = row();
        assert_eq!(null.validate_and_transform_row(&mut r).unwrap(), 1);
        assert_eq!(r["n"], Value::Null);

        // A column's setting overrides the file's
        let raw = schema_file(
            "on_mismatch: null\ncolumns:\n  - {name: n, type: Integer, on_mismatch: raw}\n",
        );
        let mut r = row();
        assert_eq!(raw.validate_and_transform_row(&mut r).unwrap(), 0);
        assert_eq!(r["n"], text("abc"));

        // NULL in a column that isn't nullable is still an error
        let strict = schema_file(
            "on_mismatch: null\ncolumns:\n  - {name: n, type: Integer, nullable: false}\n",
        );
        assert!(strict.validate_and_transform_row(&mut row()).is_err());
        assert!(serde_yaml::from_str::<SchemaFile>("on_mismatch: skip\ncolumns: []\n").is_err());
    }

    #[test]
    fn test_apply_to_keeps_source_order_and_unlisted_columns() {
        let file = schema_file(
            "columns:\n  - {name: b, type: String}\n  - {name: extra, type: Integer}\n",
        );
        let inferred = SchemaInferer::infer_from_rows_ordered(
            &[[("a", Value::Integer(1)), ("b", Value::Integer(2))]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()],
            &["a".to_string(), "b".to_string()],
        )
        .unwrap();
        let schema = file.apply_to(&inferred);
        let columns: Vec<_> = schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.data_type.clone()))
            .collect();
        assert_eq!(
            columns,
            [
                ("a", DataType::Integer),
                ("b", DataType::String),
                ("extra", DataType::Integer)
            ]
        );
    }
}
