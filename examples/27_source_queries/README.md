# Example 27: Source Queries

`--source-query` (YAML `source_query`) reads the result of a SQL query instead of a whole table. The query runs on the source database, as written.

```bash
tinyetl "postgresql://user@host/shop" orders.parquet \
  --source-query "SELECT id, total FROM orders WHERE created_at >= now() - interval '1 day'"
```

`run.sh` loads the same orders into each database and reads them back through one query with a `WITH`, a filter, `GROUP BY` and `ORDER BY`, checking the result:

1. SQLite and DuckDB always; PostgreSQL, MySQL and SQL Server when their `test_env` containers are running
2. `source_query` in a YAML job file
3. The mistakes that should be errors: a `#table` together with a query, a query on a CSV file, and a SQL syntax error, which is reported as the database's own message

ODBC source queries are tested in `test_env/odbc` (SQL Server, PostgreSQL, MySQL and SQLite drivers), and Snowflake in example 22.

```bash
./run.sh
```
