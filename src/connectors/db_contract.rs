//! Shared checks for database connectors (test only): typed round trips, one transaction per
//! load, deferred truncate, and streaming reads. Each connector's tests call [`run_all`] with
//! functions that open a source and a target on the same table.

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

use crate::connectors::{Source, Target};
use crate::schema::{Column, DataType, Row, Schema, Value};

pub(crate) type MakeSource = Box<dyn Fn() -> Box<dyn Source>>;
pub(crate) type MakeTarget = Box<dyn Fn() -> Box<dyn Target>>;

fn schema() -> Schema {
    let column = |name: &str, data_type| Column {
        name: name.to_string(),
        data_type,
        nullable: true,
    };
    Schema {
        columns: vec![
            column("id", DataType::Integer),
            column("name", DataType::String),
            column("price", DataType::Decimal),
            column("active", DataType::Boolean),
            column("at", DataType::DateTime),
        ],
        estimated_rows: None,
        primary_key_candidate: None,
    }
}

/// Every 50th row has no `name` key (an optional field in JSON), including the first row of
/// each batch, which must not decide the columns written.
fn row(i: i64) -> Row {
    let mut row = HashMap::from([
        ("id".to_string(), Value::Integer(i)),
        (
            "name".to_string(),
            Value::String(format!("Zoë {} \"quoted\"", i)),
        ),
        (
            "price".to_string(),
            Value::Decimal(Decimal::from_str("19.25").unwrap() + Decimal::from(i)),
        ),
        ("active".to_string(), Value::Boolean(i % 2 == 0)),
        (
            "at".to_string(),
            Value::Date(Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap()),
        ),
    ]);
    if i % 50 == 0 {
        row.remove("name");
    }
    row
}

fn rows(range: std::ops::Range<i64>) -> Vec<Row> {
    range.map(row).collect()
}

/// Write `rows` the way the transfer engine does; `finalize: false` simulates a failed run.
async fn load(make_target: &MakeTarget, rows: &[Row], truncate: bool, finalize: bool) {
    let mut target = make_target();
    target.connect().await.unwrap();
    if truncate && target.exists("t").await.unwrap() {
        target.truncate("t").await.unwrap();
    }
    target.create_table("t", &schema()).await.unwrap();
    for chunk in rows.chunks(100) {
        target.write_batch(chunk).await.unwrap();
    }
    if finalize {
        target.finalize().await.unwrap();
    }
    // Dropped here: without finalize, the load must roll back
}

/// Read every row, `batch_size` at a time.
async fn read_all(make_source: &MakeSource, batch_size: usize) -> Vec<Row> {
    let mut source = make_source();
    source.connect().await.unwrap();
    source.infer_schema(100).await.unwrap();
    source.reset().await.unwrap();
    let mut all = Vec::new();
    while source.has_more() {
        let batch = source.read_batch(batch_size).await.unwrap();
        if batch.is_empty() {
            break;
        }
        assert!(batch.len() <= batch_size);
        all.extend(batch);
    }
    all
}

fn ids(rows: &[Row]) -> Vec<i64> {
    let mut ids: Vec<i64> = rows
        .iter()
        .map(|r| match r.get("id") {
            Some(Value::Integer(i)) => *i,
            other => panic!("id is {:?}", other),
        })
        .collect();
    ids.sort();
    ids
}

pub(crate) async fn run_all(make_source: MakeSource, make_target: MakeTarget) {
    // Typed round trip, and a streaming read across batch boundaries
    load(&make_target, &rows(0..250), false, true).await;
    let read = read_all(&make_source, 64).await;
    assert_eq!(
        ids(&read),
        (0..250).collect::<Vec<_>>(),
        "every row exactly once"
    );
    let r7 = read.iter().find(|r| r["id"] == Value::Integer(7)).unwrap();
    assert_eq!(r7["name"], Value::String("Zoë 7 \"quoted\"".to_string()));
    let price = match &r7["price"] {
        Value::Decimal(d) => *d,
        other => panic!("price is {:?}", other),
    };
    assert_eq!(price, Decimal::from_str("26.25").unwrap());
    let names = read
        .iter()
        .filter(|r| matches!(r.get("name"), Some(Value::String(_))))
        .count();
    assert_eq!(
        names, 245,
        "a key missing from some rows is NULL only in those rows"
    );

    // reset() after reading starts over
    let mut source = make_source();
    source.connect().await.unwrap();
    source.infer_schema(100).await.unwrap();
    source.reset().await.unwrap();
    assert_eq!(source.read_batch(10).await.unwrap().len(), 10);
    source.reset().await.unwrap();
    let mut again = 0;
    while source.has_more() {
        let batch = source.read_batch(100).await.unwrap();
        if batch.is_empty() {
            break;
        }
        again += batch.len();
    }
    assert_eq!(again, 250, "reset re-reads from the start");
    drop(source);

    // A load that never reaches finalize leaves the table as it was
    load(&make_target, &rows(1000..1300), false, false).await;
    assert_eq!(
        read_all(&make_source, 1000).await.len(),
        250,
        "failed append rolled back"
    );
    load(&make_target, &rows(1000..1300), true, false).await;
    assert_eq!(
        read_all(&make_source, 1000).await.len(),
        250,
        "failed --truncate rolled back"
    );

    // Successful append, --truncate, and --truncate with nothing to load
    load(&make_target, &rows(250..260), false, true).await;
    assert_eq!(read_all(&make_source, 1000).await.len(), 260);
    load(&make_target, &rows(0..5), true, true).await;
    assert_eq!(ids(&read_all(&make_source, 1000).await), [0, 1, 2, 3, 4]);
    load(&make_target, &[], true, true).await;
    assert!(
        read_all(&make_source, 1000).await.is_empty(),
        "empty --truncate"
    );
}
