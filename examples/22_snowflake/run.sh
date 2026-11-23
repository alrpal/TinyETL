#!/bin/bash
# Example 22: Snowflake load and read against a real account
#
# Needs SNOWFLAKE_ACCOUNT, SNOWFLAKE_USER, SNOWFLAKE_PRIVATE_KEY_PATH, SNOWFLAKE_WAREHOUSE,
# SNOWFLAKE_DATABASE and SNOWFLAKE_SCHEMA (optionally SNOWFLAKE_ROLE), exported or in a .env file
# at the repository root. Skips itself otherwise. See README.md in this directory for the setup.

set -e
cd "$(dirname "$0")"

echo "Running Example 22: Snowflake"

if [ -f ../../.env ]; then
    set -a
    . ../../.env
    set +a
fi

for var in SNOWFLAKE_ACCOUNT SNOWFLAKE_USER SNOWFLAKE_PRIVATE_KEY_PATH SNOWFLAKE_WAREHOUSE \
           SNOWFLAKE_DATABASE SNOWFLAKE_SCHEMA; do
    if [ -z "${!var}" ]; then
        echo "⚠️  SKIPPED: $var is not set. See README.md in this directory for the setup."
        exit 0
    fi
done

TINYETL=../../target/release/tinyetl
COMPARE="python3 -I ../../test_env/odbc/compare.py --ignore-header-case"
DATA=../../test_env/odbc/typed.csv
ROWS=$(($(wc -l < "$DATA") - 1))
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

SNOWFLAKE_URL="snowflake://${SNOWFLAKE_USER}@${SNOWFLAKE_ACCOUNT}/${SNOWFLAKE_DATABASE}/${SNOWFLAKE_SCHEMA}?warehouse=${SNOWFLAKE_WAREHOUSE}"
if [ -n "$SNOWFLAKE_ROLE" ]; then SNOWFLAKE_URL="${SNOWFLAKE_URL}&role=${SNOWFLAKE_ROLE}"; fi
export SNOWFLAKE_URL

FAILED=0
pass() { echo "✅ PASS: $*"; }
fail() { echo "❌ FAIL: $*"; FAILED=1; }

# Rows in a table, by reading it back
count_rows() {
    rm -f "$WORK/count.csv"
    $TINYETL "$SNOWFLAKE_URL#$1" "$WORK/count.csv" >/dev/null 2>&1 || { echo error; return; }
    echo $(($(wc -l < "$WORK/count.csv") - 1))
}

echo ""
echo "Step 1: stage + COPY INTO, then read back"
if $TINYETL "$DATA" "$SNOWFLAKE_URL#TINYETL_EXAMPLE_TYPED" --truncate; then
    rm -f "$WORK/typed.csv"
    if $TINYETL "$SNOWFLAKE_URL#TINYETL_EXAMPLE_TYPED" "$WORK/typed.csv" &&
        $COMPARE "$DATA" "$WORK/typed.csv" id; then
        pass "values and column order match after a staged load"
    else
        fail "read back differs from $DATA"
    fi
else
    fail "staged load failed"
fi

echo ""
echo "Step 2: append, then --truncate"
$TINYETL "$DATA" "$SNOWFLAKE_URL#TINYETL_EXAMPLE_TYPED" >/dev/null 2>&1 || fail "append run failed"
n=$(count_rows TINYETL_EXAMPLE_TYPED)
[ "$n" = "$((ROWS * 2))" ] && pass "append: $n rows" || fail "append: expected $((ROWS * 2)) rows, got $n"
$TINYETL "$DATA" "$SNOWFLAKE_URL#TINYETL_EXAMPLE_TYPED" --truncate >/dev/null 2>&1 || fail "--truncate run failed"
n=$(count_rows TINYETL_EXAMPLE_TYPED)
[ "$n" = "$ROWS" ] && pass "--truncate: $n rows" || fail "--truncate: expected $ROWS rows, got $n"

echo ""
echo "Step 3: batched INSERT (load_method = insert, the path used on Azure/GCP accounts)"
if $TINYETL run insert_config.yaml; then
    rm -f "$WORK/insert.csv"
    if $TINYETL "$SNOWFLAKE_URL#TINYETL_EXAMPLE_INSERT" "$WORK/insert.csv" &&
        $COMPARE "$DATA" "$WORK/insert.csv" id; then
        pass "values and column order match after an INSERT load"
    else
        fail "read back differs from $DATA"
    fi
else
    fail "INSERT load failed"
fi

echo ""
echo "Step 4: 1,000,000 rows (several staged files; memory should stay bounded)"
awk 'BEGIN {
    print "id,name,amount,created_at";
    for (i = 1; i <= 1000000; i++)
        printf "%d,customer %d,%d.%02d,2024-%02d-%02d 12:%02d:%02d\n", i, i, i % 10000, i % 100, (i % 12) + 1, (i % 28) + 1, i % 60, (i * 7) % 60
}' > "$WORK/big.csv"
TIME_LOG="$WORK/time.txt"
if [ "$(uname)" = "Darwin" ]; then TIME="/usr/bin/time -l"; else TIME="/usr/bin/time -v"; fi
start=$(date +%s)
if $TIME $TINYETL "$WORK/big.csv" "$SNOWFLAKE_URL#TINYETL_EXAMPLE_BIG" --truncate 2> "$TIME_LOG" >/dev/null; then
    elapsed=$(($(date +%s) - start))
    rss=$(grep -E "maximum resident set size|Maximum resident set size" "$TIME_LOG" | grep -oE "[0-9]+" | head -1)
    [ "$(uname)" = "Darwin" ] && rss=$((rss / 1024)) # bytes -> KB on macOS
    n=$(count_rows TINYETL_EXAMPLE_BIG)
    [ "$n" = "1000000" ] && pass "loaded 1,000,000 rows in ${elapsed}s, peak memory $((rss / 1024)) MB" \
        || fail "expected 1000000 rows, got $n"
else
    grep -vE "^\s+[0-9]+ " "$TIME_LOG" | tail -5
    fail "1M-row load failed"
fi

echo ""
echo "Step 5: read a table TinyETL didn't create (Snowflake's shared sample data)"
SAMPLE_URL="snowflake://${SNOWFLAKE_USER}@${SNOWFLAKE_ACCOUNT}/SNOWFLAKE_SAMPLE_DATA/TPCH_SF1?warehouse=${SNOWFLAKE_WAREHOUSE}"
if [ -n "$SNOWFLAKE_ROLE" ]; then SAMPLE_URL="${SAMPLE_URL}&role=${SNOWFLAKE_ROLE}"; fi
rm -f "$WORK/nation.csv"
if $TINYETL "$SAMPLE_URL#NATION" "$WORK/nation.csv" >/dev/null 2>&1; then
    n=$(($(wc -l < "$WORK/nation.csv") - 1))
    [ "$n" = "25" ] && pass "read 25 rows from SNOWFLAKE_SAMPLE_DATA.TPCH_SF1.NATION ($(head -1 "$WORK/nation.csv"))" \
        || fail "expected 25 nations, got $n"
else
    echo "⚠️  Could not read SNOWFLAKE_SAMPLE_DATA (it may not be shared with this account); skipping"
fi

echo ""
echo "Step 6: --source-query (a CTE, an aggregate and ORDER BY, run in Snowflake)"
rm -f "$WORK/regions.csv"
if $TINYETL "$SAMPLE_URL" "$WORK/regions.csv" --source-query \
    "WITH n AS (SELECT * FROM NATION) SELECT N_REGIONKEY AS region, COUNT(*) AS nations FROM n GROUP BY 1 ORDER BY 1;" \
    >/dev/null 2>&1; then
    got=$(tail -n +2 "$WORK/regions.csv" | paste -sd'|' -)
    [ "$got" = "0,5|1,5|2,5|3,5|4,5" ] && pass "5 regions with 5 nations each" \
        || fail "expected 5 regions of 5 nations, got $got"
else
    echo "⚠️  Could not query SNOWFLAKE_SAMPLE_DATA (it may not be shared with this account); skipping"
fi

echo ""
if [ "$FAILED" = 0 ]; then
    echo "✅ Example 22 completed successfully"
else
    echo "❌ Example 22 had failures"
    exit 1
fi
