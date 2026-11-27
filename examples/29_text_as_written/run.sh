#!/bin/bash
# Example 29: text that looks like a number or a date stays as written
#
#   1. With no schema file, codes with leading zeros or a leading + (01826, 012345, +15551234)
#      keep their text, in CSV, SQLite, DuckDB and Parquet
#   2. A schema file's String columns keep the source text exactly (01826, 0012.50, 3/4/2021,
#      2024-01-01, true), in DuckDB, SQLite and Parquet; columns it doesn't list are inferred
#   3. JSON strings in String columns stay strings
#   4. on_mismatch: fail (the default), null and raw for text that doesn't parse
#   5. infer_types=false reads every column as text
#   6. Reloading with --truncate gives the same rows
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
# A source's rows as CSV lines joined with |, header first
readback() { rm -f "$WORK/back.csv"; run "$1" "$WORK/back.csv"; tr -d '\r' < "$WORK/back.csv" | paste -sd'|' -; }
# A source's column types (name:TYPE,...) from --preview
types() {
    "$TINYETL" "$1" "$WORK/unused.csv" --preview 1 --log-level error |
        awk '/Original Schema Preview/ {on=1; next} /Data Preview/ {on=0} on && /^│/ && !/Column/ {print $2 ":" $4}' |
        paste -sd',' -
}
sqlite_types() {
    python3 -I -c "import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(','.join(f'{n}:{t}' for _, n, t, *_ in con.execute('PRAGMA table_info(t)')))" "$1"
}

echo "Running Example 29: text as written"

cat > "$WORK/p.csv" <<'EOF'
"ZipCode","Phone","Lic","Flag","Amount","Day","Note"
"01826","9195550102","012345","true","0012.50","3/4/2021","2024-01-01"
"28801","+15550104","7","false","3.25","3/5/2021",""
EOF

echo ""
echo "1. No schema file: codes keep their text"
EXPECTED_CSV='ZipCode,Phone,Lic,Flag,Amount,Day,Note|01826,9195550102,012345,true,0012.50,'
run "$WORK/p.csv" "$WORK/copy.csv"
got=$(tr -d '\r' < "$WORK/copy.csv" | paste -sd'|' -)
case "$got" in
    "$EXPECTED_CSV"*) ;;
    *) fail "CSV copy: $got" ;;
esac
pass "CSV → CSV: 01826, 012345, +15550104 and 0012.50 unchanged"
run "$WORK/p.csv" "$WORK/inferred.db#t"
[ "$(sqlite_types "$WORK/inferred.db")" = "ZipCode:TEXT,Phone:TEXT,Lic:TEXT,Flag:BOOLEAN,Amount:TEXT,Day:DATE,Note:DATE" ] \
    || fail "SQLite types: $(sqlite_types "$WORK/inferred.db")"
[ "$(python3 -I -c "import sqlite3, sys; print(*sqlite3.connect(sys.argv[1]).execute('SELECT ZipCode, Lic, Phone FROM t ORDER BY ZipCode').fetchone(), sep=',')" "$WORK/inferred.db")" = "01826,012345,9195550102" ] \
    || fail "SQLite values"
pass "SQLite: the code columns are TEXT, and keep their text"

echo ""
echo "2. A schema file's String columns keep the source text"
cat > "$WORK/schema.yaml" <<'EOF'
columns:
  - name: ZipCode
    type: String
  - name: Phone
    type: String
  - name: Lic
    type: String
  - name: Flag
    type: String
  - name: Amount
    type: String
  - name: Day
    type: String
EOF
EXPECTED='ZipCode,Phone,Lic,Flag,Amount,Day,Note|01826,9195550102,012345,true,0012.50,3/4/2021,2024-01-01T00:00:00+00:00|28801,+15550104,7,false,3.25,3/5/2021,'
for target in "$WORK/out.duckdb#t" "$WORK/out.db#t" "$WORK/out.parquet"; do
    run "$WORK/p.csv" "$target" --schema-file "$WORK/schema.yaml"
    got=$(readback "$target")
    [ "$got" = "$EXPECTED" ] || fail "$target: $got"
done
pass "DuckDB, SQLite and Parquet: every listed value exactly as in the file"
[ "$(types "$WORK/out.duckdb#t")" = "ZipCode:TEXT,Phone:TEXT,Lic:TEXT,Flag:TEXT,Amount:TEXT,Day:TEXT,Note:TIMESTAMP" ] \
    || fail "DuckDB types: $(types "$WORK/out.duckdb#t")"
[ "$(types "$WORK/out.parquet")" = "ZipCode:TEXT,Phone:TEXT,Lic:TEXT,Flag:TEXT,Amount:TEXT,Day:TEXT,Note:TIMESTAMP" ] \
    || fail "Parquet types: $(types "$WORK/out.parquet")"
pass "the listed columns are text; Note, which the schema doesn't list, is still inferred"

echo ""
echo "3. JSON strings stay strings"
cat > "$WORK/p.jsonl" <<'EOF'
{"ZipCode": "01826", "Lic": "2024-01-01", "Flag": "true", "Phone": 9195550102}
EOF
cat > "$WORK/json_schema.yaml" <<'EOF'
columns:
  - {name: ZipCode, type: String}
  - {name: Lic, type: String}
  - {name: Flag, type: String}
  - {name: Phone, type: String}
EOF
run "$WORK/p.jsonl" "$WORK/out.jsonl" --schema-file "$WORK/json_schema.yaml"
[ "$(cat "$WORK/out.jsonl")" = '{"ZipCode":"01826","Lic":"2024-01-01","Flag":"true","Phone":"9195550102"}' ] \
    || fail "JSON: $(cat "$WORK/out.jsonl")"
pass "\"2024-01-01\" and \"true\" stay strings; the number 9195550102 becomes the string \"9195550102\""

echo ""
echo "4. on_mismatch for text that doesn't parse"
printf 'id,amount\n1,10.5\n2,abc\n3,\n' > "$WORK/amounts.csv"
policy() { printf 'on_mismatch: %s\ncolumns:\n  - {name: id, type: Integer}\n  - {name: amount, type: Decimal}\n' "$1" > "$WORK/policy.yaml"; }
policy fail
err=$("$TINYETL" "$WORK/amounts.csv" "$WORK/a.csv" --schema-file "$WORK/policy.yaml" --log-level error 2>&1 || true)
echo "$err" | grep -q "Column 'amount' value 'abc' is not a valid Decimal" || fail "fail: $err"
pass "fail (the default): stops with the column and the value"
policy null
rm -f "$WORK/a.csv"
warning=$("$TINYETL" "$WORK/amounts.csv" "$WORK/a.csv" --schema-file "$WORK/policy.yaml" --log-level warn 2>&1)
echo "$warning" | grep -q "1 values didn't parse" || fail "null warning: $warning"
[ "$(tr -d '\r' < "$WORK/a.csv" | paste -sd'|' -)" = "id,amount|1,10.5|2,|3," ] || fail "null: $(cat "$WORK/a.csv")"
pass "null: abc becomes NULL and the transfer says how many values it replaced"
policy raw
rm -f "$WORK/a.csv"
run "$WORK/amounts.csv" "$WORK/a.csv" --schema-file "$WORK/policy.yaml"
[ "$(tr -d '\r' < "$WORK/a.csv" | paste -sd'|' -)" = "id,amount|1,10.5|2,abc|3," ] || fail "raw: $(cat "$WORK/a.csv")"
pass "raw: abc is kept in a file target"

echo ""
echo "5. infer_types=false"
run "$WORK/p.csv" "$WORK/raw.duckdb#t" --source-option infer_types=false
[ "$(types "$WORK/raw.duckdb#t")" = "ZipCode:TEXT,Phone:TEXT,Lic:TEXT,Flag:TEXT,Amount:TEXT,Day:TEXT,Note:TEXT" ] \
    || fail "infer_types=false: $(types "$WORK/raw.duckdb#t")"
pass "every column is text, with no schema file"

echo ""
echo "6. Reloading with --truncate"
run "$WORK/p.csv" "$WORK/out.duckdb#t" --schema-file "$WORK/schema.yaml" --truncate
[ "$(readback "$WORK/out.duckdb#t")" = "$EXPECTED" ] || fail "reload: $(readback "$WORK/out.duckdb#t")"
pass "the same 2 rows and values"

echo ""
echo "✅ Example 29 completed successfully"
