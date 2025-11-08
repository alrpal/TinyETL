# TinyETL
**Fast, zero-config ETL in a single binary**

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/alrpal/tinyetl/actions)
[![Coverage](https://img.shields.io/badge/coverage-60%25-brightgreen)](https://github.com/alrpal/tinyetl/actions)
[![Version](https://img.shields.io/badge/version-0.1.0-blue)](https://github.com/alrpal/tinyetl/releases)
[![Rust Edition](https://img.shields.io/badge/rust-2021-orange)](https://doc.rust-lang.org/edition-guide/rust-2021/index.html)
[![Binary Size](https://img.shields.io/badge/binary-21MB-green)](https://github.com/alrpal/tinyetl/releases)

<!-- TODO: Add demo GIF here showing MySQL → Parquet or CSV → SQLite -->

Transform and move data between any format or database — **instantly**. No dependencies, no config files, just one command.

```bash
# MySQL → Parquet with inline transformation 
tinyetl "mysql://user:pass@host/db#orders" orders.parquet \
  --transform "total_usd=row.amount * row.exchange_rate"

# Stream 100k+ rows/sec from CSV → SQLite
tinyetl large_dataset.csv results.db --batch-size 50000

# Download & convert web data
tinyetl "https://api.data.gov/export.json" analysis.parquet
```

## Why TinyETL?

✅ **Single 21MB binary** — no dependencies, no installation headaches  
✅ **40-145k+ rows/sec streaming** — handles massive datasets efficiently  
✅ **Zero configuration** — automatic schema detection and table creation  
✅ **Lua transformations** — powerful data transformations  
✅ **Universal connectivity** — CSV, JSON, Parquet, Avro, MySQL, PostgreSQL, SQLite  
✅ **Cross-platform** — Linux, macOS, Windows ready

## Quick Install

**Download the binary** (recommended):
```bash
# Download latest release for your platform
curl -L https://github.com/alrpal/tinyetl/releases/latest/download/tinyetl-linux -o tinyetl
chmod +x tinyetl
```

**Or install with Cargo**:
```bash
cargo install tinyetl
```

**Verify installation**:
```bash
tinyetl --version  # Should show: tinyetl 0.1.0
```

## Get Started in 30 Seconds

```bash
# File format conversion (auto-detects schemas)
tinyetl data.csv output.parquet
tinyetl data.json analysis.db

# Database to database 
tinyetl "postgresql://user:pass@host/db#users" "mysql://user:pass@host/db#users"

# Transform while transferring
tinyetl sales.csv results.db --transform "profit=row.revenue - row.costs; margin=profit/revenue"

# Process large datasets efficiently  
tinyetl huge_dataset.csv output.parquet --batch-size 100000

# Download and convert web data
tinyetl "https://example.com/api/export" local_data.json --source-type=csv
```

## Usage
```
tinyetl [OPTIONS] <SOURCE> <TARGET>

Arguments:
  <SOURCE>  Source: file path, URL, or database connection string
  <TARGET>  Target: file path or database connection string

Key Options:
      --batch-size <N>           Rows per batch [default: 10000]
      --transform "<expressions>" Inline Lua transformations  
      --transform-file <file>    Lua transformation script
      --preview <N>              Preview N rows without transferring
      --dry-run                  Validate without transferring
      --source-type <type>       Force source type: csv, json, parquet, avro
  -h, --help                     Show all options
```

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
- **HTTP/HTTPS** - Download from web servers
  ```bash
  tinyetl "https://example.com/data.csv" output.parquet
  tinyetl "https://api.example.com/export" data.csv --source-type=csv
  ```
- **SSH/SCP** - Secure file transfer
  ```bash
  tinyetl "ssh://user@server.com/data/file.csv" output.json
  tinyetl "ssh://user@server.com:2222/remote/data.parquet" local.csv
  ```

**Protocol Features:**
- **file://** - Local file system (default for simple paths)
- **http://** and **https://** - Web downloads with progress tracking
- **ssh://** - Secure shell file transfer using SCP
- **--source-type** parameter for format override (useful for URLs without clear extensions)

#### Database Sources

**Supported Databases:**
- **SQLite** - Embedded database
- **PostgreSQL** - Advanced open-source database
- **MySQL** - Popular relational database

**Connection Examples:**
```bash
# SQLite
tinyetl "sqlite:///path/to/db.sqlite#table" output.csv
tinyetl data.csv "sqlite:///output.db#customers"

# PostgreSQL  
tinyetl "postgresql://user:pass@localhost/mydb#orders" output.parquet
tinyetl data.csv "postgresql://user:pass@localhost/mydb#customers"

# MySQL
tinyetl "mysql://user:pass@localhost:3306/mydb#products" output.json
tinyetl data.csv "mysql://user:pass@localhost:3306/mydb#sales"
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
tinyetl data.csv "postgresql://user:pass@localhost/mydb#customers"
tinyetl data.csv "postgresql://admin:secret@db.example.com:5432/analytics#sales_data"
```

**MySQL:**
```bash
# Basic format  
mysql://username:password@hostname:port/database#table_name

# Examples
tinyetl data.csv "mysql://user:pass@localhost:3306/mydb#customers"
tinyetl data.csv "mysql://admin:secret@db.example.com:3306/analytics#sales_data"

# Default table name is 'data' if not specified
tinyetl data.csv "mysql://user:pass@localhost:3306/mydb"  # Creates table named 'data'
```

**SQLite:**
```bash
# File path (table name inferred from filename without extension)
tinyetl data.csv output.db              # Creates table named 'output'
tinyetl data.csv /path/to/database.db   # Creates table named 'database'

# Explicit table name using connection string format
tinyetl data.csv "sqlite:///path/to/database.db#custom_table"
```

**Important Notes:**
- Table names are automatically created if they don't exist
- For MySQL, the database must exist before running TinyETL
- Connection strings should be quoted to prevent shell interpretation
- Default ports: PostgreSQL (5432), MySQL (3306)

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

For complex transformations, create a Lua file with a `transform` function. **Note**: With Lua files, you have full control over which columns to include - only columns explicitly returned are kept:

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

### Command Line Options

| Option | Description | Default |
|--------|-------------|---------|
| `--infer-schema` | Auto-detect columns and types | enabled |
| `--batch-size <BATCH_SIZE>` | Number of rows per batch | 10000 |
| `--preview <N>` | Show first N rows and inferred schema without copying | - |
| `--dry-run` | Validate source/target without transferring data | disabled |
| `--log-level <LOG_LEVEL>` | Log level: info, warn, error | info |
| `--skip-existing` | Skip rows already in target if primary key detected | disabled |
| `--transform-file <FILE>` | Path to Lua file containing a 'transform' function | - |
| `--transform <EXPRESSIONS>` | Inline transformation expressions (semicolon-separated) | - |
| `-h, --help` | Print help | - |
| `-V, --version` | Print version | - |

### Examples

```bash
# Basic CSV to SQLite transfer
tinyetl sales_data.csv analytics.db

# JSON array to CSV conversion
tinyetl user_data.json users.csv

# Preview large dataset before transfer
tinyetl large_dataset.csv target.db --preview 5

# Transfer with verbose logging
tinyetl source.csv target.db --log-level info

# Validate connections without transferring
tinyetl remote_data.csv local.db --dry-run

# Apply transformations during transfer
tinyetl raw_sales.csv processed.db --transform "revenue=row.quantity * row.price; profit_margin=revenue * 0.3"

# Complex data cleaning with Lua file
tinyetl messy_data.csv clean.db --transform-file cleanup.lua --preview 3

# Load Parquet files to PostgreSQL
tinyetl large_dataset.parquet "postgresql://user:pass@localhost/db#table"

# Load CSV to MySQL
tinyetl customers.csv "mysql://user:pass@localhost:3306/mydb#customers"

# Transfer between MySQL databases
tinyetl "mysql://user1:pass1@host1:3306/sourcedb#sales" "mysql://user2:pass2@host2:3306/targetdb#sales_backup"

# Convert CSV to Parquet format
tinyetl data.csv output.parquet

# Convert CSV to Avro format
tinyetl data.csv output.avro

# Load Avro file to PostgreSQL
tinyetl data.avro "postgresql://user:pass@localhost/db#table"

# Transfer between PostgreSQL databases
tinyetl "postgresql://source_db/table" "postgresql://target_db/new_table"

# JSON to Parquet with transformations
tinyetl raw_events.json analytics.parquet --transform "event_date=row.timestamp:match('^[^T]+'); user_id=tonumber(row.id)"
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
- Achieve transfer speeds of 40k+ rows per second for typical datasets
- Cross-platform compatibility (Linux, macOS, Windows)

### Roadmap

**MVP (Current Focus):**
- Core CSV, JSON, SQLite connectors ✅
- MySQL and PostgreSQL connectors ✅
- Schema inference ✅
- Lua-based data transformations ✅
- Batch processing ✅
- Basic CLI interface ✅

**Future Enhancements:**
- Advanced transformation functions and libraries
- Multi-file processing with glob patterns
- Configuration file support
- Advanced schema mapping
- Data validation and quality checks


### License

This project is licensed under a modified Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

Commercial use is explicitly allowed with the exception of reselling TinyETL in a cloud SaaS format, please contact licensing@tinyetl.com for additional commercial licensing terms.


---

Built with Rust for performance, safety, and reliability.
