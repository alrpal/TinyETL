# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] - 2025-11-18

### Added
- **JSON as a Tier-1 Datatype**: Full support for JSON columns across all connectors
  - Added `Json` variant to `DataType` and `Value` enums in schema system
  - JSON values stored as `serde_json::Value` internally
  - Schema files now accept `type: json` for column definitions
  - JSON default values supported in schema files
  - Type inference automatically detects JSON values
  - Comprehensive test coverage for JSON operations

### Connector Support
- **PostgreSQL**: Maps JSON to native `JSONB` type for optimal performance
- **MySQL**: Maps JSON to native `JSON` type
- **SQLite**: Stores JSON as `TEXT` (SQLite's standard approach)
- **DuckDB**: Maps JSON to native `JSON` type
- **MSSQL**: Stores JSON as `NVARCHAR(MAX)`
- **Snowflake**: Maps JSON to `VARIANT` (Snowflake's semi-structured data type)
- **CSV**: Serializes JSON to compact string representation
- **JSON**: Preserves JSON objects natively
- **Parquet**: Stores JSON as UTF8 strings via Arrow
- **Avro**: Stores JSON as string type
- **ODBC**: Stores JSON as `NVARCHAR(MAX)`

### Enhanced
- Transformer (Lua): JSON values converted to strings for Lua script processing
- Arrow integration: JSON mapped to `ArrowDataType::Utf8` for compatibility
- Schema validation: JSON type fully integrated with validation pipeline

### Examples
- Added Example 17: SQLite JSON to Parquet demonstrating JSON column handling
- Includes preview mode demonstration and round-trip validation

### Fixed
- Fixed JSON serialization in Parquet writer to output proper JSON strings instead of Rust debug format
- Fixed `RUST_LOG=debug` environment variable now properly enables debug logging

## [0.7.0] - 2025-11-15

### Added
- ODBC connector support for broader database compatibility

### Fixed
- Fixed MySQL JSON columns being read as NULL values - now properly extracted as strings
- Resolved cargo install issues
- Various minor bug fixes

## [0.5.0] - 2025-11-12

### Changed
- **BREAKING**: Schema inference now defaults all columns to `nullable: true` for safety
  - Previous behavior could incorrectly infer `NOT NULL` constraints based on limited sample data
  - This prevents constraint violations when appending data with different null patterns
  - Users requiring strict `NOT NULL` constraints must now use explicit schema files
  - Affects all database connectors (DuckDB, SQLite, MySQL, PostgreSQL, MSSQL)

### Fixed
- Fixed NOT NULL constraint violations when appending to existing DuckDB tables
- Resolved issue where schema inferred from first batch caused failures on subsequent batches with NULL values

### Documentation
- Added prominent notes about nullable default behavior in README
- Clarified that explicit schema files are required for strict validation

## [0.4.0] - 2025-11-11

### Added
- DuckDB connector (source and destination)

### Changed
- Internal schema types migrated to Arrow datatypes

## [0.3.1] - 2025-11-11

### Added
- MySQL source support - can now read data from MySQL databases
- Additional test coverage for improved reliability

### Changed

### Fixed

## [0.3.0] - 2025

### Added
- Initial release with CSV, JSON, Parquet, SQLite, MySQL (target), PostgreSQL, MSSQL, and Avro support
- File, HTTP, SSH, and Snowflake protocol support
- YAML configuration files
- Schema validation
- Environment variable support for secrets
- Data transformation capabilities

[Unreleased]: https://github.com/alrpal/TinyETL/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/alrpal/TinyETL/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/alrpal/TinyETL/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/alrpal/TinyETL/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/alrpal/TinyETL/releases/tag/v0.3.0
