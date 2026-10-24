//! Selective cursor snapshots retain SQLite's comparison semantics.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{create, rows};
use ivmlite_test::open_with_extension;
use rusqlite::types::Value;
use rusqlite::Connection;

fn fixture() -> Connection {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE facts(g ANY) STRICT;
         INSERT INTO facts VALUES
           (NULL),(-1),(0),(42),(42.5),(300000),
           (2251799813685247),(2251799813685248),
           (9007199254740992),(9007199254740993),
           (9223372036854775807),(-9223372036854775808),
           (1e999),(-1e999),(''),('42'),('042'),('42.5'),
           ('3.0e+5'),(' 42 '),('42x'),('a'),('A'),('a '),
           ('2251799813685247.0'),('2251799813685248.0'),
           ('9007199254740992'),('9007199254740993'),
           ('9223372036854775807'),('-9223372036854775808'),
           ('1e999'),('a'||char(0)||'b'),(x''),(x'3432');
         CREATE TABLE expected(g,n);
         INSERT INTO expected SELECT g,COUNT(*) FROM facts GROUP BY g;
         CREATE TABLE ints(k INTEGER);
         INSERT INTO ints VALUES (-1),(0),(42),(42.5),(9007199254740993);
         CREATE TABLE texts(k TEXT);
         INSERT INTO texts VALUES ('42'),('42.5'),('a'),('9007199254740993');
         CREATE TABLE reals(k REAL);
         INSERT INTO reals VALUES (42),(42.5),(9007199254740992);
         CREATE TABLE nums(k NUMERIC);
         INSERT INTO nums VALUES (42),(42.5),('a');
         CREATE TABLE blobs(k BLOB);
         INSERT INTO blobs VALUES (42),(42.5),('42'),(x''),(x'3432');",
    )
    .unwrap();
    create(&c, "roll", "SELECT g,COUNT(*) AS n FROM facts GROUP BY g").unwrap();
    c
}

fn assert_join(c: &Connection, rhs: &str) {
    for op in ["=", ">", ">=", "<", "<="] {
        for (left, right) in [("v.g", "r.k"), ("r.k", "v.g")] {
            // CROSS JOIN fixes the RHS as the outer loop, so SQLite can
            // supply the virtual table's usable comparison argument.
            let tail = format!("FROM {rhs} r CROSS JOIN {{table}} v WHERE {left} {op} {right}");
            let select = format!("SELECT typeof(v.g),v.g,v.n {tail}");
            assert_eq!(
                exact_rows(c, &select.replace("{table}", "roll"), &[]),
                exact_rows(c, &select.replace("{table}", "expected"), &[]),
                "RHS {rhs}, comparison {left} {op} {right}",
            );
        }
    }
}

#[test]
fn join_rhs_affinity_keeps_numeric_text_matches() {
    let c = fixture();
    for rhs in ["ints", "texts", "reals", "nums", "blobs"] {
        assert_join(&c, rhs);
    }
}

fn bound_rows(c: &Connection, sql: &str, value: &Value) -> Vec<Vec<ExactValue>> {
    exact_rows(c, sql, std::slice::from_ref(value))
}

#[test]
fn bound_storage_classes_and_numeric_boundaries_match_ordinary_sqlite() {
    let c = fixture();
    let mut values = vec![
        Value::Null,
        Value::Integer(i64::MIN),
        Value::Integer(-1),
        Value::Integer(0),
        Value::Integer(42),
        Value::Integer(2_251_799_813_685_248),
        Value::Integer(9_007_199_254_740_993),
        Value::Integer(i64::MAX),
        Value::Real(-0.0),
        Value::Real(42.0),
        Value::Real(42.5),
        Value::Real(f64::NEG_INFINITY),
        Value::Real(f64::INFINITY),
        Value::Blob(vec![]),
        Value::Blob(b"42".to_vec()),
    ];
    values.extend(
        [
            "", "42", "042", "42.0", "42x", "3.0e+5", " 42 ", "1e999", "a\0b",
        ]
        .map(|value| Value::Text(value.into())),
    );
    for value in values {
        for op in ["=", ">", ">=", "<", "<="] {
            for predicate in [format!("g {op} ?1"), format!("?1 {op} g")] {
                for tail in [
                    predicate.clone(),
                    format!("{predicate} AND g >= ?1 AND g <= ?1"),
                ] {
                    assert_eq!(
                        bound_rows(
                            &c,
                            &format!("SELECT typeof(g),g,n FROM roll WHERE {tail}"),
                            &value
                        ),
                        bound_rows(
                            &c,
                            &format!("SELECT typeof(g),g,n FROM expected WHERE {tail}"),
                            &value
                        ),
                        "bound {value:?}, predicate {tail}",
                    );
                }
            }
        }
    }
}

#[test]
fn standalone_real_signed_zero_filters_preserve_public_snapshot_bits() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE facts(g); INSERT INTO facts VALUES (-0.0); CREATE TABLE expected(g,n); INSERT INTO expected SELECT g,COUNT(*) FROM facts GROUP BY g").unwrap();
    create(&c, "roll", "SELECT g,COUNT(*) AS n FROM facts GROUP BY g").unwrap();
    let source = exact_rows(&c, "SELECT typeof(g),g FROM facts", &[]);
    let snapshot = exact_rows(&c, "SELECT typeof(g),g,n FROM roll", &[]);
    assert_eq!(snapshot.len(), 1);
    assert!(matches!(&source[0][1], ExactValue::Real(_)));
    assert!(matches!(&snapshot[0][1], ExactValue::Real(_)));
    println!("standalone signed-zero source={source:?}; public snapshot={snapshot:?}");
    // GROUP BY can choose a numeric representative; selective reads must
    // preserve the public snapshot's exact representation. Base GROUP BY
    // independently determines whether each comparison matches, without
    // imposing its representative bits on ivmlite's existing output format.
    for value in [Value::Real(-0.0), Value::Real(0.0), Value::Integer(0)] {
        for op in ["=", ">", ">=", "<", "<="] {
            for predicate in [format!("g {op} ?1"), format!("?1 {op} g")] {
                let expected = bound_rows(
                    &c,
                    &format!("SELECT typeof(g),g,n FROM expected WHERE {predicate}"),
                    &value,
                );
                let actual = bound_rows(
                    &c,
                    &format!("SELECT typeof(g),g,n FROM roll WHERE {predicate}"),
                    &value,
                );
                assert_eq!(actual.len(), expected.len(), "{predicate}, {value:?}");
                assert_eq!(
                    actual,
                    if expected.is_empty() {
                        Vec::new()
                    } else {
                        snapshot.clone()
                    },
                    "{predicate}, {value:?}"
                );
            }
        }
    }
}

#[test]
fn compound_rhs_affinity_can_differ_from_value_class() {
    let c = fixture();
    foreign_rhs::register(&c);
    // A compound SELECT can expose one arm's affinity while returning
    // another arm's unconverted storage class. Cover both arm orders.
    for rhs in [
        "(SELECT k FROM ints UNION ALL SELECT '42')",
        "(SELECT k FROM texts UNION ALL SELECT 42)",
        "(SELECT '42' AS k UNION ALL SELECT k FROM ints)",
        "(SELECT 42 AS k UNION ALL SELECT k FROM texts)",
        "(SELECT CAST(NULL AS INTEGER) AS k UNION ALL SELECT '42')",
        "(SELECT CAST(NULL AS TEXT) AS k UNION ALL SELECT 42)",
        "foreign_numeric_rhs",
        "foreign_text_rhs",
    ] {
        assert_join(&c, rhs);
    }
}

#[test]
fn cast_rhs_affinity_keeps_numeric_text_matches() {
    let c = fixture();
    for rhs in [
        "(SELECT CAST('42' AS INTEGER) AS k)",
        "(SELECT CAST(42 AS TEXT) AS k)",
        "(SELECT CAST('42.5' AS REAL) AS k)",
        "(SELECT CAST('3.0e+5' AS NUMERIC) AS k)",
    ] {
        assert_join(&c, rhs);
    }
}

#[test]
fn invalid_utf8_rhs_keeps_sqlite_residual_comparisons() {
    let c = fixture();
    for rhs in ["CAST(x'80' AS TEXT)", "CAST(x'343280' AS TEXT)"] {
        for op in ["=", ">", ">=", "<", "<="] {
            for predicate in [format!("g {op} {rhs}"), format!("{rhs} {op} g")] {
                for tail in [predicate.clone(), format!("{predicate} AND g >= 0")] {
                    assert_eq!(
                        rows(&c, &format!("SELECT typeof(g),g,n FROM roll WHERE {tail}")),
                        rows(
                            &c,
                            &format!("SELECT typeof(g),g,n FROM expected WHERE {tail}")
                        ),
                        "predicate {tail}",
                    );
                }
            }
        }
    }
}

#[test]
fn expressions_of_keys_keep_the_full_scan_plan() {
    let c = fixture();
    for expression in ["+g", "CAST(g AS NUMERIC)", "CAST(g AS TEXT)"] {
        for op in ["=", ">", ">=", "<", "<="] {
            for rhs in ["42", "42.5", "'42'"] {
                let tail = format!("WHERE {expression} {op} {rhs}");
                let sql = format!("SELECT typeof(g),g,n FROM roll {tail}");
                assert_eq!(
                    rows(&c, &sql),
                    rows(&c, &format!("SELECT typeof(g),g,n FROM expected {tail}")),
                    "{tail}",
                );
                let plan = rows(&c, &format!("EXPLAIN QUERY PLAN {sql}"));
                assert!(
                    plan.iter().any(|row| matches!(&row[3], Value::Text(detail) if detail.contains("VIRTUAL TABLE INDEX 0:"))),
                    "expression must stay residual: {sql}, plan {plan:?}",
                );
            }
        }
    }
}

#[test]
fn ordinary_join_keeps_sqlites_default_virtual_row_estimate() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE facts(g); INSERT INTO facts VALUES (42);
         CREATE TABLE rhs(k INTEGER);
         WITH RECURSIVE seq(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM seq WHERE x<999)
         INSERT INTO rhs SELECT x FROM seq;
         ANALYZE;",
    )
    .unwrap();
    create(&c, "roll", "SELECT g,COUNT(*) AS n FROM facts GROUP BY g").unwrap();
    let trace = ivmlite_test::read_work::ReadTrace::start(&c).unwrap();
    assert_eq!(
        rows(&c, "SELECT v.g,v.n FROM roll v JOIN rhs r ON v.g=r.k"),
        vec![vec![Value::Integer(42), Value::Integer(1)]],
    );
    assert_eq!(trace.finish().unwrap().len(), 1);

    // An equality join can hide a bad row estimate behind an automatic
    // index. This ordinary Cartesian join lets SQLite choose either loop
    // order and exposes repeated snapshots of the one-group virtual table.
    let trace = ivmlite_test::read_work::ReadTrace::start(&c).unwrap();
    assert_eq!(
        rows(&c, "SELECT SUM(v.n) FROM roll v JOIN rhs r"),
        vec![vec![Value::Integer(1000)]],
    );
    assert_eq!(
        trace.finish().unwrap().len(),
        1,
        "ordinary Cartesian join must snapshot the virtual table once",
    );
}

// Encode REAL bits as well as storage classes. This keeps the independent
// ordinary GROUP BY oracle sensitive to typed values and signed zero.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ExactValue {
    Null,
    Integer(i64),
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}

fn exact_values(row: &rusqlite::Row<'_>, columns: usize) -> rusqlite::Result<Vec<ExactValue>> {
    (0..columns)
        .map(|column| {
            Ok(match row.get::<_, Value>(column)? {
                Value::Null => ExactValue::Null,
                Value::Integer(value) => ExactValue::Integer(value),
                Value::Real(value) => ExactValue::Real(value.to_bits()),
                Value::Text(value) => ExactValue::Text(value),
                Value::Blob(value) => ExactValue::Blob(value),
            })
        })
        .collect()
}

fn exact_rows(c: &Connection, sql: &str, binds: &[Value]) -> Vec<Vec<ExactValue>> {
    let mut statement = c.prepare(sql).unwrap();
    let columns = statement.column_count();
    let mut result = statement
        .query_map(rusqlite::params_from_iter(binds), |row| {
            exact_values(row, columns)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    result.sort();
    result
}

fn assert_exact_oracle(c: &Connection, tail: &str, binds: &[Value]) {
    assert_eq!(
        exact_rows(c, &format!("SELECT * FROM roll {tail}"), binds),
        exact_rows(c, &format!("SELECT * FROM expected {tail}"), binds),
        "{tail}, binds={binds:?}",
    );
}

#[test]
fn multikey_output_prefix_quoted_names_and_rebinding_match_base_group_by() {
    let c = open_with_extension(None).unwrap();
    // Input group-key positions are 3 and 1; output positions are 0 and 1.
    c.execute_batch(
        "CREATE TABLE facts(amount REAL, \"time\"\"key\", id INTEGER PRIMARY KEY, \"monitor id\");
         INSERT INTO facts VALUES
           (0.5,10,1,42),(0.25,10,2,42),(1.5,20,3,42),
           (2.0,10,4,'42'),(2.5,20,5,'042'),(4.0,NULL,6,NULL),
           (8.0,x'',7,x'3432'),(16.0,'a',8,'a'),(32.0,'a ',9,'A');
         CREATE TABLE expected(\"monitor id\",\"time\"\"key\",n,s);
         INSERT INTO expected SELECT \"monitor id\",\"time\"\"key\",COUNT(*),SUM(amount)
           FROM facts GROUP BY \"monitor id\",\"time\"\"key\";",
    )
    .unwrap();
    create(&c, "roll", "SELECT \"monitor id\",\"time\"\"key\",COUNT(*) AS n,SUM(amount) AS s FROM facts GROUP BY \"monitor id\",\"time\"\"key\"").unwrap();
    let sql = "SELECT * FROM {table} WHERE \"monitor id\" = ?1 AND \"time\"\"key\" >= ?2 AND \"time\"\"key\" < ?3";
    let mut actual = c.prepare(&sql.replace("{table}", "roll")).unwrap();
    let mut expected = c.prepare(&sql.replace("{table}", "expected")).unwrap();
    for binds in [
        vec![Value::Integer(42), Value::Integer(10), Value::Integer(21)],
        vec![
            Value::Text("42".into()),
            Value::Integer(10),
            Value::Integer(20),
        ],
        vec![Value::Null, Value::Integer(0), Value::Integer(30)],
        vec![
            Value::Blob(b"42".to_vec()),
            Value::Blob(vec![]),
            Value::Blob(vec![255]),
        ],
        vec![Value::Integer(42), Value::Integer(20), Value::Integer(21)],
    ] {
        let collect = |statement: &mut rusqlite::Statement<'_>| {
            let mut values = statement
                .query_map(rusqlite::params_from_iter(&binds), |r| exact_values(r, 4))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            values.sort();
            values
        };
        assert_eq!(collect(&mut actual), collect(&mut expected), "{binds:?}");
    }
    for op in ["=", ">", ">=", "<", "<="] {
        for key in ["\"monitor id\"", "\"time\"\"key\""] {
            assert_exact_oracle(
                &c,
                &format!("WHERE {key} {op} ?1 AND n >= 1"),
                &[Value::Integer(20)],
            );
        }
    }
    for predicate in [
        "\"monitor id\" = ?1 AND \"monitor id\" = ?2",
        "\"monitor id\" > ?1 AND \"monitor id\" <= ?1 AND ?2 IS NOT NULL",
    ] {
        assert_exact_oracle(
            &c,
            &format!("WHERE {predicate}"),
            &[Value::Integer(42), Value::Text("42".into())],
        );
    }
    let trace = ivmlite_test::read_work::ReadTrace::start(&c).unwrap();
    assert_exact_oracle(
        &c,
        "WHERE \"monitor id\" = 42 AND \"time\"\"key\" >= 10 AND n >= 1",
        &[],
    );
    let captured = trace.finish().unwrap();
    assert_eq!(captured.len(), 1);
    for branch in captured[0].sql.split(" UNION ALL ") {
        let predicate = branch.split_once(" WHERE ").unwrap().1;
        assert!(predicate.contains("\"monitor id\"") && predicate.contains("\"time\"\"key\""));
        assert!(!predicate.contains("\"n\"") && !predicate.contains("\"s\""));
    }
    create(&c, "\"r\"\"oll\"", "SELECT \"monitor id\",\"time\"\"key\",COUNT(*) AS n,SUM(amount) AS s FROM facts GROUP BY \"monitor id\",\"time\"\"key\"").unwrap();
    assert_eq!(
        exact_rows(
            &c,
            "SELECT * FROM \"r\"\"oll\" WHERE \"monitor id\"=42 AND \"time\"\"key\">=10",
            &[]
        ),
        exact_rows(
            &c,
            "SELECT * FROM expected WHERE \"monitor id\"=42 AND \"time\"\"key\">=10",
            &[]
        )
    );
}

unsafe extern "C" fn reverse_nocase(
    _context: *mut std::ffi::c_void,
    left_len: i32,
    left: *const std::ffi::c_void,
    right_len: i32,
    right: *const std::ffi::c_void,
) -> i32 {
    // SQLite passes valid byte slices for the duration of this callback.
    let left = unsafe { std::slice::from_raw_parts(left.cast::<u8>(), left_len as usize) };
    let right = unsafe { std::slice::from_raw_parts(right.cast::<u8>(), right_len as usize) };
    match right
        .iter()
        .map(u8::to_ascii_lowercase)
        .cmp(left.iter().map(u8::to_ascii_lowercase))
    {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[test]
fn collations_remain_residual_and_unsupported_terms_do_not_enter_nested_sql() {
    use ivmlite_test::read_work::ReadTrace;
    let c = fixture();
    let rc = unsafe {
        rusqlite::ffi::sqlite3_create_collation_v2(
            c.handle(),
            c"reverse_nocase".as_ptr(),
            rusqlite::ffi::SQLITE_UTF8,
            std::ptr::null_mut(),
            Some(reverse_nocase),
            None,
        )
    };
    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
    for collation in ["BINARY", "NOCASE", "RTRIM", "reverse_nocase"] {
        for op in ["=", ">", ">=", "<", "<="] {
            let tail = format!("WHERE g COLLATE {collation} {op} 'a'");
            let trace = ReadTrace::start(&c).unwrap();
            assert_exact_oracle(&c, &tail, &[]);
            let captured = trace.finish().unwrap();
            assert_eq!(captured.len(), 1);
            assert_eq!(
                captured[0].sql.contains(" WHERE "),
                collation == "BINARY",
                "{tail}: {captured:?}"
            );
        }
    }
    c.execute_batch(
        "CREATE TABLE rowids(captured_rowid,g,n); INSERT INTO rowids SELECT rowid,g,n FROM roll",
    )
    .unwrap();
    for predicate in [
        "n > 0",
        "g IS NULL",
        "g IS 42",
        "g LIKE 'a%'",
        "roll IS NULL",
        "rowid > 2",
    ] {
        let expected_predicate = predicate
            .replace("roll IS NULL", "NULL IS NULL")
            .replace("rowid", "captured_rowid");
        let oracle = if predicate.contains("rowid") {
            "rowids"
        } else {
            "expected"
        };
        let trace = ReadTrace::start(&c).unwrap();
        assert_eq!(
            exact_rows(&c, &format!("SELECT g,n FROM roll WHERE {predicate}"), &[]),
            exact_rows(
                &c,
                &format!("SELECT g,n FROM {oracle} WHERE {expected_predicate}"),
                &[]
            )
        );
        let captured = trace.finish().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(
            !captured[0].sql.contains(" WHERE "),
            "{predicate}: {captured:?}"
        );
    }
    // Keeping the virtual table outermost makes its RHS join term unusable
    // at xBestIndex time; it must collect one unfiltered snapshot.
    let trace = ReadTrace::start(&c).unwrap();
    assert_eq!(
        exact_rows(
            &c,
            "SELECT v.g,v.n FROM roll v CROSS JOIN ints r WHERE v.g=r.k",
            &[]
        ),
        exact_rows(
            &c,
            "SELECT v.g,v.n FROM expected v CROSS JOIN ints r WHERE v.g=r.k",
            &[]
        )
    );
    let captured = trace.finish().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(!captured[0].sql.contains(" WHERE "));
}

#[test]
fn in_or_and_order_limit_match_independent_typeless_materialization() {
    let c = fixture();
    for tail in [
        "WHERE g IN (42,42,NULL,'42',x'3432')",
        "WHERE g IN ()",
        "WHERE g IN (SELECT k FROM ints UNION ALL SELECT NULL UNION ALL SELECT 42)",
        "WHERE g = 42 OR g >= 'a'",
        "WHERE n > 0 ORDER BY g,n LIMIT 2 OFFSET 1",
        "WHERE g >= 42 ORDER BY g DESC,n LIMIT 3",
    ] {
        assert_exact_oracle(&c, tail, &[]);
    }
    for binds in [
        vec![Value::Integer(42), Value::Integer(42), Value::Null],
        vec![
            Value::Text("a".into()),
            Value::Text("A".into()),
            Value::Text("a ".into()),
        ],
        vec![
            Value::Blob(vec![]),
            Value::Null,
            Value::Blob(b"42".to_vec()),
        ],
    ] {
        assert_exact_oracle(&c, "WHERE g IN (?1,?2,?3,NULL)", &binds);
    }
}

#[test]
fn selective_refresh_rekey_rollback_savepoint_reopen_and_overlapping_snapshots() {
    use common::{durable_state, refresh, TempFile};
    let file = TempFile::new("selective-lifecycle");
    let query = "SELECT g,h,COUNT(*) AS n,SUM(amount) AS s FROM facts GROUP BY g,h";
    let rematerialize = |c: &Connection| {
        c.execute_batch("DELETE FROM expected; INSERT INTO expected SELECT g,h,COUNT(*),SUM(amount) FROM facts GROUP BY g,h").unwrap();
    };
    let check = |c: &Connection| {
        for tail in [
            "WHERE g >= 1",
            "WHERE g = 2 AND h >= 10 AND h < 30",
            "WHERE h > 10",
            "WHERE g >= 1 AND n > 0",
        ] {
            assert_exact_oracle(c, tail, &[]);
        }
    };
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE facts(id INTEGER PRIMARY KEY,g,h,amount REAL); INSERT INTO facts VALUES (1,1,10,0.5),(2,2,10,0.25),(3,2,20,1.5),(4,3,30,2.0); CREATE TABLE expected(g,h,n,s)").unwrap();
        create(&c, "roll", query).unwrap();
        rematerialize(&c);
        check(&c);
        let original = durable_state(&c, "roll");
        c.execute_batch("INSERT INTO facts VALUES (5,4,40,4.0); DELETE FROM facts WHERE id=1; UPDATE facts SET g=5,h=50 WHERE id=3").unwrap();
        check(&c); // Reads preserve the old explicitly maintained output.
        assert_eq!(durable_state(&c, "roll"), original);
        refresh(&c, "roll").unwrap();
        rematerialize(&c);
        check(&c);
        for (begin, rollback) in [
            ("BEGIN", "ROLLBACK"),
            ("SAVEPOINT s", "ROLLBACK TO s; RELEASE s"),
        ] {
            c.execute_batch(begin).unwrap();
            c.execute_batch("UPDATE facts SET g=9; DELETE FROM facts WHERE id=5")
                .unwrap();
            refresh(&c, "roll").unwrap();
            rematerialize(&c);
            check(&c);
            c.execute_batch(rollback).unwrap();
            check(&c);
        }
        let before = exact_rows(&c, "SELECT g,h,n,s FROM roll WHERE g>=2", &[]);
        let mut first = c.prepare("SELECT g,h,n,s FROM roll WHERE g>=2").unwrap();
        let mut second = c.prepare("SELECT g,h,n,s FROM roll WHERE h>=10").unwrap();
        let mut first_rows = first.query([]).unwrap();
        let mut second_rows = second.query([]).unwrap();
        let mut retained_first =
            vec![exact_values(first_rows.next().unwrap().unwrap(), 4).unwrap()];
        let mut retained_second =
            vec![exact_values(second_rows.next().unwrap().unwrap(), 4).unwrap()];
        c.execute_batch(
            "UPDATE facts SET amount=amount+0.5; INSERT INTO facts VALUES (6,6,60,8.0)",
        )
        .unwrap();
        refresh(&c, "roll").unwrap();
        while let Some(row) = first_rows.next().unwrap() {
            retained_first.push(exact_values(row, 4).unwrap());
        }
        while let Some(row) = second_rows.next().unwrap() {
            retained_second.push(exact_values(row, 4).unwrap());
        }
        retained_first.sort();
        retained_second.sort();
        assert_eq!(retained_first, before);
        assert_eq!(retained_second, before);
        rematerialize(&c);
        check(&c);
        assert_ne!(
            exact_rows(&c, "SELECT g,h,n,s FROM roll WHERE g>=2", &[]),
            before
        );
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    check(&c);
    c.execute_batch("DROP TABLE roll").unwrap();
}

#[test]
fn selective_broken_view_keeps_existing_diagnostic_and_can_drop() {
    use common::TempFile;
    let file = TempFile::new("selective-broken");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE facts(g); INSERT INTO facts VALUES (42)")
            .unwrap();
        create(&c, "roll", "SELECT g,COUNT(*) AS n FROM facts GROUP BY g").unwrap();
    }
    Connection::open(file.path())
        .unwrap()
        .execute_batch("UPDATE __ivm_view SET plan='invalid' WHERE name='roll'")
        .unwrap();
    let c = open_with_extension(Some(file.path())).unwrap();
    let full = c
        .execute_batch("SELECT * FROM roll")
        .unwrap_err()
        .to_string();
    let filtered = c
        .execute_batch("SELECT * FROM roll WHERE g=42")
        .unwrap_err()
        .to_string();
    assert_eq!(filtered, full);
    assert!(full.contains("cannot be maintained"), "{full}");
    c.execute_batch("DROP TABLE roll").unwrap();
}

#[test]
fn actual_nested_work_distinguishes_seek_scan_and_candidate_materialization() {
    use ivmlite_test::read_work::{ReadTrace, ReadWork};
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE facts(g,h,status,latency REAL); WITH RECURSIVE seq(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM seq WHERE x<19999) INSERT INTO facts SELECT x,x%100,x%3,0.5 FROM seq").unwrap();
    create(
        &c,
        "roll",
        "SELECT g,h,status,AVG(latency) AS a,MIN(latency) AS lo,MAX(latency) AS hi,COUNT(*) AS n FROM facts GROUP BY g,h,status",
    )
    .unwrap();
    let measure = |tail: &str, binds: &[Value], expected_output: usize| -> ReadWork {
        let trace = ReadTrace::start(&c).unwrap();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let progress = Arc::clone(&callbacks);
        c.progress_handler(
            100,
            Some(move || {
                progress.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
        let output = exact_rows(&c, &format!("SELECT * FROM roll {tail}"), binds);
        assert_eq!(output.len(), expected_output, "{tail}: outer output");
        c.progress_handler(0, None::<fn() -> bool>).unwrap();
        let captured = trace.finish().unwrap();
        assert_eq!(captured.len(), 1, "{tail}: {captured:?}");
        let read = captured.into_iter().next().unwrap();
        let eqp = rows(&c, &format!("EXPLAIN QUERY PLAN {}", read.expanded_sql));
        println!("nested work {tail}: {}; whole_query_progress_callbacks={}; output_rows={}; EQP={eqp:?}",serde_json::to_string(&read).unwrap(),callbacks.load(Ordering::Relaxed),output.len());
        assert!(!read.sql.contains("ORDER BY") && !read.sql.contains("LIMIT"));
        assert!(read.vm_steps > 0);
        read
    };
    let full = measure("", &[], 20000);
    assert_eq!(full.rows_returned, 20000);
    assert!(full.fullscan_steps >= 19999);
    let equality = measure("WHERE g=?1", &[Value::Integer(12345)], 1);
    let range = measure(
        "WHERE g=?1 AND h>=?2 AND h<?3",
        &[
            Value::Integer(12345),
            Value::Integer(40),
            Value::Integer(50),
        ],
        1,
    );
    let real = measure("WHERE g=?1", &[Value::Real(12345.0)], 1);
    for read in [&equality, &range, &real] {
        assert_eq!(read.rows_returned, 1);
        // Require both candidate reduction and actual bounded work. A plain
        // OR candidate can materialize one row while still scanning all keys.
        assert!(read.fullscan_steps < 100, "{read:?}");
        assert!(read.vm_steps < full.vm_steps / 10, "{read:?}");
        assert!(read.sql.contains("?1") && !read.sql.contains("12345"));
        assert!(read.expanded_sql.contains("12345"));
    }
    let leading_range = measure("WHERE g>=?1", &[Value::Integer(19990)], 10);
    assert_eq!(leading_range.rows_returned, 10);
    assert!(leading_range.fullscan_steps < 100, "{leading_range:?}");
    assert!(
        leading_range.vm_steps < full.vm_steps / 10,
        "{leading_range:?}"
    );
    for (op, expected) in [("<", 10), ("<=", 11)] {
        let upper_bound = measure(&format!("WHERE g{op}?1"), &[Value::Integer(10)], expected);
        assert_eq!(upper_bound.rows_returned, expected);
        assert!(upper_bound.fullscan_steps < 100, "{upper_bound:?}");
        assert!(upper_bound.vm_steps < full.vm_steps / 10, "{upper_bound:?}");
    }
    let nonleading = measure(
        "WHERE h>=?1 AND h<?2",
        &[Value::Integer(40), Value::Integer(50)],
        2000,
    );
    assert_eq!(nonleading.rows_returned, 2000);
    assert!(
        nonleading.fullscan_steps >= 19999,
        "nonleading must honestly report scan: {nonleading:?}"
    );
    let text = measure("WHERE g=?1", &[Value::Text("12345".into())], 0);
    assert_eq!(text.rows_returned, 1); // An outer BLOB-affinity comparison rejects this candidate.
    assert!(
        text.fullscan_steps >= 19999,
        "TEXT class broadening scan: {text:?}"
    );
}

// A deterministic foreign virtual table separates declared affinity from
// returned storage class. Ordinary tables would coerce these values on insert;
// compound SELECT affinity is intentionally not guaranteed by SQLite.
mod foreign_rhs {
    use rusqlite::{ffi, Connection};
    use std::ffi::{c_char, c_int, c_void};

    fn guard(action: impl FnOnce() -> c_int) -> c_int {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or(ffi::SQLITE_ERROR)
    }

    #[repr(C)]
    struct Table {
        base: ffi::sqlite3_vtab,
        text_declaration: bool,
    }
    #[repr(C)]
    struct Cursor {
        base: ffi::sqlite3_vtab_cursor,
        done: bool,
    }
    static NUMERIC: bool = false;
    static TEXT: bool = true;
    // SAFETY: optional callbacks are represented by nullable function pointers.
    // SQLite retains this static module for the connection's lifetime.
    static MODULE: ffi::sqlite3_module = ffi::sqlite3_module {
        iVersion: 1,
        xConnect: Some(connect),
        xBestIndex: Some(best_index),
        xDisconnect: Some(disconnect),
        xOpen: Some(open),
        xClose: Some(close),
        xFilter: Some(filter),
        xNext: Some(next),
        xEof: Some(eof),
        xColumn: Some(column),
        xRowid: Some(rowid),
        ..unsafe { std::mem::zeroed() }
    };

    pub fn register(c: &Connection) {
        for (name, declaration) in [
            (c"foreign_numeric_rhs", &NUMERIC),
            (c"foreign_text_rhs", &TEXT),
        ] {
            let rc = unsafe {
                ffi::sqlite3_create_module_v2(
                    c.handle(),
                    name.as_ptr(),
                    &MODULE,
                    (declaration as *const bool).cast_mut().cast(),
                    None,
                )
            };
            assert_eq!(rc, ffi::SQLITE_OK);
        }
        assert_eq!(
            super::rows(c, "SELECT typeof(k),k FROM foreign_numeric_rhs"),
            vec![vec![
                super::Value::Text("text".into()),
                super::Value::Text("42".into())
            ]]
        );
        assert_eq!(
            super::rows(c, "SELECT typeof(k),k FROM foreign_text_rhs"),
            vec![vec![
                super::Value::Text("real".into()),
                super::Value::Real(42.0)
            ]]
        );
    }

    unsafe extern "C" fn connect(
        db: *mut ffi::sqlite3,
        aux: *mut c_void,
        _argc: c_int,
        _argv: *const *const c_char,
        out: *mut *mut ffi::sqlite3_vtab,
        _error: *mut *mut c_char,
    ) -> c_int {
        guard(|| {
            let text_declaration = unsafe { *aux.cast::<bool>() };
            let declaration = if text_declaration {
                c"CREATE TABLE x(k TEXT)"
            } else {
                c"CREATE TABLE x(k NUMERIC)"
            };
            let rc = unsafe { ffi::sqlite3_declare_vtab(db, declaration.as_ptr()) };
            if rc != ffi::SQLITE_OK {
                return rc;
            }
            let table = Box::new(Table {
                base: unsafe { std::mem::zeroed() },
                text_declaration,
            });
            unsafe { *out = Box::into_raw(table).cast() };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn best_index(
        _table: *mut ffi::sqlite3_vtab,
        _info: *mut ffi::sqlite3_index_info,
    ) -> c_int {
        guard(|| ffi::SQLITE_OK)
    }
    unsafe extern "C" fn disconnect(table: *mut ffi::sqlite3_vtab) -> c_int {
        guard(|| {
            unsafe { drop(Box::from_raw(table.cast::<Table>())) };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn open(
        table: *mut ffi::sqlite3_vtab,
        out: *mut *mut ffi::sqlite3_vtab_cursor,
    ) -> c_int {
        guard(|| {
            let cursor = Box::new(Cursor {
                base: ffi::sqlite3_vtab_cursor { pVtab: table },
                done: false,
            });
            unsafe { *out = Box::into_raw(cursor).cast() };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn close(cursor: *mut ffi::sqlite3_vtab_cursor) -> c_int {
        guard(|| {
            unsafe { drop(Box::from_raw(cursor.cast::<Cursor>())) };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn filter(
        cursor: *mut ffi::sqlite3_vtab_cursor,
        _idx: c_int,
        _text: *const c_char,
        _argc: c_int,
        _argv: *mut *mut ffi::sqlite3_value,
    ) -> c_int {
        guard(|| {
            unsafe { (*cursor.cast::<Cursor>()).done = false };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn next(cursor: *mut ffi::sqlite3_vtab_cursor) -> c_int {
        guard(|| {
            unsafe { (*cursor.cast::<Cursor>()).done = true };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn eof(cursor: *mut ffi::sqlite3_vtab_cursor) -> c_int {
        guard(|| unsafe { (*cursor.cast::<Cursor>()).done as c_int })
    }
    unsafe extern "C" fn column(
        cursor: *mut ffi::sqlite3_vtab_cursor,
        context: *mut ffi::sqlite3_context,
        _column: c_int,
    ) -> c_int {
        guard(|| {
            let table = unsafe { &*(*cursor).pVtab.cast::<Table>() };
            if table.text_declaration {
                unsafe { ffi::sqlite3_result_double(context, 42.0) }
            } else {
                unsafe {
                    ffi::sqlite3_result_text(context, c"42".as_ptr(), 2, ffi::SQLITE_STATIC())
                }
            };
            ffi::SQLITE_OK
        })
    }
    unsafe extern "C" fn rowid(_cursor: *mut ffi::sqlite3_vtab_cursor, out: *mut i64) -> c_int {
        guard(|| {
            unsafe { *out = 1 };
            ffi::SQLITE_OK
        })
    }
}
