pub mod csv;
pub mod json;
pub mod sqlite;

use async_trait::async_trait;
use crate::{Result, schema::{Schema, Row}};

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
}

/// Factory function to create a source connector from a connection string
pub fn create_source(connection_string: &str) -> Result<Box<dyn Source>> {
    if connection_string.ends_with(".csv") {
        Ok(Box::new(csv::CsvSource::new(connection_string)?))
    } else if connection_string.ends_with(".json") {
        Ok(Box::new(json::JsonSource::new(connection_string)?))
    } else if (connection_string.contains(".db#") || connection_string.ends_with(".db"))
        || connection_string.starts_with("sqlite:") {
        Ok(Box::new(sqlite::SqliteSource::new(connection_string)?))
    } else {
        Err(crate::TinyEtlError::Configuration(
            format!("Unsupported source type: {}. SQLite sources require table specification: file.db#table", connection_string)
        ))
    }
}

/// Factory function to create a target connector from a connection string
pub fn create_target(connection_string: &str) -> Result<Box<dyn Target>> {
    if connection_string.ends_with(".csv") {
        Ok(Box::new(csv::CsvTarget::new(connection_string)?))
    } else if connection_string.ends_with(".json") {
        Ok(Box::new(json::JsonTarget::new(connection_string)?))
    } else if connection_string.ends_with(".db") || connection_string.starts_with("sqlite:") {
        Ok(Box::new(sqlite::SqliteTarget::new(connection_string)?))
    } else {
        Err(crate::TinyEtlError::Configuration(
            format!("Unsupported target type: {}", connection_string)
        ))
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
            assert!(err.to_string().contains("SQLite source requires table specification"));
        }
    }
    
    #[test] 
    fn test_create_sqlite_with_sqlite_prefix() {
        let source = create_source("sqlite:test.db#table");
        assert!(source.is_ok());
    }
    
    #[test]
    fn test_create_unsupported_source() {
        let source = create_source("test.xlsx");
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
    fn test_create_sqlite_target() {
        let target = create_target("output.db");
        assert!(target.is_ok());
    }
    
    #[test]
    fn test_create_unsupported_target() {
        let target = create_target("output.xlsx");
        assert!(target.is_err());
    }
}
