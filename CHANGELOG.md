# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Tab, semicolon and pipe delimited files** (#21). CSV sources detect the delimiter from the first lines (comma, tab, semicolon or pipe), `.tsv` files are read and written tab-separated, and the `delimiter` option sets it explicitly for sources and targets, including over HTTP, SSH and S3
- **Gzip and Zstandard compressed files** (#12). Add `.gz` or `.zst` to any file format (`data.csv.gz`, `events.jsonl.gz`, `trips.parquet.zst`): sources are decompressed to a temporary file with flat memory, targets are compressed when the transfer finishes. Works locally and over HTTP, SSH and S3. Multi-member gzip files (`cat a.gz b.gz`) are read whole
- **`.json` files that hold JSON Lines** (#16), such as GH Archive and the CrateDB example datasets. A `.json` file that starts with `{` is read as objects one after another, which also covers pretty-printed objects back to back and a single object
- `--source-option KEY=VALUE` and `--target-option KEY=VALUE` set endpoint options from the command line (HTTP headers and auth, S3 credentials, Snowflake keys, the CSV delimiter), which before could only be set in a YAML config. `generate-config` writes them into the YAML
- Example `24_database_correctness`: for SQLite, DuckDB, PostgreSQL, MySQL and SQL Server, checks that reads return every row exactly once and that a failed load (with or without `--truncate`) leaves the table unchanged. Run against v0.11.0, it reproduces all three database bugs fixed in 0.11.1
- Tests for the transfer engine's contract with targets (when tables are created, truncated, appended to and finalized; value conversion; transforms; schema-file rejection; preview and dry run), and a shared database test that SQLite and DuckDB run against temp files (typed round trip, streaming reads, rollback of unfinished loads, deferred truncate)

### Changed
- **Nested JSON objects and arrays stay JSON** (#3) instead of becoming strings: they load into `jsonb` in PostgreSQL (`JSON` in DuckDB, `VARIANT` in Snowflake) and stay nested when written to JSON. CSV, Parquet and other text targets get the same compact JSON text as before
- Errors print as plain text (`Error: Can't read config file job.yaml: No such file or directory`) instead of Rust's debug form (`Error: Configuration("…")`, `Error: Os { code: 2, … }`), and the config-file and schema-file errors name the file. Failed transfers end with the same `Error:` line instead of a timestamped log entry

### Fixed
- **SQLite and MySQL targets dropped values when rows had different keys.** They took the columns to insert from the first row of each batch, so with JSON sources whose objects have optional fields, a field missing from that first row was lost for the whole batch: loading a GH Archive hour into SQLite kept 0 of 3,245 `org` values. MySQL also wrote a missing value as an empty string instead of NULL, and SQL Server rejected such rows ("Row has 7 values but schema expects 8"). Every row is now written with the table's columns, and a missing key is NULL

## [0.11.1] - 2026-11-22

### Fixed
- **Reading from a database could silently return duplicated and missing rows.** PostgreSQL, MySQL, SQLite, DuckDB and SQL Server sources paged through tables with `LIMIT … OFFSET` and no `ORDER BY`, and the database is free to return rows in a different order for each page. Reading a 1M-row PostgreSQL table returned 1,000,000 rows but only 689,360 distinct ones (310,640 missing, 249,200 duplicated). Each source now runs one `SELECT` and streams the result, which also makes reads faster on large tables (1M rows: MySQL 10.4 s → 3.5 s, SQL Server 10.5 s → 2.8 s, PostgreSQL 3.4 s → 2.1 s) with flat memory
- **A failed load into a database left a partial table, and a failed `--truncate` load left it empty.** PostgreSQL, MySQL, SQLite, DuckDB and SQL Server targets now load in one transaction, with `--truncate`'s delete inside it, committed only when the transfer succeeds; on failure the table keeps its previous rows. (CrateDB has no transactions, so this doesn't apply there.) MySQL now empties the table with `DELETE` instead of `TRUNCATE`, which would commit implicitly
- **`--truncate` never worked on the native SQL Server target.** The check for an existing table always reported that it didn't exist, so rows were appended instead
- Reading DuckDB now keeps decimals, timestamps and dates as their types (they were read as text)
- SQLite `BLOB` values holding text (such as JSON written as bytes) were read as NULL; they are now read as text, and other binary values as hex ([#13](https://github.com/alrpal/TinyETL/issues/13))
- SQLite and DuckDB columns declared `JSON` are read as JSON, so they become `JSONB` in PostgreSQL, `VARIANT` in Snowflake, `JSON` in DuckDB and MySQL, and nested values in JSON output ([#13](https://github.com/alrpal/TinyETL/issues/13)). A SQLite `json` column is read as text if a sampled value isn't valid JSON (SQLite doesn't enforce it)
- SQLite declared types are matched the way SQLite does (by substring), so `VARCHAR(255)`, `DECIMAL(10,2)`, `BIGINT` and similar are recognised; they were all read as text
- A CSV with a header and no rows produced a schema with no columns, so loading it into a database failed (MySQL) or created an empty table; its columns are now kept as text columns

## [0.11.0] - 2026-11-21

### Added
- **Snowflake source and target** (`snowflake://user@account/database/schema?warehouse=WH#table`), replacing the mock `snowflake://` protocol
  - Writes rows as Zstandard-compressed CSV files to the table's own stage and loads them with one `COPY INTO`, in a transaction together with `--truncate`'s `DELETE`, so a failed load leaves the table unchanged
  - Key-pair authentication (`SNOWFLAKE_PRIVATE_KEY_PATH` or the `private_key_path` option, optionally passphrase-protected), or a password / programmatic access token
  - Reads stream `SELECT * FROM <table>` and keep NUMBER, FLOAT, BOOLEAN, DATE, TIMESTAMP and VARIANT/OBJECT/ARRAY types
  - Accounts on Azure or Google Cloud load with batched INSERTs (stage uploads are AWS-only so far); `load_method` forces either path
  - Built on the `firn` client crate, kept behind an internal interface so the client can be swapped later
- **Excel source and target** ([#11](https://github.com/alrpal/TinyETL/issues/11), [#23](https://github.com/alrpal/TinyETL/pull/23), contributed by Michael Descy)
  - Reads `.xlsx`, `.xlsm`, `.xlsb`, `.xls` and `.ods`; writes `.xlsx`. `file.xlsx#Sheet` picks the sheet to read or names the sheet to write
  - Keeps Excel's types: dates, booleans, whole numbers as integers, text cells as text (leading zeros survive)
  - Writes stream rows to a temporary file, with a bold header and date formats; exports over Excel's 1,048,576-row limit fail with a message
  - Works over S3 (`s3://bucket/file.xlsx`) and HTTP(S)
  - Both libraries (`calamine`, `rust_xlsxwriter`) are pure Rust, so the build still needs only a C/C++ compiler
- **JSON Lines** (`.jsonl`, `.ndjson`) as a source and target, including over S3
- Example `23_excel`: CSV → Excel → CSV round trip, named sheets, transforms, Excel → SQLite
- **CrateDB** as a source and target through `postgresql://` URLs ([#22](https://github.com/alrpal/TinyETL/pull/22), contributed by Andreas Motl)
- Example `22_snowflake`: load and read against a real account (skipped unless `SNOWFLAKE_*` variables are set)
- Example `21_csv_to_postgres_odbc`: a CSV → PostgreSQL → CSV round trip through the PostgreSQL ODBC driver
- Prebuilt **Linux ARM64** binary (`tinyetl-linux-aarch64`), built natively on GitHub's arm64 runner
- `test_env/odbc/run.sh`: tests the `odbc://` connector on Linux against SQL Server, PostgreSQL, MySQL and SQLite with their ODBC drivers
- **S3 Protocol**: `s3://bucket/path/file.ext` as both a source and a target for CSV, JSON, Parquet and Avro files
  - Works with Amazon S3 and S3-compatible stores (e.g. Cloudflare R2) via the `endpoint` option
  - Credentials from `AWS_*` environment variables, IAM roles, or per-endpoint YAML options (`region`, `endpoint`, `access_key_id`, `secret_access_key`, `session_token`, `allow_http`, `path_style`)
  - First remote protocol that supports writing: the output is staged locally and uploaded when the transfer completes
- `examples/19_s3`: round trip against a real bucket (skipped unless `TINYETL_S3_BUCKET` and AWS credentials are set)
- Example `20_csv_to_postgres`: a CSV → PostgreSQL → CSV round trip against the `test_env` Postgres container

### Fixed
- **Linux and macOS binaries no longer need unixODBC installed** ([#25](https://github.com/alrpal/TinyETL/issues/25)). The binaries failed to start on machines without `libodbc` (`error while loading shared libraries: libodbc.so.2`). The unixODBC driver manager is now compiled into the binary, so it has no runtime dependencies again
  - On macOS, drivers registered by Homebrew are still found automatically. Set `ODBCSYSINI` to use a different `odbcinst.ini` location
- **The Linux binary no longer needs OpenSSL installed.** It linked the system's `libssl.so.3`, so it failed to start on systems without it (such as `debian:12-slim`). OpenSSL is now compiled into the Linux binary; it still uses the system's CA certificates. macOS and Windows use the operating system's TLS libraries as before
- The Linux binary is now built on Ubuntu 22.04, so it runs on systems with glibc 2.35 or later (Ubuntu 22.04+, Debian 12+, Fedora 36+). Built on Ubuntu 24.04, it required glibc 2.38 and failed on Debian 12 and Ubuntu 22.04
- Reading from PostgreSQL or MySQL now keeps the table's column order. Previously the columns in the output file came out in a random order
- **`odbc://` works with any ODBC driver, not just SQL Server.** The connector used SQL Server-only SQL (`[name]` quoting, `OFFSET … FETCH` paging, `SET IMPLICIT_TRANSACTIONS ON`, multi-row `VALUES`), so it failed on other databases. It now quotes identifiers the way the driver reports, reads with a single streaming `SELECT`, uses standard ODBC transactions and parameter-array inserts, and picks `CREATE TABLE` column types for SQL Server, PostgreSQL, MySQL/MariaDB and SQLite (ANSI SQL types for other databases). Tested with SQL Server (ODBC Driver 18), PostgreSQL (psqlODBC), MySQL (MariaDB Connector/ODBC) and SQLite (SQLite ODBC)
  - Values read through ODBC keep their types (integers, decimals, booleans, dates) instead of all becoming text
  - Text is exchanged as UTF-16, so non-Latin characters survive (SQL Server stored them as `?`)
  - `schema.table` names are quoted part by part, and strings are no longer limited to `VARCHAR(255)` on SQL Server
  - A row was dropped at every batch boundary when reading through ODBC
  - On Debian and Ubuntu, drivers installed with `apt` (registered by file name, such as `Driver={PostgreSQL Unicode}`) can now be loaded; TinyETL resolves them to `/usr/lib/<arch>/odbc/`
  - TinyETL switches to a UTF-8 character locale when the environment has none (common in containers), which some drivers need to convert text
- Reading Parquet files with decimal, date, 8/16-bit integer, float32 or non-nanosecond timestamp columns replaced each value with the text `unsupported_type_row_N`. They now keep their values (types without a TinyETL equivalent are read as text)
- Reading a PostgreSQL table that hadn't been analyzed yet logged `Copying 18446744073709551615 rows`. TinyETL now falls back to an exact row count
- **Loading into PostgreSQL (and CrateDB) failed when a column mixed value types** ([#24](https://github.com/alrpal/TinyETL/issues/24)), with `invalid byte sequence for encoding "UTF8": 0x00` (CrateDB: `readerIndex: …, writerIndex: …`). Each value was sent with its own type; a column holding both `84` and `29.85` (stored as text) got integers and strings, and the prepared statement reused between batches then received binary integers where it expected text. Values are now converted to the column's type before they are written, for every target, and the PostgreSQL target sends every value, NULLs included, with the column's type. A value that can't be converted (`abc` in an integer column) fails with the column name and value
- Loading decimals into a PostgreSQL `NUMERIC` column failed (`column … is of type numeric but expression is of type text`); decimals are now sent as numbers
- Reading `NUMERIC`/`DECIMAL` columns from PostgreSQL, MySQL and SQL Server returned NULL for every value, as did PostgreSQL `JSON`/`JSONB` and SQL Server `INT`, `SMALLINT`, `TINYINT` and `REAL` columns. They now keep their values
- Writing Avro files with timestamp, date or JSON columns failed (`Cannot convert value Date(…) to Avro type String`): the writer took column types from the Avro schema's canonical form, which drops logical types. It now uses TinyETL's column types
- Reading Avro files was quadratic: each batch reopened the file and skipped every record already read (1M rows took 44 s; now 1.8 s). Avro timestamp and date fields are also read as dates instead of integers
- Reading Parquet with `--batch-size` below 1,024 dropped rows: each 1,024-row chunk was cut to the batch size and the rest discarded
- Writing Parquet failed for `Date` columns, and an empty source produced no Parquet file at all (now a valid file with the schema)
- Counting rows in a CSV for the progress bar read the whole file into memory; it now counts lines in chunks
- `--preview` printed values in Rust's debug format (`Integer(0)`, `String("ALGERIA…`), in a random column order, in fixed 15-character columns that broke the table's borders. It now prints plain values in the schema's column order, with columns sized to their contents
- `nullable` in a schema file is optional again and defaults to `true`, as documented (leaving it out was an error)
- Creating a decimal column on CrateDB failed (`NUMERIC storage is only supported if precision and scale are specified`). TinyETL detects CrateDB and creates `NUMERIC(38,10)` there

### Changed
- **Parquet, JSON and Avro now stream.** Rows are written a batch at a time instead of being held until the end, and files are read a batch at a time instead of being loaded whole. Converting a 10M-row (843 MB) CSV peaks at 32–70 MB for every file format; before, a 1M-row CSV took 1.3–2.3 GB to write as Parquet, JSON or Avro, and reading JSON loaded the whole file
- File targets write to a temporary file next to the destination and rename it into place when the transfer finishes, so a failed run leaves an existing file untouched (it used to leave a half-written file)
- Parquet files are written with Snappy compression (previously uncompressed; the 1M-row test file went from 55 MB to 19 MB)
- JSON output keys follow the column order instead of alphabetical order, and reading JSON keeps the file's key order for the columns
- JSON, JSON Lines and Avro sources count their rows with a quick scan before copying, so the progress bar shows for them as for CSV (Avro reads only block headers: 0.2 s for 10M rows; JSON about 1.5 s per 2 GB)
- **Schema inference: a column of whole and fractional numbers (`84`, `29.85`) is now a decimal column**; it used to become text. Blank values (`" "`) no longer decide a column's type, and are loaded as NULL in non-text columns (previously a single blank made the whole column text)
- Values are converted to their column's inferred type before writing. A column inferred as text now receives `84` as the string `"84"` (JSON output quotes it), and text that parses as the column's type is converted (`"12"` in an integer column becomes `12`)
- `--skip-existing` is documented as not implemented yet (it never had an effect); `tinyetl generate-default-config` now matches the README example
- CI also runs on pushes to `master`, and the README shows its real status instead of fixed "build passing" and "coverage" badges
- PostgreSQL decimal columns are created as `NUMERIC` (previously `DECIMAL`, the same type)
- Upgraded `arrow`/`parquet` from 57 to 58 and `object_store` from 0.12 to 0.13
- Upgraded `odbc-api` from 8.x to 30.x (required for the bundled unixODBC)
- Upgraded `duckdb` from 1.4.1 to 1.4.5, which builds with Visual Studio 2026 (MSVC 14.5x). 1.4.1's bundled `fmt` library used `stdext::checked_array_iterator`, which that compiler removed, so Windows builds failed
- **Building from source now requires Rust 1.94 or later** (`rust-version` was previously listed as 1.70, which the dependencies had already outgrown). TinyETL builds on current stable Rust again: `sqlx` 0.6 and `mlua` 0.8 no longer compiled on Rust 1.100, so they are upgraded to `sqlx` 0.9 and `mlua` 0.12
- Building from source on Linux no longer needs `libssl-dev`: sqlx's unused query macros are disabled, which built a second, non-vendored OpenSSL for the build host
- Release workflow checks that binaries link no third-party shared libraries and run in clean `ubuntu:22.04`, `debian:12-slim` and `ubuntu:24.04` containers
- Building from source no longer needs unixODBC installed (`unixodbc-dev` / `brew install unixodbc`): it is compiled from the vendored sources. A C/C++ compiler (plus `perl` on Linux, for OpenSSL) is all that's required

### Removed
- The mock `snowflake://` protocol and its examples (`03_snowflake_mock_source`, `04_snowflake_mock_target`); `snowflake://` now connects to Snowflake

### Licensing
- The Linux and macOS binaries include unixODBC 2.3.14 under the GNU LGPL. The Linux binary also includes OpenSSL (Apache License 2.0). License texts ship with each download; see `THIRD_PARTY_LICENSES.md`. `LICENSE` now states that its additional commercial terms do not apply to these bundled libraries

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

[Unreleased]: https://github.com/alrpal/TinyETL/compare/v0.11.1...HEAD
[0.11.1]: https://github.com/alrpal/TinyETL/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/alrpal/TinyETL/compare/v0.10.0...v0.11.0
[0.5.0]: https://github.com/alrpal/TinyETL/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/alrpal/TinyETL/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/alrpal/TinyETL/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/alrpal/TinyETL/releases/tag/v0.3.0
