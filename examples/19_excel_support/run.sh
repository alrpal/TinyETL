#!/bin/bash

# TinyETL Excel Connector Example
# Demonstrates reading from and writing to Excel files (.xlsx)

set -e

echo "=== TinyETL Excel Connector Example ==="
echo ""

# Navigate to the project root
cd "$(dirname "$0")/../.."

# Build if needed
if [ ! -f "target/release/tinyetl" ] && [ ! -f "target/debug/tinyetl" ]; then
    echo "Building TinyETL..."
    cargo build --release
fi

# Use release build if available, otherwise debug
TINYETL="./target/release/tinyetl"
if [ ! -f "$TINYETL" ]; then
    TINYETL="./target/debug/tinyetl"
fi

cd examples/19_excel_support

echo "Step 1: Creating sample CSV data"
echo "----------------------------------------------------------------------"
cat > employees.csv << 'EOF'
id,first_name,last_name,email,department,salary,hire_date,is_active
1,Alice,Johnson,alice@example.com,Engineering,95000,2020-03-15,true
2,Bob,Smith,bob@example.com,Sales,75000,2019-07-22,true
3,Carol,Williams,carol@example.com,Marketing,68000,2021-01-10,true
4,David,Brown,david@example.com,Engineering,88000,2018-11-05,true
5,Eve,Davis,eve@example.com,HR,72000,2022-02-28,false
EOF

echo "✓ Sample data created: employees.csv"
echo ""

echo "Step 2: CSV to Excel (.xlsx)"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.csv employees.xlsx"
echo ""
$TINYETL employees.csv employees.xlsx

if [ -f employees.xlsx ]; then
    echo "✓ Excel file created: employees.xlsx"
    ls -lh employees.xlsx
else
    echo "✗ Failed to create Excel file"
    exit 1
fi
echo ""

echo "Step 3: Excel to CSV (round-trip test)"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.xlsx employees_output.csv"
echo ""
$TINYETL employees.xlsx employees_output.csv

if [ -f employees_output.csv ]; then
    echo "✓ CSV created from Excel: employees_output.csv"
    echo ""
    echo "Preview of output:"
    head -n 5 employees_output.csv
else
    echo "✗ Failed to convert Excel to CSV"
    exit 1
fi
echo ""

echo "Step 4: Excel to Excel with sheet name"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.xlsx output.xlsx#EmployeeData"
echo ""
$TINYETL employees.xlsx "output.xlsx#EmployeeData"

if [ -f output.xlsx ]; then
    echo "✓ Excel file created with custom sheet: output.xlsx (sheet: EmployeeData)"
    ls -lh output.xlsx
else
    echo "✗ Failed to create Excel file with custom sheet"
    exit 1
fi
echo ""

echo "Step 5: Excel to JSON"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.xlsx employees.json"
echo ""
$TINYETL employees.xlsx employees.json

if [ -f employees.json ]; then
    echo "✓ JSON created from Excel: employees.json"
    echo ""
    echo "Preview of output:"
    head -n 10 employees.json
else
    echo "✗ Failed to convert Excel to JSON"
    exit 1
fi
echo ""

echo "Step 6: CSV to Excel with transformation"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.csv transformed.xlsx --transform \"full_name=row.first_name .. ' ' .. row.last_name; annual_bonus=row.salary * 0.1\""
echo ""
$TINYETL employees.csv transformed.xlsx \
    --transform "full_name=row.first_name .. ' ' .. row.last_name; annual_bonus=row.salary * 0.1"

if [ -f transformed.xlsx ]; then
    echo "✓ Transformed Excel file created: transformed.xlsx"
    ls -lh transformed.xlsx
    echo ""
    echo "Converting back to CSV to verify transformation:"
    $TINYETL transformed.xlsx transformed_output.csv
    echo "Preview:"
    head -n 3 transformed_output.csv
else
    echo "✗ Failed to create transformed Excel file"
    exit 1
fi
echo ""

echo "Step 7: Preview Excel data"
echo "----------------------------------------------------------------------"
echo "Command: tinyetl employees.xlsx output.json --preview 3"
echo ""
$TINYETL employees.xlsx dummy.json --preview 3
echo ""

echo "=== All Excel Examples Completed Successfully ==="
echo ""
echo "Files created:"
echo "  - employees.xlsx (Excel from CSV)"
echo "  - employees_output.csv (CSV from Excel)"
echo "  - output.xlsx (Excel with custom sheet name)"
echo "  - employees.json (JSON from Excel)"
echo "  - transformed.xlsx (Excel with transformations)"
echo ""
