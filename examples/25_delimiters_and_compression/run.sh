#!/bin/bash
# Example 25: delimited files (issue #21) and compressed files (issue #12)
#
#   1. Semicolon, pipe and tab separated .csv files are detected and read like the comma file
#   2. .tsv files are written and read tab-separated
#   3. The delimiter option sets the delimiter for reading and writing
#   4. Every file format round-trips through .gz and .zst
#   5. Files compressed by other tools (gzip, zstd), including concatenated gzip, are read
#   6. A .csv.gz is downloaded over HTTP (a local server)
#   7. Optional, with network access: the CrateDB dataset from issue #12
#
# Each step checks its result and the script exits non-zero on a wrong answer.
# Set TINYETL_EXAMPLES_OFFLINE=1 to skip step 7.

set -e
cd "$(dirname "$0")"

TINYETL=${TINYETL:-$(pwd)/../../target/release/tinyetl}
COMPARE="python3 -I $(pwd)/compare.py"
WORK=$(mktemp -d)
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT
run() { "$TINYETL" "$@" --log-level warn; }
pass() { echo "  ✓ $1"; }
# Read a source into a fresh JSON file (an existing JSON target is appended to)
readback() { rm -f "$WORK/back.json"; run "$@" "$WORK/back.json"; }

echo "Running Example 25: delimiters and compression"

# Source data: quoted fields that contain the other delimiters, a decimal column and a date
cat > "$WORK/orders.csv" <<'EOF'
id,customer,city,amount,ordered_at
1,"Smith; Jr.",Berlin,12.50,2024-01-15
2,Ana|Lopez,"Porto, PT",7.25,2024-02-01
3,"Lee	Min",Seoul,100.00,2024-02-29
4,O'Brien,Dublin,0.99,2024-03-10
5,"Quote ""Q"" Ltd",Oslo,42,2024-04-01
EOF
run "$WORK/orders.csv" "$WORK/expected.json"

echo ""
echo "1. Other delimiters are detected (#21)"
python3 -I - "$WORK" <<'EOF'
import csv, sys
work = sys.argv[1]
rows = list(csv.reader(open(f"{work}/orders.csv", newline="")))
for name, delim in [("semicolon", ";"), ("pipe", "|"), ("tab", "\t")]:
    with open(f"{work}/orders_{name}.csv", "w", newline="") as f:
        csv.writer(f, delimiter=delim, lineterminator="\n").writerows(rows)
EOF
for name in semicolon pipe tab; do
    run "$WORK/orders_$name.csv" "$WORK/from_$name.json"
    $COMPARE "$WORK/expected.json" "$WORK/from_$name.json"
    pass "$name-separated .csv read like the comma file"
done

echo ""
echo "2. .tsv files are tab-separated (#21)"
run "$WORK/orders.csv" "$WORK/orders.tsv"
head -1 "$WORK/orders.tsv" | grep -q $'^id\tcustomer\tcity\tamount\tordered_at$'
run "$WORK/orders.tsv" "$WORK/from_tsv.json"
$COMPARE "$WORK/expected.json" "$WORK/from_tsv.json"
pass "written with tabs and read back"

echo ""
echo "3. The delimiter option (#21)"
run "$WORK/orders.csv" "$WORK/piped.csv" --target-option "delimiter=|"
head -1 "$WORK/piped.csv" | grep -q '^id|customer|city|amount|ordered_at$'
run "$WORK/piped.csv" "$WORK/from_piped.json" --source-option "delimiter=|"
$COMPARE "$WORK/expected.json" "$WORK/from_piped.json"
pass "written and read with delimiter=|"
if run "$WORK/orders.csv" "$WORK/x.parquet" --target-option "delimiter=|" 2>/dev/null; then
    echo "FAIL: a delimiter for a Parquet target was accepted"; exit 1
fi
pass "a delimiter for a format without one is an error"

echo ""
echo "4. Every format round-trips through .gz and .zst (#12)"
for format in csv tsv json jsonl parquet avro xlsx; do
    for codec in gz zst; do
        file="$WORK/orders.$format.$codec"
        run "$WORK/orders.csv" "$file"
        magic=$(head -c 4 "$file" | od -An -tx1 | tr -d ' \n')
        case "$codec" in
            gz) [[ "$magic" == 1f8b* ]] || { echo "FAIL: $file isn't gzip ($magic)"; exit 1; } ;;
            zst) [[ "$magic" == 28b52ffd ]] || { echo "FAIL: $file isn't zstd ($magic)"; exit 1; } ;;
        esac
        readback "$file"
        $COMPARE "$WORK/expected.json" "$WORK/back.json"
    done
    pass "$format.gz and $format.zst"
done

echo ""
echo "5. Files compressed by other tools (#12)"
gzip -c "$WORK/orders.csv" > "$WORK/by_gzip.csv.gz"
readback "$WORK/by_gzip.csv.gz"
$COMPARE "$WORK/expected.json" "$WORK/back.json"
pass "gzip"
# Concatenated gzip members, as produced by `cat a.gz b.gz` or parallel compressors
head -3 "$WORK/orders.csv" | gzip -c > "$WORK/multi.csv.gz"
tail -n +4 "$WORK/orders.csv" | gzip -c >> "$WORK/multi.csv.gz"
readback "$WORK/multi.csv.gz"
$COMPARE "$WORK/expected.json" "$WORK/back.json"
pass "concatenated gzip members"
if command -v zstd >/dev/null; then
    zstd -q -c "$WORK/orders_semicolon.csv" > "$WORK/by_zstd.csv.zst"
    readback "$WORK/by_zstd.csv.zst"
    $COMPARE "$WORK/expected.json" "$WORK/back.json"
    pass "zstd (and a semicolon delimiter inside the compressed file)"
else
    echo "  ⚠️  zstd command not installed; skipped"
fi

echo ""
echo "6. A .csv.gz over HTTP (#12)"
PORT=$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -I -m http.server "$PORT" --bind 127.0.0.1 --directory "$WORK" >/dev/null 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do
    python3 -I -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:$PORT/')" 2>/dev/null && break
    sleep 0.1
done
readback "http://127.0.0.1:$PORT/by_gzip.csv.gz"
$COMPARE "$WORK/expected.json" "$WORK/back.json"
pass "downloaded and decompressed"

echo ""
echo "7. The CrateDB dataset from issue #12"
URL=https://cdn.crate.io/downloads/datasets/cratedb-datasets/misc/worldcities.csv.gz
if [ -n "$TINYETL_EXAMPLES_OFFLINE" ]; then
    echo "  ⚠️  skipped (TINYETL_EXAMPLES_OFFLINE is set)"
elif ! curl -sfI -m 10 "$URL" >/dev/null 2>&1; then
    echo "  ⚠️  skipped: $URL isn't reachable"
else
    run "$URL" "$WORK/worldcities.jsonl"
    rows=$(wc -l < "$WORK/worldcities.jsonl" | tr -d ' ')
    [ "$rows" -eq 47868 ] || { echo "FAIL: expected 47868 cities, got $rows"; exit 1; }
    pass "worldcities.csv.gz: $rows rows"
fi

echo ""
echo "✅ Example 25 completed successfully"
