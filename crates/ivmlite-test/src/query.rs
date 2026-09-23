use crate::{Agg, AggFn, ColumnType, Predicate, Schema, ViewQuery};
use ivmlite_core::{Database, Join};

/// Enumerate v0's query space.
///
/// Spec §9.2: v0 has finitely many combinations, so enumeration beats
/// randomness — reproducible and complete. Randomness is left to the update
/// sequences.
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    let int_cols: Vec<usize> = (0..schema.arity())
        .filter(|i| schema.columns[*i].ty == ColumnType::Integer)
        .collect();

    let mut group_by_choices: Vec<Vec<usize>> = Vec::new();
    for i in 0..schema.arity() {
        group_by_choices.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            group_by_choices.push(vec![i, j]);
        }
    }

    let mut agg_choices: Vec<Vec<Agg>> = vec![vec![Agg {
        func: AggFn::Count,
        column: None,
    }]];
    for i in &int_cols {
        agg_choices.push(vec![Agg {
            func: AggFn::Sum,
            column: Some(*i),
        }]);
        agg_choices.push(vec![
            Agg {
                func: AggFn::Sum,
                column: Some(*i),
            },
            Agg {
                func: AggFn::Count,
                column: None,
            },
        ]);
    }

    let mut predicates = vec![Predicate::None];
    for i in &int_cols {
        predicates.push(Predicate::IntGt {
            column: *i,
            value: 4,
        });
    }
    for i in 0..schema.arity() {
        if schema.columns[i].nullable {
            predicates.push(Predicate::IsNotNull { column: i });
        }
    }

    let mut out = Vec::new();
    for group_by in &group_by_choices {
        for aggs in &agg_choices {
            for predicate in &predicates {
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

/// Enumerate v0's two-table join space: every pair of same-typed key columns,
/// crossed with the single-table dimensions (group-by, aggregates, predicates)
/// over the joined row — `left`'s columns, then `right`'s.
///
/// Keys of different types are never generated: `lower` rejects them (SQLite
/// would compare them under numeric affinity).
pub fn enumerate_join(left: &Schema, right: &Schema) -> Vec<ViewQuery> {
    // The joined row, as a schema, so the single-table enumerator can supply
    // the other dimensions. Its name and column names are never rendered.
    let joined = Schema {
        table: "joined".into(),
        columns: left
            .columns
            .iter()
            .chain(right.columns.iter())
            .cloned()
            .collect(),
    };
    let shapes = enumerate(&joined);
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
/// first, then, when `db` has at least two tables, the join queries over its
/// first two.
pub fn enumerate_database(db: &Database) -> Vec<ViewQuery> {
    let tables = db.tables();
    let mut out = enumerate(&tables[0]);
    if tables.len() >= 2 {
        out.extend(enumerate_join(&tables[0], &tables[1]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::kv;
    use crate::{Column, ColumnType, Schema};

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
        let two = Database::new(vec![kv("t0"), kv("t1")]);
        let all = enumerate_database(&two);
        let single = enumerate(&kv("t0"));
        assert_eq!(
            &all[..single.len()],
            &single[..],
            "single-table queries come first"
        );
        assert_eq!(
            all.len(),
            single.len() + enumerate_join(&kv("t0"), &kv("t1")).len()
        );
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
        // I4: an exhaustive match rather than three any() calls — adding a new
        // Predicate variant later fails to compile here, forcing the matching
        // coverage assertion to be added, instead of quietly missing a whole
        // branch and staying green the way IsNotNull once did.
        let mut saw_none = false;
        let mut saw_int_gt = false;
        let mut saw_is_not_null = false;
        for q in &qs {
            match &q.predicate {
                Predicate::None => saw_none = true,
                Predicate::IntGt { .. } => saw_int_gt = true,
                Predicate::IsNotNull { .. } => saw_is_not_null = true,
            }
        }
        assert!(saw_none, "enumerate must produce Predicate::None");
        assert!(saw_int_gt, "enumerate must produce Predicate::IntGt");
        assert!(
            saw_is_not_null,
            "enumerate must produce Predicate::IsNotNull"
        );
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
