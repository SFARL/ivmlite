//! M2c spec §6: named intermediate REAL overflow cases outside the normal
//! differential domain. Their exact results follow algebraically; evaluate
//! cannot be used because fsum's intermediate partials must stay finite.

use std::collections::BTreeMap;

use ivmlite_core::{Row, Value, ZSet};
use ivmlite_test::{
    compare_with_oracles, view_query_to_sql, Agg, AggFn, Column, ColumnType, Database, Engine,
    Evaluated, Predicate, Schema, SqliteExtensionEngine, ViewQuery,
};

fn check(terms: &[f64], exact_sum: f64) -> ZSet {
    assert!(terms.iter().all(|x| x.is_finite()));
    let database = Database::single(Schema {
        table: "t".into(),
        columns: vec![
            Column {
                name: "k".into(),
                ty: ColumnType::Text,
                nullable: false,
            },
            Column {
                name: "x".into(),
                ty: ColumnType::Real,
                nullable: false,
            },
        ],
    });
    let query = ViewQuery {
        group_by: vec![0],
        aggs: [AggFn::Sum, AggFn::Total, AggFn::Avg]
            .into_iter()
            .map(|func| Agg {
                func,
                column: Some(1),
            })
            .collect(),
        predicate: Predicate::None,
        join: None,
    };
    let key = Row::new(vec![Value::Text("a".into())]);
    let bases = BTreeMap::from([(
        "t".into(),
        ZSet::from_rows(
            terms
                .iter()
                .map(|&x| (Row::new(vec![Value::Text("a".into()), Value::Real(x)]), 1)),
        ),
    )]);
    let expected = Evaluated {
        rows: ZSet::from_rows([(
            Row::new(vec![
                Value::Text("a".into()),
                Value::Real(exact_sum),
                Value::Real(exact_sum),
                Value::Real(exact_sum / terms.len() as f64),
            ]),
            1,
        )]),
        // The exact magnitude exceeds MAX even though the final sum is finite.
        magnitudes: (0..3)
            .map(|i| ((key.clone(), i), (f64::INFINITY, terms.len() as i64)))
            .collect(),
    };
    let mut engine = SqliteExtensionEngine::new();
    engine.create_view(&database, &query, &bases).unwrap();
    let got = engine.materialize().unwrap();
    // Preserve the order that overflows SQLite's running accumulator. The
    // generic oracle expands a sorted Z-set, which can cancel -MAX first.
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x REAL)").unwrap();
    for x in terms {
        c.execute("INSERT INTO t VALUES ('a', ?1)", [x]).unwrap();
    }
    let mut statement = c.prepare(&view_query_to_sql(&query, &database)).unwrap();
    let rows = statement
        .query_map([], |r| {
            let mut values = vec![Value::Text(r.get(0)?)];
            for i in 1..4 {
                values.push(r.get::<_, Option<f64>>(i)?.map_or(Value::Null, Value::Real));
            }
            Ok((Row::new(values), 1))
        })
        .unwrap();
    let sqlite = ZSet::from_rows(rows.map(Result::unwrap));
    compare_with_oracles(&got, &expected, Some(&sqlite), &query).unwrap();
    sqlite
}

#[test]
fn sqlite_running_real_overflow_uses_an_explicit_finite_reference() {
    // ±(MAX + MAX - MAX) = ±MAX, with no input infinity.
    for sign in [1.0, -1.0] {
        let max = sign * f64::MAX;
        let sqlite = check(&[max, max, -max], max);
        assert!(sqlite.iter().all(|(row, _)| row.0[1..].iter().all(
            |value| matches!(value, Value::Real(x) if x.is_infinite()) || *value == Value::Null
        )));
    }
}

#[test]
fn sqlite_running_real_cancellation_uses_an_explicit_zero_reference() {
    let sqlite = check(&[f64::MAX, f64::MAX, -f64::MAX, -f64::MAX], 0.0);
    // SQLite 3.53 ignores its non-finite correction and returns infinity.
    // Versions that add the correction can instead return NULL (NaN).
    assert!(
        sqlite.iter().all(|(row, _)| row.0[1..].iter().all(
            |value| matches!(value, Value::Real(x) if x.is_infinite()) || *value == Value::Null
        )),
        "{sqlite:?}"
    );
}
