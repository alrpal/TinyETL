# TinyETL Examples

This directory contains examples demonstrating TinyETL's capabilities, including the new protocol abstraction layer that supports both local files and cloud platforms like Snowflake.

## Quick Start

Run all examples:

```bash
cd examples
chmod +x run_all_examples.sh
./run_all_examples.sh
```

## Architecture Overview

TinyETL now supports two layers:

1. **Connectors**: Handle data format (CSV, JSON, Parquet, SQLite, Postgres)
2. **Protocols**: Handle transport/authentication (file://, snowflake://, future: databricks://, onelake://)

This separation allows you to read from Snowflake and write to local Parquet, or read CSV and write to Snowflake, etc.

## Examples

### 01_basic_csv_to_json
**Backward Compatibility Test**

Demonstrates that existing file-based operations still work without protocol prefixes.

```bash
tinyetl input.csv output.json
```

**What it tests:**
- Legacy file path handling
- CSV to JSON conversion
- Basic data validation

### 02_file_protocol_explicit
**File Protocol Test**

Shows explicit `file://` protocol usage, which will be important for consistency when cloud protocols are added.

```bash
tinyetl "file:///path/to/input.csv" "file:///path/to/output.parquet"
```

**What it tests:**
- File protocol URL parsing
- Protocol abstraction layer
- CSV to Parquet conversion

### 03_snowflake_mock_source
**Snowflake Source Protocol**

Demonstrates reading from Snowflake (mock implementation) and writing to local files.

```bash
tinyetl "snowflake://user:pass@account.region.cloud/db/schema?warehouse=WH&table=sales" output.csv
```

**What it tests:**
- Snowflake URL parsing
- Protocol-to-connector delegation
- Mock data export workflow

**Connection String Format:**
```
snowflake://username:password@account.region.cloud/database/schema?warehouse=WAREHOUSE_NAME&table=TABLE_NAME&role=ROLE_NAME
```

### 04_snowflake_mock_target
**Snowflake Target Protocol**

Demonstrates reading local files and writing to Snowflake (mock implementation).

```bash
tinyetl input.csv "snowflake://user:pass@account.region.cloud/db/schema?warehouse=WH&table=orders"
```

**What it tests:**
- CSV to Snowflake workflow
- Temporary file handling
- Mock data import simulation

### 20_csv_to_postgres
**PostgreSQL Round Trip**

Loads a CSV into PostgreSQL, checks the stored values with `psql`, then exports the table back to CSV. Needs the `tinyetl-postgres` container from `test_env` (`cd test_env && docker-compose up -d postgres`); skips if it isn't running.

```bash
tinyetl customers.csv "postgres://testuser:testpass@localhost:5432/testdb#customers" --truncate
tinyetl "postgres://testuser:testpass@localhost:5432/testdb#customers" customers_export.csv
```

**What it tests:**
- Creating a PostgreSQL table from an inferred CSV schema
- `--truncate`, so re-runs replace the rows instead of appending
- Reading a table back out, including one TinyETL didn't create (the seeded `employees` table)

## Protocol Architecture

### Current Implementation Status

| Protocol | Status | Description |
|----------|--------|-------------|
| `file://` | ✅ Production | Local file system operations |
| `snowflake://` | 🔄 Mock | Cloud data warehouse (Snowflake) |
| `databricks://` | 📋 Planned | Databricks lakehouse platform |
| `onelake://` | 📋 Planned | Microsoft OneLake |

### How Protocols Work

1. **URL Parsing**: Connection strings are parsed to extract protocol, credentials, and parameters
2. **Protocol Selection**: The appropriate protocol handler is instantiated
3. **Connector Delegation**: Protocol creates the appropriate connector (CSV, Parquet, etc.)
4. **Data Flow**: Protocol manages authentication/transport while connector handles format

### Adding New Protocols

To add support for a new cloud platform:

1. Create `src/protocols/your_platform.rs`
2. Implement the `Protocol` trait
3. Add URL parsing for your platform's connection format
4. Register in `src/protocols/mod.rs`

Example structure for Databricks:

```rust
// databricks://token@workspace.cloud.databricks.com/catalog/schema?table=table_name
pub struct DatabricksProtocol;

#[async_trait]
impl Protocol for DatabricksProtocol {
    async fn create_source(&self, url: &Url) -> Result<Box<dyn Source>> {
        // Parse Databricks URL, authenticate, export to temp file
        // Return appropriate connector for the temp file
    }
    // ... other methods
}
```

## Running Individual Examples

Each example has its own `run.sh` script:

```bash
cd examples/01_basic_csv_to_json
./run.sh
```

## Production Implementation Notes

The current Snowflake implementation is a **mock** that demonstrates the architecture. For production use:

1. **Replace mock with real Snowflake API calls**
2. **Implement proper authentication** (JWT tokens, key-pair auth)
3. **Use Snowflake's COPY INTO/UNLOAD commands** for efficient data transfer
4. **Add connection pooling and error retry logic**
5. **Support Snowflake stages** for intermediate file storage

Example production workflow:
```
CSV → Protocol uploads to Snowflake stage → COPY INTO table
Table → UNLOAD to stage → Protocol downloads → Parquet
```

## Validation

Each example includes validation to ensure:
- ✅ Output files are created
- ✅ Data integrity is maintained  
- ✅ Expected record counts match
- ✅ Required fields are present
- ✅ Protocols parse URLs correctly

## Future Enhancements

1. **Real Snowflake Integration**: Replace mock with actual Snowflake connector
2. **Databricks Support**: Add databricks:// protocol
3. **Microsoft OneLake**: Add onelake:// protocol  
4. **AWS S3/Glue**: Add s3:// and glue:// protocols
5. **BigQuery**: Add bigquery:// protocol
6. **Streaming Sources**: Kafka, Kinesis protocols
7. **Authentication Methods**: OAuth, service accounts, etc.

## Troubleshooting

**Build fails**: Ensure you have Rust 1.70+ and all dependencies
**Permission denied**: Make scripts executable with `chmod +x run.sh`
**Binary not found**: Run `cargo build --release` first
**Example fails**: Check individual example output for specific error details
