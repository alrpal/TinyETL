# Example 26: JSON Documents

Checks JSON Lines in `.json` files ([#16](https://github.com/alrpal/TinyETL/issues/16)), nested JSON kept as JSON ([#3](https://github.com/alrpal/TinyETL/issues/3)) and SQLite JSON columns stored as BLOBs ([#13](https://github.com/alrpal/TinyETL/issues/13)). Every step checks its result and the script fails on a wrong answer.

```bash
./run.sh
TINYETL_EXAMPLES_OFFLINE=1 ./run.sh   # skip the download in step 7
```

1. A `.json` file holding JSON Lines (the GH Archive layout), pretty-printed objects back to back, and a file with a byte order mark each load all 60 rows
2. Nested objects and arrays stay nested in JSON output, and a key missing from the first rows (`org`) is kept for the rows that have it
3. SQLite: all 60 rows and 10 orgs are written; `json_extract(actor, '$.login')` reads the nested values
4. SQLite `JSON` columns holding BLOB or TEXT documents are read as JSON; NULL stays NULL
5. DuckDB: nested values are loaded into JSON columns and read back nested
6. PostgreSQL and MySQL (when the `test_env` containers are running): nested values land in `jsonb` / `JSON` columns, and `actor->>'login'` works
7. With network access: the GH Archive hour `2015-01-01-15.json.gz` loads 11,351 events, 3,245 with an org

Needs Python 3 (for test data and checks) and, for step 6, Docker with the `test_env` containers.
