//! Excel workbooks: read `.xlsx`, `.xlsm`, `.xlsb`, `.xls` and `.ods` (calamine), write `.xlsx`
//! (rust_xlsxwriter). `file.xlsx#Sheet` picks the sheet; both libraries are pure Rust.

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Reader};
use chrono::{NaiveTime, TimeZone, Utc};
use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Workbook};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::{
    connectors::{Source, Target},
    date_parser::DateParser,
    schema::{Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

/// File extensions read as spreadsheets.
const READ_EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "ods"];

/// Rows in an Excel sheet, including the header row.
const MAX_ROWS: u32 = 1_048_576;

/// Columns in an Excel sheet.
const MAX_COLUMNS: usize = 16_384;

/// Integers beyond ±2^53 lose digits as Excel numbers (doubles), so they are written as text.
const MAX_EXACT_INTEGER: i64 = 1 << 53;

/// Split `file.xlsx#Sheet` into the path and the sheet name.
fn split_sheet(connection_string: &str) -> (&str, Option<&str>) {
    match connection_string.split_once('#') {
        Some((path, sheet)) if !sheet.is_empty() => (path, Some(sheet)),
        Some((path, _)) => (path, None),
        None => (connection_string, None),
    }
}

fn extension(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
}

/// Whether `connection_string` names a spreadsheet this connector reads.
pub fn is_spreadsheet(connection_string: &str) -> bool {
    let (path, _) = split_sheet(connection_string);
    extension(path).is_some_and(|ext| READ_EXTENSIONS.contains(&ext.as_str()))
}

/// Column names from the header row: blank headers become `column_N`, and repeated names get
/// `_2`, `_3`, … so no column overwrites another.
fn header_names(cells: &[Data]) -> Vec<String> {
    let mut seen = HashSet::new();
    cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let text = match cell {
                Data::Empty => String::new(),
                Data::Float(f) if f.fract() == 0.0 => format!("{}", *f as i64),
                other => other.to_string().trim().to_string(),
            };
            let base = if text.is_empty() {
                format!("column_{}", i + 1)
            } else {
                text
            };
            let mut name = base.clone();
            let mut n = 2;
            while !seen.insert(name.clone()) {
                name = format!("{}_{}", base, n);
                n += 1;
            }
            name
        })
        .collect()
}

/// A cell as a TinyETL value. Text cells stay text (Excel already typed them, and text such as
/// `02134` must keep its leading zero); numbers without a fractional part become integers,
/// because Excel stores every number as a double.
fn cell_value(cell: &Data) -> Value {
    match cell {
        Data::Empty | Data::Error(_) => Value::Null,
        Data::Int(i) => Value::Integer(*i),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < MAX_EXACT_INTEGER as f64 {
                Value::Integer(*f as i64)
            } else {
                match Decimal::try_from(*f) {
                    Ok(d) => Value::Decimal(d),
                    Err(_) => Value::String(f.to_string()),
                }
            }
        }
        Data::String(s) => Value::String(s.clone()),
        Data::Bool(b) => Value::Boolean(*b),
        Data::DateTime(dt) if dt.is_duration() => {
            // A time span (e.g. [h]:mm:ss); keep it readable as text
            let secs = (dt.as_f64() * 86_400.0).round() as i64;
            Value::String(format!(
                "{}:{:02}:{:02}",
                secs / 3600,
                (secs % 3600) / 60,
                secs % 60
            ))
        }
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(naive) => Value::Date(Utc.from_utc_datetime(&naive)),
            None => Value::String(dt.as_f64().to_string()),
        },
        Data::DateTimeIso(s) => {
            DateParser::try_parse(s).unwrap_or_else(|| Value::String(s.clone()))
        }
        Data::DurationIso(s) => Value::String(s.clone()),
    }
}

pub struct ExcelSource {
    file_path: PathBuf,
    sheet_name: Option<String>,
    headers: Vec<String>,
    rows: Vec<Row>,
    position: usize,
}

impl ExcelSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (path, sheet) = split_sheet(connection_string);
        Ok(Self {
            file_path: PathBuf::from(path),
            sheet_name: sheet.map(str::to_string),
            headers: Vec::new(),
            rows: Vec::new(),
            position: 0,
        })
    }

    /// Read the whole sheet (the spreadsheet libraries load a sheet at once anyway).
    fn load(&mut self) -> Result<()> {
        let mut workbook = open_workbook_auto(&self.file_path).map_err(|e| {
            TinyEtlError::Connection(format!(
                "Failed to open spreadsheet {}: {}",
                self.file_path.display(),
                e
            ))
        })?;
        let sheets = workbook.sheet_names();
        let sheet = match &self.sheet_name {
            Some(name) if sheets.contains(name) => name.clone(),
            Some(name) => {
                return Err(TinyEtlError::Configuration(format!(
                    "Sheet '{}' not found in {}. Sheets: {}",
                    name,
                    self.file_path.display(),
                    sheets.join(", ")
                )))
            }
            None => sheets.first().cloned().ok_or_else(|| {
                TinyEtlError::Configuration(format!("{} has no sheets", self.file_path.display()))
            })?,
        };
        let range = workbook.worksheet_range(&sheet).map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to read sheet '{}': {}", sheet, e))
        })?;

        let mut rows = range.rows();
        self.headers = rows.next().map(header_names).unwrap_or_default();
        self.rows = rows
            .filter(|cells| cells.iter().any(|cell| !matches!(cell, Data::Empty)))
            .map(|cells| {
                self.headers
                    .iter()
                    .zip(cells)
                    .map(|(name, cell)| (name.clone(), cell_value(cell)))
                    .collect()
            })
            .collect();
        self.position = 0;
        Ok(())
    }
}

#[async_trait]
impl Source for ExcelSource {
    async fn connect(&mut self) -> Result<()> {
        self.load()
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        let sample = &self.rows[..self.rows.len().min(sample_size)];
        let mut schema = SchemaInferer::infer_from_rows_ordered(sample, &self.headers)?;
        schema.estimated_rows = Some(self.rows.len());
        Ok(schema)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let end = (self.position + batch_size).min(self.rows.len());
        let batch = self.rows[self.position..end].to_vec();
        self.position = end;
        Ok(batch)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(Some(self.rows.len()))
    }

    async fn reset(&mut self) -> Result<()> {
        self.position = 0;
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.position < self.rows.len()
    }
}

/// Writes one sheet of an `.xlsx` workbook. Rows go to a temporary file as they arrive
/// (rust_xlsxwriter's constant-memory mode), and the workbook is assembled in `finalize`.
pub struct ExcelTarget {
    file_path: PathBuf,
    sheet_name: String,
    workbook: Option<Workbook>,
    schema: Option<Schema>,
    next_row: u32,
    date_format: Format,
    datetime_format: Format,
}

impl ExcelTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (path, sheet) = split_sheet(connection_string);
        if extension(path).as_deref() != Some("xlsx") {
            return Err(TinyEtlError::Configuration(format!(
                "TinyETL writes Excel files as .xlsx; {} has a different extension",
                path
            )));
        }
        Ok(Self {
            file_path: PathBuf::from(path),
            sheet_name: sheet.unwrap_or("Sheet1").to_string(),
            workbook: None,
            schema: None,
            next_row: 0,
            date_format: Format::new().set_num_format("yyyy-mm-dd"),
            datetime_format: Format::new().set_num_format("yyyy-mm-dd hh:mm:ss"),
        })
    }

    fn xlsx_error(&self, what: &str, e: rust_xlsxwriter::XlsxError) -> TinyEtlError {
        TinyEtlError::DataTransfer(format!(
            "Excel {} ({}): {}",
            what,
            self.file_path.display(),
            e
        ))
    }

    /// Start the workbook and write the header row.
    fn start(&mut self, schema: &Schema) -> Result<()> {
        if schema.columns.len() > MAX_COLUMNS {
            return Err(TinyEtlError::Configuration(format!(
                "Excel sheets hold at most {} columns; the data has {}",
                MAX_COLUMNS,
                schema.columns.len()
            )));
        }
        let mut workbook = Workbook::new();
        let header = Format::new().set_bold();
        let sheet = workbook
            .add_worksheet_with_constant_memory()
            .map_err(|e| self.xlsx_error("worksheet", e))?;
        sheet
            .set_name(&self.sheet_name)
            .map_err(|e| self.xlsx_error(&format!("sheet name '{}'", self.sheet_name), e))?;
        for (col, column) in schema.columns.iter().enumerate() {
            sheet
                .write_string_with_format(0, col as u16, &column.name, &header)
                .map_err(|e| self.xlsx_error("header", e))?;
        }
        self.workbook = Some(workbook);
        self.next_row = 1;
        Ok(())
    }
}

#[async_trait]
impl Target for ExcelTarget {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        self.schema = Some(schema.clone());
        self.start(schema)
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?;
        let workbook = self
            .workbook
            .as_mut()
            .ok_or_else(|| TinyEtlError::Configuration("Workbook not started".to_string()))?;
        let sheet = workbook
            .worksheet_from_index(0)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Excel worksheet: {}", e)))?;

        for row in rows {
            if self.next_row >= MAX_ROWS {
                return Err(TinyEtlError::DataTransfer(format!(
                    "Excel sheets hold at most {} rows ({} plus the header); write a CSV or \
                     Parquet file instead",
                    MAX_ROWS,
                    MAX_ROWS - 1
                )));
            }
            let r = self.next_row;
            for (c, column) in schema.columns.iter().enumerate() {
                let c = c as u16;
                let value = row.get(&column.name).unwrap_or(&Value::Null);
                if matches!(value, Value::Null) {
                    continue;
                }
                let written = match value {
                    Value::Null => unreachable!("skipped above"),
                    Value::String(s) => sheet.write_string(r, c, s),
                    Value::Integer(i) if i.abs() <= MAX_EXACT_INTEGER => {
                        sheet.write_number(r, c, *i as f64)
                    }
                    Value::Integer(i) => sheet.write_string(r, c, i.to_string()),
                    Value::Decimal(d) => match f64::try_from(*d) {
                        Ok(f) => sheet.write_number(r, c, f),
                        Err(_) => sheet.write_string(r, c, d.to_string()),
                    },
                    Value::Boolean(b) => sheet.write_boolean(r, c, *b),
                    Value::Date(dt) => {
                        let naive = dt.naive_utc();
                        let format = if naive.time() == NaiveTime::MIN {
                            &self.date_format
                        } else {
                            &self.datetime_format
                        };
                        sheet.write_datetime_with_format(r, c, naive, format)
                    }
                    Value::Json(j) => sheet.write_string(r, c, j.to_string()),
                };
                written.map(|_| ()).map_err(|e| {
                    TinyEtlError::DataTransfer(format!(
                        "Excel: failed to write column '{}' of row {}: {}",
                        column.name, r, e
                    ))
                })?;
            }
            self.next_row += 1;
        }
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        if self.workbook.is_none() {
            // No create_table call (an empty source): still produce a valid workbook
            let schema = self.schema.clone().unwrap_or(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
            self.start(&schema)?;
        }
        let path = self.file_path.clone();
        let mut workbook = self.workbook.take().expect("started above");
        workbook.save(&path).map_err(|e| self.xlsx_error("save", e))
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // The file is replaced when the workbook is saved
        Ok(())
    }

    fn supports_append(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType};
    use std::collections::HashMap;
    use std::str::FromStr;

    fn column(name: &str, data_type: DataType) -> Column {
        Column {
            name: name.to_string(),
            data_type,
            nullable: true,
        }
    }

    fn schema(columns: Vec<Column>) -> Schema {
        Schema {
            columns,
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }

    async fn write(path: &str, schema: &Schema, rows: &[Row]) {
        let mut target = ExcelTarget::new(path).unwrap();
        target.connect().await.unwrap();
        target.create_table("t", schema).await.unwrap();
        target.write_batch(rows).await.unwrap();
        target.finalize().await.unwrap();
    }

    async fn read(path: &str) -> (Schema, Vec<Row>) {
        let mut source = ExcelSource::new(path).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(1000).await.unwrap();
        let mut rows = Vec::new();
        while source.has_more() {
            rows.extend(source.read_batch(2).await.unwrap());
        }
        (schema, rows)
    }

    #[test]
    fn test_spreadsheet_detection() {
        assert!(is_spreadsheet("data.xlsx"));
        assert!(is_spreadsheet("data.XLSX"));
        assert!(is_spreadsheet("data.xls#Sales"));
        assert!(is_spreadsheet("data.ods"));
        assert!(!is_spreadsheet("data.csv"));
        assert!(!is_spreadsheet("xlsx.csv"));
        assert_eq!(split_sheet("a.xlsx#Q1 2024"), ("a.xlsx", Some("Q1 2024")));
        assert_eq!(split_sheet("a.xlsx#"), ("a.xlsx", None));
    }

    #[test]
    fn test_target_only_writes_xlsx() {
        assert!(ExcelTarget::new("out.xlsx").is_ok());
        assert!(ExcelTarget::new("out.xls").is_err());
        assert_eq!(
            ExcelTarget::new("out.xlsx#Report").unwrap().sheet_name,
            "Report"
        );
    }

    #[test]
    fn test_header_names() {
        let names = header_names(&[
            Data::String("id".into()),
            Data::Empty,
            Data::String("id".into()),
            Data::Float(2024.0),
        ]);
        assert_eq!(names, ["id", "column_2", "id_2", "2024"]);
    }

    #[test]
    fn test_cell_values() {
        assert_eq!(cell_value(&Data::Float(42.0)), Value::Integer(42));
        assert_eq!(
            cell_value(&Data::Float(29.85)),
            Value::Decimal(Decimal::from_str("29.85").unwrap())
        );
        assert_eq!(
            cell_value(&Data::String("02134".into())),
            Value::String("02134".into())
        );
        assert_eq!(cell_value(&Data::Empty), Value::Null);
    }

    #[tokio::test]
    async fn test_round_trip_keeps_types_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xlsx");
        let path = path.to_str().unwrap();
        let schema = schema(vec![
            column("name", DataType::String),
            column("qty", DataType::Integer),
            column("price", DataType::Decimal),
            column("active", DataType::Boolean),
            column("day", DataType::Date),
            column("at", DataType::DateTime),
            column("big", DataType::Integer),
        ]);
        let day = Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let at = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 5).unwrap();
        let row: Row = HashMap::from([
            ("name".to_string(), Value::String("Zoë 🚀".into())),
            ("qty".to_string(), Value::Integer(3)),
            (
                "price".to_string(),
                Value::Decimal(Decimal::from_str("19.99").unwrap()),
            ),
            ("active".to_string(), Value::Boolean(true)),
            ("day".to_string(), Value::Date(day)),
            ("at".to_string(), Value::Date(at)),
            ("big".to_string(), Value::Integer(9_007_199_254_740_993)),
        ]);
        let blank: Row = HashMap::from([("name".to_string(), Value::Null)]);
        let second: Row = HashMap::from([("name".to_string(), Value::String("b".into()))]);
        write(path, &schema, &[row, blank, second]).await;

        let (read_schema, rows) = read(path).await;
        let names: Vec<&str> = read_schema
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["name", "qty", "price", "active", "day", "at", "big"]
        );
        assert_eq!(rows.len(), 2, "the all-empty row is skipped");
        let r = &rows[0];
        assert_eq!(r["name"], Value::String("Zoë 🚀".into()));
        assert_eq!(r["qty"], Value::Integer(3));
        assert_eq!(
            r["price"],
            Value::Decimal(Decimal::from_str("19.99").unwrap())
        );
        assert_eq!(r["active"], Value::Boolean(true));
        assert_eq!(r["day"], Value::Date(day));
        assert_eq!(r["at"], Value::Date(at));
        // Beyond 2^53: written as text so no digits are lost
        assert_eq!(r["big"], Value::String("9007199254740993".into()));
        assert_eq!(rows[1]["qty"], Value::Null);
    }

    #[tokio::test]
    async fn test_sheet_selection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("book.xlsx");
        let path = format!("{}#Sales", file.display());
        let row: Row = HashMap::from([("x".to_string(), Value::Integer(1))]);
        write(&path, &schema(vec![column("x", DataType::Integer)]), &[row]).await;

        let (_, rows) = read(&path).await;
        assert_eq!(rows[0]["x"], Value::Integer(1));
        // The first sheet is the default
        let (_, rows) = read(file.to_str().unwrap()).await;
        assert_eq!(rows.len(), 1);

        let mut missing = ExcelSource::new(&format!("{}#Nope", file.display())).unwrap();
        let err = missing.connect().await.unwrap_err().to_string();
        assert!(err.contains("Sheets: Sales"), "{}", err);
    }
}
