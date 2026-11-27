//! PostgreSQL, MySQL and SQL Server: one table per database with a column for each type that
//! matters, holding ordinary values, the type's extremes and NULL. Each test skips when its
//! database can't be reached (see `main.rs`).

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use sqlx::Connection;
use tinyetl::schema::DataType;

use crate::{check, run, Check};

const TABLE: &str = "tinyetl_types";

/// Whether an unreachable database fails the test instead of skipping it
fn required() -> bool {
    std::env::var("TINYETL_TEST_REQUIRE_DATABASES").as_deref() == Ok("1")
}

fn url(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// Report an unreachable database: a skip, or a failure when databases are required
fn unreachable(db: &str, error: impl std::fmt::Display) {
    if required() {
        panic!("{} unreachable: {}", db, error);
    }
    println!("⚠ {} skipped: {}", db, error);
}

/// Run the checks, drop the table whatever happens, then pass on a failure
async fn run_then<F, Fut>(db: &str, location: &str, checks: &[Check], cleanup: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let result = AssertUnwindSafe(run(db, location, checks))
        .catch_unwind()
        .await;
    cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// A check whose values are owned strings (for generated values)
fn check_owned(column: &str, data_type: DataType, values: Vec<String>) -> Check {
    Check {
        column: column.to_string(),
        data_type,
        values,
    }
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn databases_postgres() {
    let url = url(
        "TINYETL_TEST_POSTGRES_URL",
        "postgres://testuser:testpass@localhost:5432/testdb",
    );
    let mut conn =
        match tokio::time::timeout(Duration::from_secs(5), sqlx::PgConnection::connect(&url)).await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return unreachable("PostgreSQL", e),
            Err(_) => return unreachable("PostgreSQL", "connection timed out"),
        };

    sqlx::raw_sql(
        "DROP TABLE IF EXISTS tinyetl_types;
         DROP TYPE IF EXISTS tinyetl_mood;
         CREATE TYPE tinyetl_mood AS ENUM ('happy', 'sad');
         CREATE TABLE tinyetl_types (
             id serial PRIMARY KEY,
             c_smallint smallint,
             c_integer integer,
             c_bigint bigint,
             c_numeric numeric(10,2),
             c_numeric_wide numeric,
             c_numeric_huge numeric,
             c_real real,
             c_double double precision,
             c_money money,
             c_boolean boolean,
             c_date date,
             c_timestamp timestamp,
             c_timestamp6 timestamp(6),
             c_timestamptz timestamptz,
             c_time time,
             c_interval interval,
             c_uuid uuid,
             c_json json,
             c_jsonb jsonb,
             c_bytea bytea,
             c_text text,
             c_varchar varchar(10),
             c_char char(5),
             c_text_array text[],
             c_int_array integer[],
             c_inet inet,
             c_enum tinyetl_mood,
             c_bit bit(3)
         );
         SET TIME ZONE 'UTC';
         INSERT INTO tinyetl_types (c_smallint, c_integer, c_bigint, c_numeric, c_numeric_wide,
             c_numeric_huge, c_real, c_double, c_money, c_boolean, c_date, c_timestamp,
             c_timestamp6, c_timestamptz, c_time, c_interval, c_uuid, c_json, c_jsonb, c_bytea,
             c_text, c_varchar, c_char, c_text_array, c_int_array, c_inet, c_enum, c_bit)
         VALUES
             (1, 42, 7, 12.50, 12345678901234567890.12345678, 123456789012345678901234567890,
              1.5, 0.1, 12.34, true, '2024-01-15', '2024-01-15 10:30:00',
              '2024-01-15 10:30:00.123456', '2024-01-15 10:30:00+02', '13:45:30',
              '1 year 2 months 3 days 04:05:06', '123e4567-e89b-12d3-a456-426614174000',
              '{\"a\": 1, \"b\": [1, 2]}', '{\"a\": 1, \"b\": [1, 2]}', '\\xdeadbeef',
              'hello', 'abc', 'ab', '{a,b}', '{1,2,3}', '192.168.0.1', 'happy', B'101'),
             (-32768, 2147483647, -9223372036854775808, -99999999.99,
              0.0000000000000000000000000001, -1, -3.25, 2.5, -5.00, false, '9999-12-31',
              '1900-01-01 00:00:01', '2000-02-29 23:59:59.999999', '2024-07-01 23:30:00-05',
              '00:00:01', '-1 days', '00000000-0000-0000-0000-000000000000', '[]', '{}',
              '\\x', '01826', '', 'abcde', '{}', '{-1}', '::1', 'sad', B'000'),
             (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL, NULL);",
    )
    .execute(&mut conn)
    .await
    .expect("create the PostgreSQL table");

    let checks = [
        check(
            "c_smallint",
            DataType::Integer,
            &["int 1", "int -32768", "null"],
        ),
        check(
            "c_integer",
            DataType::Integer,
            &["int 42", "int 2147483647", "null"],
        ),
        check(
            "c_bigint",
            DataType::Integer,
            &["int 7", "int -9223372036854775808", "null"],
        ),
        check(
            "c_numeric",
            DataType::Decimal,
            &["dec 12.50", "dec -99999999.99", "null"],
        ),
        check(
            "c_numeric_wide",
            DataType::Decimal,
            &[
                "dec 12345678901234567890.12345678",
                "dec 0.0000000000000000000000000001",
                "null",
            ],
        ),
        // decision: 30 digits don't fit a Decimal (28-29 digits); the value must survive as text
        check(
            "c_numeric_huge",
            DataType::Decimal,
            &["str 123456789012345678901234567890", "dec -1", "null"],
        ),
        check(
            "c_real",
            DataType::Decimal,
            &["dec 1.5", "dec -3.25", "null"],
        ),
        check(
            "c_double",
            DataType::Decimal,
            &["dec 0.1", "dec 2.5", "null"],
        ),
        // decision: money is an exact amount; a Decimal, without the locale's currency symbol
        check(
            "c_money",
            DataType::Decimal,
            &["dec 12.34", "dec -5.00", "null"],
        ),
        check(
            "c_boolean",
            DataType::Boolean,
            &["bool true", "bool false", "null"],
        ),
        check(
            "c_date",
            DataType::Date,
            &["date 2024-01-15", "date 9999-12-31", "null"],
        ),
        check(
            "c_timestamp",
            DataType::DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 1900-01-01T00:00:01Z", "null"],
        ),
        check(
            "c_timestamp6",
            DataType::DateTime,
            &[
                "ts 2024-01-15T10:30:00.123456Z",
                "ts 2000-02-29T23:59:59.999999Z",
                "null",
            ],
        ),
        check(
            "c_timestamptz",
            DataType::DateTime,
            &["ts 2024-01-15T08:30:00Z", "ts 2024-07-02T04:30:00Z", "null"],
        ),
        // decision: TinyETL has no time-of-day type; text in PostgreSQL's own format
        check(
            "c_time",
            DataType::String,
            &["str 13:45:30", "str 00:00:01", "null"],
        ),
        // decision: intervals as PostgreSQL's own text
        check(
            "c_interval",
            DataType::String,
            &["str 1 year 2 mons 3 days 04:05:06", "str -1 days", "null"],
        ),
        // decision: UUIDs as canonical lowercase text
        check(
            "c_uuid",
            DataType::String,
            &[
                "str 123e4567-e89b-12d3-a456-426614174000",
                "str 00000000-0000-0000-0000-000000000000",
                "null",
            ],
        ),
        check(
            "c_json",
            DataType::Json,
            &["json {\"a\":1,\"b\":[1,2]}", "json []", "null"],
        ),
        check(
            "c_jsonb",
            DataType::Json,
            &["json {\"a\":1,\"b\":[1,2]}", "json {}", "null"],
        ),
        // decision: binary as 0x-prefixed lowercase hex, like the SQLite connector
        check(
            "c_bytea",
            DataType::String,
            &["str 0xdeadbeef", "str 0x", "null"],
        ),
        check(
            "c_text",
            DataType::String,
            &["str hello", "str 01826", "null"],
        ),
        check("c_varchar", DataType::String, &["str abc", "str ", "null"]),
        // decision: char(n) comes back padded, as PostgreSQL stores it
        check(
            "c_char",
            DataType::String,
            &["str ab   ", "str abcde", "null"],
        ),
        // decision: arrays are JSON arrays
        check(
            "c_text_array",
            DataType::Json,
            &["json [\"a\",\"b\"]", "json []", "null"],
        ),
        check(
            "c_int_array",
            DataType::Json,
            &["json [1,2,3]", "json [-1]", "null"],
        ),
        check(
            "c_inet",
            DataType::String,
            &["str 192.168.0.1", "str ::1", "null"],
        ),
        check(
            "c_enum",
            DataType::String,
            &["str happy", "str sad", "null"],
        ),
        // decision: bit strings as their digits
        check("c_bit", DataType::String, &["str 101", "str 000", "null"]),
    ];

    let location = format!("{}#{}", url, TABLE);
    run_then("PostgreSQL", &location, &checks, || async move {
        let _ =
            sqlx::raw_sql("DROP TABLE IF EXISTS tinyetl_types; DROP TYPE IF EXISTS tinyetl_mood;")
                .execute(&mut conn)
                .await;
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn databases_mysql() {
    let url = url(
        "TINYETL_TEST_MYSQL_URL",
        "mysql://testuser:testpass@localhost:3306/testdb",
    );
    let mut conn =
        match tokio::time::timeout(Duration::from_secs(5), sqlx::MySqlConnection::connect(&url))
            .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return unreachable("MySQL", e),
            Err(_) => return unreachable("MySQL", "connection timed out"),
        };

    sqlx::raw_sql(
        "DROP TABLE IF EXISTS tinyetl_types;
         CREATE TABLE tinyetl_types (
             id int AUTO_INCREMENT PRIMARY KEY,
             c_tinyint tinyint,
             c_tinyint1 tinyint(1),
             c_bool bool,
             c_smallint smallint,
             c_mediumint mediumint,
             c_int int,
             c_bigint bigint,
             c_ubigint bigint unsigned,
             c_decimal decimal(10,2),
             c_decimal65 decimal(65,30),
             c_float float,
             c_double double,
             c_bit1 bit(1),
             c_year year,
             c_date date,
             c_datetime datetime,
             c_datetime6 datetime(6),
             c_timestamp timestamp NULL,
             c_time time,
             c_char char(5),
             c_varchar varchar(50),
             c_text text,
             c_enum enum('small', 'large'),
             c_set set('a', 'b', 'c'),
             c_json json,
             c_blob blob,
             c_binary binary(16)
         );
         SET time_zone = '+00:00';
         INSERT INTO tinyetl_types (c_tinyint, c_tinyint1, c_bool, c_smallint, c_mediumint,
             c_int, c_bigint, c_ubigint, c_decimal, c_decimal65, c_float, c_double, c_bit1,
             c_year, c_date, c_datetime, c_datetime6, c_timestamp, c_time, c_char, c_varchar,
             c_text, c_enum, c_set, c_json, c_blob, c_binary)
         VALUES
             (-128, 1, true, 1, -8388608, 42, 7, 18446744073709551615, 12.50,
              1.123456789012345678901234567, 1.5, 0.1, b'1', 2024, '2024-01-15',
              '2024-01-15 10:30:00', '2024-01-15 10:30:00.123456', '2024-01-15 10:30:00',
              '13:45:30', 'ab', 'hello', '01826', 'small', 'a,b', '{\"a\": 1, \"b\": [1, 2]}',
              x'DEADBEEF', x'00112233445566778899AABBCCDDEEFF'),
             (127, 0, false, -32768, 8388607, 2147483647, -9223372036854775808, 0, -99999999.99,
              0, -3.25, 2.5, b'0', 1901, '1000-01-01', '9999-12-31 23:59:59',
              '2000-02-29 23:59:59.999999', '2038-01-19 03:14:07', '-838:59:59', 'abcde', '',
              'x', 'large', '', '[]', x'', x'00000000000000000000000000000000'),
             (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL);",
    )
    .execute(&mut conn)
    .await
    .expect("create the MySQL table");

    let checks = [
        check(
            "c_tinyint",
            DataType::Integer,
            &["int -128", "int 127", "null"],
        ),
        // decision: tinyint(1) and bool are MySQL's booleans
        check(
            "c_tinyint1",
            DataType::Boolean,
            &["bool true", "bool false", "null"],
        ),
        check(
            "c_bool",
            DataType::Boolean,
            &["bool true", "bool false", "null"],
        ),
        check(
            "c_smallint",
            DataType::Integer,
            &["int 1", "int -32768", "null"],
        ),
        check(
            "c_mediumint",
            DataType::Integer,
            &["int -8388608", "int 8388607", "null"],
        ),
        check(
            "c_int",
            DataType::Integer,
            &["int 42", "int 2147483647", "null"],
        ),
        check(
            "c_bigint",
            DataType::Integer,
            &["int 7", "int -9223372036854775808", "null"],
        ),
        // decision: bigint unsigned can exceed a 64-bit signed integer; Decimal keeps it exact
        check(
            "c_ubigint",
            DataType::Decimal,
            &["dec 18446744073709551615", "dec 0", "null"],
        ),
        check(
            "c_decimal",
            DataType::Decimal,
            &["dec 12.50", "dec -99999999.99", "null"],
        ),
        // decision: scale 30 is beyond a Decimal's 28; the value is kept exactly at scale 28
        check(
            "c_decimal65",
            DataType::Decimal,
            &[
                "dec 1.1234567890123456789012345670",
                "dec 0.0000000000000000000000000000",
                "null",
            ],
        ),
        check(
            "c_float",
            DataType::Decimal,
            &["dec 1.5", "dec -3.25", "null"],
        ),
        check(
            "c_double",
            DataType::Decimal,
            &["dec 0.1", "dec 2.5", "null"],
        ),
        // decision: bit(1) is a flag
        check(
            "c_bit1",
            DataType::Boolean,
            &["bool true", "bool false", "null"],
        ),
        // decision: a year is a whole number
        check(
            "c_year",
            DataType::Integer,
            &["int 2024", "int 1901", "null"],
        ),
        check(
            "c_date",
            DataType::Date,
            &["date 2024-01-15", "date 1000-01-01", "null"],
        ),
        check(
            "c_datetime",
            DataType::DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 9999-12-31T23:59:59Z", "null"],
        ),
        check(
            "c_datetime6",
            DataType::DateTime,
            &[
                "ts 2024-01-15T10:30:00.123456Z",
                "ts 2000-02-29T23:59:59.999999Z",
                "null",
            ],
        ),
        check(
            "c_timestamp",
            DataType::DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 2038-01-19T03:14:07Z", "null"],
        ),
        // decision: time (which can be negative or over 24 hours in MySQL) as MySQL's text
        check(
            "c_time",
            DataType::String,
            &["str 13:45:30", "str -838:59:59", "null"],
        ),
        // MySQL drops a CHAR column's trailing spaces when reading
        check("c_char", DataType::String, &["str ab", "str abcde", "null"]),
        check(
            "c_varchar",
            DataType::String,
            &["str hello", "str ", "null"],
        ),
        check("c_text", DataType::String, &["str 01826", "str x", "null"]),
        check(
            "c_enum",
            DataType::String,
            &["str small", "str large", "null"],
        ),
        check("c_set", DataType::String, &["str a,b", "str ", "null"]),
        check(
            "c_json",
            DataType::Json,
            &["json {\"a\":1,\"b\":[1,2]}", "json []", "null"],
        ),
        // decision: binary as 0x-prefixed lowercase hex, like the SQLite connector
        check(
            "c_blob",
            DataType::String,
            &["str 0xdeadbeef", "str 0x", "null"],
        ),
        check(
            "c_binary",
            DataType::String,
            &[
                "str 0x00112233445566778899aabbccddeeff",
                "str 0x00000000000000000000000000000000",
                "null",
            ],
        ),
    ];

    let location = format!("{}#{}", url, TABLE);
    run_then("MySQL", &location, &checks, || async move {
        let _ = sqlx::raw_sql("DROP TABLE IF EXISTS tinyetl_types")
            .execute(&mut conn)
            .await;
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// SQL Server
// ---------------------------------------------------------------------------------------------

type MssqlClient = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;

async fn mssql_connect(location: &str) -> Result<MssqlClient, String> {
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    let url = url::Url::parse(location).map_err(|e| e.to_string())?;
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let mut config = tiberius::Config::new();
    config.host(url.host_str().unwrap_or("localhost"));
    config.port(url.port().unwrap_or(1433));
    let database = url.path().trim_start_matches('/');
    if !database.is_empty() {
        config.database(database);
    }
    config.authentication(tiberius::AuthMethod::sql_server(
        decode(url.username()),
        decode(url.password().unwrap_or("")),
    ));
    config.encryption(tiberius::EncryptionLevel::Required);
    config.trust_cert();
    let tcp = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(config.get_addr()),
    )
    .await
    .map_err(|_| "connection timed out".to_string())?
    .map_err(|e| e.to_string())?;
    tcp.set_nodelay(true).ok();
    tokio::time::timeout(
        Duration::from_secs(10),
        tiberius::Client::connect(config, tcp.compat_write()),
    )
    .await
    .map_err(|_| "login timed out".to_string())?
    .map_err(|e| e.to_string())
}

#[tokio::test]
async fn databases_mssql() {
    let url = url(
        "TINYETL_TEST_MSSQL_URL",
        "mssql://sa:TestPass123%21@localhost:1433/master",
    );
    let mut client = match mssql_connect(&url).await {
        Ok(client) => client,
        Err(e) => return unreachable("SQL Server", e),
    };

    let long_text = "x".repeat(5000);
    client
        .simple_query(format!(
            "IF OBJECT_ID('tinyetl_types') IS NOT NULL DROP TABLE tinyetl_types;
             CREATE TABLE tinyetl_types (
                 id int IDENTITY PRIMARY KEY,
                 c_bit bit,
                 c_tinyint tinyint,
                 c_smallint smallint,
                 c_int int,
                 c_bigint bigint,
                 c_decimal decimal(10,2),
                 c_decimal38 decimal(38,10),
                 c_money money,
                 c_smallmoney smallmoney,
                 c_real real,
                 c_float float,
                 c_date date,
                 c_datetime datetime,
                 c_datetime2 datetime2(7),
                 c_smalldatetime smalldatetime,
                 c_datetimeoffset datetimeoffset,
                 c_time time,
                 c_uniqueidentifier uniqueidentifier,
                 c_char char(5),
                 c_varchar varchar(50),
                 c_nvarchar nvarchar(50),
                 c_nvarchar_max nvarchar(max),
                 c_varbinary varbinary(16),
                 c_xml xml
             );
             INSERT INTO tinyetl_types (c_bit, c_tinyint, c_smallint, c_int, c_bigint, c_decimal,
                 c_decimal38, c_money, c_smallmoney, c_real, c_float, c_date, c_datetime,
                 c_datetime2, c_smalldatetime, c_datetimeoffset, c_time, c_uniqueidentifier,
                 c_char, c_varchar, c_nvarchar, c_nvarchar_max, c_varbinary, c_xml)
             VALUES
                 (1, 255, 1, 42, 7, 12.50, 1234567890123456.1234567890, 12.34, 5.25, 1.5, 0.1,
                  '2024-01-15', '2024-01-15 10:30:00.500', '2024-01-15 10:30:00.1234567',
                  '2024-01-15 10:30', '2024-01-15 10:30:00 +02:00', '13:45:30',
                  '123e4567-e89b-12d3-a456-426614174000', 'ab', 'hello', N'café 日本', N'{long}',
                  0xDEADBEEF, '<a>1</a>'),
                 (0, 0, -32768, 2147483647, -9223372036854775808, -99999999.99,
                  -0.0000000001, -922337203685477.5808, -214748.3648, -3.25, 2.5,
                  '0001-01-01', '1753-01-01 00:00:01', '9999-12-31 23:59:59.9999999',
                  '1900-01-01 00:01', '2024-07-01 23:30:00 -05:00', '00:00:01',
                  '00000000-0000-0000-0000-000000000000', 'abcde', '', N'01826', N'', 0x,
                  '<b/>'),
                 (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                  NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);",
            long = long_text
        ))
        .await
        .expect("create the SQL Server table")
        .into_results()
        .await
        .expect("create the SQL Server table");

    let checks = [
        check(
            "c_bit",
            DataType::Boolean,
            &["bool true", "bool false", "null"],
        ),
        check(
            "c_tinyint",
            DataType::Integer,
            &["int 255", "int 0", "null"],
        ),
        check(
            "c_smallint",
            DataType::Integer,
            &["int 1", "int -32768", "null"],
        ),
        check(
            "c_int",
            DataType::Integer,
            &["int 42", "int 2147483647", "null"],
        ),
        check(
            "c_bigint",
            DataType::Integer,
            &["int 7", "int -9223372036854775808", "null"],
        ),
        check(
            "c_decimal",
            DataType::Decimal,
            &["dec 12.50", "dec -99999999.99", "null"],
        ),
        check(
            "c_decimal38",
            DataType::Decimal,
            &[
                "dec 1234567890123456.1234567890",
                "dec -0.0000000001",
                "null",
            ],
        ),
        // decision: money is an exact Decimal (trailing zeros don't matter, digits do)
        check(
            "c_money",
            DataType::Decimal,
            &["dec 12.34", "dec -922337203685477.5808", "null"],
        ),
        check(
            "c_smallmoney",
            DataType::Decimal,
            &["dec 5.25", "dec -214748.3648", "null"],
        ),
        check(
            "c_real",
            DataType::Decimal,
            &["dec 1.5", "dec -3.25", "null"],
        ),
        check(
            "c_float",
            DataType::Decimal,
            &["dec 0.1", "dec 2.5", "null"],
        ),
        check(
            "c_date",
            DataType::Date,
            &["date 2024-01-15", "date 0001-01-01", "null"],
        ),
        check(
            "c_datetime",
            DataType::DateTime,
            &[
                "ts 2024-01-15T10:30:00.500Z",
                "ts 1753-01-01T00:00:01Z",
                "null",
            ],
        ),
        check(
            "c_datetime2",
            DataType::DateTime,
            &[
                "ts 2024-01-15T10:30:00.123456700Z",
                "ts 9999-12-31T23:59:59.999999900Z",
                "null",
            ],
        ),
        check(
            "c_smalldatetime",
            DataType::DateTime,
            &["ts 2024-01-15T10:30:00Z", "ts 1900-01-01T00:01:00Z", "null"],
        ),
        check(
            "c_datetimeoffset",
            DataType::DateTime,
            &["ts 2024-01-15T08:30:00Z", "ts 2024-07-02T04:30:00Z", "null"],
        ),
        // decision: TinyETL has no time-of-day type; text as hh:mm:ss
        check(
            "c_time",
            DataType::String,
            &["str 13:45:30", "str 00:00:01", "null"],
        ),
        // decision: UUIDs as canonical lowercase text
        check(
            "c_uniqueidentifier",
            DataType::String,
            &[
                "str 123e4567-e89b-12d3-a456-426614174000",
                "str 00000000-0000-0000-0000-000000000000",
                "null",
            ],
        ),
        // decision: char(n) comes back padded, as SQL Server stores it
        check(
            "c_char",
            DataType::String,
            &["str ab   ", "str abcde", "null"],
        ),
        check(
            "c_varchar",
            DataType::String,
            &["str hello", "str ", "null"],
        ),
        check(
            "c_nvarchar",
            DataType::String,
            &["str café 日本", "str 01826", "null"],
        ),
        check_owned(
            "c_nvarchar_max",
            DataType::String,
            vec![
                format!("str {}", long_text),
                "str ".to_string(),
                "null".to_string(),
            ],
        ),
        // decision: binary as 0x-prefixed lowercase hex, like the SQLite connector
        check(
            "c_varbinary",
            DataType::String,
            &["str 0xdeadbeef", "str 0x", "null"],
        ),
        check(
            "c_xml",
            DataType::String,
            &["str <a>1</a>", "str <b/>", "null"],
        ),
    ];

    let location = format!("{}#{}", url, TABLE);
    run_then("SQL Server", &location, &checks, || async move {
        if let Ok(stream) = client
            .simple_query("IF OBJECT_ID('tinyetl_types') IS NOT NULL DROP TABLE tinyetl_types")
            .await
        {
            let _ = stream.into_results().await;
        }
    })
    .await;
}
