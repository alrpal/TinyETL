# Example 13: YAML Configuration Files

This example demonstrates TinyETL's new YAML configuration file functionality, which allows you to define complex ETL jobs in reusable configuration files instead of long command-line arguments.

## What's New

TinyETL now supports running jobs from YAML configuration files using the `run` subcommand:

```bash
tinyetl run config.yaml
```

## Key Features

### 1. **Structured Configuration**
- Clean, readable YAML format
- All CLI options available in config files
- Better organization for complex ETL jobs

### 2. **Environment Variable Substitution**
- Use `${VAR_NAME}` syntax for environment variables
- Perfect for secrets, connection strings, and dynamic paths
- Similar to how `--source-secret-id` resolves to `TINYETL_SECRET_{id}`

### 3. **Multi-line Transforms**
- Inline Lua transforms with proper formatting
- No more escaping complex transform logic in command lines
- Support for detailed comments and documentation

### 4. **Version Control Friendly**
- Store and version your ETL job definitions
- Share configurations across teams
- Document complex transformation logic

## Example Files

### `basic_config.yaml`
A simple CSV to JSON transformation with inline Lua transforms:
- Demonstrates basic source/target configuration
- Shows multi-line Lua transforms
- Calculates derived fields (full name, annual salary, years of service)

### `advanced_config.yaml` 
Advanced database-to-file transfer with environment variables:
- PostgreSQL source with environment variable substitution
- External schema and transform files
- Production-ready configuration pattern

### `s3_config.yaml`
Multi-database example (MySQL to S3):
- Complex connection strings with environment variables
- S3 output with AWS credentials from environment
- Advanced transform logic with conditional statements

## Running the Examples

```bash
# Run the demo script
./run.sh

# Or run individual examples
tinyetl run basic_config.yaml
tinyetl run advanced_config.yaml  # Requires DB setup
tinyetl run s3_config.yaml        # Requires AWS credentials
```

## YAML Configuration Schema

```yaml
version: 1

source:
  uri: "connection_string_or_file_path"
  # Additional source parameters can be added here

target:
  uri: "connection_string_or_file_path"  
  # Additional target parameters can be added here

options:
  batch_size: 10000           # Number of rows per batch
  infer_schema: true          # Auto-detect column types
  schema_file: "schema.yaml"  # Override with external schema
  preview: 10                 # Show N rows without transfer
  dry_run: false              # Validate without transferring
  log_level: "info"           # info, warn, error
  skip_existing: false        # Skip if target exists
  truncate: false             # Truncate target before writing
  transform: 
    type: file                # External transform file
    value: "script.lua"       # Path to Lua file containing a 'transform' function
  source_type: "csv"          # Force source file type
```

## Environment Variable Examples

Set environment variables and use them in your config:

```bash
export DB_HOST="localhost"
export DB_USER="myuser" 
export DB_PASS="mypass"
export OUTPUT_DIR="/data/exports"

# Your config can then use:
# uri: "postgres://${DB_USER}:${DB_PASS}@${DB_HOST}/mydb"
# uri: "${OUTPUT_DIR}/output.parquet"
```

## Migration from CLI

**Before (CLI):**
```bash
tinyetl employees.csv output.json \
  --batch-size 1000 \
  --transform "full_name = row.first_name .. ' ' .. row.last_name; annual_salary = row.monthly_salary * 12" \
  --preview 5
```

**After (Config File):**
```yaml
version: 1
source:
  uri: "employees.csv"
target:
  uri: "output.json"
options:
  batch_size: 1000
  preview: 5
  transform:
    type: script
    value: |
      -- Calculate full name and annual salary
      full_name = row.first_name .. " " .. row.last_name
      annual_salary = row.monthly_salary * 12
```

```bash
tinyetl run my_job.yaml
```

## Benefits

- **Maintainability**: Complex jobs are easier to read and modify
- **Reusability**: Same config can be used across environments
- **Documentation**: Comments and structure make jobs self-documenting  
- **Security**: Environment variables keep secrets out of config files
- **Collaboration**: Teams can share and review ETL job definitions
- **Automation**: Perfect for CI/CD pipelines and scheduled jobs

## Next Steps

1. Try the basic example: `./run.sh`
2. Create your own config file for your data
3. Use environment variables for sensitive information
4. Version control your ETL job definitions
5. Set up automated pipelines using config files
