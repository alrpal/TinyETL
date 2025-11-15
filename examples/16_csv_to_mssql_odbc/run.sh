#!/bin/bash
# Example 16: CSV to MSSQL via ODBC Driver

set -e
cd "$(dirname "$0")"

echo "Running Example 16: CSV to MSSQL via ODBC"
echo "Input: customers.csv -> Output: odbc://DSN=...;UID=...;PWD=...#customers"

# Check if ODBC Driver 17 for SQL Server is installed
echo "Checking for ODBC Driver 17 for SQL Server..."
if [ ! -f "/opt/homebrew/lib/libmsodbcsql.17.dylib" ] && [ ! -f "/usr/local/lib/libmsodbcsql.17.dylib" ]; then
    echo "⚠️  ODBC Driver 17 for SQL Server not found."
    echo ""
    echo "To install on macOS:"
    echo "  brew tap microsoft/mssql-release https://github.com/Microsoft/homebrew-mssql-release"
    echo "  brew update"
    echo "  HOMEBREW_ACCEPT_EULA=Y brew install msodbcsql17"
    echo ""
    echo "Testing ODBC protocol connection string parsing in dry-run mode..."
    
    # Run tinyetl command in dry-run mode with ODBC connection string
    ../../target/release/tinyetl --dry-run customers.csv "odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=TestPass123!#customers"

    if [ $? -eq 0 ]; then
        echo "✅ PASS: ODBC protocol connection string parsed successfully"
        echo "✅ PASS: Schema validation completed"
        echo "✅ PASS: Command would create table 'customers' via ODBC"
    else
        echo "❌ FAIL: ODBC protocol parsing failed"
        exit 1
    fi
    
    echo ""
    echo "To run this example with actual data transfer:"
    echo "1. Install ODBC Driver 17 for SQL Server (see instructions above)"
    echo "2. Start MSSQL container: cd ../../test_env && docker-compose up -d mssql"
    echo "3. Wait for container to be ready (about 30-60 seconds)"
    echo "4. Re-run this script"
    exit 0
fi

echo "✅ ODBC Driver 17 for SQL Server is installed"

# Check if Docker is running and MSSQL container exists
if ! docker ps | grep -q "tinyetl-mssql"; then
    echo "⚠️  MSSQL container not running. Starting test environment..."
    echo "Run 'docker-compose -f ../../test_env/docker-compose.yml up -d mssql' to start MSSQL"
    echo ""
    echo "Testing ODBC connection string parsing in dry-run mode..."
    
    # Run tinyetl command in dry-run mode
    ../../target/release/tinyetl --dry-run customers.csv "odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes#customers"

    if [ $? -eq 0 ]; then
        echo "✅ PASS: ODBC connection string parsed successfully"
        echo "✅ PASS: Schema validation completed"
        echo "✅ PASS: Command would create table 'customers' via ODBC"
    else
        echo "❌ FAIL: ODBC parsing failed"
        exit 1
    fi
    
    echo ""
    echo "To run this example with actual data transfer:"
    echo "1. Start MSSQL container: cd ../../test_env && docker-compose up -d mssql"
    echo "2. Wait for container to be ready (about 30-60 seconds)"
    echo "3. Re-run this script"
    exit 0
fi

echo "✅ MSSQL container is running. Proceeding with ODBC data transfer..."

# Wait a moment for container to be fully ready
echo "Waiting for MSSQL to be ready..."
sleep 5

# Test connection first
echo "Testing MSSQL connection..."
if docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "SELECT 1" >/dev/null 2>&1; then
    echo "✅ MSSQL connection successful"
else
    echo "❌ MSSQL connection failed. Container may still be initializing..."
    echo "Try waiting a bit longer and re-running the script."
    exit 1
fi

# Create the database if it doesn't exist
echo "Creating testdb database if it doesn't exist..."
docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "IF NOT EXISTS (SELECT * FROM sys.databases WHERE name = 'testdb') CREATE DATABASE testdb"

# Verify the database exists
echo "Verifying testdb database exists..."
DB_EXISTS=$(docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "SELECT name FROM sys.databases WHERE name = 'testdb'" -h -1 | tr -d ' \r\n' | grep -c "testdb" || echo "0")

if [ "$DB_EXISTS" = "0" ]; then
    echo "❌ FAIL: testdb database does not exist"
    exit 1
fi
echo "✅ testdb database exists"

# Run actual data transfer using ODBC
echo "Transferring data to MSSQL via ODBC..."
echo "Using connection string: Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=***;TrustServerCertificate=yes"

../../target/release/tinyetl customers.csv "odbc://Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;Database=testdb;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes#customers"

if [ $? -eq 0 ]; then
    echo "✅ PASS: Data transfer via ODBC completed successfully"
    
    # Verify the data was inserted
    echo "Verifying data in MSSQL..."
    ROW_COUNT=$(docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "USE testdb; SELECT COUNT(*) FROM customers" -h -1 | tr -d ' \r\n' | tail -1)
    
    if [ "$ROW_COUNT" = "6" ]; then
        echo "✅ PASS: All 6 rows successfully inserted into MSSQL via ODBC"
        
        # Show sample data
        echo ""
        echo "Sample data from MSSQL table (via ODBC):"
        docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "USE testdb; SELECT TOP 3 customer_id, first_name, last_name, email FROM customers ORDER BY customer_id"
        
        echo ""
        echo "✅ PASS: ODBC connector successfully wrote data to MSSQL"
        echo "✅ PASS: Verified ODBC Driver 17 for SQL Server integration"
    else
        echo "❌ FAIL: Expected 6 rows, but found $ROW_COUNT"
        exit 1
    fi
else
    echo "❌ FAIL: Data transfer via ODBC failed"
    exit 1
fi

echo ""
echo "✅ Example 16 completed successfully"
echo ""
echo "This example demonstrated:"
echo "  - CSV to MSSQL data transfer using ODBC protocol"
echo "  - ODBC Driver 17 for SQL Server integration"
echo "  - Connection string format: odbc://Driver={...};Server=...;Database=...;UID=...;PWD=...#table"
echo "  - Automatic table creation via ODBC"
echo "  - Data insertion and verification"
