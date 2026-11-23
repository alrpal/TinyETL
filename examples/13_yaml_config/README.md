# Example 13: YAML Job Files

A job file holds the same settings as a command line, so a job can be saved, reviewed and rerun:

```bash
tinyetl run basic_config.yaml
```

Only `source` and `target` are required:

```yaml
source: employees.csv
target: employees_output.json
```

Every other key has the name of a command-line flag (`batch_size` is `--batch-size`, `transform_file` is `--transform-file`). A key TinyETL doesn't know is an error, so a typo fails instead of being silently ignored.

## Files

- `basic_config.yaml`: CSV to JSON with a multi-line Lua transform that adds full name, annual salary and years of service
- `employees.csv`: the input

## What `run.sh` shows

1. Running `basic_config.yaml`
2. `${VAR:-default}`: an environment variable with a default value
3. `tinyetl generate-config`: turning a command line into a job file
4. A misspelled key (`batchsize`) failing with an error

```bash
./run.sh
```

## Command line vs job file

```bash
tinyetl employees.csv output.json \
  --batch-size 1000 \
  --transform "full_name = row.first_name .. ' ' .. row.last_name; annual_salary = row.monthly_salary * 12"
```

```yaml
source: employees.csv
target: output.json
batch_size: 1000
transform: |
  full_name = row.first_name .. " " .. row.last_name
  annual_salary = row.monthly_salary * 12
```

A source or target with options (HTTP headers, S3 credentials, CSV delimiter) uses a map:

```yaml
source:
  uri: https://example.com/export.csv
  options:
    auth.bearer: ${API_TOKEN}
target: export.parquet
```

See "YAML Configuration Files" in the main README for every key.
