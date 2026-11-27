# Example 29: Text As Written

Zip codes, phone numbers and IDs often look like numbers but aren't: `01826` must not load as `1826`. TinyETL reads CSV values as the text in the file and converts a value only when that keeps it as written, so codes with leading zeros or a leading `+` stay text. A schema file's `String` columns keep the source text exactly:

```bash
tinyetl people.csv people.duckdb#people --schema-file schema.yaml   # String columns as written
tinyetl export.csv raw.parquet --source-option infer_types=false    # every column as text
```

`run.sh` checks:

1. With no schema file, `01826`, `012345`, `+15550104` and `0012.50` keep their text in CSV and SQLite
2. A schema file's String columns keep `01826`, `0012.50`, `3/4/2021`, `2024-01-01` and `true` exactly, in DuckDB, SQLite and Parquet; a column the schema doesn't list is still inferred
3. JSON strings in String columns stay strings, and a JSON number becomes text
4. `on_mismatch`: `fail` (the default) names the column and the value, `null` writes NULL and reports how many values it replaced, `raw` keeps the text in a file target
5. `infer_types=false` makes every column text
6. Reloading with `--truncate` gives the same rows

```bash
./run.sh
```
