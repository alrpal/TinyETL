#!/bin/bash
# Example 21: CSV -> PostgreSQL -> CSV through ODBC (psqlODBC driver)

set -e
cd "$(dirname "$0")"

TINYETL=../../target/release/tinyetl
OUTPUT=customers_export.csv

echo "Running Example 21: CSV to PostgreSQL and back, through ODBC"

if ! docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "tinyetl-postgres"; then
    echo "⚠️  SKIPPED: the tinyetl-postgres container is not running."
    echo "Start it with: cd ../../test_env && docker-compose up -d postgres"
    exit 0
fi

# The PostgreSQL ODBC driver: Homebrew (brew install psqlodbc) or Debian/Ubuntu (apt install odbc-postgresql)
DRIVER=""
for candidate in /opt/homebrew/lib/psqlodbcw.so /usr/local/lib/psqlodbcw.so /usr/lib/*/odbc/psqlodbcw.so /usr/lib64/psqlodbcw.so; do
    if [ -f "$candidate" ]; then DRIVER=$candidate; break; fi
done
if [ -z "$DRIVER" ]; then
    echo "⚠️  SKIPPED: the PostgreSQL ODBC driver is not installed."
    echo "Install it with: brew install psqlodbc (macOS) or sudo apt install odbc-postgresql (Debian/Ubuntu)"
    exit 0
fi
echo "Driver: $DRIVER"

ODBC="odbc://Driver=$DRIVER;Server=localhost;Port=5432;Database=testdb;UID=testuser;PWD=testpass"
psql_query() { docker exec tinyetl-postgres psql -U testuser -d testdb -tAc "$1"; }

# 1. Load the CSV through ODBC. --truncate keeps re-runs from appending duplicate rows.
echo ""
echo "Step 1: Loading customers.csv into PostgreSQL through ODBC..."
$TINYETL customers.csv "$ODBC#customers_odbc" --truncate

CSV_ROWS=$(tail -n +2 customers.csv | grep -c .)
ROW_COUNT=$(psql_query "SELECT COUNT(*) FROM customers_odbc")
if [ "$ROW_COUNT" != "$CSV_ROWS" ]; then
    echo "❌ FAIL: expected $CSV_ROWS rows in customers_odbc, found $ROW_COUNT"
    exit 1
fi
echo "✅ PASS: $ROW_COUNT rows in table customers_odbc"

STORED=$(psql_query "COPY (SELECT customer_id, first_name, last_name, email, phone,
    to_char(registration_date, 'YYYY-MM-DD'), status FROM customers_odbc ORDER BY customer_id)
    TO STDOUT WITH (FORMAT csv)")
if [ "$STORED" != "$(tail -n +2 customers.csv)" ]; then
    echo "❌ FAIL: data stored in PostgreSQL does not match customers.csv"
    diff <(tail -n +2 customers.csv) <(echo "$STORED") || true
    exit 1
fi
echo "✅ PASS: stored values match customers.csv"

# 2. Read it back through ODBC
echo ""
echo "Step 2: Exporting customers_odbc back to CSV through ODBC..."
rm -f "$OUTPUT"
$TINYETL "$ODBC#customers_odbc" "$OUTPUT"

EXPORT_ROWS=$(tail -n +2 "$OUTPUT" | grep -c .)
if [ "$EXPORT_ROWS" != "$CSV_ROWS" ]; then
    echo "❌ FAIL: expected $CSV_ROWS rows in $OUTPUT, found $EXPORT_ROWS"
    exit 1
fi
if [ "$(head -1 "$OUTPUT")" != "$(head -1 customers.csv)" ]; then
    echo "❌ FAIL: $OUTPUT columns are not in the table's order"
    exit 1
fi
echo "✅ PASS: exported $EXPORT_ROWS rows to $OUTPUT, columns in table order"

echo ""
echo "✅ Example 21 completed successfully"
