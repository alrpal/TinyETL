use async_trait::async_trait;
use csv::{ReaderBuilder, WriterBuilder};
use rust_decimal::Decimal;
use serde_json;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::{
    connectors::{Source, Target},
    date_parser::DateParser,
    schema::{Row, Schema, Value},
    Result, TinyEtlError,
};

/// Whether a path is read and written by the CSV connector (`.csv`, or `.tsv` for tabs)
pub fn is_csv(path: &str) -> bool {
    path.ends_with(".csv") || is_tsv(path)
}

fn is_tsv(path: &str) -> bool {
    path.ends_with(".tsv")
}

/// Endpoint options read by CSV sources and targets (see `crate::options`)
pub const OPTIONS: &[&str] = &["delimiter"];

/// The `delimiter` endpoint option: one ASCII character, or `tab` / `\t`
fn delimiter_option(options: &HashMap<String, String>) -> Result<Option<u8>> {
    let Some(value) = options.get("delimiter") else {
        return Ok(None);
    };
    match value.as_str() {
        "tab" | "\\t" | "\t" => Ok(Some(b'\t')),
        v if v.len() == 1 && v.is_ascii() => Ok(Some(v.as_bytes()[0])),
        v => Err(TinyEtlError::Configuration(format!(
            "Invalid delimiter {:?}: use a single character, or \"tab\"",
            v
        ))),
    }
}

/// Characters tried when a CSV file's delimiter isn't given
const DELIMITER_CANDIDATES: [u8; 4] = *b",\t;|";

/// Guess the delimiter from the first lines of a file: the candidate that appears the same
/// (non-zero) number of times, outside quotes, on every sampled line. Comma when nothing fits.
fn sniff_delimiter(sample: &[u8], complete: bool) -> u8 {
    let mut lines: Vec<[usize; 4]> = Vec::new();
    let mut counts = [0usize; 4];
    let mut in_quotes = false;
    let mut line_has_content = false;
    for &b in sample {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'\n' if !in_quotes => {
                if line_has_content {
                    lines.push(counts);
                }
                counts = [0; 4];
                line_has_content = false;
                if lines.len() == 20 {
                    break;
                }
                continue;
            }
            _ if !in_quotes => {
                if let Some(i) = DELIMITER_CANDIDATES.iter().position(|&c| c == b) {
                    counts[i] += 1;
                }
            }
            _ => {}
        }
        if b != b'\r' {
            line_has_content = true;
        }
    }
    // A last line without a newline counts only if the sample is the whole file
    if line_has_content && complete && lines.len() < 20 {
        lines.push(counts);
    }
    let Some(header) = lines.first() else {
        return b',';
    };
    let mut best: Option<(usize, usize)> = None; // (candidate index, count per line)
    for i in 0..DELIMITER_CANDIDATES.len() {
        let n = header[i];
        if n > 0 && lines.iter().all(|line| line[i] == n) && best.is_none_or(|(_, m)| n > m) {
            best = Some((i, n));
        }
    }
    best.map_or(b',', |(i, _)| DELIMITER_CANDIDATES[i])
}

pub struct CsvSource {
    file_path: PathBuf,
    /// From the `delimiter` option or a `.tsv` name; otherwise detected in `connect`
    delimiter_option: Option<u8>,
    delimiter: u8,
    reader: Option<csv::Reader<std::fs::File>>,
    headers: Vec<String>,
    current_position: u64,
    has_more_data: bool,
}

impl CsvSource {
    pub fn new(file_path: &str) -> Result<Self> {
        Self::with_options(file_path, &HashMap::new())
    }

    pub fn with_options(file_path: &str, options: &HashMap<String, String>) -> Result<Self> {
        let delimiter_option = delimiter_option(options)?.or(is_tsv(file_path).then_some(b'\t'));
        Ok(Self {
            file_path: PathBuf::from(file_path),
            delimiter_option,
            delimiter: delimiter_option.unwrap_or(b','),
            reader: None,
            headers: Vec::new(),
            current_position: 0,
            has_more_data: true,
        })
    }

    fn open_reader(&self) -> Result<csv::Reader<std::fs::File>> {
        let file = std::fs::File::open(&self.file_path)?;
        Ok(ReaderBuilder::new()
            .has_headers(true)
            .delimiter(self.delimiter)
            .from_reader(file))
    }

    fn detect_delimiter(path: &Path) -> Result<u8> {
        let mut sample = Vec::with_capacity(64 * 1024);
        let file = std::fs::File::open(path)?;
        file.take(64 * 1024).read_to_end(&mut sample)?;
        let complete = (sample.len() as u64) < 64 * 1024;
        Ok(sniff_delimiter(&sample, complete))
    }

    fn parse_value(value: &str) -> Value {
        // Try to parse as different types

        // Try integer first
        if let Ok(int_val) = value.parse::<i64>() {
            return Value::Integer(int_val);
        }

        // Try decimal
        if let Ok(decimal_val) = value.parse::<Decimal>() {
            return Value::Decimal(decimal_val);
        }

        // Try boolean
        if let Ok(bool_val) = value.parse::<bool>() {
            return Value::Boolean(bool_val);
        }

        // Try date/datetime using the shared parser
        if let Some(date_value) = DateParser::try_parse(value) {
            return date_value;
        }

        // Default to string
        if value.is_empty() {
            Value::Null
        } else {
            Value::String(value.to_string())
        }
    }
}

#[async_trait]
impl Source for CsvSource {
    async fn connect(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(format!(
                "CSV file not found: {}",
                self.file_path.display()
            )));
        }

        self.delimiter = match self.delimiter_option {
            Some(d) => d,
            None => Self::detect_delimiter(&self.file_path)?,
        };
        if self.delimiter != b',' {
            tracing::info!("Reading with delimiter {:?}", self.delimiter as char);
        }

        let mut reader = self.open_reader()?;

        // Read and store headers
        self.headers = reader.headers()?.iter().map(|h| h.to_string()).collect();

        // Reset file for actual reading
        self.reader = Some(self.open_reader()?);

        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        if self.reader.is_none() {
            self.connect().await?;
        }

        let mut sample_rows = Vec::new();
        let mut count = 0;

        if let Some(ref mut reader) = self.reader {
            for result in reader.records() {
                if count >= sample_size {
                    break;
                }

                let record = result?;
                let mut row = Row::new();

                for (i, field) in record.iter().enumerate() {
                    if let Some(header) = self.headers.get(i) {
                        let value = Self::parse_value(field);
                        row.insert(header.clone(), value);
                    }
                }

                sample_rows.push(row);
                count += 1;
            }
        }

        // Reset for future reading
        self.reset().await?;

        // Custom schema inference that preserves column order from CSV headers
        crate::schema::SchemaInferer::infer_from_rows_ordered(&sample_rows, &self.headers)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.reader.is_none() {
            self.connect().await?;
        }

        let mut rows = Vec::new();
        let mut count = 0;

        if let Some(ref mut reader) = self.reader {
            let mut records_iter = reader.records();

            while count < batch_size {
                match records_iter.next() {
                    Some(Ok(record)) => {
                        let mut row = Row::new();

                        for (i, field) in record.iter().enumerate() {
                            if let Some(header) = self.headers.get(i) {
                                let value = Self::parse_value(field);
                                row.insert(header.clone(), value);
                            }
                        }

                        rows.push(row);
                        count += 1;
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => {
                        // No more records available
                        self.has_more_data = false;
                        break;
                    }
                }
            }
        }

        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        // Count lines in chunks rather than loading the file (quoted newlines make this an
        // estimate, which is all the progress bar needs)
        let mut file = std::fs::File::open(&self.file_path)?;
        let mut buffer = vec![0u8; 1 << 20];
        let (mut lines, mut last) = (0usize, b'\n');
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            lines += buffer[..n].iter().filter(|&&b| b == b'\n').count();
            last = buffer[n - 1];
        }
        if last != b'\n' {
            lines += 1; // a last line without a newline
        }
        // Subtract 1 for the header
        Ok(Some(lines.saturating_sub(1)))
    }

    async fn reset(&mut self) -> Result<()> {
        self.reader = Some(self.open_reader()?);
        self.current_position = 0;
        self.has_more_data = true;
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.has_more_data && self.reader.is_some()
    }
}

pub struct CsvTarget {
    file_path: PathBuf,
    delimiter: u8,
    writer: Option<csv::Writer<std::fs::File>>,
    headers_written: bool,
    column_order: Vec<String>,
}

impl CsvTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        Self::with_options(file_path, &HashMap::new())
    }

    pub fn with_options(file_path: &str, options: &HashMap<String, String>) -> Result<Self> {
        let delimiter = delimiter_option(options)?
            .or(is_tsv(file_path).then_some(b'\t'))
            .unwrap_or(b',');
        Ok(Self {
            file_path: PathBuf::from(file_path),
            delimiter,
            writer: None,
            headers_written: false,
            column_order: Vec::new(),
        })
    }

    fn open_writer(&self) -> Result<csv::Writer<std::fs::File>> {
        let file = std::fs::File::create(&self.file_path)?;
        Ok(WriterBuilder::new()
            .delimiter(self.delimiter)
            .from_writer(file))
    }

    fn value_to_string(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Decimal(d) => d.to_string(),
            Value::Boolean(b) => b.to_string(),
            Value::Date(dt) => dt.to_rfc3339(),
            Value::Json(j) => serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string()),
            Value::Null => String::new(),
        }
    }
}

#[async_trait]
impl Target for CsvTarget {
    async fn connect(&mut self) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.writer = Some(self.open_writer()?);
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        if self.writer.is_none() {
            self.connect().await?;
        }

        // Store column order from schema
        self.column_order = schema.columns.iter().map(|c| c.name.clone()).collect();

        // Write headers
        if let Some(ref mut writer) = self.writer {
            writer.write_record(&self.column_order)?;
            self.headers_written = true;
        }

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if self.writer.is_none() {
            return Err(TinyEtlError::Connection("Writer not connected".to_string()));
        }

        let mut written_count = 0;

        if let Some(ref mut writer) = self.writer {
            for row in rows {
                // Use the stored column order from the schema
                let record: Vec<String> = if !self.column_order.is_empty() {
                    // Use schema-defined column order
                    self.column_order
                        .iter()
                        .map(|key| {
                            row.get(key)
                                .map(CsvTarget::value_to_string)
                                .unwrap_or_default()
                        })
                        .collect()
                } else {
                    // Fallback for when schema wasn't used (maintain old behavior)
                    let keys: Vec<_> = row.keys().cloned().collect();
                    keys.iter()
                        .map(|key| {
                            row.get(key)
                                .map(CsvTarget::value_to_string)
                                .unwrap_or_default()
                        })
                        .collect()
                };

                writer.write_record(&record)?;
                written_count += 1;
            }
        }

        Ok(written_count)
    }

    async fn finalize(&mut self) -> Result<()> {
        if let Some(ref mut writer) = self.writer {
            writer.flush()?;
        }
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // For CSV files, truncation means recreating the file
        self.writer = Some(self.open_writer()?);
        self.headers_written = false;
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // CSV files don't support true append (would need to skip headers)
        // So we return false to force truncation for existing files
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_csv_source_connection() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name,age").unwrap();
        writeln!(temp_file, "1,Alice,25").unwrap();
        writeln!(temp_file, "2,Bob,30").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_csv_source_schema_inference() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name,age,active").unwrap();
        writeln!(temp_file, "1,Alice,25,true").unwrap();
        writeln!(temp_file, "2,Bob,30,false").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let schema = source.infer_schema(10).await.unwrap();
        assert_eq!(schema.columns.len(), 4);

        let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, crate::schema::DataType::Integer);
    }

    #[tokio::test]
    async fn test_csv_source_read_batch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap();
        writeln!(temp_file, "2,Bob").unwrap();
        writeln!(temp_file, "3,Charlie").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let rows = source.read_batch(2).await.unwrap();
        assert_eq!(rows.len(), 2);

        if let Some(Value::String(name)) = rows[0].get("name") {
            assert_eq!(name, "Alice");
        } else {
            panic!("Expected string value for name");
        }
    }

    #[tokio::test]
    async fn test_csv_target_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let result = target.connect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_value_types() {
        // Test integer parsing
        assert!(matches!(CsvSource::parse_value("42"), Value::Integer(42)));

        // Test decimal parsing
        let decimal_result = CsvSource::parse_value("3.14");
        assert!(matches!(decimal_result, Value::Decimal(d) if d == Decimal::new(314, 2)));

        // Test boolean parsing
        assert!(matches!(
            CsvSource::parse_value("true"),
            Value::Boolean(true)
        ));
        assert!(matches!(
            CsvSource::parse_value("false"),
            Value::Boolean(false)
        ));

        // Test datetime parsing
        assert!(matches!(
            CsvSource::parse_value("2023-01-01T12:00:00Z"),
            Value::Date(_)
        ));

        // Test empty string to null
        assert!(matches!(CsvSource::parse_value(""), Value::Null));

        // Test string fallback
        assert!(matches!(CsvSource::parse_value("hello"), Value::String(s) if s == "hello"));
    }

    #[tokio::test]
    async fn test_csv_source_file_not_found() {
        let mut source = CsvSource::new("/nonexistent/file.csv").unwrap();
        let result = source.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_csv_source_estimated_row_count() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap();
        writeln!(temp_file, "2,Bob").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let count = source.estimated_row_count().await.unwrap();
        assert_eq!(count, Some(2)); // 2 data rows (excluding header)

        // A last line without a trailing newline still counts
        let mut no_newline = NamedTempFile::new().unwrap();
        write!(no_newline, "id\n1\n2").unwrap();
        let source = CsvSource::new(no_newline.path().to_str().unwrap()).unwrap();
        assert_eq!(source.estimated_row_count().await.unwrap(), Some(2));
    }

    #[tokio::test]
    async fn test_csv_source_has_more() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "id,name").unwrap();
        writeln!(temp_file, "1,Alice").unwrap();

        let mut source = CsvSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        assert!(source.has_more());
    }

    #[tokio::test]
    async fn test_csv_target_write_batch_without_connection() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let row = std::collections::HashMap::new();
        let result = target.write_batch(&[row]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_csv_target_create_table_and_write() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        // Create schema
        let schema = crate::schema::Schema {
            columns: vec![
                crate::schema::Column {
                    name: "id".to_string(),
                    data_type: crate::schema::DataType::Integer,
                    nullable: false,
                },
                crate::schema::Column {
                    name: "name".to_string(),
                    data_type: crate::schema::DataType::String,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        target.create_table("test", &schema).await.unwrap();

        // Write some data
        let mut row = std::collections::HashMap::new();
        row.insert("id".to_string(), Value::Integer(1));
        row.insert("name".to_string(), Value::String("Alice".to_string()));

        let written = target.write_batch(&[row]).await.unwrap();
        assert_eq!(written, 1);

        target.finalize().await.unwrap();
    }

    #[tokio::test]
    async fn test_csv_target_value_to_string() {
        assert_eq!(
            CsvTarget::value_to_string(&Value::String("test".to_string())),
            "test"
        );
        assert_eq!(CsvTarget::value_to_string(&Value::Integer(42)), "42");
        assert_eq!(
            CsvTarget::value_to_string(&Value::Decimal(Decimal::new(314, 2))),
            "3.14"
        );
        assert_eq!(CsvTarget::value_to_string(&Value::Boolean(true)), "true");
        assert_eq!(CsvTarget::value_to_string(&Value::Null), "");

        let dt = chrono::Utc::now();
        assert!(CsvTarget::value_to_string(&Value::Date(dt)).contains("T"));
    }

    #[tokio::test]
    async fn test_csv_target_exists() {
        let temp_file = NamedTempFile::new().unwrap();
        let target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let exists = target.exists("test").await.unwrap();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_csv_target_write_batch_with_all_value_types() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(temp_file.path().to_str().unwrap()).unwrap();
        target.connect().await.unwrap();

        let mut row = std::collections::HashMap::new();
        row.insert("string".to_string(), Value::String("test".to_string()));
        row.insert("integer".to_string(), Value::Integer(42));
        row.insert("decimal".to_string(), Value::Decimal(Decimal::new(314, 2)));
        row.insert("boolean".to_string(), Value::Boolean(true));
        row.insert("date".to_string(), Value::Date(chrono::Utc::now()));
        row.insert("null".to_string(), Value::Null);

        let written = target.write_batch(&[row]).await.unwrap();
        assert_eq!(written, 1);
    }

    #[tokio::test]
    async fn test_csv_column_order_debug() {
        // Create source CSV with specific header order
        let mut source_file = NamedTempFile::new().unwrap();
        writeln!(source_file, "one,two,three").unwrap();
        writeln!(source_file, "1,blah,bell").unwrap();

        // Set up source
        let mut source = CsvSource::new(source_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        // Debug: print headers from source
        println!("Source headers: {:?}", source.headers);

        // Infer schema
        let schema = source.infer_schema(10).await.unwrap();

        // Debug: print schema column order
        let schema_columns: Vec<_> = schema.columns.iter().map(|c| c.name.clone()).collect();
        println!("Schema columns: {:?}", schema_columns);

        // Reset source and read data
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();

        // Debug: print first row data
        if let Some(first_row) = rows.first() {
            println!("First row keys: {:?}", first_row.keys().collect::<Vec<_>>());
            for (key, value) in first_row {
                println!("  {}: {:?}", key, value);
            }
        }

        // Set up target
        let target_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();

        // Debug: print target column order
        println!("Target column order: {:?}", target.column_order);

        // Write data to target
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // Read and print the output
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        println!("Output content:\n{}", output_content);
    }

    #[tokio::test]
    async fn test_csv_to_csv_full_roundtrip() {
        // Create source CSV with specific header order
        let mut source_file = NamedTempFile::new().unwrap();
        writeln!(source_file, "one,two,three").unwrap();
        writeln!(source_file, "1,blah,bell").unwrap();
        writeln!(source_file, "2,blo,aff").unwrap();

        // Set up source
        let mut source = CsvSource::new(source_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        // Infer schema
        let schema = source.infer_schema(10).await.unwrap();

        // Verify schema preserves original column order
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.columns[0].name, "one");
        assert_eq!(schema.columns[1].name, "two");
        assert_eq!(schema.columns[2].name, "three");

        // Set up target
        let target_file = NamedTempFile::new().unwrap();
        let mut target = CsvTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();

        // Reset source and read all data
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();

        // Write data to target
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // Read the output file and verify header order is preserved
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        let lines: Vec<&str> = output_content.lines().collect();

        // Check that headers are in the correct order (same as input)
        let header_line = lines[0];
        assert_eq!(
            header_line, "one,two,three",
            "Headers should maintain original CSV order"
        );

        // Verify data integrity and order
        assert_eq!(lines.len(), 3); // header + 2 data rows
        assert_eq!(
            lines[1], "1,blah,bell",
            "First data row should maintain column order"
        );
        assert_eq!(
            lines[2], "2,blo,aff",
            "Second data row should maintain column order"
        );
    }

    #[test]
    fn test_sniff_delimiter() {
        assert_eq!(sniff_delimiter(b"a,b,c\n1,2,3\n", true), b',');
        assert_eq!(sniff_delimiter(b"a\tb\tc\n1\t2\t3\n", true), b'\t');
        assert_eq!(sniff_delimiter(b"a;b\r\n1,5;2\r\n", true), b';');
        assert_eq!(sniff_delimiter(b"a|b|c\n1|2|3", true), b'|');
        // Delimiters inside quotes don't count, and quoted fields may span lines
        assert_eq!(
            sniff_delimiter(b"name;note\n\"Smith, J\";\"a\nb, c\"\n", true),
            b';'
        );
        // Commas in a tab file's values don't win when they aren't on every line
        assert_eq!(sniff_delimiter(b"a\tb\n1,5\t2\n3\t4\n", true), b'\t');
        // One column, or nothing consistent: comma
        assert_eq!(sniff_delimiter(b"id\n1\n2\n", true), b',');
        assert_eq!(sniff_delimiter(b"", true), b',');
        // A cut-off last line in a partial sample is ignored
        assert_eq!(sniff_delimiter(b"a\tb\n1\t2\n3", false), b'\t');
    }

    #[test]
    fn test_delimiter_option() {
        let opt = |v: &str| HashMap::from([("delimiter".to_string(), v.to_string())]);
        assert_eq!(delimiter_option(&HashMap::new()).unwrap(), None);
        assert_eq!(delimiter_option(&opt("|")).unwrap(), Some(b'|'));
        assert_eq!(delimiter_option(&opt("tab")).unwrap(), Some(b'\t'));
        assert_eq!(delimiter_option(&opt("\\t")).unwrap(), Some(b'\t'));
        assert_eq!(delimiter_option(&opt("\t")).unwrap(), Some(b'\t'));
        assert!(delimiter_option(&opt("||")).is_err());
        assert!(delimiter_option(&opt("")).is_err());
    }

    async fn read_all(source: &mut CsvSource) -> (Vec<String>, Vec<Row>) {
        source.connect().await.unwrap();
        source.infer_schema(100).await.unwrap();
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();
        (source.headers.clone(), rows)
    }

    #[tokio::test]
    async fn test_other_delimiters() {
        let dir = tempfile::tempdir().unwrap();

        // Detected in a .csv file
        let pipes = dir.path().join("pipes.csv");
        std::fs::write(&pipes, "id|name\n1|Smith, J\n2|Lee\n").unwrap();
        let (headers, rows) = read_all(&mut CsvSource::new(pipes.to_str().unwrap()).unwrap()).await;
        assert_eq!(headers, ["id", "name"]);
        assert_eq!(rows[0]["name"], Value::String("Smith, J".to_string()));

        // .tsv is tab-separated without detection
        let tsv = dir.path().join("data.tsv");
        std::fs::write(&tsv, "id\tnote\n1\ta;b;c\n").unwrap();
        let (headers, rows) = read_all(&mut CsvSource::new(tsv.to_str().unwrap()).unwrap()).await;
        assert_eq!(headers, ["id", "note"]);
        assert_eq!(rows[0]["note"], Value::String("a;b;c".to_string()));

        // The option overrides detection
        let semi = dir.path().join("semi.csv");
        std::fs::write(&semi, "a;b\n1;2\n").unwrap();
        let options = HashMap::from([("delimiter".to_string(), ",".to_string())]);
        let mut source = CsvSource::with_options(semi.to_str().unwrap(), &options).unwrap();
        let (headers, _) = read_all(&mut source).await;
        assert_eq!(headers, ["a;b"]);

        // Targets: .tsv writes tabs, the option writes anything
        let schema = crate::schema::SchemaInferer::infer_from_rows_ordered(
            &rows,
            &["id".to_string(), "note".to_string()],
        )
        .unwrap();
        for (name, options, expected) in [
            ("out.tsv", HashMap::new(), "id\tnote\n1\ta;b;c\n"),
            (
                "out.csv",
                HashMap::from([("delimiter".to_string(), "|".to_string())]),
                "id|note\n1|a;b;c\n",
            ),
            ("plain.csv", HashMap::new(), "id,note\n1,a;b;c\n"),
        ] {
            let path = dir.path().join(name);
            let mut target = CsvTarget::with_options(path.to_str().unwrap(), &options).unwrap();
            target.create_table("t", &schema).await.unwrap();
            target.write_batch(&rows).await.unwrap();
            target.finalize().await.unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        }
    }
}
