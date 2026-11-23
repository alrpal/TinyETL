#!/bin/bash
# Example 13: YAML job files
set -e
cd "$(dirname "$0")"

TINYETL="../../target/release/tinyetl"
if [ ! -f "$TINYETL" ]; then
    echo "Build TinyETL first: cargo build --release"
    exit 1
fi

echo "=== 1. Run a job file: CSV to JSON with a Lua transform ==="
cat basic_config.yaml
$TINYETL run basic_config.yaml
echo "First record:"
head -n 12 employees_output.json
grep -q '"years_service"' employees_output.json

echo ""
echo "=== 2. Environment variables, with a default ==="
cat > /tmp/tinyetl_example13_env.yaml <<'EOF'
source: employees.csv
target: ${EXAMPLE13_OUTPUT:-employees_output.csv}
truncate: true
EOF
cat /tmp/tinyetl_example13_env.yaml
$TINYETL run /tmp/tinyetl_example13_env.yaml
test "$(wc -l < employees_output.csv)" -eq "$(wc -l < employees.csv)"

echo ""
echo "=== 3. Turn a command line into a job file ==="
$TINYETL generate-config employees.csv employees.db --batch-size 500 \
    --transform "full_name = row.first_name .. ' ' .. row.last_name"

echo ""
echo "=== 4. A typo is an error, not ignored ==="
cat > /tmp/tinyetl_example13_typo.yaml <<'EOF'
source: employees.csv
target: employees_output.json
batchsize: 500
EOF
if $TINYETL run /tmp/tinyetl_example13_typo.yaml; then
    echo "FAIL: the misspelled key was accepted"
    exit 1
fi

rm -f /tmp/tinyetl_example13_env.yaml /tmp/tinyetl_example13_typo.yaml
echo ""
echo "✅ Example 13 completed successfully"
