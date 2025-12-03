# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0] - 2024-12-03

### Added
- **Source and Target Options in YAML Configuration**: YAML configs can now specify connector-specific options under `source.options` and `target.options` sections
  - Allows passing protocol-specific configuration without cluttering the main options section
  - Options are passed to the respective protocol handlers for specialized processing
  - Particularly useful for HTTP sources with custom headers and authentication
  
- **HTTP Protocol Options Support**: HTTP/HTTPS sources now support comprehensive options
  - **Custom Headers**: Add custom HTTP headers with `header.HeaderName` syntax
    - Example: `header.User-Agent: "TinyETL/0.10.0"`
    - Example: `header.Accept: "text/csv"`
    - Example: `header.X-API-Version: "v2"`
  - **Basic Authentication**: Configure using `auth.basic.username` and `auth.basic.password` options
  - **Bearer Token Authentication**: Configure using `auth.bearer` option
  - Supports environment variable substitution for sensitive values (e.g., `auth.bearer: "${BEARER_TOKEN}"`)
  - Headers and authentication automatically logged (credentials masked) for debugging

### Enhanced
- **Protocol Abstraction**: All protocols now support the options parameter throughout the API
  - `Protocol::create_source()` accepts options HashMap
  - `Protocol::create_target()` accepts options HashMap  
  - File protocol: Options parameter available for future extensions
  - SSH protocol: Options parameter available for future extensions
  - Snowflake protocol: Options parameter available for future extensions
  
- **Default Configuration Template**: Updated `generate-default-config` output to include examples of source and target options
  - Shows commented examples of HTTP authentication methods
  - Demonstrates custom header configuration
  - Includes environment variable usage examples

### Examples
- Added Example 18: HTTP with Authentication demonstrating various authentication methods
  - `config.yaml` - Basic authentication example
  - `bearer_config.yaml` - Bearer token authentication
  - `custom_headers_config.yaml` - Custom HTTP headers
  - `public_config.yaml` - Public endpoint with optional headers
  - Includes test HTTP server setup via Docker Compose

### Documentation
- Updated YAML configuration format to show source/target options structure
- Added inline documentation for HTTP authentication options
- Enhanced default config template with practical examples

## [0.9.0] - 2024-11-24

### BREAKING CHANGES
- **Transform Configuration Format**: YAML transform configuration now requires explicit `type` and `value` fields
  - Old format: `transform: "expression"` or `transform_file: "file.lua"`
  - New format: `transform: { type: inline, value: "expression" }` or `transform: { type: file, value: "file.lua" }`
  - **Migration required** for existing YAML config files using transformations
  - CLI arguments (`--transform`, `--transform-file`) remain unchanged and fully compatible

### Added
- **Config Generation Commands**: New `generate-config` and `generate-default-config` CLI subcommands for easier YAML configuration creation
  - `tinyetl generate-config [OPTIONS] <SOURCE> <TARGET>` - Generate YAML config from CLI arguments
  - `tinyetl generate-default-config` - Output example configuration with comments
- **GitHub Actions PR Workflow**: Automated CI pipeline for pull requests with formatting, linting, and testing
- **Enhanced Transform Config**: Improved YAML serialization for transformation configurations with tagged union format
  - Supports `type: file`, `type: inline`, `type: script`, and `type: none` formats
  - Better type safety and clarity in YAML configuration
  - Explicit configuration prevents ambiguity

### Changed
- **Refactored Config Module**: Split YAML-specific configuration into separate `yaml_config.rs` module for better code organization
- **Transform Config Structure**: Updated from string-based to enum-based with proper serde support for type-safe YAML serialization
- **README Updates**: 
  - Updated version badge to 0.9.0
  - Updated binary size to 15MB
  - Enhanced configuration documentation with inline comments and new transform format examples
- **Code Quality**: Fixed numerous clippy warnings across the codebase for better maintainability
  - Replaced `.last()` with `.next_back()` for iterator optimizations
  - Added missing `#[allow(dead_code)]` attributes for test utilities
  - Improved error handling and type conversions

### Fixed
- Compiler warnings and clippy lints throughout the codebase
- Inconsistent formatting in various modules
- Missing or incomplete error messages in configuration parsing

### Documentation
- Added comprehensive inline comments to default configuration example
- Improved YAML configuration format documentation with all options explained
- Added examples for environment variable usage in configurations
- Updated command-line help text with new subcommands
- Enhanced README with new transform configuration format

### Internal
- Extracted YAML configuration logic into dedicated module for better separation of concerns
- Improved test coverage for configuration serialization/deserialization
- Enhanced type safety in transform configuration handling

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
