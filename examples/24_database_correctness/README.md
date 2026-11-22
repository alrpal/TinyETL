# Example 24: Database correctness

Checks that reading from and loading into databases is correct and all-or-nothing. It runs against SQLite and DuckDB always, and against PostgreSQL, MySQL and SQL Server when their `test_env` containers are running (`cd test_env && docker-compose up -d`); missing databases are skipped.

For each database:

1. **Reads return every row exactly once.** 300,000 generated rows are loaded, read back, and compared value by value. Before v0.11.1, sources paged with `LIMIT … OFFSET` and no `ORDER BY`, and PostgreSQL returned duplicated and missing rows for tables larger than a quarter of its `shared_buffers` (the rows here are wide enough to cross that line).
2. **A failed load changes nothing.** A 10,000-row file with one row that fails schema-file validation in the middle is loaded in batches of 1,000, once appending and once with `--truncate`. The table must still hold its original 3 rows.
3. **Successful loads behave as documented:** `--truncate` replaces the rows, a plain run appends, and `--truncate` with an empty source empties the table.

```bash
cargo build --release
bash examples/24_database_correctness/run.sh

# More rows, or a different binary (e.g. to show a regression)
ROWS=1000000 bash examples/24_database_correctness/run.sh
TINYETL=/path/to/other/tinyetl bash examples/24_database_correctness/run.sh
```

The run takes under a minute with all five databases. It leaves two tables in each server database, `tinyetl_ex24_read` and `tinyetl_ex24_load`, which it replaces on the next run.
