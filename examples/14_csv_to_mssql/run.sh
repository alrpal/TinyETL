#!/bin/bash
# Example 14: CSV to MSSQL conversion

set -e
cd "$(dirname "$0")"

echo "Running Example 14: CSV to MSSQL"
echo "Input: customers.csv -> Output: mssql://testuser:testpass@localhost:1433/testdb#customers"

# Check if Docker is running and MSSQL container exists
if ! docker ps | grep -q "tinyetl-mssql"; then
    echo "⚠️  MSSQL container not running. Starting test environment..."
    echo "Run 'docker-compose -f ../../test_env/docker-compose.yml up -d mssql' to start MSSQL"
    echo ""
    echo "Testing MSSQL protocol connection string parsing in dry-run mode..."
    
    # Run tinyetl command in dry-run mode
    ../../target/release/tinyetl --dry-run customers.csv "mssql://testuser:testpass@localhost:1433/testdb#customers"
    
    if [ $? -eq 0 ]; then
        echo "✅ PASS: MSSQL protocol connection string parsed successfully"
        echo "✅ PASS: Schema validation completed"
        echo "✅ PASS: Command would create table 'customers' in database 'testdb'"
    else
        echo "❌ FAIL: MSSQL protocol parsing failed"
        exit 1
    fi
    
    echo ""
    echo "To run this example with actual data transfer:"
    echo "1. Start MSSQL container: cd ../../test_env && docker-compose up -d mssql"
    echo "2. Wait for container to be ready (about 30-60 seconds)"
    echo "3. Re-run this script"
else
    echo "✅ MSSQL container is running. Proceeding with data transfer..."
    
    # Wait a moment for container to be fully ready
    echo "Waiting for MSSQL to be ready..."
    sleep 5
    
    # Test connection first
    echo "Testing MSSQL connection..."
    if docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U testuser -P testpass -Q "SELECT 1" >/dev/null 2>&1; then
        echo "✅ MSSQL connection successful"
    else
        echo "❌ MSSQL connection failed. Container may still be initializing..."
        echo "Try waiting a bit longer and re-running the script."
        exit 1
    fi
    
    # Run actual data transfer
    echo "Transferring data to MSSQL..."
    ../../target/release/tinyetl customers.csv "mssql://testuser:testpass@localhost:1433/testdb#customers"
    
    if [ $? -eq 0 ]; then
        echo "✅ PASS: Data transfer completed successfully"
        
        # Verify the data was inserted
        echo "Verifying data in MSSQL..."
        ROW_COUNT=$(docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U testuser -P testpass -Q "SELECT COUNT(*) FROM testdb.dbo.customers" -h -1 | tr -d ' \r\n')
        
        if [ "$ROW_COUNT" = "6" ]; then
            echo "✅ PASS: All 6 rows successfully inserted into MSSQL"
            
            # Show sample data
            echo ""
            echo "Sample data from MSSQL table:"
            docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U testuser -P testpass -Q "SELECT TOP 3 customer_id, first_name, last_name, email FROM testdb.dbo.customers ORDER BY customer_id"
        else
            echo "❌ FAIL: Expected 6 rows, but found $ROW_COUNT"
            exit 1
        fi
    else
        echo "❌ FAIL: Data transfer failed"
        exit 1
    fi
fi

echo "✅ Example 14 completed successfully"
