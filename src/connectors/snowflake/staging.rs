//! Zstandard-compressed CSV files staged for `COPY INTO`. zstd makes files several times
//! smaller than fast gzip at the same speed, and uploading them is usually the slow part.
//!
//! Every non-NULL value is written in double quotes, and NULL as an empty unquoted field. With
//! `FIELD_OPTIONALLY_ENCLOSED_BY = '"'` and `EMPTY_FIELD_AS_NULL = TRUE`, Snowflake loads `""` as
//! an empty string and a bare empty field as NULL, so the two never collide (unlike a `\N`
//! sentinel, which real data could contain).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tempfile::TempDir;

use crate::{schema::Value, Result, TinyEtlError};

/// The `FILE_FORMAT` that matches what [`ChunkWriter`] writes.
pub(super) const FILE_FORMAT: &str = "TYPE = CSV FIELD_DELIMITER = ',' \
    FIELD_OPTIONALLY_ENCLOSED_BY = '\"' ESCAPE_UNENCLOSED_FIELD = NONE \
    EMPTY_FIELD_AS_NULL = TRUE NULL_IF = () COMPRESSION = ZSTD ENCODING = 'UTF8'";

/// zstd level for staged files: fast, and already much smaller than gzip.
const ZSTD_LEVEL: i32 = 3;

/// Text for a value in a staged file or a bind parameter, or `None` for NULL. Timestamps are
/// UTC (the target columns are `TIMESTAMP_NTZ`); decimals keep their exact digits.
pub(super) fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Decimal(d) => Some(d.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Date(dt) => Some(dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()),
        Value::Json(j) => Some(j.to_string()),
    }
}

/// Append one CSV line for `fields` to `out`.
fn csv_line<'a>(out: &mut Vec<u8>, fields: impl Iterator<Item = Option<&'a str>>) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(b',');
        }
        if let Some(text) = field {
            out.push(b'"');
            out.extend_from_slice(text.replace('"', "\"\"").as_bytes());
            out.push(b'"');
        }
    }
    out.push(b'\n');
}

/// A finished, closed staged file.
#[derive(Debug)]
pub(super) struct StagedFile {
    pub path: PathBuf,
    pub rows: usize,
}

struct OpenFile {
    encoder: zstd::stream::write::Encoder<'static, BufWriter<File>>,
    path: PathBuf,
    rows: usize,
}

/// Writes rows into numbered `part-NNNNN.csv.zst` files of at most `max_rows` rows each.
pub(super) struct ChunkWriter {
    dir: TempDir,
    max_rows: usize,
    next_index: usize,
    current: Option<OpenFile>,
    line: Vec<u8>,
}

impl ChunkWriter {
    pub fn new(max_rows: usize) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("tinyetl_snowflake_")
            .tempdir()?;
        Ok(Self {
            dir,
            max_rows: max_rows.max(1),
            next_index: 0,
            current: None,
            line: Vec::new(),
        })
    }

    /// Add a row. Returns the file it completed, if this row filled one.
    pub fn write_row<'a>(
        &mut self,
        fields: impl Iterator<Item = Option<&'a str>>,
    ) -> Result<Option<StagedFile>> {
        if self.current.is_none() {
            self.next_index += 1;
            let path = self
                .dir
                .path()
                .join(format!("part-{:05}.csv.zst", self.next_index));
            let file = BufWriter::new(File::create(&path)?);
            self.current = Some(OpenFile {
                encoder: zstd::stream::write::Encoder::new(file, ZSTD_LEVEL)?,
                path,
                rows: 0,
            });
        }
        let current = self.current.as_mut().expect("opened above");
        self.line.clear();
        csv_line(&mut self.line, fields);
        current.encoder.write_all(&self.line)?;
        current.rows += 1;
        if current.rows >= self.max_rows {
            return self.finish();
        }
        Ok(None)
    }

    /// Close the file being written, if any, and return it.
    pub fn finish(&mut self) -> Result<Option<StagedFile>> {
        let Some(open) = self.current.take() else {
            return Ok(None);
        };
        let mut writer = open.encoder.finish()?;
        writer.flush().map_err(TinyEtlError::Io)?;
        Ok(Some(StagedFile {
            path: open.path,
            rows: open.rows,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn read_zst(path: &std::path::Path) -> String {
        let mut text = String::new();
        zstd::stream::read::Decoder::new(File::open(path).unwrap())
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn test_csv_fields() {
        let mut out = Vec::new();
        csv_line(
            &mut out,
            [
                Some("plain"),
                None,
                Some(""),
                Some("say \"hi\", ok\nnext"),
                Some("日本語 🚀"),
            ]
            .into_iter(),
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"plain\",,\"\",\"say \"\"hi\"\", ok\nnext\",\"日本語 🚀\"\n"
        );
    }

    #[test]
    fn test_value_text() {
        use chrono::{DateTime, Utc};
        use std::str::FromStr;
        assert_eq!(value_text(&Value::Null), None);
        assert_eq!(value_text(&Value::Boolean(true)).as_deref(), Some("true"));
        assert_eq!(
            value_text(&Value::Decimal(
                rust_decimal::Decimal::from_str("1234567.125").unwrap()
            ))
            .as_deref(),
            Some("1234567.125")
        );
        let dt: DateTime<Utc> = DateTime::parse_from_rfc3339("2024-01-15T10:30:00.5Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            value_text(&Value::Date(dt)).as_deref(),
            Some("2024-01-15 10:30:00.500")
        );
        assert_eq!(
            value_text(&Value::Json(serde_json::json!({"a": [1, null]}))).as_deref(),
            Some(r#"{"a":[1,null]}"#)
        );
    }

    #[test]
    fn test_chunk_rotation() {
        let mut writer = ChunkWriter::new(2).unwrap();
        assert!(writer.write_row([Some("1")].into_iter()).unwrap().is_none());
        let first = writer.write_row([Some("2")].into_iter()).unwrap().unwrap();
        assert_eq!(first.rows, 2);
        assert_eq!(read_zst(&first.path), "\"1\"\n\"2\"\n");
        assert!(writer.write_row([None].into_iter()).unwrap().is_none());
        let last = writer.finish().unwrap().unwrap();
        assert_eq!(last.rows, 1);
        assert_eq!(read_zst(&last.path), "\n");
        assert!(last.path.ends_with("part-00002.csv.zst"));
        assert!(writer.finish().unwrap().is_none());
    }
}
