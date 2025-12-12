pub mod avro;
pub mod csv;
pub mod duckdb;
pub mod excel;
pub mod json;
pub mod mssql;
pub mod mysql;
pub mod odbc;
pub mod parquet;
pub mod postgres;
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
    if connection_string.ends_with(".csv") {
        Ok(Box::new(csv::CsvSource::new(connection_string)?))
    } else if connection_string.ends_with(".json") {
        Ok(Box::new(json::JsonSource::new(connection_string)?))
    } else if connection_string.ends_with(".parquet") {
        Ok(Box::new(parquet::ParquetSource::new(connection_string)?))
    } else if connection_string.ends_with(".avro") {
        Ok(Box::new(avro::AvroSource::new(connection_string)?))
    } else if connection_string.ends_with(".xlsx") || connection_string.ends_with(".xls") || connection_string.contains(".xlsx#") || connection_string.contains(".xls#") {
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
    } else {
        Err(crate::TinyEtlError::Configuration(
            format!("Unsupported source type: {}. Supported formats: file.csv, file.json, file.parquet, file.avro, file.xlsx, file.xlsx#sheet, file.xls, file.db#table, file.duckdb#table, postgres://user:pass@host:port/db#table, mysql://user:pass@host:port/db#table, mssql://user:pass@host:port/db#table, odbc://connection_string#table", connection_string)
        ))
    }
}

/// Factory function to create a target connector from a connection string
/// Supports both protocol-based and legacy file-based connections
pub fn create_target(connection_string: &str) -> Result<Box<dyn Target>> {
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
        } else {
            return Err(crate::TinyEtlError::Configuration(format!(
                "Unsupported protocol in: {}. Supported protocols: duckdb://, sqlite://, postgres://, mysql://, mssql://, odbc://",
                connection_string
            )));
        }
    }

    // Handle legacy file-based and non-protocol database connections
    if connection_string.ends_with(".csv") {
        Ok(Box::new(csv::CsvTarget::new(connection_string)?))
    } else if connection_string.ends_with(".json") {
        Ok(Box::new(json::JsonTarget::new(connection_string)?))
    } else if connection_string.ends_with(".parquet") {
        Ok(Box::new(parquet::ParquetTarget::new(connection_string)?))
    } else if connection_string.ends_with(".avro") {
        Ok(Box::new(avro::AvroTarget::new(connection_string)?))
    } else if connection_string.ends_with(".xlsx") || connection_string.ends_with(".xls") || connection_string.contains(".xlsx#") || connection_string.contains(".xls#") {
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
            file.csv, file.json, file.parquet, file.avro, file.xlsx, file.xlsx#sheet, file.xls, file.db, file.db#table, file.duckdb, file.duckdb#table, \
            duckdb://path/file.duckdb#table, sqlite://path/file.db#table, postgres://user:pass@host:port/db#table, mysql://user:pass@host:port/db#table, mssql://user:pass@host:port/db#table, odbc://connection_string#table",
            connection_string
        )))
    }
}

/// New protocol-aware factory functions that handle modern connection strings
/// Use these for new protocol support (snowflake://, onelake://, etc.)
/// Create a source using the new protocol abstraction
pub async fn create_source_from_url(connection_string: &str) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type_and_options(connection_string, None, &std::collections::HashMap::new()).await
}

/// Create a source using the new protocol abstraction with optional type hint
pub async fn create_source_from_url_with_type(
    connection_string: &str,
    source_type: Option<&str>,
) -> Result<Box<dyn Source>> {
    create_source_from_url_with_type_and_options(connection_string, source_type, &std::collections::HashMap::new()).await
}

/// Create a source using the new protocol abstraction with optional type hint and options
pub async fn create_source_from_url_with_type_and_options(
    connection_string: &str,
    source_type: Option<&str>,
    options: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn Source>> {
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
            crate::protocols::create_source_from_url_with_type(connection_string, source_type, options).await
        }
    } else {
        // Fallback to legacy connector system for backward compatibility
        create_source(connection_string)
    }
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
    // Check if this looks like a protocol URL
    if connection_string.contains("://") {
        // Try database connectors first for database protocols
        if connection_string.starts_with("sqlite://")
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
        // Fallback to legacy connector system for backward compatibility
        create_target(connection_string)
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
