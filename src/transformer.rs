use mlua::{Function, Lua, Table, Value as LuaValue};
use rust_decimal::Decimal;
use std::path::Path;
use tracing::{debug, warn};

use crate::{
    schema::{Column, Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

/// Configuration for data transformation
#[derive(Debug, Clone)]
pub enum TransformConfig {
    /// Path to a Lua file containing a transform function
    File(String),
    /// Semicolon-separated inline expressions like "col1=row.old_col * 2; col2=row.name .. '_suffix'"
    Inline(String),
    /// Multi-line Lua script with individual assignments (for YAML configs)
    Script(String),
    /// No transformation
    None,
}

impl Default for TransformConfig {
    fn default() -> Self {
        TransformConfig::None
    }
}

/// Row transformer using Lua scripting
#[derive(Debug)]
pub struct Transformer {
    lua: Lua,
    has_transform: bool,
    inferred_schema: Option<Schema>,
}

impl Transformer {
    /// Create a new transformer with the given configuration
    pub fn new(config: &TransformConfig) -> Result<Self> {
        let lua = Lua::new();
        let mut transformer = Self {
            lua,
            has_transform: false,
            inferred_schema: None,
        };

        match config {
            TransformConfig::File(path) => {
                transformer.load_from_file(path)?;
            }
            TransformConfig::Inline(expressions) => {
                transformer.load_from_expressions(expressions)?;
            }
            TransformConfig::Script(script) => {
                transformer.load_from_script(script)?;
            }
            TransformConfig::None => {
                // No transformation needed
            }
        }

        Ok(transformer)
    }

    /// Load transformation from a Lua file
    fn load_from_file(&mut self, path: &str) -> Result<()> {
        if !Path::new(path).exists() {
            return Err(TinyEtlError::Configuration(format!(
                "Transform file not found: {}",
                path
            )));
        }

        let lua_code = std::fs::read_to_string(path).map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to read transform file {}: {}", path, e))
        })?;

        self.lua.load(&lua_code).exec().map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to execute Lua file {}: {}", path, e))
        })?;

        let globals = self.lua.globals();
        let _transform_fn: Function = globals.get("transform").map_err(|_| {
            TinyEtlError::Configuration("Lua file must contain a 'transform' function".to_string())
        })?;

        self.has_transform = true;

        debug!("Loaded transform function from file: {}", path);
        Ok(())
    }

    /// Load transformation from inline expressions
    fn load_from_expressions(&mut self, expressions: &str) -> Result<()> {
        let lua_code = self.build_transform_function(expressions)?;

        self.lua.load(&lua_code).exec().map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to execute inline expressions: {}", e))
        })?;

        let globals = self.lua.globals();
        let _transform_fn: Function = globals.get("transform").map_err(|_| {
            TinyEtlError::Configuration(
                "Failed to create transform function from expressions".to_string(),
            )
        })?;

        self.has_transform = true;

        debug!(
            "Created transform function from expressions: {}",
            expressions
        );
        Ok(())
    }

    /// Load transformation from a multi-line script (for YAML configs)
    fn load_from_script(&mut self, script: &str) -> Result<()> {
        let lua_code = self.build_script_function(script)?;

        self.lua
            .load(&lua_code)
            .exec()
            .map_err(|e| TinyEtlError::Configuration(format!("Failed to execute script: {}", e)))?;

        let globals = self.lua.globals();
        let _transform_fn: Function = globals.get("transform").map_err(|_| {
            TinyEtlError::Configuration(
                "Failed to create transform function from script".to_string(),
            )
        })?;

        self.has_transform = true;

        debug!("Created transform function from script: {}", script);
        Ok(())
    }

    /// Build a Lua transform function from semicolon-separated expressions
    fn build_transform_function(&self, expressions: &str) -> Result<String> {
        let assignments: Vec<&str> = expressions
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if assignments.is_empty() {
            return Err(TinyEtlError::Configuration(
                "No valid expressions provided".to_string(),
            ));
        }

        let mut lua_code = String::new();
        lua_code.push_str("function transform(row)\n");
        lua_code.push_str("  local result = {}\n");
        lua_code.push_str("  -- Copy all existing columns first\n");
        lua_code.push_str("  for k, v in pairs(row) do\n");
        lua_code.push_str("    result[k] = v\n");
        lua_code.push_str("  end\n");
        lua_code.push_str("  -- Apply transformations (can override existing columns)\n");

        for assignment in assignments {
            if let Some(equals_pos) = assignment.find('=') {
                let col_name = assignment[..equals_pos].trim();
                let expression = assignment[equals_pos + 1..].trim();

                // Validate column name
                if col_name.is_empty() || !col_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                {
                    return Err(TinyEtlError::Configuration(format!(
                        "Invalid column name: {}",
                        col_name
                    )));
                }

                lua_code.push_str(&format!("  result['{}'] = {}\n", col_name, expression));
            } else {
                return Err(TinyEtlError::Configuration(format!(
                    "Invalid expression format (missing '='): {}",
                    assignment
                )));
            }
        }

        lua_code.push_str("  return result\n");
        lua_code.push_str("end\n");

        debug!("Generated Lua code:\n{}", lua_code);
        Ok(lua_code)
    }

    /// Build a Lua transform function from multi-line script assignments
    fn build_script_function(&self, script: &str) -> Result<String> {
        let lines: Vec<&str> = script
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && !s.starts_with("--")) // Skip empty lines and comments
            .collect();

        if lines.is_empty() {
            return Err(TinyEtlError::Configuration(
                "No valid assignments provided in script".to_string(),
            ));
        }

        let mut lua_code = String::new();
        lua_code.push_str("function transform(row)\n");
        lua_code.push_str("  local result = {}\n");
        lua_code.push_str("  -- Copy all existing columns first\n");
        lua_code.push_str("  for k, v in pairs(row) do\n");
        lua_code.push_str("    result[k] = v\n");
        lua_code.push_str("  end\n");
        lua_code.push_str("  -- Apply transformations from script\n");

        // Process each line as either a local variable or result assignment
        for line in lines {
            if let Some(equals_pos) = line.find('=') {
                let col_name = line[..equals_pos].trim();
                let expression = line[equals_pos + 1..].trim();

                // Validate variable/column name
                if col_name.is_empty() || !col_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                {
                    return Err(TinyEtlError::Configuration(format!(
                        "Invalid variable/column name: {}",
                        col_name
                    )));
                }

                // Check if this should be a local variable or result column
                // Local variables are those used in subsequent expressions but not meant as output
                // For now, we'll treat everything as a local variable first, then add to result
                lua_code.push_str(&format!("  local {} = {}\n", col_name, expression));
                lua_code.push_str(&format!("  result['{}'] = {}\n", col_name, col_name));
            } else {
                return Err(TinyEtlError::Configuration(format!(
                    "Invalid line format (missing '='): {}",
                    line
                )));
            }
        }

        lua_code.push_str("  return result\n");
        lua_code.push_str("end\n");

        debug!("Generated Lua code from script:\n{}", lua_code);
        Ok(lua_code)
    }

    /// Check if this transformer has any transformation logic
    pub fn is_enabled(&self) -> bool {
        self.has_transform
    }

    /// Transform a batch of rows and infer schema from the first transformed row
    /// Filters out rows where the transform function returns nil
    pub fn transform_batch(&mut self, rows: &[Row]) -> Result<Vec<Row>> {
        if !self.is_enabled() {
            return Ok(rows.to_vec());
        }

        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let mut transformed_rows = Vec::new();
        let mut schema_inferred = false;

        for row in rows.iter() {
            if let Some(transformed_row) = self.transform_row(row)? {
                // Infer schema from the first successfully transformed row
                if !schema_inferred {
                    self.infer_schema_from_first_row(&transformed_row)?;
                    schema_inferred = true;
                }

                // Validate the transformed row against the inferred schema
                let validated_row = self.validate_and_filter_row(transformed_row)?;
                transformed_rows.push(validated_row);
            }
            // If transform_row returns None, the row is filtered out (skip it)
        }

        Ok(transformed_rows)
    }

    /// Transform a single row using the Lua function
    /// Returns None if the row should be filtered out (when Lua returns nil)
    fn transform_row(&self, row: &Row) -> Result<Option<Row>> {
        if !self.has_transform {
            return Err(TinyEtlError::Configuration(
                "No transform function loaded".to_string(),
            ));
        }

        let globals = self.lua.globals();
        let transform_fn: Function = globals.get("transform").map_err(|e| {
            TinyEtlError::Transform(format!("Failed to get transform function: {}", e))
        })?;

        // Convert Row to Lua table
        let lua_row = self.row_to_lua_table(row)?;

        // Call the transform function
        let result: LuaValue = transform_fn.call(lua_row).map_err(|e| {
            TinyEtlError::Transform(format!("Lua transform function failed: {}", e))
        })?;

        // Handle filtering: if Lua returns nil, filter out this row
        match result {
            LuaValue::Nil => Ok(None),
            LuaValue::Table(table) => {
                // Convert result back to Row
                let row = self.lua_table_to_row(table)?;
                // Also check if the table is empty (another way to filter)
                if row.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(row))
                }
            }
            _ => Err(TinyEtlError::Transform(
                "Transform function must return a table or nil".to_string(),
            )),
        }
    }

    /// Convert a Row to a Lua table
    fn row_to_lua_table(&self, row: &Row) -> Result<Table> {
        let table = self
            .lua
            .create_table()
            .map_err(|e| TinyEtlError::Transform(format!("Failed to create Lua table: {}", e)))?;

        for (key, value) in row {
            let lua_value = match value {
                Value::String(s) => LuaValue::String(self.lua.create_string(s)?),
                Value::Integer(i) => LuaValue::Integer(*i),
                Value::Decimal(d) => {
                    // Convert Decimal to f64 for Lua
                    let f: f64 = (*d).try_into().unwrap_or(0.0);
                    LuaValue::Number(f)
                }
                Value::Boolean(b) => LuaValue::Boolean(*b),
                Value::Date(dt) => LuaValue::String(self.lua.create_string(&dt.to_rfc3339())?),
                Value::Json(j) => {
                    // Convert JSON to Lua string representation
                    let json_str = serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string());
                    LuaValue::String(self.lua.create_string(&json_str)?)
                }
                Value::Null => LuaValue::Nil,
            };
            table.set(key.as_str(), lua_value)?;
        }

        Ok(table)
    }

    /// Convert a Lua table back to a Row
    fn lua_table_to_row(&self, table: Table) -> Result<Row> {
        let mut row = Row::new();

        for pair in table.pairs::<String, LuaValue>() {
            let (key, lua_value) = pair.map_err(|e| {
                TinyEtlError::Transform(format!("Failed to iterate Lua table: {}", e))
            })?;

            let value = match lua_value {
                LuaValue::String(s) => {
                    let str_val = s.to_str().map_err(|e| {
                        TinyEtlError::Transform(format!("Failed to convert Lua string: {}", e))
                    })?;
                    Value::String(str_val.to_string())
                }
                LuaValue::Integer(i) => Value::Integer(i),
                LuaValue::Number(f) => {
                    // Convert f64 to Decimal
                    match Decimal::try_from(f) {
                        Ok(d) => Value::Decimal(d),
                        Err(_) => Value::String(f.to_string()),
                    }
                }
                LuaValue::Boolean(b) => Value::Boolean(b),
                LuaValue::Nil => Value::Null,
                _ => {
                    warn!(
                        "Unsupported Lua value type for column '{}', converting to string",
                        key
                    );
                    Value::String(format!("{:?}", lua_value))
                }
            };

            row.insert(key, value);
        }

        Ok(row)
    }

    /// Infer schema from the first transformed row
    fn infer_schema_from_first_row(&mut self, row: &Row) -> Result<()> {
        let columns: Vec<Column> = row
            .iter()
            .map(|(name, value)| {
                let data_type = SchemaInferer::infer_type(value);
                Column {
                    name: name.clone(),
                    data_type,
                    nullable: matches!(value, Value::Null),
                }
            })
            .collect();

        self.inferred_schema = Some(Schema {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        });

        debug!(
            "Inferred schema from first transformed row: {} columns",
            self.inferred_schema.as_ref().unwrap().columns.len()
        );
        Ok(())
    }

    /// Validate and filter a row based on the inferred schema
    fn validate_and_filter_row(&self, mut row: Row) -> Result<Row> {
        let schema = self
            .inferred_schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::Transform("Schema not inferred yet".to_string()))?;

        let expected_columns: std::collections::HashSet<_> =
            schema.columns.iter().map(|c| c.name.clone()).collect();

        // Remove columns not in the schema (dropped from first row)
        row.retain(|col_name, _| expected_columns.contains(col_name));

        // Add missing columns as null
        for column in &schema.columns {
            if !row.contains_key(&column.name) {
                row.insert(column.name.clone(), Value::Null);
            }
        }

        // Type validation could be added here if needed
        // For now, we trust Lua to return consistent types

        Ok(row)
    }

    /// Get the inferred schema from transformed data
    pub fn get_inferred_schema(&self) -> Option<&Schema> {
        self.inferred_schema.as_ref()
    }

    /// Update the base schema with transformations (to be called after processing first batch)
    pub fn merge_with_base_schema(&self, base_schema: &Schema) -> Result<Schema> {
        match &self.inferred_schema {
            Some(transform_schema) => {
                // If we have transformations, use the transformed schema
                Ok(transform_schema.clone())
            }
            None => {
                // No transformations, use base schema as-is
                Ok(base_schema.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DataType, Value};
    use std::collections::HashMap;

    #[test]
    fn test_no_transformation() {
        let config = TransformConfig::None;
        let transformer = Transformer::new(&config).unwrap();
        assert!(!transformer.is_enabled());
    }

    #[test]
    fn test_inline_expressions() {
        let config = TransformConfig::Inline(
            "full_name=row.first_name .. ' ' .. row.last_name; age_next_year=row.age + 1"
                .to_string(),
        );
        let mut transformer = Transformer::new(&config).unwrap();
        assert!(transformer.is_enabled());

        let mut row = HashMap::new();
        row.insert("first_name".to_string(), Value::String("John".to_string()));
        row.insert("last_name".to_string(), Value::String("Doe".to_string()));
        row.insert("age".to_string(), Value::Integer(30));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        // Check new columns
        assert_eq!(
            transformed.get("full_name"),
            Some(&Value::String("John Doe".to_string()))
        );
        assert_eq!(transformed.get("age_next_year"), Some(&Value::Integer(31)));
        // Check original columns are preserved
        assert_eq!(
            transformed.get("first_name"),
            Some(&Value::String("John".to_string()))
        );
        assert_eq!(
            transformed.get("last_name"),
            Some(&Value::String("Doe".to_string()))
        );
        assert_eq!(transformed.get("age"), Some(&Value::Integer(30)));
    }

    #[test]
    fn test_script_transform() {
        let script = r#"
full_name = row.first_name .. " " .. row.last_name
annual_salary = row.monthly_salary * 12
hire_year = tonumber(string.sub(row.hire_date, 1, 4))
current_year = 2024
years_service = current_year - hire_year
"#;
        let config = TransformConfig::Script(script.to_string());
        let mut transformer = Transformer::new(&config).unwrap();
        assert!(transformer.is_enabled());

        let mut row = HashMap::new();
        row.insert("first_name".to_string(), Value::String("John".to_string()));
        row.insert("last_name".to_string(), Value::String("Doe".to_string()));
        row.insert(
            "monthly_salary".to_string(),
            Value::Decimal(rust_decimal::Decimal::new(5000, 0)),
        );
        row.insert(
            "hire_date".to_string(),
            Value::String("2020-01-15".to_string()),
        );

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        // Check new columns
        assert_eq!(
            transformed.get("full_name"),
            Some(&Value::String("John Doe".to_string()))
        );
        assert_eq!(
            transformed.get("annual_salary"),
            Some(&Value::Decimal(rust_decimal::Decimal::new(60000, 0)))
        );
        assert_eq!(transformed.get("years_service"), Some(&Value::Integer(4)));
        // Check original columns are preserved
        assert_eq!(
            transformed.get("first_name"),
            Some(&Value::String("John".to_string()))
        );
        assert_eq!(
            transformed.get("monthly_salary"),
            Some(&Value::Decimal(rust_decimal::Decimal::new(5000, 0)))
        );
    }

    #[test]
    fn test_invalid_expression() {
        let config = TransformConfig::Inline("invalid_syntax".to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_inference() {
        let config = TransformConfig::Inline("new_col=42; str_col='test'".to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let row = HashMap::new();
        let result = transformer.transform_batch(&[row]).unwrap();

        let schema = transformer.get_inferred_schema().unwrap();
        assert_eq!(schema.columns.len(), 2);

        let col_names: Vec<_> = schema.columns.iter().map(|c| &c.name).collect();
        assert!(col_names.contains(&&"new_col".to_string()));
        assert!(col_names.contains(&&"str_col".to_string()));
    }

    #[test]
    fn test_transform_file_missing() {
        let config = TransformConfig::File("non_existent.lua".to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Transform file not found"));
    }

    #[test]
    fn test_invalid_inline_expression_no_equals() {
        let config = TransformConfig::Inline("invalid syntax without equals".to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid expression format"));
    }

    #[test]
    fn test_invalid_column_name() {
        let config = TransformConfig::Inline("invalid-name=42".to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid column name"));
    }

    #[test]
    fn test_empty_expressions() {
        let config = TransformConfig::Inline("".to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No valid expressions provided"));
    }

    #[test]
    fn test_transform_batch_empty() {
        let config = TransformConfig::Inline("new_col=42".to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let result = transformer.transform_batch(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_transform_batch_disabled() {
        let config = TransformConfig::None;
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("test".to_string(), Value::String("value".to_string()));

        let result = transformer.transform_batch(&[row.clone()]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], row);
    }

    #[test]
    fn test_different_value_types() {
        let config = TransformConfig::Inline(
            "int_col=42; float_col=3.14; bool_col=true; str_col='hello'".to_string(),
        );
        let mut transformer = Transformer::new(&config).unwrap();

        let row = HashMap::new();
        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        assert_eq!(transformed.get("int_col"), Some(&Value::Integer(42)));
        assert_eq!(
            transformed.get("float_col"),
            Some(&Value::Decimal(Decimal::new(314, 2)))
        );
        assert_eq!(transformed.get("bool_col"), Some(&Value::Boolean(true)));
        assert_eq!(
            transformed.get("str_col"),
            Some(&Value::String("hello".to_string()))
        );
        // Note: nil values in Lua don't create table entries, so we don't test null_col here
    }

    #[test]
    fn test_schema_merge_with_base() {
        let config = TransformConfig::None;
        let transformer = Transformer::new(&config).unwrap();

        let base_schema = Schema {
            columns: vec![Column {
                name: "test".to_string(),
                data_type: DataType::String,
                nullable: false,
            }],
            estimated_rows: Some(100),
            primary_key_candidate: None,
        };

        let merged = transformer.merge_with_base_schema(&base_schema).unwrap();
        assert_eq!(merged.columns.len(), 1);
        assert_eq!(merged.columns[0].name, "test");
    }

    #[test]
    fn test_validate_and_filter_row_missing_columns() {
        let config = TransformConfig::Inline("new_col=42".to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        // First transform to establish schema
        let row = HashMap::new();
        transformer.transform_batch(&[row]).unwrap();

        // Test with a row missing expected columns
        let mut incomplete_row = HashMap::new();
        incomplete_row.insert("extra".to_string(), Value::String("extra".to_string()));

        let validated = transformer.validate_and_filter_row(incomplete_row).unwrap();
        assert!(validated.contains_key("new_col"));
        assert_eq!(validated.get("new_col"), Some(&Value::Null));
    }

    #[test]
    fn test_row_to_lua_table_and_back() {
        let config = TransformConfig::Inline("result_col=row.test_col".to_string());
        let transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert(
            "test_col".to_string(),
            Value::String("test_value".to_string()),
        );

        // Test conversion to Lua table
        let lua_table = transformer.row_to_lua_table(&row).unwrap();

        // Test conversion back to row
        let converted_row = transformer.lua_table_to_row(lua_table).unwrap();
        assert_eq!(
            converted_row.get("test_col"),
            Some(&Value::String("test_value".to_string()))
        );
    }

    #[test]
    fn test_date_value_conversion() {
        let config = TransformConfig::Inline("date_col=row.input_date".to_string());
        let transformer = Transformer::new(&config).unwrap();

        let date = chrono::Utc::now();
        let mut row = HashMap::new();
        row.insert("input_date".to_string(), Value::Date(date));

        let lua_table = transformer.row_to_lua_table(&row).unwrap();
        let converted_row = transformer.lua_table_to_row(lua_table).unwrap();

        // Date should be converted to string in RFC3339 format
        match converted_row.get("input_date") {
            Some(Value::String(s)) => {
                assert!(s.contains("T")); // RFC3339 format contains 'T'
            }
            _ => panic!("Expected string representation of date"),
        }
    }

    #[test]
    fn test_lua_file_transformation() {
        use std::fs;
        use std::path::Path;

        // Create a temporary Lua file
        let lua_content = r#"
function transform(row)
    local result = {}
    if row.name then
        result.upper_name = string.upper(row.name)
        result.name_length = string.len(row.name)
    end
    return result
end
"#;

        let temp_file = "/tmp/test_transform.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();
        assert!(transformer.is_enabled());

        let mut row = HashMap::new();
        row.insert("name".to_string(), Value::String("john".to_string()));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        assert_eq!(
            transformed.get("upper_name"),
            Some(&Value::String("JOHN".to_string()))
        );
        assert_eq!(transformed.get("name_length"), Some(&Value::Integer(4)));

        // Clean up
        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_lua_file_invalid_syntax() {
        use std::fs;

        let lua_content = "invalid lua syntax {{{";
        let temp_file = "/tmp/test_invalid.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_lua_file_missing_transform_function() {
        use std::fs;

        let lua_content = r#"
function other_function()
    return "not transform"
end
"#;
        let temp_file = "/tmp/test_no_transform.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let result = Transformer::new(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must contain a 'transform' function"));

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_multiple_batch_transforms() {
        let config =
            TransformConfig::Inline("counter=1; batch_col=row.input or 'default'".to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        // First batch
        let mut row1 = HashMap::new();
        row1.insert("input".to_string(), Value::String("first".to_string()));
        let result1 = transformer.transform_batch(&[row1]).unwrap();

        // Second batch
        let mut row2 = HashMap::new();
        row2.insert("input".to_string(), Value::String("second".to_string()));
        let result2 = transformer.transform_batch(&[row2]).unwrap();

        assert_eq!(result1.len(), 1);
        assert_eq!(result2.len(), 1);

        // Both should have the schema applied
        assert!(result1[0].contains_key("counter"));
        assert!(result2[0].contains_key("counter"));
    }

    #[test]
    fn test_mixed_value_types_in_lua() {
        let config = TransformConfig::Inline(
            "mixed=type(row.value) == 'number' and row.value * 2 or 'not_number'".to_string(),
        );
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row1 = HashMap::new();
        row1.insert("value".to_string(), Value::Integer(21));
        let result1 = transformer.transform_batch(&[row1]).unwrap();

        let mut row2 = HashMap::new();
        row2.insert("value".to_string(), Value::String("text".to_string()));
        let result2 = transformer.transform_batch(&[row2]).unwrap();

        assert_eq!(result1[0].get("mixed"), Some(&Value::Integer(42)));
        assert_eq!(
            result2[0].get("mixed"),
            Some(&Value::String("not_number".to_string()))
        );
    }

    #[test]
    fn test_default_implementation() {
        let config = TransformConfig::default();
        let transformer = Transformer::new(&config).unwrap();
        assert!(!transformer.is_enabled());
    }

    #[test]
    fn test_transform_row_no_function_loaded() {
        // Create transformer with no transform function
        let config = TransformConfig::None;
        let transformer = Transformer::new(&config).unwrap();

        let row = HashMap::new();
        let result = transformer.transform_row(&row);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No transform function loaded"));
    }

    #[test]
    fn test_transform_function_returns_nil() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    -- Return nil to filter out the row
    return nil
end
"#;
        let temp_file = "/tmp/test_filter_nil.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("test".to_string(), Value::String("value".to_string()));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 0); // Row should be filtered out

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_transform_function_returns_empty_table() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    -- Return empty table to filter out the row
    return {}
end
"#;
        let temp_file = "/tmp/test_filter_empty.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("test".to_string(), Value::String("value".to_string()));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 0); // Row should be filtered out due to empty table

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_transform_function_returns_non_table() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    -- Return a string instead of table
    return "invalid"
end
"#;
        let temp_file = "/tmp/test_invalid_return.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("test".to_string(), Value::String("value".to_string()));

        let result = transformer.transform_batch(&[row]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Transform function must return a table or nil"));

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_unsupported_lua_value_type() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    local result = {}
    -- Return a function as a value (unsupported type)
    result.func_value = function() return "test" end
    result.thread_value = coroutine.create(function() end)
    return result
end
"#;
        let temp_file = "/tmp/test_unsupported_types.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let row = HashMap::new();
        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        // Unsupported types should be converted to string representations
        let transformed = &result[0];
        assert!(transformed.contains_key("func_value"));
        assert!(transformed.contains_key("thread_value"));

        // Values should be converted to strings with debug format
        if let Some(Value::String(s)) = transformed.get("func_value") {
            assert!(s.contains("Function"));
        } else {
            panic!("Expected string representation of function");
        }

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_lua_table_creation_and_conversion() {
        // Test the row_to_lua_table and lua_table_to_row conversion path
        let config = TransformConfig::Inline("test=42".to_string());
        let transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("test".to_string(), Value::String("value".to_string()));
        row.insert("int_val".to_string(), Value::Integer(42));
        row.insert(
            "float_val".to_string(),
            Value::Decimal(Decimal::new(314, 2)),
        );
        row.insert("bool_val".to_string(), Value::Boolean(true));
        row.insert("null_val".to_string(), Value::Null);

        let lua_table = transformer.row_to_lua_table(&row).unwrap();
        let converted_row = transformer.lua_table_to_row(lua_table).unwrap();

        // Verify all value types are handled correctly
        assert_eq!(
            converted_row.get("test"),
            Some(&Value::String("value".to_string()))
        );
        assert_eq!(converted_row.get("int_val"), Some(&Value::Integer(42)));
        assert_eq!(
            converted_row.get("float_val"),
            Some(&Value::Decimal(Decimal::new(314, 2)))
        );
        assert_eq!(converted_row.get("bool_val"), Some(&Value::Boolean(true)));

        // Note: In Lua, nil values don't create table entries, so null_val won't be preserved
        // This is correct behavior - null values are effectively filtered out in Lua tables
        assert_eq!(converted_row.get("null_val"), None);
        assert_eq!(converted_row.len(), 4); // Should have 4 entries, not 5
    }

    #[test]
    fn test_lua_explicit_nil_handling() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    local result = {}
    result.keep_value = row.input
    result.explicit_nil = nil  -- This won't be in the table
    result.explicit_null = "NULL"  -- This will be a string
    return result
end
"#;
        let temp_file = "/tmp/test_nil_handling.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let mut row = HashMap::new();
        row.insert("input".to_string(), Value::String("test".to_string()));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        assert_eq!(
            transformed.get("keep_value"),
            Some(&Value::String("test".to_string()))
        );
        assert_eq!(transformed.get("explicit_nil"), None); // nil values don't create entries
        assert_eq!(
            transformed.get("explicit_null"),
            Some(&Value::String("NULL".to_string()))
        );
        assert_eq!(transformed.len(), 2); // Only 2 entries should exist

        let _ = fs::remove_file(temp_file);
    }

    #[test]
    fn test_schema_merge_without_inferred_schema() {
        // Test the case where no transformations occurred and no inferred schema exists
        let config = TransformConfig::None;
        let transformer = Transformer::new(&config).unwrap();

        let base_schema = Schema {
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
            estimated_rows: Some(1000),
            primary_key_candidate: Some("id".to_string()),
        };

        // Since no transformations were applied, should return base schema as-is
        let merged = transformer.merge_with_base_schema(&base_schema).unwrap();
        assert_eq!(merged.columns.len(), 2);
        assert_eq!(merged.columns[0].name, "id");
        assert_eq!(merged.columns[1].name, "name");
        assert_eq!(merged.estimated_rows, Some(1000));
        assert_eq!(merged.primary_key_candidate, Some("id".to_string()));
    }

    #[test]
    fn test_schema_merge_with_inferred_schema() {
        let config =
            TransformConfig::Inline("derived_col=row.value * 2; new_str='constant'".to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        // Process a row to infer schema
        let mut row = HashMap::new();
        row.insert("value".to_string(), Value::Integer(10));
        transformer.transform_batch(&[row]).unwrap();

        let base_schema = Schema {
            columns: vec![Column {
                name: "value".to_string(),
                data_type: DataType::Integer,
                nullable: false,
            }],
            estimated_rows: Some(100),
            primary_key_candidate: None,
        };

        // Should return the inferred schema from transformations
        let merged = transformer.merge_with_base_schema(&base_schema).unwrap();
        let col_names: Vec<&String> = merged.columns.iter().map(|c| &c.name).collect();
        assert!(col_names.contains(&&"derived_col".to_string()));
        assert!(col_names.contains(&&"new_str".to_string()));
    }

    #[test]
    fn test_lua_string_creation_all_value_types() {
        use std::fs;

        let lua_content = r#"
function transform(row)
    local result = {}
    -- Test string interpolation with date
    if row.date_val then
        result.formatted_date = "Date: " .. row.date_val
    end
    -- Copy all values
    for k, v in pairs(row) do
        result[k] = v
    end
    return result
end
"#;
        let temp_file = "/tmp/test_string_creation.lua";
        fs::write(temp_file, lua_content).unwrap();

        let config = TransformConfig::File(temp_file.to_string());
        let mut transformer = Transformer::new(&config).unwrap();

        let date = chrono::Utc::now();
        let mut row = HashMap::new();
        row.insert("date_val".to_string(), Value::Date(date));
        row.insert("str_val".to_string(), Value::String("test".to_string()));
        row.insert("int_val".to_string(), Value::Integer(123));

        let result = transformer.transform_batch(&[row]).unwrap();
        assert_eq!(result.len(), 1);

        let transformed = &result[0];
        assert!(transformed.contains_key("formatted_date"));
        assert!(transformed.contains_key("date_val"));
        assert!(transformed.contains_key("str_val"));
        assert!(transformed.contains_key("int_val"));

        let _ = fs::remove_file(temp_file);
    }
}
