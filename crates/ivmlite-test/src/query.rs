use crate::{Agg, AggFn, CmpOp, ColumnType, Predicate, Schema, ViewQuery};
use ivmlite_core::{Database, Join, Value};

/// Every aggregate function, each enumerated over every column (M2c spec §6).
pub const AGGREGATE_FUNCTIONS: [AggFn; 6] = [
    AggFn::Sum,
    AggFn::Total,
    AggFn::Avg,
    AggFn::Count,
    AggFn::Min,
    AggFn::Max,
];

/// Enumerate the single-table query space.
///
/// Spec §9.2: the subset has finitely many combinations, so enumeration
/// beats randomness — reproducible and complete. Randomness is left to the
/// update sequences.
///
/// M2c spec §6 widens it to every aggregate, and the full cross would be
/// large, so it is restricted (parent spec §9.2 item 4, amended): every
/// aggregate set meets every group-by under no predicate, and each predicate
/// meets every group-by with two aggregate sets, `[COUNT(*)]` and one that
/// mixes SUM, MIN, AVG and `COUNT(column)`. Over a 3-column table that is
/// 6 × 37 + 6 × 2 × 24 = 510 queries.
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    let arity = schema.arity();
    let agg = |func: AggFn, column: usize| Agg {
        func,
        column: Some(column % arity),
    };
    let filtered_aggs = vec![
        vec![count_star()],
        vec![
            agg(AggFn::Sum, 0),
            agg(AggFn::Min, 1),
            agg(AggFn::Avg, 2),
            agg(AggFn::Count, 0),
        ],
    ];
    restricted_cross(
        &group_by_choices(schema),
        &agg_choices(schema),
        &filtered_aggs,
        &predicates(schema),
    )
}

/// Enumerate the two-table join space: every pair of same-affinity key
/// columns, crossed with the group-bys, aggregates and join predicates over
/// the joined row — `left`'s columns, then `right`'s.
///
/// Keys of different affinities are never generated: `lower` rejects them
/// (M2c spec §5).
///
/// The predicates are `join_predicates`, not the single-table set (M1b Phase
/// 2a, Ruling 4): one comparison and one null test per column, which reaches
/// every position of the joined row. The aggregates rotate with the column
/// (`join_agg_choices`), and the predicates meet only `[COUNT(*)]`: per key
/// pair, over a 6-column joined row, 21 × 7 + 21 × 12 = 399 queries.
pub fn enumerate_join(left: &Schema, right: &Schema) -> Vec<ViewQuery> {
    // The joined row, as a schema, so the dimension functions can run on it.
    // Its name and column names are never rendered.
    let joined = Schema {
        table: "joined".into(),
        columns: left
            .columns
            .iter()
            .chain(right.columns.iter())
            .cloned()
            .collect(),
    };
    let shapes = restricted_cross(
        &group_by_choices(&joined),
        &join_agg_choices(&joined),
        &[vec![count_star()]],
        &join_predicates(&joined),
    );
    let mut out = Vec::new();
    for (left_column, l) in left.columns.iter().enumerate() {
        for (right_column, r) in right.columns.iter().enumerate() {
            if l.ty != r.ty {
                continue;
            }
            for shape in &shapes {
                out.push(ViewQuery {
                    join: Some(Join {
                        right: right.table.clone(),
                        left_column,
                        right_column,
                    }),
                    ..shape.clone()
                });
            }
        }
    }
    out
}

/// Every query `gen_case` can pick for `db`: the anchor's single-table queries
/// and, when `db` has at least two tables, the join queries over its first
/// two, **alternating** — single, join, single, join, …, then the rest of the
/// longer list (M1b Phase 2a, Ruling 5). `gen_case` picks by `seed % len`, so
/// any run of consecutive seeds draws both kinds.
///
/// # Panics
/// If `db` declares no tables: the single-table queries are enumerated over
/// `db.tables()[0]`.
pub fn enumerate_database(db: &Database) -> Vec<ViewQuery> {
    let tables = db.tables();
    let single = enumerate(&tables[0]);
    if tables.len() < 2 {
        return single;
    }
    let joins = enumerate_join(&tables[0], &tables[1]);
    let mut out = Vec::with_capacity(single.len() + joins.len());
    let mut single = single.into_iter();
    let mut joins = joins.into_iter();
    loop {
        match (single.next(), joins.next()) {
            (None, None) => return out,
            (s, j) => out.extend(s.into_iter().chain(j)),
        }
    }
}

/// For each group-by: every aggregate set of `aggs` with no predicate, then
/// every aggregate set of `filtered_aggs` with every predicate of
/// `predicates` but `Predicate::None` (M2c spec §6).
fn restricted_cross(
    group_bys: &[Vec<usize>],
    aggs: &[Vec<Agg>],
    filtered_aggs: &[Vec<Agg>],
    predicates: &[Predicate],
) -> Vec<ViewQuery> {
    let query = |group_by: &[usize], aggs: &[Agg], predicate: &Predicate| ViewQuery {
        group_by: group_by.to_vec(),
        aggs: aggs.to_vec(),
        predicate: predicate.clone(),
        join: None,
    };
    let mut out = Vec::new();
    for group_by in group_bys {
        for aggs in aggs {
            out.push(query(group_by, aggs, &Predicate::None));
        }
        for aggs in filtered_aggs {
            for predicate in predicates.iter().filter(|p| **p != Predicate::None) {
                out.push(query(group_by, aggs, predicate));
            }
        }
    }
    out
}

fn group_by_choices(schema: &Schema) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    for i in 0..schema.arity() {
        out.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            out.push(vec![i, j]);
        }
    }
    out
}

fn count_star() -> Agg {
    Agg {
        func: AggFn::Count,
        column: None,
    }
}

/// `[COUNT(*)]`, then for each column and each function `[f(c)]` and
/// `[f(c), COUNT(*)]`.
fn agg_choices(schema: &Schema) -> Vec<Vec<Agg>> {
    let mut out = vec![vec![count_star()]];
    for column in 0..schema.arity() {
        for func in AGGREGATE_FUNCTIONS {
            let agg = Agg {
                func,
                column: Some(column),
            };
            out.push(vec![agg.clone()]);
            out.push(vec![agg, count_star()]);
        }
    }
    out
}

/// `[COUNT(*)]`, then for joined column `c`
/// `[AGGREGATE_FUNCTIONS[c % 6](c), COUNT(*)]`: over a 6-column joined row
/// every function appears once, on both sides of the table boundary.
fn join_agg_choices(joined: &Schema) -> Vec<Vec<Agg>> {
    let mut out = vec![vec![count_star()]];
    for column in 0..joined.arity() {
        let func = AGGREGATE_FUNCTIONS[column % AGGREGATE_FUNCTIONS.len()];
        out.push(vec![
            Agg {
                func,
                column: Some(column),
            },
            count_star(),
        ]);
    }
    out
}

/// The literals a comparison on a column of affinity `ty` uses, picked by
/// an index (`literals(ty)[i % len]`): values of several classes, each
/// meeting the column through SQLite's comparison affinity (M2c spec §5),
/// near the middle of the column's pool, so each operator keeps some rows
/// and drops others.
///
/// A TEXT column never meets a REAL literal, which `lower` refuses
/// (M2c spec §5, amendment 4).
fn literals(ty: ColumnType) -> Vec<Value> {
    let text = |s: &str| Value::Text(s.into());
    match ty {
        ColumnType::Integer | ColumnType::Numeric => {
            vec![Value::Int(3), Value::Real(2.5), text("3")]
        }
        ColumnType::Real => vec![Value::Real(2.0), text("2.5"), Value::Int(3)],
        ColumnType::Text => vec![text("v3"), Value::Int(12)],
        ColumnType::Blob => vec![Value::Int(2), text("v1"), Value::Blob(vec![1])],
    }
}

fn literal(ty: ColumnType, i: usize) -> Value {
    let all = literals(ty);
    all[i % all.len()].clone()
}

/// The single-table predicates: `None`, every operator on every column (the
/// literal picked by the operator's index in `CmpOp::ALL`), and `IS NULL` /
/// `IS NOT NULL` on every nullable column.
fn predicates(schema: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in schema.columns.iter().enumerate() {
        for (i, op) in CmpOp::ALL.into_iter().enumerate() {
            out.push(Predicate::Compare {
                column,
                op,
                value: literal(col.ty, i),
            });
        }
    }
    for (column, col) in schema.columns.iter().enumerate() {
        if col.nullable {
            out.push(Predicate::IsNull { column });
            out.push(Predicate::IsNotNull { column });
        }
    }
    out
}

/// The join predicates (Ruling 4): `None`, one comparison per column with the
/// operator `CmpOp::ALL[column % 6]` and the literal picked by the column,
/// and one null test per nullable column — `IS NULL` on even columns,
/// `IS NOT NULL` on odd ones.
fn join_predicates(joined: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in joined.columns.iter().enumerate() {
        out.push(Predicate::Compare {
            column,
            op: CmpOp::ALL[column % CmpOp::ALL.len()],
            value: literal(col.ty, column),
        });
    }
    for (column, col) in joined.columns.iter().enumerate() {
        if col.nullable {
            out.push(if column % 2 == 0 {
                Predicate::IsNull { column }
            } else {
                Predicate::IsNotNull { column }
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::kv;
    use crate::{Column, ColumnType, Schema};
    use ivmlite_core::CmpOp;

    #[test]
    fn enumerate_join_pairs_only_same_typed_keys() {
        let (l, r) = (kv("t0"), kv("t1"));
        let qs = enumerate_join(&l, &r);
        assert!(!qs.is_empty());
        for q in &qs {
            let j = q.join.as_ref().expect("every join query has a join");
            assert_eq!(j.right, "t1");
            assert_eq!(l.columns[j.left_column].ty, r.columns[j.right_column].ty);
        }
    }

    #[test]
    fn the_swapped_database_joins_keys_at_different_positions() {
        // Over `gen_database(2)` every key pair is (i, i); the swapped database
        // exists so the join space also holds pairs whose positions differ.
        let db = crate::gen_database_with_swapped_right_table();
        let mut pairs: Vec<(usize, usize)> = enumerate_join(&db.tables()[0], &db.tables()[1])
            .iter()
            .map(|q| {
                let j = q.join.as_ref().expect("every join query has a join");
                (j.left_column, j.right_column)
            })
            .collect();
        pairs.dedup();
        assert_eq!(pairs, vec![(0, 2), (1, 1), (2, 0)]);
        let db = crate::gen_real_numeric_database();
        let mut pairs: Vec<(usize, usize)> = enumerate_join(&db.tables()[0], &db.tables()[1])
            .iter()
            .map(|q| {
                let j = q.join.as_ref().expect("every join query has a join");
                (j.left_column, j.right_column)
            })
            .collect();
        pairs.dedup();
        assert_eq!(pairs, vec![(0, 2), (1, 0), (2, 1)]);
    }

    #[test]
    fn enumerate_join_groups_across_the_table_boundary() {
        // Spec §9.2: multi-column group keys that cross the boundary between
        // the two tables are exactly where joins are most likely to have bugs.
        let qs = enumerate_join(&kv("t0"), &kv("t1"));
        assert!(qs
            .iter()
            .any(|q| q.group_by.iter().any(|&c| c < 2) && q.group_by.iter().any(|&c| c >= 2)));
    }

    #[test]
    fn enumerate_database_adds_join_queries_only_with_two_tables() {
        let one = Database::single(kv("t0"));
        assert_eq!(enumerate_database(&one), enumerate(&kv("t0")));
    }

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
            ],
        }
    }

    #[test]
    fn enumerate_covers_the_v0_space_and_is_nonempty() {
        let qs = enumerate(&orders());
        assert!(!qs.is_empty());
        assert!(
            qs.iter().all(|q| !q.group_by.is_empty()),
            "v0 forbids global aggregates: over an empty table `SELECT SUM(v) FROM t` \
             returns 1 NULL row while `... GROUP BY g` returns 0, so the two cannot \
             share one row-deletion rule (spec §5.2)"
        );
        assert!(
            qs.iter().all(|q| !q.aggs.is_empty()),
            "v0's root operator must be an Aggregate — otherwise the __w weight makes the materialized table's row count disagree with a plain SQL view's (spec §5.2)"
        );
        // I4: an exhaustive match — adding a new Predicate variant later fails
        // to compile here, forcing the matching coverage assertion to be added,
        // instead of quietly missing a whole branch and staying green the way
        // IsNotNull once did.
        let mut saw_none = false;
        let mut saw_compare: Vec<(CmpOp, ColumnType)> = Vec::new();
        let mut saw_is_null = false;
        let mut saw_is_not_null = false;
        for q in &qs {
            match &q.predicate {
                Predicate::None => saw_none = true,
                Predicate::Compare { column, op, .. } => {
                    saw_compare.push((*op, orders().columns[*column].ty))
                }
                Predicate::IsNull { .. } => saw_is_null = true,
                Predicate::IsNotNull { .. } => saw_is_not_null = true,
            }
        }
        assert!(saw_none, "enumerate must produce Predicate::None");
        for op in CmpOp::ALL {
            for ty in [ColumnType::Integer, ColumnType::Text] {
                assert!(
                    saw_compare.contains(&(op, ty)),
                    "enumerate must compare a {ty:?} column with {op:?}"
                );
            }
        }
        assert!(saw_is_null, "enumerate must produce Predicate::IsNull");
        assert!(
            saw_is_not_null,
            "enumerate must produce Predicate::IsNotNull"
        );
    }

    #[test]
    fn the_enumeration_has_the_recorded_size() {
        // Recorded in the parent spec §9.2 amendment (M2c spec §6).
        let db = crate::gen_database(2);
        assert_eq!(enumerate(&db.tables()[0]).len(), 510);
        for db in [
            crate::gen_database(2),
            crate::gen_database_with_swapped_right_table(),
            crate::gen_real_numeric_database(),
        ] {
            assert_eq!(enumerate_join(&db.tables()[0], &db.tables()[1]).len(), 1197);
        }
    }

    #[test]
    fn the_enumeration_covers_every_aggregate_on_every_column() {
        let db = crate::gen_database(2);
        let queries = enumerate(&db.tables()[0]);
        for func in [
            AggFn::Sum,
            AggFn::Total,
            AggFn::Avg,
            AggFn::Count,
            AggFn::Min,
            AggFn::Max,
        ] {
            for c in 0..3 {
                assert!(
                    queries.iter().any(|q| q.aggs.contains(&Agg {
                        func,
                        column: Some(c)
                    })),
                    "{func:?}({c})"
                );
            }
        }
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        for func in [
            AggFn::Sum,
            AggFn::Total,
            AggFn::Avg,
            AggFn::Count,
            AggFn::Min,
            AggFn::Max,
        ] {
            assert!(
                joins
                    .iter()
                    .any(|q| q.aggs.iter().any(|a| a.func == func && a.column.is_some())),
                "{func:?}"
            );
        }
    }

    #[test]
    fn comparison_literals_follow_the_recorded_affinity_rotation() {
        // Pin the inputs as well as lower's acceptance: a different literal
        // can lower successfully while silently losing conversion coverage.
        let expected = |ty: ColumnType| match ty {
            ColumnType::Integer | ColumnType::Numeric => {
                vec![Value::Int(3), Value::Real(2.5), Value::Text("3".into())]
            }
            ColumnType::Real => {
                vec![Value::Real(2.0), Value::Text("2.5".into()), Value::Int(3)]
            }
            ColumnType::Text => vec![Value::Text("v3".into()), Value::Int(12)],
            ColumnType::Blob => {
                vec![
                    Value::Int(2),
                    Value::Text("v1".into()),
                    Value::Blob(vec![1]),
                ]
            }
        };
        for db in [
            crate::gen_database(2),
            crate::gen_database_with_swapped_right_table(),
            crate::gen_real_numeric_database(),
        ] {
            let schema = &db.tables()[0];
            for q in enumerate(schema) {
                if let Predicate::Compare { column, op, value } = q.predicate {
                    let literals = expected(schema.columns[column].ty);
                    let index = CmpOp::ALL
                        .iter()
                        .position(|candidate| *candidate == op)
                        .unwrap();
                    assert_eq!(value, literals[index % literals.len()]);
                }
            }
            let columns: Vec<_> = db.tables().iter().flat_map(|t| &t.columns).collect();
            for q in enumerate_join(&db.tables()[0], &db.tables()[1]) {
                if let Predicate::Compare { column, value, .. } = q.predicate {
                    let literals = expected(columns[column].ty);
                    assert_eq!(value, literals[column % literals.len()]);
                }
            }
        }
    }

    #[test]
    fn every_literal_is_one_lower_accepts() {
        for db in [crate::gen_database(2), crate::gen_real_numeric_database()] {
            for q in enumerate_database(&db) {
                ivmlite_core::lower_query(&q, &db).unwrap_or_else(|e| panic!("{q:?}: {e}"));
            }
        }
    }

    #[test]
    fn join_predicates_reach_every_column_of_the_joined_row() {
        // Ruling 4: the join space's job is to evaluate a predicate at every
        // position of the joined row, both sides of the boundary.
        let db = crate::gen_database(2);
        let qs = enumerate_join(&db.tables()[0], &db.tables()[1]);
        for column in 0..6 {
            assert!(
                qs.iter().any(
                    |q| matches!(q.predicate, Predicate::Compare { column: c, .. } if c == column)
                ),
                "no comparison on joined column {column}"
            );
            assert!(
                qs.iter().any(|q| matches!(
                    q.predicate,
                    Predicate::IsNull { column: c } | Predicate::IsNotNull { column: c } if c == column
                )),
                "no null test on joined column {column}"
            );
        }
    }

    #[test]
    fn enumerate_database_alternates_single_table_and_join_queries() {
        // Ruling 5: any run of consecutive seeds draws both kinds.
        let two = Database::new(vec![kv("t0"), kv("t1")]);
        let all = enumerate_database(&two);
        let single = enumerate(&kv("t0"));
        let joins = enumerate_join(&kv("t0"), &kv("t1"));
        assert_eq!(all.len(), single.len() + joins.len());
        let paired = single.len().min(joins.len());
        for i in 0..paired {
            assert_eq!(
                all[2 * i],
                single[i],
                "even positions are single-table queries"
            );
            assert_eq!(all[2 * i + 1], joins[i], "odd positions are join queries");
        }
        let rest = if single.len() > paired {
            &single[paired..]
        } else {
            &joins[paired..]
        };
        assert_eq!(&all[2 * paired..], rest);
    }

    #[test]
    fn only_count_may_omit_its_column() {
        // Spec §5.2 (`lower`): every aggregate but COUNT(*) names a column.
        let db = crate::gen_database(2);
        let single = enumerate(&db.tables()[0]);
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        for q in single.iter().chain(&joins) {
            for agg in &q.aggs {
                assert!(
                    agg.func == AggFn::Count || agg.column.is_some(),
                    "{:?} must name a column: {q:?}",
                    agg.func
                );
            }
        }
        assert!(
            single.iter().any(|q| q
                .aggs
                .iter()
                .any(|a| a.func == AggFn::Count && a.column.is_none())),
            "COUNT(*) is enumerated"
        );
    }
}
