//! JSON files: an array of objects (`.json`) or one object per line (JSON Lines: `.jsonl`,
//! `.ndjson`). Both are read and written as streams, so memory stays at about one batch.
//! A `.json` file that starts with `{` is read as a sequence of objects, which covers JSON
//! Lines saved as `.json`, pretty-printed objects one after another, and a single object.

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::de::{Deserializer as _, SeqAccess, Visitor};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;

use crate::{
    connectors::{output_file::OutputFile, Source, Target},
    date_parser::DateParser,
    schema::{Row, Schema, SchemaInferer, Value},
    Result, TinyEtlError,
};

/// JSON values decoded ahead of the reader.
const READ_AHEAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
enum JsonFormat {
    /// `[ {...}, {...} ]`
    Array,
    /// One object per line
    Lines,
    /// Objects one after another, separated by any whitespace (read only)
    Sequence,
}

impl JsonFormat {
    fn for_path(path: &Path) -> Self {
        match path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .as_deref()
        {
            Some("jsonl") | Some("ndjson") => JsonFormat::Lines,
            _ => JsonFormat::Array,
        }
    }
}

/// Whether a connection string names a JSON Lines file.
pub fn is_json_lines(connection_string: &str) -> bool {
    JsonFormat::for_path(Path::new(connection_string)) == JsonFormat::Lines
}

type Item = std::result::Result<serde_json::Value, String>;

/// The values of a JSON file, decoded on a background thread one at a time and handed over
/// through a bounded channel. Dropping it stops the thread.
struct JsonValues {
    // Mutex only to make the receiver Sync (Source requires it); reached through get_mut
    receiver: Mutex<Receiver<Item>>,
    peeked: Option<Item>,
    done: bool,
}

impl JsonValues {
    fn open(path: &Path, format: JsonFormat) -> Result<Self> {
        let file = File::open(path).map_err(|e| {
            TinyEtlError::Connection(format!("Failed to open {}: {}", path.display(), e))
        })?;
        let mut reader = BufReader::new(file);
        // Skip a UTF-8 byte order mark, which serde_json rejects
        if reader.fill_buf()?.starts_with(&[0xEF, 0xBB, 0xBF]) {
            reader.consume(3);
        }
        let (sender, receiver) = sync_channel(READ_AHEAD);
        std::thread::Builder::new()
            .name("tinyetl-json-reader".to_string())
            .spawn(move || match format {
                JsonFormat::Array => read_array(reader, &sender),
                JsonFormat::Lines => read_lines(reader, &sender),
                JsonFormat::Sequence => read_sequence(reader, &sender),
            })
            .map_err(TinyEtlError::Io)?;
        let mut values = Self {
            receiver: Mutex::new(receiver),
            peeked: None,
            done: false,
        };
        values.peek();
        Ok(values)
    }

    fn peek(&mut self) {
        if self.peeked.is_none() && !self.done {
            let receiver = self.receiver.get_mut().unwrap_or_else(|e| e.into_inner());
            match receiver.recv() {
                Ok(item) => self.peeked = Some(item),
                Err(_) => self.done = true, // the reader thread finished
            }
        }
    }

    fn has_more(&self) -> bool {
        self.peeked.is_some()
    }

    fn next_value(&mut self) -> Result<Option<serde_json::Value>> {
        self.peek();
        let item = self.peeked.take();
        self.peek();
        match item {
            None => Ok(None),
            Some(Ok(value)) => Ok(Some(value)),
            Some(Err(message)) => Err(TinyEtlError::DataTransfer(message)),
        }
    }
}

/// Send each element of a top-level JSON array.
fn read_array(reader: BufReader<File>, sender: &SyncSender<Item>) {
    struct Elements<'a>(&'a SyncSender<Item>);

    impl<'de> Visitor<'de> for Elements<'_> {
        type Value = ();

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an array of objects")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                if self.0.send(Ok(value)).is_err() {
                    break; // nobody is reading any more
                }
            }
            Ok(())
        }
    }

    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let result = deserializer
        .deserialize_seq(Elements(sender))
        .and_then(|()| deserializer.end());
    if let Err(e) = result {
        let message = if e.is_data() && e.line() == 1 && e.column() <= 1 {
            "JSON file must contain an array of objects".to_string()
        } else {
            format!("Invalid JSON: {}", e)
        };
        let _ = sender.send(Err(message));
    }
}

/// Send each non-blank line of a JSON Lines file.
fn read_lines(reader: BufReader<File>, sender: &SyncSender<Item>) {
    for (index, line) in reader.lines().enumerate() {
        let item = match line {
            Err(e) => Err(format!("Failed to read line {}: {}", index + 1, e)),
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => serde_json::from_str(&line)
                .map_err(|e| format!("Invalid JSON on line {}: {}", index + 1, e)),
        };
        let failed = item.is_err();
        if sender.send(item).is_err() || failed {
            break;
        }
    }
}

/// Send each top-level value of a file of whitespace-separated JSON values.
fn read_sequence(reader: BufReader<File>, sender: &SyncSender<Item>) {
    let values = serde_json::Deserializer::from_reader(reader).into_iter::<serde_json::Value>();
    for value in values {
        let item = value.map_err(|e| format!("Invalid JSON: {}", e));
        let failed = item.is_err();
        if sender.send(item).is_err() || failed {
            break;
        }
    }
}

/// The first byte that isn't whitespace (or a UTF-8 byte order mark), to tell a JSON array
/// from a sequence of objects.
fn first_significant_byte(path: &Path) -> Result<Option<u8>> {
    use std::io::Read;
    let mut buffer = [0u8; 4096];
    let mut file = File::open(path)?;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(None);
        }
        let found = buffer[..n]
            .iter()
            .copied()
            .find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0xEF | 0xBB | 0xBF));
        if found.is_some() {
            return Ok(found);
        }
    }
}

/// Lines in a JSON Lines file, for the progress bar (blank lines are counted too, so this
/// can be slightly high).
fn count_lines(mut file: File) -> Result<usize> {
    use std::io::Read;
    let mut buffer = vec![0u8; 1 << 20];
    let (mut lines, mut last) = (0usize, b'\n');
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        lines += memchr::memchr_iter(b'\n', &buffer[..n]).count();
        last = buffer[n - 1];
    }
    Ok(lines + usize::from(last != b'\n'))
}

/// Elements of the top-level array (counted by commas outside strings and nested values), or
/// with `sequence`, the top-level objects and arrays of a sequence of values.
fn count_array_elements(mut file: File, sequence: bool) -> Result<usize> {
    use std::io::Read;
    let mut buffer = vec![0u8; 1 << 20];
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    let (mut commas, mut any_element, mut top_level) = (0usize, false, 0usize);
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];
        let mut i = 0;
        while i < n {
            if in_string {
                if escaped {
                    escaped = false;
                    i += 1;
                    continue;
                }
                // Jump to the next quote or backslash; strings are most of the bytes
                match memchr::memchr2(b'"', b'\\', &chunk[i..]) {
                    None => i = n,
                    Some(offset) => {
                        i += offset;
                        if chunk[i] == b'"' {
                            in_string = false;
                        } else {
                            escaped = true;
                        }
                        i += 1;
                    }
                }
                continue;
            }
            let b = chunk[i];
            i += 1;
            match b {
                b'"' => {
                    in_string = true;
                    any_element |= depth == 1;
                }
                b'[' | b'{' => {
                    any_element |= depth == 1;
                    top_level += usize::from(depth == 0);
                    depth += 1;
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 1 => commas += 1,
                b' ' | b'\t' | b'\r' | b'\n' => {}
                _ => any_element |= depth == 1,
            }
        }
    }
    Ok(if sequence {
        top_level
    } else if any_element {
        commas + 1
    } else {
        0
    })
}

fn json_value_to_value(json_val: &serde_json::Value) -> Value {
    match json_val {
        serde_json::Value::String(s) => {
            // Try to parse string as date first, fall back to string
            DateParser::try_parse(s).unwrap_or_else(|| Value::String(s.clone()))
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                match Decimal::try_from(f) {
                    Ok(d) => Value::Decimal(d),
                    Err(_) => Value::String(n.to_string()),
                }
            } else {
                Value::String(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Null => Value::Null,
        // Nested objects and arrays stay JSON (a JSON/JSONB column, nested in JSON output)
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Value::Json(json_val.clone()),
    }
}

/// A row for each JSON object; other values (numbers, arrays at the top level) are skipped.
fn object_to_row(value: &serde_json::Value) -> Option<Row> {
    value.as_object().map(|object| {
        object
            .iter()
            .map(|(key, value)| (key.clone(), json_value_to_value(value)))
            .collect()
    })
}

pub struct JsonSource {
    file_path: PathBuf,
    format: JsonFormat,
    values: Option<JsonValues>,
    /// Whether rows have been read since the file was opened
    consumed: bool,
}

impl JsonSource {
    pub fn new(file_path: &str) -> Result<Self> {
        let file_path = PathBuf::from(file_path);
        Ok(Self {
            format: JsonFormat::for_path(&file_path),
            file_path,
            values: None,
            consumed: false,
        })
    }

    fn open(&mut self) -> Result<()> {
        if !self.file_path.exists() {
            return Err(TinyEtlError::Connection(format!(
                "JSON file not found: {}",
                self.file_path.display()
            )));
        }
        // A .json file of objects rather than an array: JSON Lines, or objects back to back
        if self.format != JsonFormat::Lines {
            self.format = match first_significant_byte(&self.file_path)? {
                Some(b'{') => JsonFormat::Sequence,
                _ => JsonFormat::Array,
            };
        }
        let mut values = JsonValues::open(&self.file_path, self.format)?;
        // Report a file that isn't a JSON array (or valid JSON Lines) here, not mid-transfer
        if let Some(Err(message)) = &values.peeked {
            let message = message.clone();
            values.peeked = None;
            return Err(TinyEtlError::Configuration(message));
        }
        self.values = Some(values);
        self.consumed = false;
        Ok(())
    }
}

#[async_trait]
impl Source for JsonSource {
    async fn connect(&mut self) -> Result<()> {
        self.open()
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        // Sample from a separate pass so the main reader still starts at the first record
        let mut sample = JsonValues::open(&self.file_path, self.format)?;
        let mut rows = Vec::new();
        let mut key_order: Vec<String> = Vec::new();
        while rows.len() < sample_size {
            let Some(value) = sample.next_value()? else {
                break;
            };
            if let Some(object) = value.as_object() {
                for key in object.keys() {
                    if !key_order.contains(key) {
                        key_order.push(key.clone());
                    }
                }
            }
            if let Some(row) = object_to_row(&value) {
                rows.push(row);
            }
        }
        let mut schema = SchemaInferer::infer_from_rows_ordered(&rows, &key_order)?;
        schema.estimated_rows = None;
        Ok(schema)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let Some(values) = self.values.as_mut() else {
            return Ok(Vec::new());
        };
        let mut rows = Vec::with_capacity(batch_size.min(READ_AHEAD));
        while rows.len() < batch_size {
            let Some(value) = values.next_value()? else {
                break;
            };
            if let Some(row) = object_to_row(&value) {
                rows.push(row);
            }
        }
        self.consumed |= !rows.is_empty();
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        // A quick scan of the bytes, without parsing values, for the progress bar
        let file = File::open(&self.file_path)?;
        Ok(Some(match self.format {
            JsonFormat::Lines => count_lines(file)?,
            JsonFormat::Array => count_array_elements(file, false)?,
            JsonFormat::Sequence => count_array_elements(file, true)?,
        }))
    }

    async fn reset(&mut self) -> Result<()> {
        if self.consumed || self.values.is_none() {
            self.open()?;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.values.as_ref().is_some_and(JsonValues::has_more)
    }
}

/// Writes JSON as rows arrive: a pretty-printed array for `.json`, one compact object per line
/// for `.jsonl` / `.ndjson`. Keys follow the column order.
pub struct JsonTarget {
    file_path: PathBuf,
    format: JsonFormat,
    schema: Option<Schema>,
    writer: Option<BufWriter<File>>,
    output: Option<OutputFile>,
    written: usize,
    /// truncate() was called: replace the file instead of appending to it
    replace: bool,
}

impl JsonTarget {
    pub fn new(file_path: &str) -> Result<Self> {
        let file_path = PathBuf::from(file_path);
        Ok(Self {
            format: JsonFormat::for_path(&file_path),
            file_path,
            schema: None,
            writer: None,
            output: None,
            written: 0,
            replace: false,
        })
    }

    fn value_to_json(&self, value: &Value) -> serde_json::Value {
        match value {
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Integer(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
            Value::Decimal(d) => {
                // Convert Decimal to f64 for JSON representation
                match (*d).try_into() {
                    Ok(f) => {
                        if let Some(n) = serde_json::Number::from_f64(f) {
                            serde_json::Value::Number(n)
                        } else {
                            serde_json::Value::String(d.to_string())
                        }
                    }
                    Err(_) => serde_json::Value::String(d.to_string()),
                }
            }
            Value::Boolean(b) => serde_json::Value::Bool(*b),
            Value::Date(dt) => serde_json::Value::String(dt.to_rfc3339()),
            Value::Json(j) => j.clone(),
            Value::Null => serde_json::Value::Null,
        }
    }

    fn row_to_json(&self, row: &Row) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        match &self.schema {
            Some(schema) => {
                for column in &schema.columns {
                    let value = row
                        .get(&column.name)
                        .map(|v| self.value_to_json(v))
                        .unwrap_or(serde_json::Value::Null);
                    object.insert(column.name.clone(), value);
                }
            }
            None => {
                for (key, value) in row {
                    object.insert(key.clone(), self.value_to_json(value));
                }
            }
        }
        serde_json::Value::Object(object)
    }

    /// Write one record in the file's format.
    fn write_value(&mut self, value: &serde_json::Value) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?;
        match self.format {
            JsonFormat::Lines => {
                serde_json::to_writer(&mut *writer, value)?;
                writer.write_all(b"\n")?;
            }
            JsonFormat::Array | JsonFormat::Sequence => {
                // Same layout as serde_json::to_string_pretty for the whole array
                writer.write_all(if self.written == 0 {
                    b"[\n  "
                } else {
                    b",\n  "
                })?;
                let text = serde_json::to_string_pretty(value)?;
                writer.write_all(text.replace('\n', "\n  ").as_bytes())?;
            }
        }
        self.written += 1;
        Ok(())
    }
}

#[async_trait]
impl Target for JsonTarget {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn create_table(&mut self, _table_name: &str, schema: &Schema) -> Result<()> {
        self.schema = Some(schema.clone());
        let (output, file) = OutputFile::create(&self.file_path)?;
        self.writer = Some(BufWriter::new(file));
        self.output = Some(output);
        self.written = 0;

        // Appending: copy the existing records into the new file first, one at a time.
        // An empty file has no records.
        let has_content = std::fs::metadata(&self.file_path).is_ok_and(|m| m.len() > 0);
        if has_content && !self.replace {
            let mut existing = JsonValues::open(&self.file_path, self.format)?;
            while let Some(value) = existing.next_value().map_err(|e| {
                TinyEtlError::DataTransfer(format!(
                    "Can't append to {}: {} (use --truncate to replace it)",
                    self.file_path.display(),
                    e
                ))
            })? {
                self.write_value(&value)?;
            }
        }
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        for row in rows {
            let value = self.row_to_json(row);
            self.write_value(&value)?;
        }
        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        let (Some(mut writer), Some(output)) = (self.writer.take(), self.output.take()) else {
            return Ok(());
        };
        if self.format == JsonFormat::Array {
            writer.write_all(if self.written == 0 { b"[]" } else { b"\n]" })?;
        }
        writer.flush()?;
        drop(writer);
        output.commit()
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.file_path.exists())
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // The file is replaced when the transfer finishes
        self.replace = true;
        Ok(())
    }

    fn supports_append(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn two_column_schema() -> Schema {
        use crate::schema::{Column, DataType};
        Schema {
            columns: ["zeta", "alpha"]
                .iter()
                .map(|name| Column {
                    name: name.to_string(),
                    data_type: DataType::Integer,
                    nullable: true,
                })
                .collect(),
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }

    fn rows(range: std::ops::Range<i64>) -> Vec<Row> {
        range
            .map(|i| {
                Row::from([
                    ("zeta".to_string(), Value::Integer(i)),
                    ("alpha".to_string(), Value::Integer(-i)),
                ])
            })
            .collect()
    }

    async fn write(path: &Path, rows: &[Row], truncate: bool) {
        let mut target = JsonTarget::new(path.to_str().unwrap()).unwrap();
        target.connect().await.unwrap();
        if truncate {
            target.truncate("t").await.unwrap();
        }
        target
            .create_table("t", &two_column_schema())
            .await
            .unwrap();
        for chunk in rows.chunks(1000) {
            target.write_batch(chunk).await.unwrap();
        }
        target.finalize().await.unwrap();
    }

    async fn read(path: &Path, batch_size: usize) -> (Schema, Vec<Row>) {
        let mut source = JsonSource::new(path.to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(100).await.unwrap();
        source.reset().await.unwrap();
        let mut all = Vec::new();
        while source.has_more() {
            let batch = source.read_batch(batch_size).await.unwrap();
            assert!(batch.len() <= batch_size);
            all.extend(batch);
        }
        (schema, all)
    }

    #[tokio::test]
    async fn test_array_and_lines_stream_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["data.json", "data.jsonl", "data.ndjson"] {
            let path = dir.path().join(name);
            write(&path, &rows(0..10_000), false).await;
            let (schema, read_rows) = read(&path, 3_000).await;
            assert_eq!(read_rows.len(), 10_000, "{}", name);
            assert_eq!(read_rows[9_999]["zeta"], Value::Integer(9_999));
            // Columns keep the file's key order, which is the column order it was written in
            let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["zeta", "alpha"], "{}", name);
        }
    }

    #[tokio::test]
    async fn test_row_counts_for_progress() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            ("a.json", "[]", 0),
            ("b.json", "[\n  {\"a\": 1}\n]", 1),
            (
                "c.json",
                r#"[{"a": "x,]{", "b": [1, 2, {"c": 3}]}, {"a": "q\"uote,"}, 5]"#,
                3,
            ),
            ("d.jsonl", "{\"a\":1}\n{\"a\":2}", 2),
            ("e.jsonl", "{\"a\":1}\n{\"a\":2}\n", 2),
        ];
        for (name, text, expected) in cases {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            let source = JsonSource::new(path.to_str().unwrap()).unwrap();
            assert_eq!(
                source.estimated_row_count().await.unwrap(),
                Some(expected),
                "{}",
                name
            );
        }
    }

    #[tokio::test]
    async fn test_array_layout_matches_pretty_printing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pretty.json");
        write(&path, &rows(1..3), false).await;
        let expected = serde_json::to_string_pretty(&serde_json::json!([
            {"zeta": 1, "alpha": -1},
            {"zeta": 2, "alpha": -2}
        ]))
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);

        write(&path, &[], true).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    }

    #[tokio::test]
    async fn test_lines_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.jsonl");
        write(&path, &rows(1..3), false).await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"zeta\":1,\"alpha\":-1}\n{\"zeta\":2,\"alpha\":-2}\n"
        );
    }

    #[tokio::test]
    async fn test_append_and_truncate() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["out.json", "out.jsonl"] {
            let path = dir.path().join(name);
            write(&path, &rows(0..3), false).await;
            write(&path, &rows(3..5), false).await; // append
            assert_eq!(read(&path, 100).await.1.len(), 5, "{}", name);
            write(&path, &rows(0..2), true).await; // --truncate
            assert_eq!(read(&path, 100).await.1.len(), 2, "{}", name);
        }
    }

    #[tokio::test]
    async fn test_unfinished_write_keeps_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep.json");
        std::fs::write(&path, "[]").unwrap();
        let mut target = JsonTarget::new(path.to_str().unwrap()).unwrap();
        target
            .create_table("t", &two_column_schema())
            .await
            .unwrap();
        target.write_batch(&rows(0..5)).await.unwrap();
        drop(target);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn test_bad_input_errors() {
        let dir = tempfile::tempdir().unwrap();
        let scalar = dir.path().join("scalar.json");
        std::fs::write(&scalar, "42").unwrap();
        let err = JsonSource::new(scalar.to_str().unwrap())
            .unwrap()
            .connect()
            .await
            .unwrap_err();
        assert!(err.to_string().contains("array of objects"), "{}", err);

        // A sequence of objects that breaks partway reports where
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{\"a\": 1}\n{\"a\": 2\n").unwrap();
        let mut source = JsonSource::new(broken.to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        let err = source.read_batch(10).await.unwrap_err();
        assert!(err.to_string().contains("Invalid JSON"), "{}", err);

        let lines = dir.path().join("bad.jsonl");
        std::fs::write(&lines, "{\"a\": 1}\n\n{oops}\n").unwrap();
        let mut source = JsonSource::new(lines.to_str().unwrap()).unwrap();
        source.connect().await.unwrap();
        let err = source.read_batch(10).await.unwrap_err();
        assert!(err.to_string().contains("line 3"), "{}", err);
    }

    #[tokio::test]
    async fn test_json_source_connection() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice", "age": 25}},
            {{"id": 2, "name": "Bob", "age": 30}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_ok());
        assert!(source.has_more());
    }

    #[tokio::test]
    async fn test_json_source_schema_inference() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice", "active": true}},
            {{"id": 2, "name": "Bob", "active": false}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        source.connect().await.unwrap();

        let schema = source.infer_schema(10).await.unwrap();
        assert_eq!(schema.columns.len(), 3);

        let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, crate::schema::DataType::Integer);
    }

    #[tokio::test]
    async fn test_json_source_read_batch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"[
            {{"id": 1, "name": "Alice"}},
            {{"id": 2, "name": "Bob"}},
            {{"id": 3, "name": "Charlie"}}
        ]"#
        )
        .unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
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
    async fn test_json_invalid_format() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#""not an array""#).unwrap();

        let mut source = JsonSource::new(temp_file.path().to_str().unwrap()).unwrap();
        let result = source.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_json_target_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        let result = target.connect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_json_target_write_with_schema() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(temp_file.path().to_str().unwrap()).unwrap();

        // Create a schema
        let schema = Schema {
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
                crate::schema::Column {
                    name: "active".to_string(),
                    data_type: crate::schema::DataType::Boolean,
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        target.create_table("test", &schema).await.unwrap();

        // Create test data
        let mut row1 = std::collections::HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));
        row1.insert("active".to_string(), Value::Boolean(true));

        let mut row2 = std::collections::HashMap::new();
        row2.insert("id".to_string(), Value::Integer(2));
        row2.insert("name".to_string(), Value::String("Bob".to_string()));
        row2.insert("active".to_string(), Value::Boolean(false));

        let written = target.write_batch(&[row1, row2]).await.unwrap();
        assert_eq!(written, 2);

        target.finalize().await.unwrap();

        // Verify the output
        let output_content = std::fs::read_to_string(temp_file.path()).unwrap();
        let parsed_json: serde_json::Value = serde_json::from_str(&output_content).unwrap();

        assert!(parsed_json.is_array());
        let array = parsed_json.as_array().unwrap();
        assert_eq!(array.len(), 2);

        // Verify first object
        let first_obj = &array[0];
        assert_eq!(first_obj["id"], 1);
        assert_eq!(first_obj["name"], "Alice");
        assert_eq!(first_obj["active"], true);
    }

    #[tokio::test]
    async fn test_json_target_value_conversion() {
        let target = JsonTarget::new("/tmp/test.json").unwrap();

        assert_eq!(
            target.value_to_json(&Value::String("test".to_string())),
            serde_json::Value::String("test".to_string())
        );
        assert_eq!(
            target.value_to_json(&Value::Integer(42)),
            serde_json::Value::Number(serde_json::Number::from(42))
        );
        assert_eq!(
            target.value_to_json(&Value::Boolean(true)),
            serde_json::Value::Bool(true)
        );
        assert_eq!(target.value_to_json(&Value::Null), serde_json::Value::Null);

        // Test decimal conversion
        let decimal_val = target.value_to_json(&Value::Decimal(Decimal::new(314, 2)));
        assert!(decimal_val.is_number());
    }

    #[tokio::test]
    async fn test_json_roundtrip() {
        // Create source JSON
        let source_file = NamedTempFile::new().unwrap();
        let source_path = source_file.path().to_str().unwrap();
        std::fs::write(
            source_path,
            r#"[
            {"id": 1, "name": "Alice", "active": true},
            {"id": 2, "name": "Bob", "active": false}
        ]"#,
        )
        .unwrap();

        // Read with JsonSource
        let mut source = JsonSource::new(source_path).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(10).await.unwrap();
        source.reset().await.unwrap();
        let rows = source.read_batch(100).await.unwrap();

        // Write with JsonTarget
        let target_file = NamedTempFile::new().unwrap();
        let mut target = JsonTarget::new(target_file.path().to_str().unwrap()).unwrap();
        target.create_table("test", &schema).await.unwrap();
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // Verify output
        let output_content = std::fs::read_to_string(target_file.path()).unwrap();
        let parsed_json: serde_json::Value = serde_json::from_str(&output_content).unwrap();

        assert!(parsed_json.is_array());
        let array = parsed_json.as_array().unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array[0]["name"], "Alice");
        assert_eq!(array[1]["name"], "Bob");
    }

    #[tokio::test]
    async fn test_json_file_of_objects() {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            // JSON Lines saved as .json (gharchive, the CrateDB datasets)
            ("lines.json", "{\"id\": 1}\n{\"id\": 2}\n\n{\"id\": 3}\n"),
            // Pretty-printed objects back to back
            (
                "pretty.json",
                "{\n  \"id\": 1\n}\n{\n  \"id\": 2\n}{\"id\": 3}",
            ),
            // A byte order mark and leading whitespace
            (
                "bom.json",
                "\u{feff}\n  {\"id\": 1}\n{\"id\": 2}\n{\"id\": 3}",
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            let (schema, rows) = read(&path, 2).await;
            assert_eq!(schema.columns[0].name, "id", "{}", name);
            assert_eq!(rows.len(), 3, "{}", name);
            assert_eq!(rows[2]["id"], Value::Integer(3), "{}", name);
            let source = JsonSource::new(path.to_str().unwrap()).unwrap();
            let mut source = source;
            source.connect().await.unwrap();
            assert_eq!(
                source.estimated_row_count().await.unwrap(),
                Some(3),
                "{}",
                name
            );
        }

        // A single object is one row
        let single = dir.path().join("single.json");
        std::fs::write(&single, "{\"a\": 1, \"b\": \"x\"}").unwrap();
        let (_, rows) = read(&single, 10).await;
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn test_nested_values_stay_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested.jsonl");
        std::fs::write(
            &path,
            "{\"id\": 1, \"actor\": {\"login\": \"a\"}, \"tags\": [1, 2]}\n{\"id\": 2, \"actor\": {\"login\": \"b\"}, \"tags\": []}\n",
        )
        .unwrap();
        let (schema, rows) = read(&path, 10).await;
        let types: Vec<_> = schema.columns.iter().map(|c| c.data_type.clone()).collect();
        use crate::schema::DataType;
        assert_eq!(types, [DataType::Integer, DataType::Json, DataType::Json]);
        assert_eq!(
            rows[0]["actor"],
            Value::Json(serde_json::json!({"login": "a"}))
        );

        // Written back out, they're nested, not quoted
        let out = dir.path().join("out.jsonl");
        let mut target = JsonTarget::new(out.to_str().unwrap()).unwrap();
        target.connect().await.unwrap();
        target.create_table("t", &schema).await.unwrap();
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&out)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
            r#"{"id":1,"actor":{"login":"a"},"tags":[1,2]}"#
        );
    }
}
