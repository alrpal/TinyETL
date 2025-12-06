# TinyETL
**Fast, zero-config ETL in a single binary**

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/alrpal/tinyetl/actions)
[![Coverage](https://img.shields.io/badge/coverage-60%25-brightgreen)](https://github.com/alrpal/tinyetl/actions)
[![Version](https://img.shields.io/badge/version-0.10.0-blue)](https://github.com/alrpal/tinyetl/releases)
[![Rust Edition](https://img.shields.io/badge/rust-2021-orange)](https://doc.rust-lang.org/edition-guide/rust-2021/index.html)
[![Binary Size](https://img.shields.io/badge/binary-15MB-green)](https://github.com/alrpal/tinyetl/releases)

![TinyETL Demo](examples/tinyetl_preview2-normal-framerate.gif)

Transform and move data between any format or database **instantly**. No dependencies, no config files, just one command.

```bash
# MySQL → Parquet with inline transformation 
tinyetl "mysql://user:@host/db#orders" orders.parquet \
  --transform "total_usd=row.amount * row.exchange_rate"

# Stream 100k+ rows/sec from CSV → SQLite
tinyetl large_dataset.csv results.db --batch-size 50000

# Download & convert web data
tinyetl "https://api.data.gov/export.json" analysis.parquet
```

## Why TinyETL?

✅ **Single 15 MB binary** — no dependencies, no installation headaches  
✅ **180k+ rows/sec streaming** — handles massive datasets efficiently  
✅ **Zero configuration** — automatic schema detection and table creation (override with schema and config files in yaml)  
   *Note: Auto-inferred schemas default all columns to nullable for safety*

✅ **Lua transformations** — powerful data transformations  
✅ **Universal connectivity** — CSV, JSON, Parquet, Avro, MySQL, PostgreSQL, SQLite, CrateDB, DuckDB, MSSQL, ODBC. Coming soon: Snowflake, Databricks, OneLake

✅ **Cross-platform** — Linux, macOS, Windows ready

## Quick Install

**Download the binary** (recommended):

Visit the [releases page](https://github.com/alrpal/tinyetl/releases/latest) and download the appropriate binary for your platform:
- Linux x64, Linux ARM64
- macOS Intel, macOS Apple Silicon  
- Windows x64, Windows ARM64

<!--
**Linux x64:**
```bash
curl -L https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-linux-x64.tar.gz | tar xz
chmod +x tinyetl
sudo mv tinyetl /usr/local/bin/  # Optional: add to PATH
```

**Linux ARM64:**
```bash
curl -L https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-linux-arm64.tar.gz | tar xz
chmod +x tinyetl
sudo mv tinyetl /usr/local/bin/  # Optional: add to PATH
```

**macOS Intel:**
```bash
curl -L https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-macos-intel.tar.gz | tar xz
chmod +x tinyetl
sudo mv tinyetl /usr/local/bin/  # Optional: add to PATH
```

**macOS Apple Silicon:**
```bash
curl -L https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-macos-apple-silicon.tar.gz | tar xz
chmod +x tinyetl
sudo mv tinyetl /usr/local/bin/  # Optional: add to PATH
```

**Windows x64:**
```powershell
# Download and extract using PowerShell
Invoke-WebRequest -Uri "https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-windows-x64.zip" -OutFile "tinyetl.zip"
Expand-Archive -Path "tinyetl.zip" -DestinationPath "."
# Move tinyetl.exe to a directory in your PATH
```

**Windows ARM64:**
```powershell
# Download and extract using PowerShell  
Invoke-WebRequest -Uri "https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-windows-arm64.zip" -OutFile "tinyetl.zip"
Expand-Archive -Path "tinyetl.zip" -DestinationPath "."
# Move tinyetl.exe to a directory in your PATH
```
-->

**Or install with Cargo** (builds from source):
```bash
cargo install tinyetl
```

**Verify installation**:
```bash
tinyetl --version
```

## Get Started in 30 Seconds

```bash
# File format conversion (auto-detects schemas)
tinyetl data.csv output.parquet
tinyetl data.json analysis.db

# Database to database 
tinyetl "postgresql://user:@host/db#users" "mysql://user:@host/db#users"

# Transform while transferring
tinyetl sales.csv results.db --transform "profit=row.revenue - row.costs; margin=profit/revenue"

# Process large datasets efficiently  
tinyetl huge_dataset.csv output.parquet --batch-size 100000

# Download and convert web data
tinyetl "https://example.com/api/export" local_data.json --source-type=csv

# Run complex ETL jobs from configuration files
tinyetl run my_etl_job.yaml
```

## Usage
<div style="overflow-x: auto;">

```
Usage: tinyetl [OPTIONS] <SOURCE> <TARGET>

Direct Transfer:
  <SOURCE>  Source connection string (file path or connection string)
  <TARGET>  Target connection string (file path or connection string)

Options:
      --infer-schema             Auto-detect columns and types
      --schema-file <FILE>       Path to schema file (YAML) to override auto-detection
      --batch-size <BATCH_SIZE>  Number of rows per batch [default: 10000]
      --preview <N>              Show first N rows and inferred schema without copying
      --dry-run                  Validate source/target without transferring data
      --log-level <LOG_LEVEL>    Log level: info, warn, error [default: info]
      --skip-existing            Skip rows already in target if primary key detected
      --truncate                 Truncate target before writing (overrides append-first behavior)
      --transform-file <FILE>    Path to Lua file containing a 'transform' function
      --transform <EXPRESSIONS>  Inline transformation expressions (semicolon-separated, e.g., "new_col=row.old_col * 2; name=row.first .. ' ' .. row.last")
      --source-type <TYPE>       Force source file type (csv, json, parquet) - useful for HTTP URLs without clear extensions
      --source-secret-id <ID>    Secret ID for source password (resolves to TINYETL_SECRET_{id})
      --dest-secret-id <ID>      Secret ID for destination password (resolves to TINYETL_SECRET_{id})
  -h, --help                     Print help
  -V, --version                  Print version

Config File Mode:
  run <CONFIG_FILE>  Run ETL job from YAML configuration file
  
Config File Generation Modes:
  generate-config [OPTIONS] <SOURCE> <TARGET>  Generate a YAML configuration file from CLI arguments and output to STDOUT
  generate-default-config                      Generate a default YAML configuration example and output to STDOUT
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

# Download from web
tinyetl "https://example.com/data.csv" output.json
tinyetl "https://api.example.com/export" data.csv --source-type=csv

# Secure file transfer via SSH
tinyetl "ssh://user@server.com/data.csv" output.parquet

# Database operations
tinyetl data.csv "postgresql://user:pass@localhost/mydb#customers"
tinyetl data.csv "mysql://user:pass@localhost:3306/mydb#customers"
tinyetl "sqlite:///source.db#users" output.csv

# Data inspection and validation
tinyetl data.csv output.db --preview 10
tinyetl data.csv output.db --dry-run

# Advanced options
tinyetl data.csv output.db --batch-size 5000
tinyetl data.csv output.db --preview 10

# Transfer with custom batch size
tinyetl data.csv output.db --batch-size 5000

# Dry run to validate without transferring
tinyetl data.csv output.db --dry-run

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

**Protocol Features:**
- **file://** - Local file system (default for simple paths)
- **http://** and **https://** - Web downloads with progress tracking
  - Supports Basic and Bearer token authentication
  - Custom HTTP headers via YAML configuration
  - Environment variable substitution for secure credential management
- **ssh://** - Secure shell file transfer using SCP
- **--source-type** parameter for format override (useful for URLs without clear extensions)

#### Database Sources

**Supported Databases:**
- **SQLite** - Embedded database
- **PostgreSQL** - Advanced open-source database
- **MySQL** - Popular relational database
- **CrateDB** - Distributed analytical database optimized for OLAP workloads
- **DuckDB** - Embedded analytical database optimized for OLAP workloads
- **ODBC** - Universal database connectivity (SQL Server, Oracle, DB2, and more)

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

# CrateDB
tinyetl "postgresql://crate:@localhost/doc#orders" output.parquet
tinyetl data.csv "postgresql://crate:@localhost/doc#customers"

# DuckDB
tinyetl "products.duckdb#inventory" output.csv
tinyetl data.csv "analytics.duckdb#sales"

# ODBC - SQL Server (Windows with Trusted Auth)
tinyetl data.csv "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost\MSSQLSERVER01;Database=master;Trusted_Connection=Yes;TrustServerCertificate=Yes#customers"

# ODBC - SQL Server (with username/password)
tinyetl data.csv "odbc://Driver={SQL Server};Server=localhost;Database=mydb;UID=sa;PWD=MyPass123#orders"

# ODBC - Reading from SQL Server
tinyetl "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost;Database=master;Trusted_Connection=Yes#employees" output.csv

# DuckDB
tinyetl "products.duckdb#inventory" output.csv
tinyetl data.csv "analytics.duckdb#sales"
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

**Supported source types:** `csv`, `json`, `parquet`, `avro`

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

**CrateDB:**
```bash
# Basic format
postgresql://username:password@hostname:port/schema#table_name

# Examples
tinyetl data.csv "postgresql://user:@localhost/doc#customers"
tinyetl data.csv "postgresql://admin:@db.example.com:5432/analytics#sales_data"
```

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

**ODBC (Universal Database Connectivity):**

ODBC provides connectivity to SQL Server, Oracle, DB2, and many other databases. The connection string format varies by driver.

```bash
# SQL Server with Windows Authentication (Trusted Connection)
tinyetl data.csv "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost\MSSQLSERVER01;Database=master;Trusted_Connection=Yes;TrustServerCertificate=Yes#customers"

# SQL Server with SQL Authentication
tinyetl data.csv "odbc://Driver={SQL Server};Server=localhost;Database=mydb;UID=sa;PWD=MyPass123#orders"

# SQL Server - Reading data
tinyetl "odbc://Driver={ODBC Driver 18 for SQL Server};Server=localhost;Database=mydb;Trusted_Connection=Yes#sales" output.csv

# Oracle Database
tinyetl data.csv "odbc://Driver={Oracle ODBC Driver};Server=localhost:1521;Database=orcl;UID=user;PWD=pass#employees"

# IBM DB2
tinyetl data.csv "odbc://Driver={IBM DB2 ODBC DRIVER};Database=sample;Hostname=localhost;Port=50000;UID=db2admin;PWD=pass#customers"
```

**ODBC Driver Requirements:**
- **Windows**: Drivers typically pre-installed or available via vendor installers
- **macOS**: Install via Homebrew 
- **Linux**: Install via package manager 

**Important ODBC Notes:**
- Connection strings must be URL-encoded if they contain special characters
- The database must exist before using TinyETL
- Table names are specified after `#` separator
- Always quote ODBC connection strings to prevent shell interpretation
- For SQL Server driver names: `ODBC Driver 18 for SQL Server` (latest), `ODBC Driver 17 for SQL Server`, or `SQL Server` (older)

**Important Notes:**
- Table names are automatically created if they don't exist
- For MySQL and ODBC databases, the database must exist before running TinyETL
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

**Environment Variable Pattern:**
- Format: `TINYETL_SECRET_{protocol}_{type}` or `TINYETL_SECRET_{protocol}`
- Examples:
  - `TINYETL_SECRET_mysql_dest` - for MySQL destination connections
  - `TINYETL_SECRET_postgres_source` - for PostgreSQL and CrateDB source connections
  - `TINYETL_SECRET_mysql` - generic MySQL password (works for both source/dest)


#### Security Warnings

TinyETL automatically detects when passwords are included in CLI parameters and warns you:

```bash
# This will show a security warning:
tinyetl data.csv "mysql://user:password123@localhost/db#table"
Warning: Using passwords in CLI parameters is insecure. Consider using --source-secret-id / --dest-secret-id.
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
7. **Type Safety**: Lua values are automatically converted to appropriate SQL types
7. **Error Handling**: Transformation errors stop the process with clear error messages

#### Available Data Types

- **Strings**: `"text"` or `'text'`
- **Numbers**: `42`, `3.14` (integers become INTEGER, decimals become REAL)
- **Booleans**: `true`, `false`
- **Null**: `nil` (becomes NULL in target)
- **Dates**: Input as RFC3339 strings, can be manipulated as strings

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

**schema.yaml:**
```yaml
columns:
  - name: "id"
    type: "Integer"
    nullable: false
    
  - name: "email"
    type: "Text"
    nullable: false
    pattern: "^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
    
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
    pattern: "^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}"
```

#### Supported Data Types

- **Text** - String values, supports regex pattern validation
- **Integer** - Whole numbers (32-bit signed integers)  
- **Decimal** - High-precision decimal numbers using `rust_decimal`
- **Boolean** - True/false values (accepts: true, false, 1, 0, yes, no)
- **DateTime** - ISO 8601 date-time strings, supports pattern validation

#### Schema Field Properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | String | ✅ | Column name (must match source data) |
| `type` | String | ✅ | Data type: Text, Integer, Decimal, Boolean, DateTime |
| `nullable` | Boolean | ❌ | Allow null/empty values (default: true) |
| `pattern` | String | ❌ | Regex pattern for validation (Text/DateTime types) |
| `default` | Any | ❌ | Default value when field is missing or null |

#### Validation Behavior

**Type Conversion:**
- Automatic conversion from string representations to target types
- Boolean parsing: `"true"`, `"1"`, `"yes"` → `true`; `"false"`, `"0"`, `"no"` → `false`
- Integer parsing: `"123"` → `123`
- Decimal parsing: `"123.45"` → `123.45` (high precision)

**Null Handling:**
- Missing fields use `default` value if specified
- Null/empty values use `default` value if specified  
- If `nullable: false`, null values without defaults cause validation errors

**Pattern Validation:**
- Regex patterns applied to Text and DateTime fields
- Validation failures stop processing with clear error messages
- Use double backslashes `` for regex escapes in YAML

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
    type: "Text"
    nullable: false
    
  - name: "email"
    type: "Text"
    nullable: false
    pattern: "^[\w\.-]+@[\w\.-]+\.[a-zA-Z]{2,}$"
    
  - name: "phone"
    type: "Text"
    nullable: true
    pattern: "^\+?[1-9]\d{1,14}$"  # E.164 format
    
  - name: "registration_date"
    type: "DateTime"
    nullable: false
    pattern: "^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}"
    
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
    type: "Text"
    nullable: false
    pattern: "^ORD-\d{8}$"  # Format: ORD-12345678
    
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
  #   header.User-Agent: "TinyETL/0.10.0"
  #   header.Accept: "text/csv"
  #   auth.basic.username: "user"
  #   auth.basic.password: "${PASSWORD}"
  #   auth.bearer: "${BEARER_TOKEN}"

target:
  uri: "employees_output.json"    # or database connection string
  # Optional target-specific options
  # options:
  #   header.X-Custom: "value"

# The "options" key and all other keys beneath "options" can be omitted.
# Sensible default values will be used for omitted keys.

options:
  batch_size: 10000               # Number of rows per batch
  infer_schema: true              # Auto-detect column types
  schema_file: "schema path.yaml" # Override with external schema
  preview: 10                     # Show N rows without transfer
  dry_run: false                  # Validate without transferring
  log_level: info                 # info, warn, error (lowercase in YAML)
  skip_existing: false            # Skip if target exists
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
    header.User-Agent: "TinyETL/0.10.0"
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
→ Connecting to source: sales.csv
→ Inferring schema...
→ 8 columns detected
→ Transformation enabled
→ Connecting to target: local.db#sales
→ Schema updated by transformations: 12 columns
→ Copying 100,000 rows
████████████ 100% (145k rows/sec)
→ Done in 2.3s
```

### Performance Goals

- Handle datasets up to 5 million rows efficiently
- Maintain low memory footprint through streaming
- Achieve transfer speeds of 180k+ rows per second for typical datasets
- Cross-platform compatibility (Linux, macOS, Windows)

### Roadmap

**MVP (Current Focus):**
- Core CSV, JSON, SQLite connectors ✅
- MySQL and PostgreSQL connectors ✅
- Schema inference ✅
- Lua-based data transformations ✅
- Batch processing ✅
- Basic CLI interface ✅
- YAML configuration files ✅

**Future Enhancements:**
- Advanced transformation functions and libraries
- Multi-file processing with glob patterns
- Advanced schema mapping
- Data validation and quality checks

### Contributors

Special thanks to:
- Michael Descy @mjdescy 
- Chris Riccomini @criccomini 

for contributing to TinyETL!


### License

This project is licensed under a modified Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

Commercial use is explicitly allowed with the exception of reselling TinyETL in a cloud SaaS format, please contact licensing@tinyetl.com for additional commercial licensing terms.


---

Built with Rust for performance, safety, and reliability.
