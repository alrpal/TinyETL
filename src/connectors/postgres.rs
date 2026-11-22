use async_trait::async_trait;
use chrono::TimeZone;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use sqlx::{postgres::PgConnectOptions, Column, PgPool, Row as SqlxRow};
use std::str::FromStr;
use std::sync::Mutex;

use crate::{
    connectors::{row_stream::RowStream, Source, Target},
    schema::{DataType, Row, Schema, SchemaInferer, Value},
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
                "SELECT * FROM ({}) AS subquery LIMIT {}",
                custom_query, sample_size
            )
        } else {
            format!("SELECT * FROM {} LIMIT {}", self.table_name, sample_size)
        };

        let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
            .fetch_all(pool)
            .await
            .map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to fetch sample data: {}", e))
            })?;

        if rows.is_empty() {
            return Ok(Schema {
                columns: Vec::new(),
                estimated_rows: Some(0),
                primary_key_candidate: None,
            });
        }

        // Convert PostgreSQL rows to our Row format
        let mut schema_rows = Vec::new();
        for row in &rows {
            let mut schema_row = Row::new();
            for column in row.columns() {
                let col_name = column.name();
                let value = Self::extract_value(row, column)?;
                schema_row.insert(col_name.to_string(), value);
            }
            schema_rows.push(schema_row);
        }

        // Keep the result set's column order; rows are hash maps and don't preserve it.
        let column_order: Vec<String> = rows[0]
            .columns()
            .iter()
            .map(|column| column.name().to_string())
            .collect();
        let mut schema = SchemaInferer::infer_from_rows_ordered(&schema_rows, &column_order)?;

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
        let sql = match &self.query {
            Some(query) => format!("SELECT * FROM ({}) AS subquery", query),
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
        let col_name = column.name();

        // Try different PostgreSQL types in order of likelihood
        if let Ok(val) = row.try_get::<Option<String>, _>(col_name) {
            match val {
                Some(s) => Ok(Value::String(s)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i64>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i32>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<i16>, _>(col_name) {
            match val {
                Some(i) => Ok(Value::Integer(i as i64)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<Decimal>, _>(col_name) {
            // NUMERIC / DECIMAL
            Ok(val.map_or(Value::Null, Value::Decimal))
        } else if let Ok(val) = row.try_get::<Option<f64>, _>(col_name) {
            match val {
                Some(f) => {
                    // Convert f64 to Decimal
                    match Decimal::try_from(f) {
                        Ok(d) => Ok(Value::Decimal(d)),
                        Err(_) => Ok(Value::String(f.to_string())),
                    }
                }
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<f32>, _>(col_name) {
            match val {
                Some(f) => {
                    // Convert f32 to Decimal via f64
                    match Decimal::try_from(f as f64) {
                        Ok(d) => Ok(Value::Decimal(d)),
                        Err(_) => Ok(Value::String(f.to_string())),
                    }
                }
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<bool>, _>(col_name) {
            match val {
                Some(b) => Ok(Value::Boolean(b)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(col_name) {
            match val {
                Some(dt) => Ok(Value::Date(dt)),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(col_name) {
            match val {
                Some(dt) => Ok(Value::Date(chrono::Utc.from_utc_datetime(&dt))),
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(col_name) {
            match val {
                Some(date) => {
                    let datetime = date.and_hms_opt(0, 0, 0).unwrap();
                    Ok(Value::Date(chrono::Utc.from_utc_datetime(&datetime)))
                }
                None => Ok(Value::Null),
            }
        } else if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(col_name) {
            // JSON / JSONB
            Ok(val.map_or(Value::Null, Value::Json))
        } else {
            Ok(Value::Null)
        }
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
            // TRUNCATE is transactional in PostgreSQL; CrateDB has no TRUNCATE
            let sql = if self.cratedb {
                format!("DELETE FROM \"{}\"", table)
            } else {
                format!("TRUNCATE TABLE \"{}\"", table)
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

        let mut create_sql = format!("CREATE TABLE IF NOT EXISTS \"{}\" (", actual_table_name);

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
                "INSERT INTO \"{}\" ({}) VALUES {}",
                table_name,
                column_names.join(", "),
                placeholders.join(", ")
            );

            // Build query and bind all values
            let mut query = sqlx::query(sqlx::AssertSqlSafe(insert_sql.as_str()));

            // Every value is bound with its column's type, NULLs included. sqlx caches the
            // prepared statement by its SQL text, so if a parameter's type changed between
            // chunks, a binary integer could be sent where the cached statement expects text
            // (Postgres: "invalid byte sequence for encoding UTF8: 0x00").
            for (offset, row) in chunk.iter().enumerate() {
                for col in &schema.columns {
                    let value = row.get(&col.name).unwrap_or(&Value::Null);
                    query = match (&col.data_type, value) {
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

        let exists_query =
            "SELECT EXISTS (SELECT * FROM information_schema.tables WHERE table_name = $1)";

        let exists: bool = sqlx::query_scalar(exists_query)
            .bind(table_name)
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
