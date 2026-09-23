use crate::{Agg, AggFn, ColumnType, Predicate, Schema, ViewQuery};

/// Spec §5.2's plan IR.
///
/// v0's legal shape is always `Scan → Filter? → Project → Aggregate`, which
/// `lower` guarantees. The `Join` variant is left for Phase 3, to arrive with the
/// join operator: adding now a variant that every match arm can only answer with
/// `unreachable!()` would leave, at every match, code no test can reach.
///
/// Spec §5.2 writes the expressions inside nodes as `Expr`; this implementation
/// uses `Vec<usize>` (column indices) and `Predicate` instead. v0's group-by keys
/// can only be bare columns (§5.2), and nothing in the predicate whitelist
/// (§6.1) needs an expression tree, so in v0 `Expr` would be an empty shell with
/// a single `Column(usize)` variant. Introducing it once the first feature
/// genuinely needs expressions is a local change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Scan {
        table: String,
        /// **Currently written but never read** (final review Finding G):
        /// `node.rs`'s `Node::build` destructures `Plan::Scan { table, .. }`,
        /// discarding `columns` through `..`, and `Node::Scan` does not hold it
        /// at all. It landed in Task 1, when `Plan` had no consumer and the
        /// structural test `lowers_to_scan_filter_project_aggregate`, asserting
        /// on the field itself, was its only reader; once Task 3 gave `Plan` its
        /// first real consumer (`Node::build`), the field became dead data.
        /// **Do not delete it**: M1b's delta-table reader is its plausible first
        /// consumer — it needs to know which base-table columns to `SELECT`
        /// rather than reading all of them unconditionally. Until then it is a
        /// statement of intent, not any constraint currently in force.
        columns: Vec<usize>,
    },
    Filter {
        input: Box<Plan>,
        predicate: Predicate,
    },
    Project {
        input: Box<Plan>,
        /// The **input** column indices to keep, in output order.
        columns: Vec<usize>,
    },
    Aggregate {
        input: Box<Plan>,
        /// Indices are relative to `Project`'s **output**, not the base table.
        group_by: Vec<usize>,
        /// Each `Agg::column` is likewise already remapped to `Project`'s output positions.
        aggs: Vec<Agg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PlanError {}

/// Lower the harness's flat `ViewQuery` into an operator tree, enforcing §5.2's legality checks at the boundary.
///
/// This is the first place a `ViewQuery` crosses outside `enumerate`: the
/// engine consumes it directly. `enumerate` never produces an illegal shape
/// (`enumerate_covers_the_v0_space_and_is_nonempty` guards that), but a
/// `ViewQuery` can be constructed freely, so the checks must live here.
///
/// `schema` is the base table the query reads. Its column count drives the
/// out-of-range checks and its column types drive the type checks: v0 supports
/// `SUM` and `IntGt` over INTEGER columns only, and rejects them over TEXT at
/// this boundary rather than letting the engine silently disagree with SQLite.
pub fn lower(query: &ViewQuery, schema: &Schema) -> Result<Plan, PlanError> {
    let table = schema.table.as_str();
    let arity = schema.arity();
    if query.group_by.is_empty() {
        return Err(PlanError(
            "spec §5.2: a view's root operator must be an Aggregate with a \
             non-empty GROUP BY; global aggregates are forbidden (over an empty \
             table one returns 1 row where a grouped aggregate returns 0, so the \
             rule \"delete the row when its group's count reaches zero\" is wrong \
             for it)"
                .into(),
        ));
    }
    if query.aggs.is_empty() {
        return Err(PlanError(
            "spec §5.2: a query with no aggs is really Scan→Project made \
             directly into a view, a shape in which Z-set weights and SQL row \
             counts disagree"
                .into(),
        ));
    }

    // `SUM` must carry a column. This check is **not** redundant: `Agg::column`
    // is an `Option<usize>` (`None` for `COUNT(*)`), so the type system cannot
    // stop `Agg { func: Sum, column: None }`, and `AggState::absorb` can only
    // `expect` SUM's column — without this gate a legally constructed
    // `ViewQuery` would pass through `lower` and `Node::build` and panic only on
    // the first batch of deltas. The reasoning of `out_of_range_column_is_rejected`
    // applies unchanged: the engine should have no foreseeable panic path after
    // create_view.
    for (i, agg) in query.aggs.iter().enumerate() {
        if agg.func == AggFn::Sum && agg.column.is_none() {
            return Err(PlanError(format!(
                "spec §5.2: agg {i} is SUM but names no column; only COUNT(*) may omit one"
            )));
        }
    }

    let check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} references column index {c}, but table {table} has only {arity} columns"
            )))
        } else {
            Ok(())
        }
    };
    for &c in &query.group_by {
        check(c, "group_by")?;
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            check(c, "agg")?;
        }
    }
    match &query.predicate {
        Predicate::None => {}
        Predicate::IntGt { column, .. } | Predicate::IsNotNull { column } => {
            check(*column, "predicate")?
        }
    }

    // Type checks. These run after the bounds checks because they index
    // `schema.columns`, which is only safe once every index is known valid.
    //
    // v0 cannot agree with SQLite on either of these over a TEXT column, so
    // they are rejected here instead of producing a silent divergence
    // (external review P2-1). Measured against SQLite, `STRICT` table
    // `t(g INTEGER, v TEXT)`:
    // - `SUM(v)` coerces numeric-looking text and returns `7.0` — a REAL — for
    //   rows `('7')` and `('abc')`. v0's `Value` has no `Real` variant at all
    //   (floating-point addition is not associative), and its accumulator only
    //   sees `Value::Int`, so it would report NULL.
    // - `v > 3` is true for every TEXT value, because SQLite orders storage
    //   classes as NULL < INTEGER/REAL < TEXT < BLOB. v0's `passes()` returns
    //   false for TEXT.
    // `IS NOT NULL` is type-agnostic and stays allowed on any column.
    let require_integer = |c: usize, what: &str| -> Result<(), PlanError> {
        let col = &schema.columns[c];
        if col.ty == ColumnType::Integer {
            Ok(())
        } else {
            Err(PlanError(format!(
                "{what} over column {c} (`{}`) of type {:?} is not supported: \
                 v0 allows it over INTEGER columns only",
                col.name, col.ty
            )))
        }
    };
    for agg in &query.aggs {
        if let (AggFn::Sum, Some(c)) = (agg.func, agg.column) {
            require_integer(c, "SUM")?;
        }
    }
    if let Predicate::IntGt { column, .. } = &query.predicate {
        require_integer(*column, "IntGt")?;
    }

    // The columns the projection keeps: group keys first (in their original
    // order), then each agg's column (in order), deduplicated. The order must be
    // deterministic, or Aggregate's index remapping has nothing to line up
    // against (spec §9.4).
    let mut keep: Vec<usize> = Vec::new();
    for &c in &query.group_by {
        if !keep.contains(&c) {
            keep.push(c);
        }
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            if !keep.contains(&c) {
                keep.push(c);
            }
        }
    }
    // `keep` always starts with the group_by columns, in their original order
    // and (at this point) deduplicated, so as long as group_by itself has no
    // duplicates, remap(group_by[i]) is provably always i — the group_by
    // remapping is always the trivial identity and cannot go wrong through a
    // remapping bug. The part that can go wrong, and the only part of this logic
    // worth testing, is the agg-column remapping (those columns land after the
    // group_by ones in `keep`, at positions that depend on dedup and order, not
    // trivially).
    let remap = |c: usize| {
        keep.iter()
            .position(|&k| k == c)
            .expect("keep is built from the group_by and agg columns, so it contains them")
    };

    // Scan takes every column: Filter's predicate is evaluated against **base-table** indices, and narrowing happens after Filter.
    let mut node = Plan::Scan {
        table: table.to_string(),
        columns: (0..arity).collect(),
    };
    if query.predicate != Predicate::None {
        node = Plan::Filter {
            input: Box::new(node),
            predicate: query.predicate.clone(),
        };
    }
    node = Plan::Project {
        input: Box::new(node),
        columns: keep.clone(),
    };
    Ok(Plan::Aggregate {
        input: Box::new(node),
        group_by: query.group_by.iter().map(|&c| remap(c)).collect(),
        aggs: query
            .aggs
            .iter()
            .map(|a| Agg {
                func: a.func,
                column: a.column.map(remap),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};

    fn q(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery {
            group_by,
            aggs,
            predicate,
        }
    }

    /// A table named `orders` whose columns are all INTEGER. The lowering tests
    /// here never push rows through the plan, so an all-INTEGER schema makes
    /// every `SUM` / `IntGt` legal without asserting anything about data.
    fn ints(arity: usize) -> Schema {
        Schema {
            table: "orders".into(),
            columns: (0..arity)
                .map(|i| Column {
                    name: format!("c{i}"),
                    ty: ColumnType::Integer,
                    nullable: true,
                })
                .collect(),
        }
    }

    /// `t(g INTEGER, v TEXT)` — the shape external review P2-1 reproduced
    /// the divergence on.
    fn int_then_text() -> Schema {
        Schema {
            table: "t".into(),
            columns: vec![
                Column {
                    name: "g".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
            ],
        }
    }

    fn count() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    fn sum(column: usize) -> Agg {
        Agg {
            func: AggFn::Sum,
            column: Some(column),
        }
    }

    #[test]
    fn lowers_to_scan_filter_project_aggregate() {
        // The predicate uses column 1 and the aggregate needs only column 0 —
        // Project must narrow 2 columns to 1, and Aggregate's group_by indices
        // must be remapped to the narrowed positions.
        let plan = lower(
            &q(
                vec![0],
                vec![count()],
                Predicate::IntGt {
                    column: 1,
                    value: 3,
                },
            ),
            &ints(2),
        )
        .expect("a legal query must lower");

        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("the root operator must be an Aggregate (spec §5.2): {plan:?}");
        };
        assert_eq!(
            group_by,
            &vec![0],
            "after narrowing, the group key lands at position 0"
        );
        assert_eq!(aggs.len(), 1);

        let Plan::Project { input, columns } = &**input else {
            panic!("a Project must sit below the Aggregate: {input:?}");
        };
        assert_eq!(columns, &vec![0], "only column 0 is used by the aggregate");

        let Plan::Filter { input, predicate } = &**input else {
            panic!("a Filter must sit below the Project: {input:?}");
        };
        assert_eq!(
            predicate,
            &Predicate::IntGt {
                column: 1,
                value: 3
            }
        );

        let Plan::Scan { table, columns } = &**input else {
            panic!("the bottom node must be a Scan: {input:?}");
        };
        assert_eq!(table, "orders");
        assert_eq!(
            columns,
            &vec![0, 1],
            "Scan takes every column — Filter evaluates against the original indices"
        );
    }

    #[test]
    fn no_filter_node_when_predicate_is_none() {
        // Predicate::None should not produce an always-true Filter node: an
        // extra node is a pointless traversal on every batch, and it makes
        // "Filter is correctly skipped" unobservable.
        let plan = lower(&q(vec![0], vec![count()], Predicate::None), &ints(2)).unwrap();
        let Plan::Aggregate { input, .. } = &plan else {
            panic!("{plan:?}")
        };
        let Plan::Project { input, .. } = &**input else {
            panic!("{input:?}")
        };
        assert!(
            matches!(&**input, Plan::Scan { .. }),
            "under Predicate::None the Scan should come directly, with no always-true Filter inserted: {input:?}"
        );
    }

    #[test]
    fn projection_keeps_group_keys_and_summed_columns_in_a_stable_order() {
        // The group key is column 1 and SUM is over column 0 — the order after
        // narrowing must be deterministic and predictable, or Aggregate's index
        // remapping has nothing to line up against (spec §9.4).
        let plan = lower(&q(vec![1], vec![sum(0)], Predicate::None), &ints(2)).unwrap();
        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("{plan:?}")
        };
        let Plan::Project { columns, .. } = &**input else {
            panic!("{input:?}")
        };
        assert_eq!(
            columns,
            &vec![1, 0],
            "group keys first (in original order), then each agg's column (in order)"
        );
        assert_eq!(
            group_by,
            &vec![0],
            "the group key is remapped to narrowed position 0"
        );
        assert_eq!(
            aggs[0].column,
            Some(1),
            "SUM's column is remapped to narrowed position 1"
        );
    }

    #[test]
    fn a_column_used_as_both_group_key_and_sum_target_is_projected_once() {
        // A column that is both a group key and a SUM target must not appear in
        // the projection twice — twice would make Project's output width
        // disagree with what Aggregate expects.
        let plan = lower(&q(vec![0], vec![sum(0)], Predicate::None), &ints(2)).unwrap();
        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("{plan:?}")
        };
        let Plan::Project { columns, .. } = &**input else {
            panic!("{input:?}")
        };
        assert_eq!(columns, &vec![0], "projected once after dedup");
        assert_eq!(group_by, &vec![0]);
        assert_eq!(aggs[0].column, Some(0));
    }

    #[test]
    fn two_aggs_sharing_a_non_group_by_column_are_projected_once() {
        // When two aggs share one non-group-by column (here both SUM(1)), the
        // dedup must ask "is this column already in keep", not "does this column
        // equal some group_by column" — the latter does nothing for this case,
        // since column 1 is not in group_by at all, so the dedup condition is
        // always true and `Project.columns` becomes `[0, 1, 1]`: a 3-wide
        // projection over a 2-column table.
        let plan = lower(&q(vec![0], vec![sum(1), sum(1)], Predicate::None), &ints(2)).unwrap();
        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("{plan:?}")
        };
        let Plan::Project { columns, .. } = &**input else {
            panic!("{input:?}")
        };
        assert_eq!(
            columns,
            &vec![0, 1],
            "a column shared by two aggs is projected once"
        );
        assert_eq!(group_by, &vec![0]);
        assert_eq!(
            aggs[0].column,
            Some(1),
            "the first SUM is remapped to narrowed position 1"
        );
        assert_eq!(
            aggs[1].column,
            Some(1),
            "the second SUM is remapped to position 1 too"
        );
    }

    #[test]
    fn empty_group_by_is_rejected_at_the_boundary() {
        // Spec §5.2: global aggregates are forbidden — over an empty table one
        // returns 1 row (with value NULL) where a grouped aggregate returns 0,
        // so "delete the row when its group's count reaches zero" is wrong for
        // it. This used to hold only generator-side (enumerate never produces
        // the shape); now that the engine consumes ViewQuery directly, the
        // boundary check must live here.
        let err = lower(&q(vec![], vec![count()], Predicate::None), &ints(2))
            .expect_err("an empty group_by must be rejected");
        assert!(
            err.0.contains("group_by") || err.0.contains("GROUP BY"),
            "the error should name group_by as the problem: {}",
            err.0
        );
    }

    #[test]
    fn empty_aggs_is_rejected_at_the_boundary() {
        // The root operator must be an Aggregate; an "Aggregate" with no aggs is
        // really Scan→Project made directly into a view, exactly the shape §5.2
        // rules illegal (a Z-set weight of 2 would show as 2 rows where a plain
        // SQL view shows 3).
        let err = lower(&q(vec![0], vec![], Predicate::None), &ints(2))
            .expect_err("empty aggs must be rejected");
        assert!(
            err.0.contains("agg"),
            "the error should name aggs as the problem: {}",
            err.0
        );
    }

    #[test]
    fn sum_without_a_column_is_rejected_at_the_boundary() {
        // `Agg::column` is an `Option<usize>` (`None` for `COUNT(*)`), so
        // `Agg { func: Sum, column: None }` is a legal construction the type
        // system cannot stop. Without this check it passes through `lower` and
        // `Node::build` and panics only when `AggState::absorb` first processes
        // the agg — exactly the "foreseeable panic path after create_view" that
        // `out_of_range_column_is_rejected` says must be avoided. The only
        // producer, `enumerate`, never produces the shape, but a `ViewQuery` can
        // be constructed freely outside `enumerate` (M1b's create-view path will).
        let bad = Agg {
            func: AggFn::Sum,
            column: None,
        };
        let err = lower(&q(vec![0], vec![count(), bad], Predicate::None), &ints(2))
            .expect_err("a SUM without a column must be rejected");
        assert!(
            err.0.contains("SUM"),
            "the error should name SUM as the problem: {}",
            err.0
        );
        assert!(
            err.0.contains('1'),
            "the error should say which agg it is (index 1 here): {}",
            err.0
        );
    }

    #[test]
    fn out_of_range_column_is_rejected() {
        // An out-of-range index must fail while lowering, not panic at refresh
        // — the engine should have no foreseeable panic path after create_view.
        //
        // **This pins only the group_by branch of the bounds check** (final
        // review Finding C): `lower` has three independent `check(...)` calls
        // (group_by / agg column / predicate column), and this case keeps the
        // aggs and the predicate in range, with only group_by=[7] out of range.
        // Deleting either of the other two `check`s leaves this test green —
        // the two tests below are what pin them individually.
        let err = lower(&q(vec![7], vec![count()], Predicate::None), &ints(2))
            .expect_err("an out-of-range group key must be rejected");
        assert!(
            err.0.contains('7'),
            "the error should name the out-of-range index: {}",
            err.0
        );
    }

    #[test]
    fn out_of_range_agg_column_is_rejected() {
        // Final review Finding C: `out_of_range_column_is_rejected` covers only
        // the group_by branch. Measured with a surgical mutation: deleting only
        // the agg-column `check(c, "agg")?` (keeping the other two) leaves
        // `out_of_range_column_is_rejected` green, `create_view` succeeds, and
        // the first non-empty delta panics inside `Row::get` — exactly the
        // "foreseeable panic path after create_view" that the comment on the
        // SUM-column check in `lower` says is closed. Here group_by and the
        // predicate are legal and only the agg column (column 7) is out of
        // range, pinning this branch on its own.
        let bad = Agg {
            func: AggFn::Sum,
            column: Some(7),
        };
        let err = lower(&q(vec![0], vec![bad], Predicate::None), &ints(2))
            .expect_err("an out-of-range agg column must be rejected");
        assert!(
            err.0.contains('7'),
            "the error should name the out-of-range index: {}",
            err.0
        );
        assert!(
            err.0.contains("agg"),
            "the error should name the agg as the problem: {}",
            err.0
        );
    }

    #[test]
    fn out_of_range_predicate_column_is_rejected() {
        // Found in the same review as the previous test: deleting only the
        // predicate-column `check(*column, "predicate")?` (keeping the other
        // two) likewise leaves `out_of_range_column_is_rejected` green. Here
        // group_by and the agg column are legal and only the column the
        // predicate references (column 7) is out of range, pinning this branch
        // on its own.
        let err = lower(
            &q(
                vec![0],
                vec![count()],
                Predicate::IntGt {
                    column: 7,
                    value: 3,
                },
            ),
            &ints(2),
        )
        .expect_err("an out-of-range predicate column must be rejected");
        assert!(
            err.0.contains('7'),
            "the error should name the out-of-range index: {}",
            err.0
        );
        assert!(
            err.0.contains("predicate"),
            "the error should name the predicate as the problem: {}",
            err.0
        );
    }

    #[test]
    fn sum_over_a_text_column_is_rejected_at_the_boundary() {
        // SQLite coerces numeric-looking text inside SUM and can return a REAL
        // (measured: SUM over ('7'), ('abc') is 7.0). v0 has no Real value and
        // its accumulator only sees Value::Int, so it would report NULL. The
        // query must be refused at create_view, not answered differently.
        let err = lower(&q(vec![0], vec![sum(1)], Predicate::None), &int_then_text())
            .expect_err("SUM over a TEXT column must be rejected");
        assert!(
            err.0.contains("SUM") && err.0.contains("Text"),
            "the error must name SUM and the offending column type: {}",
            err.0
        );
    }

    #[test]
    fn int_gt_over_a_text_column_is_rejected_at_the_boundary() {
        // SQLite orders storage classes NULL < INTEGER/REAL < TEXT < BLOB, so
        // `v > 3` is true for every TEXT value; v0's `passes()` says false.
        let err = lower(
            &q(
                vec![0],
                vec![count()],
                Predicate::IntGt {
                    column: 1,
                    value: 3,
                },
            ),
            &int_then_text(),
        )
        .expect_err("IntGt over a TEXT column must be rejected");
        assert!(
            err.0.contains("IntGt") && err.0.contains("Text"),
            "the error must name IntGt and the offending column type: {}",
            err.0
        );
    }

    #[test]
    fn is_not_null_over_a_text_column_is_still_allowed() {
        // Guards against over-rejecting: IS NOT NULL is type-agnostic, and the
        // enumerated v0 space uses it on TEXT columns.
        lower(
            &q(vec![0], vec![count()], Predicate::IsNotNull { column: 1 }),
            &int_then_text(),
        )
        .expect("IS NOT NULL over a TEXT column is legal");
    }
}
