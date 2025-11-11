# Example 15: CSV to DuckDB (Source & Target)

This example demonstrates using DuckDB as both a target and source in TinyETL.

## Overview

DuckDB is an embedded analytical database (similar to SQLite but optimized for analytics). This example shows:

1. **Part 1**: Loading CSV data into a DuckDB database
2. **Part 2**: Reading data from DuckDB and exporting to JSON

## Files

- `products.csv` - Sample product data with 6 records
- `products.duckdb` - DuckDB database (created by the script)
- `products_output.json` - JSON export from DuckDB (created by the script)
- `run.sh` - Script to run both parts of the example

## Running the Example

```bash
./run.sh
```

Or from the project root:

```bash
cd examples/15_csv_to_duckdb
../../target/release/tinyetl products.csv "products.duckdb#products"
../../target/release/tinyetl "products.duckdb#products" products_output.json
```

## Connection String Format

DuckDB uses the following format:

- **Target (write)**: `<file>.duckdb#<table_name>`
- **Source (read)**: `<file>.duckdb#<table_name>`

Example:
```bash
tinyetl input.csv "data.duckdb#my_table"
tinyetl "data.duckdb#my_table" output.json
```

## Why DuckDB?

- **Embedded**: No server setup required, just a file
- **Fast**: Optimized for analytical queries
- **SQL**: Full SQL support for complex queries
- **Portable**: Single file database, easy to share
- **Analytics**: Better performance than SQLite for OLAP workloads

## Validation

The script validates:
- Database file is created
- Correct number of records are written
- Data can be read back successfully
- JSON output contains expected fields

## Requirements

Optional (for validation):
- `duckdb` CLI tool: `brew install duckdb` (macOS)
- `jq` for JSON parsing: `brew install jq`
