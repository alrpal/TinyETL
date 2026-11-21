//! The interface between the Snowflake connector and the client library that talks to Snowflake.
//!
//! Only TinyETL and Arrow types cross this boundary, so the library behind it (currently `firn`,
//! in `firn_client.rs`) can be replaced without touching the connector.

use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use futures_util::stream::{BoxStream, StreamExt};
use std::path::Path;

use crate::{
    connectors::arrow_values::record_batch_to_rows,
    schema::{DataType, Row},
    Result,
};

/// A result column, as Snowflake describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

/// A query result, read batch by batch. Batches use native Arrow types (decimals, timestamps,
/// dates); VARIANT / OBJECT / ARRAY columns are JSON text with `logicalType` field metadata.
pub struct QueryStream {
    pub columns: Vec<ColumnInfo>,
    /// Rows in the whole result, when Snowflake reports it.
    pub total_rows: Option<u64>,
    pub batches: BoxStream<'static, Result<RecordBatch>>,
}

#[async_trait]
pub trait SnowflakeClient: Send + Sync {
    /// Run a statement, binding `binds` to its `?` placeholders as text (`None` is NULL).
    async fn execute(&self, sql: &str, binds: Vec<Option<String>>) -> Result<()>;

    /// Run a query and stream its result.
    async fn query(&self, sql: &str) -> Result<QueryStream>;

    /// Upload an already gzip-compressed local file to a stage location such as
    /// `@%ORDERS/tinyetl/<run>/`.
    async fn put(&self, local_file: &Path, stage_location: &str) -> Result<()>;

    /// End the session.
    async fn close(&self) -> Result<()>;
}

/// Run a query with a small result and collect all of its rows.
pub async fn query_rows(client: &dyn SnowflakeClient, sql: &str) -> Result<Vec<Row>> {
    let mut result = client.query(sql).await?;
    let mut rows = Vec::new();
    while let Some(batch) = result.batches.next().await {
        rows.extend(record_batch_to_rows(&batch?)?);
    }
    Ok(rows)
}
