# TinyETL Examples

This directory contains examples demonstrating TinyETL's capabilities: file formats, databases (including Snowflake), and remote protocols such as HTTP, SSH and S3.

## Quick Start

Run all examples:

```bash
cd examples
chmod +x run_all_examples.sh
./run_all_examples.sh
```

## Architecture Overview

TinyETL now supports two layers:

1. **Connectors**: Handle data formats and databases (CSV, JSON, Parquet, Avro, SQLite, DuckDB, Postgres, MySQL, MSSQL, ODBC, Snowflake)
2. **Protocols**: Handle transport/authentication for files (file://, http(s)://, ssh://, s3://; future: databricks://, onelake://)

This separation allows you to read a CSV over HTTP and write it to Snowflake, or read a Snowflake table and write Parquet to S3, etc.

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

### 19_s3
**Read and write Amazon S3**

```bash
tinyetl run upload_config.yaml     # employees.csv -> s3://$TINYETL_S3_BUCKET/tinyetl-example/employees.parquet
tinyetl run download_config.yaml   # that object -> output_employees.json
```

- Round-trips data through a real bucket, as Parquet (YAML configs) and CSV (plain CLI)
- Writes the CSV object twice to confirm overwriting
- Needs `TINYETL_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_REGION`, exported or in a `.env` file at the repo root; skips itself otherwise

See [`19_s3/README.md`](19_s3/README.md) for the setup.

### 20_csv_to_postgres
**PostgreSQL Round Trip**

Loads a CSV into PostgreSQL, checks the stored values with `psql`, then exports the table back to CSV. It also exports a pre-seeded table with `DECIMAL` salaries and checks the values, and loads 2,500 generated rows whose column mixes whole numbers, decimals and blanks (issue #24), checking that the column is `numeric`, blanks are NULL, and the sum matches. Needs the `tinyetl-postgres` container from `test_env` (`cd test_env && docker-compose up -d postgres`); skips if it isn't running.

```bash
tinyetl customers.csv "postgres://testuser:testpass@localhost:5432/testdb#customers" --truncate
tinyetl "postgres://testuser:testpass@localhost:5432/testdb#customers" customers_export.csv
```

**What it tests:**
- Creating a PostgreSQL table from an inferred CSV schema
- `--truncate`, so re-runs replace the rows instead of appending
- Reading a table back out, including one TinyETL didn't create (the seeded `employees` table)

### 21_csv_to_postgres_odbc
**PostgreSQL Round Trip through ODBC**

The same round trip as example 20, but through the `odbc://` connector and the PostgreSQL ODBC driver (`brew install psqlodbc` or `apt install odbc-postgresql`). Needs the `tinyetl-postgres` container; skips if the container or driver is missing.

```bash
tinyetl customers.csv "odbc://Driver=/opt/homebrew/lib/psqlodbcw.so;Server=localhost;Port=5432;Database=testdb;UID=testuser;PWD=testpass#customers_odbc" --truncate
```

**What it tests:**
- Creating a table through ODBC with PostgreSQL column types
- Reading it back through ODBC with the table's column order

For ODBC against SQL Server, MySQL and SQLite too, run `test_env/odbc/run.sh` (Linux, in Docker).

### 22_snowflake
**Snowflake Load and Read**

Loads a typed CSV into Snowflake with stage + `COPY INTO` and with batched INSERTs, checks append and `--truncate`, loads 1,000,000 rows, and reads Snowflake's sample data. Needs a Snowflake account and key pair; skips itself otherwise. See [`22_snowflake/README.md`](22_snowflake/README.md) for the setup.

```bash
tinyetl data.csv "snowflake://TINYETL_LOADER@MYORG-MYACCOUNT/TINYETL_TEST/EXAMPLES?warehouse=TINYETL_WH#orders"
```

## Protocol Architecture

### Current Implementation Status

| Protocol | Status | Description |
|----------|--------|-------------|
| `file://` | ✅ Production | Local file system operations |
| `http://`, `https://` | ✅ Production | Web downloads (read only), with basic/bearer auth and custom headers |
| `ssh://` | ✅ Production | SCP file transfer (read only) |
| `s3://` | ✅ Production | Amazon S3 and S3-compatible stores, read and write (see `19_s3`) |
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

## Validation

Each example includes validation to ensure:
- ✅ Output files are created
- ✅ Data integrity is maintained  
- ✅ Expected record counts match
- ✅ Required fields are present
- ✅ Protocols parse URLs correctly

## Future Enhancements

1. **Databricks Support**: Add databricks:// protocol
2. **Microsoft OneLake**: Add onelake:// protocol  
3. **AWS Glue**: Add glue:// protocol
4. **BigQuery**: Add bigquery:// protocol
5. **Streaming Sources**: Kafka, Kinesis protocols
6. **Authentication Methods**: OAuth, service accounts, etc.

## Troubleshooting

**Build fails**: Ensure you have Rust 1.70+ and all dependencies
**Permission denied**: Make scripts executable with `chmod +x run.sh`
**Binary not found**: Run `cargo build --release` first
**Example fails**: Check individual example output for specific error details
