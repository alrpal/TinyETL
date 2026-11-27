//! Type detection across every source connector.
//!
//! Each connector reads a fixture full of the values that trip up type detection: codes with
//! leading zeros, numbers with thousands separators, dates in several layouts, booleans in
//! mixed case, nested values, the extremes of each database type. A check names a column, the
//! type TinyETL should give it, and the values a target should receive, rendered as text
//! (`int 1`, `dec 1.50`, `str 01826`, `date 2024-01-15`, `ts 2024-01-15T10:30:00Z`,
//! `json {"a":1}`, `bool true`, `null`).
//!
//! Rows go through the same steps as a transfer with no schema file: `infer_schema(1000)`,
//! then every row converted to the schema with `coerce_row_with` (as `TransferEngine` does).
//!
//! The file connectors always run. PostgreSQL, MySQL and SQL Server run against the
//! `test_env` containers (or `TINYETL_TEST_POSTGRES_URL` / `_MYSQL_URL` / `_MSSQL_URL`) and
//! are skipped when they can't connect, unless `TINYETL_TEST_REQUIRE_DATABASES=1`.
//! With `TINYETL_TEST_ODBC=1`, each database table is also read through `odbc://` with an
//! installed driver; `test_env/odbc/run.sh` does that on Linux in Docker.
//! Snowflake runs when `SNOWFLAKE_*` settings are in the environment or the repo's `.env`.
//!
//! `cargo test --test type_detection -- --nocapture` prints every connector's results.

mod databases;
mod files;
mod snowflake;
mod text;

use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::Timelike;
use tinyetl::connectors::create_source_from_url_with_type_and_options;
use tinyetl::schema::{coerce_row_with, DataType, Row, Schema, Value};

/// What one column should come out as
pub struct Check {
    pub column: String,
    pub data_type: DataType,
    pub values: Vec<String>,
}

pub fn check(column: &str, data_type: DataType, values: &[&str]) -> Check {
    Check {
        column: column.to_string(),
        data_type,
        values: values.iter().map(|v| v.to_string()).collect(),
    }
}

/// A value as the text the checks compare
pub fn render(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(s) => format!("str {}", s),
        Value::Integer(i) => format!("int {}", i),
        Value::Decimal(d) => format!("dec {}", d),
        Value::Boolean(b) => format!("bool {}", b),
        Value::Date(dt) => {
            let naive = dt.naive_utc();
            if naive.time().num_seconds_from_midnight() == 0 && naive.time().nanosecond() == 0 {
                format!("date {}", naive.format("%Y-%m-%d"))
            } else {
                format!(
                    "ts {}",
                    dt.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
                )
            }
        }
        Value::Json(j) => format!("json {}", j),
    }
}

/// Read a source the way a transfer does: infer, then convert every row to the schema
pub async fn detect(location: &str) -> (Schema, Vec<Row>) {
    try_detect(location)
        .await
        .unwrap_or_else(|e| panic!("{}: {}", location, e))
}

/// [`detect`], with errors as text
pub async fn try_detect(location: &str) -> Result<(Schema, Vec<Row>), String> {
    try_detect_with(location, &HashMap::new()).await
}

/// [`try_detect`] with endpoint options (`--source-option`)
pub async fn try_detect_with(
    location: &str,
    options: &HashMap<String, String>,
) -> Result<(Schema, Vec<Row>), String> {
    let mut source = create_source_from_url_with_type_and_options(location, None, options)
        .await
        .map_err(|e| e.to_string())?;
    source.connect().await.map_err(|e| e.to_string())?;
    let schema = source.infer_schema(1000).await.map_err(|e| e.to_string())?;
    source.reset().await.map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    while source.has_more() {
        let batch = source.read_batch(500).await.map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        rows.extend(batch);
    }
    let day_first = source.day_first_columns();
    for row in &mut rows {
        coerce_row_with(row, &schema, &day_first);
    }
    Ok((schema, rows))
}

/// Compare a source's schema and rows with the checks; returns the differences, one per line
pub fn compare(schema: &Schema, rows: &[Row], checks: &[Check]) -> Vec<String> {
    let mut problems = Vec::new();
    for check in checks {
        let Some(column) = schema.columns.iter().find(|c| c.name == check.column) else {
            let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
            problems.push(format!(
                "{}: no such column (columns: {})",
                check.column,
                names.join(", ")
            ));
            continue;
        };
        let got: Vec<String> = rows
            .iter()
            .take(check.values.len())
            .map(|row| render(row.get(&check.column).unwrap_or(&Value::Null)))
            .collect();
        if column.data_type != check.data_type || got != check.values {
            let mut line = format!("{}:", check.column);
            if column.data_type != check.data_type {
                let _ = write!(
                    line,
                    " type {:?}, expected {:?};",
                    column.data_type, check.data_type
                );
            }
            if got != check.values {
                let _ = write!(line, " values {:?}, expected {:?}", got, check.values);
            }
            problems.push(line);
        }
    }
    problems
}

/// Print a connector's results and fail the test if any check failed
pub fn report(connector: &str, total: usize, problems: Vec<String>) {
    if problems.is_empty() {
        println!("✓ {}: {} columns as expected", connector, total);
        return;
    }
    let mut message = format!(
        "{}: {} of {} columns differ\n",
        connector,
        problems.len(),
        total
    );
    for problem in &problems {
        let _ = writeln!(message, "  ✗ {}", problem);
    }
    println!("{}", message);
    panic!("{}", message);
}

/// Run `checks` against one source and report
pub async fn run(connector: &str, location: &str, checks: &[Check]) {
    let (schema, rows) = detect(location).await;
    report(connector, checks.len(), compare(&schema, &rows, checks));
}
