use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    String,
    Integer,
    Float,
    Boolean,
    Date,
    DateTime,
    Null,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
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
            Value::Float(_) => DataType::Float,
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
            DataType::String => write!(f, "STRING"),
            DataType::Integer => write!(f, "INTEGER"),
            DataType::Float => write!(f, "REAL"),
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
        assert_eq!(DataType::String.to_string(), "STRING");
        assert_eq!(DataType::Integer.to_string(), "INTEGER");
        assert_eq!(DataType::Float.to_string(), "REAL");
        assert_eq!(DataType::Boolean.to_string(), "BOOLEAN");
        assert_eq!(DataType::Date.to_string(), "DATE");
        assert_eq!(DataType::DateTime.to_string(), "TIMESTAMP");
        assert_eq!(DataType::Null.to_string(), "NULL");
    }
    
    #[test]
    fn test_value_type_inference() {
        assert_eq!(SchemaInferer::infer_type(&Value::String("test".to_string())), DataType::String);
        assert_eq!(SchemaInferer::infer_type(&Value::Integer(42)), DataType::Integer);
        assert_eq!(SchemaInferer::infer_type(&Value::Float(3.14)), DataType::Float);
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
