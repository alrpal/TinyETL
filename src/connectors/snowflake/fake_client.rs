//! In-memory [`SnowflakeClient`] for testing the connector without a Snowflake account. It
//! records every statement and upload, and answers queries from canned results.

use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::client::{ColumnInfo, QueryStream, SnowflakeClient};
use crate::{Result, TinyEtlError};

/// What the fake saw, shared with the test after the client is moved into a connector.
#[derive(Default)]
pub(super) struct Recorded {
    /// `EXEC <sql>` / `QUERY <sql>` / `PUT <stage>` in call order.
    pub log: Vec<String>,
    /// Bind values of each `execute` call that had any.
    pub binds: Vec<Vec<Option<String>>>,
    /// Decompressed contents of each uploaded file.
    pub uploads: Vec<String>,
}

#[derive(Clone)]
pub(super) struct FakeResult {
    pub columns: Vec<ColumnInfo>,
    pub total_rows: Option<u64>,
    pub batches: Vec<RecordBatch>,
}

#[derive(Default)]
pub(super) struct FakeClient {
    pub recorded: Arc<Mutex<Recorded>>,
    /// Queries whose SQL contains the key get the result.
    results: Vec<(String, FakeResult)>,
    /// Statements whose SQL contains this fail.
    fail_on: Option<String>,
}

impl FakeClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_result(mut self, sql_contains: &str, result: FakeResult) -> Self {
        self.results.push((sql_contains.to_string(), result));
        self
    }

    pub fn failing_on(mut self, sql_contains: &str) -> Self {
        self.fail_on = Some(sql_contains.to_string());
        self
    }

    fn check(&self, sql: &str) -> Result<()> {
        match &self.fail_on {
            Some(needle) if sql.contains(needle.as_str()) => Err(TinyEtlError::DataTransfer(
                format!("Snowflake: fake failure for {}", needle),
            )),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl SnowflakeClient for FakeClient {
    async fn execute(&self, sql: &str, binds: Vec<Option<String>>) -> Result<()> {
        let mut recorded = self.recorded.lock().unwrap();
        recorded.log.push(format!("EXEC {}", sql));
        if !binds.is_empty() {
            recorded.binds.push(binds);
        }
        drop(recorded);
        self.check(sql)
    }

    async fn query(&self, sql: &str) -> Result<QueryStream> {
        self.recorded
            .lock()
            .unwrap()
            .log
            .push(format!("QUERY {}", sql));
        self.check(sql)?;
        let result = self
            .results
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, result)| result.clone())
            .unwrap_or(FakeResult {
                columns: vec![],
                total_rows: None,
                batches: vec![],
            });
        Ok(QueryStream {
            columns: result.columns,
            total_rows: result.total_rows,
            batches: stream::iter(result.batches.into_iter().map(Ok)).boxed(),
        })
    }

    async fn put(&self, local_file: &Path, stage_location: &str) -> Result<()> {
        let mut text = String::new();
        zstd::stream::read::Decoder::new(std::fs::File::open(local_file)?)?
            .read_to_string(&mut text)?;
        let mut recorded = self.recorded.lock().unwrap();
        recorded.log.push(format!("PUT {}", stage_location));
        recorded.uploads.push(text);
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}
