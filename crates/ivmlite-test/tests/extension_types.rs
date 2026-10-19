//! M2c spec §2–§5 against the real loaded extension: storage classes,
//! affinity, capture, the new aggregates and MIN/MAX.

mod common;
use common::*;

use ivmlite_core::{from_sort_key, Value};
use ivmlite_test::open_with_extension;
use rusqlite::types::Value as Sql;
use rusqlite::{ffi, Connection};
use std::collections::BTreeMap;
use std::ffi::CStr;

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
    // A STRICT `ANY` column keeps every value's own storage class, and its
    // groups keep them too.
    c.execute_batch(
        "INSERT INTO s VALUES (1, 1), (1.0, 2), ('1', 3), (x'01', 4), (NULL, 5), (2.5, 6), (2.5, 7)",
    )
    .unwrap();
    refresh(&c, "h").unwrap();
    assert_eq!(
        rows(&c, "SELECT typeof(k), k, \"COUNT(*)\" FROM h"),
        rows(
            &c,
            "SELECT 'real', 1.0, 2 UNION ALL SELECT 'text', '1', 1 UNION ALL \
             SELECT 'blob', x'01', 1 UNION ALL SELECT 'null', NULL, 1 UNION ALL \
             SELECT 'real', 2.5, 2"
        )
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

#[test]
fn min_and_max_survive_retracting_the_current_extreme() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 5), ('a', 3), ('a', 9), ('b', 'x');",
    )
    .unwrap();
    let sql = "SELECT k, MIN(v), MAX(v), COUNT(*) FROM t GROUP BY k";
    create(&c, "m", sql).unwrap();
    assert_matches_oracle(&c, "m", sql);
    c.execute_batch("DELETE FROM t WHERE v = 3; DELETE FROM t WHERE v = 9;")
        .unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
    // Retract the current minimum and add smaller ones in the same refresh:
    // the read must see the refresh's own pending writes.
    c.execute_batch("DELETE FROM t WHERE v = 5; INSERT INTO t VALUES ('a', 1), ('a', 2);")
        .unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
    c.execute_batch("INSERT INTO t VALUES ('c', NULL)").unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
}

const EXTREMA_QUERY: &str = "SELECT k, MIN(x), MAX(x), COUNT(*) FROM t GROUP BY k";

/// Inspect every persisted entry, so a correct visible extreme cannot hide
/// stale values, a wrong duplicate weight, or a vanished group's state.
fn assert_extremum_entries(c: &Connection, expected: &[(Value, i64)]) {
    for aggregate in 0..2 {
        let table = format!("__ivm_state_m_0_agg_extremum_{aggregate}");
        let entries: Vec<(Value, i64)> = c
            .prepare(&format!("SELECT val, w FROM {table} ORDER BY val"))
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .map(|r| {
                let (val, weight) = r.unwrap();
                (from_sort_key(&val).unwrap(), weight)
            })
            .collect();
        assert_eq!(entries, expected, "persisted entries in {table}");
    }
    assert_matches_oracle(c, "m", EXTREMA_QUERY);
}

fn extrema_fixture(values: &str) -> Connection {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(&format!(
        "CREATE TABLE t(k TEXT, x); INSERT INTO t VALUES {values};"
    ))
    .unwrap();
    create(&c, "m", EXTREMA_QUERY).unwrap();
    c
}

#[test]
fn min_and_max_retract_every_non_null_value_from_a_surviving_group() {
    let c = extrema_fixture("('a', NULL), ('a', NULL), ('a', 3), ('a', 9)");
    assert_extremum_entries(&c, &[(Value::Int(3), 1), (Value::Int(9), 1)]);
    c.execute_batch("DELETE FROM t WHERE x IS NOT NULL")
        .unwrap();
    refresh(&c, "m").unwrap();
    assert_extremum_entries(&c, &[]);
    assert_eq!(
        one_row(&c, "SELECT * FROM m"),
        vec![Sql::Text("a".into()), Sql::Null, Sql::Null, Sql::Integer(2)]
    );
}

#[test]
fn min_and_max_remove_every_persisted_entry_of_a_vanished_group() {
    let c = extrema_fixture("('a', 3), ('a', 9)");
    assert_extremum_entries(&c, &[(Value::Int(3), 1), (Value::Int(9), 1)]);
    c.execute_batch("DELETE FROM t").unwrap();
    refresh(&c, "m").unwrap();
    assert_extremum_entries(&c, &[]);
    assert!(rows(&c, "SELECT * FROM m").is_empty());
}

#[test]
fn min_and_max_preserve_a_value_retracted_and_readded_in_one_refresh() {
    let c = extrema_fixture("('a', 3), ('a', 9)");
    let expected = [(Value::Int(3), 1), (Value::Int(9), 1)];
    assert_extremum_entries(&c, &expected);
    // The NULL changes COUNT(*) and forces the aggregate to read its stored
    // extremes even though the same-value retraction and addition cancel.
    c.execute_batch("DELETE FROM t WHERE x = 3; INSERT INTO t VALUES ('a', 3), ('a', NULL)")
        .unwrap();
    refresh(&c, "m").unwrap();
    assert_extremum_entries(&c, &expected);
}

#[test]
fn min_and_max_keep_a_duplicate_extreme_with_weight_one_after_one_retraction() {
    let c = extrema_fixture("('a', 3), ('a', 3), ('a', 9), ('a', 9)");
    assert_extremum_entries(&c, &[(Value::Int(3), 2), (Value::Int(9), 2)]);
    c.execute_batch(
        "DELETE FROM t WHERE rowid = (SELECT rowid FROM t WHERE x = 3 LIMIT 1);
         DELETE FROM t WHERE rowid = (SELECT rowid FROM t WHERE x = 9 LIMIT 1);",
    )
    .unwrap();
    refresh(&c, "m").unwrap();
    assert_extremum_entries(&c, &[(Value::Int(3), 1), (Value::Int(9), 1)]);
}

#[test]
fn min_and_max_order_across_storage_classes() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, v);
         INSERT INTO t VALUES ('a', X'00'), ('a', 'text'), ('a', 2.5), ('a', 2), ('a', NULL);",
    )
    .unwrap();
    let sql = "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k";
    create(&c, "m", sql).unwrap();
    for delete in ["v = 2", "v = 2.5", "v = 'text'"] {
        assert_matches_oracle(&c, "m", sql);
        c.execute_batch(&format!("DELETE FROM t WHERE {delete}"))
            .unwrap();
        refresh(&c, "m").unwrap();
    }
    assert_matches_oracle(&c, "m", sql);
}

#[test]
fn a_numeric_tie_gives_min_the_integer_and_max_the_real() {
    // M2c spec §4 "Ties" for MIN; amendment 1 for MAX.
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 1.0), ('a', 1);")
        .unwrap();
    create(&c, "m", "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k").unwrap();
    assert_eq!(
        one_row(&c, "SELECT typeof(\"MIN(v)\"), typeof(\"MAX(v)\") FROM m"),
        vec![Sql::Text("integer".into()), Sql::Text("real".into())]
    );
}

/// The SQL text of every statement currently prepared on `c`'s handle that
/// contains `needle`. The extension prepares and caches its statements on the
/// same handle (Phase 6 spec §3), so this reads the engine's own text.
fn prepared_statements(c: &Connection, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: `c`'s own live handle, used from this thread only;
    // `sqlite3_next_stmt` walks its statement list without changing it, and
    // `sqlite3_sql` returns the text of a statement that stays prepared here.
    unsafe {
        let mut stmt = ffi::sqlite3_next_stmt(c.handle(), std::ptr::null_mut());
        while !stmt.is_null() {
            let sql = CStr::from_ptr(ffi::sqlite3_sql(stmt))
                .to_string_lossy()
                .into_owned();
            if sql.contains(needle) {
                out.push(sql);
            }
            stmt = ffi::sqlite3_next_stmt(c.handle(), stmt);
        }
    }
    out.sort();
    out
}

#[test]
fn the_extreme_is_read_through_the_state_tables_primary_key() {
    // M2c spec §6: the plan of the exact statement the engine prepared for
    // the read (its text is pinned by `state::first_sql`'s own test).
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 1)")
        .unwrap();
    create(&c, "m", "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k").unwrap();
    c.execute_batch("INSERT INTO t VALUES ('a', 2)").unwrap();
    refresh(&c, "m").unwrap();
    let tables: Vec<String> = rows(
        &c,
        "SELECT name FROM sqlite_schema WHERE name LIKE '__ivm_state_m_%_agg_extremum_%' ORDER BY name",
    )
    .into_iter()
    .map(|r| match &r[0] {
        Sql::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    })
    .collect();
    assert_eq!(
        tables,
        [
            "__ivm_state_m_0_agg_extremum_0",
            "__ivm_state_m_0_agg_extremum_1"
        ]
    );
    let reads = prepared_statements(&c, "_agg_extremum_");
    // MIN (aggregate 0) reads ascending, MAX (aggregate 1) descending.
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert!(
        reads[0].contains(&tables[0]) && reads[0].ends_with("ORDER BY val"),
        "{reads:?}"
    );
    assert!(
        reads[1].contains(&tables[1]) && reads[1].ends_with("ORDER BY val DESC"),
        "{reads:?}"
    );
    for sql in reads {
        let plan: Vec<String> = c
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([Sql::Blob(vec![])], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            plan.iter().any(|d| d.contains("USING PRIMARY KEY (key=?)")),
            "{sql}: {plan:?}"
        );
        assert!(
            !plan.iter().any(|d| d.contains("TEMP B-TREE")),
            "{sql}: {plan:?}"
        );
    }
}

#[test]
fn min_over_a_join_sees_both_tables_changes_in_one_refresh() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE a(k TEXT, x); CREATE TABLE b(k TEXT, y);
         INSERT INTO a VALUES ('p', 5), ('q', 1); INSERT INTO b VALUES ('p', 1), ('q', 2);",
    )
    .unwrap();
    let sql = "SELECT a.k, MIN(a.x), MAX(b.y), MIN(b.y) FROM a JOIN b ON a.k = b.k GROUP BY a.k";
    create(&c, "j", sql).unwrap();
    c.execute_batch(
        "INSERT INTO a VALUES ('p', 3); DELETE FROM a WHERE x = 5;
         INSERT INTO b VALUES ('p', 7); DELETE FROM b WHERE y = 2;",
    )
    .unwrap();
    refresh(&c, "j").unwrap();
    assert_matches_oracle(&c, "j", sql);
    // Both tables retract their current minimum of group `p` and add a
    // smaller one, in the same refresh.
    c.execute_batch(
        "DELETE FROM a WHERE x = 3; INSERT INTO a VALUES ('p', 2);
         DELETE FROM b WHERE y = 1; INSERT INTO b VALUES ('p', 0);",
    )
    .unwrap();
    refresh(&c, "j").unwrap();
    assert_matches_oracle(&c, "j", sql);
}

#[test]
fn dropping_a_min_view_removes_its_extremum_tables() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v)").unwrap();
    let before = objects(&c);
    create(&c, "m", "SELECT k, MIN(v) FROM t GROUP BY k").unwrap();
    c.execute_batch("DROP TABLE m").unwrap();
    assert_eq!(objects(&c), before);
}

#[test]
fn filters_follow_sqlites_comparison_affinity() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER, x TEXT, b);
         INSERT INTO t VALUES ('a', 3, '3', 3), ('a', 1, 'abc', '3'), ('b', 2, '2', X'00'), ('b', NULL, NULL, 2.5);",
    )
    .unwrap();
    let views: Vec<(String, String)> = [
        "i > '2'",
        "i = 2.5",
        "i < 9223372036854775808",
        "x = 3",
        "x > 2",
        "x != 'abc'",
        "b = 3",
        "b = '3'",
        "b > X'00'",
        "b < 'a'",
        "b >= 2.5",
        "i IS NULL",
        "i = '3.0'",
        "i = '2.5e1'",
        "i = '-0.0'",
    ]
    .iter()
    .enumerate()
    .map(|(n, cond)| {
        (
            format!("f{n}"),
            format!("SELECT k, COUNT(*) FROM t WHERE {cond} GROUP BY k"),
        )
    })
    .collect();
    for (name, sql) in &views {
        create(&c, name, sql).unwrap();
        assert_matches_oracle(&c, name, sql);
    }
    c.execute_batch(
        "INSERT INTO t VALUES ('a', 7, '7', 'z'), ('c', 2, '1e1', 3.0), ('c', 25, '25', 0.0), ('d', 0, '0', -0.0);
         DELETE FROM t WHERE i = 1;",
    )
    .unwrap();
    for (name, sql) in &views {
        refresh(&c, name).unwrap();
        assert_matches_oracle(&c, name, sql);
    }
    let e = create(
        &c,
        "bad",
        "SELECT k, COUNT(*) FROM t WHERE x > 2.5 GROUP BY k",
    )
    .unwrap_err();
    assert!(
        e.to_string().contains("REAL literal cannot be compared"),
        "{e}"
    );
}

#[test]
fn joins_match_canonical_keys_and_refuse_mixed_affinity() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE a(k, v TEXT); CREATE TABLE b(k, w TEXT);
         CREATE TABLE ai(k INTEGER, v TEXT); CREATE TABLE br(k REAL, w TEXT);
         INSERT INTO a VALUES (1, 'x'), (2.5, 'y'); INSERT INTO b VALUES (1.0, 'p'), ('1', 'q'), (2.5, 'r');",
    )
    .unwrap();
    let sql = "SELECT a.v, COUNT(*) FROM a JOIN b ON a.k = b.k GROUP BY a.v";
    create(&c, "j", sql).unwrap();
    assert_matches_oracle(&c, "j", sql);
    c.execute_batch("INSERT INTO a VALUES (1.0, 'x'); DELETE FROM b WHERE w = 'r';")
        .unwrap();
    refresh(&c, "j").unwrap();
    assert_matches_oracle(&c, "j", sql);
    let e = create(
        &c,
        "bad",
        "SELECT ai.v, COUNT(*) FROM ai JOIN br ON ai.k = br.k GROUP BY ai.v",
    )
    .unwrap_err();
    assert!(
        e.to_string().contains("INTEGER affinity") && e.to_string().contains("REAL affinity"),
        "{e}"
    );
}
