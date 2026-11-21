# TinyETL
**Fast, zero-config ETL in a single binary**

[![CI](https://github.com/alrpal/TinyETL/actions/workflows/pr.yaml/badge.svg?branch=master)](https://github.com/alrpal/TinyETL/actions/workflows/pr.yaml)
[![Version](https://img.shields.io/badge/version-0.11.0-blue)](https://github.com/alrpal/tinyetl/releases)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](https://www.rust-lang.org/tools/install)
[![Download size](https://img.shields.io/badge/download-~20MB-green)](https://github.com/alrpal/tinyetl/releases)

![TinyETL Demo](examples/tinyetl_preview2-normal-framerate.gif)

Transform and move data between any format or database **instantly**. No dependencies, no config files, just one command.

```bash
# MySQL → Parquet with inline transformation 
tinyetl "mysql://user:@host/db#orders" orders.parquet \
  --transform "total_usd=row.amount * row.exchange_rate"

# Load a CSV into Snowflake (staged and loaded with COPY INTO)
tinyetl orders.csv "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders"

# Excel sheet → Parquet on S3
tinyetl "budget.xlsx#Q4" s3://my-bucket/finance/q4.parquet

# Download & convert web data
tinyetl "https://api.data.gov/export.json" analysis.parquet
```

## Why TinyETL?

✅ **Single binary** — about a 20 MB download (50 MB unpacked), with no runtime dependencies: no Python, no JVM, no shared libraries to install  
✅ **Fast** — 150k–380k rows/sec for a 1M-row CSV on a laptop (see [Performance](#performance))  
✅ **Zero configuration** — automatic schema detection and table creation (override with schema and config files in yaml)  
   *Note: Auto-inferred schemas default all columns to nullable for safety*

✅ **Lua transformations** — powerful data transformations  
✅ **Universal connectivity** — CSV, JSON, Excel, Parquet, Avro, MySQL, PostgreSQL, SQLite, CrateDB, DuckDB, MSSQL, ODBC, Snowflake. Coming soon: Databricks, OneLake

✅ **Cross-platform** — Linux, macOS, Windows ready

## Quick Install

**Download the binary** (recommended):

Visit the [releases page](https://github.com/alrpal/tinyetl/releases/latest) and download the appropriate binary for your platform:
- Linux x64, Linux ARM64
- macOS Apple Silicon, macOS Intel
- Windows x64, Windows ARM64

The Linux binary needs glibc 2.35 or later (Ubuntu 22.04+, Debian 12+, Fedora 36+). On older systems, such as RHEL/Rocky 9 or Amazon Linux 2023, build from source with Cargo (below).

Each download is a single self-contained binary with nothing else to install. The Linux and macOS binaries include the unixODBC driver manager (LGPL-2.1-or-later); see [License](#license).

**Linux x86_64** (use `tinyetl-linux-aarch64` for ARM64):
```bash
mkdir tinyetl && cd tinyetl
curl -L https://github.com/alrpal/TinyETL/releases/latest/download/tinyetl-linux-x86_64.tar.gz | tar xz
sudo mv tinyetl /usr/local/bin/   # optional: put it on your PATH
```

**macOS Apple Silicon** (use `tinyetl-macos-intel-x86_64` for Intel Macs):
```bash
mkdir tinyetl && cd tinyetl
curl -L https://github.com/alrpal/TinyETL/releases/latest/download/tinyetl-macos-apple-silicon-aarch64.tar.gz | tar xz
sudo mv tinyetl /usr/local/bin/
```
The binary isn't notarized by Apple. If you downloaded it with a browser and macOS blocks it, run `xattr -d com.apple.quarantine tinyetl` (files fetched with `curl` aren't affected).

**Windows x86_64** (use `tinyetl-windows-aarch64` for ARM64):
```powershell
Invoke-WebRequest -Uri "https://github.com/alrpal/TinyETL/releases/latest/download/tinyetl-windows-x86_64.zip" -OutFile "tinyetl.zip"
Expand-Archive -Path "tinyetl.zip" -DestinationPath "tinyetl"
# then add the tinyetl folder to your PATH
```

Each archive also contains `LICENSE`, `THIRD_PARTY_LICENSES.md` and a `licenses/` folder.

**Or install with Cargo** (builds from source):
```bash
cargo install tinyetl
```

Building from source needs Rust 1.94 or later (`rustup update`) and a C/C++ compiler. The bundled C libraries (DuckDB, SQLite, Lua, unixODBC, AWS-LC, and OpenSSL on Linux) are compiled from source, so no system libraries are needed. A clean build takes several minutes, mostly DuckDB:
- Debian/Ubuntu: `sudo apt install build-essential perl` (`perl` is used to build OpenSSL)
- macOS: `xcode-select --install`

**Verify installation**:
```bash
tinyetl --version
```

## Get Started in 30 Seconds

```bash
# File format conversion (auto-detects schemas)
tinyetl data.csv output.parquet
tinyetl data.json analysis.db
tinyetl report.xlsx report.csv

# Database to database 
tinyetl "postgresql://user:@host/db#users" "mysql://user:@host/db#users"

# Transform while transferring
tinyetl sales.csv results.db --transform "profit=row.revenue - row.costs; margin=profit/revenue"

# Load a large file in big batches (memory stays flat)
tinyetl huge_dataset.csv "analytics.duckdb#events" --batch-size 100000

# Download and convert web data
tinyetl "https://example.com/api/export" local_data.json --source-type=csv

# Run complex ETL jobs from configuration files
tinyetl run my_etl_job.yaml
```

## Usage
<div style="overflow-x: auto;">

```
A tiny ETL tool for moving data between sources

Usage: tinyetl [OPTIONS] [SOURCE] [TARGET]
       tinyetl <COMMAND>

Commands:
  run                      Run a job from a YAML configuration file
  generate-default-config  Generate a default YAML configuration example and output to STDOUT
  generate-config          Generate a YAML configuration file from CLI arguments and output to STDOUT
  help                     Print this message or the help of the given subcommand(s)

Arguments:
  [SOURCE]  Source connection string (file path or connection string)
  [TARGET]  Target connection string (file path or connection string)

Options:
      --infer-schema             Auto-detect columns and types
      --schema-file <FILE>       Path to schema file (YAML) to override auto-detection
      --batch-size <BATCH_SIZE>  Number of rows per batch [default: 10000]
      --preview <N>              Show first N rows and inferred schema without copying
      --dry-run                  Validate source/target without transferring data
      --log-level <LOG_LEVEL>    Log level: info, warn, error [default: info]
      --skip-existing            Not implemented yet; has no effect
      --truncate                 Truncate target before writing (overrides append-first behavior)
      --transform-file <FILE>    Path to Lua file containing a 'transform' function
      --transform <EXPRESSIONS>  Inline transformation expressions (semicolon-separated, e.g., "new_col=row.old_col * 2; name=row.first .. ' ' .. row.last")
      --source-type <TYPE>       Force source file type (csv, json, parquet, avro, xlsx, ...) - useful for URLs without clear extensions
      --source-secret-id <ID>    Secret ID for source password (resolves to TINYETL_SECRET_{id})
      --dest-secret-id <ID>      Secret ID for destination password (resolves to TINYETL_SECRET_{id})
  -h, --help                     Print help
  -V, --version                  Print version
```

</div>

Basic usage examples:

```bash
# Local file operations
tinyetl data.csv output.db
tinyetl data.csv output.parquet
tinyetl data.csv output.avro
tinyetl data.json output.csv
tinyetl data.avro output.json
tinyetl "budget.xlsx#Q4" q4.csv
tinyetl "postgresql://user:@localhost/mydb#orders" orders.xlsx

# Download from web
tinyetl "https://example.com/data.csv" output.json
tinyetl "https://api.example.com/export" data.csv --source-type=csv

# Secure file transfer via SSH
tinyetl "ssh://user@server.com/data.csv" output.parquet

# Database operations
tinyetl data.csv "postgresql://user:pass@localhost/mydb#customers"
tinyetl data.csv "mysql://user:pass@localhost:3306/mydb#customers"
tinyetl "sqlite:///source.db#users" output.csv

# Cloud storage and warehouses
tinyetl s3://my-bucket/raw/orders.csv orders.parquet
tinyetl orders.parquet "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders"

# Data inspection and validation
tinyetl data.csv output.db --preview 10     # show rows and the inferred schema
tinyetl data.csv output.db --dry-run        # check connections and schema only

# Replace a table's rows instead of appending
tinyetl data.csv output.db --truncate

# Custom batch size
tinyetl data.csv output.db --batch-size 5000

# Apply inline transformations
tinyetl data.csv output.db --transform "full_name=row.first_name .. ' ' .. row.last_name; age_next_year=row.age + 1"

# Apply transformations from Lua file
tinyetl data.csv output.db --transform-file transform.lua
```

### Supported Data Sources and Targets

TinyETL supports two main categories of data sources and targets:

#### File-Based Sources (Multiple Protocols)

**File Formats:**
- **CSV** - Comma-separated values
- **JSON** - JavaScript Object Notation (array of objects)
- **Parquet** - Columnar storage format
- **Avro** - Binary serialization format with schema evolution
- **Excel** - Reads `.xlsx`, `.xlsm`, `.xlsb`, `.xls` and OpenDocument `.ods`; writes `.xlsx`. Pick a sheet with `file.xlsx#Sheet` (see "Excel" below)

**Access Protocols:**
- **Local Files** - Direct file system access
  ```bash
  tinyetl data.csv output.json
  tinyetl data.csv output.avro
  tinyetl /path/to/file.parquet data.csv
  tinyetl data.avro output.json
  ```
- **HTTP/HTTPS** - Download from web servers (supports authentication and custom headers via YAML config)
  ```bash
  tinyetl "https://example.com/data.csv" output.parquet
  tinyetl "https://api.example.com/export" data.csv --source-type=csv
  # For authenticated APIs, use YAML config (see HTTP Source Options section)
  ```
- **SSH/SCP** - Secure file transfer
  ```bash
  tinyetl "ssh://user@server.com/data/file.csv" output.json
  tinyetl "ssh://user@server.com:2222/remote/data.parquet" local.csv
  ```

- **Amazon S3** - Read and write single objects (also works with S3-compatible stores such as Cloudflare R2)
  ```bash
  tinyetl s3://my-bucket/raw/orders.csv orders.parquet
  tinyetl "postgres://user@host/db#orders" s3://my-bucket/exports/orders.parquet
  ```

**Protocol Features:**
- **file://** - Local file system (default for simple paths)
- **http://** and **https://** - Web downloads with progress tracking
  - Supports Basic and Bearer token authentication
  - Custom HTTP headers via YAML configuration
  - Environment variable substitution for secure credential management
- **ssh://** - Secure shell file transfer using SCP
- **s3://** - Amazon S3 and S3-compatible object storage, as both source and target
  - Credentials from `AWS_*` environment variables, IAM roles, or YAML options (see S3 Options section)
- **--source-type** parameter for format override (useful for URLs without clear extensions)

#### Database Sources

**Supported Databases:**
- **SQLite** - Embedded database
- **PostgreSQL** - Advanced open-source database
- **MySQL** - Popular relational database
- **SQL Server** - Native driver (`mssql://`), no ODBC driver needed
- **CrateDB** - Distributed SQL database, through `postgresql://` URLs
- **DuckDB** - Embedded analytical database optimized for OLAP workloads
- **ODBC** - Any database with an ODBC driver (tested: SQL Server, PostgreSQL, MySQL/MariaDB, SQLite)
- **Snowflake** - Cloud data warehouse; bulk loads through stage + `COPY INTO`

**Connection Examples:**
```bash
# SQLite
tinyetl "sqlite:///path/to/db.sqlite#table" output.csv
tinyetl data.csv "sqlite:///output.db#customers"

# PostgreSQL  
tinyetl "postgresql://user:@localhost/mydb#orders" output.parquet
tinyetl data.csv "postgresql://user:@localhost/mydb#customers"

# MySQL
tinyetl "mysql://user:@localhost:3306/mydb#products" output.json
tinyetl data.csv "mysql://user:@localhost:3306/mydb#sales"

# SQL Server (native)
tinyetl "mssql://sa:pass@localhost:1433/mydb#orders" output.parquet
tinyetl data.csv "mssql://sa:pass@localhost:1433/mydb#customers"

# CrateDB
tinyetl "postgresql://crate:@localhost/doc#orders" output.parquet
tinyetl data.csv "postgresql://crate:@localhost/doc#customers"

# DuckDB
tinyetl "products.duckdb#inventory" output.csv
tinyetl data.csv "analytics.duckdb#sales"

# Snowflake (key-pair auth; see "Snowflake" below)
tinyetl data.csv "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders"
tinyetl "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders" orders.parquet

# ODBC (any database with an ODBC driver; see "ODBC" below)
tinyetl data.csv "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost;Database=mydb;UID=sa;PWD=MyPass123;TrustServerCertificate=yes#orders"
tinyetl "odbc://Driver={PostgreSQL Unicode};Server=localhost;Database=mydb;UID=user;PWD=pass#public.orders" output.csv
```

#### Source Type Override

When using HTTP/HTTPS or SSH protocols, URLs may not always indicate the file format clearly (e.g., API endpoints, URLs with query parameters). Use the `--source-type` parameter to explicitly specify the format:

```bash
# API endpoint that returns CSV data
tinyetl "https://api.example.com/export?format=csv&limit=1000" output.json --source-type=csv

# Google Drive download (no file extension in URL)
tinyetl "https://drive.google.com/uc?id=FILE_ID&export=download" data.csv --source-type=csv

# SSH file without clear extension
tinyetl "ssh://user@server.com/data/export_20241107" output.parquet --source-type=json

# Local files usually don't need source-type (auto-detected from extension)
tinyetl data.csv output.json  # No --source-type needed
```

**Supported source types:** `csv`, `json`, `parquet`, `avro`, `xlsx` (and the other spreadsheet extensions above)

### Excel

```bash
tinyetl sales.xlsx sales.csv                 # first sheet
tinyetl "sales.xlsx#Q4 2024" q4.parquet      # a named sheet
tinyetl "postgresql://user:@host/db#orders" "orders.xlsx#Orders"   # write; the sheet defaults to Sheet1
```

- **Reading:** the first row is the header; blank header cells become `column_N` and repeated names get `_2`, `_3`, …. Rows with no values are skipped. The whole sheet is loaded into memory (the spreadsheet libraries read a sheet at once), so use CSV or Parquet for very large data.
- **Types:** Excel stores every number as a double: whole numbers are read as integers, others as decimals. Date cells are read as dates, booleans as booleans, and text cells stay text (so `02134` keeps its leading zero). Error cells (`#N/A`, `#DIV/0!`) are read as NULL.
- **Writing:** produces an `.xlsx` with a bold header row; dates get a date format, and integers beyond ±2^53, which a double can't hold exactly, are written as text. Rows are streamed to a temporary file, so memory stays low. A sheet holds at most 1,048,576 rows; larger exports fail with a message. Writing replaces an existing file (it doesn't add a sheet to it).
- **S3 and HTTP:** `s3://bucket/report.xlsx` works as a source and a target; HTTP(S) URLs ending in a spreadsheet extension, or with `--source-type xlsx`, work as sources.

### Database Connection Strings

TinyETL uses standard database connection URLs with an optional table specification using the `#` separator.

**PostgreSQL:**
```bash
# Basic format
postgresql://username:password@hostname:port/database#table_name

# Examples
tinyetl data.csv "postgresql://user:@localhost/mydb#customers"
tinyetl data.csv "postgresql://admin:@db.example.com:5432/analytics#sales_data"
```

**MySQL:**
```bash
# Basic format  
mysql://username:password@hostname:port/database#table_name

# Examples
tinyetl data.csv "mysql://user:@localhost:3306/mydb#customers"
tinyetl data.csv "mysql://admin:@db.example.com:3306/analytics#sales_data"

# Default table name is 'data' if not specified
tinyetl data.csv "mysql://user:@localhost:3306/mydb"  # Creates table named 'data'
```

**SQLite:**
```bash
# File path (table name inferred from filename without extension)
tinyetl data.csv output.db              # Creates table named 'output'
tinyetl data.csv /path/to/database.db   # Creates table named 'database'

# Explicit table name using connection string format
tinyetl data.csv "sqlite:///path/to/database.db#custom_table"
```

**SQL Server (native):**
```bash
# Basic format (username and password are required)
mssql://username:password@hostname:port/database#table_name

# Examples
tinyetl data.csv "mssql://sa:MyPass123@localhost:1433/mydb#customers"
tinyetl "mssql://sa:MyPass123@localhost:1433/mydb#orders" orders.csv
```
Tables are in the login's default schema (usually `dbo`); `schema.table` names aren't supported by the native connector yet. For other schemas, Windows authentication or other driver features, use `odbc://` with Microsoft's ODBC driver (below).

**CrateDB:**
```bash
# Basic format
postgresql://username:password@hostname:port/schema#table_name

# Examples
tinyetl data.csv "postgresql://user:@localhost/doc#customers"
tinyetl data.csv "postgresql://admin:@db.example.com:5432/analytics#sales_data"
```
TinyETL detects CrateDB and creates decimal columns as `NUMERIC(38,10)`, since CrateDB needs a precision and scale.

**DuckDB:**
```bash
# File path (table name inferred from filename without extension)
tinyetl data.csv output.duckdb              # Creates table named 'output'
tinyetl data.csv /path/to/analytics.duckdb   # Creates table named 'analytics'

# Explicit table name using # separator
tinyetl data.csv "analytics.duckdb#sales_data"

# Reading from DuckDB table
tinyetl "products.duckdb#inventory" output.csv
tinyetl "analytics.duckdb#daily_sales" report.parquet
```

**ODBC:**

`odbc://` works with any database that has an ODBC driver. Everything after `odbc://` up to the last `#` is passed to the driver unchanged, and the text after `#` is the table (`table`, or `schema.table`).

```bash
# SQL Server (Microsoft ODBC Driver 18)
tinyetl data.csv "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost,1433;Database=mydb;UID=sa;PWD=MyPass123;TrustServerCertificate=yes#customers"

# SQL Server with Windows Authentication
tinyetl data.csv "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost\MSSQLSERVER01;Database=master;Trusted_Connection=Yes;TrustServerCertificate=Yes#customers"

# PostgreSQL (psqlODBC)
tinyetl "odbc://Driver={PostgreSQL Unicode};Server=localhost;Port=5432;Database=mydb;UID=user;PWD=pass#public.orders" orders.parquet

# MySQL / MariaDB (MariaDB Connector/ODBC also works with MySQL servers)
tinyetl data.csv "odbc://Driver={MariaDB Unicode};Server=localhost;Port=3306;Database=mydb;UID=user;PWD=pass#sales"

# SQLite (SQLite ODBC driver)
tinyetl data.csv "odbc://Driver={SQLite3};Database=/path/to/file.db#customers"

# A DSN configured in odbc.ini, or a driver library given by path
tinyetl "odbc://DSN=warehouse;UID=user;PWD=pass#facts" facts.csv
tinyetl data.csv "odbc://Driver=/opt/homebrew/lib/psqlodbcw.so;Server=localhost;Database=mydb;UID=user;PWD=pass#customers"
```

How it works:
- **Reading** runs one `SELECT * FROM <table>` and streams the rows. Column types come from the driver, so integers, decimals, booleans and dates keep their types
- **Writing** creates the table if it doesn't exist, using column types for the database the driver reports: SQL Server, PostgreSQL, MySQL/MariaDB and SQLite get the same types as TinyETL's native connectors; other databases get standard ANSI SQL types (`VARCHAR(4000)`, `BIGINT`, `DECIMAL(38,10)`, `BOOLEAN`, `DATE`, `TIMESTAMP`). To control the types, create the table yourself first. Rows are inserted in batches with ODBC parameter arrays inside one transaction, committed at the end
- Identifiers are quoted the way the driver reports (`"name"` for most databases, `` `name` `` for MySQL)
- Text is exchanged with the driver as UTF-16, so any Unicode text works

Tested with SQL Server (ODBC Driver 18), PostgreSQL (psqlODBC), MySQL (MariaDB Connector/ODBC) and SQLite (SQLite ODBC Driver); see `test_env/odbc/`. Other databases (Oracle, DB2, Snowflake, …) should work through the ANSI fallback but haven't been tested.

**ODBC Driver Requirements:**

TinyETL includes the ODBC driver manager, so you only need the driver for your database. You do not need to install unixODBC.
- **Windows**: Drivers are typically pre-installed or available via vendor installers. TinyETL uses the driver manager built into Windows. Non-ASCII text on Windows hasn't been tested yet
- **macOS**: Install drivers via Homebrew (e.g. `brew install microsoft/mssql-release/msodbcsql18`, `brew install psqlodbc`). TinyETL finds drivers registered in Homebrew's `odbcinst.ini` automatically. Oracle's MySQL Connector/ODBC for macOS is built for the iODBC driver manager and needs `libiodbc` installed; the MariaDB driver doesn't
- **Linux**: Install drivers via your package manager (`apt install odbc-postgresql odbc-mariadb libsqliteodbc`, Microsoft's `msodbcsql18`). Drivers registered by file name in `/etc/odbcinst.ini`, as Debian and Ubuntu do, are found in `/usr/lib/<arch>/odbc/` when you name them with `Driver=`. For a DSN in `odbc.ini`, give its `Driver` as a full library path (or set `LD_LIBRARY_PATH=/usr/lib/<arch>/odbc`)
- Drivers and DSNs are read from `odbcinst.ini` / `odbc.ini` in `/etc`, or from the directory in the `ODBCSYSINI` environment variable if set. You can also skip registration and give the driver's library path: `Driver=/path/to/libdriver.so`

**Important ODBC Notes:**
- Always quote ODBC connection strings to prevent shell interpretation
- Put values containing `;` in braces, e.g. `PWD={pa;ss}`
- The database must exist before using TinyETL
- For SQL Server driver names: `ODBC Driver 18 for SQL Server` (latest), `ODBC Driver 17 for SQL Server`, or `SQL Server` (older). Driver 18 encrypts by default; add `TrustServerCertificate=yes` for servers with self-signed certificates

**Snowflake:**

```bash
# Format
snowflake://user[:password]@account/database/schema?warehouse=WH[&role=ROLE]#table

# Load a file into a table (created if it doesn't exist)
export SNOWFLAKE_PRIVATE_KEY_PATH=~/.snowflake/rsa_key.p8
tinyetl orders.csv "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders"

# Replace the table's rows instead of appending
tinyetl orders.csv "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders" --truncate

# Export a table (or schema.table / database.schema.table)
tinyetl "snowflake://LOADER@MYORG-MYACCOUNT/ANALYTICS/RAW?warehouse=LOAD_WH#orders" orders.parquet
```

- **Account:** the account identifier, `ORGNAME-ACCOUNTNAME` (or a legacy locator such as `xy12345.us-east-1`). `SELECT CURRENT_ORGANIZATION_NAME() || '-' || CURRENT_ACCOUNT_NAME()` shows it
- **Authentication:** use key-pair authentication; Snowflake is phasing out password-only sign-in, especially for service users. Point TinyETL at an unencrypted or passphrase-protected PKCS#8 private key with `SNOWFLAKE_PRIVATE_KEY_PATH` (and `SNOWFLAKE_PRIVATE_KEY_PASSPHRASE`), or with the `private_key_path` / `private_key_passphrase` options in a YAML config. Alternatively, put a password or programmatic access token in the URL, or pass it with `--source-secret-id` / `--dest-secret-id`. A key wins over a password
- **Loading:** rows are written to gzip CSV files, uploaded to the table's own stage (`@%table`, so no `CREATE STAGE` privilege is needed) and loaded with one `COPY INTO` at the end. The load, and the `DELETE` for `--truncate`, run in one transaction: if anything fails, the table keeps its old rows. Memory use stays flat, since each staged file (500,000 rows by default) is uploaded as soon as it's full
- **Azure and Google Cloud accounts:** TinyETL can only upload to stages on AWS-hosted accounts so far. On other clouds it detects this and loads with batched `INSERT` statements instead, which works but is slower and uses more warehouse time. Reading works on every cloud
- **Reading** streams the result of `SELECT * FROM <table>`. NUMBER, FLOAT, BOOLEAN, DATE, TIMESTAMP and VARIANT/OBJECT/ARRAY (as JSON) keep their types; other types are read as text
- **Names:** plain names (letters, digits, `_`, `$`) are left unquoted, so Snowflake stores them upper-case: a CSV column `customer_id` becomes `CUSTOMER_ID`, which you can query as `customer_id`, and reading the table back gives `CUSTOMER_ID`. Names with other characters are quoted and keep their exact case
- **Column types for new tables:** `VARCHAR`, `NUMBER(38,0)`, `NUMBER(38,10)`, `BOOLEAN`, `DATE`, `TIMESTAMP_NTZ` (TinyETL's timestamps are UTC) and `VARIANT` for JSON. To choose your own types, create the table first

YAML options for Snowflake endpoints (under `source.options` / `target.options`):

| Option | Default | Meaning |
|---|---|---|
| `private_key_path` | `SNOWFLAKE_PRIVATE_KEY_PATH` | Private key for key-pair auth |
| `private_key_passphrase` | `SNOWFLAKE_PRIVATE_KEY_PASSPHRASE` | Passphrase of an encrypted key |
| `warehouse`, `role` | from the URL | Override the URL's values |
| `load_method` | `auto` | `auto` (stage on AWS, INSERT elsewhere), `stage` or `insert` |
| `stage_file_rows` | `500000` | Rows per staged file |

See [`examples/22_snowflake`](examples/22_snowflake/) for a complete setup, from creating a key pair to a test run.

**Important Notes:**
- Tables are created if they don't exist; the database (and schema) must already exist
- Existing tables are appended to; `--truncate` replaces their rows
- DuckDB is optimized for analytical (OLAP) workloads and offers better performance than SQLite for aggregations
- Connection strings should be quoted to prevent shell interpretation
- Default ports: PostgreSQL (5432), CrateDB (5432), MySQL (3306)

### Secure Password Management

TinyETL provides secure methods for handling database passwords to avoid exposing sensitive credentials in command-line arguments or shell history.

#### Environment Variables (Recommended)

Instead of including passwords directly in connection strings, use environment variables and use the `--source-secret-id` and `--dest-secret-id` parameters:

```bash
# Set custom environment variables
export TINYETL_SECRET_prod_db="production_password"
export TINYETL_SECRET_dev_db="development_password"

# Use explicit secret IDs
tinyetl "postgres://user@prod.example.com/db#orders" \
        "mysql://user@dev.example.com:3306/testdb#orders" \
        --source-secret-id prod_db \
        --dest-secret-id dev_db
```

**How it works:** `--source-secret-id ID` reads the environment variable `TINYETL_SECRET_ID` and sets it as the password in the source URL (`--dest-secret-id` does the same for the target). The ID is any name you choose, such as `prod_db`. It works for URL-style connection strings (`postgres://`, `mysql://`, `mssql://`, `snowflake://`); for `odbc://`, put the password in the connection string or a DSN, and for S3 use the AWS variables.


#### Security Warnings

TinyETL automatically detects when passwords are included in CLI parameters and warns you:

```bash
# This will show a security warning:
tinyetl data.csv "mysql://user:password123@localhost/db#table"
WARN Warning: Using passwords in CLI parameters for target is insecure. Consider using --source-secret-id / --dest-secret-id.
```

**Security Best Practices:**
- ✅ Use environment variables for passwords
- ✅ Use explicit secret IDs for multiple environments
- ✅ Keep connection strings in CI/CD without embedded passwords
- ❌ Avoid putting passwords directly in connection strings
- ❌ Avoid storing passwords in shell scripts or command history

### Data Transformations

TinyETL supports powerful data transformations using Lua scripting during the ETL process. Transform, combine, filter, and modify data as it flows from source to target.

#### Using Inline Transformations

Use the `--transform` option with semicolon-separated expressions. **All original columns are automatically preserved**, and you can add new columns or override existing ones:

```bash
# Add a new column while keeping all existing columns
tinyetl users.csv output.db --transform "full_name=row.first_name .. ' ' .. row.last_name"

# Multiple transformations (original columns + new calculated columns)
tinyetl sales.csv output.db --transform "total=row.quantity * row.price; profit=total * 0.3; year=2024"

# Override existing columns and add new ones
tinyetl data.csv output.db --transform "email=string.lower(row.email); age_group=row.age < 30 and 'young' or 'mature'"
```

#### Using Lua Files

For complex transformations, create a Lua file with a `transform` function. **Note**: With Lua files, you have full control over which columns to include. Only columns explicitly returned are kept:

**transform.lua:**
```lua
function transform(row)
    -- Create result table
    local result = {}
    
    -- Explicitly copy fields you want to keep
    result.id = row.id
    result.created_at = row.created_at
    
    -- Create new calculated fields
    result.full_name = row.first_name .. ' ' .. row.last_name
    result.total_amount = row.quantity * row.unit_price
    result.discount = result.total_amount > 100 and result.total_amount * 0.1 or 0
    result.final_amount = result.total_amount - result.discount
    
    -- String processing
    if row.email then
        result.email_domain = row.email:match('@(.+)')
        result.is_business_email = result.email_domain:find('%.com$') and true or false
    end
    
    -- Date manipulation (dates come as RFC3339 strings)
    if row.birth_date then
        local year = tonumber(row.birth_date:match('^(%d%d%d%d)'))
        result.age = 2024 - year
        result.generation = year < 1980 and 'Gen X' or year < 1997 and 'Millennial' or 'Gen Z'
    end
    
    return result
end
```

```bash
tinyetl data.csv output.db --transform-file transform.lua
```

#### YAML Configuration (New Format)

Starting with version 0.9.0, TinyETL supports an enhanced YAML format for transformations with better type safety and clarity. The new format uses a tagged union structure:

```yaml
# Multi-line script (best for complex logic)
transform:
  type: script
  value: |
    full_name = row.first_name .. ' ' .. row.last_name
    total_amount = row.quantity * row.unit_price
    profit = total_amount * 0.3

# Inline expressions (simple one-liners)
transform:
  type: inline
  value: "total=row.qty * row.price; profit=total - row.cost"

# External Lua file (reusable transformations)
transform:
  type: file
  value: "transform.lua"


```

**Backwards Compatibility**: Old YAML configs without the `type` field are no longer supported.

#### Transformation Rules

1. **Schema Inference**: The output schema is determined by the first transformed row
2. **Column Preservation**: For inline expressions, all original columns are preserved by default
3. **Column Override**: Transformations can override existing columns with new values
4. **New Columns**: New columns returned by transform are added to the target schema  
5. **Column Filtering**: For Lua files, only columns returned by the transform function are kept
6. **Row Filtering**: Return `nil` or empty table `{}` from Lua functions to filter out rows
7. **Type Conversion**: Lua values are converted back to TinyETL types (see below)
8. **Error Handling**: Transformation errors stop the process with clear error messages

#### Available Data Types

- **Strings**: `"text"` or `'text'`
- **Numbers**: `42`, `3.14` (Lua integers become integers, other numbers become decimals)
- **Booleans**: `true`, `false`
- **Null**: `nil` (becomes NULL in target)
- **Dates**: Passed to Lua as RFC 3339 strings (`2024-01-15T10:30:00+00:00`), returned as strings
- **JSON**: Passed to Lua as JSON text

#### Lua Built-ins Available

- String functions: `string.find()`, `string.match()`, `string.gsub()`, concatenation with `..`
- Math functions: `math.floor()`, `math.ceil()`, `math.abs()`, basic operators
- Logic: `and`, `or`, `not`, conditional expressions
- Pattern matching with string methods

#### Row Filtering (Lua Files Only)

You can filter out rows by returning `nil` or an empty table `{}` from your transform function:

```lua
function transform(row)
    -- Filter out rows with missing data
    if not row.email or row.email == "" then
        return nil  -- Remove this row
    end
    
    -- Filter by conditions
    if row.age and row.age < 18 then
        return nil  -- Remove minors
    end
    
    -- Filter out test data
    if row.country ~= "United States" then
        return nil  -- Keep only US records
    end
    
    -- Transform and return the row
    row.full_name = row.first_name .. ' ' .. row.last_name
    return row
end
```

**Note**: Inline expressions (`--transform`) always preserve all rows. Row filtering only works with Lua files (`--transform-file`).

#### Examples

```bash
# Clean and standardize phone numbers
tinyetl contacts.csv clean_contacts.db --transform "clean_phone=row.phone:gsub('[^%d]', '')"

# Category mapping
tinyetl products.csv categorized.db --transform "category=row.price < 50 and 'budget' or row.price < 200 and 'standard' or 'premium'"

# Extract year from date and calculate age
tinyetl people.csv processed.db --transform "birth_year=tonumber(row.birth_date:match('^(%d%d%d%d)')); age=2024 - birth_year"

# Filter and transform with Lua file (removes invalid rows)
tinyetl messy_data.csv clean_data.db --transform-file filter.lua

# Preview transformations before applying
tinyetl data.csv output.db --transform "total=row.qty * row.price" --preview 5
```

#### Schema File Format

Create a YAML file defining your expected schema. Here's the complete format:

> **📝 Note on Schema Inference:** When TinyETL automatically infers schema from your data, all columns are marked as `nullable: true` by default. This is a safety measure for ETL operations—sample data may not represent all possible values in your dataset. If you need `NOT NULL` constraints or other strict validation, you must provide an explicit schema file.
>
> **How types are inferred:** column types come from the first 1,000 rows. A column whose values all have one type gets that type; whole and fractional numbers together (`84`, `29.85`) make a decimal column; any other mix makes a text column. Blank values (empty or whitespace only) don't count. While copying, every value is converted to its column's type: numbers in a text column become text, text that parses in a numeric, boolean, date or JSON column is converted, and blank values in non-text columns become NULL. A later value that can't be converted (`abc` in an integer column) is written unchanged where the target allows it (files, SQLite) and fails with the column name on typed databases such as PostgreSQL. Use a schema file to fix the types up front.

**schema.yaml:**
```yaml
columns:
  - name: "id"
    type: "Integer"
    nullable: false
    
  - name: "email"
    type: "String"
    nullable: false
    pattern: '^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$'
    
  - name: "age"
    type: "Integer" 
    nullable: true
    default: 0
    
  - name: "salary"
    type: "Decimal"
    nullable: true
    
  - name: "is_active"
    type: "Boolean"
    nullable: false
    default: true
    
  - name: "created_at"
    type: "DateTime"
    nullable: true
    pattern: '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}'
```

#### Supported Data Types

Type names are case-insensitive:

- **String** - Text values, supports regex pattern validation
- **Integer** - Whole numbers (64-bit signed)
- **Decimal** - Exact decimal numbers (`rust_decimal`, up to 28 significant digits)
- **Boolean** - `true` / `false`
- **Date**, **DateTime** - Dates and timestamps (values are UTC)
- **Json** - JSON values; text in a Json column is parsed as JSON

#### Schema Field Properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | String | ✅ | Column name (must match source data) |
| `type` | String | ✅ | Data type: String, Integer, Decimal, Boolean, Date, DateTime, Json |
| `nullable` | Boolean | ❌ | Allow null/empty values (default: true) |
| `pattern` | String | ❌ | Regex pattern for validation (Text/DateTime types) |
| `default` | Any | ❌ | Default value when field is missing or null |

#### Validation Behavior

**Type Checking:**
- Each value must already have the column's type. CSV values are typed as they're read: whole numbers, decimals, `true`/`false` and dates are recognized; anything else is text
- Text in a `Json` column is parsed as JSON
- A mismatch (`yes` in a Boolean column, `abc` in an Integer column) stops the transfer with the column name

**Null Handling:**
- Missing fields use `default` value if specified
- Null/empty values use `default` value if specified  
- If `nullable: false`, null values without defaults cause validation errors

**Pattern Validation:**
- Regex patterns applied to Text and DateTime fields
- Validation failures stop processing with clear error messages
- Write patterns in single quotes (`pattern: '^\d{4}$'`): in double-quoted YAML strings, `\d` is an invalid escape

**Error Handling:**
- Schema validation errors halt processing immediately
- Clear error messages indicate field name, expected type, and actual value
- Pattern validation failures show the expected regex pattern

#### Schema Examples

**Customer Data Schema:**
```yaml
columns:
  - name: "customer_id"
    type: "Integer"
    nullable: false
    
  - name: "full_name"
    type: "String"
    nullable: false
    
  - name: "email"
    type: "String"
    nullable: false
    pattern: '^[\w\.-]+@[\w\.-]+\.[a-zA-Z]{2,}$'
    
  - name: "phone"
    type: "String"
    nullable: true
    pattern: '^\+?[1-9]\d{1,14}$'  # E.164 format
    
  - name: "registration_date"
    type: "DateTime"
    nullable: false
    pattern: '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}'
    
  - name: "is_premium"
    type: "Boolean"
    nullable: false
    default: false
    
  - name: "credit_limit"
    type: "Decimal"
    nullable: true
    default: "0.00"
```

**Sales Data Schema:**
```yaml
columns:
  - name: "order_id"
    type: "String"
    nullable: false
    pattern: '^ORD-\d{8}$'  # Format: ORD-12345678
    
  - name: "quantity"
    type: "Integer"
    nullable: false
    default: 1
    
  - name: "unit_price"
    type: "Decimal" 
    nullable: false
    
  - name: "discount_rate"
    type: "Decimal"
    nullable: true
    default: "0.0"
    
  - name: "total_amount"
    type: "Decimal"
    nullable: false
    
  - name: "order_date"
    type: "DateTime"
    nullable: false
```

#### Integration with Transformations

Schema validation occurs **before** transformations, ensuring input data quality:

```bash
# Validate input, then transform 
tinyetl raw_sales.csv processed.db 
  --schema-file sales_schema.yaml 
  --transform "profit=row.total_amount * 0.3; order_year=tonumber(row.order_date:match('^(%d%d%d%d)'))"
```

**Validation Flow:**
1. **Load schema** from YAML file
2. **Validate each row** against schema (type checking, patterns, nullability)
3. **Apply defaults** for missing/null fields
4. **Apply transformations** to validated data
5. **Transfer** to target

#### Use Cases

- **Data Quality Assurance**: Ensure incoming data meets quality standards
- **API Integration**: Validate data from external APIs before processing
- **ETL Pipelines**: Enforce consistent schemas across pipeline stages
- **Data Contracts**: Define and enforce data format agreements
- **Migration Validation**: Ensure data integrity during system migrations

## YAML Configuration Files

For complex ETL jobs, TinyETL supports YAML configuration files that make your data pipelines more maintainable and version-controllable:

```bash
# Run ETL job from configuration file
tinyetl run config.yaml
```

### Benefits of Configuration Files

✅ **Clean, readable YAML format** - Better organization than long command lines  
✅ **Environment variable substitution** - Perfect for secrets and dynamic values  
✅ **Multi-line transformations** - Complex Lua transforms with proper formatting  
✅ **Version control friendly** - Store and share ETL job definitions  
✅ **Reusable configurations** - Same job across environments  

### Configuration File Format

```yaml
# This is a configuration file for TinyETL.
# It is in YAML format.
# Comments start with the '#' character and are ignored.
# Blank lines are ignored.
#
# To use this configuration:
# 1. Customize this configuration as needed.
# 2. Save it as 'config.yaml' or with a similar name of your choosing.
# 3. Use the configuration by running: `tinyetl run config.yaml`.
#
# You may generate a configuration file from CLI arguments using:
# `tinyetl generate-config [OPTIONS] <SOURCE> <TARGET> > config.yaml`
#
# Use `${VAR_NAME}` syntax to insert dynamic values from environment variables.
# For example, to use a database password from an environment variable:
#   uri: "postgres://user:${DB_PASSWORD}@localhost:5432/dbname"

version: 1

source:
  uri: "employees.csv"            # or database connection string
  # Optional source-specific options (e.g., for HTTP sources)
  # options:
  #   header.User-Agent: "TinyETL/0.11.0"
  #   header.Accept: "text/csv"
  #   auth.basic.username: "user"
  #   auth.basic.password: "${PASSWORD}"
  #   auth.bearer: "${BEARER_TOKEN}"

target:
  uri: "employees_output.json"    # or database connection string
  # Optional target-specific options
  # options:
  #   region: "us-east-1"         # e.g. S3 target (s3://bucket/path/file.parquet)
  #   access_key_id: "${AWS_ACCESS_KEY_ID}"
  #   secret_access_key: "${AWS_SECRET_ACCESS_KEY}"

# The "options" key and all other keys beneath "options" can be omitted.
# Sensible default values will be used for omitted keys.

options:
  batch_size: 10000               # Number of rows per batch
  infer_schema: true              # Auto-detect column types
  schema_file: "schema path.yaml" # Override with external schema
  preview: 10                     # Show N rows without transfer
  dry_run: false                  # Validate without transferring
  log_level: info                 # info, warn, error (lowercase in YAML)
  skip_existing: false            # Not implemented yet; has no effect
  source_type: "csv"              # Force source file type
  truncate: false                 # Truncate target before writing
  
  # Transform configuration supports multiple formats:
  
  # 1. Multi-line script (recommended for complex transformations)
  transform:
    type: script
    value: |
      -- Calculate derived fields
      full_name = row.first_name .. " " .. row.last_name
      annual_salary = row.monthly_salary * 12
      hire_year = tonumber(string.sub(row.hire_date, 1, 4))

  # 2. Inline single-line expressions (for simple transformations)
  # transform:
  #   type: inline
  #   value: "total=row.quantity * row.price; profit=total - row.cost"

  # 3. External Lua file (for reusable transformation logic)
  # transform:
  #   type: file
  #   value: "transform.lua"

  # 4. No transformation (default)
  # transform:
  #   type: none
```

### Source and Target Options

TinyETL supports connector-specific options that can be passed to sources and targets. These options are particularly useful for HTTP sources that require authentication or custom headers.

#### HTTP Source Options

When reading from HTTP/HTTPS URLs, you can specify authentication and custom headers:

```yaml
version: 1

source:
  uri: "http://api.example.com/data.csv"
  options:
    # Custom headers (prefix with "header.")
    header.User-Agent: "TinyETL/0.11.0"
    header.Accept: "text/csv"
    header.X-API-Version: "v2"
    
    # Basic authentication
    auth.basic.username: "myuser"
    auth.basic.password: "${API_PASSWORD}"
    
    # OR Bearer token authentication
    # auth.bearer: "${API_TOKEN}"

target:
  uri: "output.json"

options:
  batch_size: 1000
```

**Available HTTP Options:**
- `header.{HeaderName}` - Add custom HTTP headers (e.g., `header.User-Agent`, `header.Accept`)
- `auth.basic.username` - Username for HTTP Basic authentication
- `auth.basic.password` - Password for HTTP Basic authentication  
- `auth.bearer` - Bearer token for token-based authentication

**Security Best Practice:** Use environment variables for sensitive values like passwords and tokens:
```yaml
source:
  uri: "https://api.example.com/data"
  options:
    auth.bearer: "${API_TOKEN}"  # Read from environment variable
```

See [Example 18: HTTP with Authentication](examples/18_http_with_auth/) for complete working examples.

#### S3 Options

`s3://bucket/path/file.ext` works as both a source and a target for CSV, JSON, Parquet, Avro and Excel (`.xlsx`) files:

```bash
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_REGION=eu-west-1

tinyetl s3://my-bucket/raw/orders.csv orders.parquet
tinyetl orders.csv s3://my-bucket/exports/orders.parquet
```

Settings are read from the standard AWS environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_ENDPOINT`, `AWS_ALLOW_HTTP`). On AWS infrastructure, IAM roles (EC2 instance profiles, ECS task roles, web identity) are picked up automatically. In a YAML config, each endpoint can override them:

```yaml
version: 1

source:
  uri: "s3://my-bucket/raw/orders.csv"
  options:
    region: "eu-west-1"
    access_key_id: "${S3_ACCESS_KEY_ID}"
    secret_access_key: "${S3_SECRET_ACCESS_KEY}"

target:
  uri: "s3://other-bucket/exports/orders.parquet"
  options:
    endpoint: "https://<account-id>.r2.cloudflarestorage.com"   # S3-compatible stores
    access_key_id: "${R2_ACCESS_KEY_ID}"
    secret_access_key: "${R2_SECRET_ACCESS_KEY}"
```

**Available S3 Options:**
- `region` - AWS region of the bucket (defaults to `us-east-1`)
- `endpoint` - Custom endpoint URL for S3-compatible stores
- `access_key_id`, `secret_access_key`, `session_token` - Static credentials
- `allow_http` - Allow non-TLS endpoints (`true`/`false`; implied by an `http://` endpoint option)
- `path_style` - `true` for path-style requests (the default), `false` for virtual-hosted-style

**Behavior and limitations:**
- One object per source or target; prefixes and wildcards are not supported
- The file format comes from the object key's extension (sources can override it with `--source-type`)
- Data is staged in a local temporary file, so the machine needs free disk space for the object
- Writing replaces an existing object. The upload happens once the transfer has finished, so a failed run leaves the existing object untouched
- The credentials need `s3:ListBucket` on the bucket and `s3:GetObject` / `s3:PutObject` on its objects (the target checks whether the object exists before writing)
- Profiles in `~/.aws/credentials` / `~/.aws/config` and AWS SSO are not read; use environment variables instead
- `--source-secret-id` / `--dest-secret-id` do not apply to S3 URLs

See [Example 19: S3](examples/19_s3/) for a complete working example.

### Environment Variables

Use `${VAR_NAME}` syntax for dynamic values:

```yaml
version: 1

source:
  uri: "postgresql://${DB_USER}:${DB_PASSWORD}@${DB_HOST}:5432/mydb#users"

target:
  uri: "${OUTPUT_PATH}/processed_users.parquet"

options:
  batch_size: ${BATCH_SIZE:-5000}  # Default to 5000 if not set
  log_level: "${LOG_LEVEL:-info}"
```

```bash
# Set environment variables
export DB_USER="myuser"
export DB_PASSWORD="mypassword"
export DB_HOST="localhost"
export OUTPUT_PATH="/data/exports"

# Run with environment substitution
tinyetl run my_job.yaml
```

### Examples

**Simple file conversion:**
```yaml
# basic_job.yaml
version: 1
source:
  uri: "sales_data.csv"
target:
  uri: "sales_data.parquet"
options:
  batch_size: 50000
  infer_schema: true
```

**Database with transformations:**
```yaml
# etl_job.yaml  
version: 1
source:
  uri: "postgresql://user:${DB_PASSWORD}@host/db#raw_orders"
target:
  uri: "analytics.db#processed_orders"
options:
  truncate: true
  transform:
    type: script
    value: |
      -- Calculate order totals and profit margins
      total_amount = row.quantity * row.unit_price
      profit_margin = (total_amount - row.cost) / total_amount
      order_year = tonumber(string.sub(row.order_date, 1, 4))
```

Run any configuration file with:
```bash
tinyetl run my_job.yaml
```

#### Command Examples

```bash
# Basic schema validation
tinyetl customer_data.csv clean_customers.db --schema-file customer_schema.yaml

# Schema validation with preview
tinyetl large_import.csv target.db --schema-file validation.yaml --preview 5

# Combined schema validation and transformations
tinyetl sales_export.csv analytics.db 
  --schema-file sales_schema.yaml 
  --transform "profit_margin=(row.total_amount - row.cost) / row.total_amount"

# Validate API data before processing
tinyetl "https://api.example.com/export" local_data.db 
  --source-type=csv 
  --schema-file api_schema.yaml
```

### Sample Output

```
$ tinyetl sales.csv "sales.db#sales" --transform "total=row.amount * row.quantity"
INFO tinyetl::transfer: → Connecting to source: sales.csv
INFO tinyetl::transfer: → Connecting to target: sales.db#sales
INFO tinyetl::transfer: → Inferring schema from source...
INFO tinyetl::transfer: → 8 columns detected
INFO tinyetl::transfer: → Transformation enabled
INFO tinyetl::transfer: → Schema updated by transformations: 9 columns
INFO tinyetl::transfer: → Creating target table: sales
INFO tinyetl::transfer: → Copying 100000 rows
INFO tinyetl::transfer: → Done in 1.0s
INFO tinyetl: Transfer completed successfully!
INFO tinyetl: Processed 100000 rows in 0.98s (101586 rows/sec)
```

(Timestamps trimmed. A progress bar is shown while copying.)

### Performance

A 1,000,000-row, 8-column CSV (83 MB) converted on an Apple M4 Pro with the default batch size:

| Target | Time | Rows/sec | Peak memory |
|---|---|---|---|
| CSV | 2.6 s | 383k | 107 MB |
| DuckDB | 3.1 s | 331k | 185 MB |
| Parquet | 3.7 s | 289k | 1.4 GB |
| JSON | 4.5 s | 236k | 2.3 GB |
| Avro | 4.6 s | 217k | 1.3 GB |
| Excel (.xlsx) | 5.3 s | 190k | 107 MB |
| SQLite | 6.7 s | 151k | 118 MB |

Sources are read in batches, and CSV, Excel and database targets write each batch as it arrives, so their memory use stays flat. The Parquet, JSON and Avro targets currently hold all rows in memory until the end, so for very large exports prefer CSV or a database (or split the input). Network databases and warehouses depend mostly on the server and the network.

### Roadmap

**Done:**
- Files: CSV, JSON, Parquet, Avro, Excel ✅
- Databases: SQLite, DuckDB, PostgreSQL, MySQL, SQL Server, CrateDB, any ODBC driver ✅
- Snowflake (staged bulk loads) ✅
- Remote files over HTTP(S), SSH and S3 ✅
- Schema inference, schema files, Lua transforms, YAML jobs ✅

**Next:**
- Streaming Parquet, JSON and Avro writes (bounded memory)
- Databricks and OneLake
- `--skip-existing` (the flag exists but has no effect yet)
- Advanced transformation functions and libraries
- Multi-file processing with glob patterns
- Advanced schema mapping
- Data validation and quality checks

### Contributors

Special thanks to:
- Michael Descy @mjdescy (Excel)
- Chris Riccomini @criccomini
- Andreas Motl @amotl (CrateDB)

for contributing to TinyETL!


### License

This project is licensed under a modified Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

Commercial use is explicitly allowed with the exception of reselling TinyETL in a cloud SaaS format, please contact licensing@tinyetl.com for additional commercial licensing terms.

**Bundled third-party libraries:** to keep the binary free of runtime dependencies, the Linux and macOS binaries statically link the [unixODBC](https://www.unixodbc.org/) driver manager (2.3.14), licensed under the GNU LGPL v2.1 or later, the Linux binary also includes OpenSSL (Apache License 2.0), and every binary includes AWS-LC (ISC/Apache-2.0, used for TLS by the Snowflake client). They are not covered by TinyETL's license, and the commercial terms above do not apply to them. Their license texts ship with every download, and [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) explains where to get their source and how to rebuild TinyETL against a modified unixODBC.


---

Built with Rust for performance, safety, and reliability.
