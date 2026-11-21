use crate::{
    connectors::{self, Source, Target},
    protocols::{extension_from_path, Protocol},
    schema::{Row, Schema},
    Result, TinyEtlError,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use object_store::{
    aws::AmazonS3Builder, buffered::BufWriter, path::Path as ObjectPath, ObjectStore,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};
use url::Url;

/// File formats that can be staged through S3 (must match the file connectors).
const SUPPORTED_FORMATS: [&str; 4] = ["csv", "json", "parquet", "avro"];

/// S3 protocol for reading and writing single objects (`s3://bucket/path/to/file.ext`).
///
/// Objects are staged through a local temporary file: sources are downloaded and then read
/// by the matching file connector; targets are written locally by the file connector and
/// uploaded when the transfer is finalized.
///
/// Credentials and settings come from the standard `AWS_*` environment variables and can be
/// overridden per endpoint with options (`region`, `endpoint`, `access_key_id`,
/// `secret_access_key`, `session_token`, `allow_http`, `path_style`).
pub struct S3Protocol;

impl Default for S3Protocol {
    fn default() -> Self {
        Self::new()
    }
}

impl S3Protocol {
    pub fn new() -> Self {
        Self
    }

    /// Split an s3:// URL into (bucket, object key)
    fn parse_location(url: &Url) -> Result<(String, ObjectPath)> {
        let bucket = url
            .host_str()
            .filter(|b| !b.is_empty())
            .ok_or_else(|| {
                TinyEtlError::Configuration(format!(
                    "S3 URL must specify a bucket (s3://bucket/key): {}",
                    url
                ))
            })?
            .to_string();

        let raw_key = url.path().trim_start_matches('/');
        if raw_key.is_empty() || raw_key.ends_with('/') {
            return Err(TinyEtlError::Configuration(format!(
                "S3 URL must point to a single object (s3://bucket/path/file.ext): {}",
                url
            )));
        }

        let key = ObjectPath::from_url_path(raw_key).map_err(|e| {
            TinyEtlError::Configuration(format!("Invalid S3 object key in '{}': {}", url, e))
        })?;

        Ok((bucket, key))
    }

    /// Work out the file format from the object key, or from an explicit type override
    fn resolve_format(url: &Url, source_type: Option<&str>) -> Result<String> {
        let format = source_type
            .map(|t| t.to_lowercase())
            .or_else(|| extension_from_path(url.path()));

        match format {
            Some(f) if SUPPORTED_FORMATS.contains(&f.as_str()) => Ok(f),
            Some(f) => Err(TinyEtlError::Configuration(format!(
                "Unsupported file format '{}' for {}. Supported formats: {}",
                f,
                url,
                SUPPORTED_FORMATS.join(", ")
            ))),
            None => Err(TinyEtlError::Configuration(format!(
                "Cannot determine the file format of {}. The object key needs a file extension ({}), or use --source-type for sources.",
                url,
                SUPPORTED_FORMATS.join(", ")
            ))),
        }
    }

    fn parse_bool(key: &str, value: &str) -> Result<bool> {
        match value.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(TinyEtlError::Configuration(format!(
                "Invalid value '{}' for S3 option '{}': expected true or false",
                value, key
            ))),
        }
    }

    /// Build the S3 client from AWS_* environment variables plus per-endpoint options
    fn build_store(
        bucket: &str,
        options: &HashMap<String, String>,
    ) -> Result<Arc<dyn ObjectStore>> {
        let mut builder = AmazonS3Builder::from_env().with_bucket_name(bucket);
        let mut allow_http: Option<bool> = None;
        let mut http_endpoint = false;

        for (key, value) in options {
            builder = match key.as_str() {
                "region" => builder.with_region(value),
                "endpoint" => {
                    http_endpoint = value.starts_with("http://");
                    builder.with_endpoint(value)
                }
                "access_key_id" => builder.with_access_key_id(value),
                "secret_access_key" => builder.with_secret_access_key(value),
                "session_token" => builder.with_token(value),
                "allow_http" => {
                    allow_http = Some(Self::parse_bool(key, value)?);
                    builder
                }
                "path_style" => {
                    builder.with_virtual_hosted_style_request(!Self::parse_bool(key, value)?)
                }
                other => {
                    return Err(TinyEtlError::Configuration(format!(
                        "Unknown S3 option '{}'. Supported options: region, endpoint, access_key_id, secret_access_key, session_token, allow_http, path_style",
                        other
                    )))
                }
            };
        }

        // A plain http:// endpoint (e.g. a local S3-compatible server) only works with allow_http, so imply it
        // unless the user set it explicitly.
        if let Some(allow) = allow_http.or(http_endpoint.then_some(true)) {
            builder = builder.with_allow_http(allow);
        }

        let store = builder.build().map_err(|e| {
            TinyEtlError::Configuration(format!("Failed to configure S3 client: {}", e))
        })?;

        Ok(Arc::new(store))
    }
}

#[async_trait]
impl Protocol for S3Protocol {
    async fn create_source(
        &self,
        url: &Url,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>> {
        self.create_source_with_type(url, None, options).await
    }

    async fn create_source_with_type(
        &self,
        url: &Url,
        source_type: Option<&str>,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Source>> {
        let (bucket, key) = Self::parse_location(url)?;
        let format = Self::resolve_format(url, source_type)?;
        let store = Self::build_store(&bucket, options)?;
        let location = format!("s3://{}/{}", bucket, key);

        Ok(Box::new(
            S3Source::download(store, &key, &location, &format).await?,
        ))
    }

    async fn create_target(
        &self,
        url: &Url,
        options: &HashMap<String, String>,
    ) -> Result<Box<dyn Target>> {
        let (bucket, key) = Self::parse_location(url)?;
        let format = Self::resolve_format(url, None)?;
        let store = Self::build_store(&bucket, options)?;
        let location = format!("s3://{}/{}", bucket, key);

        Ok(Box::new(S3Target::new(store, key, location, &format)?))
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        if url.scheme() != "s3" {
            return Err(TinyEtlError::Configuration(format!(
                "S3 protocol requires s3:// scheme, got: {}",
                url.scheme()
            )));
        }

        Self::parse_location(url).map(|_| ())
    }

    fn name(&self) -> &'static str {
        "s3"
    }
}

/// Convert an object store error into a TinyETL error with the object location in the message
fn s3_error(action: &str, location: &str, error: object_store::Error) -> TinyEtlError {
    match error {
        object_store::Error::NotFound { .. } => {
            TinyEtlError::Connection(format!("S3 object not found: {}", location))
        }
        other => {
            let message = other.to_string();
            // With no credentials configured the client falls back to the EC2 instance
            // metadata service, which fails with an unhelpful error anywhere outside AWS.
            let hint = if message.contains("169.254.169.254") {
                " (no AWS credentials found: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, or the access_key_id / secret_access_key options)"
            } else {
                ""
            };
            TinyEtlError::Connection(format!(
                "Failed to {} {}: {}{}",
                action, location, message, hint
            ))
        }
    }
}

fn staging_dir() -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix("tinyetl_s3_")
        .tempdir()
        .map_err(TinyEtlError::Io)
}

/// Source that reads an S3 object through a locally staged copy.
/// The staged file is removed when the source is dropped.
pub struct S3Source {
    inner: Box<dyn Source>,
    _staging: TempDir,
}

impl S3Source {
    /// Download `key` from `store` into a temporary file and open it with the file connector
    /// for `format`. `location` is the human-readable location used in logs and errors.
    pub async fn download(
        store: Arc<dyn ObjectStore>,
        key: &ObjectPath,
        location: &str,
        format: &str,
    ) -> Result<Self> {
        let staging = staging_dir()?;
        let local_path = staging.path().join(format!("source.{}", format));

        info!("Downloading {}", location);

        let result = store
            .get(key)
            .await
            .map_err(|e| s3_error("download", location, e))?;

        let mut file = tokio::fs::File::create(&local_path).await?;
        let mut stream = result.into_stream();
        let mut total_bytes: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| s3_error("download", location, e))?;
            total_bytes += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);

        info!("Download completed, {} bytes received", total_bytes);

        let inner = connectors::create_source(&local_path.to_string_lossy())?;

        Ok(Self {
            inner,
            _staging: staging,
        })
    }
}

#[async_trait]
impl Source for S3Source {
    async fn connect(&mut self) -> Result<()> {
        self.inner.connect().await
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        self.inner.infer_schema(sample_size).await
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        self.inner.read_batch(batch_size).await
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        self.inner.estimated_row_count().await
    }

    async fn reset(&mut self) -> Result<()> {
        self.inner.reset().await
    }

    fn has_more(&self) -> bool {
        self.inner.has_more()
    }
}

/// Target that writes a local staging file with the file connector and uploads it to S3 when
/// the transfer is finalized. An existing object is replaced; nothing is uploaded if the
/// transfer fails before `finalize`.
pub struct S3Target {
    inner: Box<dyn Target>,
    store: Arc<dyn ObjectStore>,
    key: ObjectPath,
    location: String,
    local_path: PathBuf,
    _staging: TempDir,
}

impl S3Target {
    /// Create a target that uploads to `key` in `store`, written in `format`.
    /// `location` is the human-readable location used in logs and errors.
    pub fn new(
        store: Arc<dyn ObjectStore>,
        key: ObjectPath,
        location: String,
        format: &str,
    ) -> Result<Self> {
        let staging = staging_dir()?;
        let local_path = staging.path().join(format!("target.{}", format));
        let inner = connectors::create_target(&local_path.to_string_lossy())?;

        Ok(Self {
            inner,
            store,
            key,
            location,
            local_path,
            _staging: staging,
        })
    }

    async fn object_exists(&self) -> Result<bool> {
        match self.store.head(&self.key).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(s3_error("access", &self.location, e)),
        }
    }
}

#[async_trait]
impl Target for S3Target {
    async fn connect(&mut self) -> Result<()> {
        // Check the bucket and credentials up front so that problems surface before any data
        // is read, rather than at the final upload.
        self.object_exists().await?;
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

        if !self.local_path.exists() {
            warn!("No data was written, skipping upload to {}", self.location);
            return Ok(());
        }

        info!("Uploading to {}", self.location);

        let mut file = tokio::fs::File::open(&self.local_path).await?;
        let mut writer = BufWriter::new(Arc::clone(&self.store), self.key.clone());

        let upload = async {
            let bytes = tokio::io::copy(&mut file, &mut writer).await?;
            writer.shutdown().await?;
            Ok::<u64, std::io::Error>(bytes)
        };

        let bytes = upload.await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to upload {}: {}", self.location, e))
        })?;

        info!("Upload completed, {} bytes sent", bytes);
        Ok(())
    }

    async fn exists(&self, _table_name: &str) -> Result<bool> {
        self.object_exists().await
    }

    async fn truncate(&mut self, _table_name: &str) -> Result<()> {
        // The staging file always starts empty and the upload replaces the whole object,
        // so there is nothing to clear.
        Ok(())
    }

    fn supports_append(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType, Value};
    use object_store::memory::InMemory;

    fn memory_store() -> Arc<dyn ObjectStore> {
        Arc::new(InMemory::new())
    }

    fn sample_schema() -> Schema {
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
                    nullable: false,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        }
    }

    fn sample_rows() -> Vec<Row> {
        [(1, "Alice"), (2, "Bob")]
            .into_iter()
            .map(|(id, name)| {
                let mut row = Row::new();
                row.insert("id".to_string(), Value::Integer(id));
                row.insert("name".to_string(), Value::String(name.to_string()));
                row
            })
            .collect()
    }

    async fn write_rows(store: &Arc<dyn ObjectStore>, key: &str, format: &str) {
        let mut target = S3Target::new(
            Arc::clone(store),
            ObjectPath::from(key),
            format!("s3://bucket/{}", key),
            format,
        )
        .unwrap();

        target.connect().await.unwrap();
        target.create_table("data", &sample_schema()).await.unwrap();
        assert_eq!(target.write_batch(&sample_rows()).await.unwrap(), 2);
        target.finalize().await.unwrap();
    }

    async fn read_rows(store: &Arc<dyn ObjectStore>, key: &str, format: &str) -> Vec<Row> {
        let mut source = S3Source::download(
            Arc::clone(store),
            &ObjectPath::from(key),
            &format!("s3://bucket/{}", key),
            format,
        )
        .await
        .unwrap();

        source.connect().await.unwrap();
        source.infer_schema(100).await.unwrap();
        source.reset().await.unwrap();
        source.read_batch(100).await.unwrap()
    }

    #[test]
    fn test_validate_s3_url() {
        let protocol = S3Protocol::new();

        let url = Url::parse("s3://my-bucket/data.csv").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        let url = Url::parse("s3://my-bucket/nested/path/data.parquet").unwrap();
        assert!(protocol.validate_url(&url).is_ok());

        // Wrong scheme
        let url = Url::parse("https://my-bucket/data.csv").unwrap();
        assert!(protocol.validate_url(&url).is_err());

        // No object key
        let url = Url::parse("s3://my-bucket").unwrap();
        assert!(protocol.validate_url(&url).is_err());
        let url = Url::parse("s3://my-bucket/").unwrap();
        assert!(protocol.validate_url(&url).is_err());

        // Prefix rather than a single object
        let url = Url::parse("s3://my-bucket/prefix/").unwrap();
        assert!(protocol.validate_url(&url).is_err());
    }

    #[test]
    fn test_parse_location() {
        let url = Url::parse("s3://my-bucket/exports/2024/orders.parquet").unwrap();
        let (bucket, key) = S3Protocol::parse_location(&url).unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(key.as_ref(), "exports/2024/orders.parquet");

        // Percent-encoded characters are decoded into the object key
        let url = Url::parse("s3://my-bucket/my%20folder/orders.csv").unwrap();
        let (_, key) = S3Protocol::parse_location(&url).unwrap();
        assert_eq!(key.as_ref(), "my folder/orders.csv");
    }

    #[test]
    fn test_resolve_format() {
        let url = Url::parse("s3://bucket/data.CSV").unwrap();
        assert_eq!(S3Protocol::resolve_format(&url, None).unwrap(), "csv");

        // Explicit type wins over the extension
        let url = Url::parse("s3://bucket/export.txt").unwrap();
        assert_eq!(
            S3Protocol::resolve_format(&url, Some("JSON")).unwrap(),
            "json"
        );

        // Unsupported or missing extension
        assert!(S3Protocol::resolve_format(&url, None).is_err());
        let url = Url::parse("s3://bucket/export").unwrap();
        assert!(S3Protocol::resolve_format(&url, None).is_err());
    }

    #[test]
    fn test_build_store_options() {
        let mut options = HashMap::new();
        options.insert("region".to_string(), "eu-west-1".to_string());
        options.insert("endpoint".to_string(), "http://localhost:9000".to_string());
        options.insert("access_key_id".to_string(), "key".to_string());
        options.insert("secret_access_key".to_string(), "secret".to_string());
        options.insert("path_style".to_string(), "true".to_string());
        assert!(S3Protocol::build_store("bucket", &options).is_ok());

        let mut options = HashMap::new();
        options.insert("regoin".to_string(), "eu-west-1".to_string());
        let err = S3Protocol::build_store("bucket", &options)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("Unknown S3 option 'regoin'"));

        let mut options = HashMap::new();
        options.insert("allow_http".to_string(), "maybe".to_string());
        assert!(S3Protocol::build_store("bucket", &options).is_err());
    }

    #[tokio::test]
    async fn test_target_requires_supported_extension() {
        let protocol = S3Protocol::new();
        let url = Url::parse("s3://bucket/output").unwrap();
        let result = protocol.create_target(&url, &HashMap::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_source_reads_object() {
        let store = memory_store();
        store
            .put(
                &ObjectPath::from("in/people.csv"),
                "id,name\n1,Alice\n2,Bob\n".into(),
            )
            .await
            .unwrap();

        let rows = read_rows(&store, "in/people.csv", "csv").await;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].get("name"),
            Some(&Value::String("Alice".to_string()))
        );
    }

    #[tokio::test]
    async fn test_source_object_not_found() {
        let store = memory_store();
        let result = S3Source::download(
            store,
            &ObjectPath::from("missing.csv"),
            "s3://bucket/missing.csv",
            "csv",
        )
        .await;

        let err = result.err().unwrap().to_string();
        assert!(err.contains("S3 object not found: s3://bucket/missing.csv"));
    }

    #[tokio::test]
    async fn test_target_uploads_on_finalize() {
        let store = memory_store();
        let key = ObjectPath::from("out/people.csv");

        let mut target = S3Target::new(
            Arc::clone(&store),
            key.clone(),
            "s3://bucket/out/people.csv".to_string(),
            "csv",
        )
        .unwrap();

        target.connect().await.unwrap();
        assert!(!target.exists("people").await.unwrap());
        target
            .create_table("people", &sample_schema())
            .await
            .unwrap();
        target.write_batch(&sample_rows()).await.unwrap();

        // Nothing is uploaded until the transfer is finalized
        assert!(store.head(&key).await.is_err());

        target.finalize().await.unwrap();
        assert!(target.exists("people").await.unwrap());

        let bytes = store.get(&key).await.unwrap().bytes().await.unwrap();
        let content = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(content, "id,name\n1,Alice\n2,Bob\n");
    }

    #[tokio::test]
    async fn test_target_overwrites_existing_object() {
        let store = memory_store();
        let key = ObjectPath::from("out/people.csv");
        store.put(&key, "stale contents".into()).await.unwrap();

        write_rows(&store, "out/people.csv", "csv").await;

        let bytes = store.get(&key).await.unwrap().bytes().await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("stale"));
    }

    #[tokio::test]
    async fn test_round_trip_all_formats() {
        let store = memory_store();

        for format in SUPPORTED_FORMATS {
            let key = format!("round_trip/people.{}", format);
            write_rows(&store, &key, format).await;

            let rows = read_rows(&store, &key, format).await;
            assert_eq!(rows.len(), 2, "row count for {}", format);

            let mut names: Vec<String> = rows
                .iter()
                .map(|row| match row.get("name") {
                    Some(Value::String(name)) => name.clone(),
                    other => panic!("unexpected name value for {}: {:?}", format, other),
                })
                .collect();
            names.sort();
            assert_eq!(names, vec!["Alice", "Bob"], "names for {}", format);
        }
    }
}
