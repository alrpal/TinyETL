# Example 16: CSV to MSSQL via ODBC

This example demonstrates how to transfer data from a CSV file to Microsoft SQL Server using the ODBC protocol with ODBC Driver 17 for SQL Server.

## Prerequisites

### 1. Install ODBC Driver 17 for SQL Server

On macOS:
```bash
brew tap microsoft/mssql-release https://github.com/Microsoft/homebrew-mssql-release
brew update
HOMEBREW_ACCEPT_EULA=Y brew install msodbcsql17
```

The driver will be installed at:
- Intel Macs: `/usr/local/lib/libmsodbcsql.17.dylib`
- Apple Silicon: `/opt/homebrew/lib/libmsodbcsql.17.dylib`

### 2. Start SQL Server Container

```bash
cd ../../test_env
docker-compose up -d mssql
```

Wait 30-60 seconds for the container to fully initialize.

### 3. Create the testdb Database

**IMPORTANT:** Before using tinyetl with ODBC, you must create the target database first:

```bash
docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "CREATE DATABASE testdb"
```

## Running the Example

```bash
cd examples/16_csv_to_mssql_odbc
./run.sh
```

Or manually:
```bash
tinyetl customers.csv "odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes#customers"
```

## ODBC Connection String Format

```
odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes#tablename
```

### Key Components:

- **Driver**: ODBC driver name (use `{ODBC Driver 17 for SQL Server}`)
- **Server**: Server address and port (format: `hostname,port`)
- **Database**: Target database name (must exist before running tinyetl)
- **UID**: User ID (username)
- **PWD**: Password
- **TrustServerCertificate**: Set to `yes` for local development (bypasses SSL certificate validation)
- **#tablename**: Table name (after the `#` separator)

## Common Issues

### Error: "Cannot open database 'testdb' requested by the login"

**Solution:** Create the database first:
```bash
docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "CREATE DATABASE testdb"
```

Or use the `master` database:
```bash
odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=master;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes#customers
```

### Error: "ODBC Driver 17 for SQL Server not found"

**Solution:** Install the driver (see Prerequisites above).

### Error: "Container not running"

**Solution:** Start the MSSQL container:
```bash
cd ../../test_env && docker-compose up -d mssql
```

## What Gets Transferred

- **Source:** `customers.csv` (6 rows)
- **Target:** MSSQL table `customers` via ODBC
- **Schema:** Auto-inferred from CSV headers
- **Table:** Automatically created if it doesn't exist

## Verification

After running, verify the data:
```bash
docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "USE testdb; SELECT * FROM customers"
```

## Comparison with Example 14

- **Example 14:** Uses native tiberius/tokio-based MSSQL connector
- **Example 16:** Uses ODBC driver for broader compatibility

Both achieve the same result, but ODBC provides:
- Cross-database compatibility (same code works for different databases)
- Industry-standard protocol
- More driver options
