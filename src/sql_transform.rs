//! SQL transforms (`--sql`, YAML `sql:`).
//!
//! Every source row is loaded into a table named `source` in a temporary DuckDB database, the
//! query runs there once, and its result becomes the transfer's source. Unlike the row-by-row
//! Lua transforms, the query sees the whole dataset, so aggregates, joins of the table with
//! itself, window functions, `DISTINCT` and `ORDER BY` give the right answer. DuckDB spills to
//! disk, so memory stays bounded; the staged copy needs free disk space in the temp directory.

use async_trait::async_trait;
use std::time::Instant;
use tracing::info;

use crate::{
    connectors::{
        duckdb::{DuckdbSource, DuckdbTarget},
        Source, Target,
    },
    schema::{Row, Schema, SchemaFile},
    Result, TinyEtlError,
};

/// The table name the query reads from
pub const TABLE: &str = "source";

/// Load every row of `source` into a temporary DuckDB table and return a source that reads
/// the result of `sql` over it, with the result's schema. Rows are checked against the schema
/// file first, if there is one; `schema` is the input's schema.
pub async fn stage(
    mut source: Box<dyn Source>,
    schema: &Schema,
    schema_file: Option<&SchemaFile>,
    sql: &str,
    batch_size: usize,
) -> Result<(Box<dyn Source>, Schema)> {
    if schema.columns.is_empty() {
        return Err(TinyEtlError::Configuration(
            "The SQL transform needs a source with columns".to_string(),
        ));
    }
    let started = Instant::now();
    let dir = tempfile::Builder::new()
        .prefix("tinyetl-sql-")
        .tempdir()
        .map_err(|e| {
            TinyEtlError::DataTransfer(format!(
                "Failed to create a temporary directory for the SQL transform: {}",
                e
            ))
        })?;
    let db_path = dir.path().join("stage.duckdb").display().to_string();

    info!("→ Loading the source into DuckDB for the SQL transform");
    let mut staging = DuckdbTarget::new(&format!("{}#{}", db_path, TABLE))?;
    staging.connect().await?;
    staging.create_table(TABLE, schema).await?;
    source.reset().await?;
    let mut staged = 0;
    let mut nulled = 0;
    while source.has_more() {
        let mut batch = source.read_batch(batch_size).await?;
        if batch.is_empty() {
            break;
        }
        nulled += crate::transfer::TransferEngine::prepare_batch(&mut batch, schema_file, schema)?;
        staged += staging.write_batch(&batch).await?;
    }
    staging.finalize().await?;
    drop(staging);
    if nulled > 0 {
        tracing::warn!(
            "{} values didn't parse as their column's type and were loaded as NULL (on_mismatch: null)",
            nulled
        );
    }
    info!(
        "→ Loaded {} rows in {:.1}s; running the SQL transform",
        staged,
        started.elapsed().as_secs_f64()
    );

    let mut result = DuckdbSource::with_query(&db_path, sql)?;
    result.connect().await?;
    let result_schema = result.infer_schema(0).await.map_err(|e| match e {
        TinyEtlError::DataTransfer(message) => TinyEtlError::Configuration(format!(
            "SQL transform (the source rows are in a table named \"{}\"): {}",
            TABLE,
            message.trim_start_matches("Failed to run the query: ")
        )),
        other => other,
    })?;
    Ok((Box::new(Staged { result, _dir: dir }), result_schema))
}

/// The query's result; the temporary directory is deleted when the transfer drops it
struct Staged {
    result: DuckdbSource,
    _dir: tempfile::TempDir,
}

#[async_trait]
impl Source for Staged {
    async fn connect(&mut self) -> Result<()> {
        Ok(()) // connected by `stage`
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        self.result.infer_schema(sample_size).await
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        self.result.read_batch(batch_size).await
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        self.result.estimated_row_count().await
    }

    async fn reset(&mut self) -> Result<()> {
        self.result.reset().await
    }

    fn has_more(&self) -> bool {
        self.result.has_more()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::create_source;
    use crate::schema::{DataType, Value};

    async fn transform(csv: &str, sql: &str) -> Result<(Schema, Vec<Row>)> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.csv");
        std::fs::write(&path, csv).unwrap();
        let mut source = create_source(path.to_str().unwrap())?;
        source.connect().await?;
        let schema = source.infer_schema(1000).await?;
        let (mut result, result_schema) = stage(source, &schema, None, sql, 2).await?;
        result.reset().await?;
        let mut rows = Vec::new();
        while result.has_more() {
            let batch = result.read_batch(100).await?;
            if batch.is_empty() {
                break;
            }
            rows.extend(batch);
        }
        Ok((result_schema, rows))
    }

    #[tokio::test]
    async fn test_aggregate_over_all_batches() {
        // Batches of 2 rows: an aggregate must still see all of them
        let (schema, rows) = transform(
            "region,amount\nwest,10.10\neast,20.20\nwest,30.30\neast,40.40\nwest,0.01\n",
            "SELECT region, COUNT(*) AS n, SUM(amount) AS total FROM source GROUP BY region ORDER BY region",
        )
        .await
        .unwrap();
        assert_eq!(schema.columns[1].data_type, DataType::Integer);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["region"], Value::String("east".into()));
        assert_eq!(rows[0]["n"], Value::Integer(2));
        assert_eq!(rows[0]["total"], Value::Decimal("60.6".parse().unwrap()));
        assert_eq!(rows[1]["total"], Value::Decimal("40.41".parse().unwrap()));
    }

    #[tokio::test]
    async fn test_nested_results_are_json() {
        let (schema, rows) = transform(
            "id,tag\n1,a\n2,b\n3,a\n",
            "SELECT tag, list(id ORDER BY id) AS ids FROM source GROUP BY tag ORDER BY tag",
        )
        .await
        .unwrap();
        assert_eq!(schema.columns[1].data_type, DataType::Json);
        assert_eq!(rows[0]["ids"], Value::Json(serde_json::json!([1, 3])));
    }

    #[tokio::test]
    async fn test_errors_name_the_table() {
        let err = transform("a\n1\n", "SELECT * FROM input")
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("a table named \"source\""), "{}", err);
    }
}
