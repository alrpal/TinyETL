//! Gzip and zstd compressed files (`data.csv.gz`, `events.jsonl.zst`, `t.parquet.gz`, ...).
//!
//! A compressed source is decompressed, streaming, into a temporary file named after the inner
//! format, which that format's connector then reads; a compressed target writes the inner format
//! to a temporary file and compresses it into place in `finalize`. Every format works this way,
//! and the inner connector keeps its own streaming, row counts and atomic output.

use async_trait::async_trait;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::connectors::output_file::OutputFile;
use crate::connectors::{create_source_with_options, create_target_with_options, Source, Target};
use crate::schema::{Row, Schema};
use crate::{Result, TinyEtlError};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Codec {
    Gzip,
    Zstd,
}

impl Codec {
    /// The codec named by a path's last extension, and the path without it
    pub fn from_path(path: &str) -> Option<(Codec, &str)> {
        let lower = path.to_ascii_lowercase();
        for (suffix, codec) in [(".gz", Codec::Gzip), (".zst", Codec::Zstd)] {
            if lower.ends_with(suffix) && path.len() > suffix.len() {
                return Some((codec, &path[..path.len() - suffix.len()]));
            }
        }
        None
    }

    fn name(self) -> &'static str {
        match self {
            Codec::Gzip => "gzip",
            Codec::Zstd => "zstd",
        }
    }

    fn magic(self) -> &'static [u8] {
        match self {
            Codec::Gzip => &[0x1f, 0x8b],
            Codec::Zstd => &[0x28, 0xb5, 0x2f, 0xfd],
        }
    }

    /// Decompress `from` into `to`. A file that isn't actually compressed (say, an HTTP server
    /// already decoded it) is copied as is.
    fn decompress(self, from: &Path, to: &Path) -> std::io::Result<u64> {
        let mut input = BufReader::new(File::open(from)?);
        let mut start = [0u8; 4];
        let n = read_up_to(&mut input, &mut start)?;
        let header = &start[..n];
        let mut input = header.chain(input);
        let mut output = BufWriter::new(File::create(to)?);
        let written = if header.starts_with(self.magic()) {
            match self {
                // MultiGzDecoder also reads files made of several gzip members (`cat a.gz b.gz`)
                Codec::Gzip => {
                    std::io::copy(&mut flate2::read::MultiGzDecoder::new(input), &mut output)?
                }
                Codec::Zstd => std::io::copy(&mut zstd::Decoder::new(input)?, &mut output)?,
            }
        } else {
            std::io::copy(&mut input, &mut output)?
        };
        output.flush()?;
        Ok(written)
    }

    fn compress(self, from: &Path, to: File) -> std::io::Result<()> {
        let mut input = BufReader::new(File::open(from)?);
        let output = BufWriter::new(to);
        match self {
            Codec::Gzip => {
                let mut encoder =
                    flate2::write::GzEncoder::new(output, flate2::Compression::default());
                std::io::copy(&mut input, &mut encoder)?;
                encoder.finish()?.flush()
            }
            Codec::Zstd => {
                let mut encoder = zstd::Encoder::new(output, 3)?;
                std::io::copy(&mut input, &mut encoder)?;
                encoder.finish()?.flush()
            }
        }
    }
}

fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// The inner file name, e.g. `data.csv` for `/in/data.csv.gz`
fn inner_name(inner_path: &str) -> String {
    Path::new(inner_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".to_string())
}

fn staging_dir() -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix("tinyetl_decompress_")
        .tempdir()
        .map_err(TinyEtlError::Io)
}

fn io_error(context: String, e: std::io::Error) -> TinyEtlError {
    TinyEtlError::Io(std::io::Error::new(e.kind(), format!("{}: {}", context, e)))
}

/// Check that the inner format is a file format, so `x.gz` fails when it's created rather than
/// after decompressing
fn check_inner(path: &str, inner: &str) -> Result<()> {
    let inner = inner.to_ascii_lowercase();
    let known = crate::connectors::csv::is_csv(&inner)
        || inner.ends_with(".json")
        || crate::connectors::json::is_json_lines(&inner)
        || inner.ends_with(".parquet")
        || inner.ends_with(".avro")
        || crate::connectors::excel::is_spreadsheet(&inner);
    if known {
        Ok(())
    } else {
        Err(TinyEtlError::Configuration(format!(
            "Can't tell the format inside {}: name it like data.csv.gz, events.jsonl.zst or table.parquet.gz",
            path
        )))
    }
}

pub struct CompressedSource {
    path: PathBuf,
    codec: Codec,
    inner_name: String,
    options: HashMap<String, String>,
    inner: Option<Box<dyn Source>>,
    staging: Option<TempDir>,
}

impl CompressedSource {
    pub(crate) fn new(
        path: &str,
        codec: Codec,
        inner_path: &str,
        options: &HashMap<String, String>,
    ) -> Result<Self> {
        check_inner(path, inner_path)?;
        Ok(Self {
            path: PathBuf::from(path),
            codec,
            inner_name: inner_name(inner_path),
            options: options.clone(),
            inner: None,
            staging: None,
        })
    }

    fn inner(&mut self) -> Result<&mut Box<dyn Source>> {
        self.inner
            .as_mut()
            .ok_or_else(|| TinyEtlError::Connection("Source not connected".to_string()))
    }
}

#[async_trait]
impl Source for CompressedSource {
    async fn connect(&mut self) -> Result<()> {
        if self.inner.is_none() {
            if !self.path.exists() {
                return Err(TinyEtlError::Connection(format!(
                    "File not found: {}",
                    self.path.display()
                )));
            }
            let staging = staging_dir()?;
            let staged = staging.path().join(&self.inner_name);
            let (codec, from) = (self.codec, self.path.clone());
            let to = staged.clone();
            let bytes = tokio::task::spawn_blocking(move || codec.decompress(&from, &to))
                .await
                .map_err(|e| TinyEtlError::DataTransfer(e.to_string()))?
                .map_err(|e| {
                    io_error(
                        format!(
                            "Failed to decompress {} ({})",
                            self.path.display(),
                            codec.name()
                        ),
                        e,
                    )
                })?;
            tracing::info!(
                "Decompressed {} ({}, {} bytes)",
                self.path.display(),
                self.codec.name(),
                bytes
            );
            self.inner = Some(create_source_with_options(
                &staged.to_string_lossy(),
                &self.options,
            )?);
            self.staging = Some(staging);
        }
        let inner = self.inner()?;
        inner.connect().await
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        let inner = self.inner()?;
        inner.infer_schema(sample_size).await
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        let inner = self.inner()?;
        inner.read_batch(batch_size).await
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        match &self.inner {
            Some(inner) => inner.estimated_row_count().await,
            None => Ok(None),
        }
    }

    async fn reset(&mut self) -> Result<()> {
        let inner = self.inner()?;
        inner.reset().await
    }

    fn has_more(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| inner.has_more())
    }
}

pub struct CompressedTarget {
    path: PathBuf,
    codec: Codec,
    inner: Box<dyn Target>,
    staged: PathBuf,
    _staging: TempDir,
}

impl CompressedTarget {
    pub(crate) fn new(
        path: &str,
        codec: Codec,
        inner_path: &str,
        options: &HashMap<String, String>,
    ) -> Result<Self> {
        check_inner(path, inner_path)?;
        let staging = staging_dir()?;
        let staged = staging.path().join(inner_name(inner_path));
        let inner = create_target_with_options(&staged.to_string_lossy(), options)?;
        Ok(Self {
            path: PathBuf::from(path),
            codec,
            inner,
            staged,
            _staging: staging,
        })
    }
}

#[async_trait]
impl Target for CompressedTarget {
    async fn connect(&mut self) -> Result<()> {
        self.inner.connect().await
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        self.inner.create_table(table_name, schema).await
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        self.inner.write_batch(rows).await
    }

    async fn finalize(&mut self) -> Result<()> {
        self.inner.finalize().await?;
        let (output, file) = OutputFile::create(&self.path)?;
        let (codec, staged) = (self.codec, self.staged.clone());
        tokio::task::spawn_blocking(move || codec.compress(&staged, file))
            .await
            .map_err(|e| TinyEtlError::DataTransfer(e.to_string()))?
            .map_err(|e| io_error(format!("Failed to compress {}", self.path.display()), e))?;
        output.commit()
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        Ok(self.path.exists())
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        // The file is replaced in finalize; this resets the staged copy
        self.inner.truncate(table_name).await
    }

    fn supports_append(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType, Value};

    fn schema() -> Schema {
        Schema {
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
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }

    fn rows(n: i64) -> Vec<Row> {
        (0..n)
            .map(|i| {
                Row::from([
                    ("id".to_string(), Value::Integer(i)),
                    ("name".to_string(), Value::String(format!("name {}", i))),
                ])
            })
            .collect()
    }

    async fn write(path: &Path, rows: &[Row]) {
        let mut target =
            create_target_with_options(&path.to_string_lossy(), &HashMap::new()).unwrap();
        target.connect().await.unwrap();
        target.create_table("t", &schema()).await.unwrap();
        target.write_batch(rows).await.unwrap();
        target.finalize().await.unwrap();
    }

    async fn read(path: &Path) -> Vec<Row> {
        let mut source =
            create_source_with_options(&path.to_string_lossy(), &HashMap::new()).unwrap();
        source.connect().await.unwrap();
        source.infer_schema(100).await.unwrap();
        assert!(source.estimated_row_count().await.unwrap().is_some());
        source.reset().await.unwrap();
        let mut all = Vec::new();
        while source.has_more() {
            let batch = source.read_batch(128).await.unwrap();
            if batch.is_empty() {
                break;
            }
            all.extend(batch);
        }
        all
    }

    #[test]
    fn test_codec_from_path() {
        assert_eq!(Codec::from_path("a.csv.gz"), Some((Codec::Gzip, "a.csv")));
        assert_eq!(
            Codec::from_path("a.JSONL.ZST"),
            Some((Codec::Zstd, "a.JSONL"))
        );
        assert_eq!(Codec::from_path("a.csv"), None);
        assert_eq!(Codec::from_path(".gz"), None);
    }

    #[tokio::test]
    async fn test_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "a.csv.gz",
            "a.tsv.gz",
            "a.jsonl.gz",
            "a.json.zst",
            "a.parquet.gz",
            "a.avro.zst",
        ] {
            let path = dir.path().join(name);
            write(&path, &rows(300)).await;
            let mut magic = [0u8; 2];
            File::open(&path).unwrap().read_exact(&mut magic).unwrap();
            assert!(
                magic == [0x1f, 0x8b] || magic == [0x28, 0xb5],
                "{} is compressed",
                name
            );
            let read = read(&path).await;
            assert_eq!(read.len(), 300, "{}", name);
            assert_eq!(
                read[299]["name"],
                Value::String("name 299".to_string()),
                "{}",
                name
            );
        }
        // Nothing but the outputs is left in the directory
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 6);
    }

    #[tokio::test]
    async fn test_concatenated_gzip_and_uncompressed_input() {
        let dir = tempfile::tempdir().unwrap();
        let gz = |text: &str| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(text.as_bytes()).unwrap();
            e.finish().unwrap()
        };
        // Two gzip members, as `cat a.gz b.gz` makes
        let path = dir.path().join("two.jsonl.gz");
        let mut bytes = gz("{\"id\":1}\n");
        bytes.extend(gz("{\"id\":2}\n"));
        std::fs::write(&path, bytes).unwrap();
        let mut source =
            create_source_with_options(&path.to_string_lossy(), &HashMap::new()).unwrap();
        source.connect().await.unwrap();
        source.infer_schema(10).await.unwrap();
        source.reset().await.unwrap();
        assert_eq!(source.read_batch(10).await.unwrap().len(), 2);

        // A .gz name holding plain text (e.g. already decoded by an HTTP server) still reads
        let plain = dir.path().join("plain.csv.gz");
        std::fs::write(&plain, "id\n1\n2\n3\n").unwrap();
        let mut source =
            create_source_with_options(&plain.to_string_lossy(), &HashMap::new()).unwrap();
        source.connect().await.unwrap();
        source.infer_schema(10).await.unwrap();
        source.reset().await.unwrap();
        assert_eq!(source.read_batch(10).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_errors() {
        let err = create_source_with_options("data.gz", &HashMap::new())
            .err()
            .unwrap();
        assert!(err
            .to_string()
            .contains("Can't tell the format inside data.gz"));
        assert!(create_target_with_options("out.xyz.gz", &HashMap::new()).is_err());

        let mut missing = create_source_with_options("missing.csv.gz", &HashMap::new()).unwrap();
        assert!(missing
            .connect()
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("File not found"));

        // A corrupt file fails with the file name
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.csv.gz");
        std::fs::write(&bad, [0x1f, 0x8b, 0x08, 0x00, 0x01, 0x02]).unwrap();
        let mut source =
            create_source_with_options(&bad.to_string_lossy(), &HashMap::new()).unwrap();
        let err = source.connect().await.err().unwrap().to_string();
        assert!(
            err.contains("Failed to decompress") && err.contains("bad.csv.gz"),
            "{}",
            err
        );

        // A failed transfer leaves no output
        let path = dir.path().join("out.csv.gz");
        let mut target =
            create_target_with_options(&path.to_string_lossy(), &HashMap::new()).unwrap();
        target.connect().await.unwrap();
        target.create_table("t", &schema()).await.unwrap();
        target.write_batch(&rows(5)).await.unwrap();
        drop(target);
        assert!(!path.exists());
    }
}
