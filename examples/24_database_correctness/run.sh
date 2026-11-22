#!/bin/bash
# Example 24: database reads and loads are correct and all-or-nothing
#
# For each database (SQLite and DuckDB always; PostgreSQL, MySQL and SQL Server when their
# test_env containers are running):
#   1. Load ROWS generated rows, read them back, and check every row comes back exactly once
#      with the same values (reads must not page with LIMIT/OFFSET, which can duplicate and
#      skip rows)
#   2. A load that fails partway (a schema-file pattern rejects one row in the middle) must
#      leave the table unchanged, with and without --truncate
#   3. Append, --truncate, and --truncate with an empty source behave as documented

set -e
cd "$(dirname "$0")"

TINYETL=${TINYETL:-../../target/release/tinyetl}
COMPARE="python3 -I ../../test_env/odbc/compare.py"
ROWS=${ROWS:-300000}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "Running Example 24: database correctness ($ROWS rows)"

# Test data: a big file, a seed, a file with one bad row in the middle, an empty file
# The comment column makes the table larger than a quarter of PostgreSQL's default
# shared_buffers (32 MB); only then do its sequential scans start at varying positions, which
# is what made OFFSET paging return duplicated and missing rows
awk -v n="$ROWS" 'BEGIN { print "id,name,amount,created_at,comment";
    pad = sprintf("%120s", ""); gsub(/ /, "x", pad);
    for (i = 1; i <= n; i++)
        printf "%d,customer %d,%d.%02d,2024-%02d-%02d 12:%02d:%02d,%s %d\n", i, i, i % 10000, i % 100,
            (i % 12) + 1, (i % 28) + 1, i % 60, (i * 7) % 60, pad, i }' > "$WORK/big.csv"
printf 'id,note\n1,ok\n2,ok\n3,ok\n' > "$WORK/seed.csv"
awk 'BEGIN { print "id,note"; for (i = 1; i <= 10000; i++) printf "%d,%s\n", i, (i == 5000 ? "BAD" : "ok") }' > "$WORK/bad.csv"
sed 's/BAD/ok/' "$WORK/bad.csv" > "$WORK/good.csv"
printf 'id,note\n' > "$WORK/empty.csv"
printf "columns:\n  - name: id\n    type: Integer\n  - name: note\n    type: String\n    pattern: '^ok\$'\n" > "$WORK/schema.yaml"

FAILED=0
pass() { echo "  ✅ $*"; }
fail() { echo "  ❌ $*"; FAILED=1; }

# Rows in a table, by reading it back
count() {
    rm -f "$WORK/count.csv"
    $TINYETL "$1" "$WORK/count.csv" >/dev/null 2>&1 || { echo "error"; return; }
    echo $(($(wc -l < "$WORK/count.csv") - 1))
}

check() {
    local label=$1 base=$2
    echo ""
    echo "== $label"

    # 1. Read back a large table
    $TINYETL "$WORK/big.csv" "$base#tinyetl_ex24_read" --truncate >/dev/null 2>&1 || { fail "loading $ROWS rows"; return; }
    rm -f "$WORK/read.csv"
    $TINYETL "$base#tinyetl_ex24_read" "$WORK/read.csv" >/dev/null 2>&1 || { fail "reading the table"; return; }
    local distinct
    distinct=$(python3 -I -c 'import csv, sys; print(len({r["id"] for r in csv.DictReader(open(sys.argv[1]))}))' "$WORK/read.csv")
    local read_rows=$(($(wc -l < "$WORK/read.csv") - 1))
    if [ "$read_rows" = "$ROWS" ] && [ "$distinct" = "$ROWS" ] && $COMPARE --ignore-header-case "$WORK/big.csv" "$WORK/read.csv" id >/dev/null; then
        pass "read $ROWS rows, each exactly once, values match"
    else
        fail "read $read_rows rows, $distinct distinct (expected $ROWS)"
    fi

    # 2. A failed load leaves the table unchanged
    local t="$base#tinyetl_ex24_load"
    $TINYETL "$WORK/seed.csv" "$t" --truncate >/dev/null 2>&1 || true  # checked by the row count below
    $TINYETL "$WORK/bad.csv" "$t" --schema-file "$WORK/schema.yaml" --batch-size 1000 >/dev/null 2>&1 \
        && fail "the bad file should have failed"
    [ "$(count "$t")" = "3" ] && pass "failed append left the table unchanged" \
        || fail "failed append left $(count "$t") rows (expected 3)"
    $TINYETL "$WORK/bad.csv" "$t" --schema-file "$WORK/schema.yaml" --batch-size 1000 --truncate >/dev/null 2>&1 \
        && fail "the bad file should have failed"
    [ "$(count "$t")" = "3" ] && pass "failed --truncate load left the table unchanged" \
        || fail "failed --truncate load left $(count "$t") rows (expected 3)"

    # 3. Successful loads
    $TINYETL "$WORK/good.csv" "$t" --truncate --batch-size 1000 >/dev/null 2>&1 || true  # checked by the row count below
    [ "$(count "$t")" = "10000" ] && pass "--truncate replaced the rows" || fail "--truncate: $(count "$t") rows (expected 10000)"
    $TINYETL "$WORK/seed.csv" "$t" >/dev/null 2>&1 || true  # checked by the row count below
    [ "$(count "$t")" = "10003" ] && pass "append added 3 rows" || fail "append: $(count "$t") rows (expected 10003)"
    $TINYETL "$WORK/empty.csv" "$t" --truncate >/dev/null 2>&1 || true  # checked by the row count below
    [ "$(count "$t")" = "0" ] && pass "--truncate with an empty source emptied the table" \
        || fail "empty --truncate: $(count "$t") rows (expected 0)"
}

running() { docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1"; }

check "SQLite" "$WORK/test.db"
check "DuckDB" "$WORK/test.duckdb"
if running tinyetl-postgres; then
    check "PostgreSQL" "postgres://testuser:testpass@localhost:5432/testdb"
else
    echo ""; echo "⚠️  PostgreSQL skipped: the tinyetl-postgres container isn't running"
fi
if running tinyetl-mysql; then
    check "MySQL" "mysql://testuser:testpass@localhost:3306/testdb"
else
    echo ""; echo "⚠️  MySQL skipped: the tinyetl-mysql container isn't running"
fi
if running tinyetl-mssql; then
    check "SQL Server" "mssql://sa:TestPass123!@localhost:1433/master"
else
    echo ""; echo "⚠️  SQL Server skipped: the tinyetl-mssql container isn't running"
fi

echo ""
if [ "$FAILED" = 0 ]; then
    echo "✅ Example 24 completed successfully"
else
    echo "❌ Example 24 had failures"
    exit 1
fi
