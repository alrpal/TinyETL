#!/bin/bash
# Example 23: Excel (.xlsx) as a source and a target

set -e
cd "$(dirname "$0")"

echo "Running Example 23: Excel"

TINYETL=../../target/release/tinyetl
COMPARE="python3 -I ../../test_env/odbc/compare.py"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

FAILED=0
pass() { echo "✅ PASS: $*"; }
fail() { echo "❌ FAIL: $*"; FAILED=1; }

echo ""
echo "Step 1: CSV -> Excel -> CSV"
$TINYETL employees.csv "$WORK/employees.xlsx" >/dev/null
$TINYETL "$WORK/employees.xlsx" "$WORK/employees.csv" >/dev/null
if $COMPARE employees.csv "$WORK/employees.csv" id; then
    pass "values (decimals, dates, booleans, Unicode, a blank cell) and column order survive the round trip"
else
    fail "round trip differs from employees.csv"
fi

echo ""
echo "Step 2: a named sheet"
$TINYETL employees.csv "$WORK/book.xlsx#Staff" >/dev/null
$TINYETL "$WORK/book.xlsx#Staff" "$WORK/staff.json" >/dev/null
n=$(python3 -I -c 'import json, sys; print(len(json.load(open(sys.argv[1]))))' "$WORK/staff.json")
[ "$n" = "5" ] && pass "read 5 rows from sheet Staff" || fail "expected 5 rows from sheet Staff, got $n"
if err=$($TINYETL "$WORK/book.xlsx#Missing" "$WORK/missing.csv" 2>&1); then
    fail "reading a sheet that doesn't exist should fail"
elif echo "$err" | grep -q "Sheets: Staff"; then
    pass "a missing sheet fails and lists the sheets"
else
    fail "unexpected error for a missing sheet: $err"
fi

echo ""
echo "Step 3: transform while writing Excel"
$TINYETL employees.csv "$WORK/transformed.xlsx" \
    --transform "full_name=row.first_name .. ' ' .. row.last_name; bonus=row.salary * 0.1" >/dev/null
$TINYETL "$WORK/transformed.xlsx" "$WORK/transformed.csv" >/dev/null
first=$(python3 -I -c 'import csv, sys
r = next(csv.DictReader(open(sys.argv[1])))
print(r["full_name"], r["bonus"])' "$WORK/transformed.csv")
[ "$first" = "Alice Johnson 9500.05" ] && pass "computed columns: $first" \
    || fail "expected 'Alice Johnson 9500.05', got '$first'"

echo ""
echo "Step 4: Excel -> SQLite -> CSV"
$TINYETL "$WORK/employees.xlsx" "$WORK/staff.db#employees" >/dev/null
$TINYETL "$WORK/staff.db#employees" "$WORK/from_sqlite.csv" >/dev/null
if $COMPARE employees.csv "$WORK/from_sqlite.csv" id; then
    pass "Excel data loads into SQLite unchanged"
else
    fail "SQLite copy differs from employees.csv"
fi

echo ""
if [ "$FAILED" = 0 ]; then
    echo "✅ Example 23 completed successfully"
else
    echo "❌ Example 23 had failures"
    exit 1
fi
