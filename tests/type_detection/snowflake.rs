//! Snowflake: a table with a column for each Snowflake type, built with the `firn` client and
//! read back with TinyETL's Snowflake connector. Runs when `SNOWFLAKE_ACCOUNT`, `_USER`,
//! `_PRIVATE_KEY_PATH`, `_WAREHOUSE`, `_DATABASE` and `_SCHEMA` (optionally `_ROLE`) are in
//! the environment or the repository's `.env`; otherwise it's skipped.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use tinyetl::schema::DataType::{self, Boolean, Date, DateTime, Decimal, Integer, Json};

use crate::{check, compare, report, try_detect_with};

const STRING: DataType = DataType::String;
const TABLE: &str = "TINYETL_TYPE_DETECTION";

/// The Snowflake settings, from the environment or `.env`
fn settings() -> Option<HashMap<String, String>> {
    let mut values: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("SNOWFLAKE_"))
        .collect();
    let env_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if let Ok(text) = std::fs::read_to_string(env_file) {
        for line in text.lines() {
            let line = line.trim().trim_start_matches("export ");
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if key.starts_with("SNOWFLAKE_") && !value.is_empty() {
                    values
                        .entry(key.trim().to_string())
                        .or_insert(value.to_string());
                }
            }
        }
    }
    let required = [
        "ACCOUNT",
        "USER",
        "PRIVATE_KEY_PATH",
        "WAREHOUSE",
        "DATABASE",
        "SCHEMA",
    ];
    let missing: Vec<_> = required
        .iter()
        .filter(|k| !values.contains_key(&format!("SNOWFLAKE_{}", k)))
        .collect();
    if !missing.is_empty() {
        println!("⚠ Snowflake skipped: SNOWFLAKE_{:?} not set", missing);
        return None;
    }
    Some(values)
}

#[tokio::test]
async fn snowflake() {
    let Some(env) = settings() else {
        return;
    };
    let get = |k: &str| env.get(&format!("SNOWFLAKE_{}", k)).cloned();
    let config = firn::ConnectionConfig {
        account: get("ACCOUNT"),
        user: get("USER"),
        warehouse: get("WAREHOUSE"),
        database: get("DATABASE"),
        schema: get("SCHEMA"),
        role: get("ROLE"),
        authenticator: Some("snowflake_jwt".to_string()),
        private_key_file: get("PRIVATE_KEY_PATH").map(Into::into),
        private_key_file_pwd: get("PRIVATE_KEY_PASSPHRASE").map(firn::SecretString::from),
        application: Some("TinyETL tests".to_string()),
        ..firn::ConnectionConfig::default()
    };
    let api = firn::SnowflakeApiBuilder::from_connection_config(config)
        .and_then(firn::SnowflakeApiBuilder::build)
        .expect("Snowflake configuration");
    let exec = |sql: String| {
        let api = &api;
        async move {
            api.exec(&sql)
                .await
                .unwrap_or_else(|e| panic!("{}: {}", sql, e));
        }
    };
    exec(format!(
        "CREATE OR REPLACE TABLE {TABLE} (
            C_INT NUMBER(38,0), C_BIG NUMBER(38,0), C_DEC NUMBER(10,2), C_DEC38 NUMBER(38,10),
            C_FLOAT FLOAT, C_BOOL BOOLEAN, C_DATE DATE, C_TIME TIME, C_NTZ TIMESTAMP_NTZ(9),
            C_TZ TIMESTAMP_TZ, C_LTZ TIMESTAMP_LTZ, C_TEXT VARCHAR, C_CHAR CHAR(5),
            C_BINARY BINARY, C_VARIANT VARIANT, C_OBJECT OBJECT, C_ARRAY ARRAY
        )"
    ))
    .await;
    exec(format!(
        "INSERT INTO {TABLE}
         SELECT 9223372036854775807, 12345678901234567890, 123.45, 123.4567890123, 0.1, TRUE,
                '2024-01-15'::DATE, '10:30:00'::TIME, '2024-01-15 10:30:00.123456'::TIMESTAMP_NTZ,
                '2024-01-15 10:30:00 +02:00'::TIMESTAMP_TZ, '2024-01-15 10:30:00 +00:00'::TIMESTAMP_LTZ,
                '01826', 'ab', TO_BINARY('DEADBEEF', 'HEX'), PARSE_JSON('{{\"a\": 1}}'),
                OBJECT_CONSTRUCT('k', 1), ARRAY_CONSTRUCT(1, 2)
         UNION ALL
         SELECT -1, NULL, -0.01, NULL, 1.5, FALSE, NULL, NULL, NULL, NULL, NULL, '', NULL, NULL,
                NULL, NULL, NULL"
    ))
    .await;

    let checks = [
        check("C_INT", Integer, &["int 9223372036854775807", "int -1"]),
        // decision: NUMBER(38,0) is how Snowflake stores every integer, so it's an Integer
        // column; a value past i64 comes through as an exact decimal rather than being lost
        check("C_BIG", Integer, &["dec 12345678901234567890", "null"]),
        check("C_DEC", Decimal, &["dec 123.45", "dec -0.01"]),
        check("C_DEC38", Decimal, &["dec 123.4567890123", "null"]),
        check("C_FLOAT", Decimal, &["dec 0.1", "dec 1.5"]),
        check("C_BOOL", Boolean, &["bool true", "bool false"]),
        check("C_DATE", Date, &["date 2024-01-15", "null"]),
        // decision: no Time type, so times are text
        check("C_TIME", STRING, &["str 10:30:00", "null"]),
        check(
            "C_NTZ",
            DateTime,
            &["ts 2024-01-15T10:30:00.123456Z", "null"],
        ),
        check("C_TZ", DateTime, &["ts 2024-01-15T08:30:00Z", "null"]),
        check("C_LTZ", DateTime, &["ts 2024-01-15T10:30:00Z", "null"]),
        check("C_TEXT", STRING, &["str 01826", "str "]),
        check("C_CHAR", STRING, &["str ab", "null"]),
        // decision: binary values are hex text, 0x-prefixed
        check("C_BINARY", STRING, &["str 0xdeadbeef", "null"]),
        check("C_VARIANT", Json, &["json {\"a\":1}", "null"]),
        check("C_OBJECT", Json, &["json {\"k\":1}", "null"]),
        check("C_ARRAY", Json, &["json [1,2]", "null"]),
    ];

    let mut url = format!(
        "snowflake://{}@{}/{}/{}?warehouse={}",
        get("USER").unwrap(),
        get("ACCOUNT").unwrap(),
        get("DATABASE").unwrap(),
        get("SCHEMA").unwrap(),
        get("WAREHOUSE").unwrap()
    );
    if let Some(role) = get("ROLE") {
        url.push_str(&format!("&role={}", role));
    }
    let options = HashMap::from([(
        "private_key_path".to_string(),
        get("PRIVATE_KEY_PATH").unwrap(),
    )]);
    let result = AssertUnwindSafe(async {
        let problems = match try_detect_with(&format!("{}#{}", url, TABLE), &options).await {
            Ok((schema, rows)) => compare(&schema, &rows, &checks),
            Err(e) => vec![format!("error: {}", e)],
        };
        report("Snowflake", checks.len(), problems);
    })
    .catch_unwind()
    .await;
    exec(format!("DROP TABLE IF EXISTS {TABLE}")).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
