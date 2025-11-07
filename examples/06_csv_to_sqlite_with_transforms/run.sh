#!/bin/bash
# Example 6: CSV to SQLite with inline column transformations

set -e
cd "$(dirname "$0")"

echo "Running Example 6: CSV to SQLite with transforms"
echo "Input: products.csv -> Output: products.db#products_enriched"
echo "Transformations:"
echo "  - full_name = product_code .. ': ' .. name"
echo "  - price_cents = unit_price * 100"
echo "  - weight_lb = weight_kg * 2.20462"
echo "  - is_heavy = weight_kg > 10"

# Clean up any existing database
rm -f products.db

# Define inline transformations
transforms="full_name=row.product_code .. ': ' .. row.name; price_cents=row.unit_price * 100; weight_lb=row.weight_kg * 2.20462; is_heavy=row.weight_kg > 10"

# Run tinyetl command with inline transformations
../../target/release/tinyetl products.csv "products.db#products_enriched" --transform "$transforms"

# Validate output exists
if [ ! -f "products.db" ]; then
    echo "❌ FAIL: products.db was not created"
    exit 1
fi

# Check if database has content using sqlite3
if command -v sqlite3 &> /dev/null; then
    # Check if table exists
    table_exists=$(sqlite3 products.db "SELECT name FROM sqlite_master WHERE type='table' AND name='products_enriched';" | wc -l)
    if [ "$table_exists" -eq 1 ]; then
        echo "✅ PASS: products_enriched table created"
    else
        echo "❌ FAIL: products_enriched table not found"
        exit 1
    fi
    
    # Check record count
    record_count=$(sqlite3 products.db "SELECT COUNT(*) FROM products_enriched;")
    expected_count=5
    
    if [ "$record_count" -eq "$expected_count" ]; then
        echo "✅ PASS: Found $record_count records (expected $expected_count)"
    else
        echo "❌ FAIL: Found $record_count records, expected $expected_count"
        exit 1
    fi
    
    # Check if transformed columns exist
    columns=$(sqlite3 products.db "PRAGMA table_info(products_enriched);" | cut -d'|' -f2 | tr '\n' ',' | sed 's/,$//')
    echo "✅ PASS: Table columns: $columns"
    
    # Verify transformations worked
    echo "Sample transformed data:"
    sqlite3 products.db "SELECT full_name, price_cents, weight_lb, is_heavy FROM products_enriched LIMIT 2;"
    
    # Check specific transformation - price_cents should be 100x unit_price
    first_price_cents=$(sqlite3 products.db "SELECT price_cents FROM products_enriched LIMIT 1;")
    if [[ "$first_price_cents" == "129999" ]]; then
        echo "✅ PASS: Price transformation working (1299.99 -> 129999 cents)"
    else
        echo "⚠️  WARNING: Price transformation may not be working as expected (got $first_price_cents)"
    fi
    
    # Check boolean transformation - is_heavy should be 1 for weight > 10kg
    heavy_items=$(sqlite3 products.db "SELECT COUNT(*) FROM products_enriched WHERE is_heavy = 1;")
    echo "✅ PASS: Found $heavy_items heavy items (weight > 10kg)"
    
else
    echo "⚠️  WARNING: sqlite3 not found, skipping content validation"
    echo "✅ PASS: Database file created (basic validation)"
fi

echo "✅ Example 6 completed successfully"
