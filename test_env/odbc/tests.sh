#!/bin/bash
# Runs inside the tinyetl-odbc-drivers container (see run.sh). For each database, sends
# typed.csv through odbc:// and checks the result by reading the table back with TinyETL.
#
# Expects: /dist/tinyetl (Linux build), /odbc (this directory), and the test_env database
# containers on the same Docker network. Tables named tinyetl_odbc_test must not exist yet
# (run.sh drops them first), so the CREATE TABLE path is tested too.

set -uo pipefail
TINYETL=/dist/tinyetl
DATA=/odbc/typed.csv
TABLE=tinyetl_odbc_test
WORK=$(mktemp -d)
ROWS=$(($(wc -l < "$DATA") - 1))
FAILED=0

# name -> connection string (without odbc:// and #table)
declare -A CONN=(
    [sqlserver]="Driver={ODBC Driver 18 for SQL Server};Server=tinyetl-mssql,1433;Database=testdb;UID=SA;PWD=TestPass123!;TrustServerCertificate=yes"
    [postgres]="Driver={PostgreSQL Unicode};Server=tinyetl-postgres;Port=5432;Database=testdb;UID=testuser;PWD=testpass"
    [mysql]="Driver={MariaDB Unicode};Server=tinyetl-mysql;Port=3306;Database=testdb;UID=testuser;PWD=testpass"
    [sqlite]="Driver={SQLite3};Database=$WORK/test.sqlite"
)

pass() { echo "  ✅ $*"; }
fail() { echo "  ❌ $*"; FAILED=1; }

# Run tinyetl quietly; show its output only if it fails
etl() {
    local out
    if ! out=$("$TINYETL" "$@" 2>&1); then
        echo "$out" | grep -v " INFO " | sed 's/^/     /' >&2
        return 1
    fi
    echo "$out" | grep -o "ODBC: connected to .*" | head -1 | sed 's/^/     /'
}

# Number of rows in the table, read back through ODBC
count_rows() {
    rm -f "$WORK/count.csv"
    "$TINYETL" "odbc://$1#$TABLE" "$WORK/count.csv" >/dev/null 2>&1 || { echo "error"; return; }
    echo $(($(wc -l < "$WORK/count.csv") - 1))
}

echo "tinyetl linked libraries:"
ldd "$TINYETL" | sed 's/^/  /'
if ldd "$TINYETL" | grep -qE "libodbc|libltdl|libssl"; then
    fail "the binary links a system ODBC or OpenSSL library"
fi

for db in sqlserver postgres mysql sqlite; do
    conn=${CONN[$db]}
    echo ""
    echo "=== $db"

    if etl "$DATA" "odbc://$conn#$TABLE"; then
        pass "created $TABLE and wrote $ROWS rows"
    else
        fail "write failed"; continue
    fi

    rm -f "$WORK/out.csv"
    if etl "odbc://$conn#$TABLE" "$WORK/out.csv" && python3 -I /odbc/compare.py "$DATA" "$WORK/out.csv" id | sed 's/^/     /'; then
        pass "read back: values and column order match"
    else
        fail "read back differs from the input"
    fi

    if etl "$DATA" "odbc://$conn#$TABLE" >/dev/null; then
        n=$(count_rows "$conn")
        [ "$n" = "$((ROWS * 2))" ] && pass "append: $n rows" || fail "append: expected $((ROWS * 2)) rows, got $n"
    else
        fail "append run failed"
    fi

    if etl "$DATA" "odbc://$conn#$TABLE" --truncate >/dev/null; then
        n=$(count_rows "$conn")
        [ "$n" = "$ROWS" ] && pass "--truncate: $n rows" || fail "--truncate: expected $ROWS rows, got $n"
    else
        fail "--truncate run failed"
    fi

    # --source-query: the connection string without #table, and the query's result
    rm -f "$WORK/query.csv"
    if etl "odbc://$conn" "$WORK/query.csv" --source-query "SELECT COUNT(*) AS n FROM $TABLE;"; then
        n=$(sed -n 2p "$WORK/query.csv" | tr -d '\r')
        [ "$n" = "$ROWS" ] && pass "--source-query: COUNT(*) = $n" || fail "--source-query: expected $ROWS, got $n"
    else
        fail "--source-query run failed"
    fi
done

# A table TinyETL didn't create: test_env seeds employees in Postgres
echo ""
echo "=== postgres (pre-seeded employees table)"
rm -f "$WORK/employees.csv"
if etl "odbc://${CONN[postgres]}#public.employees" "$WORK/employees.csv"; then
    pass "read $(($(wc -l < "$WORK/employees.csv") - 1)) rows from public.employees ($(head -1 "$WORK/employees.csv"))"
else
    fail "reading public.employees failed"
fi

echo ""
if [ "$FAILED" = 0 ]; then echo "All ODBC tests passed"; else echo "Some ODBC tests FAILED"; fi
exit "$FAILED"
