#!/bin/bash
# Example 27: source queries (--source-query / YAML source_query)
#
# Loads the same orders into each database, then reads them back through a query with a CTE,
# a filter, an aggregate and ORDER BY, and checks the result. SQLite and DuckDB always run;
# PostgreSQL, MySQL and SQL Server run when their test_env containers are running.
# Also checks the YAML form and the mistakes that should be errors.

set -e
cd "$(dirname "$0")"

TINYETL=${TINYETL:-$(pwd)/../../target/release/tinyetl}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
run() { "$TINYETL" "$@" --log-level error; }
pass() { echo "  ✓ $1"; }
fail() { echo "  ✗ FAIL: $1"; exit 1; }
running() { docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1"; }

echo "Running Example 27: source queries"

cat > "$WORK/orders.csv" <<'EOF'
id,region,amount
1,west,10
2,east,20
3,west,30
4,east,40
5,west,50
6,east,60
EOF

QUERY="WITH big AS (SELECT * FROM ex27_orders WHERE amount > 10)
SELECT region, COUNT(*) AS orders, SUM(amount) AS total
FROM big GROUP BY region ORDER BY region;"
EXPECTED='east,3,120|west,2,80'

# Load the orders, run the query, and compare the result (as region,orders,total lines)
check() {
    local name=$1 database=$2; shift 2
    run "$WORK/orders.csv" "$database#ex27_orders" --truncate "$@"
    rm -f "$WORK/result.csv"
    run "$database" "$WORK/result.csv" --source-query "$QUERY" "${@/--dest-secret-id/--source-secret-id}"
    local got
    got=$(tail -n +2 "$WORK/result.csv" | tr -d '\r' | sed 's/\.0*,/,/g; s/\.0*$//' | paste -sd'|' -)
    [ "$got" = "$EXPECTED" ] || fail "$name: expected $EXPECTED, got $got"
    pass "$name"
}

echo ""
echo "1. The same query on each database"
check "SQLite" "$WORK/shop.db"
check "DuckDB" "$WORK/shop.duckdb"
export TINYETL_SECRET_ex27_pg=testpass TINYETL_SECRET_ex27_my=testpass TINYETL_SECRET_ex27_ms='TestPass123!'
if running tinyetl-postgres; then
    check "PostgreSQL" "postgres://testuser@localhost:5432/testdb" --dest-secret-id ex27_pg
else
    echo "  ⚠️  PostgreSQL skipped: the tinyetl-postgres container isn't running"
fi
if running tinyetl-mysql; then
    check "MySQL" "mysql://testuser@localhost:3306/testdb" --dest-secret-id ex27_my
else
    echo "  ⚠️  MySQL skipped: the tinyetl-mysql container isn't running"
fi
if running tinyetl-mssql; then
    check "SQL Server" "mssql://sa@localhost:1433/master" --dest-secret-id ex27_ms
else
    echo "  ⚠️  SQL Server skipped: the tinyetl-mssql container isn't running"
fi

echo ""
echo "2. In a YAML job file"
cat > "$WORK/job.yaml" <<EOF
source: $WORK/shop.db
source_query: |
  SELECT id, amount * 2 AS doubled
  FROM ex27_orders
  WHERE region = 'east'
  ORDER BY id
target: $WORK/east.csv
EOF
"$TINYETL" run "$WORK/job.yaml" >/dev/null 2>&1 || fail "the YAML job failed"
[ "$(tail -n +2 "$WORK/east.csv" | paste -sd'|' -)" = "2,40|4,80|6,120" ] || fail "YAML job: $(cat "$WORK/east.csv")"
pass "source_query in a job file"

echo ""
echo "3. Mistakes are errors"
if run "$WORK/shop.db#ex27_orders" "$WORK/x.csv" --source-query "SELECT 1" 2>/dev/null; then
    fail "a #table together with a query was accepted"
fi
pass "a #table together with --source-query"
if run "$WORK/orders.csv" "$WORK/x.json" --source-query "SELECT 1" 2>/dev/null; then
    fail "a query on a CSV file was accepted"
fi
pass "--source-query on a file"
err=$("$TINYETL" "$WORK/shop.db" "$WORK/x.csv" --source-query "SELEC 1" --log-level error 2>&1 || true)
echo "$err" | grep -q 'near "SELEC": syntax error' || fail "SQL error message: $err"
pass "a SQL error names the problem: ${err#Error: }"

echo ""
echo "✅ Example 27 completed successfully"
