use crate::{Agg, AggFn, CmpOp, ColumnType, Predicate, Schema, ViewQuery};
use ivmlite_core::{Database, Join, Value};

/// Enumerate v0's single-table query space.
///
/// Spec §9.2: v0 has finitely many combinations, so enumeration beats
/// randomness — reproducible and complete. Randomness is left to the update
/// sequences.
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    cross(
        &group_by_choices(schema),
        &agg_choices(schema),
        &predicates(schema),
    )
}

/// Enumerate v0's two-table join space: every pair of same-typed key columns,
/// crossed with the group-bys, aggregates and join predicates over the joined
/// row — `left`'s columns, then `right`'s.
///
/// Keys of different types are never generated: `lower` rejects them (SQLite
/// would compare them under numeric affinity).
///
/// The predicates are `join_predicates`, not the single-table set (M1b Phase
/// 2a, Ruling 4): one comparison and one null test per column, which reaches
/// every position of the joined row. The operators rotate with the column, so
/// over a 4-column joined row only `>`, `>=`, `<` and `<=` appear; `=` and `!=`
/// are covered by the single-table space.
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
    let shapes = cross(
        &group_by_choices(&joined),
        &agg_choices(&joined),
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

fn cross(group_bys: &[Vec<usize>], aggs: &[Vec<Agg>], predicates: &[Predicate]) -> Vec<ViewQuery> {
    let mut out = Vec::new();
    for group_by in group_bys {
        for aggs in aggs {
            for predicate in predicates {
                out.push(ViewQuery {
                    group_by: group_by.clone(),
                    aggs: aggs.clone(),
                    predicate: predicate.clone(),
                    join: None,
                });
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

fn agg_choices(schema: &Schema) -> Vec<Vec<Agg>> {
    let count = Agg {
        func: AggFn::Count,
        column: None,
    };
    let mut out = vec![vec![count.clone()]];
    for (i, col) in schema.columns.iter().enumerate() {
        if col.ty != ColumnType::Integer {
            continue;
        }
        let sum = Agg {
            func: AggFn::Sum,
            column: Some(i),
        };
        out.push(vec![sum.clone()]);
        out.push(vec![sum, count.clone()]);
    }
    out
}

/// The literal every enumerated comparison on a column of type `ty` uses: the
/// middle of `Domain::default()`'s values (`0..8`, rendered `"v0"`..`"v7"` for
/// TEXT), so each operator keeps some rows and drops others, and `=` / `!=`
/// meet rows equal to the literal.
fn literal(ty: ColumnType) -> Value {
    match ty {
        ColumnType::Integer => Value::Int(4),
        ColumnType::Text => Value::Text("v4".into()),
    }
}

/// The single-table predicates: `None`, every operator on every column, and
/// `IS NULL` / `IS NOT NULL` on every nullable column.
fn predicates(schema: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in schema.columns.iter().enumerate() {
        for op in CmpOp::ALL {
            out.push(Predicate::Compare {
                column,
                op,
                value: literal(col.ty),
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
/// operator `CmpOp::ALL[column % 6]`, and one null test per nullable column —
/// `IS NULL` on even columns, `IS NOT NULL` on odd ones.
fn join_predicates(joined: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in joined.columns.iter().enumerate() {
        out.push(Predicate::Compare {
            column,
            op: CmpOp::ALL[column % CmpOp::ALL.len()],
            value: literal(col.ty),
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
    use ivmlite_core::{CmpOp, Value};

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
        assert_eq!(pairs, vec![(0, 1), (1, 0)]);
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
    fn enumerate_sizes_match_the_plan() {
        // M1b Phase 2a, Ruling 4.
        let db = crate::gen_database(2);
        assert_eq!(enumerate(&db.tables()[0]).len(), 153);
        assert_eq!(enumerate_join(&db.tables()[0], &db.tables()[1]).len(), 900);
        let swapped = crate::gen_database_with_swapped_right_table();
        assert_eq!(
            enumerate_join(&swapped.tables()[0], &swapped.tables()[1]).len(),
            900
        );
    }

    #[test]
    fn every_comparison_literal_has_its_columns_type() {
        let db = crate::gen_database(2);
        let (l, r) = (&db.tables()[0], &db.tables()[1]);
        let joined: Vec<ColumnType> = l.columns.iter().chain(&r.columns).map(|c| c.ty).collect();
        let single = enumerate(l).into_iter().map(|q| (q, &joined[..2]));
        let joins = enumerate_join(l, r).into_iter().map(|q| (q, &joined[..]));
        for (q, types) in single.chain(joins) {
            if let Predicate::Compare { column, value, .. } = &q.predicate {
                let ok = matches!(
                    (types[*column], value),
                    (ColumnType::Integer, Value::Int(_)) | (ColumnType::Text, Value::Text(_))
                );
                assert!(ok, "{q:?}");
            }
        }
    }

    #[test]
    fn join_predicates_reach_every_column_of_the_joined_row() {
        // Ruling 4: the join space's job is to evaluate a predicate at every
        // position of the joined row, both sides of the boundary.
        let db = crate::gen_database(2);
        let qs = enumerate_join(&db.tables()[0], &db.tables()[1]);
        for column in 0..4 {
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
    fn enumerate_only_sums_integer_columns() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Sum {
                    let idx = agg.column.expect("SUM must have a column");
                    assert_eq!(
                        schema.columns[idx].ty,
                        ColumnType::Integer,
                        "v0 has no floating point, so SUM may only apply to INTEGER columns"
                    );
                }
            }
        }
    }

    #[test]
    fn enumerate_always_count_has_none() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Count {
                    assert!(
                        agg.column.is_none(),
                        "COUNT(*) must have column: None (spec §5.2)"
                    );
                }
            }
        }
    }
}
