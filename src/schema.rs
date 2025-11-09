use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use rust_decimal::Decimal;
use regex::Regex;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    String,
    Integer,
    Decimal,
    Boolean,
    Date,
    DateTime,
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFileColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub nullable: bool,
    pub pattern: Option<String>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFile {
    pub columns: Vec<SchemaFileColumn>,
}

impl SchemaFile {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let schema_file: SchemaFile = serde_yaml::from_str(&content)
            .map_err(|e| crate::TinyEtlError::Configuration(format!("Invalid schema file: {}", e)))?;
        
        // Validate the schema file
        schema_file.validate()?;
        
        Ok(schema_file)
    }
    
    pub fn validate(&self) -> Result<()> {
        for column in &self.columns {
            // Validate data type
            match column.data_type.to_lowercase().as_str() {
                "string" | "integer" | "decimal" | "boolean" | "date" | "datetime" => {},
                _ => return Err(crate::TinyEtlError::Configuration(
                    format!("Invalid data type '{}' for column '{}'", column.data_type, column.name)
                )),
            }
            
            // Validate regex pattern if provided
            if let Some(pattern) = &column.pattern {
                Regex::new(pattern).map_err(|e| 
                    crate::TinyEtlError::Configuration(
                        format!("Invalid regex pattern '{}' for column '{}': {}", pattern, column.name, e)
                    )
                )?;
            }
        }
        Ok(())
    }
    
    pub fn to_schema(&self) -> Result<Schema> {
        let columns = self.columns.iter().map(|col| {
            let data_type = match col.data_type.to_lowercase().as_str() {
                "string" => DataType::String,
                "integer" => DataType::Integer,
                "decimal" => DataType::Decimal,
                "boolean" => DataType::Boolean,
                "date" => DataType::Date,
                "datetime" => DataType::DateTime,
                _ => DataType::String, // Already validated, so this shouldn't happen
            };
            
            Column {
                name: col.name.clone(),
                data_type,
                nullable: col.nullable,
            }
        }).collect();
        
        Ok(Schema {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        })
    }
    
    pub fn validate_and_transform_row(&self, row: &mut Row) -> Result<()> {
        for schema_col in &self.columns {
            let value = row.get(&schema_col.name);
            
            // Check for required columns
            if !schema_col.nullable && (value.is_none() || matches!(value, Some(Value::Null))) {
                // Apply default if available
                if let Some(default_str) = &schema_col.default {
                    let default_value = self.parse_default_value(default_str, &schema_col.data_type)?;
                    row.insert(schema_col.name.clone(), default_value);
                } else {
                    return Err(crate::TinyEtlError::DataValidation(
                        format!("Required column '{}' is missing or null", schema_col.name)
                    ));
                }
            }
            
            // Validate existing values
            if let Some(val) = row.get(&schema_col.name) {
                self.validate_column_value(val, schema_col)?;
            }
        }
        Ok(())
    }
    
    fn validate_column_value(&self, value: &Value, schema_col: &SchemaFileColumn) -> Result<()> {
        // Skip null values if column is nullable
        if matches!(value, Value::Null) && schema_col.nullable {
            return Ok(());
        }
        
        // Validate data type
        let expected_type = match schema_col.data_type.to_lowercase().as_str() {
            "string" => DataType::String,
            "integer" => DataType::Integer,
            "decimal" => DataType::Decimal,
            "boolean" => DataType::Boolean,
            "date" => DataType::Date,
            "datetime" => DataType::DateTime,
            _ => return Err(crate::TinyEtlError::DataValidation(
                format!("Unknown data type '{}' for column '{}'", schema_col.data_type, schema_col.name)
            )),
        };
        
        let actual_type = SchemaInferer::infer_type(value);
        if actual_type != expected_type && actual_type != DataType::Null {
            return Err(crate::TinyEtlError::DataValidation(
                format!("Column '{}' expected type {:?}, got {:?}", schema_col.name, expected_type, actual_type)
            ));
        }
        
        // Validate pattern for string values
        if let (Some(pattern), Value::String(s)) = (&schema_col.pattern, value) {
            let regex = Regex::new(pattern).unwrap(); // Already validated in validate()
            if !regex.is_match(s) {
                return Err(crate::TinyEtlError::DataValidation(
                    format!("Column '{}' value '{}' does not match pattern '{}'", schema_col.name, s, pattern)
                ));
            }
        }
        
        Ok(())
    }
    
    fn parse_default_value(&self, default_str: &str, data_type: &str) -> Result<Value> {
        match data_type.to_lowercase().as_str() {
            "string" => Ok(Value::String(default_str.to_string())),
            "integer" => {
                let parsed = default_str.parse::<i64>()
                    .map_err(|_| crate::TinyEtlError::Configuration(
                        format!("Invalid default integer value: '{}'", default_str)
                    ))?;
                Ok(Value::Integer(parsed))
            },
            "decimal" => {
                let parsed = default_str.parse::<Decimal>()
                    .map_err(|_| crate::TinyEtlError::Configuration(
                        format!("Invalid default decimal value: '{}'", default_str)
                    ))?;
                Ok(Value::Decimal(parsed))
            },
            "boolean" => {
                let parsed = default_str.parse::<bool>()
                    .map_err(|_| crate::TinyEtlError::Configuration(
                        format!("Invalid default boolean value: '{}'", default_str)
                    ))?;
                Ok(Value::Boolean(parsed))
            },
            "date" | "datetime" => {
                // Try to parse as RFC3339 first, then as date
                let parsed = chrono::DateTime::parse_from_rfc3339(default_str)
                    .or_else(|_| {
                        // Try parsing as date only
                        chrono::NaiveDate::parse_from_str(default_str, "%Y-%m-%d")
                            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().fixed_offset())
                    })
                    .map_err(|_| crate::TinyEtlError::Configuration(
                        format!("Invalid default date/datetime value: '{}'", default_str)
                    ))?;
                Ok(Value::Date(parsed.with_timezone(&Utc)))
            },
            _ => Err(crate::TinyEtlError::Configuration(
                format!("Unsupported data type for default value: '{}'", data_type)
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub columns: Vec<Column>,
    pub estimated_rows: Option<usize>,
    pub primary_key_candidate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(String),
    Integer(i64),
    Decimal(Decimal),
    Boolean(bool),
    Date(DateTime<Utc>),
    Null,
}

pub type Row = HashMap<String, Value>;

pub struct SchemaInferer;

impl SchemaInferer {
    pub fn infer_from_rows(rows: &[Row]) -> Result<Schema> {
        if rows.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        let mut column_types: HashMap<String, Vec<DataType>> = HashMap::new();
        
        // Collect all unique column names
        let mut all_columns = std::collections::HashSet::new();
        for row in rows {
            for key in row.keys() {
                all_columns.insert(key.clone());
            }
        }

        // Analyze each column's data types
        for col_name in &all_columns {
            let mut types = Vec::new();
            for row in rows {
                let data_type = match row.get(col_name) {
                    Some(value) => Self::infer_type(value),
                    None => DataType::Null,
                };
                types.push(data_type);
            }
            column_types.insert(col_name.clone(), types);
        }

        // Determine final type for each column
        let columns = all_columns
            .iter()
            .map(|col_name| {
                let types = column_types.get(col_name).unwrap();
                let (data_type, nullable) = Self::resolve_column_type(types);
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
            Value::Null => DataType::Null,
        }
    }
    
    pub fn resolve_column_type(types: &[DataType]) -> (DataType, bool) {
        let has_null = types.iter().any(|t| matches!(t, DataType::Null));
        
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
        if non_null_types.iter().all(|t| std::mem::discriminant(*t) == std::mem::discriminant(first_type)) {
            (first_type.clone(), has_null)
        } else {
            // Mixed types default to String
            (DataType::String, has_null)
        }
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
        assert!(!id_col.nullable);
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
        let optional_col = schema.columns.iter().find(|c| c.name == "optional").unwrap();
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
        assert_eq!(DataType::Null.to_string(), "NULL");
    }
    
    #[test]
    fn test_value_type_inference() {
        assert_eq!(SchemaInferer::infer_type(&Value::String("test".to_string())), DataType::String);
        assert_eq!(SchemaInferer::infer_type(&Value::Integer(42)), DataType::Integer);
        assert_eq!(SchemaInferer::infer_type(&Value::Decimal(Decimal::new(314, 2))), DataType::Decimal);
        assert_eq!(SchemaInferer::infer_type(&Value::Boolean(true)), DataType::Boolean);
        assert_eq!(SchemaInferer::infer_type(&Value::Date(Utc::now())), DataType::DateTime);
        assert_eq!(SchemaInferer::infer_type(&Value::Null), DataType::Null);
    }
    
    #[test]
    fn test_column_type_resolution() {
        // All same type
        let types = vec![DataType::Integer, DataType::Integer, DataType::Integer];
        let (resolved_type, nullable) = SchemaInferer::resolve_column_type(&types);
        assert_eq!(resolved_type, DataType::Integer);
        assert!(!nullable);
        
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
        
        let optional_col = schema.columns.iter().find(|c| c.name == "optional").unwrap();
        assert!(optional_col.nullable);
    }
    
    #[test]
    fn test_missing_columns_in_rows() {
        // Test case where some rows don't have all columns
        let mut row1 = HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));
        row1.insert("email".to_string(), Value::String("alice@example.com".to_string()));
        
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
}
