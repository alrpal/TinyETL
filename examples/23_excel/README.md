# Example 23: Excel

Reads and writes Excel workbooks, checking the results:

1. `employees.csv` → `employees.xlsx` → CSV, compared value by value (decimals, dates, booleans, Unicode, a blank cell) and in column order.
2. Writes and reads a named sheet (`book.xlsx#Staff`), and checks that asking for a sheet that doesn't exist fails with the list of sheets.
3. Adds computed columns with `--transform` while writing Excel.
4. Loads the workbook into SQLite and compares the result.

```bash
cargo build --release
bash examples/23_excel/run.sh
```

Outputs go to a temporary directory, so the example leaves no files behind.

## Commands

```bash
tinyetl employees.csv employees.xlsx                 # write; the sheet is named Sheet1
tinyetl employees.csv "book.xlsx#Staff"              # write a named sheet
tinyetl employees.xlsx employees.csv                 # read the first sheet
tinyetl "book.xlsx#Staff" staff.json                 # read a named sheet
tinyetl employees.xlsx "staff.db#employees"          # Excel into SQLite
```

TinyETL reads `.xlsx`, `.xlsm`, `.xlsb`, `.xls` and `.ods`, and writes `.xlsx`. See the Excel section of the main README for how types, headers and large sheets are handled.
