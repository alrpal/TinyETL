# Example 25: Delimiters and Compression

Checks the delimited-file support from [#21](https://github.com/alrpal/TinyETL/issues/21) and the compressed-file support from [#12](https://github.com/alrpal/TinyETL/issues/12). Every step compares its output with the expected rows and the script fails on a wrong answer.

```bash
./run.sh
TINYETL_EXAMPLES_OFFLINE=1 ./run.sh   # skip the download in step 7
```

1. Semicolon, pipe and tab separated `.csv` files are detected, including quoted fields that contain the other delimiters
2. `.tsv` files are written and read tab-separated
3. `--target-option delimiter=|` / `--source-option delimiter=|`; a delimiter for a Parquet target is an error
4. CSV, TSV, JSON, JSON Lines, Parquet, Avro and Excel each round-trip through `.gz` and `.zst`
5. Files compressed with `gzip` (also several gzip members concatenated) and `zstd` are read
6. A `.csv.gz` is downloaded from a local HTTP server and decompressed
7. With network access: the CrateDB `worldcities.csv.gz` from #12 loads all 47,868 rows

Needs Python 3 (for test data and checks). `zstd` is used if installed.
