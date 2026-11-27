mod arrow_values;
pub mod avro;
pub mod compressed;
pub mod csv;
#[cfg(test)]
mod db_contract;
pub mod duckdb;
pub mod excel;
pub mod json;
pub mod mssql;
pub mod mysql;
pub mod odbc;
mod output_file;
pub mod parquet;
pub mod postgres;
mod row_stream;
pub mod snowflake;
pub mod sqlite;

use crate::{
    schema::{Row, Schema},
    Result,
};
use async_trait::async_trait;

#[async_trait]
pub trait Source: Send + Sync {
    /// Connect to the source and validate it's accessible
    async fn connect(&mut self) -> Result<()>;

    /// Infer schema by reading a sample of data
    async fn infer_schema(&mut self, sample_size: usize) -> Result<Schema>;

    /// Read data in batches
    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>>;

    /// Get estimated total row count if available
    async fn estimated_row_count(&self) -> Result<Option<usize>>;

    /// Reset to beginning for re-reading
    async fn reset(&mut self) -> Result<()>;

    /// Check if there's more data to read
    fn has_more(&self) -> bool;

    /// Columns whose numeric dates read day first (`13/04/2024`), as found by `infer_schema`.
    /// Only text sources (CSV, JSON) have any; values in them are converted with that order.
    fn day_first_columns(&self) -> Vec<String> {
        Vec::new()
    }
}

#[async_trait]
pub trait Target: Send + Sync {
    /// Connect to the target and validate it's accessible
    async fn connect(&mut self) -> Result<()>;

    /// Create the target table/structure based on schema
    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()>;

    /// Write a batch of rows
    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize>;

    /// Finalize the write operation
    async fn finalize(&mut self) -> Result<()>;

    /// Check if target already exists
    async fn exists(&self, table_name: &str) -> Result<bool>;

    /// Truncate the target (remove all existing data)
    /// This should be called before writing if truncate mode is enabled
    async fn truncate(&mut self, table_name: &str) -> Result<()>;

    /// Check if the target supports appending data
    /// Returns true if data can be appended to existing target, false if it requires truncation
    fn supports_append(&self) -> bool;
}

/// Factory function to create a source connector from a connection string
/// For new protocol-based connections (snowflake://, etc.), use create_source_from_url instead
pub fn create_source(connection_string: &str) -> Result<Box<dyn Source>> {
    create_source_with_options(connection_string, &std::collections::HashMap::new())
}

/// Like [`create_source`], with endpoint options for the connectors that take them (CSV's
/// `delimiter`, Snowflake's key settings)
pub fn create_source_with_options(
    connection_string: &str,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Source>> {
    if !connection_string.contains("://") {
        if let Some((codec, inner)) = compressed::Codec::from_path(connection_string) {
            return Ok(Box::new(compressed::CompressedSource::new(
                connection_string,
                codec,
                inner,
                options,
            )?));
        }
    }
    if csv::is_csv(connection_string) {
        Ok(Box::new(csv::CsvSource::with_options(
            connection_string,
            options,
        )?))
    } else if connection_string.ends_with(".json") || json::is_json_lines(connection_string) {
        Ok(Box::new(json::JsonSource::new(connection_string)?))
    } else if connection_string.ends_with(".parquet") {
        Ok(Box::new(parquet::ParquetSource::new(connection_string)?))
    } else if connection_string.ends_with(".avro") {
        Ok(Box::new(avro::AvroSource::new(connection_string)?))
    } else if excel::is_spreadsheet(connection_string) {
        Ok(Box::new(excel::ExcelSource::new(connection_string)?))
    } else if (connection_string.contains(".duckdb#") || connection_string.ends_with(".duckdb"))
        || connection_string.starts_with("duckdb:")
    {
        Ok(Box::new(duckdb::DuckdbSource::new(connection_string)?))
    } else if (connection_string.contains(".db#") || connection_string.ends_with(".db"))
        || connection_string.starts_with("sqlite:")
    {
        Ok(Box::new(sqlite::SqliteSource::new(connection_string)?))
    } else if connection_string.starts_with("postgres://")
        || connection_string.starts_with("postgresql://")
    {
        Ok(Box::new(postgres::PostgresSource::new(connection_string)?))
    } else if connection_string.starts_with("mysql://") {
        Ok(Box::new(mysql::MysqlSource::new(connection_string)?))
    } else if connection_string.starts_with("mssql://")
        || connection_string.starts_with("sqlserver://")
    {
        Ok(Box::new(mssql::MssqlSource::new(connection_string)?))
    } else if connection_string.starts_with("odbc://") {
        Ok(Box::new(odbc::OdbcSource::new(connection_string)?))
    } else if connection_string.starts_with("snowflake://") {
        Ok(Box::new(snowflake::SnowflakeSource::new(
            connection_string,
            options,
        )?))
    } else {
        Err(crate::TinyEtlError::Configuration(
            format!("Unsupported source type: {}. Supported formats: file.csv, file.tsv, file.json, file.jsonl, file.parquet, file.avro, file.xlsx[#sheet] (also .xlsm, .xlsb, .xls, .ods), file.db#table, file.duckdb#table, postgres://user:pass@host:port/db#table, mysql://user:pass@host:port/db#table, mssql://user:pass@host:port/db#table, odbc://connection_string#table, snowflake://user@account/db/schema#table", connection_string)
        ))
    }
}

/// Factory function to create a target connector from a connection string
/// Supports both protocol-based and legacy file-based connections
pub fn create_target(connection_string: &str) -> Result<Box<dyn Target>> {
    create_target_with_options(connection_string, &std::collections::HashMap::new())
}

/// Like [`create_target`], with endpoint options for the connectors that take them
pub fn create_target_with_options(
    connection_string: &str,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Target>> {
    // Handle protocol-based connections first
    if connection_string.contains("://") {
        if connection_string.starts_with("duckdb://") {
            return Ok(Box::new(duckdb::DuckdbTarget::new(connection_string)?));
        } else if connection_string.starts_with("sqlite://") {
            return Ok(Box::new(sqlite::SqliteTarget::new(connection_string)?));
        } else if connection_string.starts_with("postgres://")
            || connection_string.starts_with("postgresql://")
        {
            return Ok(Box::new(postgres::PostgresTarget::new(connection_string)?));
        } else if connection_string.starts_with("mysql://") {
            return Ok(Box::new(mysql::MysqlTarget::new(connection_string)?));
        } else if connection_string.starts_with("mssql://")
            || connection_string.starts_with("sqlserver://")
        {
            return Ok(Box::new(mssql::MssqlTarget::new(connection_string)?));
        } else if connection_string.starts_with("odbc://") {
            return Ok(Box::new(odbc::OdbcTarget::new(connection_string)?));
        } else if connection_string.starts_with("snowflake://") {
            return Ok(Box::new(snowflake::SnowflakeTarget::new(
                connection_string,
                options,
            )?));
        } else {
            return Err(crate::TinyEtlError::Configuration(format!(
                "Unsupported protocol in: {}. Supported protocols: duckdb://, sqlite://, postgres://, mysql://, mssql://, odbc://, snowflake://",
                connection_string
            )));
        }
    }

    // Handle legacy file-based and non-protocol database connections
    if let Some((codec, inner)) = compressed::Codec::from_path(connection_string) {
        return Ok(Box::new(compressed::CompressedTarget::new(
            connection_string,
            codec,
            inner,
            options,
        )?));
    }
    if csv::is_csv(connection_string) {
        Ok(Box::new(csv::CsvTarget::with_options(
            connection_string,
            options,
        )?))
    } else if connection_string.ends_with(".json") || json::is_json_lines(connection_string) {
        Ok(Box::new(json::JsonTarget::new(connection_string)?))
    } else if connection_string.ends_with(".parquet") {
        Ok(Box::new(parquet::ParquetTarget::new(connection_string)?))
    } else if connection_string.ends_with(".avro") {
        Ok(Box::new(avro::AvroTarget::new(connection_string)?))
    } else if excel::is_spreadsheet(connection_string) {
        Ok(Box::new(excel::ExcelTarget::new(connection_string)?))
    } else if connection_string.contains(".duckdb#")
        || connection_string.ends_with(".duckdb")
        || connection_string.starts_with("duckdb:")
    {
        // Legacy DuckDB support: file.duckdb, file.duckdb#table, duckdb:file.duckdb
        Ok(Box::new(duckdb::DuckdbTarget::new(connection_string)?))
    } else if connection_string.contains(".db#")
        || connection_string.ends_with(".db")
        || connection_string.starts_with("sqlite:")
    {
        // Legacy SQLite support: file.db, file.db#table, sqlite:file.db
        Ok(Box::new(sqlite::SqliteTarget::new(connection_string)?))
    } else {
        Err(crate::TinyEtlError::Configuration(format!(
            "Unsupported target type: {}. Supported formats: \
            file.csv, file.tsv, file.json, file.jsonl, file.parquet, file.avro, file.xlsx[#sheet], file.db, file.db#table, file.duckdb, file.duckdb#table, \
            duckdb://path/file.duckdb#table, sqlite://path/file.db#table, postgres://user:pass@host:port/db#table, mysql://user:pass@host:port/db#table, mssql://user:pass@host:port/db#table, odbc://connection_string#table, snowflake://user@account/db/schema#table",
            connection_string
        )))
    }
}

/// New protocol-aware factory functions that handle modern connection strings
/// Use these for new protocol support (snowflake://, onelake://, etc.)
/// Create a source using the new protocol abstraction
pub async fn create_source_from_url(connection_string: &str) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type_and_options(
        connection_string,
        None,
        &std::collections::HashMap::new(),
    )
    .await
}

/// Create a source using the new protocol abstraction with optional type hint
pub async fn create_source_from_url_with_type(
    connection_string: &str,
    source_type: Option<&str>,
) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type_and_options(
        connection_string,
        source_type,
        &std::collections::HashMap::new(),
    )
    .await
}

/// Create a source using the new protocol abstraction with optional type hint and options
pub async fn create_source_from_url_with_type_and_options(
    connection_string: &str,
    source_type: Option<&str>,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Source>> {
    crate::options::check(
        crate::options::Role::Source,
        connection_string,
        source_type,
        options,
    )?;
    // Snowflake is a database, but unlike the others it takes endpoint options (key path, ...)
    if connection_string.starts_with("snowflake://") {
        return Ok(Box::new(snowflake::SnowflakeSource::new(
            connection_string,
            options,
        )?));
    }
    // Check if this looks like a protocol URL
    if connection_string.contains("://") {
        // Try database connectors first for database protocols
        if connection_string.starts_with("duckdb://")
            || connection_string.starts_with("sqlite://")
            || connection_string.starts_with("postgres://")
            || connection_string.starts_with("postgresql://")
            || connection_string.starts_with("mysql://")
            || connection_string.starts_with("mssql://")
            || connection_string.starts_with("sqlserver://")
            || connection_string.starts_with("odbc://")
        {
            // Database connectors don't support protocol options yet
            create_source(connection_string)
        } else {
            // Fall back to protocol abstraction for other protocols (file://, snowflake://, etc.)
            crate::protocols::create_source_from_url_with_type(
                connection_string,
                source_type,
                options,
            )
            .await
        }
    } else {
        // Bare paths: the extension-based factory
        create_source_with_options(connection_string, options)
    }
}

/// A database source that reads the result of `query` instead of a whole table
/// (`--source-query`). The connection string names the database without a `#table`.
pub fn create_query_source(
    connection_string: &str,
    query: &str,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Source>> {
    crate::options::check(
        crate::options::Role::Source,
        connection_string,
        None,
        options,
    )?;
    let query = normalize_query(query)?;
    let configuration = |message: String| crate::TinyEtlError::Configuration(message);

    let scheme = connection_string
        .split_once("://")
        .map(|(scheme, _)| scheme);
    let is_file_db = |extensions: &[&str]| {
        scheme.is_none()
            && extensions.iter().any(|ext| {
                connection_string
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .ends_with(ext)
            })
    };
    let kind = match scheme {
        Some("postgres" | "postgresql") => "postgres",
        Some("mysql") => "mysql",
        Some("mssql" | "sqlserver") => "mssql",
        Some("odbc") => "odbc",
        Some("snowflake") => "snowflake",
        Some("sqlite") => "sqlite",
        Some("duckdb") => "duckdb",
        None if connection_string.starts_with("sqlite:")
            || is_file_db(&[".db", ".sqlite", ".sqlite3"]) =>
        {
            "sqlite"
        }
        None if connection_string.starts_with("duckdb:") || is_file_db(&[".duckdb"]) => "duckdb",
        _ => {
            return Err(configuration(
                "--source-query needs a database source (PostgreSQL, MySQL, SQL Server, SQLite, \
                 DuckDB, ODBC or Snowflake)"
                    .to_string(),
            ))
        }
    };
    // With a query there's no table. An ODBC connection string is passed to the driver whole,
    // since its values may contain `#`; for the others a `#table` is a mistake.
    let database =
        if kind == "odbc" {
            connection_string
        } else {
            match connection_string.split_once('#') {
                Some((_, table)) if !table.is_empty() => return Err(configuration(
                    "--source-query reads the query's result, so leave the #table off the source"
                        .to_string(),
                )),
                Some((database, _)) => database,
                None => connection_string,
            }
        };

    Ok(match kind {
        "postgres" => Box::new(postgres::PostgresSource::with_query(database, &query)?),
        "mysql" => Box::new(mysql::MysqlSource::with_query(database, &query)?),
        "mssql" => Box::new(mssql::MssqlSource::with_query(database, &query)?),
        "odbc" => Box::new(odbc::OdbcSource::with_query(database, &query)?),
        "snowflake" => Box::new(snowflake::SnowflakeSource::with_query(
            database, &query, options,
        )?),
        "sqlite" => Box::new(sqlite::SqliteSource::with_query(database, &query)?),
        _ => Box::new(duckdb::DuckdbSource::with_query(database, &query)?),
    })
}

/// Trim a query and drop trailing semicolons, so it can be sampled or wrapped
fn normalize_query(query: &str) -> Result<String> {
    let query = query
        .trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    if query.is_empty() {
        return Err(crate::TinyEtlError::Configuration(
            "--source-query is empty".to_string(),
        ));
    }
    Ok(query.to_string())
}

/// Create a target using the new protocol abstraction
pub async fn create_target_from_url(connection_string: &str) -> Result<Box<dyn Target>> {
    create_target_from_url_with_options(connection_string, &std::collections::HashMap::new()).await
}

/// Create a target using the new protocol abstraction with options
pub async fn create_target_from_url_with_options(
    connection_string: &str,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Target>> {
    crate::options::check(
        crate::options::Role::Target,
        connection_string,
        None,
        options,
    )?;
    if connection_string.starts_with("snowflake://") {
        return Ok(Box::new(snowflake::SnowflakeTarget::new(
            connection_string,
            options,
        )?));
    }
    // Check if this looks like a protocol URL
    if connection_string.contains("://") {
        // Try database connectors first for database protocols
        if connection_string.starts_with("duckdb://")
            || connection_string.starts_with("sqlite://")
            || connection_string.starts_with("postgres://")
            || connection_string.starts_with("postgresql://")
            || connection_string.starts_with("mysql://")
            || connection_string.starts_with("mssql://")
            || connection_string.starts_with("sqlserver://")
            || connection_string.starts_with("odbc://")
        {
            // Database connectors don't support protocol options yet
            create_target(connection_string)
        } else {
            // Fall back to protocol abstraction for other protocols (file://, snowflake://, etc.)
            crate::protocols::create_target_from_url_with_options(connection_string, options).await
        }
    } else {
        // Bare paths: the extension-based factory
        create_target_with_options(connection_string, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_csv_source() {
        let source = create_source("test.csv");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_json_source() {
        let source = create_source("test.json");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_parquet_source() {
        let source = create_source("test.parquet");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_sqlite_source() {
        let result = create_source("test.db#table");
        if let Err(ref e) = result {
            println!("SQLite source creation failed: {}", e);
        }
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_sqlite_source_without_table() {
        let result = create_source("test.db");
        assert!(result.is_err());
        if let Err(err) = result {
            // The error will come from the SQLite constructor, not the factory
            assert!(err
                .to_string()
                .contains("SQLite source requires table specification"));
        }
    }

    #[test]
    fn test_create_sqlite_with_sqlite_prefix() {
        let source = create_source("sqlite:test.db#table");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_postgres_source() {
        let source = create_source("postgres://user:pass@localhost:5432/db#table");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_postgres_source_without_table() {
        let result = create_source("postgres://user:pass@localhost:5432/db");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_postgresql_source() {
        let source = create_source("postgresql://user:pass@localhost:5432/db#table");
        assert!(source.is_ok());
    }

    #[test]
    fn test_create_unsupported_source() {
        let source = create_source("test.unsupported");
        assert!(source.is_err());
    }

    #[test]
    fn test_create_csv_target() {
        let target = create_target("output.csv");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_json_target() {
        let target = create_target("output.json");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_parquet_target() {
        let target = create_target("output.parquet");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_sqlite_target() {
        let target = create_target("output.db");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_sqlite_target_with_table() {
        let target = create_target("output.db#mytable");
        assert!(target.is_ok());
    }

    #[test]
    fn test_query_source_errors() {
        let none = std::collections::HashMap::new();
        let err = |url: &str, query: &str| {
            create_query_source(url, query, &none)
                .err()
                .expect("should fail")
                .to_string()
        };
        assert!(err("data.csv", "SELECT 1").contains("needs a database source"));
        assert!(err("s3://b/data.parquet", "SELECT 1").contains("needs a database source"));
        assert!(err("out.db#t", "SELECT 1").contains("leave the #table off"));
        assert!(err("postgres://u@h/db#t", "SELECT 1").contains("leave the #table off"));
        assert!(err("postgres://u@h/db", " ; ").contains("is empty"));
        for url in [
            "postgres://u@h/db",
            "postgresql://u@h/db",
            "mysql://u@h/db",
            "mssql://u:p@h/db",
            "odbc://DSN=x;PWD={a#b}",
            "data.db",
            "sqlite:///tmp/data.db",
            "data.duckdb",
            "snowflake://u@acct/db/schema?warehouse=WH",
        ] {
            assert!(
                create_query_source(url, "SELECT 1;", &none).is_ok(),
                "{}",
                url
            );
        }
    }

    /// Load rows into a table, then read them back through a query
    async fn check_query_source(table_url: &str, database_url: &str) {
        use crate::schema::{Column, DataType, Schema, Value};
        let schema = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: "region".into(),
                    data_type: DataType::String,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    data_type: DataType::Integer,
                    nullable: true,
                },
            ],
            estimated_rows: None,
            primary_key_candidate: None,
        };
        let rows: Vec<crate::schema::Row> = (1..=6)
            .map(|i| {
                [
                    ("id".to_string(), Value::Integer(i)),
                    (
                        "region".to_string(),
                        Value::String(if i % 2 == 0 { "east" } else { "west" }.into()),
                    ),
                    ("amount".to_string(), Value::Integer(i * 10)),
                ]
                .into_iter()
                .collect()
            })
            .collect();
        let mut target = create_target(table_url).unwrap();
        target.connect().await.unwrap();
        target.create_table("", &schema).await.unwrap();
        target.write_batch(&rows).await.unwrap();
        target.finalize().await.unwrap();

        // A CTE, an aggregate, ORDER BY and a trailing semicolon
        let query = "WITH big AS (SELECT * FROM sales WHERE amount > 10)
                     SELECT region, COUNT(*) AS orders, SUM(amount) AS total
                     FROM big GROUP BY region ORDER BY region;";
        let mut source =
            create_query_source(database_url, query, &std::collections::HashMap::new()).unwrap();
        source.connect().await.unwrap();
        let schema = source.infer_schema(100).await.unwrap();
        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["region", "orders", "total"]);
        assert_eq!(schema.columns[1].data_type, DataType::Integer);
        source.reset().await.unwrap();
        let mut read = Vec::new();
        while source.has_more() {
            let batch = source.read_batch(100).await.unwrap();
            if batch.is_empty() {
                break;
            }
            read.extend(batch);
        }
        let summary: Vec<(String, String, String)> = read
            .iter()
            .map(|r| {
                let text = |k: &str| match &r[k] {
                    Value::String(s) => s.clone(),
                    Value::Integer(i) => i.to_string(),
                    Value::Decimal(d) => d.normalize().to_string(),
                    other => format!("{:?}", other),
                };
                (text("region"), text("orders"), text("total"))
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("east".into(), "3".into(), "120".into()),
                ("west".into(), "2".into(), "80".into())
            ]
        );
    }

    #[tokio::test]
    async fn test_sqlite_query_source() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("q.db").display().to_string();
        check_query_source(&format!("{}#sales", db), &db).await;
    }

    #[tokio::test]
    async fn test_duckdb_query_source() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("q.duckdb").display().to_string();
        check_query_source(&format!("{}#sales", db), &db).await;
    }

    #[tokio::test]
    async fn test_database_url_targets_skip_protocols() {
        for url in [
            "duckdb://output.duckdb#t",
            "sqlite://output.db#t",
            "postgres://u:p@localhost/db#t",
        ] {
            assert!(
                create_target_from_url_with_options(url, &Default::default())
                    .await
                    .is_ok(),
                "{}",
                url
            );
        }
    }

    #[test]
    fn test_create_sqlite_protocol_target() {
        let target = create_target("sqlite://output.db#mytable");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_postgres_target() {
        let target = create_target("postgres://user:pass@localhost:5432/db");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_postgres_target_with_table() {
        let target = create_target("postgres://user:pass@localhost:5432/db#mytable");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_postgresql_target() {
        let target = create_target("postgresql://user:pass@localhost:5432/db");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_mysql_target() {
        let target = create_target("mysql://user:pass@localhost:3306/db");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_mysql_target_with_table() {
        let target = create_target("mysql://user:pass@localhost:3306/db#mytable");
        assert!(target.is_ok());
    }

    #[test]
    fn test_create_unsupported_target() {
        let target = create_target("output.unsupported");
        assert!(target.is_err());
    }

    #[test]
    fn test_create_unsupported_protocol() {
        let target = create_target("redis://localhost:6379");
        assert!(target.is_err());
    }
}
