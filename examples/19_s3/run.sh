#!/bin/bash
# Example 19: S3 read/write round trip against a real bucket
#
# Requires TINYETL_S3_BUCKET, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY and AWS_REGION,
# either exported in the shell or defined in a .env file at the repository root.

set -e
cd "$(dirname "$0")"

echo "Running Example 19: S3 round trip"

if [ -f ../../.env ]; then
    set -a
    . ../../.env
    set +a
fi

if [ -z "$TINYETL_S3_BUCKET" ] || [ -z "$AWS_ACCESS_KEY_ID" ] || [ -z "$AWS_SECRET_ACCESS_KEY" ]; then
    echo "⚠️  SKIPPED: TINYETL_S3_BUCKET, AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are not all set"
    echo "   See README.md in this directory for the setup."
    exit 0
fi

TINYETL=../../target/release/tinyetl
PREFIX="s3://$TINYETL_S3_BUCKET/tinyetl-example"
EXPECTED=6
rm -f output_*.json

count_records() {
    grep -o '"employee_id"' "$1" | wc -l | tr -d ' '
}

check() {
    if [ "$(count_records "$1")" -eq "$EXPECTED" ]; then
        echo "✅ PASS: $2 ($EXPECTED records)"
    else
        echo "❌ FAIL: $2 - expected $EXPECTED records, found $(count_records "$1")"
        exit 1
    fi
}

echo "Step 1: employees.csv -> $PREFIX/employees.parquet (YAML config)"
$TINYETL run upload_config.yaml

echo "Step 2: $PREFIX/employees.parquet -> output_employees.json (YAML config)"
$TINYETL run download_config.yaml
check output_employees.json "Parquet round trip"

echo "Step 3: CSV round trip from the CLI, run twice to confirm overwrite"
$TINYETL employees.csv "$PREFIX/employees.csv"
$TINYETL employees.csv "$PREFIX/employees.csv"
$TINYETL "$PREFIX/employees.csv" output_from_csv.json
check output_from_csv.json "CSV round trip with overwrite"

echo "✅ Example 19 completed successfully"
