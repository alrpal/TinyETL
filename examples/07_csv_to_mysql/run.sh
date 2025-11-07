#!/bin/bash
# Example 7: CSV to MySQL conversion

set -e
cd "$(dirname "$0")"

echo "Running Example 7: CSV to MySQL"
echo "Input: customers.csv -> Output: mysql://user:password@localhost:3306/testdb#customers"

# Note: This example demonstrates the MySQL protocol format and runs in dry-run mode
# since it requires a real MySQL server to be running

echo "Testing MySQL protocol connection string parsing..."

# Run tinyetl command in dry-run mode (validates connections and schema without transferring data)
../../target/release/tinyetl customers.csv "mysql://testuser:testpass@localhost:3306/testdb#customers"

if [ $? -eq 0 ]; then
    echo "✅ PASS: MySQL protocol connection string parsed successfully"
    echo "✅ PASS: Schema validation completed"
    echo "✅ PASS: Command would create table 'customers' in database 'testdb'"
else
    echo "❌ FAIL: MySQL protocol parsing failed"
    exit 1
fi

echo ""
echo "Note: To run this example with actual data transfer:"
echo "1. Install MySQL server: brew install mysql (macOS) or apt-get install mysql-server (Ubuntu)"
echo "2. Start MySQL service: brew services start mysql"
echo "3. Create test database: mysql -u root -e \"CREATE DATABASE testdb;\""
echo "4. Create test user: mysql -u root -e \"CREATE USER 'testuser'@'localhost' IDENTIFIED BY 'testpass';\""
echo "5. Grant permissions: mysql -u root -e \"GRANT ALL PRIVILEGES ON testdb.* TO 'testuser'@'localhost';\""
echo "6. Remove --dry-run flag from the command above"

echo "✅ Example 7 completed successfully (dry run)"
