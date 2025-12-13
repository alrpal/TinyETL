use async_trait::async_trait;
use calamine::{open_workbook, Data as ExcelData, Reader, Xlsx};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    connectors::{Source, Target},
    date_parser::DateParser,
    schema::{Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

pub struct ExcelSource {
    file_path: PathBuf,
    sheet_name: Option<String>,
    data: Vec<Row>,
    headers: Vec<String>,
    current_index: usize,
}

impl ExcelSource {
    pub fn new(file_path: &str) -> Result<Self> {
        // Parse optional sheet name: file.xlsx#SheetName
        let (path, sheet_name) = if file_path.contains('#') {
            let parts: Vec<&str> = file_path.splitn(2, '#').collect();
            (parts[0], Some(parts[1].to_string()))
        } else {
            (file_path, None)
        };

        Ok(Self {
            file_path: PathBuf::from(path),
            sheet_name,
            data: Vec::new(),
            headers: Vec::new(),
            current_index: 0,
        })
    }

    fn excel_value_to_value(&self, excel_val: &ExcelData) -> Value {
        match excel_val {
            ExcelData::Int(i) => Value::Integer(*i),
            ExcelData::Float(f) => {
                // Try to convert to Decimal for precision
                match Decimal::try_from(*f) {
                    Ok(d) => Value::Decimal(d),
                    Err(_) => Value::String(f.to_string()),
                }
            }
            ExcelData::String(s) => {
                // Try to parse as date/datetime first
                if let Some(date_value) = DateParser::try_parse(s) {
                    date_value
                } else {
                    Value::String(s.clone())
                }
            }
            ExcelData::Bool(b) => Value::Boolean(*b),
            ExcelData::DateTime(dt) => {
                // Excel stores dates as f64 (days since 1900-01-01)
                // Convert to string representation
                Value::String(format!("{}", dt))
            }
            ExcelData::DateTimeIso(dt) => {
                // ISO 8601 datetime string
                Value::String(dt.clone())
            }
            ExcelData::DurationIso(d) => {
                // ISO 8601 duration string
                Value::String(d.clone())
            }
            ExcelData::Error(_e) => {
                // Treat errors as null
                Value::Null
            }
            ExcelData::Empty => Value::Null,
        }
    }

    fn load_data(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(format!(
                "Excel file not found: {}",
                self.file_path.display()
            )));
        }

        let mut workbook: Xlsx<_> = open_workbook(&self.file_path).map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to open Excel file {}: {}",
                self.file_path.display(),
                e
            ))
        })?;

        // Determine which sheet to read
        let sheet_name = if let Some(ref name) = self.sheet_name {
            name.clone()
        } else {
            // Use the first sheet if no sheet name is specified
            workbook
                .sheet_names()
                .first()
                .ok_or_else(|| TinyEtlError::Configuration("Excel file has no sheets".to_string()))?
                .clone()
        };

        let range = workbook.worksheet_range(&sheet_name).map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to read sheet '{}': {}", sheet_name, e))
        })?;

        let mut rows_iter = range.rows();

        // First row is headers
        if let Some(header_row) = rows_iter.next() {
            self.headers = header_row
                .iter()
                .map(|cell| match cell {
                    ExcelData::String(s) => s.clone(),
                    ExcelData::Int(i) => i.to_string(),
                    ExcelData::Float(f) => f.to_string(),
                    _ => "Column".to_string(),
                })
                .collect();
        } else {
            return Err(TinyEtlError::Configuration(
                "Excel file has no header row".to_string(),
            ));
        }

        // Read all data rows
        for excel_row in rows_iter {
            let mut row = Row::new();

            for (i, cell) in excel_row.iter().enumerate() {
                if let Some(header) = self.headers.get(i) {
                    let value = self.excel_value_to_value(cell);
                    row.insert(header.clone(), value);
                }
            }

            // Only add non-empty rows
            if !row.is_empty() {
                self.data.push(row);
            }
        }

        Ok(())
    }

    fn infer_schema_with_order(&self, rows: &[Row]) -> Result<Schema> {
        if rows.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        let mut column_types: HashMap<String, Vec<crate::schema::DataType>> = HashMap::new();

        // Use the Excel headers order instead of HashMap iteration order
        for col_name in &self.headers {
            let mut types = Vec::new();
            for row in rows {
                let data_type = match row.get(col_name) {
                    Some(value) => SchemaInferer::infer_type(value),
                    None => crate::schema::DataType::Null,
                };
                types.push(data_type);
            }
            column_types.insert(col_name.clone(), types);
        }

        // Determine final type for each column, preserving header order
        let columns = self
            .headers
            .iter()
            .filter_map(|col_name| {
                column_types.get(col_name).map(|types| {
                    let (data_type, nullable) = SchemaInferer::resolve_column_type(types);
                    crate::schema::Column {
                        name: col_name.clone(),
                        data_type,
                        nullable,
                    }
                })
            })
            .collect();

        Ok(Schema {
            columns,
            estimated_rows: Some(rows.len()),
            primary_key_candidate: None,
        })
    }
}

#[async_trait]
impl Source for ExcelSource {
    async fn connect(&mut self) -> Result<()> {
        self.load_data()?;
        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.data.is_empty() {
            self.connect().await?;
        }

        let sample_data = self
            .data
            .iter()
            .take(sample_size)
            .cloned()
            .collect::<Vec<_>>();
        self.infer_schema_with_order(&sample_data)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        let end_index = std::cmp::min(self.current_index + batch_size, self.data.len());

        for i in self.current_index..end_index {
            rows.push(self.data[i].clone());
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

pub struct ExcelTarget {
    file_path: PathBuf,
    sheet_name: String,
    accumulated_rows: Vec<Row>,
    schema: Option<Schema>,
}

impl ExcelTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        // Parse optional sheet name: file.xlsx#SheetName
        let (path, sheet_name) = if file_path.contains('#') {
            let parts: Vec<&str> = file_path.splitn(2, '#').collect();
            (parts[0], parts[1].to_string())
        } else {
            (file_path, "Sheet1".to_string())
        };

        Ok(Self {
            file_path: PathBuf::from(path),
            sheet_name,
            accumulated_rows: Vec::new(),
            schema: None,
        })
    }

    #[allow(dead_code)]
    fn value_to_excel_value(&self, value: &Value) -> ExcelData {
        match value {
            Value::String(s) => ExcelData::String(s.clone()),
            Value::Integer(i) => ExcelData::Int(*i),
            Value::Decimal(d) => {
                // Convert Decimal to f64 for Excel
                ExcelData::Float(d.to_string().parse::<f64>().unwrap_or(0.0))
            }
            Value::Boolean(b) => ExcelData::Bool(*b),
            Value::Date(d) => ExcelData::String(d.to_string()),
            Value::Json(j) => ExcelData::String(j.to_string()),
            Value::Null => ExcelData::Empty,
        }
    }
}

#[async_trait]
impl Target for ExcelTarget {
    async fn connect(&mut self) -> Result<()> {
        // Excel targets don't need connection setup
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        self.schema = Some(schema.clone());
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        // Accumulate rows for writing at finalize
        self.accumulated_rows.extend(rows.iter().cloned());
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        use xlsxwriter::*;

        let workbook = Workbook::new(&self.file_path.to_string_lossy()).map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to create Excel file {}: {}",
                self.file_path.display(),
                e
            ))
        })?;

        let mut sheet = workbook
            .add_worksheet(Some(&self.sheet_name))
            .map_err(|e| {
                TinyEtlError::Connection(format!(
                    "Failed to create worksheet '{}': {}",
                    self.sheet_name, e
                ))
            })?;

        if let Some(ref schema) = self.schema {
            // Write headers
            for (col_idx, column) in schema.columns.iter().enumerate() {
                sheet
                    .write_string(0, col_idx as u16, &column.name, None)
                    .map_err(|e| {
                        TinyEtlError::DataTransfer(format!("Failed to write header: {}", e))
                    })?;
            }

            // Write data rows
            for (row_idx, row) in self.accumulated_rows.iter().enumerate() {
                for (col_idx, column) in schema.columns.iter().enumerate() {
                    let excel_row = (row_idx + 1) as u32; // +1 for header row
                    let excel_col = col_idx as u16;

                    if let Some(value) = row.get(&column.name) {
                        match value {
                            Value::String(s) => {
                                sheet
                                    .write_string(excel_row, excel_col, s, None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write string: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Integer(i) => {
                                sheet
                                    .write_number(excel_row, excel_col, *i as f64, None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write integer: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Decimal(d) => {
                                let f = d.to_string().parse::<f64>().unwrap_or(0.0);
                                sheet
                                    .write_number(excel_row, excel_col, f, None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write decimal: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Boolean(b) => {
                                sheet
                                    .write_boolean(excel_row, excel_col, *b, None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write boolean: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Date(d) => {
                                sheet
                                    .write_string(excel_row, excel_col, &d.to_string(), None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write date: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Json(j) => {
                                sheet
                                    .write_string(excel_row, excel_col, &j.to_string(), None)
                                    .map_err(|e| {
                                        TinyEtlError::DataTransfer(format!(
                                            "Failed to write json: {}",
                                            e
                                        ))
                                    })?;
                            }
                            Value::Null => {
                                // Leave cell empty for null values
                            }
                        }
                    }
                }
            }
        }

        workbook.close().map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to close Excel file {}: {}",
                self.file_path.display(),
                e
            ))
        })?;

        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // Delete the file if it exists
        if self.file_path.exists() {
            std::fs::remove_file(&self.file_path)?;
        }
        self.accumulated_rows.clear();
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // Excel files don't support true append - we need to read, modify, write
        // For simplicity, we'll return false and rely on truncate mode
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_excel_source_parsing() {
        // This test would require a sample Excel file
        // For now, just test the constructor
        let source = ExcelSource::new("test.xlsx");
        assert!(source.is_ok());

        let source_with_sheet = ExcelSource::new("test.xlsx#Sheet2");
        assert!(source_with_sheet.is_ok());
        assert_eq!(
            source_with_sheet.unwrap().sheet_name,
            Some("Sheet2".to_string())
        );
    }

    #[tokio::test]
    async fn test_excel_target_creation() {
        let target = ExcelTarget::new("output.xlsx");
        assert!(target.is_ok());

        let target_with_sheet = ExcelTarget::new("output.xlsx#MySheet");
        assert!(target_with_sheet.is_ok());
        assert_eq!(target_with_sheet.unwrap().sheet_name, "MySheet");
    }

    #[tokio::test]
    async fn test_excel_value_conversion() {
        let source = ExcelSource::new("test.xlsx").unwrap();

        // Test integer conversion
        let int_val = source.excel_value_to_value(&ExcelData::Int(42));
        assert!(matches!(int_val, Value::Integer(42)));

        // Test float conversion
        let float_val = source.excel_value_to_value(&ExcelData::Float(3.14));
        assert!(matches!(float_val, Value::Decimal(_)));

        // Test string conversion
        let str_val = source.excel_value_to_value(&ExcelData::String("test".to_string()));
        assert!(matches!(str_val, Value::String(_)));

        // Test boolean conversion
        let bool_val = source.excel_value_to_value(&ExcelData::Bool(true));
        assert!(matches!(bool_val, Value::Boolean(true)));

        // Test empty conversion
        let null_val = source.excel_value_to_value(&ExcelData::Empty);
        assert!(matches!(null_val, Value::Null));
    }
}
