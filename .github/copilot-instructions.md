# TinyETL - AI Coding Agent Instructions

## Project Overview

TinyETL is a **zero-config ETL tool** built in Rust that transfers data between formats (CSV, JSON, Parquet, Avro) and databases (PostgreSQL, MySQL, SQLite, DuckDB, MSSQL via ODBC) in a single 15MB binary. Performance target: 180k+ rows/sec. Current version: 0.10.0.

**Core Philosophy**: Auto-detect everything (schemas, formats, connections) while allowing explicit overrides via schema files and YAML configs.

## Architecture: Protocol + Connector Abstraction

TinyETL separates **transport** (how to access data) from **format** (how to parse/write data):

- **Protocols** (`src/protocols/`): Handle authentication and data access
  - `file://` - Local filesystem (default for plain paths)
  - `http://`, `https://` - Web downloads with Basic/Bearer auth, custom headers
  - `ssh://` - SCP file transfer
  - `snowflake://` - Cloud warehouse (currently mock implementation)
  
- **Connectors** (`src/connectors/`): Handle data format I/O via `Source` and `Target` traits
  - Files: CSV, JSON, Parquet, Avro, Excel (.xlsx/.xls)
  - Databases: PostgreSQL, MySQL, SQLite, DuckDB, MSSQL (via ODBC)

**Key Pattern**: `Protocol::create_source()` → returns `Box<dyn Source>`. This allows reading from Snowflake and writing to local Parquet, or vice versa.

## Critical Development Conventions

### 1. Connection String Format
```
# Databases: <protocol>://<creds>@<host>/<db>#<table>
postgresql://user:pass@localhost/mydb#orders
mysql://user@localhost:3306/db#customers

# Files with protocols
file:///path/to/data.csv
https://api.example.com/export.csv
ssh://user@server.com/data.csv

# Legacy file paths (backward compatible - no protocol)
data.csv
/path/to/file.parquet
output.duckdb#sales  # File with table specifier
```

**Important**: Table names are specified with `#` separator, NOT `/`. The `#` delimiter is consistent across all database connection strings.

### 2. Schema Inference Safety Rule
Auto-inferred schemas **always mark columns as nullable** (`nullable: true`). This is intentional for ETL safety—sample data may not represent all possible values. Only explicit schema files (`--schema-file schema.yaml`) can enforce `NOT NULL` constraints.

### 3. Transformation Behavior Differences
- **Inline transforms** (`--transform "col=expr"`): Preserve ALL original columns, add new ones
- **Lua file transforms** (`--transform-file script.lua`): Only columns explicitly returned are kept
- Transform schema is inferred from the **first transformed row**

### 4. Append-First Target Behavior
When a target exists:
1. If `--truncate` → delete and recreate
2. If target `supports_append()` → append new data
3. Otherwise → auto-truncate and warn

This differs from typical "fail if exists" behavior. See `TransferEngine::execute()` in `src/transfer.rs:88-113`.

### 5. Testing Pattern
- Unit tests in each module (`#[cfg(test)] mod tests`)
- Integration tests via `examples/*/run.sh` - these are executable documentation
- CI runs: `cargo fmt --check`, `cargo clippy`, `cargo test` (see `.github/workflows/pr.yaml`)
- **No integration test directory** - examples serve dual purpose

### 6. Error Handling with `TinyEtlError`
Custom error types in `src/error.rs`:
```rust
TinyEtlError::Connection    // Source/target connection failures
TinyEtlError::SchemaInference  // Auto-detection failures
TinyEtlError::Configuration  // Invalid config, URLs, params
TinyEtlError::Transform      // Lua script errors
TinyEtlError::DataValidation // Schema validation failures
```

Use `Result<T>` alias (`src/error.rs:48`). Never use bare `anyhow::Result`.

## Developer Workflows

### Build and Run
```bash
# Development
cargo build
cargo run -- data.csv output.json

# Release (15MB stripped binary with LTO)
cargo build --release
./target/release/tinyetl --version

# Clean build
cargo clean
cargo build --release
```

### Testing
```bash
# Run all tests
cargo test

# Run specific test
cargo test test_csv_column_order

# Run examples (integration tests)
cd examples && ./run_all_examples.sh
cd examples/01_basic_csv_to_json && ./run.sh

# Format and lint (required before PR)
cargo fmt --all
cargo clippy --all-features --all-targets -- -D warnings
```

### Debugging
```bash
# Enable verbose logging
RUST_LOG=debug cargo run -- data.csv output.json --log-level info

# Test protocol URL parsing
cargo run -- --help
cargo run -- "snowflake://user:pass@account/db/schema?warehouse=WH&table=sales" output.csv --dry-run
```

## Key Files and Their Roles

- **`src/main.rs`**: CLI parsing, config loading (YAML or args), connector creation, execution orchestration
- **`src/transfer.rs`**: `TransferEngine::execute()` - core ETL loop with batching, progress bars, schema handling
- **`src/schema.rs`**: Type system (`DataType`, `Value`, `Row`), schema inference, validation, YAML schema file format
- **`src/transformer.rs`**: Lua VM integration, inline expression parsing, schema transformation
- **`src/yaml_config.rs`**: YAML config format, env var substitution (`${VAR_NAME}`), config↔CLI conversion
- **`src/secrets.rs`**: Environment-based password injection (`TINYETL_SECRET_*`), security warnings
- **`src/protocols/mod.rs`**: `Protocol` trait, factory pattern for URL scheme→protocol mapping
- **`src/connectors/mod.rs`**: `Source`/`Target` traits, factory functions for format detection

## Adding New Features

### Add a New File Format Connector
1. Create `src/connectors/<format>.rs` with `Source` and `Target` implementations
2. Register in `src/connectors/mod.rs::create_source()` and `create_target()` by file extension
3. Add to `Cargo.toml` dependencies if new crate needed
4. Create example in `examples/NN_<format>_demo/` with `run.sh` script
5. Update README.md supported formats section

### Add a New Protocol
1. Create `src/protocols/<protocol>.rs` implementing `Protocol` trait
2. Register in `src/protocols/mod.rs::create_protocol()` by URL scheme
3. Implement `create_source()` and `create_target()` to return appropriate connectors
4. Add URL validation in `validate_url()`
5. Add example in `examples/` demonstrating the protocol
6. Update README.md protocol table

### Add a Transform Function
Transformations use **mlua 0.8** with Lua 5.4 vendored. Available in transform functions:
- `row.<field>` - access input columns
- String ops: `..` (concat), `string.match()`, `string.gsub()`, `string.find()`
- Math: `math.floor()`, `tonumber()`, operators
- Logic: `and`, `or`, `not`, ternary: `condition and true_val or false_val`
- Return `nil` or `{}` to filter out rows (Lua files only)

See `src/transformer.rs:1-100` for implementation details.

## YAML Configuration Format (v1)

```yaml
version: 1
source:
  uri: "employees.csv"
  options:  # Protocol-specific (HTTP auth, headers)
    auth.bearer: "${API_TOKEN}"
    header.User-Agent: "TinyETL/0.10.0"
target:
  uri: "output.json"
options:
  batch_size: 10000
  transform:
    type: script  # or: inline, file, none
    value: |
      full_name = row.first .. " " .. row.last
```

**Breaking change in v0.9.0**: Transform configs require explicit `type` field. Old format without `type` is deprecated.

## Common Patterns

### Reading Database → File
```rust
// In Protocol::create_source():
// 1. Parse connection URL (use url::Url crate)
// 2. Extract credentials, host, database, table from URL
// 3. Return appropriate database Source connector
let db_source = Box::new(PostgresSource::new(connection_string)?);
```

### Column Order Preservation
CSV and Excel sources preserve header order via `self.headers: Vec<String>`. Schema column order must match for correct target output. See `src/connectors/csv.rs:37-78` and `src/connectors/excel.rs:157-183` for the pattern.

### Schema File Validation
Schema files (`--schema-file`) validate **before** transformations. Flow:
1. Load schema YAML
2. Validate row types, patterns, nullability
3. Apply defaults for missing fields
4. **Then** run transformations
5. Transfer validated+transformed data

See `src/transfer.rs:146-156` for validation integration.

### Environment Variable Substitution
Use `${VAR_NAME}` in YAML configs. Processed in `yaml_config.rs::substitute_env_vars()`. Works in URIs, passwords, transform file paths. Security: Credentials are masked in logs.

## Dependencies and Constraints

- **Rust Edition**: 2021, MSRV 1.70
- **Async Runtime**: Tokio 1.0 with "full" features
- **Arrow/Parquet**: Version 57.0 (keep synchronized)
- **Database Drivers**: sqlx 0.6, tiberius 0.12, odbc-api 8.0, duckdb 1.4.1
- **Lua**: mlua 0.8 with lua54 and vendored features (bundles Lua, no system dependency)
- **Excel**: calamine 0.26 (reading), xlsxwriter 0.6 (writing)
- **Release Profile**: LTO enabled, single codegen unit, symbols stripped for 15MB binary

## ODBC Platform Notes
- Linux: Requires `unixodbc-dev` package
- macOS: Requires `unixodbc` from Homebrew (separate x86/ARM builds)
- Windows: Uses native ODBC driver manager (no extra deps)

See `.github/workflows/release.yml:53-89` for platform-specific setup.

## Current Branch Context
Working on: `add-excel-connector`

### Excel Connector Implementation Details
- **Reading**: Uses `calamine` crate for .xlsx/.xls parsing
- **Writing**: Uses `xlsxwriter` crate for .xlsx generation  
- **Sheet Syntax**: `file.xlsx#SheetName` (optional, defaults to first sheet for reading, "Sheet1" for writing)
- **Column Order**: Preserved from Excel header row using `self.headers: Vec<String>`
- **Memory Model**: Loads entire Excel file into memory (suitable for datasets <100k rows)
- **Append Mode**: Not supported (`supports_append()` returns `false`) - Excel targets truncate-and-write
- **Type Conversions**: Excel DateTime → String, Excel Float → Decimal, Excel Int → Integer
