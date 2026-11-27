//! Typed file formats and embedded databases: Excel, Parquet, Avro, SQLite, DuckDB. Each
//! fixture is written with the format's own library, not with TinyETL.

use crate::{check, run};
use std::sync::Arc;
use tinyetl::schema::DataType::{self, Boolean, Date, DateTime, Decimal, Integer, Json};

const STRING: DataType = DataType::String;

#[tokio::test]
async fn excel() {
    use rust_xlsxwriter::{Format, Formula, Workbook};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.xlsx");
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let headers = [
        "int",
        "float",
        "big_int",
        "code_text",
        "number_text",
        "bool",
        "date",
        "datetime",
        "percent",
        "currency",
        "formula",
        "empty",
        "mixed",
    ];
    for (c, h) in headers.iter().enumerate() {
        sheet.write_string(0, c as u16, *h).unwrap();
    }
    let date_format = Format::new().set_num_format("yyyy-mm-dd");
    let datetime_format = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
    let percent = Format::new().set_num_format("0%");
    let currency = Format::new().set_num_format("$#,##0.00");
    let date = |d: u32| chrono::NaiveDate::from_ymd_opt(2024, 1, d).unwrap();
    for (r, (int, float, text, flag, day)) in [
        (42.0, 2.75, "01826", true, 15),
        (-7.0, 0.1, "00501", false, 16),
    ]
    .into_iter()
    .enumerate()
    {
        let r = r as u32 + 1;
        sheet.write_number(r, 0, int).unwrap();
        sheet.write_number(r, 1, float).unwrap();
        sheet.write_number(r, 2, 12_345_678_901_234.0).unwrap();
        sheet.write_string(r, 3, text).unwrap();
        sheet.write_string(r, 4, "42").unwrap();
        sheet.write_boolean(r, 5, flag).unwrap();
        sheet
            .write_date_with_format(r, 6, date(day), &date_format)
            .unwrap();
        sheet
            .write_datetime_with_format(
                r,
                7,
                date(day).and_hms_opt(10, 30, 0).unwrap(),
                &datetime_format,
            )
            .unwrap();
        sheet
            .write_number_with_format(r, 8, 0.45, &percent)
            .unwrap();
        sheet
            .write_number_with_format(r, 9, 1234.5, &currency)
            .unwrap();
        sheet
            .write_formula(
                r,
                10,
                Formula::new(format!("=A{}*2", r + 1)).set_result((int * 2.0).to_string()),
            )
            .unwrap();
    }
    sheet.write_number(1, 12, 1.0).unwrap();
    sheet.write_string(2, 12, "abc").unwrap();
    workbook.save(&path).unwrap();

    let checks = [
        check("int", Integer, &["int 42", "int -7"]),
        check("float", Decimal, &["dec 2.75", "dec 0.1"]),
        check(
            "big_int",
            Integer,
            &["int 12345678901234", "int 12345678901234"],
        ),
        check("code_text", STRING, &["str 01826", "str 00501"]),
        // Text cells are text, even when they look like numbers: Excel typed them
        check("number_text", STRING, &["str 42", "str 42"]),
        check("bool", Boolean, &["bool true", "bool false"]),
        check("date", Date, &["date 2024-01-15", "date 2024-01-16"]),
        check(
            "datetime",
            DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 2024-01-16T10:30:00Z"],
        ),
        // Percent and currency are display formats; the cell holds the number
        check("percent", Decimal, &["dec 0.45", "dec 0.45"]),
        check("currency", Decimal, &["dec 1234.5", "dec 1234.5"]),
        check("formula", Integer, &["int 84", "int -14"]),
        check("empty", STRING, &["null", "null"]),
        check("mixed", STRING, &["str 1", "str abc"]),
    ];
    run("Excel", path.to_str().unwrap(), &checks).await;
}

#[tokio::test]
async fn parquet() {
    use arrow::array::*;
    use arrow::datatypes::{DataType as A, Field, Int32Type, Int64Type, Schema};
    use arrow::record_batch::RecordBatch;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.parquet");
    // 2024-01-15 is day 19737; 10:30:00.123456 that day in microseconds
    let day = 19_737i32;
    let micros = day as i64 * 86_400_000_000 + 37_800_123_456;
    let struct_array = StructArray::from(vec![
        (
            Arc::new(Field::new("a", A::Int64, true)),
            Arc::new(Int64Array::from(vec![Some(1), None])) as ArrayRef,
        ),
        (
            Arc::new(Field::new("b", A::Utf8, true)),
            Arc::new(StringArray::from(vec![Some("x"), Some("y")])) as ArrayRef,
        ),
    ]);
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("int8", Arc::new(Int8Array::from(vec![Some(-128), None]))),
        (
            "int16",
            Arc::new(Int16Array::from(vec![Some(32767), Some(0)])),
        ),
        (
            "int32",
            Arc::new(Int32Array::from(vec![Some(-2147483648), Some(1)])),
        ),
        (
            "int64",
            Arc::new(Int64Array::from(vec![Some(i64::MAX), None])),
        ),
        (
            "uint8",
            Arc::new(UInt8Array::from(vec![Some(255), Some(0)])),
        ),
        (
            "uint64",
            Arc::new(UInt64Array::from(vec![Some(u64::MAX), Some(1)])),
        ),
        (
            "float32",
            Arc::new(Float32Array::from(vec![Some(1.5), Some(-0.25)])),
        ),
        (
            "float64",
            Arc::new(Float64Array::from(vec![Some(0.1), Some(1e20)])),
        ),
        (
            "decimal_10_2",
            Arc::new(
                Decimal128Array::from(vec![Some(12345), Some(-1)])
                    .with_precision_and_scale(10, 2)
                    .unwrap(),
            ),
        ),
        (
            "decimal_38_10",
            Arc::new(
                Decimal128Array::from(vec![Some(1_234_567_890_123_456_789i128), Some(0)])
                    .with_precision_and_scale(38, 10)
                    .unwrap(),
            ),
        ),
        (
            "bool",
            Arc::new(BooleanArray::from(vec![Some(true), Some(false)])),
        ),
        ("date32", Arc::new(Date32Array::from(vec![Some(day), None]))),
        (
            "timestamp",
            Arc::new(TimestampMicrosecondArray::from(vec![Some(micros), None])),
        ),
        (
            "timestamp_zone",
            Arc::new(
                TimestampMillisecondArray::from(vec![Some(micros / 1000), None])
                    .with_timezone("+02:00"),
            ),
        ),
        (
            "time",
            Arc::new(Time64MicrosecondArray::from(vec![
                Some(37_800_123_456),
                None,
            ])),
        ),
        (
            "text",
            Arc::new(StringArray::from(vec![Some("01826"), Some("")])),
        ),
        (
            "large_text",
            Arc::new(LargeStringArray::from(vec![Some("café"), None])),
        ),
        (
            "binary",
            Arc::new(BinaryArray::from_opt_vec(vec![
                Some(&[0xde, 0xad, 0xbe, 0xef]),
                None,
            ])),
        ),
        (
            "dictionary",
            Arc::new(
                vec!["red", "blue"]
                    .into_iter()
                    .collect::<DictionaryArray<Int32Type>>(),
            ),
        ),
        (
            "list",
            Arc::new(ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
                Some(vec![Some(1), Some(2)]),
                Some(vec![]),
            ])),
        ),
        ("struct", Arc::new(struct_array)),
    ];
    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
            .collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        columns.into_iter().map(|(_, a)| a).collect(),
    )
    .unwrap();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema, None)
            .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let checks = [
        check("int8", Integer, &["int -128", "null"]),
        check("int16", Integer, &["int 32767", "int 0"]),
        check("int32", Integer, &["int -2147483648", "int 1"]),
        check("int64", Integer, &["int 9223372036854775807", "null"]),
        check("uint8", Integer, &["int 255", "int 0"]),
        // Past i64: an exact decimal
        check("uint64", Decimal, &["dec 18446744073709551615", "dec 1"]),
        check("float32", Decimal, &["dec 1.5", "dec -0.25"]),
        check(
            "float64",
            Decimal,
            &["dec 0.1", "dec 100000000000000000000"],
        ),
        check("decimal_10_2", Decimal, &["dec 123.45", "dec -0.01"]),
        check(
            "decimal_38_10",
            Decimal,
            &["dec 123456789.0123456789", "dec 0.0000000000"],
        ),
        check("bool", Boolean, &["bool true", "bool false"]),
        check("date32", Date, &["date 2024-01-15", "null"]),
        check(
            "timestamp",
            DateTime,
            &["ts 2024-01-15T10:30:00.123456Z", "null"],
        ),
        // The instant, in UTC
        check(
            "timestamp_zone",
            DateTime,
            &["ts 2024-01-15T10:30:00.123Z", "null"],
        ),
        // decision: no Time type, so times are text
        check("time", STRING, &["str 10:30:00.123456", "null"]),
        check("text", STRING, &["str 01826", "str "]),
        check("large_text", STRING, &["str café", "null"]),
        // decision: binary values are hex text, 0x-prefixed
        check("binary", STRING, &["str 0xdeadbeef", "null"]),
        check("dictionary", STRING, &["str red", "str blue"]),
        check("list", Json, &["json [1,2]", "json []"]),
        check(
            "struct",
            Json,
            &[
                "json {\"a\":1,\"b\":\"x\"}",
                "json {\"a\":null,\"b\":\"y\"}",
            ],
        ),
    ];
    run("Parquet", path.to_str().unwrap(), &checks).await;
}

#[tokio::test]
async fn avro() {
    use apache_avro::types::{Record, Value as V};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.avro");
    let schema = apache_avro::Schema::parse_str(
        r#"{"type": "record", "name": "cases", "fields": [
            {"name": "int", "type": "int"},
            {"name": "long", "type": "long"},
            {"name": "float", "type": "float"},
            {"name": "double", "type": "double"},
            {"name": "bool", "type": "boolean"},
            {"name": "text", "type": "string"},
            {"name": "bytes", "type": "bytes"},
            {"name": "decimal", "type": {"type": "bytes", "logicalType": "decimal", "precision": 10, "scale": 2}},
            {"name": "date", "type": {"type": "int", "logicalType": "date"}},
            {"name": "timestamp_millis", "type": {"type": "long", "logicalType": "timestamp-millis"}},
            {"name": "timestamp_micros", "type": {"type": "long", "logicalType": "timestamp-micros"}},
            {"name": "time_millis", "type": {"type": "int", "logicalType": "time-millis"}},
            {"name": "uuid", "type": {"type": "string", "logicalType": "uuid"}},
            {"name": "enum", "type": {"type": "enum", "name": "color", "symbols": ["red", "blue"]}},
            {"name": "array", "type": {"type": "array", "items": "long"}},
            {"name": "map", "type": {"type": "map", "values": "string"}},
            {"name": "record", "type": {"type": "record", "name": "inner", "fields": [{"name": "a", "type": "long"}]}},
            {"name": "optional", "type": ["null", "long"]}
        ]}"#,
    )
    .unwrap();
    let day = 19_737i32;
    let millis = day as i64 * 86_400_000 + 37_800_123;
    let mut writer = apache_avro::Writer::new(&schema, std::fs::File::create(&path).unwrap());
    for (i, (int, text, color)) in [(1, "01826", 0u32), (-2, "", 1)].into_iter().enumerate() {
        let mut record = Record::new(&schema).unwrap();
        record.put("int", V::Int(int));
        record.put("long", V::Long(if i == 0 { i64::MAX } else { 0 }));
        record.put("float", V::Float(1.5));
        record.put("double", V::Double(0.1));
        record.put("bool", V::Boolean(i == 0));
        record.put("text", V::String(text.to_string()));
        record.put("bytes", V::Bytes(vec![0xde, 0xad]));
        record.put(
            "decimal",
            V::Decimal(apache_avro::Decimal::from(vec![0x30, 0x39])),
        ); // 12345
        record.put("date", V::Date(day));
        record.put("timestamp_millis", V::TimestampMillis(millis));
        record.put("timestamp_micros", V::TimestampMicros(millis * 1000 + 456));
        record.put("time_millis", V::TimeMillis(37_800_123));
        record.put(
            "uuid",
            V::Uuid("123e4567-e89b-12d3-a456-426614174000".parse().unwrap()),
        );
        record.put(
            "enum",
            V::Enum(color, ["red", "blue"][color as usize].to_string()),
        );
        record.put("array", V::Array(vec![V::Long(1), V::Long(2)]));
        record.put(
            "map",
            V::Map(
                [("k".to_string(), V::String("v".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        );
        record.put("record", V::Record(vec![("a".to_string(), V::Long(5))]));
        record.put(
            "optional",
            if i == 0 {
                V::Union(1, Box::new(V::Long(7)))
            } else {
                V::Union(0, Box::new(V::Null))
            },
        );
        writer.append(record).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);

    let checks = [
        check("int", Integer, &["int 1", "int -2"]),
        check("long", Integer, &["int 9223372036854775807", "int 0"]),
        check("float", Decimal, &["dec 1.5", "dec 1.5"]),
        check("double", Decimal, &["dec 0.1", "dec 0.1"]),
        check("bool", Boolean, &["bool true", "bool false"]),
        check("text", STRING, &["str 01826", "str "]),
        // decision: binary values are hex text, 0x-prefixed
        check("bytes", STRING, &["str 0xdead", "str 0xdead"]),
        check("decimal", Decimal, &["dec 123.45", "dec 123.45"]),
        check("date", Date, &["date 2024-01-15", "date 2024-01-15"]),
        check(
            "timestamp_millis",
            DateTime,
            &["ts 2024-01-15T10:30:00.123Z", "ts 2024-01-15T10:30:00.123Z"],
        ),
        check(
            "timestamp_micros",
            DateTime,
            &[
                "ts 2024-01-15T10:30:00.123456Z",
                "ts 2024-01-15T10:30:00.123456Z",
            ],
        ),
        check(
            "time_millis",
            STRING,
            &["str 10:30:00.123", "str 10:30:00.123"],
        ),
        check(
            "uuid",
            STRING,
            &[
                "str 123e4567-e89b-12d3-a456-426614174000",
                "str 123e4567-e89b-12d3-a456-426614174000",
            ],
        ),
        check("enum", STRING, &["str red", "str blue"]),
        check("array", Json, &["json [1,2]", "json [1,2]"]),
        check("map", Json, &["json {\"k\":\"v\"}", "json {\"k\":\"v\"}"]),
        check("record", Json, &["json {\"a\":5}", "json {\"a\":5}"]),
        check("optional", Integer, &["int 7", "null"]),
    ];
    run("Avro", path.to_str().unwrap(), &checks).await;
}

/// Declared types, SQLite's loose storage, and columns with no declared type
#[tokio::test]
async fn sqlite() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.db");
    let mut conn =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE cases (
            integer_ INTEGER, int_ INT, bigint_ BIGINT, tinyint_ TINYINT,
            real_ REAL, double_ DOUBLE, float_ FLOAT, numeric_ NUMERIC, decimal_ DECIMAL(10,2),
            boolean_ BOOLEAN, date_ DATE, datetime_ DATETIME, timestamp_ TIMESTAMP,
            text_ TEXT, varchar_ VARCHAR(255), char_ CHAR(3), clob_ CLOB, blob_ BLOB, json_ JSON,
            untyped_ints, untyped_mixed, integer_holding_text INTEGER
        );
        INSERT INTO cases VALUES (
            1, 2, 9223372036854775807, 127,
            3.14, 0.1, 1.5, 10.5, 123.45,
            1, '2024-01-15', '2024-01-15 10:30:00', '2024-01-15T10:30:00Z',
            '01826', 'abc', 'xyz', 'long text', x'DEADBEEF', '{\"a\": 1}',
            1, 1, 5
        );
        INSERT INTO cases VALUES (
            NULL, -2, -1, 0,
            -0.5, 2.5, NULL, 7, -0.01,
            0, '2023-12-31', '2023-12-31 23:59:59', NULL,
            '00501', NULL, NULL, NULL, NULL, '[1, 2]',
            2, 'abc', 'abc'
        );",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();

    let checks = [
        check("integer_", Integer, &["int 1", "null"]),
        check("int_", Integer, &["int 2", "int -2"]),
        check("bigint_", Integer, &["int 9223372036854775807", "int -1"]),
        check("tinyint_", Integer, &["int 127", "int 0"]),
        check("real_", Decimal, &["dec 3.14", "dec -0.5"]),
        check("double_", Decimal, &["dec 0.1", "dec 2.5"]),
        check("float_", Decimal, &["dec 1.5", "null"]),
        check("numeric_", Decimal, &["dec 10.5", "dec 7"]),
        check("decimal_", Decimal, &["dec 123.45", "dec -0.01"]),
        check("boolean_", Boolean, &["bool true", "bool false"]),
        check("date_", Date, &["date 2024-01-15", "date 2023-12-31"]),
        check(
            "datetime_",
            DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 2023-12-31T23:59:59Z"],
        ),
        check("timestamp_", DateTime, &["ts 2024-01-15T10:30:00Z", "null"]),
        check("text_", STRING, &["str 01826", "str 00501"]),
        check("varchar_", STRING, &["str abc", "null"]),
        check("char_", STRING, &["str xyz", "null"]),
        check("clob_", STRING, &["str long text", "null"]),
        // decision: binary values are hex text, 0x-prefixed
        check("blob_", STRING, &["str 0xdeadbeef", "null"]),
        check("json_", Json, &["json {\"a\":1}", "json [1,2]"]),
        // No declared type: the values decide
        check("untyped_ints", Integer, &["int 1", "int 2"]),
        check("untyped_mixed", STRING, &["str 1", "str abc"]),
        // SQLite stores text in an INTEGER column as text; it's kept, not lost
        check("integer_holding_text", Integer, &["int 5", "str abc"]),
    ];
    run("SQLite", &format!("{}#cases", path.display()), &checks).await;
}

#[tokio::test]
async fn duckdb() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.duckdb");
    let conn = duckdb::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TYPE mood AS ENUM ('happy', 'sad');
        CREATE TABLE cases (
            tinyint_ TINYINT, smallint_ SMALLINT, integer_ INTEGER, bigint_ BIGINT,
            hugeint_ HUGEINT, utinyint_ UTINYINT, ubigint_ UBIGINT,
            decimal_4_2 DECIMAL(4,2), decimal_38_10 DECIMAL(38,10), real_ REAL, double_ DOUBLE,
            boolean_ BOOLEAN, date_ DATE, time_ TIME, timestamp_ TIMESTAMP,
            timestamptz_ TIMESTAMPTZ, interval_ INTERVAL, uuid_ UUID, varchar_ VARCHAR,
            blob_ BLOB, json_ JSON, enum_ mood, list_ INTEGER[], struct_ STRUCT(a INTEGER, b VARCHAR),
            map_ MAP(VARCHAR, INTEGER)
        );
        INSERT INTO cases VALUES (
            -128, 32767, 1, 9223372036854775807,
            12345678901234567890, 255, 18446744073709551615,
            12.34, 123.4567890123, 1.5, 0.1,
            true, '2024-01-15', '10:30:00', '2024-01-15 10:30:00.123456',
            '2024-01-15 10:30:00+02', INTERVAL 1 DAY + INTERVAL 2 HOUR,
            '123e4567-e89b-12d3-a456-426614174000', '01826',
            '\\xDE\\xAD\\xBE\\xEF'::BLOB, '{\"a\": 1}', 'happy', [1, 2], {'a': 1, 'b': 'x'},
            MAP {'k': 1}
        );
        INSERT INTO cases (tinyint_, varchar_) VALUES (0, '');",
    )
    .unwrap();
    drop(conn);

    let checks = [
        check("tinyint_", Integer, &["int -128", "int 0"]),
        check("smallint_", Integer, &["int 32767", "null"]),
        check("integer_", Integer, &["int 1", "null"]),
        check("bigint_", Integer, &["int 9223372036854775807", "null"]),
        // Past i64: an exact decimal
        check("hugeint_", Decimal, &["dec 12345678901234567890", "null"]),
        check("utinyint_", Integer, &["int 255", "null"]),
        check("ubigint_", Decimal, &["dec 18446744073709551615", "null"]),
        check("decimal_4_2", Decimal, &["dec 12.34", "null"]),
        check("decimal_38_10", Decimal, &["dec 123.4567890123", "null"]),
        check("real_", Decimal, &["dec 1.5", "null"]),
        check("double_", Decimal, &["dec 0.1", "null"]),
        check("boolean_", Boolean, &["bool true", "null"]),
        check("date_", Date, &["date 2024-01-15", "null"]),
        // decision: no Time or Interval type, so they're text
        check("time_", STRING, &["str 10:30:00", "null"]),
        check(
            "timestamp_",
            DateTime,
            &["ts 2024-01-15T10:30:00.123456Z", "null"],
        ),
        check(
            "timestamptz_",
            DateTime,
            &["ts 2024-01-15T08:30:00Z", "null"],
        ),
        check("interval_", STRING, &["str 1 day 02:00:00", "null"]),
        check(
            "uuid_",
            STRING,
            &["str 123e4567-e89b-12d3-a456-426614174000", "null"],
        ),
        check("varchar_", STRING, &["str 01826", "str "]),
        // decision: binary values are hex text, 0x-prefixed
        check("blob_", STRING, &["str 0xdeadbeef", "null"]),
        check("json_", Json, &["json {\"a\":1}", "null"]),
        check("enum_", STRING, &["str happy", "null"]),
        check("list_", Json, &["json [1,2]", "null"]),
        check("struct_", Json, &["json {\"a\":1,\"b\":\"x\"}", "null"]),
        check("map_", Json, &["json {\"k\":1}", "null"]),
    ];
    run("DuckDB", &format!("{}#cases", path.display()), &checks).await;
}
