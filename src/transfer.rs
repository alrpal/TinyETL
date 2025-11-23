use indicatif::{ProgressBar, ProgressStyle};
use std::time::Instant;
use tracing::{error, info, warn};

use crate::{
    config::Config,
    connectors::{Source, Target},
    schema::{Schema, SchemaFile},
    transformer::Transformer,
    Result, TinyEtlError,
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
        let schema = if let Some(schema_file_path) = &config.schema_file {
            info!("→ Loading schema from file: {}", schema_file_path);
            let schema_file = SchemaFile::from_file(schema_file_path)?;
            // Even when using a schema file, we need to initialize the source
            // (e.g., for pagination state in SQLite)
            let _ = source.infer_schema(1).await?;
            schema_file.to_schema()?
        } else {
            info!("→ Inferring schema from source...");
            source.infer_schema(1000).await?
        };
        info!("→ {} columns detected", schema.columns.len());

        // Step 3: Handle preview mode
        if let Some(preview_rows) = config.preview {
            return Self::handle_preview(source, &schema, preview_rows, config).await;
        }

        // Step 4: Handle dry run mode
        if config.dry_run {
            return Self::handle_dry_run(source, target, &schema, config).await;
        }

        // Step 5: Initialize transformer and determine final schema
        let mut transformer = Transformer::new(&config.transform)?;
        let final_schema = if transformer.is_enabled() {
            info!("→ Transformation enabled");

            // Read a small sample to infer the transformed schema
            source.reset().await?;
            let sample_batch = source.read_batch(1).await?;

            if !sample_batch.is_empty() {
                let transformed_sample = transformer.transform_batch(&sample_batch)?;
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

        // Load schema file for validation if provided
        let schema_file = if let Some(schema_file_path) = &config.schema_file {
            Some(SchemaFile::from_file(schema_file_path)?)
        } else {
            None
        };

        let mut total_rows = 0;
        let mut batches_processed = 0;

        source.reset().await?;

        while source.has_more() {
            let mut batch = source.read_batch(config.batch_size).await?;
            if batch.is_empty() {
                break;
            }

            // Apply schema validation and defaults if schema file is provided
            if let Some(ref schema_file) = schema_file {
                for row in &mut batch {
                    schema_file.validate_and_transform_row(row)?;
                }
            }

            // Apply transformations
            let processed_batch = if transformer.is_enabled() {
                transformer.transform_batch(&batch)?
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

    async fn handle_preview(
        mut source: Box<dyn Source>,
        schema: &Schema,
        preview_rows: usize,
        config: &Config,
    ) -> Result<TransferStats> {
        // Initialize transformer for preview
        let mut transformer = Transformer::new(&config.transform)?;

        println!("\nOriginal Schema Preview:");
        Self::print_schema(schema);

        source.reset().await?;
        let sample_data = source.read_batch(preview_rows).await?;

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

        println!("\nData Preview ({} rows):", preview_rows);
        Self::print_data_table(&final_data);

        Ok(TransferStats {
            total_rows: 0,
            total_time: std::time::Duration::from_secs(0),
            rows_per_second: 0.0,
            batches_processed: 0,
        })
    }

    fn print_schema(schema: &Schema) {
        println!("┌─────────────────────┬───────────────┬──────────┐");
        println!("│ Column              │ Type          │ Nullable │");
        println!("├─────────────────────┼───────────────┼──────────┤");

        for column in &schema.columns {
            println!(
                "│ {:<19} │ {:<13} │ {:<8} │",
                column.name,
                format!("{}", column.data_type),
                format!("{}", column.nullable)
            );
        }
        println!("└─────────────────────┴───────────────┴──────────┘");
    }

    fn print_data_table(data: &[crate::schema::Row]) {
        if data.is_empty() {
            println!("No data to display");
            return;
        }

        // Print column headers
        let headers: Vec<&String> = data[0].keys().collect();

        // Print top border
        print!("┌");
        for i in 0..headers.len() {
            print!("─────────────────");
            if i < headers.len() - 1 {
                print!("┬");
            }
        }
        println!("┐");

        // Print headers
        print!("│");
        for header in &headers {
            print!(" {:<15} │", header);
        }
        println!();

        // Print separator
        print!("├");
        for i in 0..headers.len() {
            print!("─────────────────");
            if i < headers.len() - 1 {
                print!("┼");
            }
        }
        println!("┤");

        // Print data rows
        for row in data {
            print!("│");
            for header in &headers {
                let value_str = match row.get(*header) {
                    Some(value) => format!("{:?}", value).chars().take(15).collect(),
                    None => "NULL".to_string(),
                };
                print!(" {:<15} │", value_str);
            }
            println!();
        }

        // Print bottom border
        print!("└");
        for i in 0..headers.len() {
            print!("─────────────────");
            if i < headers.len() - 1 {
                print!("┴");
            }
        }
        println!("┘");
    }

    async fn handle_dry_run(
        mut source: Box<dyn Source>,
        target: Box<dyn Target>,
        schema: &Schema,
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
            let test_batch = source.read_batch(10).await?; // Small sample for testing

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
}
