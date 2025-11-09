#!/bin/bash

# Schema File Example
# This example demonstrates how to use a schema file to enforce data types and constraints

echo "=== TinyETL Schema File Example ==="

# First, let's see what happens without a schema file (auto-detection)
echo "1. Running without schema file (auto-detection):"
echo "./target/release/tinyetl employees.csv employees_auto.db#employees --preview 5"
echo ""

../../target/release/tinyetl employees.csv employees_auto.db#employees --preview 5

echo ""
echo "2. Running with schema file (enforced schema):"
echo "./target/release/tinyetl employees.csv employees_schema.db#employees --schema-file ../schemas/employee_schema.yaml --preview 5"
echo ""

../../target/release/tinyetl employees.csv employees_schema.db#employees --schema-file ../schemas/employee_schema.yaml --preview 5

echo ""
echo "3. Full transfer with schema validation:"
echo "./target/release/tinyetl employees.csv employees_final.db#employees --schema-file ../schemas/employee_schema.yaml"
echo ""

../../target/release/tinyetl employees.csv employees_final.db#employees --schema-file ../schemas/employee_schema.yaml

echo ""
echo "=== Schema file ensures data consistency and type safety! ==="
