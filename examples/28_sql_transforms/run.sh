#!/bin/bash
# Example 28: SQL transforms (--sql / YAML sql:)
#
# The source rows are a DuckDB table named `source`; the query sees the whole dataset.
#   1. An aggregate over 200,000 rows read in batches: totals match an exact sum in Python
#   2. A window function (running totals) and QUALIFY (keep the latest row per id)
#   3. Nested JSON in, DuckDB JSON functions, lists and structs out as JSON
#   4. Into SQLite, and into PostgreSQL when the test_env container is running
#   5. In a YAML job file
#   6. A schema file checks the source rows before the query runs
#   7. Mistakes are errors: a wrong table name, SQL together with a Lua transform
#
# Each step checks its result and the script exits non-zero on a wrong answer.

set -e
cd "$(dirname "$0")"

TINYETL=${TINYETL:-$(pwd)/../../target/release/tinyetl}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
run() { "$TINYETL" "$@" --log-level error; }
pass() { echo "  ✓ $1"; }
fail() { echo "  ✗ FAIL: $1"; exit 1; }
running() { docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1"; }
rows() { tail -n +2 "$1" | tr -d '\r' | paste -sd'|' -; }

echo "Running Example 28: SQL transforms"

python3 -I - "$WORK" <<'EOF'
import json, random, sys
work = sys.argv[1]
random.seed(28)
with open(f"{work}/sales.csv", "w") as f:
    f.write("id,region,amount,day\n")
    for i in range(1, 200_001):
        f.write(f"{i},r{i % 7},{random.randint(1, 100_000) / 100:.2f},2024-01-{i % 28 + 1:02d}\n")
with open(f"{work}/updates.csv", "w") as f:
    f.write("id,status,updated_at\n1,new,2024-01-01T10:00:00\n1,paid,2024-01-02T09:00:00\n"
            "2,new,2024-01-01T11:00:00\n1,shipped,2024-01-03T08:00:00\n2,cancelled,2024-01-04T12:00:00\n")
with open(f"{work}/events.json", "w") as f:
    for i in range(1, 7):
        f.write(json.dumps({"id": i, "type": "PushEvent" if i % 3 else "WatchEvent",
                            "actor": {"login": f"user{i % 2}"}, "payload": {"size": i}}) + "\n")
EOF

echo ""
echo "1. An aggregate over every row"
run "$WORK/sales.csv" "$WORK/totals.csv" --batch-size 5000 \
    --sql "SELECT region, COUNT(*) AS orders, SUM(amount) AS total FROM source GROUP BY region ORDER BY region"
python3 -I - "$WORK" <<'EOF' || fail "totals differ from Python's"
import csv, sys
from decimal import Decimal
work = sys.argv[1]
expected = {}
for r in csv.DictReader(open(f"{work}/sales.csv")):
    count, total = expected.get(r["region"], (0, Decimal(0)))
    expected[r["region"]] = (count + 1, total + Decimal(r["amount"]))
got = {r["region"]: (int(r["orders"]), Decimal(r["total"])) for r in csv.DictReader(open(f"{work}/totals.csv"))}
assert got == expected, (got, expected)
EOF
pass "7 regions over 200,000 rows read in batches of 5,000; counts and exact sums match Python"

echo ""
echo "2. Window functions and QUALIFY"
run "$WORK/sales.csv" "$WORK/running.csv" \
    --sql "SELECT id, amount, SUM(amount) OVER (ORDER BY id) AS running FROM source WHERE id <= 3 ORDER BY id"
python3 -I -c "
import csv, sys
from decimal import Decimal
rows = list(csv.DictReader(open(sys.argv[1])))
total = Decimal(0)
for r in rows:
    total += Decimal(r['amount'])
    assert Decimal(r['running']) == total, rows
assert len(rows) == 3
" "$WORK/running.csv" || fail "running totals"
pass "running total with SUM() OVER (ORDER BY id)"
run "$WORK/updates.csv" "$WORK/latest.csv" \
    --sql "SELECT id, status FROM source QUALIFY row_number() OVER (PARTITION BY id ORDER BY updated_at DESC) = 1 ORDER BY id"
[ "$(rows "$WORK/latest.csv")" = "1,shipped|2,cancelled" ] || fail "latest per id: $(rows "$WORK/latest.csv")"
pass "latest row per id with QUALIFY"

echo ""
echo "3. Nested JSON"
run "$WORK/events.json" "$WORK/pushes.jsonl" \
    --sql "SELECT actor->>'login' AS login, SUM(CAST(payload->>'size' AS INT)) AS size,
                  list(id ORDER BY id) AS ids, {'events': count(*)} AS stats
           FROM source WHERE type = 'PushEvent' GROUP BY 1 ORDER BY 1"
python3 -I -c "
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
assert rows == [{'login': 'user0', 'size': 6, 'ids': [2, 4], 'stats': {'events': 2}},
                {'login': 'user1', 'size': 6, 'ids': [1, 5], 'stats': {'events': 2}}], rows
" "$WORK/pushes.jsonl" || fail "nested JSON: $(cat "$WORK/pushes.jsonl")"
pass "->> on nested objects; list() and struct results come out as JSON"

echo ""
echo "4. Into databases"
run "$WORK/sales.csv" "$WORK/out.db#totals" --sql "SELECT region, SUM(amount) AS total FROM source GROUP BY ALL"
[ "$(python3 -I -c "import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute('SELECT count(*) FROM totals').fetchone()[0])" "$WORK/out.db")" = "7" ] \
    || fail "SQLite"
pass "SQLite: 7 rows"
if running tinyetl-postgres; then
    export TINYETL_SECRET_ex28=testpass
    run "$WORK/sales.csv" "postgres://testuser@localhost:5432/testdb#tinyetl_ex28_totals" --dest-secret-id ex28 --truncate \
        --sql "SELECT region, COUNT(*) AS orders, SUM(amount) AS total FROM source GROUP BY ALL"
    result=$(docker exec tinyetl-postgres psql -U testuser -d testdb -tA -c \
        "SELECT count(*) || ',' || sum(orders) FROM tinyetl_ex28_totals; DROP TABLE tinyetl_ex28_totals;" | head -1)
    [ "$result" = "7,200000" ] || fail "PostgreSQL: $result"
    pass "PostgreSQL: 7 regions, 200,000 orders"
else
    echo "  ⚠️  PostgreSQL skipped: the tinyetl-postgres container isn't running"
fi

echo ""
echo "5. In a YAML job file"
cat > "$WORK/job.yaml" <<EOF
source: $WORK/updates.csv
target: $WORK/job.csv
sql: |
  SELECT status, COUNT(*) AS n
  FROM source
  GROUP BY status
  ORDER BY status
EOF
"$TINYETL" run "$WORK/job.yaml" >/dev/null 2>&1 || fail "the YAML job failed"
[ "$(rows "$WORK/job.csv")" = "cancelled,1|new,2|paid,1|shipped,1" ] || fail "YAML: $(rows "$WORK/job.csv")"
pass "sql: in a job file"

echo ""
echo "6. A schema file checks the source rows first"
cat > "$WORK/schema.yaml" <<'EOF'
columns:
  - name: id
    type: Integer
  - name: status
    type: String
    pattern: '^(new|paid|shipped)$'
  - name: updated_at
    type: DateTime
EOF
err=$("$TINYETL" "$WORK/updates.csv" "$WORK/x.csv" --schema-file "$WORK/schema.yaml" --log-level error \
    --sql "SELECT COUNT(*) AS n FROM source" 2>&1 || true)
echo "$err" | grep -q "'cancelled' does not match pattern" || fail "schema file: $err"
pass "a row that breaks the schema file stops the job before the query"

echo ""
echo "7. Mistakes are errors"
err=$("$TINYETL" "$WORK/updates.csv" "$WORK/x.csv" --sql "SELECT * FROM updates" --log-level error 2>&1 || true)
echo "$err" | grep -q 'a table named "source"' || fail "wrong table name: $err"
pass "a wrong table name says the rows are in \"source\""
if "$TINYETL" "$WORK/updates.csv" "$WORK/x.csv" --sql "SELECT 1" --transform "x=1" >/dev/null 2>&1; then
    fail "--sql together with --transform was accepted"
fi
pass "--sql together with a Lua transform"

echo ""
echo "✅ Example 28 completed successfully"
