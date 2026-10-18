//! M2c spec §2–§5 against the real loaded extension: storage classes,
//! affinity, capture, the new aggregates and MIN/MAX.

mod common;
use common::*;

use ivmlite_core::Value;
use ivmlite_test::open_with_extension;
use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use std::collections::BTreeMap;

/// `pragma_table_info(<table>)`'s `(name, type)` rows, in column order.
fn declared(c: &Connection, table: &str) -> Vec<(String, String)> {
    let mut s = c
        .prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap();
    s.query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn types_of(c: &Connection, table: &str, columns: &[&str]) -> Vec<String> {
    let all = declared(c, table);
    columns
        .iter()
        .map(|name| {
            all.iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{table} has no column {name}: {all:?}"))
                .1
                .clone()
        })
        .collect()
}

#[test]
fn shadow_value_columns_have_no_declared_type() {
    // Spec §3: SQLite applies no affinity to these columns, so it stores every
    // value with its storage class unchanged.
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    assert_eq!(
        types_of(&c, "__ivm_delta_orders", &["region", "amount"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_delta_orders", &["__ivm_seq", "__ivm_w"]),
        ["INTEGER", "INTEGER"]
    );
    assert_eq!(
        types_of(&c, "__ivm_pend_orders", &["region", "amount"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_out_sums", &["region", "SUM(amount)", "COUNT(*)"]),
        ["", "", ""]
    );
    assert_eq!(types_of(&c, "__ivm_out_sums", &["__w"]), ["INTEGER"]);
    assert_eq!(
        types_of(&c, "__ivm_stage_sums", &["c0", "c1", "c2", "key", "val"]),
        ["", "", "", "", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_state_sums_0_agg_groups", &["key", "val"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "sums", &["region", "SUM(amount)", "COUNT(*)"]),
        ["", "", ""]
    );
}

fn one_row(c: &Connection, sql: &str) -> Vec<Sql> {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    s.query_row([], |r| (0..n).map(|i| r.get::<_, Sql>(i)).collect())
        .unwrap()
}

#[test]
fn sum_total_avg_and_count_over_integers_and_text() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER, s TEXT) STRICT;
         INSERT INTO t VALUES ('a', 1, '1.5'), ('a', 2, 'abc'), ('a', NULL, NULL), ('b', NULL, '12');",
    )
    .unwrap();
    let sql = "SELECT k, SUM(i), TOTAL(i), AVG(i), COUNT(i), SUM(s), TOTAL(s), AVG(s), COUNT(s), \
               COUNT(*) FROM t GROUP BY k";
    create(&c, "v", sql).unwrap();
    // Every sum here is exact in f64, so SQLite's own result is the exact one.
    assert_matches_oracle(&c, "v", sql);
    c.execute_batch("DELETE FROM t WHERE s = '1.5'; INSERT INTO t VALUES ('b', 4, '0.25');")
        .unwrap();
    refresh(&c, "v").unwrap();
    assert_matches_oracle(&c, "v", sql);
}

#[test]
fn avg_differs_from_sqlite_only_by_rounding_the_exact_mean_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, s TEXT) STRICT;
         INSERT INTO t VALUES ('a', '0.1'), ('a', '0.2'), ('a', '0.3');",
    )
    .unwrap();
    create(&c, "v", "SELECT k, AVG(s), SUM(s) FROM t GROUP BY k").unwrap();
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![Sql::Text("a".into()), Sql::Real(0.2), Sql::Real(0.6)]
    );
    // SQLite rounds the sum, then divides (spec §4, "Where this differs").
    assert_eq!(
        one_row(&c, "SELECT avg(s) FROM t"),
        vec![Sql::Real(0.19999999999999998)]
    );
}

#[test]
fn sum_overflow_fails_the_refresh_with_its_message_and_changes_nothing() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER) STRICT;
         INSERT INTO t VALUES ('a', 9223372036854775807);",
    )
    .unwrap();
    create(&c, "v", "SELECT k, SUM(i), TOTAL(i) FROM t GROUP BY k").unwrap();
    c.execute_batch("INSERT INTO t VALUES ('a', 1)").unwrap();
    let e = refresh(&c, "v").expect_err("the final exact sum is outside i64");
    assert!(e.to_string().contains("integer overflow in SUM"), "{e}");
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![
            Sql::Text("a".into()),
            Sql::Integer(i64::MAX),
            Sql::Real(9_223_372_036_854_775_808.0)
        ]
    );
    // Back in range: SQLite's own `sum` would fail on this prefix; ivmlite
    // judges the final sum.
    c.execute_batch("INSERT INTO t VALUES ('a', -1)").unwrap();
    refresh(&c, "v").unwrap();
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![
            Sql::Text("a".into()),
            Sql::Integer(i64::MAX),
            Sql::Real(9_223_372_036_854_775_808.0)
        ]
    );
}

#[test]
fn a_sum_overflow_in_one_group_rolls_back_every_group() {
    // Group `a` changes fine in the same refresh in which `b` overflows; the
    // refresh fails as a whole, so neither group's state nor output changes.
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER) STRICT;
         INSERT INTO t VALUES ('a', 1), ('b', 9223372036854775807);",
    )
    .unwrap();
    let sql = "SELECT k, SUM(i), COUNT(*) FROM t GROUP BY k";
    create(&c, "v", sql).unwrap();
    let before = durable_state(&c, "v");
    let output = rows(&c, "SELECT * FROM v");
    c.execute_batch("INSERT INTO t VALUES ('a', 2), ('b', 1)")
        .unwrap();
    let e = refresh(&c, "v").expect_err("group b's final exact sum is outside i64");
    assert!(e.to_string().contains("integer overflow in SUM"), "{e}");
    assert_eq!(durable_state(&c, "v"), before);
    assert_eq!(rows(&c, "SELECT * FROM v"), output);
    assert_eq!(
        output,
        vec![
            vec![Sql::Text("a".into()), Sql::Integer(1), Sql::Integer(1)],
            vec![
                Sql::Text("b".into()),
                Sql::Integer(i64::MAX),
                Sql::Integer(1)
            ],
        ]
    );
}

/// A SQLite value as ivmlite's value, exactly (REAL bit for bit).
fn exact(v: Sql) -> Value {
    match v {
        Sql::Null => Value::Null,
        Sql::Integer(n) => Value::Int(n),
        Sql::Real(x) => Value::Real(x),
        Sql::Text(s) => Value::Text(s),
        Sql::Blob(b) => Value::Blob(b),
    }
}

/// `sql`'s rows, its first column a weight, summed by exact identity of the rest.
fn weighted(c: &Connection, sql: &str) -> BTreeMap<Vec<Value>, i64> {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    let mut out: BTreeMap<Vec<Value>, i64> = BTreeMap::new();
    let rows: Vec<(i64, Vec<Value>)> = s
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                (1..n)
                    .map(|i| r.get::<_, Sql>(i).map(exact))
                    .collect::<rusqlite::Result<_>>()?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (w, row) in rows {
        *out.entry(row).or_insert(0) += w;
    }
    out.retain(|_, w| *w != 0);
    out
}

#[test]
fn non_strict_capture_keeps_every_storage_class() {
    // Spec §6: a value of each storage class, written through INSERT, UPDATE
    // and REPLACE into a column of each affinity, is captured exactly as SQLite
    // stored it; a DELETE then cancels it by row identity.
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, i INTEGER, r REAL, n NUMERIC, x TEXT, b)",
    )
    .unwrap();
    create(&c, "v", "SELECT id, COUNT(*) FROM t GROUP BY id").unwrap();
    let values = [
        Sql::Null,
        Sql::Integer(1),
        Sql::Integer(i64::MAX),
        Sql::Real(1.0),
        Sql::Real(1.5),
        Sql::Real(-0.0),
        Sql::Real(f64::INFINITY),
        Sql::Text("12".into()),
        Sql::Text(" 3.0 ".into()),
        Sql::Text("abc".into()),
        Sql::Blob(vec![0, 0xff]),
    ];
    for (id, v) in values.iter().enumerate() {
        c.execute(
            "INSERT INTO t VALUES (?1, ?2, ?2, ?2, ?2, ?2)",
            rusqlite::params![id as i64, v],
        )
        .unwrap();
    }
    for (id, v) in values.iter().rev().enumerate() {
        c.execute(
            "UPDATE t SET i = ?2, r = ?2, n = ?2, x = ?2, b = ?2 WHERE id = ?1",
            rusqlite::params![id as i64, v],
        )
        .unwrap();
    }
    for (id, v) in values.iter().enumerate().step_by(2) {
        c.execute(
            "REPLACE INTO t VALUES (?1, ?2, ?2, ?2, ?2, ?2)",
            rusqlite::params![id as i64, v],
        )
        .unwrap();
    }
    let delta = "SELECT __ivm_w, id, i, r, n, x, b FROM __ivm_delta_t";
    assert_eq!(
        weighted(&c, delta),
        weighted(&c, "SELECT 1, id, i, r, n, x, b FROM t")
    );
    // Row 7 ends with REAL 1.0 (the reversed UPDATE loop); pinned as SQLite
    // 3.53 stores it in each affinity (measured).
    assert_eq!(
        one_row(
            &c,
            "SELECT typeof(i), typeof(r), typeof(n), typeof(x), x, typeof(b) FROM t WHERE id = 7"
        ),
        vec![
            Sql::Text("integer".into()),
            Sql::Text("real".into()),
            Sql::Text("integer".into()),
            Sql::Text("text".into()),
            Sql::Text("1.0".into()),
            Sql::Text("real".into()),
        ]
    );
    c.execute_batch("DELETE FROM t").unwrap();
    assert!(
        weighted(&c, delta).is_empty(),
        "every captured DELETE cancels its INSERT"
    );
}

#[test]
fn every_declared_type_is_accepted_and_one_and_one_point_zero_are_one_group() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k, v VARCHAR(255), d DATETIME, f FLOAT(8,2), z BOOLEAN);
         CREATE TABLE s(k ANY, v INT) STRICT;
         INSERT INTO t VALUES (1, 'a', 1, 1.5, 1), (1.0, 'b', 2, 2.5, 0), ('1', 'c', 3, 3.5, 1);",
    )
    .unwrap();
    let sql = "SELECT k, COUNT(*), SUM(f) FROM t GROUP BY k";
    create(&c, "g", sql).unwrap();
    create(&c, "h", "SELECT k, COUNT(*) FROM s GROUP BY k").unwrap();
    // Two groups, as in SQLite: 1 with 1.0, and '1'. A group with a REAL key row is REAL.
    assert_eq!(
        rows(&c, "SELECT typeof(k), k, \"COUNT(*)\", \"SUM(f)\" FROM g"),
        rows(
            &c,
            "SELECT 'real', 1.0, 2, 4.0 UNION ALL SELECT 'text', '1', 1, 3.5"
        )
    );
    assert_eq!(
        count(&c, "SELECT count(*) FROM (SELECT k FROM t GROUP BY k)"),
        2
    );
}

#[test]
fn changing_a_declared_type_with_the_same_affinity_breaks_the_view() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k VARCHAR(10), v INTEGER)")
        .unwrap();
    create(&c, "g", "SELECT k, COUNT(*) FROM t GROUP BY k").unwrap();
    c.execute_batch("DROP TABLE t; CREATE TABLE t(k TEXT, v INTEGER)")
        .unwrap();
    let e = refresh(&c, "g").expect_err("the shape records the declared type");
    assert!(e.to_string().contains("changed shape"), "{e}");
}

#[test]
fn a_sum_over_a_real_column_is_the_exact_sum_rounded_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, x REAL); INSERT INTO t VALUES ('a', 0.1), ('a', 0.2), ('a', 0.3)",
    )
    .unwrap();
    create(&c, "v", "SELECT k, SUM(x), AVG(x) FROM t GROUP BY k").unwrap();
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![Sql::Text("a".into()), Sql::Real(0.6), Sql::Real(0.2)]
    );
}
