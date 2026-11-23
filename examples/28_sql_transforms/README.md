# Example 28: SQL Transforms

`--sql` (YAML `sql:`) reshapes data on the way with DuckDB SQL. The source rows are a table named `source`, and the query sees all of them:

```bash
tinyetl sales.csv totals.parquet \
  --sql "SELECT region, COUNT(*) AS orders, SUM(amount) AS total FROM source GROUP BY region"
```

`run.sh` checks:

1. An aggregate over 200,000 rows read in batches: counts and exact decimal sums match Python
2. A running total with a window function, and the latest row per id with `QUALIFY`
3. Nested JSON: `->>` on nested objects, and `list()` / struct results written as JSON
4. Loading the result into SQLite, and into PostgreSQL when the `test_env` container is running
5. `sql:` in a YAML job file
6. A schema file checks the source rows before the query runs
7. A wrong table name, and `--sql` together with a Lua transform, are errors

```bash
./run.sh
```
