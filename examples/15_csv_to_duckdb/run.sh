#!/bin/bash
# Example 15: CSV to DuckDB and then DuckDB to JSON

set -e
cd "$(dirname "$0")"

echo "=========================================="
echo "Example 15: DuckDB Source and Target"
echo "=========================================="

# Part 1: CSV to DuckDB
echo ""
echo "Part 1: CSV -> DuckDB"
echo "Input: products.csv -> Output: products.duckdb#products"

# Clean up any existing database
rm -f products.duckdb

# Run tinyetl command to write to DuckDB
../../target/release/tinyetl products.csv "products.duckdb#products"

# Validate output exists
if [ ! -f "products.duckdb" ]; then
    echo "❌ FAIL: products.duckdb was not created"
    exit 1
fi

echo "✅ PASS: DuckDB database created"

# Check if database has content using duckdb CLI (if available)
if command -v duckdb &> /dev/null; then
    # Check record count
    record_count=$(duckdb products.duckdb "SELECT COUNT(*) FROM products;" -csv -noheader)
    expected_count=6
    
    if [ "$record_count" -eq "$expected_count" ]; then
        echo "✅ PASS: Found $record_count records (expected $expected_count)"
    else
        echo "❌ FAIL: Found $record_count records, expected $expected_count"
        exit 1
    fi
    
    # Sample some data
    echo "Sample data from DuckDB:"
    duckdb products.duckdb "SELECT * FROM products LIMIT 2;" -box
else
    echo "⚠️  DuckDB CLI not installed, skipping validation checks"
    echo "   Install with: brew install duckdb"
fi

# Part 2: DuckDB to JSON
echo ""
echo "Part 2: DuckDB -> JSON"
echo "Input: products.duckdb#products -> Output: products_output.json"

# Clean up any existing output
rm -f products_output.json

# Run tinyetl command to read from DuckDB
../../target/release/tinyetl "products.duckdb#products" products_output.json

# Validate output exists
if [ ! -f "products_output.json" ]; then
    echo "❌ FAIL: products_output.json was not created"
    exit 1
fi

# Check if output has expected number of records
if command -v jq &> /dev/null; then
    record_count=$(jq length products_output.json)
    expected_count=6
    
    if [ "$record_count" -eq "$expected_count" ]; then
        echo "✅ PASS: Found $record_count records in JSON output (expected $expected_count)"
    else
        echo "❌ FAIL: Found $record_count records, expected $expected_count"
        exit 1
    fi
    
    # Check if specific fields exist
    if jq -e '.[0] | has("product_id") and has("product_name") and has("price")' products_output.json > /dev/null; then
        echo "✅ PASS: Output contains expected fields"
    else
        echo "❌ FAIL: Output missing expected fields"
        exit 1
    fi
    
    # Display sample output
    echo "Sample JSON output:"
    jq '.[0:2]' products_output.json
else
    echo "⚠️  jq not installed, skipping JSON validation"
fi

echo ""
echo "=========================================="
echo "✅ Example 15 completed successfully"
echo "=========================================="
echo ""
echo "Summary:"
echo "  1. Loaded CSV data into DuckDB database"
echo "  2. Read data from DuckDB and exported to JSON"
echo "  3. Verified data integrity throughout the pipeline"
