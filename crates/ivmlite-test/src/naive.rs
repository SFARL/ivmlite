use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Value, ZSet};

use crate::{AggFn, CmpOp, Engine, EngineError, Predicate, ViewQuery};

/// The trivially correct reference implementation: it keeps the full base tables and recomputes on every materialize.
///
/// Two uses: showing the test framework raises no false alarms, and serving as the benchmark's "naive recompute" baseline (spec §10.2).
///
/// `apply` and `refresh` are genuinely separate phases: `apply` only appends the
/// raw `(table, Row, i64)` to `pending`, merging nothing, and `refresh` drains
/// `pending` into `base` table by table. If `apply` merged eagerly, `refresh`
/// would be a no-op and no engine ignoring the refresh contract would be caught
/// (spec §8.2).
///
/// `base` is held per table (`BTreeMap<String, ZSet>`) — since the harness went
/// multi-table, the initial state `create_view` receives is split by table
/// anyway. `materialize` aggregates the anchor's state (the first table in
/// `db`), or for a join query the nested-loop join of the anchor and the right
/// table — deliberately the simplest correct algorithm, sharing no code with
/// `JoinState`.
#[derive(Debug, Default)]
pub struct NaiveRecompute {
    query: Option<ViewQuery>,
    anchor: String,
    base: BTreeMap<String, ZSet>,
    pending: Vec<(String, Row, i64)>,
}

impl NaiveRecompute {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows the view aggregates: the anchor's base state, or — for a join —
    /// every matching pair of anchor and right rows, concatenated, with the
    /// product of their weights.
    ///
    /// A NULL key never matches: SQL's `NULL = NULL` is UNKNOWN (measured in
    /// SQLite; `sqlite_never_matches_null_join_keys` pins it on the oracle side).
    ///
    /// For a join, each side's rows with weight ≤ 0 are dropped **before**
    /// joining (spec §5.1: they do not take part in recomputation), the same
    /// rule `materialize` applies to a single table's rows. Dropping them only
    /// after the join would let two negative rows join into a positive one.
    fn input_rows(&self, query: &ViewQuery) -> ZSet {
        let anchor = self.base.get(&self.anchor).cloned().unwrap_or_default();
        let Some(join) = &query.join else {
            return anchor;
        };
        let anchor = positive_part(&anchor);
        let right = positive_part(&self.base.get(&join.right).cloned().unwrap_or_default());
        let mut joined = ZSet::new();
        for (l, &wl) in anchor.iter() {
            for (r, &wr) in right.iter() {
                let key = l.get(join.left_column);
                if *key != Value::Null && key == r.get(join.right_column) {
                    let mut values = l.0.clone();
                    values.extend(r.0.iter().cloned());
                    joined.update(Row::new(values), wl * wr);
                }
            }
        }
        joined
    }
}

/// The rows of `z` with a positive weight.
fn positive_part(z: &ZSet) -> ZSet {
    ZSet::from_rows(
        z.iter()
            .filter(|(_, &w)| w > 0)
            .map(|(row, &w)| (row.clone(), w)),
    )
}

fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::Compare { column, op, value } => {
            let cell = row.get(*column);
            // NULL <op> anything is UNKNOWN (spec §6.1), for `!=` too.
            if *cell == Value::Null {
                return false;
            }
            // `cell` and `value` are the same variant here — not because
            // `lower` guarantees it (`NaiveRecompute` never calls `lower`),
            // but because the enumerator only ever generates a comparison
            // literal of the column's own type (see
            // `every_comparison_literal_has_its_columns_type` in
            // `crates/ivmlite-test/src/query.rs`). `Value`'s derived order
            // within one variant is i64's order or `String`'s byte order —
            // SQLite's BINARY collation.
            match op {
                CmpOp::Gt => cell > value,
                CmpOp::Ge => cell >= value,
                CmpOp::Lt => cell < value,
                CmpOp::Le => cell <= value,
                CmpOp::Eq => cell == value,
                CmpOp::Ne => cell != value,
            }
        }
        Predicate::IsNull { column } => row.get(*column) == &Value::Null,
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}

impl Engine for NaiveRecompute {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        // Resolve everything that can fail before touching `self`, then commit
        // the whole new view at once. Replacing `query` first and failing on the
        // anchor lookup would leave a new query paired with the old base state.
        let anchor = db
            .tables()
            .first()
            .ok_or_else(|| EngineError("database has no tables".into()))?
            .table
            .clone();
        self.query = Some(query.clone());
        self.anchor = anchor;
        self.base = initial.clone();
        // Deltas applied but not yet refreshed belong to the view being replaced.
        self.pending.clear();
        Ok(())
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        // Deliberately unmerged: merging is refresh's job; see the type's doc comment.
        self.pending.extend(
            raw.iter()
                .map(|(row, w)| (table.to_string(), row.clone(), *w)),
        );
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        for (table, row, weight) in self.pending.drain(..) {
            self.base.entry(table).or_default().update(row, weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let query = self
            .query
            .as_ref()
            .ok_or_else(|| EngineError("materialize was called before create_view".into()))?;

        // Each aggregate slot is (running sum, count of non-NULL inputs). The
        // second is required: SUM over zero non-NULL inputs returns NULL, not 0
        // (spec §6.1, "the NULL-semantics contract for aggregates"). Keeping only
        // the running sum would silently output 0.
        let mut groups: BTreeMap<Vec<Value>, Vec<(i64, i64)>> = BTreeMap::new();

        let rows = self.input_rows(query);
        for (row, weight) in rows.iter() {
            if *weight <= 0 {
                continue;
            }
            if !passes(&query.predicate, row) {
                continue;
            }
            let key: Vec<Value> = query.group_by.iter().map(|i| row.get(*i).clone()).collect();
            let acc = groups
                .entry(key)
                .or_insert_with(|| vec![(0, 0); query.aggs.len()]);
            for (slot, agg) in acc.iter_mut().zip(&query.aggs) {
                match (agg.func, agg.column) {
                    (AggFn::Count, _) => slot.0 += weight,
                    (AggFn::Sum, Some(col)) => {
                        if let Value::Int(n) = row.get(col) {
                            slot.0 += n * weight;
                            slot.1 += weight;
                        }
                    }
                    (AggFn::Sum, None) => {
                        return Err(EngineError("SUM has no column".into()));
                    }
                }
            }
        }

        let mut out = ZSet::new();
        for (key, acc) in groups {
            let mut values = key;
            for (agg, (total, non_null)) in query.aggs.iter().zip(acc) {
                values.push(match agg.func {
                    // COUNT(*) counts rows, regardless of whether a column is NULL
                    AggFn::Count => Value::Int(total),
                    AggFn::Sum if non_null == 0 => Value::Null,
                    AggFn::Sum => Value::Int(total),
                });
            }
            out.update(Row::new(values), 1);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{join_on_k, kv, single_table_bases as single_table_case};
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
    use ivmlite_core::{Row, Value};

    fn schema() -> Schema {
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

    // A single-table case wrapped as a one-table `Database`, with initial state
    // keyed by table name — every existing single-table test cares only about
    // this one anchor table, and should stay this terse after the multi-table
    // change. This uses `test_support::single_table_bases` (m4: the
    // byte-identical version in buggy.rs was merged into it).

    fn sum_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        }
    }

    fn row(region: &str, amount: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(amount)])
    }

    fn out(region: Value, sum: i64, count: i64) -> Row {
        Row::new(vec![region, Value::Int(sum), Value::Int(count)])
    }

    #[test]
    fn joins_on_the_key_and_aggregates_the_joined_rows() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: &str, v: i64| Row::new(vec![Value::Text(k.into()), Value::Int(v)]);
        let bases = BTreeMap::from([
            (
                "t0".to_string(),
                ZSet::from_rows([(r("a", 1), 1), (r("b", 2), 1)]),
            ),
            (
                "t1".to_string(),
                ZSet::from_rows([(r("a", 10), 1), (r("a", 20), 1)]),
            ),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Int(2),
                Value::Int(30)
            ])),
            1
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn null_join_keys_never_match() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let null_key = |v: i64| Row::new(vec![Value::Null, Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(null_key(1), 1)])),
            ("t1".to_string(), ZSet::from_rows([(null_key(10), 1)])),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        assert!(e.materialize().unwrap().is_empty());
    }

    #[test]
    fn two_negative_rows_do_not_join_into_a_positive_one() {
        // Spec §5.1: a row with weight ≤ 0 takes no part in recomputation. Two
        // retractions of rows never inserted leave `("a", 2)` at -1 in t0 and
        // `("a", 10)` at -1 in t1; their product is +1, so skipping
        // non-positive weights only after joining would report a group "a".
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: &str, v: i64| Row::new(vec![Value::Text(k.into()), Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(r("a", 1), 1)])),
            ("t1".to_string(), ZSet::new()),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        e.apply("t0", &[(r("a", 2), -1)]).unwrap();
        e.apply("t1", &[(r("a", 10), -1)]).unwrap();
        e.refresh().unwrap();
        let got = e.materialize().unwrap();
        assert!(
            got.is_empty(),
            "t1 holds no row of positive weight, so the join is empty: {got:?}"
        );
    }

    #[test]
    fn recreating_a_view_discards_deltas_applied_but_not_refreshed() {
        // A reference engine reused across `create_view` calls must not carry
        // unrefreshed deltas into the new view. `create_view` replaces the query
        // and the base state, so anything still pending belongs to the old view:
        // letting it reach the next `refresh` would pollute every comparison the
        // reference engine is then used for. `IncrementalEngine` already gets this
        // right; this pins the reference engine to the same contract.
        let mut e = NaiveRecompute::new();
        let (db, bases) = single_table_case(&schema(), ZSet::new());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();
        e.apply("orders", &[(row("stale", 99), 1)]).unwrap();

        // Re-create the view from empty initial state, then refresh.
        let (db, bases) = single_table_case(&schema(), ZSet::new());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert!(
            got.is_empty(),
            "a delta applied before the view was re-created leaked into it: {got:?}"
        );
    }

    #[test]
    fn aggregates_initial_state() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1), (row("b", 3), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 15, 2)), 1);
        assert_eq!(got.weight_of(&out(Value::Text("b".into()), 3, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn applying_a_delete_updates_the_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        e.apply("orders", &[(row("a", 5), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 10, 1)), 1);
        assert_eq!(got.len(), 1, "the old (a,15,2) must disappear");
    }

    #[test]
    fn emptying_a_group_removes_it_entirely() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        e.apply("orders", &[(row("a", 10), -1)]).unwrap();
        e.refresh().unwrap();

        assert!(
            e.materialize().unwrap().is_empty(),
            "an empty group must leave no zombie row"
        );
    }

    #[test]
    fn null_forms_its_own_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Null, Value::Int(4)]), 1),
            (row("a", 1), 1),
        ]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Null, 4, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn predicate_filters_before_aggregating() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::Compare {
                column: 1,
                op: CmpOp::Gt,
                value: Value::Int(4),
            },
            join: None,
        };
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 1), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &q, &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
    }

    #[test]
    fn naive_passes_follows_three_valued_logic_for_not_equal_and_is_null() {
        let ne = Predicate::Compare {
            column: 0,
            op: CmpOp::Ne,
            value: Value::Int(4),
        };
        assert!(passes(&ne, &Row::new(vec![Value::Int(3)])));
        assert!(!passes(&ne, &Row::new(vec![Value::Int(4)])));
        assert!(
            !passes(&ne, &Row::new(vec![Value::Null])),
            "NULL != 4 is UNKNOWN"
        );
        let is_null = Predicate::IsNull { column: 0 };
        assert!(passes(&is_null, &Row::new(vec![Value::Null])));
        assert!(!passes(&is_null, &Row::new(vec![Value::Int(0)])));
    }

    /// Item 13 (a deferred minor, from the same source as I4): the `IsNotNull`
    /// predicate used to have no behavioural coverage — only its `to_sql`
    /// rendering, with nothing asserting it actually filters NULL rows out.
    #[test]
    fn is_not_null_predicate_filters_out_null_rows() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
            join: None,
        };
        let base = ZSet::from_rows([
            (row("a", 10), 1),
            (Row::new(vec![Value::Null, Value::Int(1)]), 1),
        ]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &q, &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
        assert_eq!(
            got.len(),
            1,
            "the NULL group must be filtered out by IsNotNull and not appear in the output"
        );
    }

    /// Spec §6.1's NULL-semantics contract. Note this differs from "the group is
    /// empty": the group is non-empty (COUNT(*) is positive) but the summed
    /// column is entirely NULL, and then SUM is NULL.
    #[test]
    fn sum_over_all_null_column_is_null_not_zero() {
        let nullable_amount = Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: false,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        };
        // The two identical rows are merged by the ZSet into weight 2
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
        ]);

        let mut e = NaiveRecompute::new();
        let (db, bases) = single_table_case(&nullable_amount, base.clone());
        e.create_view(&db, &q, &bases).unwrap();
        let got = e.materialize().unwrap();

        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SUM with no non-NULL input should be NULL, while COUNT(*) is still 2"
        );
    }

    /// Complements the previous test: here non-NULL inputs exist (and are
    /// non-zero), they just sum to exactly 0. If the emit branch's test were
    /// wrongly changed from `non_null == 0` to `total == 0`, this test would
    /// fail and no other would.
    #[test]
    fn sum_that_totals_zero_is_int_zero_not_null() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", -10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 0, 2)),
            1,
            "non-NULL inputs summing to exactly 0 should output Int(0), not Null"
        );
    }

    /// A pure retraction of a row never inserted (applying a delta of weight -1)
    /// leaves it in self.base with a negative weight. If materialize's
    /// `weight <= 0` guard were deleted, that negative-weight row would be
    /// aggregated as a real input row, and no other test would notice — both
    /// "delete" tests insert first and then retract to exactly 0, and the ZSet
    /// removes a row the moment its weight reaches zero, so materialize never
    /// even visits it.
    #[test]
    fn retracting_a_row_that_was_never_inserted_is_a_noop() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        // "b" never appeared in base; this retraction leaves it at weight -1 in self.base.
        e.apply("orders", &[(row("b", 999), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 10, 1)),
            1,
            "an unaffected group must stay unchanged"
        );
        assert_eq!(
            got.len(),
            1,
            "a phantom negative-weight row must produce no output and not affect other groups"
        );
    }
}
