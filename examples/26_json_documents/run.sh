#!/bin/bash
# Example 26: JSON documents (issues #16, #3 and #13)
#
#   1. A .json file holding JSON Lines (the GH Archive layout) loads, as do pretty-printed
#      objects back to back and a file with a byte order mark (#16)
#   2. Nested objects and arrays stay nested in JSON output (#3)
#   3. Loading into SQLite keeps every column, including keys missing from the first rows,
#      and stores nested values as JSON text that SQLite's JSON functions read (#3)
#   4. SQLite JSON columns stored as BLOB or TEXT are read as JSON (#13)
#   5. DuckDB: nested values land in JSON columns and read back nested (#3)
#   6. PostgreSQL and MySQL, when their test_env containers are running: nested values land
#      in jsonb / JSON columns (#3)
#   7. Optional, with network access: an hour of GH Archive (#16)
#
# Each step checks its result and the script exits non-zero on a wrong answer.
# Set TINYETL_EXAMPLES_OFFLINE=1 to skip step 7.

set -e
cd "$(dirname "$0")"

TINYETL=${TINYETL:-$(pwd)/../../target/release/tinyetl}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
run() { "$TINYETL" "$@" --log-level warn; }
pass() { echo "  ✓ $1"; }
fail() { echo "  ✗ FAIL: $1"; exit 1; }
export TINYETL_SECRET_ex26=testpass  # passwords via --dest-secret-id, not in the URL
running() { docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1"; }
# Check a JSON Lines file with a Python expression over `rows`
check() {
    python3 -I -c "import json, sys
rows = [json.loads(l) for l in open(sys.argv[1], encoding='utf-8') if l.strip()]
sys.exit(0 if ($2) else 1)" "$1"
}

echo "Running Example 26: JSON documents"

# 60 GitHub-style events, one JSON object per line, saved as .json. `org` appears only on
# every 6th event, so the first rows don't have it.
python3 -I - "$WORK" <<'EOF'
import json, sys
work = sys.argv[1]
events = []
for i in range(1, 61):
    event = {
        "id": str(1000 + i),
        "type": "PushEvent",
        "actor": {"id": i, "login": f"user{i}"},
        "repo": {"id": 500 + i, "name": f"user{i}/repo"},
        "payload": {"size": i % 4, "commits": [{"sha": f"{i:040x}", "message": f"fix #{i}"}]},
        "public": True,
    }
    if i % 6 == 0:
        event["org"] = {"id": i, "login": f"org{i}"}
    events.append(event)
with open(f"{work}/events.json", "w", encoding="utf-8") as f:
    f.writelines(json.dumps(e) + "\n" for e in events)
with open(f"{work}/pretty.json", "w", encoding="utf-8") as f:
    f.write("\n".join(json.dumps(e, indent=2) for e in events))
with open(f"{work}/bom.json", "w", encoding="utf-8-sig") as f:
    f.writelines(json.dumps(e) + "\n" for e in events)
EOF

echo ""
echo "1. .json files that hold JSON Lines (#16)"
for name in events pretty bom; do
    run "$WORK/$name.json" "$WORK/$name.jsonl"
    check "$WORK/$name.jsonl" "len(rows) == 60" || fail "$name.json: expected 60 rows"
done
pass "JSON Lines in a .json file: 60 rows"
pass "pretty-printed objects back to back: 60 rows"
pass "a file starting with a byte order mark: 60 rows"

echo ""
echo "2. Nested values stay nested (#3)"
check "$WORK/events.jsonl" "rows[0]['actor'] == {'id': 1, 'login': 'user1'}
    and rows[0]['payload']['commits'][0]['message'] == 'fix #1'
    and sum(r['org'] is not None for r in rows) == 10
    and rows[5]['org']['login'] == 'org6'" || fail "nested values changed"
pass "objects and arrays come out as JSON, not strings; all 10 orgs kept"

echo ""
echo "3. SQLite: every column, nested values as JSON text (#3)"
run "$WORK/events.json" "$WORK/events.db#events"
python3 -I - "$WORK/events.db" <<'EOF' || fail "SQLite contents"
import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
rows, orgs = con.execute("SELECT count(*), count(org) FROM events").fetchone()
login, = con.execute("SELECT json_extract(actor, '$.login') FROM events WHERE id = '1006'").fetchone()
org, = con.execute("SELECT json_extract(org, '$.login') FROM events WHERE id = '1006'").fetchone()
assert (rows, orgs, login, org) == (60, 10, "user6", "org6"), (rows, orgs, login, org)
EOF
pass "60 rows, 10 orgs; json_extract(actor, '\$.login') works"

echo ""
echo "4. SQLite JSON columns stored as BLOB or TEXT (#13)"
python3 -I - "$WORK/docs.db" <<'EOF'
import json, sqlite3, sys
con = sqlite3.connect(sys.argv[1])
con.execute("CREATE TABLE docs (id INTEGER, doc JSON)")
con.execute("INSERT INTO docs VALUES (1, CAST(? AS BLOB))", (json.dumps({"title": "Alpha", "tags": ["a", "b"]}),))
con.execute("INSERT INTO docs VALUES (2, ?)", (json.dumps({"title": "Béta", "n": 2}),))
con.execute("INSERT INTO docs VALUES (3, NULL)")
con.commit()
EOF
run "$WORK/docs.db#docs" "$WORK/docs.jsonl"
check "$WORK/docs.jsonl" "[r['doc'] for r in rows] == [{'title': 'Alpha', 'tags': ['a', 'b']}, {'title': 'Béta', 'n': 2}, None]" \
    || fail "SQLite JSON documents weren't read as JSON"
pass "BLOB and TEXT documents read as JSON, NULL stays NULL"

echo ""
echo "5. DuckDB JSON columns (#3)"
run "$WORK/events.json" "$WORK/events.duckdb#events"
run "$WORK/events.duckdb#events" "$WORK/from_duckdb.jsonl"
check "$WORK/from_duckdb.jsonl" "len(rows) == 60 and rows[0]['actor']['login'] == 'user1'
    and sum(r['org'] is not None for r in rows) == 10" || fail "DuckDB round trip"
pass "loaded and read back nested"

echo ""
echo "6. PostgreSQL jsonb and MySQL JSON (#3)"
if running tinyetl-postgres; then
    run "$WORK/events.json" "postgres://testuser@localhost:5432/testdb#tinyetl_ex26_events" --dest-secret-id ex26 --truncate
    result=$(docker exec tinyetl-postgres psql -U testuser -d testdb -tA -c \
        "SELECT (SELECT data_type FROM information_schema.columns WHERE table_name = 'tinyetl_ex26_events' AND column_name = 'actor')
             || ',' || count(*) || ',' || count(org) || ',' || max(actor->>'login') FILTER (WHERE id = '1006')
                || ',' || sum((payload->>'size')::int)
         FROM tinyetl_ex26_events; DROP TABLE tinyetl_ex26_events;" | head -1)
    [ "$result" = "jsonb,60,10,user6,90" ] || fail "PostgreSQL: got $result"
    pass "PostgreSQL: jsonb columns; actor->>'login' and payload->>'size' work"
else
    echo "  ⚠️  PostgreSQL skipped: the tinyetl-postgres container isn't running"
fi
if running tinyetl-mysql; then
    run "$WORK/events.json" "mysql://testuser@localhost:3306/testdb#tinyetl_ex26_events" --dest-secret-id ex26 --truncate
    result=$(docker exec tinyetl-mysql mysql -utestuser -ptestpass testdb -N -e \
        "SELECT CONCAT((SELECT data_type FROM information_schema.columns WHERE table_schema = 'testdb' AND table_name = 'tinyetl_ex26_events' AND column_name = 'actor'),
                ',', count(*), ',', count(org), ',', max(CASE WHEN id = '1006' THEN actor->>'$.login' END))
         FROM tinyetl_ex26_events; DROP TABLE tinyetl_ex26_events;" 2>/dev/null)
    [ "$result" = "json,60,10,user6" ] || fail "MySQL: got $result"
    pass "MySQL: JSON columns; actor->>'\$.login' works"
else
    echo "  ⚠️  MySQL skipped: the tinyetl-mysql container isn't running"
fi

echo ""
echo "7. An hour of GH Archive (#16)"
URL=https://data.gharchive.org/2015-01-01-15.json.gz
if [ -n "$TINYETL_EXAMPLES_OFFLINE" ]; then
    echo "  ⚠️  skipped (TINYETL_EXAMPLES_OFFLINE is set)"
elif ! curl -sfI -m 10 "$URL" >/dev/null 2>&1; then
    echo "  ⚠️  skipped: $URL isn't reachable"
else
    run "$URL" "$WORK/gharchive.db#events"
    result=$(python3 -I -c "import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(*con.execute(\"SELECT count(*), count(DISTINCT id), count(org), sum(json_extract(payload, '\$.size')) FROM events\").fetchone(), sep=',')" "$WORK/gharchive.db")
    [ "$result" = "11351,11351,3245,25930" ] || fail "GH Archive: got $result"
    pass "2015-01-01-15.json.gz: 11,351 events, 3,245 with an org, payload sizes sum to 25,930"
fi

echo ""
echo "✅ Example 26 completed successfully"
