//! Text sources: CSV (also as TSV and gzip) and JSON. Every CSV value is text, so the column
//! types come from detection alone.

use crate::{check, compare, report, run, try_detect, Check};
use std::io::Write;
use tinyetl::schema::DataType::{self, Boolean, Date, DateTime, Decimal, Integer, Json};

const STRING: DataType = DataType::String;

/// A column of text values and what they should come out as
struct Column {
    name: &'static str,
    raw: &'static [&'static str],
    data_type: DataType,
    values: &'static [&'static str],
}

const fn col(
    name: &'static str,
    raw: &'static [&'static str],
    data_type: DataType,
    values: &'static [&'static str],
) -> Column {
    Column {
        name,
        raw,
        data_type,
        values,
    }
}

/// The text cases. `values` are the first rows as targets should receive them.
const COLUMNS: &[Column] = &[
    // Numbers
    col(
        "int",
        &["1", "-42", "0", "9223372036854775807", ""],
        Integer,
        &[
            "int 1",
            "int -42",
            "int 0",
            "int 9223372036854775807",
            "null",
        ],
    ),
    col(
        "int_padded",
        &[" 42", "7 ", "  -3  "],
        Integer,
        &["int 42", "int 7", "int -3"],
    ),
    col(
        "big_int",
        &["12345678901234567890", "1"],
        Decimal,
        &["dec 12345678901234567890", "dec 1"],
    ),
    col(
        "decimal",
        &["3.14", "-0.5", "10"],
        Decimal,
        &["dec 3.14", "dec -0.5", "dec 10"],
    ),
    col(
        "decimal_scale",
        &["1.50", "2.00", "0.10"],
        Decimal,
        &["dec 1.50", "dec 2.00", "dec 0.10"],
    ),
    col(
        "int_and_decimal",
        &["1", "2.5", "3"],
        Decimal,
        &["dec 1", "dec 2.5", "dec 3"],
    ),
    // 28 decimal places fit exactly; 33 significant digits don't, so the text is kept
    col(
        "decimal_28dp",
        &["0.1234567890123456789012345678"],
        Decimal,
        &["dec 0.1234567890123456789012345678"],
    ),
    col(
        "decimal_too_precise",
        &["3.14159265358979323846264338327950", "1.5"],
        STRING,
        &["str 3.14159265358979323846264338327950", "str 1.5"],
    ),
    // decision: scientific notation is a number (as in Power BI and spreadsheets)
    col(
        "scientific",
        &["1.5e3", "2E-4", "-3.2e+5"],
        Decimal,
        &["dec 1500", "dec 0.0002", "dec -320000"],
    ),
    // decision: US thousands separators are numbers when every value groups digits in threes
    col(
        "thousands",
        &["1,234", "12,345,678", "999"],
        Integer,
        &["int 1234", "int 12345678", "int 999"],
    ),
    col(
        "thousands_decimal",
        &["1,234.56", "-9,876.50"],
        Decimal,
        &["dec 1234.56", "dec -9876.50"],
    ),
    // decision: currency and percent signs keep the column text (no locale guessing)
    col(
        "currency",
        &["$1,234.50", "$0.99", "-$5.00"],
        STRING,
        &["str $1,234.50", "str $0.99", "str -$5.00"],
    ),
    col(
        "percent",
        &["45%", "12.5%", "-3%"],
        STRING,
        &["str 45%", "str 12.5%", "str -3%"],
    ),
    col(
        "paren_negative",
        &["(123)", "(45.60)"],
        STRING,
        &["str (123)", "str (45.60)"],
    ),
    col(
        "nan_inf",
        &["NaN", "Infinity", "1.5"],
        STRING,
        &["str NaN", "str Infinity", "str 1.5"],
    ),
    col(
        "epoch",
        &["1705312200", "1705398600"],
        Integer,
        &["int 1705312200", "int 1705398600"],
    ),
    col(
        "year",
        &["2024", "1999"],
        Integer,
        &["int 2024", "int 1999"],
    ),
    // Codes that look like numbers
    col(
        "leading_zero",
        &["01826", "00501", "02134"],
        STRING,
        &["str 01826", "str 00501", "str 02134"],
    ),
    col(
        "plus_sign",
        &["+15551234", "+447911123456"],
        STRING,
        &["str +15551234", "str +447911123456"],
    ),
    col(
        "zip_plus4",
        &["12345-6789", "02134-1234"],
        STRING,
        &["str 12345-6789", "str 02134-1234"],
    ),
    col(
        "phone",
        &["555-123-4567", "(555) 123-4567"],
        STRING,
        &["str 555-123-4567", "str (555) 123-4567"],
    ),
    col(
        "ip_address",
        &["192.168.0.1", "10.0.0.255"],
        STRING,
        &["str 192.168.0.1", "str 10.0.0.255"],
    ),
    col(
        "version",
        &["1.2.3", "10.4"],
        STRING,
        &["str 1.2.3", "str 10.4"],
    ),
    col(
        "uuid",
        &["123e4567-e89b-12d3-a456-426614174000"],
        STRING,
        &["str 123e4567-e89b-12d3-a456-426614174000"],
    ),
    // Booleans
    col(
        "bool",
        &["true", "false", ""],
        Boolean,
        &["bool true", "bool false", "null"],
    ),
    col(
        "bool_case",
        &["TRUE", "False", "true"],
        Boolean,
        &["bool true", "bool false", "bool true"],
    ),
    col(
        "yes_no",
        &["yes", "no", "Y", "N"],
        STRING,
        &["str yes", "str no", "str Y", "str N"],
    ),
    col(
        "zero_one",
        &["0", "1", "1"],
        Integer,
        &["int 0", "int 1", "int 1"],
    ),
    // Dates: a column of dates is a Date column
    col(
        "iso_date",
        &["2024-01-15", "2023-12-31", ""],
        Date,
        &["date 2024-01-15", "date 2023-12-31", "null"],
    ),
    col(
        "us_date",
        &["1/15/2024", "12/31/2023", "2/29/2024"],
        Date,
        &["date 2024-01-15", "date 2023-12-31", "date 2024-02-29"],
    ),
    col(
        "eu_date",
        &["15/01/2024", "31/12/2023"],
        Date,
        &["date 2024-01-15", "date 2023-12-31"],
    ),
    // One layout per column: 13/04 rules out month-first, so 03/04 is 3 April
    col(
        "day_first_column",
        &["03/04/2024", "13/04/2024", "25/12/2023"],
        Date,
        &["date 2024-04-03", "date 2024-04-13", "date 2023-12-25"],
    ),
    // Nothing rules either out: month-first, like the rest of the date parsing
    col(
        "ambiguous_dates",
        &["03/04/2024", "05/06/2024"],
        Date,
        &["date 2024-03-04", "date 2024-05-06"],
    ),
    col(
        "dotted_date",
        &["15.01.2024", "31.12.2023"],
        Date,
        &["date 2024-01-15", "date 2023-12-31"],
    ),
    col(
        "month_names",
        &["Jan 15, 2024", "December 31, 2023", "15 Jan 2024"],
        Date,
        &["date 2024-01-15", "date 2023-12-31", "date 2024-01-15"],
    ),
    col(
        "datetime",
        &[
            "2024-01-15 10:30:00",
            "2024-01-15T08:00:00",
            "2024-01-15 23:59",
        ],
        DateTime,
        &[
            "ts 2024-01-15T10:30:00Z",
            "ts 2024-01-15T08:00:00Z",
            "ts 2024-01-15T23:59:00Z",
        ],
    ),
    col(
        "datetime_fraction",
        &["2024-01-15T10:30:00.123", "2024-01-15 10:30:00.123456"],
        DateTime,
        &[
            "ts 2024-01-15T10:30:00.123Z",
            "ts 2024-01-15T10:30:00.123456Z",
        ],
    ),
    col(
        "datetime_zone",
        &[
            "2024-01-15T10:30:00+02:00",
            "2024-01-15T10:30:00Z",
            "2024-01-15T10:30:00-05:00",
        ],
        DateTime,
        &[
            "ts 2024-01-15T08:30:00Z",
            "ts 2024-01-15T10:30:00Z",
            "ts 2024-01-15T15:30:00Z",
        ],
    ),
    col(
        "datetime_ampm",
        &["1/15/2024 10:30 AM", "12/31/2023 11:59:59 PM"],
        DateTime,
        &["ts 2024-01-15T10:30:00Z", "ts 2023-12-31T23:59:59Z"],
    ),
    col(
        "date_and_datetime",
        &["2024-01-15", "2024-01-16 10:00:00"],
        DateTime,
        &["date 2024-01-15", "ts 2024-01-16T10:00:00Z"],
    ),
    // decision: no Time type, so times stay text
    col(
        "time",
        &["13:45", "09:30:15"],
        STRING,
        &["str 13:45", "str 09:30:15"],
    ),
    col(
        "not_a_date",
        &["2024-13-45", "2024-01-15"],
        STRING,
        &["str 2024-13-45", "str 2024-01-15"],
    ),
    // Missing values. decision: only empty fields are NULL; NULL / N/A are text, as written
    col(
        "null_markers",
        &["1", "NULL", "2", "N/A"],
        STRING,
        &["str 1", "str NULL", "str 2", "str N/A"],
    ),
    col(
        "blank_and_spaces",
        &["", "  ", "x"],
        STRING,
        &["null", "str   ", "str x"],
    ),
    col(
        "all_empty",
        &["", "", ""],
        STRING,
        &["null", "null", "null"],
    ),
    // Text
    col(
        "mixed",
        &["1", "abc", "3"],
        STRING,
        &["str 1", "str abc", "str 3"],
    ),
    col(
        "unicode",
        &["café", "日本語", "🎉"],
        STRING,
        &["str café", "str 日本語", "str 🎉"],
    ),
    col(
        "json_text",
        &["{\"a\": 1}", "[1, 2]"],
        STRING,
        &["str {\"a\": 1}", "str [1, 2]"],
    ),
    col(
        "embedded",
        &["a,b", "line1\nline2", "say \"hi\""],
        STRING,
        &["str a,b", "str line1\nline2", "str say \"hi\""],
    ),
];

fn checks() -> Vec<Check> {
    COLUMNS
        .iter()
        .map(|c| check(c.name, c.data_type.clone(), c.values))
        .collect()
}

/// The cases as a delimited file, every non-empty field quoted
fn delimited(delimiter: char) -> String {
    let quote = |s: &str| {
        if s.is_empty() {
            String::new()
        } else {
            format!("\"{}\"", s.replace('"', "\"\""))
        }
    };
    let mut out = COLUMNS
        .iter()
        .map(|c| c.name)
        .collect::<Vec<_>>()
        .join(&delimiter.to_string());
    out.push('\n');
    let rows = COLUMNS.iter().map(|c| c.raw.len()).max().unwrap();
    for i in 0..rows {
        let fields: Vec<String> = COLUMNS
            .iter()
            .map(|c| quote(c.raw.get(i).copied().unwrap_or("")))
            .collect();
        out.push_str(&fields.join(&delimiter.to_string()));
        out.push('\n');
    }
    out
}

#[tokio::test]
async fn csv() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.csv");
    std::fs::write(&path, delimited(',')).unwrap();
    run("CSV", path.to_str().unwrap(), &checks()).await;
}

#[tokio::test]
async fn tsv_and_gzip() {
    let dir = tempfile::tempdir().unwrap();
    let tsv = dir.path().join("cases.tsv");
    std::fs::write(&tsv, delimited('\t')).unwrap();
    run("TSV", tsv.to_str().unwrap(), &checks()).await;

    let gz = dir.path().join("cases.csv.gz");
    let mut encoder =
        flate2::write::GzEncoder::new(std::fs::File::create(&gz).unwrap(), Default::default());
    encoder.write_all(delimited(',').as_bytes()).unwrap();
    encoder.finish().unwrap();
    run("CSV (gzip)", gz.to_str().unwrap(), &checks()).await;
}

/// A file's columns compared with `checks`, or the error that stopped it being read
async fn file_case(label: &str, location: &str, checks: &[Check]) -> Vec<String> {
    let problems = match try_detect(location).await {
        Ok((schema, rows)) => compare(&schema, &rows, checks),
        Err(e) => vec![format!("error: {}", e)],
    };
    problems
        .into_iter()
        .map(|p| format!("{}: {}", label, p))
        .collect()
}

/// File-level oddities: headers, ragged rows, encodings, the sample boundary
#[tokio::test]
async fn csv_files() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_string()
    };
    let mut problems = Vec::new();

    // Repeated and empty header names get unique names, as in Excel sources
    let headers = [
        check("id", Integer, &["int 1"]),
        check("name", STRING, &["str a"]),
        check("id_2", Integer, &["int 2"]),
        check("column_4", STRING, &["str x"]),
    ];
    let location = write("headers.csv", b"id,name,id,\n1,a,2,x\n");
    problems.extend(file_case("repeated headers", &location, &headers).await);

    // A byte order mark isn't part of the first column's name; CRLF line endings
    let bom = [
        check("id", Integer, &["int 1", "int 2"]),
        check("name", STRING, &["str a", "str b"]),
    ];
    let location = write("bom.csv", b"\xEF\xBB\xBFid,name\r\n1,a\r\n2,b\r\n");
    problems.extend(file_case("BOM and CRLF", &location, &bom).await);

    // decision: a short row's missing fields are NULL
    let ragged = [check("c", Integer, &["int 3", "null"])];
    let location = write("ragged.csv", b"a,b,c\n1,2,3\n4,5\n");
    problems.extend(file_case("short row", &location, &ragged).await);

    // decision: a file that isn't UTF-8 is read as Windows-1252, the usual export encoding
    let latin1 = [check("name", STRING, &["str café", "str naïve"])];
    let location = write("latin1.csv", b"name,n\ncaf\xe9,1\nna\xefve,2\n");
    problems.extend(file_case("Windows-1252", &location, &latin1).await);

    // Types come from the first 1,000 rows: a later value that doesn't fit is kept as text
    let mut late = String::from("n\n");
    for i in 1..=1000 {
        late.push_str(&format!("{}\n", i));
    }
    late.push_str("abc\n");
    let (schema, rows) = crate::detect(&write("late.csv", late.as_bytes())).await;
    let tail: Vec<String> = rows[999..].iter().map(|r| crate::render(&r["n"])).collect();
    if schema.columns[0].data_type != Integer || tail != ["int 1000", "str abc"] {
        problems.push(format!(
            "sample boundary: type {:?}, last rows {:?}",
            schema.columns[0].data_type, tail
        ));
    }

    // A header with no rows
    let (schema, _) = crate::detect(&write("empty.csv", b"a,b\n")).await;
    let types: Vec<_> = schema.columns.iter().map(|c| c.data_type.clone()).collect();
    if types != [STRING, STRING] {
        problems.push(format!("header only: types {:?}", types));
    }

    let total = headers.len() + bom.len() + ragged.len() + latin1.len() + 2;
    report("CSV files", total, problems);
}

#[tokio::test]
async fn json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.jsonl");
    let mut lines = vec![
        r#"{"int": 1, "big_int": 12345678901234567890, "float": 3.14, "small_float": 0.1, "float_scale": 1.50, "exponent": 1e3, "int_and_float": 1, "bool": true, "nothing": null, "object": {"a": 1}, "array": [1, "x"], "number_text": "123", "code": "01826", "date_text": "2024-01-15", "datetime_text": "2024-01-15T10:30:00Z", "mixed": 1}"#.to_string(),
        r#"{"int": -42, "big_int": 1, "float": -0.5, "small_float": 0.2, "float_scale": 2.00, "exponent": 2.5E-3, "int_and_float": 2.5, "bool": false, "nothing": null, "object": {"a": [true]}, "array": [], "number_text": "0045", "code": "00501", "date_text": "2023-12-31", "datetime_text": "2024-01-16T00:00:00+02:00", "mixed": "x"}"#.to_string(),
    ];
    // A key that first appears after the 1,000-row sample
    for i in 0..1000 {
        lines.push(format!(r#"{{"int": {}}}"#, i));
    }
    lines.push(r#"{"int": 7, "late": "here"}"#.to_string());
    std::fs::write(&path, lines.join("\n")).unwrap();

    let checks = [
        check("int", Integer, &["int 1", "int -42"]),
        check("big_int", Decimal, &["dec 12345678901234567890", "dec 1"]),
        check("float", Decimal, &["dec 3.14", "dec -0.5"]),
        check("small_float", Decimal, &["dec 0.1", "dec 0.2"]),
        // JSON numbers have no scale: 1.50 is the number 1.5
        check("float_scale", Decimal, &["dec 1.5", "dec 2"]),
        check("exponent", Decimal, &["dec 1000", "dec 0.0025"]),
        check("int_and_float", Decimal, &["dec 1", "dec 2.5"]),
        check("bool", Boolean, &["bool true", "bool false"]),
        check("nothing", STRING, &["null", "null"]),
        check("object", Json, &["json {\"a\":1}", "json {\"a\":[true]}"]),
        check("array", Json, &["json [1,\"x\"]", "json []"]),
        // JSON strings are text, whatever they look like; dates are the exception
        check("number_text", STRING, &["str 123", "str 0045"]),
        check("code", STRING, &["str 01826", "str 00501"]),
        check("date_text", Date, &["date 2024-01-15", "date 2023-12-31"]),
        check(
            "datetime_text",
            DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 2024-01-15T22:00:00Z"],
        ),
        check("mixed", STRING, &["str 1", "str x"]),
    ];
    let (schema, rows) = crate::detect(path.to_str().unwrap()).await;
    let mut problems = compare(&schema, &rows, &checks);
    // decision: keys after the sample still become columns, rather than being dropped
    let late = rows.last().and_then(|r| r.get("late")).map(crate::render);
    if late.as_deref() != Some("str here") {
        problems.push(format!(
            "late: a key first seen after the sample is {:?}",
            late
        ));
    }
    report("JSON", checks.len() + 1, problems);
}
