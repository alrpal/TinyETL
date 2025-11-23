use anyhow::Error as AnyhowError;

#[derive(Debug, thiserror::Error)]
pub enum TinyEtlError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Schema inference error: {0}")]
    SchemaInference(String),

    #[error("Data transfer error: {0}")]
    DataTransfer(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Config error: {0}")] // Alias for Configuration for backwards compatibility
    Config(String),

    #[error("Transform error: {0}")]
    Transform(String),

    #[error("Data validation error: {0}")]
    DataValidation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSV parsing error: {0}")]
    Csv(#[from] csv::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Lua error: {0}")]
    Lua(#[from] mlua::Error),

    #[error("General error: {0}")]
    General(#[from] AnyhowError),
}

pub type Result<T> = std::result::Result<T, TinyEtlError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_error_display() {
        let err = TinyEtlError::Connection("test connection".to_string());
        assert_eq!(err.to_string(), "Connection error: test connection");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let etl_err = TinyEtlError::from(io_err);
        assert!(matches!(etl_err, TinyEtlError::Io(_)));
    }

    #[test]
    fn test_all_error_types() {
        let schema_err = TinyEtlError::SchemaInference("bad schema".to_string());
        assert_eq!(schema_err.to_string(), "Schema inference error: bad schema");

        let transfer_err = TinyEtlError::DataTransfer("transfer failed".to_string());
        assert_eq!(
            transfer_err.to_string(),
            "Data transfer error: transfer failed"
        );

        let config_err = TinyEtlError::Configuration("bad config".to_string());
        assert_eq!(config_err.to_string(), "Configuration error: bad config");
    }

    #[test]
    fn test_csv_error_conversion() {
        // Create a proper CSV error through IO error
        let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, "CSV parse error");
        let csv_err = csv::Error::from(io_err);
        let etl_err = TinyEtlError::from(csv_err);
        assert!(matches!(etl_err, TinyEtlError::Csv(_)));
    }
}
