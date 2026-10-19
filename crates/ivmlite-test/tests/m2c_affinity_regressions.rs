//! M2c spec §5: explicit affinity cases outside the bounded enumeration.

mod common;

use std::collections::BTreeMap;

use ivmlite_core::{Row, Value};
use ivmlite_test::{
    open_with_extension, run, Agg, AggFn, Batching, CmpOp, Column, ColumnType, Database,
    IncrementalEngine, Join, NaiveRecompute, Op, Predicate, Schema, SqliteExtensionEngine,
    TestCase, ViewQuery,
};

#[test]
fn numeric_affinity_filters_keep_unparseable_text_literals() {
    for ty in [ColumnType::Integer, ColumnType::Real, ColumnType::Numeric] {
        let database = Database::single(Schema {
            table: "t".into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty,
                    nullable: true,
                },
            ],
        });
        let one = if ty == ColumnType::Real {
            Value::Real(1.0)
        } else {
            Value::Int(1)
        };
        let values = [
            one,
            Value::Real(2.5),
            Value::Text("abc".into()),
            Value::Text("z".into()),
            Value::Blob(vec![1]),
            Value::Null,
        ];
        let rows: Vec<_> = values
            .into_iter()
            .map(|value| Row::new(vec![Value::Text("a".into()), value]))
            .collect();
        for literal in ["x", "2.5"] {
            for op in [CmpOp::Gt, CmpOp::Lt] {
                let case = TestCase {
                    seed: 32,
                    database: database.clone(),
                    query: ViewQuery {
                        group_by: vec![0],
                        aggs: vec![Agg {
                            func: AggFn::Count,
                            column: None,
                        }],
                        predicate: Predicate::Compare {
                            column: 1,
                            op,
                            value: Value::Text(literal.into()),
                        },
                        join: None,
                    },
                    initial: BTreeMap::from([("t".into(), rows.clone())]),
                    ops: rows
                        .iter()
                        .cloned()
                        .flat_map(|row| {
                            [
                                ("t".into(), Op::Delete(row.clone())),
                                ("t".into(), Op::Insert(row)),
                            ]
                        })
                        .collect(),
                    batching: Batching::One,
                };
                run(&mut NaiveRecompute::new(), &case).unwrap();
                run(&mut IncrementalEngine::new(), &case).unwrap();
                run(&mut SqliteExtensionEngine::new(), &case).unwrap();
            }
        }
    }
}

#[test]
fn integer_and_numeric_joins_match_text_and_mixed_numeric_writes() {
    for ty in ["INTEGER", "NUMERIC"] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(&format!(
            "CREATE TABLE l(k {ty}); CREATE TABLE r(k {ty});
             INSERT INTO l VALUES ('abc'), (1), (1.0);
             INSERT INTO r VALUES ('abc'), (1), (1.0);"
        ))
        .unwrap();
        let sql = "SELECT l.k, COUNT(*) FROM l JOIN r ON l.k = r.k GROUP BY l.k";
        common::create(&c, "v", sql).unwrap();
        common::assert_matches_oracle(&c, "v", sql);
        assert_eq!(
            common::rows(&c, "SELECT \"COUNT(*)\" FROM v WHERE k = 1"),
            vec![vec![rusqlite::types::Value::Integer(4)]]
        );
        assert_eq!(
            common::rows(&c, "SELECT \"COUNT(*)\" FROM v WHERE k = 'abc'"),
            vec![vec![rusqlite::types::Value::Integer(1)]]
        );
        assert_eq!(
            common::rows(&c, "SELECT COUNT(*) FROM v"),
            vec![vec![rusqlite::types::Value::Integer(2)]]
        );
        assert_eq!(
            common::rows(&c, "SELECT COUNT(*) FROM l WHERE typeof(k) = 'integer'"),
            vec![vec![rusqlite::types::Value::Integer(2)]]
        );
        // Replay through the independent reference as well. SQLite has
        // normalized both numeric writes (1 and 1.0) to stored Int(1), as
        // the typeof assertion above verifies for these two affinities.
        let column_type = if ty == "INTEGER" {
            ColumnType::Integer
        } else {
            ColumnType::Numeric
        };
        let stored_rows = vec![
            Row::new(vec![Value::Text("abc".into())]),
            Row::new(vec![Value::Int(1)]),
            Row::new(vec![Value::Int(1)]),
        ];
        let case = TestCase {
            seed: 32,
            database: Database::new(
                ["l", "r"]
                    .into_iter()
                    .map(|table| Schema {
                        table: table.into(),
                        columns: vec![Column {
                            name: "k".into(),
                            ty: column_type,
                            nullable: true,
                        }],
                    })
                    .collect(),
            ),
            query: ViewQuery {
                group_by: vec![0],
                aggs: vec![Agg {
                    func: AggFn::Count,
                    column: None,
                }],
                predicate: Predicate::None,
                join: Some(Join {
                    right: "r".into(),
                    left_column: 0,
                    right_column: 0,
                }),
            },
            initial: BTreeMap::from([("l".into(), stored_rows.clone()), ("r".into(), stored_rows)]),
            ops: vec![
                ("l".into(), Op::Delete(Row::new(vec![Value::Int(1)]))),
                (
                    "r".into(),
                    Op::Insert(Row::new(vec![Value::Text("abc".into())])),
                ),
            ],
            batching: Batching::One,
        };
        run(&mut NaiveRecompute::new(), &case).unwrap();
        run(&mut IncrementalEngine::new(), &case).unwrap();
        run(&mut SqliteExtensionEngine::new(), &case).unwrap();
        c.execute_batch("DELETE FROM l WHERE rowid = 2; INSERT INTO r VALUES ('abc');")
            .unwrap();
        common::refresh(&c, "v").unwrap();
        common::assert_matches_oracle(&c, "v", sql);
    }
}
