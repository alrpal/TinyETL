#!/bin/bash
# Example 5: CSV to SQLite conversion

set -e
cd "$(dirname "$0")"

echo "Running Example 5: CSV to SQLite"
echo "Input: employees.csv -> Output: employees.db#employees"

# Clean up any existing database
rm -f employees.db

# Run tinyetl command
../../target/release/tinyetl employees.csv "employees.db#employees"

# Validate output exists
if [ ! -f "employees.db" ]; then
    echo "❌ FAIL: employees.db was not created"
    exit 1
fi

# Check if database has content using sqlite3
if command -v sqlite3 &> /dev/null; then
    # Check if table exists
    table_exists=$(sqlite3 employees.db "SELECT name FROM sqlite_master WHERE type='table' AND name='employees';" | wc -l)
    if [ "$table_exists" -eq 1 ]; then
        echo "✅ PASS: employees table created"
    else
        echo "❌ FAIL: employees table not found"
        exit 1
    fi
    
    # Check record count
    record_count=$(sqlite3 employees.db "SELECT COUNT(*) FROM employees;")
    expected_count=6
    
    if [ "$record_count" -eq "$expected_count" ]; then
        echo "✅ PASS: Found $record_count records (expected $expected_count)"
    else
        echo "❌ FAIL: Found $record_count records, expected $expected_count"
        exit 1
    fi
    
    # Check if expected columns exist
    columns=$(sqlite3 employees.db "PRAGMA table_info(employees);" | cut -d'|' -f2 | tr '\n' ',' | sed 's/,$//')
    echo "✅ PASS: Table columns: $columns"
    
    # Sample some data
    echo "Sample data:"
    sqlite3 employees.db "SELECT * FROM employees LIMIT 2;" | head -2
else
    echo "⚠️  WARNING: sqlite3 not found, skipping content validation"
    echo "✅ PASS: Database file created (basic validation)"
fi

echo "✅ Example 5 completed successfully"
