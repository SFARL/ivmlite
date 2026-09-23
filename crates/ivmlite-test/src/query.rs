use crate::{Agg, AggFn, ColumnType, Predicate, Schema, ViewQuery};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};

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
