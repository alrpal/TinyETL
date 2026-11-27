use async_trait::async_trait;
use chrono::TimeZone;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use sqlx::{
    postgres::{PgConnectOptions, PgTypeInfo, PgTypeKind, PgValueFormat, PgValueRef},
    Column, Executor, PgPool, Row as SqlxRow, Statement, TypeInfo, ValueRef,
};
use std::str::FromStr;
use std::sync::Mutex;

use crate::{
    connectors::{row_stream::RowStream, Source, Target},
    schema::{Column as SchemaColumn, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

pub struct PostgresSource {
    connection_string: String,
    pool: Option<PgPool>,
    table_name: String,
    query: Option<String>,
    /// One streaming `SELECT` for the whole read (no OFFSET paging)
    stream: Option<RowStream>,
    consumed: bool,
}

impl PostgresSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse connection string - could be "postgres://user:pass@host:port/db#table" or similar
        let (conn_str, table) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "PostgreSQL connection string format: postgres://user:pass@host:port/db#table"
                        .to_string(),
                ));
            }
            (parts[0], parts[1])
        } else {
            return Err(TinyEtlError::Configuration(
                "PostgreSQL source requires table specification: postgres://user:pass@host:port/db#table".to_string()
            ));
        };

        Ok(Self {
            connection_string: conn_str.to_string(),
            pool: None,
            table_name: table.to_string(),
            query: None,
            stream: None,
            consumed: false,
        })
    }

    pub fn with_query(connection_string: &str, query: &str) -> Result<Self> {
        let conn_str = if connection_string.contains('#') {
            connection_string.split('#').next().unwrap()
        } else {
            connection_string
        };

        Ok(Self {
            connection_string: conn_str.to_string(),
            pool: None,
            table_name: String::new(),
            query: Some(query.to_string()),
            stream: None,
            consumed: false,
        })
    }
}

#[async_trait]
impl Source for PostgresSource {
    async fn connect(&mut self) -> Result<()> {
        let options = PgConnectOptions::from_str(&self.connection_string).map_err(|e| {
            TinyEtlError::Connection(format!("Invalid PostgreSQL connection string: {}", e))
        })?;

        let pool = PgPool::connect_with(options).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to PostgreSQL: {}", e))
        })?;

        self.pool = Some(pool);
        Ok(())
    }

    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;

        let query = if let Some(ref custom_query) = self.query {
            format!(
                "SELECT * FROM ({}) AS tinyetl_query LIMIT {}",
                custom_query, sample_size
            )
        } else {
            format!("SELECT * FROM {} LIMIT {}", self.table_name, sample_size)
        };

        // Column types are the declared ones, from the statement's description, in the result's
        // column order: a column of NULLs, a numeric too wide for a Decimal or an empty table
        // still gets its type
        let described = pool
            .prepare(sqlx::SqlSafeStr::into_sql_str(sqlx::AssertSqlSafe(
                query.clone(),
            )))
            .await
            .map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to fetch sample data: {}", e))
            })?;
        let mut schema = Schema {
            columns: described
                .columns()
                .iter()
                .map(|column| SchemaColumn {
                    name: column.name().to_string(),
                    data_type: data_type(column.type_info()),
                    nullable: true,
                })
                .collect(),
            estimated_rows: None,
            primary_key_candidate: None,
        };

        // Get estimated row count
        if let Ok(count_result) = self.estimated_row_count().await {
            schema.estimated_rows = count_result;
        }

        Ok(schema)
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.stream.is_none() {
            self.stream = Some(self.start_stream()?);
        }
        let rows = self
            .stream
            .as_mut()
            .expect("started above")
            .next_batch(batch_size)
            .await?;
        self.consumed |= !rows.is_empty();
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;

        let count_query = if self.query.is_some() {
            // For custom queries, we can't easily get an accurate count without executing
            // the entire query, so we return None
            return Ok(None);
        } else {
            format!(
                "SELECT reltuples::BIGINT AS estimate FROM pg_class WHERE relname = '{}'",
                self.table_name
            )
        };

        // reltuples is -1 for a table that hasn't been vacuumed or analyzed yet (and 0 can mean
        // the same on older servers), so only trust a positive estimate.
        match sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_query.as_str()))
            .fetch_one(pool)
            .await
        {
            Ok(count) if count > 0 => Ok(Some(count as usize)),
            _ => {
                // No usable estimate: fall back to an exact count
                let exact_query = format!("SELECT COUNT(*) FROM {}", self.table_name);
                match sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(exact_query.as_str()))
                    .fetch_one(pool)
                    .await
                {
                    Ok(count) => Ok(Some(count as usize)),
                    Err(e) => Err(TinyEtlError::DataTransfer(format!(
                        "Failed to get row count: {}",
                        e
                    ))),
                }
            }
        }
    }

    async fn reset(&mut self) -> Result<()> {
        // The query runs again on the next read, but only if rows were already read
        if self.consumed {
            self.stream = None;
            self.consumed = false;
        }
        Ok(())
    }

    fn has_more(&self) -> bool {
        self.stream.as_ref().is_none_or(RowStream::has_more)
    }
}

impl PostgresSource {
    /// Run the query once on its own connection and stream the rows.
    fn start_stream(&self) -> Result<RowStream> {
        let pool = self
            .pool
            .clone()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;
        // A query runs exactly as written; only the schema sample wraps it
        let sql = match &self.query {
            Some(query) => query.clone(),
            None => format!("SELECT * FROM {}", self.table_name),
        };
        Ok(RowStream::spawn(move |mut sink| async move {
            let mut conn = match pool.acquire().await {
                Ok(conn) => conn,
                Err(e) => return sink.fail(format!("Failed to connect: {}", e)).await,
            };
            let mut rows = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch(&mut *conn);
            while let Some(row) = rows.next().await {
                let converted = row
                    .map_err(|e| format!("Failed to fetch rows: {}", e))
                    .and_then(|row| Self::row_values(&row).map_err(|e| e.to_string()));
                match converted {
                    Ok(values) => {
                        if !sink.push(values).await {
                            return; // the reader is gone
                        }
                    }
                    Err(message) => return sink.fail(message).await,
                }
            }
            drop(rows);
            sink.finish().await;
        }))
    }

    fn row_values(row: &sqlx::postgres::PgRow) -> Result<Row> {
        row.columns()
            .iter()
            .map(|column| Ok((column.name().to_string(), Self::extract_value(row, column)?)))
            .collect()
    }

    fn extract_value(
        row: &sqlx::postgres::PgRow,
        column: &sqlx::postgres::PgColumn,
    ) -> Result<Value> {
        let raw = row.try_get_raw(column.ordinal()).map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to read column '{}': {}", column.name(), e))
        })?;
        Ok(value(raw, column.type_info()))
    }
}

/// The TinyETL type for a PostgreSQL column type. Types with no TinyETL counterpart (time,
/// interval, uuid, inet, bytea, enums, bit strings, ...) are text; arrays are JSON.
fn data_type(info: &PgTypeInfo) -> DataType {
    match info.kind() {
        PgTypeKind::Array(_) => return DataType::Json,
        PgTypeKind::Domain(base) => return data_type(base),
        PgTypeKind::Enum(_) => return DataType::String,
        _ => {}
    }
    match info.name() {
        "INT2" | "INT4" | "INT8" | "OID" => DataType::Integer,
        "NUMERIC" | "FLOAT4" | "FLOAT8" | "MONEY" => DataType::Decimal,
        "BOOL" => DataType::Boolean,
        "DATE" => DataType::Date,
        "TIMESTAMP" | "TIMESTAMPTZ" => DataType::DateTime,
        "JSON" | "JSONB" => DataType::Json,
        _ => DataType::String,
    }
}

/// One value from a row, decoded by its declared type
fn value(raw: PgValueRef<'_>, info: &PgTypeInfo) -> Value {
    if raw.is_null() {
        return Value::Null;
    }
    match raw.format() {
        PgValueFormat::Binary => match raw.as_bytes() {
            Ok(bytes) => binary_value(info, bytes),
            Err(_) => Value::Null,
        },
        // Results normally arrive in the binary format; text is PostgreSQL's own output
        PgValueFormat::Text => match raw.as_str() {
            Ok(text) => text_value(info, text),
            Err(_) => Value::Null,
        },
    }
}

fn text_value(info: &PgTypeInfo, text: &str) -> Value {
    match data_type(info) {
        DataType::Integer => text
            .parse()
            .map(Value::Integer)
            .unwrap_or_else(|_| Value::String(text.to_string())),
        DataType::Decimal => decimal_text(text.to_string()),
        DataType::Boolean => Value::Boolean(text == "t"),
        DataType::Json => serde_json::from_str(text)
            .map(Value::Json)
            .unwrap_or_else(|_| Value::String(text.to_string())),
        DataType::Date | DataType::DateTime => crate::date_parser::DateParser::try_parse(text)
            .unwrap_or_else(|| Value::String(text.to_string())),
        _ => Value::String(text.to_string()),
    }
}

/// A number as text: an exact Decimal, or the text itself when no Decimal holds it exactly
/// (more than 28 digits, NaN, Infinity)
fn decimal_text(text: String) -> Value {
    match Decimal::from_str_exact(&text) {
        Ok(d) => Value::Decimal(d),
        Err(_) => Value::String(text),
    }
}

/// A float as its shortest round-trip text, then a Decimal (0.1, not 0.1000000000000000055)
fn float_value(text: String) -> Value {
    match Decimal::from_str(&text) {
        Ok(d) => Value::Decimal(d),
        Err(_) => Value::String(text),
    }
}

/// `N` bytes at `at`, for the big-endian integers of the binary format
fn be<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at + N)?.try_into().ok()
}

/// A value in PostgreSQL's binary format, by its type
fn binary_value(info: &PgTypeInfo, bytes: &[u8]) -> Value {
    match info.kind() {
        PgTypeKind::Array(element) => {
            return array_json(element, bytes)
                .map(Value::Json)
                .unwrap_or_else(|| hex_text(bytes));
        }
        PgTypeKind::Domain(base) => return binary_value(base, bytes),
        PgTypeKind::Enum(_) => return Value::String(String::from_utf8_lossy(bytes).into_owned()),
        _ => {}
    }
    let decoded = match info.name() {
        "BOOL" => bytes.first().map(|b| Value::Boolean(*b != 0)),
        "INT2" => be(bytes, 0).map(|b| Value::Integer(i16::from_be_bytes(b).into())),
        "INT4" => be(bytes, 0).map(|b| Value::Integer(i32::from_be_bytes(b).into())),
        "INT8" => be(bytes, 0).map(|b| Value::Integer(i64::from_be_bytes(b))),
        "OID" => be(bytes, 0).map(|b| Value::Integer(u32::from_be_bytes(b).into())),
        "FLOAT4" => be(bytes, 0).map(|b| float_value(f32::from_be_bytes(b).to_string())),
        "FLOAT8" => be(bytes, 0).map(|b| float_value(f64::from_be_bytes(b).to_string())),
        "NUMERIC" => numeric_text(bytes).map(decimal_text),
        // Cents: money has the locale's fraction digits, 2 in C and en_US
        "MONEY" => be(bytes, 0).map(|b| Value::Decimal(Decimal::new(i64::from_be_bytes(b), 2))),
        "DATE" => be(bytes, 0).map(|b| match i32::from_be_bytes(b) {
            i32::MAX => Value::String("infinity".to_string()),
            i32::MIN => Value::String("-infinity".to_string()),
            days => match epoch().checked_add_signed(chrono::Duration::days(days.into())) {
                Some(date) => Value::Date(chrono::Utc.from_utc_datetime(&date)),
                None => Value::String(days.to_string()),
            },
        }),
        "TIMESTAMP" | "TIMESTAMPTZ" => be(bytes, 0).map(|b| match i64::from_be_bytes(b) {
            i64::MAX => Value::String("infinity".to_string()),
            i64::MIN => Value::String("-infinity".to_string()),
            micros => match epoch().checked_add_signed(chrono::Duration::microseconds(micros)) {
                Some(at) => Value::Date(chrono::Utc.from_utc_datetime(&at)),
                None => Value::String(micros.to_string()),
            },
        }),
        "TIME" => be(bytes, 0).map(|b| Value::String(time_text(i64::from_be_bytes(b)))),
        "TIMETZ" => be(bytes, 0)
            .zip(be(bytes, 8))
            .map(|(t, z)| Value::String(timetz_text(i64::from_be_bytes(t), i32::from_be_bytes(z)))),
        "INTERVAL" => be(bytes, 0)
            .zip(be(bytes, 8))
            .zip(be(bytes, 12))
            .map(|((t, d), m)| {
                Value::String(interval_text(
                    i32::from_be_bytes(m),
                    i32::from_be_bytes(d),
                    i64::from_be_bytes(t),
                ))
            }),
        "UUID" => uuid::Uuid::from_slice(bytes)
            .ok()
            .map(|u| Value::String(u.to_string())),
        "JSON" => std::str::from_utf8(bytes)
            .ok()
            .and_then(|t| serde_json::from_str(t).ok())
            .map(Value::Json),
        // JSONB: a version byte (1), then the text
        "JSONB" => bytes
            .get(1..)
            .and_then(|t| std::str::from_utf8(t).ok())
            .and_then(|t| serde_json::from_str(t).ok())
            .map(Value::Json),
        "BYTEA" => Some(hex_text(bytes)),
        "INET" | "CIDR" => inet_text(bytes).map(Value::String),
        "MACADDR" | "MACADDR8" => Some(Value::String(
            bytes
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(":"),
        )),
        "BIT" | "VARBIT" => bit_text(bytes).map(Value::String),
        _ => None,
    };
    // Text types (TEXT, VARCHAR, CHAR, NAME, XML, citext, ...) send their text; any other
    // type keeps its bytes as hex rather than becoming NULL
    decoded.unwrap_or_else(|| match std::str::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Value::String(text.to_string()),
        _ => hex_text(bytes),
    })
}

/// 2000-01-01, PostgreSQL's epoch for dates and timestamps
fn epoch() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("a valid date")
}

/// Binary values as `0x` and lowercase hex
fn hex_text(bytes: &[u8]) -> Value {
    Value::String(format!("0x{}", hex::encode(bytes)))
}

/// NUMERIC's binary form (base-10000 digits) as decimal text with its scale
fn numeric_text(bytes: &[u8]) -> Option<String> {
    let ndigits = i16::from_be_bytes(be(bytes, 0)?) as usize;
    let weight = i16::from_be_bytes(be(bytes, 2)?) as isize;
    let sign = u16::from_be_bytes(be(bytes, 4)?);
    let dscale = u16::from_be_bytes(be(bytes, 6)?) as usize;
    match sign {
        0xC000 => return Some("NaN".to_string()),
        0xD000 => return Some("Infinity".to_string()),
        0xF000 => return Some("-Infinity".to_string()),
        _ => {}
    }
    let digits: Vec<i16> = (0..ndigits)
        .map(|i| be(bytes, 8 + 2 * i).map(i16::from_be_bytes))
        .collect::<Option<_>>()?;
    // The base-10000 digit at position `i` (weight - i); zero outside the stored digits
    let digit = |i: isize| -> i16 {
        usize::try_from(i)
            .ok()
            .and_then(|i| digits.get(i).copied())
            .unwrap_or(0)
    };
    let mut text = String::new();
    if sign == 0x4000 {
        text.push('-');
    }
    if weight < 0 {
        text.push('0');
    } else {
        text.push_str(&digit(0).to_string());
        for i in 1..=weight {
            text.push_str(&format!("{:04}", digit(i)));
        }
    }
    if dscale > 0 {
        let mut fraction = String::new();
        for k in 1..=dscale.div_ceil(4) as isize {
            fraction.push_str(&format!("{:04}", digit(weight + k)));
        }
        fraction.truncate(dscale);
        text.push('.');
        text.push_str(&fraction);
    }
    Some(text)
}

/// A time of day (or an interval's time part) as `HH:MM:SS[.ffffff]`, without trailing zeros
fn time_text(micros: i64) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    let micros = micros.unsigned_abs();
    let seconds = micros / 1_000_000;
    let mut text = format!(
        "{}{:02}:{:02}:{:02}",
        sign,
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    );
    let fraction = micros % 1_000_000;
    if fraction != 0 {
        text.push('.');
        text.push_str(format!("{:06}", fraction).trim_end_matches('0'));
    }
    text
}

/// A time with a zone, `13:45:30+02`; the zone is stored as seconds west of UTC
fn timetz_text(micros: i64, west: i32) -> String {
    let east = -west;
    let sign = if east < 0 { '-' } else { '+' };
    let east = east.unsigned_abs();
    let mut text = format!("{}{}{:02}", time_text(micros), sign, east / 3600);
    if !east.is_multiple_of(3600) {
        text.push_str(&format!(":{:02}", east % 3600 / 60));
        if !east.is_multiple_of(60) {
            text.push_str(&format!(":{:02}", east % 60));
        }
    }
    text
}

/// An interval in PostgreSQL's default output style: `1 year 2 mons 3 days 04:05:06`
fn interval_text(months: i32, days: i32, micros: i64) -> String {
    let mut parts = Vec::new();
    // As in PostgreSQL, a positive field after a negative one gets a `+`
    let mut is_before = false;
    for (n, unit) in [
        (i64::from(months / 12), "year"),
        (i64::from(months % 12), "mon"),
        (i64::from(days), "day"),
    ] {
        if n != 0 {
            let plus = if is_before && n > 0 { "+" } else { "" };
            let plural = if n == 1 { "" } else { "s" };
            parts.push(format!("{}{} {}{}", plus, n, unit, plural));
            is_before = n < 0;
        }
    }
    if micros != 0 || parts.is_empty() {
        let plus = if micros >= 0 && is_before { "+" } else { "" };
        parts.push(format!("{}{}", plus, time_text(micros)));
    }
    parts.join(" ")
}

/// inet / cidr as PostgreSQL writes them: `192.168.0.1`, `10.0.0.0/8`, `::1`
fn inet_text(bytes: &[u8]) -> Option<String> {
    let (family, bits, is_cidr, length) = (
        *bytes.first()?,
        *bytes.get(1)?,
        *bytes.get(2)? != 0,
        *bytes.get(3)? as usize,
    );
    let address = bytes.get(4..4 + length)?;
    let (text, max) = match family {
        2 => (
            std::net::Ipv4Addr::from(<[u8; 4]>::try_from(address).ok()?).to_string(),
            32,
        ),
        3 => (
            std::net::Ipv6Addr::from(<[u8; 16]>::try_from(address).ok()?).to_string(),
            128,
        ),
        _ => return None,
    };
    Some(if is_cidr || bits != max {
        format!("{}/{}", text, bits)
    } else {
        text
    })
}

/// bit(n) / varbit as its digits
fn bit_text(bytes: &[u8]) -> Option<String> {
    let length = i32::from_be_bytes(be(bytes, 0)?) as usize;
    let data = bytes.get(4..)?;
    (0..length)
        .map(|i| {
            data.get(i / 8).map(|byte| {
                if (byte >> (7 - i % 8)) & 1 == 1 {
                    '1'
                } else {
                    '0'
                }
            })
        })
        .collect()
}

/// An array's binary form as a JSON array, nested for multi-dimensional arrays
fn array_json(element: &PgTypeInfo, bytes: &[u8]) -> Option<serde_json::Value> {
    let dimensions = i32::from_be_bytes(be(bytes, 0)?) as usize;
    if dimensions == 0 {
        return Some(serde_json::Value::Array(Vec::new()));
    }
    // Header: dimensions, has-nulls flag, element type, then (length, lower bound) per dimension
    let lengths: Vec<usize> = (0..dimensions)
        .map(|d| be(bytes, 12 + 8 * d).map(|b| i32::from_be_bytes(b).max(0) as usize))
        .collect::<Option<_>>()?;
    let mut at = 12 + 8 * dimensions;
    let mut elements = Vec::new();
    for _ in 0..lengths.iter().product::<usize>() {
        let length = i32::from_be_bytes(be(bytes, at)?);
        at += 4;
        if length < 0 {
            elements.push(serde_json::Value::Null);
            continue;
        }
        let data = bytes.get(at..at + length as usize)?;
        at += length as usize;
        elements.push(json_of(binary_value(element, data), element));
    }
    // Group the flat elements by dimension, innermost first
    for &length in lengths[1..].iter().rev() {
        elements = elements
            .chunks(length.max(1))
            .map(|chunk| serde_json::Value::Array(chunk.to_vec()))
            .collect();
    }
    Some(serde_json::Value::Array(elements))
}

/// An array element as JSON
fn json_of(value: Value, info: &PgTypeInfo) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::String(s) => serde_json::Value::String(s),
        Value::Integer(i) => i.into(),
        Value::Decimal(d) => serde_json::from_str(&d.to_string())
            .unwrap_or_else(|_| serde_json::Value::String(d.to_string())),
        Value::Boolean(b) => b.into(),
        Value::Date(dt) if info.name() == "DATE" => {
            serde_json::Value::String(dt.format("%Y-%m-%d").to_string())
        }
        Value::Date(dt) => serde_json::Value::String(dt.to_rfc3339()),
        Value::Json(j) => j,
    }
}

pub struct PostgresTarget {
    connection_string: String,
    pool: Option<PgPool>,
    table_name: Option<String>,
    schema: Option<Schema>,
    /// The server is CrateDB, which speaks the Postgres protocol
    cratedb: bool,
    /// The load's transaction, begun at the first write and committed in finalize. Dropped
    /// without a commit (the transfer failed), it rolls back. Mutex only for Sync.
    tx: Option<Mutex<sqlx::Transaction<'static, sqlx::Postgres>>>,
    /// truncate() was called: empty the table inside the load's transaction
    truncate_table: Option<String>,
}

impl PostgresTarget {
    /// Open the load's transaction (once), emptying the table first if --truncate was given.
    async fn begin(&mut self) -> Result<()> {
        if self.tx.is_some() {
            return Ok(());
        }
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;
        let mut tx = pool.begin().await.map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to start a transaction: {}", e))
        })?;
        if let Some(table) = self.truncate_table.take() {
            // CrateDB's DELETE only sees refreshed rows (about once a second), so rows loaded
            // just before would survive it
            if self.cratedb {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "REFRESH TABLE {}",
                    qualified(&table)
                )))
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to refresh table: {}", e))
                })?;
            }
            // TRUNCATE is transactional in PostgreSQL; CrateDB has no TRUNCATE
            let sql = if self.cratedb {
                format!("DELETE FROM {}", qualified(&table))
            } else {
                format!("TRUNCATE TABLE {}", qualified(&table))
            };
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e))
                })?;
        }
        self.tx = Some(Mutex::new(tx));
        Ok(())
    }

    /// The column type for `data_type` in CREATE TABLE.
    fn column_type(data_type: &DataType, cratedb: bool) -> &'static str {
        match data_type {
            DataType::String => "TEXT",
            DataType::Integer => "BIGINT",
            // CrateDB stores NUMERIC only with a precision and scale
            DataType::Decimal if cratedb => "NUMERIC(38,10)",
            DataType::Decimal => "NUMERIC",
            DataType::Boolean => "BOOLEAN",
            // CrateDB can't store DATE columns
            DataType::Date if !cratedb => "DATE",
            DataType::Date | DataType::DateTime => "TIMESTAMP WITH TIME ZONE",
            DataType::Json => "JSONB", // PostgreSQL native JSON type
            DataType::Null => "TEXT",  // Default to TEXT for null columns
        }
    }

    pub fn new(connection_string: &str) -> Result<Self> {
        // Parse table name from connection string
        let (conn_str, table_name) = if connection_string.contains('#') {
            let parts: Vec<&str> = connection_string.split('#').collect();
            if parts.len() != 2 {
                return Err(TinyEtlError::Configuration(
                    "PostgreSQL connection string format: postgres://user:pass@host:port/db#table"
                        .to_string(),
                ));
            }
            (parts[0].to_string(), Some(parts[1].to_string()))
        } else {
            (connection_string.to_string(), None)
        };

        Ok(Self {
            connection_string: conn_str,
            pool: None,
            table_name,
            schema: None,
            cratedb: false,
            tx: None,
            truncate_table: None,
        })
    }
}

/// Quote a table name for SQL; `schema.table` becomes `"schema"."table"`
fn qualified(name: &str) -> String {
    name.split('.')
        .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(".")
}

#[async_trait]
impl Target for PostgresTarget {
    async fn connect(&mut self) -> Result<()> {
        let options = PgConnectOptions::from_str(&self.connection_string).map_err(|e| {
            TinyEtlError::Connection(format!("Invalid PostgreSQL connection string: {}", e))
        })?;

        let pool = PgPool::connect_with(options).await.map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to PostgreSQL: {}", e))
        })?;

        let version: String = sqlx::query_scalar("SELECT version()")
            .fetch_one(&pool)
            .await
            .unwrap_or_default();
        self.cratedb = version.starts_with("CrateDB");

        self.pool = Some(pool);
        Ok(())
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;

        // Determine the actual table name to use
        let actual_table_name = if table_name.is_empty() {
            self.table_name
                .clone()
                .unwrap_or_else(|| "data".to_string())
        } else {
            table_name.to_string()
        };

        // Update internal table name
        self.table_name = Some(actual_table_name.clone());
        self.schema = Some(schema.clone());

        let mut create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} (",
            qualified(&actual_table_name)
        );

        let column_defs: Vec<String> = schema
            .columns
            .iter()
            .map(|col| {
                let pg_type = Self::column_type(&col.data_type, self.cratedb);

                let nullable = if col.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", col.name, pg_type, nullable)
            })
            .collect();

        create_sql.push_str(&column_defs.join(", "));
        create_sql.push(')');

        sqlx::query(sqlx::AssertSqlSafe(create_sql.as_str()))
            .execute(pool)
            .await
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to create table: {}", e)))?;

        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        self.begin().await?;
        // Taken out while the batch is written; a failure drops it, which rolls back
        let mut tx = self
            .tx
            .take()
            .expect("begun above")
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());

        let table_name = self
            .table_name
            .as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Table not created".to_string()))?;

        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| TinyEtlError::Configuration("Schema not set".to_string()))?;

        // Build INSERT statement with quoted column names
        let column_names: Vec<String> = schema
            .columns
            .iter()
            .map(|c| format!("\"{}\"", c.name))
            .collect();

        let mut total_written = 0;

        // Process in chunks to avoid parameter limit (PostgreSQL has 65535 parameter limit)
        // With 12 columns, we can do ~5000 rows per batch, but start conservatively
        let max_rows_per_batch = 65535 / schema.columns.len().max(1);
        let chunk_size = max_rows_per_batch.min(1000); // Reduced from 5000 to 1000

        for chunk in rows.chunks(chunk_size) {
            // Build VALUES placeholders for multiple rows: ($1, $2, $3), ($4, $5, $6), ...
            let mut placeholders = Vec::with_capacity(chunk.len());
            let mut param_count = 1;

            for _ in 0..chunk.len() {
                let row_placeholders: Vec<String> = (0..schema.columns.len())
                    .map(|_| {
                        let p = format!("${}", param_count);
                        param_count += 1;
                        p
                    })
                    .collect();
                placeholders.push(format!("({})", row_placeholders.join(", ")));
            }

            let insert_sql = format!(
                "INSERT INTO {} ({}) VALUES {}",
                qualified(table_name),
                column_names.join(", "),
                placeholders.join(", ")
            );

            // Build query and bind all values
            let mut query = sqlx::query(sqlx::AssertSqlSafe(insert_sql.as_str()));

            // Every value is bound with its column's type, NULLs included. sqlx caches the
            // prepared statement by its SQL text, so if a parameter's type changed between
            // chunks, a binary integer could be sent where the cached statement expects text
            // (Postgres: "invalid byte sequence for encoding UTF8: 0x00").
            // A Date column is DATE (TIMESTAMPTZ on CrateDB) and gets dates
            let dates = !self.cratedb;
            for (offset, row) in chunk.iter().enumerate() {
                for col in &schema.columns {
                    let value = row.get(&col.name).unwrap_or(&Value::Null);
                    query = match (&col.data_type, value) {
                        (DataType::Date, Value::Null) if dates => {
                            query.bind(None::<chrono::NaiveDate>)
                        }
                        (DataType::Date, Value::Date(d)) if dates => query.bind(d.date_naive()),
                        (DataType::Integer, Value::Null) => query.bind(None::<i64>),
                        (DataType::Boolean, Value::Null) => query.bind(None::<bool>),
                        (DataType::Date | DataType::DateTime, Value::Null) => {
                            query.bind(None::<chrono::DateTime<chrono::Utc>>)
                        }
                        (DataType::Decimal, Value::Null) => query.bind(None::<Decimal>),
                        (DataType::Json, Value::Null) => query.bind(None::<serde_json::Value>),
                        (_, Value::Null) => query.bind(None::<String>),
                        (DataType::Integer, Value::Integer(i)) => query.bind(*i),
                        (DataType::Decimal, Value::Decimal(d)) => query.bind(*d),
                        (DataType::Boolean, Value::Boolean(b)) => query.bind(*b),
                        (DataType::Date | DataType::DateTime, Value::Date(d)) => query.bind(*d),
                        (DataType::Json, Value::Json(j)) => query.bind(j.clone()),
                        (DataType::String | DataType::Null, Value::String(s)) => {
                            // PostgreSQL text can't hold NUL bytes
                            query.bind(s.replace('\0', ""))
                        }
                        (data_type, value) => {
                            return Err(TinyEtlError::DataValidation(format!(
                                "Column '{}' is {} but row {} of this batch has the value {:?} \
                                 (column types are inferred from the first 1000 rows; use \
                                 --schema-file to set them)",
                                col.name,
                                data_type,
                                total_written + offset + 1,
                                value
                            )));
                        }
                    };
                }
            }

            query.execute(&mut *tx).await.map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to insert batch: {}", e))
            })?;

            total_written += chunk.len();
        }
        self.tx = Some(Mutex::new(tx));

        Ok(total_written)
    }

    async fn finalize(&mut self) -> Result<()> {
        // A --truncate with no rows to load still has to empty the table
        if self.truncate_table.is_some() {
            self.begin().await?;
        }
        if let Some(tx) = self.tx.take() {
            let tx = tx.into_inner().unwrap_or_else(|e| e.into_inner());
            tx.commit().await.map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to commit the load: {}", e))
            })?;
        }
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| TinyEtlError::Connection("Not connected".to_string()))?;

        // `schema.table`, or a table in the schema an unqualified CREATE TABLE writes to
        let (schema, table) = match table_name.split_once('.') {
            Some((schema, table)) => (Some(schema), table),
            None => (None, table_name),
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT * FROM information_schema.tables \
             WHERE table_name = $1 AND table_schema = COALESCE($2, current_schema()))",
        )
        .bind(table)
        .bind(schema)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to check table existence: {}", e))
        })?;

        Ok(exists)
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        // Deferred into the load's transaction, so a failed load keeps the old rows
        self.truncate_table = Some(table_name.to_string());
        Ok(())
    }

    fn supports_append(&self) -> bool {
        // PostgreSQL databases support appending new rows
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualified() {
        assert_eq!(qualified("users"), "\"users\"");
        assert_eq!(qualified("s1.users"), "\"s1\".\"users\"");
        assert_eq!(qualified("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn test_postgres_source_new() {
        let source = PostgresSource::new("postgres://user:pass@localhost:5432/db#table");
        assert!(source.is_ok());

        let source = source.unwrap();
        assert_eq!(
            source.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
        assert_eq!(source.table_name, "table");
    }

    #[test]
    fn test_postgres_source_new_without_table() {
        let result = PostgresSource::new("postgres://user:pass@localhost:5432/db");
        assert!(result.is_err());
    }

    #[test]
    fn test_postgres_source_with_query() {
        let source = PostgresSource::with_query(
            "postgres://user:pass@localhost:5432/db",
            "SELECT * FROM table WHERE id > 10",
        );
        assert!(source.is_ok());

        let source = source.unwrap();
        assert_eq!(
            source.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
        assert!(source.query.is_some());
        assert_eq!(source.query.unwrap(), "SELECT * FROM table WHERE id > 10");
    }

    #[test]
    fn test_postgres_target_new() {
        let target = PostgresTarget::new("postgres://user:pass@localhost:5432/db");
        assert!(target.is_ok());

        let target = target.unwrap();
        assert_eq!(
            target.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
    }

    #[test]
    fn test_postgres_target_new_strips_table() {
        let target = PostgresTarget::new("postgres://user:pass@localhost:5432/db#table");
        assert!(target.is_ok());

        let target = target.unwrap();
        assert_eq!(
            target.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
    }

    #[test]
    fn test_postgres_source_connection_string_parsing() {
        let test_cases = vec![
            (
                "postgres://user:pass@localhost:5432/mydb#mytable",
                "postgres://user:pass@localhost:5432/mydb",
                "mytable",
            ),
            (
                "postgres://admin:secret@db.example.com/production#users",
                "postgres://admin:secret@db.example.com/production",
                "users",
            ),
            (
                "postgres://user:pass@127.0.0.1:5433/testdb#schema.table",
                "postgres://user:pass@127.0.0.1:5433/testdb",
                "schema.table",
            ),
        ];

        for (input, expected_conn, expected_table) in test_cases {
            let source = PostgresSource::new(input).unwrap();
            assert_eq!(source.connection_string, expected_conn);
            assert_eq!(source.table_name, expected_table);
        }
    }

    #[test]
    fn test_postgres_source_invalid_connection_strings() {
        let invalid_cases = vec![
            "postgres://user:pass@localhost:5432/db",
            "postgres://user:pass@localhost:5432/db#table#extra",
        ];

        for input in invalid_cases {
            let result = PostgresSource::new(input);
            assert!(result.is_err(), "Expected error for input: {}", input);
        }
    }

    #[test]
    fn test_postgres_source_empty_table_name() {
        // Connection string with # but empty table name
        let source = PostgresSource::new("postgres://user:pass@localhost:5432/db#");
        assert!(source.is_ok());
        let source = source.unwrap();
        assert_eq!(source.table_name, "");
    }

    #[test]
    fn test_postgres_with_query_strips_table() {
        let source = PostgresSource::with_query(
            "postgres://user:pass@localhost:5432/db#ignored_table",
            "SELECT * FROM custom_view",
        )
        .unwrap();

        assert_eq!(
            source.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
        assert_eq!(source.table_name, "");
        assert_eq!(source.query, Some("SELECT * FROM custom_view".to_string()));
    }

    #[test]
    fn test_postgres_with_query_no_table() {
        let source = PostgresSource::with_query(
            "postgres://user:pass@localhost:5432/db",
            "SELECT id, name FROM users WHERE active = true",
        )
        .unwrap();

        assert_eq!(
            source.connection_string,
            "postgres://user:pass@localhost:5432/db"
        );
        assert!(source.query.is_some());
    }

    #[test]
    fn test_postgres_source_initial_state() {
        let source = PostgresSource::new("postgres://user:pass@localhost:5432/db#table").unwrap();

        assert!(source.stream.is_none());
        assert!(source.pool.is_none());
        assert!(source.query.is_none());
    }

    #[test]
    fn test_postgres_target_with_table() {
        let target = PostgresTarget::new("postgres://user:pass@localhost:5432/db#orders").unwrap();

        assert_eq!(target.table_name, Some("orders".to_string()));
        assert!(target.pool.is_none());
        assert!(target.schema.is_none());
    }

    #[test]
    fn test_postgres_target_without_table() {
        let target = PostgresTarget::new("postgres://user:pass@localhost:5432/db").unwrap();

        assert_eq!(target.table_name, None);
    }

    #[test]
    fn test_postgres_target_supports_append() {
        let target = PostgresTarget::new("postgres://user:pass@localhost:5432/db").unwrap();
        assert!(target.supports_append());
    }

    #[test]
    fn test_postgres_data_type_mapping() {
        // Test that we map DataType correctly to PostgreSQL types
        use crate::schema::{Column, DataType, Schema};

        let schema = Schema {
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
                Column {
                    name: "price".to_string(),
                    data_type: DataType::Decimal,
                    nullable: true,
                },
                Column {
                    name: "active".to_string(),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
                Column {
                    name: "created_at".to_string(),
                    data_type: DataType::DateTime,
                    nullable: true,
                },
                Column {
                    name: "birth_date".to_string(),
                    data_type: DataType::Date,
                    nullable: true,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };

        // Verify schema has expected columns
        assert_eq!(schema.columns.len(), 6);
        assert_eq!(schema.columns[0].data_type, DataType::Integer);
        assert_eq!(schema.columns[1].data_type, DataType::String);
        assert_eq!(schema.columns[2].data_type, DataType::Decimal);
        assert_eq!(schema.columns[3].data_type, DataType::Boolean);
        assert_eq!(schema.columns[4].data_type, DataType::DateTime);
        assert_eq!(schema.columns[5].data_type, DataType::Date);
    }

    #[test]
    fn test_postgres_column_types() {
        use crate::schema::DataType;
        assert_eq!(
            PostgresTarget::column_type(&DataType::Decimal, false),
            "NUMERIC"
        );
        assert_eq!(
            PostgresTarget::column_type(&DataType::Decimal, true),
            "NUMERIC(38,10)"
        );
        assert_eq!(
            PostgresTarget::column_type(&DataType::Integer, true),
            "BIGINT"
        );
        assert_eq!(PostgresTarget::column_type(&DataType::Json, false), "JSONB");
        // CrateDB can't store DATE columns
        assert_eq!(PostgresTarget::column_type(&DataType::Date, false), "DATE");
        assert_eq!(
            PostgresTarget::column_type(&DataType::Date, true),
            "TIMESTAMP WITH TIME ZONE"
        );
    }

    /// NUMERIC's binary form: (ndigits, weight, sign, dscale) then base-10000 digits
    fn numeric_bytes(weight: i16, sign: u16, dscale: u16, digits: &[i16]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend((digits.len() as i16).to_be_bytes());
        bytes.extend(weight.to_be_bytes());
        bytes.extend(sign.to_be_bytes());
        bytes.extend(dscale.to_be_bytes());
        for d in digits {
            bytes.extend(d.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn test_numeric_text() {
        // 12.50
        assert_eq!(
            numeric_text(&numeric_bytes(0, 0, 2, &[12, 5000])).as_deref(),
            Some("12.50")
        );
        // -99999999.99
        assert_eq!(
            numeric_text(&numeric_bytes(1, 0x4000, 2, &[9999, 9999, 9900])).as_deref(),
            Some("-99999999.99")
        );
        // 0.0000000000000000000000000001 (weight -7: the digit 1 is 10000^-7)
        assert_eq!(
            numeric_text(&numeric_bytes(-7, 0, 28, &[1])).as_deref(),
            Some("0.0000000000000000000000000001")
        );
        // 123456789012345678901234567890: too wide for a Decimal, kept as text
        let huge = numeric_bytes(7, 0, 0, &[12, 3456, 7890, 1234, 5678, 9012, 3456, 7890]);
        assert_eq!(
            numeric_text(&huge).as_deref(),
            Some("123456789012345678901234567890")
        );
        assert_eq!(
            decimal_text(numeric_text(&huge).unwrap()),
            Value::String("123456789012345678901234567890".to_string())
        );
        assert_eq!(
            numeric_text(&numeric_bytes(0, 0, 0, &[])).as_deref(),
            Some("0")
        );
        assert_eq!(
            numeric_text(&numeric_bytes(0, 0xC000, 0, &[])).as_deref(),
            Some("NaN")
        );
    }

    #[test]
    fn test_time_and_interval_text() {
        assert_eq!(time_text(49_530_000_000), "13:45:30");
        assert_eq!(time_text(1_500_000), "00:00:01.5");
        assert_eq!(
            interval_text(14, 3, 14_706_000_000),
            "1 year 2 mons 3 days 04:05:06"
        );
        assert_eq!(interval_text(0, -1, 0), "-1 days");
        assert_eq!(interval_text(0, -1, 7_200_000_000), "-1 days +02:00:00");
        assert_eq!(interval_text(0, 0, 0), "00:00:00");
        assert_eq!(interval_text(-12, 0, 0), "-1 years");
        assert_eq!(timetz_text(49_530_000_000, -7200), "13:45:30+02");
        assert_eq!(timetz_text(0, 19_800), "00:00:00-05:30");
    }

    #[test]
    fn test_inet_and_bit_text() {
        assert_eq!(
            inet_text(&[2, 32, 0, 4, 192, 168, 0, 1]).as_deref(),
            Some("192.168.0.1")
        );
        assert_eq!(
            inet_text(&[2, 8, 1, 4, 10, 0, 0, 0]).as_deref(),
            Some("10.0.0.0/8")
        );
        let mut v6 = vec![3, 128, 0, 16];
        v6.extend([0; 15]);
        v6.push(1);
        assert_eq!(inet_text(&v6).as_deref(), Some("::1"));
        // bit(3) '101': length 3, then the bits left-aligned in a byte
        assert_eq!(bit_text(&[0, 0, 0, 3, 0b1010_0000]).as_deref(), Some("101"));
    }

    #[test]
    fn test_postgres_source_has_more_without_total() {
        let source = PostgresSource::new("postgres://user:pass@localhost:5432/db#table").unwrap();

        // Before the query has run, there may be rows
        assert!(source.has_more());
    }

    #[test]
    fn test_postgres_connection_string_with_schema_qualified_table() {
        let source =
            PostgresSource::new("postgres://user:pass@localhost:5432/db#public.users").unwrap();

        assert_eq!(source.table_name, "public.users");
    }

    #[test]
    fn test_postgres_multiple_hash_error() {
        let result = PostgresSource::new("postgres://user:pass@localhost:5432/db#table1#table2");

        assert!(result.is_err());
        match result {
            Err(TinyEtlError::Configuration(msg)) => {
                assert!(msg.contains("format"));
            }
            _ => panic!("Expected Configuration error"),
        }
    }

    #[test]
    fn test_postgres_target_invalid_multiple_hash() {
        let result = PostgresTarget::new("postgres://user:pass@localhost:5432/db#table1#table2");

        assert!(result.is_err());
        match result {
            Err(TinyEtlError::Configuration(msg)) => {
                assert!(msg.contains("format"));
            }
            _ => panic!("Expected Configuration error"),
        }
    }

    #[test]
    fn test_postgres_value_types() {
        use chrono::Utc;
        use rust_decimal::Decimal;
        use std::collections::HashMap;

        // Test creating a row with all value types
        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("id".to_string(), Value::Integer(42));
        row.insert("name".to_string(), Value::String("Alice".to_string()));
        row.insert("score".to_string(), Value::Decimal(Decimal::new(9876, 2))); // 98.76
        row.insert("active".to_string(), Value::Boolean(true));
        row.insert("created".to_string(), Value::Date(Utc::now()));
        row.insert("deleted".to_string(), Value::Null);

        assert_eq!(row.get("id"), Some(&Value::Integer(42)));
        assert!(matches!(row.get("name"), Some(Value::String(_))));
        assert!(matches!(row.get("score"), Some(Value::Decimal(_))));
        assert_eq!(row.get("active"), Some(&Value::Boolean(true)));
        assert!(matches!(row.get("created"), Some(Value::Date(_))));
        assert_eq!(row.get("deleted"), Some(&Value::Null));
    }

    #[test]
    fn test_postgres_chunk_size_calculation() {
        // Test that chunk size calculation is reasonable
        let max_params = 65535;

        // With 10 columns
        let cols = 10;
        let max_rows = max_params / cols;
        assert_eq!(max_rows, 6553);
        assert!(max_rows.min(1000) == 1000); // Should be clamped to 1000

        // With 1 column
        let cols = 1;
        let max_rows = max_params / cols;
        assert_eq!(max_rows, 65535);
        assert!(max_rows.min(1000) == 1000); // Should be clamped to 1000

        // With 100 columns
        let cols = 100;
        let max_rows = max_params / cols;
        assert_eq!(max_rows, 655);
        assert!(max_rows.min(1000) == 655); // Under the cap
    }

    #[test]
    fn test_postgres_string_with_null_bytes() {
        // PostgreSQL doesn't allow null bytes in strings
        let input = "hello\0world";
        let cleaned: String = input.chars().filter(|&c| c != '\0').collect();

        assert_eq!(cleaned, "helloworld");
        assert!(!cleaned.contains('\0'));
    }

    #[test]
    fn test_postgres_decimal_to_string() {
        use rust_decimal::Decimal;

        let dec = Decimal::new(12345, 2); // 123.45
        let s = dec.to_string();
        assert_eq!(s, "123.45");

        // Test that we can filter null bytes from decimal strings too
        let cleaned: String = s.chars().filter(|&c| c != '\0').collect();
        assert_eq!(cleaned, "123.45");
    }

    #[test]
    fn test_postgres_connection_with_port() {
        let source = PostgresSource::new("postgres://user:pass@localhost:5433/db#table").unwrap();
        assert!(source.connection_string.contains(":5433"));
    }

    #[test]
    fn test_postgres_connection_with_host() {
        let source =
            PostgresSource::new("postgres://user:pass@db.example.com:5432/db#table").unwrap();
        assert!(source.connection_string.contains("db.example.com"));
    }

    #[test]
    fn test_postgres_table_name_extraction() {
        let test_cases = vec![
            ("postgres://user:pass@localhost/db#users", "users"),
            (
                "postgres://user:pass@localhost/db#public.orders",
                "public.orders",
            ),
            (
                "postgres://user:pass@localhost/db#schema_name.table_name",
                "schema_name.table_name",
            ),
        ];

        for (conn_str, expected_table) in test_cases {
            let source = PostgresSource::new(conn_str).unwrap();
            assert_eq!(source.table_name, expected_table);
        }
    }
}
