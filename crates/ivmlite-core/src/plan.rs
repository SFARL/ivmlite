use crate::{Agg, AggFn, Column, ColumnType, Database, Join, Predicate, Schema, ViewQuery};

/// Spec §5.2's plan IR.
///
/// v0's legal shapes are `Scan → Filter? → Project → Aggregate` and, with a
/// join, `Join(Scan, Scan) → Filter? → Project → Aggregate`; `lower` guarantees
/// that nothing else is produced.
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
    /// Spec §5.2 / §6.1: a two-table inner equi-join (M1a Phase 3). Its output
    /// row is the left child's row followed by the right child's row, so every
    /// operator above it indexes that concatenation: the left child's columns
    /// first, then the right child's.
    ///
    /// The keys are column indices into each child's output rather than spec
    /// §5.2's `Vec<(Expr, Expr)>`: v0 joins on exactly one pair of bare columns
    /// (the same reasoning as the doc comment on `Plan` about `Expr`).
    Join {
        left: Box<Plan>,
        right: Box<Plan>,
        left_key: usize,
        right_key: usize,
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
/// engine consumes it directly. `enumerate` never produces an illegal shape,
/// but a `ViewQuery` can be constructed freely, so the checks must live here.
///
/// The view's left (or only) input is the anchor, `db.tables()[0]`. In M1b the
/// table order comes from `__ivm_dep`, not from the query's FROM clause —
/// choosing "the first" is only v0's convention (M1a Phase 2 final review
/// Finding I). A join's right input is looked up by name in `db`.
///
/// Column indices are checked against the row the operators above the scans
/// see — the anchor's columns, then (for a join) the right table's — and
/// their types drive the type checks: v0 supports `SUM` and `IntGt` over
/// INTEGER columns only, and join keys of one type only.
pub fn lower(query: &ViewQuery, db: &Database) -> Result<Plan, PlanError> {
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| PlanError("the database has no tables".into()))?;
    let right = match &query.join {
        None => None,
        Some(join) => Some(resolve_join(join, anchor, db)?),
    };
    let columns: Vec<&Column> = anchor
        .columns
        .iter()
        .chain(right.into_iter().flat_map(|s| s.columns.iter()))
        .collect();
    let arity = columns.len();
    let source = match right {
        None => format!("table {}", anchor.table),
        Some(r) => format!("the joined row of {} and {}", anchor.table, r.table),
    };
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
                "{what} references column index {c}, but {source} has only {arity} columns"
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
        let col = columns[c];
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

    // Each Scan takes every column of its table: Filter's predicate is
    // evaluated against the joined (or base-table) row, and narrowing happens
    // after Filter.
    let scan = |s: &Schema| Plan::Scan {
        table: s.table.clone(),
        columns: (0..s.arity()).collect(),
    };
    let mut node = match (&query.join, right) {
        (Some(join), Some(r)) => Plan::Join {
            left: Box::new(scan(anchor)),
            right: Box::new(scan(r)),
            left_key: join.left_column,
            right_key: join.right_column,
        },
        _ => scan(anchor),
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

/// Validate a join against the database and return its right table.
fn resolve_join<'a>(
    join: &Join,
    anchor: &Schema,
    db: &'a Database,
) -> Result<&'a Schema, PlanError> {
    if join.right == anchor.table {
        return Err(PlanError(format!(
            "join: a self-join of table {} is not supported in v0 — Scan routes each \
             delta by table name, so both inputs would receive every change to the table",
            anchor.table
        )));
    }
    let right = db
        .get(&join.right)
        .ok_or_else(|| PlanError(format!("join: table {} is not in the database", join.right)))?;
    if join.left_column >= anchor.arity() {
        return Err(PlanError(format!(
            "join: left_column {} is out of range, table {} has only {} columns",
            join.left_column,
            anchor.table,
            anchor.arity()
        )));
    }
    if join.right_column >= right.arity() {
        return Err(PlanError(format!(
            "join: right_column {} is out of range, table {} has only {} columns",
            join.right_column,
            right.table,
            right.arity()
        )));
    }
    let (l, r) = (
        &anchor.columns[join.left_column],
        &right.columns[join.right_column],
    );
    if l.ty != r.ty {
        return Err(PlanError(format!(
            "join: the key columns must have the same type, but {}.{} is {:?} and {}.{} is {:?} \
             — SQLite compares an INTEGER column with a TEXT column under numeric affinity, \
             so '7' = 7 is true there, while v0 compares values exactly",
            anchor.table, l.name, l.ty, right.table, r.name, r.ty
        )));
    }
    Ok(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Database, Join, Predicate, Schema, ViewQuery};

    fn q(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery {
            group_by,
            aggs,
            predicate,
            join: None,
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
            &Database::single(ints(2)),
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
        let plan = lower(
            &q(vec![0], vec![count()], Predicate::None),
            &Database::single(ints(2)),
        )
        .unwrap();
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
        let plan = lower(
            &q(vec![1], vec![sum(0)], Predicate::None),
            &Database::single(ints(2)),
        )
        .unwrap();
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
        let plan = lower(
            &q(vec![0], vec![sum(0)], Predicate::None),
            &Database::single(ints(2)),
        )
        .unwrap();
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
        let plan = lower(
            &q(vec![0], vec![sum(1), sum(1)], Predicate::None),
            &Database::single(ints(2)),
        )
        .unwrap();
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
        let err = lower(
            &q(vec![], vec![count()], Predicate::None),
            &Database::single(ints(2)),
        )
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
        let err = lower(
            &q(vec![0], vec![], Predicate::None),
            &Database::single(ints(2)),
        )
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
        let err = lower(
            &q(vec![0], vec![count(), bad], Predicate::None),
            &Database::single(ints(2)),
        )
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
        let err = lower(
            &q(vec![7], vec![count()], Predicate::None),
            &Database::single(ints(2)),
        )
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
        let err = lower(
            &q(vec![0], vec![bad], Predicate::None),
            &Database::single(ints(2)),
        )
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
            &Database::single(ints(2)),
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
        let err = lower(
            &q(vec![0], vec![sum(1)], Predicate::None),
            &Database::single(int_then_text()),
        )
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
            &Database::single(int_then_text()),
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
            &Database::single(int_then_text()),
        )
        .expect("IS NOT NULL over a TEXT column is legal");
    }

    /// `t0(k TEXT, v INTEGER)` and `t1(k TEXT, v INTEGER)` — the shape the
    /// differential harness's `gen_database(2)` produces.
    fn two_kv() -> Database {
        let kv = |name: &str| Schema {
            table: name.into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        Database::new(vec![kv("t0"), kv("t1")])
    }

    fn join(right: &str, left_column: usize, right_column: usize) -> Option<Join> {
        Some(Join {
            right: right.into(),
            left_column,
            right_column,
        })
    }

    fn jq(
        group_by: Vec<usize>,
        aggs: Vec<Agg>,
        predicate: Predicate,
        j: Option<Join>,
    ) -> ViewQuery {
        ViewQuery {
            join: j,
            ..q(group_by, aggs, predicate)
        }
    }

    #[test]
    fn lowers_a_join_to_two_scans_under_a_join() {
        // group by t1.k (joined column 2), SUM(t0.v) (joined column 1), join on k = k.
        let plan = lower(
            &jq(vec![2], vec![sum(1)], Predicate::None, join("t1", 0, 0)),
            &two_kv(),
        )
        .expect("a legal join query must lower");
        let scan = |t: &str| Plan::Scan {
            table: t.into(),
            columns: vec![0, 1],
        };
        assert_eq!(
            plan,
            Plan::Aggregate {
                input: Box::new(Plan::Project {
                    input: Box::new(Plan::Join {
                        left: Box::new(scan("t0")),
                        right: Box::new(scan("t1")),
                        left_key: 0,
                        right_key: 0,
                    }),
                    columns: vec![2, 1],
                }),
                group_by: vec![0],
                aggs: vec![Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                }],
            }
        );
    }

    #[test]
    fn a_column_past_the_joined_row_is_rejected() {
        let err = lower(
            &jq(vec![4], vec![count()], Predicate::None, join("t1", 0, 0)),
            &two_kv(),
        )
        .expect_err("the joined row has columns 0..4");
        assert!(
            err.0.contains('4') && err.0.contains("joined row"),
            "{}",
            err.0
        );
    }

    #[test]
    fn a_join_on_an_unknown_table_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("nope", 0, 0)),
            &two_kv(),
        )
        .expect_err("the right table must be declared");
        assert!(err.0.contains("nope"), "{}", err.0);
    }

    #[test]
    fn a_self_join_is_rejected() {
        // `Node::Scan` routes deltas by table name, so both inputs of a
        // self-join would receive every change to the table.
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t0", 0, 0)),
            &two_kv(),
        )
        .expect_err("v0 does not support self-joins");
        assert!(err.0.contains("self-join"), "{}", err.0);
    }

    #[test]
    fn an_out_of_range_left_join_key_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 5, 0)),
            &two_kv(),
        )
        .expect_err("t0 has only 2 columns");
        assert!(
            err.0.contains('5') && err.0.contains("left_column"),
            "{}",
            err.0
        );
    }

    #[test]
    fn an_out_of_range_right_join_key_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 0, 5)),
            &two_kv(),
        )
        .expect_err("t1 has only 2 columns");
        assert!(
            err.0.contains('5') && err.0.contains("right_column"),
            "{}",
            err.0
        );
    }

    #[test]
    fn join_keys_of_different_types_are_rejected() {
        // Measured in SQLite: an INTEGER column compared with a TEXT column
        // uses numeric affinity, so '7' = 7 is true there, while v0 compares
        // values exactly (the plan's Ruling 4).
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 0, 1)),
            &two_kv(),
        )
        .expect_err("t0.k is TEXT and t1.v is INTEGER");
        assert!(err.0.contains("same type"), "{}", err.0);
    }

    #[test]
    fn int_gt_over_the_right_tables_text_column_is_rejected() {
        // The type checks must use the joined row's columns: joined column 2 is t1.k, a TEXT column.
        let err = lower(
            &jq(
                vec![0],
                vec![count()],
                Predicate::IntGt {
                    column: 2,
                    value: 3,
                },
                join("t1", 0, 0),
            ),
            &two_kv(),
        )
        .expect_err("IntGt over TEXT is rejected");
        assert!(
            err.0.contains("IntGt") && err.0.contains("Text"),
            "{}",
            err.0
        );
    }
}
