use indicatif::{ProgressBar, ProgressStyle};
use std::time::Instant;
use tracing::{info, warn};

use crate::{
    config::Config,
    connectors::{Source, Target},
    schema::{Schema, SchemaFile},
    transformer::Transformer,
    Result,
};

pub struct TransferEngine;

pub struct TransferStats {
    pub total_rows: usize,
    pub total_time: std::time::Duration,
    pub rows_per_second: f64,
    pub batches_processed: usize,
}

impl TransferEngine {
    pub async fn execute(
        config: &Config,
        mut source: Box<dyn Source>,
        mut target: Box<dyn Target>,
    ) -> Result<TransferStats> {
        let start_time = Instant::now();

        // Step 1: Connect to source and target
        info!("→ Connecting to source: {}", config.source);
        source.connect().await?;

        info!("→ Connecting to target: {}", config.target);
        target.connect().await?;

        // Step 2: Infer or load schema
        let schema_file = match &config.schema_file {
            Some(path) => Some(SchemaFile::from_file(path)?),
            None => None,
        };
        let schema = if let Some(schema_file) = &schema_file {
            info!(
                "→ Loading schema from file: {}",
                config.schema_file.as_deref().unwrap_or_default()
            );
            // The file's columns get their declared types; any others are inferred
            schema_file.apply_to(&source.infer_schema(1000).await?)
        } else {
            info!("→ Inferring schema from source...");
            source.infer_schema(1000).await?
        };
        info!("→ {} columns detected", schema.columns.len());

        // Step 2b: SQL transform. The source rows (checked against the schema file) go into a
        // DuckDB table; from here on the source is the query's result.
        let (mut source, schema, schema_file) = match &config.sql {
            Some(sql) => {
                if config.transform != crate::transformer::TransformConfig::None {
                    return Err(crate::TinyEtlError::Configuration(
                        "Use either a SQL transform or a Lua transform, not both".to_string(),
                    ));
                }
                let (staged, result_schema) = crate::sql_transform::stage(
                    source,
                    &schema,
                    schema_file.as_ref(),
                    sql,
                    config.batch_size,
                )
                .await?;
                info!("→ SQL transform: {} columns", result_schema.columns.len());
                (staged, result_schema, None)
            }
            None => (source, schema, schema_file),
        };

        // Step 3: Handle preview mode
        if let Some(preview_rows) = config.preview {
            return Self::handle_preview(
                source,
                &schema,
                schema_file.as_ref(),
                preview_rows,
                config,
            )
            .await;
        }

        // Step 4: Handle dry run mode
        if config.dry_run {
            return Self::handle_dry_run(source, target, &schema, schema_file.as_ref(), config)
                .await;
        }

        // Step 5: Initialize transformer and determine final schema
        let mut transformer = Transformer::new(&config.transform)?;
        let final_schema = if transformer.is_enabled() {
            info!("→ Transformation enabled");

            // Read a small sample to infer the transformed schema
            source.reset().await?;
            let mut sample_batch = source.read_batch(1).await?;
            Self::prepare_batch(&mut sample_batch, schema_file.as_ref(), &schema)?;

            if !sample_batch.is_empty() {
                transformer.transform_batch(&sample_batch)?;
                if let Some(transform_schema) = transformer.get_inferred_schema() {
                    info!(
                        "→ Schema updated by transformations: {} columns",
                        transform_schema.columns.len()
                    );
                    transform_schema.clone()
                } else {
                    schema.clone()
                }
            } else {
                schema.clone()
            }
        } else {
            schema.clone()
        };

        // Step 6: Extract table name from target
        let table_name = Self::extract_table_name(&config.target);

        // Step 7: Handle append-first logic or truncate mode
        let table_exists = target.exists(&table_name).await?;

        if table_exists {
            if config.truncate {
                info!("→ Truncating existing target: {}", table_name);
                target.truncate(&table_name).await?;
                // After truncating, we need to create the table again
                target.create_table(&table_name, &final_schema).await?;
            } else if target.supports_append() {
                info!("→ Appending to existing target: {}", table_name);
                // For append mode, we still need to set the schema for the target
                target.create_table(&table_name, &final_schema).await?;
            } else {
                // Target exists but doesn't support append - must truncate
                info!(
                    "→ Target exists but doesn't support append, truncating: {}",
                    table_name
                );
                target.truncate(&table_name).await?;
                // After truncating, we need to create the table again
                target.create_table(&table_name, &final_schema).await?;
            }
        } else {
            // Step 8: Create target table with final schema if it doesn't exist
            info!("→ Creating target table: {}", table_name);
            target.create_table(&table_name, &final_schema).await?;
        }

        // Step 9: Transfer data
        let estimated_rows = source.estimated_row_count().await?.unwrap_or(0);
        info!("→ Copying {} rows", estimated_rows);

        let progress_bar = if estimated_rows > 0 {
            let pb = ProgressBar::new(estimated_rows as u64);
            let style = ProgressStyle::default_bar()
                .template("{bar:40.cyan/blue} {percent}% ({pos}/{len}) {msg}")
                .expect("Failed to create progress bar template")
                .progress_chars("█▇▆▅▄▃▂▁  ");
            pb.set_style(style);
            Some(pb)
        } else {
            None
        };

        let mut total_rows = 0;
        let mut batches_processed = 0;
        let mut nulled = 0;

        source.reset().await?;

        while source.has_more() {
            let mut batch = source.read_batch(config.batch_size).await?;
            if batch.is_empty() {
                break;
            }

            nulled += Self::prepare_batch(&mut batch, schema_file.as_ref(), &schema)?;

            // Apply transformations; their output gets the output schema's types
            let processed_batch = if transformer.is_enabled() {
                let mut transformed = transformer.transform_batch(&batch)?;
                for row in &mut transformed {
                    crate::schema::coerce_row(row, &final_schema);
                }
                transformed
            } else {
                batch
            };

            let written = target.write_batch(&processed_batch).await?;
            total_rows += written;
            batches_processed += 1;

            if let Some(ref pb) = progress_bar {
                pb.set_position(total_rows as u64);
                pb.set_message(format!(
                    "{}k rows/sec",
                    (total_rows as f64 / start_time.elapsed().as_secs_f64() / 1000.0) as u64
                ));
            }
        }

        if let Some(pb) = progress_bar {
            pb.finish_with_message("Complete");
        }

        // Step 10: Finalize
        target.finalize().await?;
        if nulled > 0 {
            warn!(
                "{} values didn't parse as their column's type and were written as NULL (on_mismatch: null)",
                nulled
            );
        }

        let total_time = start_time.elapsed();
        let rows_per_second = total_rows as f64 / total_time.as_secs_f64();

        info!("→ Done in {:.1}s", total_time.as_secs_f64());

        Ok(TransferStats {
            total_rows,
            total_time,
            rows_per_second,
            batches_processed,
        })
    }

    /// Rows as read become rows with the schema's types: the schema file's checks and declared
    /// types first (text in a String column stays as written), then every other column is
    /// converted to its type. Sources hand over text as written (CSV, JSON strings) or typed
    /// values; the schema came from a sample, so this also gives targets that bind typed
    /// parameters (e.g. Postgres) one type per column. Returns the values set to NULL under
    /// `on_mismatch: null`.
    pub(crate) fn prepare_batch(
        batch: &mut [crate::schema::Row],
        schema_file: Option<&SchemaFile>,
        schema: &Schema,
    ) -> Result<usize> {
        let mut nulled = 0;
        for row in batch.iter_mut() {
            if let Some(schema_file) = schema_file {
                nulled += schema_file.validate_and_transform_row(row)?;
            }
            crate::schema::coerce_row(row, schema);
        }
        Ok(nulled)
    }

    async fn handle_preview(
        mut source: Box<dyn Source>,
        schema: &Schema,
        schema_file: Option<&SchemaFile>,
        preview_rows: usize,
        config: &Config,
    ) -> Result<TransferStats> {
        // Initialize transformer for preview
        let mut transformer = Transformer::new(&config.transform)?;

        println!("\nOriginal Schema Preview:");
        Self::print_schema(schema);

        source.reset().await?;
        let mut sample_data = source.read_batch(preview_rows).await?;
        Self::prepare_batch(&mut sample_data, schema_file, schema)?;

        let (final_schema, final_data) = if transformer.is_enabled() && !sample_data.is_empty() {
            println!("\nApplying transformations...");
            let transformed_data = transformer.transform_batch(&sample_data)?;

            if let Some(transform_schema) = transformer.get_inferred_schema() {
                println!("\nTransformed Schema Preview:");
                Self::print_schema(transform_schema);
                (transform_schema, transformed_data)
            } else {
                (schema, sample_data)
            }
        } else {
            (schema, sample_data)
        };

        println!("\nData Preview ({} rows):", final_data.len());
        Self::print_data_table(final_schema, &final_data);

        Ok(TransferStats {
            total_rows: 0,
            total_time: std::time::Duration::from_secs(0),
            rows_per_second: 0.0,
            batches_processed: 0,
        })
    }

    fn print_schema(schema: &Schema) {
        let rows: Vec<Vec<String>> = schema
            .columns
            .iter()
            .map(|c| {
                vec![
                    c.name.clone(),
                    c.data_type.to_string(),
                    c.nullable.to_string(),
                ]
            })
            .collect();
        Self::print_table(&["Column", "Type", "Nullable"], &rows);
    }

    /// Preview rows in the schema's column order, as plain values.
    fn print_data_table(schema: &Schema, data: &[crate::schema::Row]) {
        if data.is_empty() {
            println!("No data to display");
            return;
        }
        let headers: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        let rows: Vec<Vec<String>> = data
            .iter()
            .map(|row| {
                headers
                    .iter()
                    .map(|name| match row.get(*name) {
                        None | Some(crate::schema::Value::Null) => "NULL".to_string(),
                        Some(crate::schema::Value::Date(dt)) => {
                            let naive = dt.naive_utc();
                            if naive.time() == chrono::NaiveTime::MIN {
                                naive.format("%Y-%m-%d").to_string()
                            } else {
                                naive.format("%Y-%m-%d %H:%M:%S%.f").to_string()
                            }
                        }
                        Some(value) => value.to_string_for_arrow().unwrap_or_default(),
                    })
                    .collect()
            })
            .collect();
        Self::print_table(&headers, &rows);
    }

    /// A box-drawn table; columns are as wide as their contents, up to 30 characters.
    fn print_table(headers: &[&str], rows: &[Vec<String>]) {
        const MAX_WIDTH: usize = 30;
        let fit = |text: &str| -> String {
            let flat: String = text
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            if flat.chars().count() > MAX_WIDTH {
                flat.chars().take(MAX_WIDTH - 1).chain(['…']).collect()
            } else {
                flat
            }
        };
        let headers: Vec<String> = headers.iter().map(|h| fit(h)).collect();
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|row| row.iter().map(|v| fit(v)).collect())
            .collect();
        let widths: Vec<usize> = (0..headers.len())
            .map(|i| {
                rows.iter()
                    .filter_map(|row| row.get(i))
                    .chain([&headers[i]])
                    .map(|v| v.chars().count())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let line = |left: &str, mid: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            println!("{}{}{}", left, parts.join(mid), right);
        };
        let cells = |values: &[String]| {
            let parts: Vec<String> = values
                .iter()
                .zip(&widths)
                .map(|(v, w)| format!(" {}{} ", v, " ".repeat(w - v.chars().count())))
                .collect();
            println!("│{}│", parts.join("│"));
        };
        line("┌", "┬", "┐");
        cells(&headers);
        line("├", "┼", "┤");
        for row in &rows {
            cells(row);
        }
        line("└", "┴", "┘");
    }

    async fn handle_dry_run(
        mut source: Box<dyn Source>,
        target: Box<dyn Target>,
        schema: &Schema,
        schema_file: Option<&SchemaFile>,
        config: &Config,
    ) -> Result<TransferStats> {
        info!("Dry run mode - validating connections and schema");

        let estimated_rows = source.estimated_row_count().await?.unwrap_or(0);
        info!("Source connection validated");
        info!("Schema inferred: {} columns", schema.columns.len());
        info!("Estimated rows: {}", estimated_rows);

        // Test transformations if enabled
        let mut transformer = Transformer::new(&config.transform)?;
        if transformer.is_enabled() {
            info!("Testing transformations...");
            source.reset().await?;
            let mut test_batch = source.read_batch(10).await?; // Small sample for testing
            Self::prepare_batch(&mut test_batch, schema_file, schema)?;

            if !test_batch.is_empty() {
                let _transformed = transformer.transform_batch(&test_batch)?;
                if let Some(transform_schema) = transformer.get_inferred_schema() {
                    info!(
                        "Transformation successful: {} output columns",
                        transform_schema.columns.len()
                    );
                } else {
                    info!("Transformation successful: schema unchanged");
                }
            }
        }

        let table_name = Self::extract_table_name(&config.target);
        let table_exists = target.exists(&table_name).await?;

        if table_exists {
            warn!("Target table '{}' already exists", table_name);
        } else {
            info!("Target table '{}' will be created", table_name);
        }

        info!("Dry run completed successfully");

        Ok(TransferStats {
            total_rows: 0,
            total_time: std::time::Duration::from_secs(0),
            rows_per_second: 0.0,
            batches_processed: 0,
        })
    }

    fn extract_table_name(target: &str) -> String {
        if target.contains('#') {
            target.split('#').nth(1).unwrap_or("data").to_string()
        } else {
            // Extract filename without extension for file targets
            std::path::Path::new(target)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("data")
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, DataType, Row, Value};
    use async_trait::async_trait;
    use std::collections::HashMap;

    // Mock source for testing
    struct MockSource {
        data: Vec<Row>,
        position: usize,
        connected: bool,
    }

    impl MockSource {
        fn new(data: Vec<Row>) -> Self {
            Self {
                data,
                position: 0,
                connected: false,
            }
        }
    }

    #[async_trait]
    impl Source for MockSource {
        async fn connect(&mut self) -> Result<()> {
            self.connected = true;
            Ok(())
        }

        async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
            Ok(Schema {
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
                estimated_rows: Some(self.data.len()),
                primary_key_candidate: None,
            })
        }

        async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
            let end = std::cmp::min(self.position + batch_size, self.data.len());
            let batch = self.data[self.position..end].to_vec();
            self.position = end;
            Ok(batch)
        }

        async fn estimated_row_count(&self) -> Result<Option<usize>> {
            Ok(Some(self.data.len()))
        }

        async fn reset(&mut self) -> Result<()> {
            self.position = 0;
            Ok(())
        }

        fn has_more(&self) -> bool {
            self.position < self.data.len()
        }
    }

    // Mock target for testing
    struct MockTarget {
        written_rows: Vec<Row>,
        connected: bool,
        table_created: bool,
    }

    impl MockTarget {
        fn new() -> Self {
            Self {
                written_rows: Vec::new(),
                connected: false,
                table_created: false,
            }
        }
    }

    #[async_trait]
    impl Target for MockTarget {
        async fn connect(&mut self) -> Result<()> {
            self.connected = true;
            Ok(())
        }

        async fn create_table(&mut self, _table_name: &str, _schema: &Schema) -> Result<()> {
            self.table_created = true;
            Ok(())
        }

        async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
            self.written_rows.extend_from_slice(rows);
            Ok(rows.len())
        }

        async fn finalize(&mut self) -> Result<()> {
            Ok(())
        }

        async fn exists(&self, _table_name: &str) -> Result<bool> {
            Ok(false)
        }

        async fn truncate(&mut self, _table_name: &str) -> Result<()> {
            self.written_rows.clear();
            Ok(())
        }

        fn supports_append(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_transfer_execution() {
        let mut test_data = Vec::new();
        let mut row1 = HashMap::new();
        row1.insert("id".to_string(), Value::Integer(1));
        row1.insert("name".to_string(), Value::String("Alice".to_string()));
        test_data.push(row1);

        let mut row2 = HashMap::new();
        row2.insert("id".to_string(), Value::Integer(2));
        row2.insert("name".to_string(), Value::String("Bob".to_string()));
        test_data.push(row2);

        let source = MockSource::new(test_data);
        let target = MockTarget::new();

        let config = Config {
            source: "test.csv".to_string(),
            target: "test.db#users".to_string(),
            batch_size: 10,
            ..Default::default()
        };

        let stats = TransferEngine::execute(&config, Box::new(source), Box::new(target))
            .await
            .unwrap();

        assert_eq!(stats.total_rows, 2);
        assert_eq!(stats.batches_processed, 1);
    }

    // ---- Engine contract tests: a source with a fixed schema and a target that records
    // every call, so the order of exists / truncate / create_table / write / finalize can
    // be checked for each case

    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        rows: Vec<Row>,
        created_with: Option<(String, Schema)>,
    }

    struct FakeSource {
        schema: Schema,
        rows: Vec<Row>,
        position: usize,
    }

    impl FakeSource {
        fn new(columns: &[(&str, DataType)], rows: Vec<Row>) -> Self {
            Self {
                schema: Schema {
                    columns: columns
                        .iter()
                        .map(|(name, data_type)| Column {
                            name: name.to_string(),
                            data_type: data_type.clone(),
                            nullable: true,
                        })
                        .collect(),
                    estimated_rows: Some(rows.len()),
                    primary_key_candidate: None,
                },
                rows,
                position: 0,
            }
        }
    }

    #[async_trait]
    impl Source for FakeSource {
        async fn connect(&mut self) -> Result<()> {
            Ok(())
        }
        async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
            Ok(self.schema.clone())
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

    struct RecordingTarget {
        log: Arc<Mutex<Log>>,
        exists: bool,
        appendable: bool,
        /// Fail the write that would bring the total past this many rows
        fail_after_rows: Option<usize>,
    }

    impl RecordingTarget {
        fn new(exists: bool, appendable: bool) -> (Self, Arc<Mutex<Log>>) {
            let log = Arc::new(Mutex::new(Log::default()));
            (
                Self {
                    log: log.clone(),
                    exists,
                    appendable,
                    fail_after_rows: None,
                },
                log,
            )
        }
        fn record(&self, call: impl Into<String>) {
            self.log.lock().unwrap().calls.push(call.into());
        }
    }

    #[async_trait]
    impl Target for RecordingTarget {
        async fn connect(&mut self) -> Result<()> {
            self.record("connect");
            Ok(())
        }
        async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
            self.record("create_table");
            self.log.lock().unwrap().created_with = Some((table_name.to_string(), schema.clone()));
            Ok(())
        }
        async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
            let mut log = self.log.lock().unwrap();
            if let Some(limit) = self.fail_after_rows {
                if log.rows.len() + rows.len() > limit {
                    log.calls.push("write_batch failed".to_string());
                    return Err(crate::TinyEtlError::DataTransfer("disk full".to_string()));
                }
            }
            log.calls.push(format!("write_batch({})", rows.len()));
            log.rows.extend_from_slice(rows);
            Ok(rows.len())
        }
        async fn finalize(&mut self) -> Result<()> {
            self.record("finalize");
            Ok(())
        }
        async fn exists(&self, _table_name: &str) -> Result<bool> {
            self.record("exists");
            Ok(self.exists)
        }
        async fn truncate(&mut self, _table_name: &str) -> Result<()> {
            self.record("truncate");
            Ok(())
        }
        fn supports_append(&self) -> bool {
            self.appendable
        }
    }

    fn id_rows(n: i64) -> Vec<Row> {
        (0..n)
            .map(|i| {
                HashMap::from([
                    ("id".to_string(), Value::Integer(i)),
                    ("name".to_string(), Value::String(format!("n{}", i))),
                ])
            })
            .collect()
    }

    fn config(target: &str) -> Config {
        Config {
            source: "source.csv".to_string(),
            target: target.to_string(),
            batch_size: 10,
            ..Default::default()
        }
    }

    async fn run(config: &Config, source: FakeSource, target: RecordingTarget) -> Result<usize> {
        TransferEngine::execute(config, Box::new(source), Box::new(target))
            .await
            .map(|stats| stats.total_rows)
    }

    fn calls(log: &Arc<Mutex<Log>>) -> Vec<String> {
        log.lock().unwrap().calls.clone()
    }

    const ID_NAME: &[(&str, DataType)] = &[("id", DataType::Integer), ("name", DataType::String)];

    #[tokio::test]
    async fn test_new_target_is_created_written_in_batches_and_finalized() {
        let (target, log) = RecordingTarget::new(false, true);
        let source = FakeSource::new(ID_NAME, id_rows(25));
        assert_eq!(
            run(&config("out.db#users"), source, target).await.unwrap(),
            25
        );
        assert_eq!(
            calls(&log),
            [
                "connect",
                "exists",
                "create_table",
                "write_batch(10)",
                "write_batch(10)",
                "write_batch(5)",
                "finalize"
            ]
        );
        // The table name comes from the text after '#'
        let (table, _) = log.lock().unwrap().created_with.clone().unwrap();
        assert_eq!(table, "users");
    }

    #[tokio::test]
    async fn test_existing_target_is_appended_to() {
        let (target, log) = RecordingTarget::new(true, true);
        run(
            &config("t.db#t"),
            FakeSource::new(ID_NAME, id_rows(3)),
            target,
        )
        .await
        .unwrap();
        let calls = calls(&log);
        assert!(!calls.contains(&"truncate".to_string()), "{:?}", calls);
        // create_table is still called: it's how the target learns the schema
        assert!(calls.contains(&"create_table".to_string()));
    }

    #[tokio::test]
    async fn test_truncate_runs_before_create_table() {
        let (target, log) = RecordingTarget::new(true, true);
        let mut cfg = config("t.db#t");
        cfg.truncate = true;
        run(&cfg, FakeSource::new(ID_NAME, id_rows(3)), target)
            .await
            .unwrap();
        assert_eq!(&calls(&log)[1..4], ["exists", "truncate", "create_table"]);
    }

    #[tokio::test]
    async fn test_existing_target_that_cannot_append_is_truncated() {
        let (target, log) = RecordingTarget::new(true, false);
        run(
            &config("out.parquet"),
            FakeSource::new(ID_NAME, id_rows(3)),
            target,
        )
        .await
        .unwrap();
        assert!(calls(&log).contains(&"truncate".to_string()));
    }

    #[tokio::test]
    async fn test_missing_target_is_not_truncated_even_with_the_flag() {
        let (target, log) = RecordingTarget::new(false, true);
        let mut cfg = config("t.db#t");
        cfg.truncate = true;
        run(&cfg, FakeSource::new(ID_NAME, id_rows(3)), target)
            .await
            .unwrap();
        assert!(!calls(&log).contains(&"truncate".to_string()));
    }

    #[tokio::test]
    async fn test_empty_source_still_creates_and_finalizes() {
        let (target, log) = RecordingTarget::new(false, true);
        assert_eq!(
            run(&config("t.db#t"), FakeSource::new(ID_NAME, vec![]), target)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            calls(&log),
            ["connect", "exists", "create_table", "finalize"]
        );
    }

    #[tokio::test]
    async fn test_failed_write_stops_before_finalize() {
        // Database targets commit in finalize, so it must not run after a failure
        let (mut target, log) = RecordingTarget::new(false, true);
        target.fail_after_rows = Some(15);
        let err = run(
            &config("t.db#t"),
            FakeSource::new(ID_NAME, id_rows(25)),
            target,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("disk full"));
        let calls = calls(&log);
        assert_eq!(calls.last().unwrap(), "write_batch failed");
        assert!(!calls.contains(&"finalize".to_string()));
    }

    #[tokio::test]
    async fn test_values_are_converted_to_the_column_type_before_writing() {
        // The schema came from a sample: later rows disagree with it
        let rows = vec![
            HashMap::from([
                ("amount".to_string(), Value::Integer(84)),
                ("code".to_string(), Value::Integer(7)),
            ]),
            HashMap::from([
                ("amount".to_string(), Value::String(" ".to_string())),
                ("code".to_string(), Value::String("x".to_string())),
            ]),
        ];
        let source = FakeSource::new(
            &[("amount", DataType::Decimal), ("code", DataType::String)],
            rows,
        );
        let (target, log) = RecordingTarget::new(false, true);
        run(&config("t.db#t"), source, target).await.unwrap();
        let written = log.lock().unwrap().rows.clone();
        assert_eq!(
            written[0]["amount"],
            Value::Decimal(rust_decimal::Decimal::from(84))
        );
        assert_eq!(written[0]["code"], Value::String("7".to_string()));
        assert_eq!(written[1]["amount"], Value::Null); // blank in a numeric column
    }

    #[tokio::test]
    async fn test_transform_changes_the_schema_and_rows() {
        let (target, log) = RecordingTarget::new(false, true);
        let mut cfg = config("t.db#t");
        cfg.transform =
            crate::transformer::TransformConfig::Inline("double=row.id * 2".to_string());
        run(&cfg, FakeSource::new(ID_NAME, id_rows(3)), target)
            .await
            .unwrap();
        let log = log.lock().unwrap();
        let (_, schema) = log.created_with.clone().unwrap();
        assert!(schema.columns.iter().any(|c| c.name == "double"));
        assert_eq!(log.rows.len(), 3);
        assert_eq!(log.rows[2]["double"], Value::Integer(4));
        assert_eq!(log.rows[2]["name"], Value::String("n2".to_string())); // kept
    }

    #[tokio::test]
    async fn test_schema_file_rejects_a_bad_row_before_finalize() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("schema.yaml");
        std::fs::write(
            &schema_path,
            "columns:\n  - name: id\n    type: Integer\n  - name: name\n    type: String\n    pattern: '^n[0-9]$'\n",
        )
        .unwrap();
        let mut cfg = config("t.db#t");
        cfg.schema_file = Some(schema_path.to_string_lossy().into_owned());
        // n0..n9 match, n10 doesn't: the second batch fails validation
        let (target, log) = RecordingTarget::new(false, true);
        let err = run(&cfg, FakeSource::new(ID_NAME, id_rows(12)), target)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("does not match pattern"),
            "{}",
            err
        );
        let calls = calls(&log);
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.starts_with("write_batch"))
                .count(),
            1
        );
        assert!(!calls.contains(&"finalize".to_string()));
    }

    #[tokio::test]
    async fn test_preview_and_dry_run_never_write() {
        for mode in ["preview", "dry_run"] {
            let (target, log) = RecordingTarget::new(true, true);
            let mut cfg = config("t.db#t");
            if mode == "preview" {
                cfg.preview = Some(2);
            } else {
                cfg.dry_run = true;
            }
            run(&cfg, FakeSource::new(ID_NAME, id_rows(5)), target)
                .await
                .unwrap();
            let calls = calls(&log);
            for forbidden in ["truncate", "create_table", "finalize"] {
                assert!(
                    !calls.contains(&forbidden.to_string()),
                    "{}: {:?}",
                    mode,
                    calls
                );
            }
            assert!(
                !calls.iter().any(|c| c.starts_with("write_batch")),
                "{}",
                mode
            );
        }
    }

    #[test]
    fn test_extract_table_name() {
        assert_eq!(TransferEngine::extract_table_name("test.db#users"), "users");
        assert_eq!(TransferEngine::extract_table_name("output.csv"), "output");
        assert_eq!(TransferEngine::extract_table_name("data.json"), "data");
    }

    #[tokio::test]
    async fn test_transfer_with_preview() {
        let test_data = vec![{
            let mut row = HashMap::new();
            row.insert("id".to_string(), Value::Integer(1));
            row.insert("name".to_string(), Value::String("Alice".to_string()));
            row
        }];

        let source = MockSource::new(test_data);
        let target = MockTarget::new();

        let config = Config {
            source: "test.csv".to_string(),
            target: "test.db#users".to_string(),
            preview: Some(5),
            ..Default::default()
        };

        let stats = TransferEngine::execute(&config, Box::new(source), Box::new(target))
            .await
            .unwrap();

        // Preview mode should not transfer data
        assert_eq!(stats.total_rows, 0);
    }

    #[tokio::test]
    async fn test_transfer_with_dry_run() {
        let test_data = vec![{
            let mut row = HashMap::new();
            row.insert("id".to_string(), Value::Integer(1));
            row.insert("name".to_string(), Value::String("Alice".to_string()));
            row
        }];

        let source = MockSource::new(test_data);
        let target = MockTarget::new();

        let config = Config {
            source: "test.csv".to_string(),
            target: "test.db#users".to_string(),
            dry_run: true,
            ..Default::default()
        };

        let stats = TransferEngine::execute(&config, Box::new(source), Box::new(target))
            .await
            .unwrap();

        // Dry run should not transfer data
        assert_eq!(stats.total_rows, 0);
    }

    fn text_row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[tokio::test]
    async fn test_text_from_the_source_gets_column_types_before_a_transform() {
        // CSV rows arrive as text; Lua must still see numbers
        let (target, log) = RecordingTarget::new(false, true);
        let mut cfg = config("t.db#t");
        cfg.transform = crate::transformer::TransformConfig::Inline("big=row.n > 9".to_string());
        let source = FakeSource::new(
            &[("n", DataType::Integer)],
            vec![text_row(&[("n", "10")]), text_row(&[("n", "2")])],
        );
        run(&cfg, source, target).await.unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log.rows[0]["n"], Value::Integer(10));
        assert_eq!(log.rows[0]["big"], Value::Boolean(true));
        assert_eq!(log.rows[1]["big"], Value::Boolean(false));
    }

    #[tokio::test]
    async fn test_schema_file_types_listed_columns_and_infers_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("schema.yaml");
        std::fs::write(&schema_path, "columns:\n  - name: zip\n    type: String\n").unwrap();
        let mut cfg = config("t.db#t");
        cfg.schema_file = Some(schema_path.to_string_lossy().into_owned());
        let (target, log) = RecordingTarget::new(false, true);
        let source = FakeSource::new(
            &[("zip", DataType::Integer), ("n", DataType::Integer)],
            vec![text_row(&[("zip", "01826"), ("n", "5")])],
        );
        run(&cfg, source, target).await.unwrap();
        let log = log.lock().unwrap();
        let (_, schema) = log.created_with.clone().unwrap();
        let types: Vec<_> = schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.data_type.clone()))
            .collect();
        assert_eq!(types, [("zip", DataType::String), ("n", DataType::Integer)]);
        assert_eq!(log.rows[0]["zip"], Value::String("01826".to_string()));
        assert_eq!(log.rows[0]["n"], Value::Integer(5));
    }
}
