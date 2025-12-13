# TinyETL Excel Support Example

This example demonstrates TinyETL's Excel connector capabilities for reading from and writing to Excel files (.xlsx and .xls formats).

## Features Demonstrated

1. **CSV to Excel** - Convert CSV files to Excel format
2. **Excel to CSV** - Read Excel files and export to CSV
3. **Excel to Excel** - Copy Excel data with custom sheet names
4. **Excel to JSON** - Convert Excel data to JSON format
5. **Transformations** - Apply Lua transformations when writing to Excel
6. **Sheet Names** - Specify custom sheet names using `file.xlsx#SheetName` syntax
7. **Preview Mode** - Inspect Excel data before processing

## Running the Example

```bash
cd examples/19_excel_support
chmod +x run.sh
./run.sh
```

## Excel-Specific Features

### Sheet Name Specification

You can specify a sheet name for both reading and writing:

```bash
# Read from specific sheet
tinyetl "data.xlsx#Sheet2" output.csv

# Write to specific sheet (defaults to "Sheet1" if not specified)
tinyetl data.csv "output.xlsx#EmployeeData"
```

### Supported Excel Formats

- **.xlsx** - Modern Excel format (Office 2007+)
- **.xls** - Legacy Excel format (Office 97-2003)

### Data Type Handling

TinyETL automatically converts between Excel and TinyETL data types:

- **Numbers** - Excel integers and floats → Integer/Decimal
- **Text** - Excel strings → String (with date parsing attempt)
- **Booleans** - Excel TRUE/FALSE → Boolean
- **Dates** - Excel date values → DateTime (converted to strings)
- **Empty cells** - Excel empty → Null

### Column Order Preservation

Excel connector preserves column order from the header row, ensuring consistent output.

## Example Commands

```bash
# Basic conversion
tinyetl employees.csv employees.xlsx
tinyetl employees.xlsx employees.csv

# With sheet names
tinyetl "sales.xlsx#Q4Data" report.csv
tinyetl data.csv "report.xlsx#Summary"

# With transformations
tinyetl employees.csv output.xlsx \
  --transform "full_name=row.first_name .. ' ' .. row.last_name"

# Preview Excel data
tinyetl employees.xlsx output.json --preview 5

# Excel to database
tinyetl employees.xlsx "sqlite:///employees.db#staff"

# Database to Excel
tinyetl "postgres://user@localhost/db#orders" orders.xlsx
```

## Notes

- Excel files are read entirely into memory for processing
- Large Excel files (>100k rows) may be slower than CSV or Parquet
- Excel target writes occur at finalization (after all batches accumulated)
- Sheet names default to "Sheet1" if not specified
- Excel files don't support true append mode (truncate-and-write only)

## Use Cases

- **Report Generation** - Create formatted Excel reports from databases
- **Data Exchange** - Share data with non-technical users who prefer Excel
- **Legacy System Integration** - Import/export from systems using Excel
- **Financial Data** - Handle spreadsheet-based financial data with precision
- **Data Validation** - Preview and validate Excel files before database import
