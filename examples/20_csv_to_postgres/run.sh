#!/bin/bash
# Example 20: CSV -> PostgreSQL -> CSV round trip

set -e
cd "$(dirname "$0")"

TINYETL=../../target/release/tinyetl
PG_URL="postgres://testuser:testpass@localhost:5432/testdb"
OUTPUT=customers_export.csv
EMPLOYEES_OUTPUT=employees_export.csv

echo "Running Example 20: CSV to PostgreSQL and back"
echo "Input: customers.csv -> $PG_URL#customers -> $OUTPUT"

if ! docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "tinyetl-postgres"; then
    echo "⚠️  SKIPPED: the tinyetl-postgres container is not running."
    echo "Start it with: cd ../../test_env && docker-compose up -d postgres"
    exit 0
fi

psql_query() { docker exec tinyetl-postgres psql -U testuser -d testdb -tAc "$1"; }

echo "Waiting for PostgreSQL to accept connections..."
for _ in $(seq 1 30); do
    if docker exec tinyetl-postgres pg_isready -U testuser -d testdb >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

# 1. Load the CSV. --truncate empties the table first if it already exists, so re-runs
#    give the same result instead of appending (TinyETL appends to existing tables by default).
echo ""
echo "Step 1: Loading customers.csv into PostgreSQL..."
$TINYETL customers.csv "$PG_URL#customers" --truncate

CSV_ROWS=$(tail -n +2 customers.csv | grep -c .)
ROW_COUNT=$(psql_query "SELECT COUNT(*) FROM customers")
if [ "$ROW_COUNT" != "$CSV_ROWS" ]; then
    echo "❌ FAIL: expected $CSV_ROWS rows in customers, found $ROW_COUNT"
    exit 1
fi
echo "✅ PASS: $ROW_COUNT rows in table customers"

# 2. Check the stored values, not just the count: dump the table with psql in the
#    input's column order and compare it to the input file.
STORED=$(psql_query "COPY (SELECT customer_id, first_name, last_name, email, phone,
    to_char(registration_date, 'YYYY-MM-DD'), status FROM customers ORDER BY customer_id)
    TO STDOUT WITH (FORMAT csv)")
if [ "$STORED" != "$(tail -n +2 customers.csv)" ]; then
    echo "❌ FAIL: data stored in PostgreSQL does not match customers.csv"
    diff <(tail -n +2 customers.csv) <(echo "$STORED") || true
    exit 1
fi
echo "✅ PASS: stored values match customers.csv"

# 3. Read the table back out with TinyETL.
echo ""
echo "Step 2: Exporting the customers table back to CSV..."
rm -f "$OUTPUT"
$TINYETL "$PG_URL#customers" "$OUTPUT"

EXPORT_ROWS=$(tail -n +2 "$OUTPUT" | grep -c .)
if [ "$EXPORT_ROWS" != "$CSV_ROWS" ]; then
    echo "❌ FAIL: expected $CSV_ROWS rows in $OUTPUT, found $EXPORT_ROWS"
    exit 1
fi
if [ "$(head -1 "$OUTPUT")" != "$(head -1 customers.csv)" ]; then
    echo "❌ FAIL: $OUTPUT columns are not in the table's order"
    echo "  expected: $(head -1 customers.csv)"
    echo "  got:      $(head -1 "$OUTPUT")"
    exit 1
fi
echo "✅ PASS: exported $EXPORT_ROWS rows to $OUTPUT, columns in table order"

# 4. Export a table TinyETL did not create: test_env seeds an employees table with
#    SERIAL, DECIMAL and DATE columns (test_env/init-scripts/postgres).
echo ""
echo "Step 3: Exporting the pre-seeded employees table..."
rm -f "$EMPLOYEES_OUTPUT"
$TINYETL "$PG_URL#employees" "$EMPLOYEES_OUTPUT"

EMPLOYEE_ROWS=$(psql_query "SELECT COUNT(*) FROM employees")
EXPORTED_EMPLOYEES=$(tail -n +2 "$EMPLOYEES_OUTPUT" | grep -c .)
if [ "$EXPORTED_EMPLOYEES" != "$EMPLOYEE_ROWS" ]; then
    echo "❌ FAIL: expected $EMPLOYEE_ROWS rows in $EMPLOYEES_OUTPUT, found $EXPORTED_EMPLOYEES"
    exit 1
fi
echo "✅ PASS: exported $EXPORTED_EMPLOYEES employees to $EMPLOYEES_OUTPUT"

echo ""
echo "Sample of $OUTPUT:"
head -4 "$OUTPUT"

echo ""
echo "✅ Example 20 completed successfully"
