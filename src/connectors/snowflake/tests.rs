use super::client::ColumnInfo;
use super::fake_client::{FakeClient, FakeResult, Recorded};
use super::*;
use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType as ArrowType, Field, Schema as ArrowSchema};
use arrow::record_batch::RecordBatch;
use std::sync::{Arc, Mutex};

fn no_options() -> HashMap<String, String> {
    HashMap::new()
}

fn options(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn config(url: &str) -> SnowflakeConfig {
    SnowflakeConfig::parse_with_env(url, &no_options(), no_env).unwrap()
}

// --- configuration ---------------------------------------------------------------------------

#[test]
fn test_parse_url() {
    let c = config("snowflake://loader:s%40cret%21@myorg-acct1/ANALYTICS/RAW?warehouse=LOAD_WH&role=LOADER#orders");
    assert_eq!(c.account, "myorg-acct1");
    assert_eq!(c.user, "loader");
    assert_eq!(c.password.as_deref(), Some("s@cret!"));
    assert_eq!(
        (c.database.as_str(), c.schema.as_str()),
        ("ANALYTICS", "RAW")
    );
    assert_eq!(c.warehouse.as_deref(), Some("LOAD_WH"));
    assert_eq!(c.role.as_deref(), Some("LOADER"));
    assert_eq!(c.table, "orders");
    assert_eq!(c.load_method, LoadMethod::Auto);
    assert_eq!(c.stage_file_rows, DEFAULT_STAGE_FILE_ROWS);
    // Debug output never shows secrets
    assert!(!format!("{:?}", c).contains("s@cret"));
}

#[test]
fn test_parse_url_variants() {
    // The mock's ?table= form, a full host name, and no password
    let c = config("snowflake://u@xy12345.us-east-1.snowflakecomputing.com/db/sch?table=t");
    assert_eq!(c.account, "xy12345.us-east-1");
    assert_eq!(c.table, "t");
    assert_eq!(c.password, None);
    // #table wins over ?table=, and a qualified table name is kept as given
    assert_eq!(
        config("snowflake://u@a/db/sch?table=x#other.t").table,
        "other.t"
    );
}

#[test]
fn test_parse_url_errors() {
    for (url, needle) in [
        ("snowflake://u@a/db/sch", "missing table"),
        ("snowflake://u@a/db#t", "/database/schema"),
        ("snowflake://u@a/db/sch/extra#t", "/database/schema"),
        ("snowflake://@a/db/sch#t", "missing user"),
        (
            "snowflake://u@a/db/sch?warehose=x#t",
            "unknown parameter 'warehose'",
        ),
        ("postgres://u@a/db/sch#t", "scheme"),
    ] {
        let err = SnowflakeConfig::parse_with_env(url, &no_options(), no_env)
            .unwrap_err()
            .to_string();
        assert!(err.contains(needle), "{} -> {}", url, err);
    }
}

#[test]
fn test_options_and_env() {
    let url = "snowflake://u@a/db/sch?warehouse=URL_WH#t";
    let c = SnowflakeConfig::parse_with_env(
        url,
        &options(&[
            ("warehouse", "OPT_WH"),
            ("load_method", "insert"),
            ("stage_file_rows", "1000"),
        ]),
        |name| (name == "SNOWFLAKE_PRIVATE_KEY_PATH").then(|| "/keys/rsa.p8".to_string()),
    )
    .unwrap();
    assert_eq!(c.warehouse.as_deref(), Some("OPT_WH"));
    assert_eq!(c.load_method, LoadMethod::Insert);
    assert_eq!(c.stage_file_rows, 1000);
    assert_eq!(c.private_key_path.as_deref(), Some("/keys/rsa.p8"));
    // An explicit option beats the environment
    let c = SnowflakeConfig::parse_with_env(
        url,
        &options(&[("private_key_path", "/opt/key.p8")]),
        |_| Some("/env/key.p8".to_string()),
    )
    .unwrap();
    assert_eq!(c.private_key_path.as_deref(), Some("/opt/key.p8"));

    for bad in [
        options(&[("privte_key_path", "x")]),
        options(&[("load_method", "bulk")]),
        options(&[("stage_file_rows", "0")]),
    ] {
        assert!(SnowflakeConfig::parse_with_env(url, &bad, no_env).is_err());
    }
}

// --- SQL --------------------------------------------------------------------------------------

#[test]
fn test_identifiers() {
    assert_eq!(ident("customer_id"), "customer_id");
    assert_eq!(ident("Amount$1"), "Amount$1");
    assert_eq!(ident("order"), "\"ORDER\"");
    assert_eq!(ident("Values"), "\"VALUES\"");
    assert_eq!(ident("first name"), "\"first name\"");
    assert_eq!(ident("1st"), "\"1st\"");
    assert_eq!(ident("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(qualified("RAW.orders"), "RAW.orders");
    assert_eq!(qualified("db.my schema.t"), "db.\"my schema\".t");
    assert_eq!(table_stage("orders", "tinyetl/x/"), "@%orders/tinyetl/x/");
    assert_eq!(
        table_stage("db.raw.my table", "p/"),
        "@db.raw.%\"my table\"/p/"
    );
}

fn schema(columns: &[(&str, DataType)]) -> Schema {
    Schema {
        columns: columns
            .iter()
            .map(|(name, data_type)| Column {
                name: name.to_string(),
                data_type: data_type.clone(),
                nullable: true,
            })
            .collect(),
        estimated_rows: None,
        primary_key_candidate: None,
    }
}

#[test]
fn test_sql_builders() {
    let s = schema(&[
        ("id", DataType::Integer),
        ("price", DataType::Decimal),
        ("tags", DataType::Json),
        ("when", DataType::DateTime),
    ]);
    assert_eq!(
        create_table_sql("orders", &s),
        "CREATE TABLE IF NOT EXISTS orders (id NUMBER(38,0), price NUMBER(38,10), \
         tags VARIANT, \"WHEN\" TIMESTAMP_NTZ)"
    );
    let copy = copy_sql("orders", &s, "@%orders/tinyetl/r/");
    assert!(copy.starts_with(
        "COPY INTO orders (id, price, tags, \"WHEN\") FROM (SELECT $1, $2, PARSE_JSON($3), $4 \
         FROM @%orders/tinyetl/r/) FILE_FORMAT = (TYPE = CSV"
    ));
    assert!(copy.ends_with("ON_ERROR = ABORT_STATEMENT PURGE = TRUE"));
    assert_eq!(
        insert_sql("orders", &s, 2),
        "INSERT INTO orders (id, price, tags, \"WHEN\") SELECT column1, column2, \
         PARSE_JSON(column3), column4 FROM VALUES (?, ?, ?, ?), (?, ?, ?, ?)"
    );
}

#[test]
fn test_region_and_rows_loaded() {
    assert!(region_is_aws("AWS_US_EAST_1"));
    assert!(region_is_aws("PUBLIC.AWS_EU_WEST_1"));
    assert!(!region_is_aws("AZURE_EASTUS2"));
    assert!(!region_is_aws("GCP_US_CENTRAL1"));

    let row = |n: i64| Row::from([("rows_loaded".to_string(), Value::Integer(n))]);
    assert_eq!(rows_loaded(&[row(3), row(4)]), 7);
    assert_eq!(rows_loaded(&[]), 0);
}

// --- connector behavior against the fake client ------------------------------------------------

fn batch(ids: &[i64]) -> RecordBatch {
    let schema = Arc::new(ArrowSchema::new(vec![
        Field::new("ID", ArrowType::Int64, false),
        Field::new("NAME", ArrowType::Utf8, true),
    ]));
    let names: Vec<String> = ids.iter().map(|i| format!("n{}", i)).collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(StringArray::from(names)) as ArrayRef,
        ],
    )
    .unwrap()
}

fn copy_result(rows: i64) -> FakeResult {
    let schema = Arc::new(ArrowSchema::new(vec![Field::new(
        "rows_loaded",
        ArrowType::Int64,
        false,
    )]));
    FakeResult {
        columns: vec![],
        total_rows: Some(1),
        batches: vec![RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![rows])) as ArrayRef],
        )
        .unwrap()],
    }
}

fn target(client: FakeClient, url: &str) -> (SnowflakeTarget, Arc<Mutex<Recorded>>) {
    let recorded = client.recorded.clone();
    let mut target = SnowflakeTarget::with_config(config(url));
    target.client = Some(Box::new(client));
    (target, recorded)
}

fn rows(values: &[(i64, Option<&str>, serde_json::Value)]) -> Vec<Row> {
    values
        .iter()
        .map(|(id, name, tags)| {
            Row::from([
                ("id".to_string(), Value::Integer(*id)),
                (
                    "name".to_string(),
                    name.map(|n| Value::String(n.to_string()))
                        .unwrap_or(Value::Null),
                ),
                ("tags".to_string(), Value::Json(tags.clone())),
            ])
        })
        .collect()
}

fn log(recorded: &Arc<Mutex<Recorded>>) -> Vec<String> {
    recorded
        .lock()
        .unwrap()
        .log
        .iter()
        .map(|entry| entry.split(" (").next().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn test_stage_load_with_truncate() {
    let client = FakeClient::new().with_result("COPY INTO", copy_result(3));
    let (mut target, recorded) = target(client, "snowflake://u@a/db/sch#orders");
    target.config.stage_file_rows = 2;
    let s = schema(&[
        ("id", DataType::Integer),
        ("name", DataType::String),
        ("tags", DataType::Json),
    ]);
    target.truncate("orders").await.unwrap();
    target.create_table("orders", &s).await.unwrap();
    let batch = rows(&[
        (1, Some("a \"q\""), serde_json::json!([1])),
        (2, None, serde_json::json!({})),
        (3, Some(""), serde_json::Value::Null),
    ]);
    assert_eq!(target.write_batch(&batch).await.unwrap(), 3);
    target.finalize().await.unwrap();

    let stage = target.stage_location();
    assert_eq!(
        log(&recorded),
        vec![
            "EXEC CREATE TABLE IF NOT EXISTS orders".to_string(),
            format!("PUT {}", stage),
            format!("PUT {}", stage),
            "EXEC BEGIN".to_string(),
            "EXEC DELETE FROM orders".to_string(),
            "QUERY COPY INTO orders".to_string(),
            "EXEC COMMIT".to_string(),
        ]
    );
    let uploads = &recorded.lock().unwrap().uploads;
    assert_eq!(uploads[0], "\"1\",\"a \"\"q\"\"\",\"[1]\"\n\"2\",,\"{}\"\n");
    assert_eq!(uploads[1], "\"3\",\"\",\"null\"\n");
}

#[tokio::test]
async fn test_copy_row_mismatch_rolls_back() {
    let client = FakeClient::new().with_result("COPY INTO", copy_result(1));
    let (mut target, recorded) = target(client, "snowflake://u@a/db/sch#orders");
    let s = schema(&[("id", DataType::Integer)]);
    target.create_table("orders", &s).await.unwrap();
    target
        .write_batch(&[
            Row::from([("id".to_string(), Value::Integer(1))]),
            Row::from([("id".to_string(), Value::Integer(2))]),
        ])
        .await
        .unwrap();
    let err = target.finalize().await.unwrap_err().to_string();
    assert!(err.contains("loaded 1 rows, but 2 were staged"), "{}", err);
    let log = log(&recorded);
    assert!(log.contains(&"EXEC ROLLBACK".to_string()));
    assert!(log
        .iter()
        .any(|l| l.starts_with("EXEC REMOVE @%orders/tinyetl/")));
    assert!(!log.contains(&"EXEC COMMIT".to_string()));
}

#[tokio::test]
async fn test_failed_copy_rolls_back() {
    let client = FakeClient::new().failing_on("COPY INTO");
    let (mut target, recorded) = target(client, "snowflake://u@a/db/sch#orders");
    target
        .create_table("orders", &schema(&[("id", DataType::Integer)]))
        .await
        .unwrap();
    target
        .write_batch(&[Row::from([("id".to_string(), Value::Integer(1))])])
        .await
        .unwrap();
    assert!(target.finalize().await.is_err());
    let log = log(&recorded);
    assert!(log.contains(&"EXEC ROLLBACK".to_string()));
    assert!(!log.contains(&"EXEC COMMIT".to_string()));
}

#[tokio::test]
async fn test_insert_load() {
    let (mut target, recorded) = target(FakeClient::new(), "snowflake://u@a/db/sch#orders");
    target.use_stage = false;
    let s = schema(&[
        ("id", DataType::Integer),
        ("name", DataType::String),
        ("tags", DataType::Json),
    ]);
    target.create_table("orders", &s).await.unwrap();
    target
        .write_batch(&rows(&[
            (1, Some("a"), serde_json::json!({"k": 1})),
            (2, None, serde_json::json!(null)),
        ]))
        .await
        .unwrap();
    target.finalize().await.unwrap();

    let recorded = recorded.lock().unwrap();
    let statements: Vec<&str> = recorded.log.iter().map(String::as_str).collect();
    assert_eq!(statements[1], "EXEC BEGIN");
    assert!(statements[2].starts_with(
        "EXEC INSERT INTO orders (id, name, tags) SELECT column1, column2, PARSE_JSON(column3) \
         FROM VALUES (?, ?, ?), (?, ?, ?)"
    ));
    assert_eq!(statements[3], "EXEC COMMIT");
    assert_eq!(
        recorded.binds[0],
        vec![
            Some("1".into()),
            Some("a".into()),
            Some(r#"{"k":1}"#.into()),
            Some("2".into()),
            None,
            Some("null".into()),
        ]
    );
}

#[tokio::test]
async fn test_truncate_without_rows() {
    let (mut target, recorded) = target(FakeClient::new(), "snowflake://u@a/db/sch#orders");
    target.truncate("orders").await.unwrap();
    target
        .create_table("orders", &schema(&[("id", DataType::Integer)]))
        .await
        .unwrap();
    target.finalize().await.unwrap();
    assert_eq!(
        log(&recorded)[1..],
        ["EXEC BEGIN", "EXEC DELETE FROM orders", "EXEC COMMIT"]
    );
}

#[tokio::test]
async fn test_source_batches_across_arrow_batches() {
    let columns = vec![
        ColumnInfo {
            name: "ID".into(),
            data_type: DataType::Integer,
            nullable: false,
        },
        ColumnInfo {
            name: "NAME".into(),
            data_type: DataType::String,
            nullable: true,
        },
    ];
    let client = FakeClient::new().with_result(
        "SELECT * FROM orders",
        FakeResult {
            columns: columns.clone(),
            total_rows: Some(5),
            batches: vec![batch(&[1, 2, 3]), batch(&[4, 5])],
        },
    );
    let recorded = client.recorded.clone();
    let mut source = SnowflakeSource::with_config(config("snowflake://u@a/db/sch#orders"));
    source.client = Some(Box::new(client));

    let schema = source.infer_schema(100).await.unwrap();
    let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["ID", "NAME"]);
    assert_eq!(source.estimated_row_count().await.unwrap(), Some(5));
    // reset() right after infer_schema doesn't re-run the query
    source.reset().await.unwrap();

    let mut ids = Vec::new();
    let mut sizes = Vec::new();
    while source.has_more() {
        let batch = source.read_batch(2).await.unwrap();
        if batch.is_empty() {
            break;
        }
        sizes.push(batch.len());
        ids.extend(batch.iter().map(|r| r["ID"].clone()));
    }
    assert_eq!(sizes, [2, 2, 1]);
    assert_eq!(ids, (1..=5).map(Value::Integer).collect::<Vec<_>>());
    let queries = |r: &Arc<Mutex<Recorded>>| {
        r.lock()
            .unwrap()
            .log
            .iter()
            .filter(|l| l.starts_with("QUERY"))
            .count()
    };
    assert_eq!(queries(&recorded), 1);

    // After reading, reset() runs the query again
    source.reset().await.unwrap();
    assert_eq!(queries(&recorded), 2);
    assert_eq!(source.read_batch(10).await.unwrap().len(), 5);
}
