use std::collections::BTreeMap;

use crate::{Agg, AggFn, Arrangement, Row, Value, ZSet};

/// One agg's accumulator.
///
/// `Sum` must keep both `sum` and `non_null`: spec §6.1 states that an
/// implementation keeping only the running sum outputs `0` when "the group is
/// non-empty but the column is entirely NULL", where SQLite outputs `NULL` — and
/// that the disagreement is silent. `COUNT(*)` needs no accumulator (it is the
/// group's `rows`); its pair stays zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// Everything the operator knows about one group.
///
/// The row the group last emitted is **not** part of it: that row is a pure
/// function of this state (`output`), so it is recomputed from the old state
/// whenever it has to be retracted. That is what lets a tree rebuilt from its
/// arrangements retract what the previous tree emitted (M1b Phase 1, Ruling 1).
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupState {
    /// The total weight of the group's rows; `COUNT(*)` outputs exactly this.
    rows: i64,
    /// One accumulator per entry of `aggs`, in order.
    accs: Vec<Acc>,
}

impl GroupState {
    fn empty(aggs: usize) -> Self {
        GroupState {
            rows: 0,
            accs: vec![Acc::default(); aggs],
        }
    }

    fn is_empty(&self) -> bool {
        self.rows == 0 && self.accs.iter().all(|a| *a == Acc::default())
    }

    fn plus(&self, delta: &GroupState) -> GroupState {
        GroupState {
            rows: self.rows + delta.rows,
            accs: self
                .accs
                .iter()
                .zip(&delta.accs)
                .map(|(a, d)| Acc {
                    sum: a.sum + d.sum,
                    non_null: a.non_null + d.non_null,
                })
                .collect(),
        }
    }

    /// The stored form: `[rows, sum_0, non_null_0, sum_1, non_null_1, …]`, all
    /// `Value::Int`. The layout is fixed, so the encoding is canonical — the
    /// same state always encodes to the same row (spec §7).
    fn encode(&self) -> Row {
        let mut values = Vec::with_capacity(1 + 2 * self.accs.len());
        values.push(Value::Int(self.rows));
        for acc in &self.accs {
            values.push(Value::Int(acc.sum));
            values.push(Value::Int(acc.non_null));
        }
        Row::new(values)
    }

    /// The inverse of `encode`.
    ///
    /// # Panics
    /// If `row` does not have `encode`'s layout for `aggs` accumulators. The
    /// in-memory engine only ever stores rows `encode` produced; a state loaded
    /// from a persisted table may not be, and M1b Phase 3 turns this panic into
    /// an error (the plan's Ruling 3).
    fn decode(row: &Row, aggs: usize) -> GroupState {
        assert_eq!(
            row.len(),
            1 + 2 * aggs,
            "corrupted aggregate state: expected {} values, found {row:?}",
            1 + 2 * aggs
        );
        let int = |i: usize| match row.get(i) {
            Value::Int(n) => *n,
            other => panic!("corrupted aggregate state: value {i} is {other:?}, not an Int"),
        };
        GroupState {
            rows: int(0),
            accs: (0..aggs)
                .map(|i| Acc {
                    sum: int(1 + 2 * i),
                    non_null: int(2 + 2 * i),
                })
                .collect(),
        }
    }

    /// The row the group emits in this state, or `None` when it has no rows.
    fn output(&self, key: &Row, aggs: &[Agg]) -> Option<Row> {
        if self.rows <= 0 {
            return None;
        }
        let mut values: Vec<Value> = key.0.clone();
        for (acc, agg) in self.accs.iter().zip(aggs) {
            values.push(match agg.func {
                AggFn::Count => Value::Int(self.rows),
                AggFn::Sum if acc.non_null == 0 => Value::Null,
                AggFn::Sum => Value::Int(acc.sum),
            });
        }
        Some(Row::new(values))
    }
}

/// Spec §6.2's aggregate operator.
///
/// Its whole state lives in `state`, an `Arrangement` keyed by group key, whose
/// value for a group is that group's `GroupState::encode` with weight 1 — the
/// arrangement a SQLite provider will back with `__ivm_state_<view>_<op>`
/// (spec §7). The arrangement is passed in rather than created here, so a tree
/// can be rebuilt from persisted state (M1b Phase 1).
///
/// **The state must survive between batches**: the second batch can retract
/// the row the first one emitted only because the first batch's accumulators
/// are still in `state`. Pinned by
/// `aggregate_retracts_across_two_batches_through_the_same_node` in `node.rs`
/// and by `a_state_rebuilt_from_its_arrangement_retracts_what_the_old_one_emitted`.
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    state: Box<dyn Arrangement>,
}

impl std::fmt::Debug for AggState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Arrangement` is not `Debug` (see `JoinState`'s impl).
        f.debug_struct("AggState")
            .field("group_by", &self.group_by)
            .field("aggs", &self.aggs)
            .finish_non_exhaustive()
    }
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>, state: Box<dyn Arrangement>) -> AggState {
        AggState {
            group_by,
            aggs,
            state,
        }
    }

    /// Absorb one batch of input deltas, returning the delta this operator **emits**.
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // Phase one: fold the whole batch into one delta per touched group.
        // Emitting per input row would send out retraction pairs for
        // intermediate states, when only the batch's net change should be
        // visible. A `BTreeMap` rather than a `Vec` with a linear search keeps
        // a batch touching N groups at O(N log N) (external review P2-2), and
        // yields the groups in key order for the emit loop.
        let mut deltas: BTreeMap<Row, GroupState> = BTreeMap::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            let d = deltas
                .entry(key)
                .or_insert_with(|| GroupState::empty(self.aggs.len()));
            d.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                // `lower` rejects a SUM without a column
                // (`sum_without_a_column_is_rejected_at_the_boundary`), so a
                // tree built through `Node::build` never reaches this `expect`.
                let col = agg
                    .column
                    .expect("SUM must have a column; a tree built through lower was checked at the boundary");
                // A NULL input goes into neither sum nor non_null — where the
                // "all NULL outputs NULL" contract lands in the state.
                if let Value::Int(v) = row.get(col) {
                    d.accs[i].sum += v * w;
                    d.accs[i].non_null += w;
                }
            }
        }

        // Phase two: for each touched group, compare the output of its old
        // state with that of its new state, emit the difference, and store the
        // new state. Emitting nothing when the output is unchanged is not
        // observable in the returned `ZSet` (an equal -1/+1 pair cancels in
        // `ZSet::update`); see the matching n/a row in docs/mutation-gates.md.
        let mut out = ZSet::new();
        for (key, delta) in deltas {
            let old = self.load(&key);
            let new = old.plus(&delta);
            let old_out = old.output(&key, &self.aggs);
            let new_out = new.output(&key, &self.aggs);
            if new_out != old_out {
                if let Some(row) = old_out {
                    out.update(row, -1);
                }
                if let Some(row) = new_out {
                    out.update(row, 1);
                }
            }
            self.store(&key, &old, &new);
        }
        out
    }

    /// The group's stored state, or the empty state if it has none.
    fn load(&self, key: &Row) -> GroupState {
        let mut values = self.state.get(key);
        match (values.next(), values.next()) {
            (None, _) => GroupState::empty(self.aggs.len()),
            (Some((value, 1)), None) => GroupState::decode(&value, self.aggs.len()),
            (first, second) => panic!(
                "corrupted aggregate state for group {key:?}: expected at most one value \
                 of weight 1, found {first:?} and {second:?}"
            ),
        }
    }

    /// Replace the group's stored state `old` with `new`. A group whose state
    /// is all zero is not stored at all (spec §5.1: no zombie entries).
    fn store(&mut self, key: &Row, old: &GroupState, new: &GroupState) {
        if old == new {
            return;
        }
        if !old.is_empty() {
            self.state.update(key, &old.encode(), -1);
        }
        if !new.is_empty() {
            self.state.update(key, &new.encode(), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Mirrors;
    use crate::{AggFn, ArrangementId, ArrangementRole, MemArrangement, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    fn sum_state() -> AggState {
        // The group key is column 0; SUM is over column 1.
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
            Box::new(MemArrangement::new()),
        )
    }

    fn count_state() -> AggState {
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            Box::new(MemArrangement::new()),
        )
    }

    #[test]
    fn a_changed_sum_emits_a_retraction_pair_not_a_bare_insert() {
        // Spec §6.2: when SUM goes from 100 to 150 the emitted delta is
        // (key,100) w=-1 and (key,150) w=+1, not a single +1 row. This is IVM's
        // biggest source of bugs.
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));
        assert_eq!(first, ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ]),
            "the old output row must be retracted and the new one emitted"
        );
    }

    #[test]
    fn an_unchanged_group_emits_nothing() {
        // A group that is touched but whose **output** did not change should
        // not emit a (-1, +1) pair either.
        //
        // The input must be two **different** rows (one in, one out), not the
        // same row's +1/-1: the latter cancels to an empty set inside
        // `ZSet::from_rows`, so `absorb` would see no input at all, `deltas`
        // would be empty and the emit loop would never run — the test would
        // pass, for a reason unrelated to what it claims to guard.
        //
        // **This test does not guard the `new_out != old_out` check** (measured:
        // with that check always true this test stays green — the row retracted
        // and the row re-emitted are the same, and `ZSet::update` cancels them
        // exactly). What it really pins is that `COUNT(*)` equals the group's
        // total weight: with `d.rows += w` changed to `d.rows += 1` this batch's
        // row count goes from 2 to 4, the output changes, and this test goes
        // red. See the two matching rows in docs/mutation-gates.md.
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), int(2)]), 1),
        ]));
        // Swap one of the group's rows: the rows change but their count does not, so COUNT's output does not change.
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(3)]), 1),
            (row(vec![txt("a"), int(1)]), -1),
        ]));
        assert!(
            d.is_empty(),
            "a group touched but with an unchanged output must not emit: {d:?}"
        );
    }

    #[test]
    fn a_group_that_empties_is_retracted_and_not_replaced() {
        // Spec §5.2: a grouped aggregate returns 0 rows over an empty table
        // (unlike a global aggregate). When a group's count reaches zero, only
        // the retraction is emitted, with no new row.
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]),
            "retract only, emit no new row"
        );
    }

    #[test]
    fn sum_over_only_null_inputs_is_null_not_zero() {
        // Spec §6.1 (measured): when the group is non-empty but the column is
        // entirely NULL, the group appears, COUNT(*) is positive, and SUM is
        // NULL. An implementation keeping only the running sum outputs 0 and
        // silently disagrees with SQLite.
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), Value::Null]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)]),
            "SUM must be NULL, not Int(0)"
        );
    }

    #[test]
    fn sum_that_genuinely_totals_zero_is_int_zero_not_null() {
        // The counterpart to the previous test: with non-NULL inputs that sum to
        // exactly 0 it must be Int(0). An implementation that only asks "is the
        // sum 0" outputs NULL here.
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), int(-5)]), 1),
        ]));
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(0)]), 1)]));
    }

    #[test]
    fn a_group_whose_last_non_null_input_leaves_falls_back_to_null() {
        // When every non-NULL input is deleted but the group is still non-empty,
        // SUM must go from an Int back to NULL — a path only an implementation
        // keeping both sum and the non_null count gets right.
        let mut s = sum_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), -1),
                (row(vec![txt("a"), Value::Null]), 1),
            ]),
            "the group remains (its NULL row is still there), but SUM falls back to NULL"
        );
    }

    #[test]
    fn sum_scales_each_input_by_its_weight() {
        // SUM must accumulate `v * w`, not ignore the weight with `+= v`.
        //
        // Every SUM test above uses input weights of ±1, where `v * w` either
        // equals `v` or is masked by the "non_null reaches zero → output NULL"
        // rule, so the "ignore the weight" mutation left all of them green
        // (measured: with `+= v * w` changed to `+= v`, the whole suite stayed
        // green). This test closes that gap: one row of weight 3 must contribute
        // 15, and retracting one share of it must bring the sum back to 10 —
        // both steps happen while a non-NULL input is still present, so the NULL
        // rule cannot mask them. Weights above ±1 are reachable in practice:
        // consolidation merges duplicate rows within a batch.
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), 3)]));
        assert_eq!(
            first,
            ZSet::from_rows([(row(vec![txt("a"), int(15)]), 1)]),
            "a row of weight 3 contributes 5*3=15, not 5"
        );

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(15)]), -1),
                (row(vec![txt("a"), int(10)]), 1),
            ]),
            "retracting one share takes the sum from 15 to 10; an implementation ignoring weights would reach 20"
        );
    }

    #[test]
    fn each_agg_reads_its_own_accumulator() {
        // The aggs are deliberately ordered `[Count, Sum]`, so SUM sits at index
        // 1, not 0.
        //
        // Every test above has a single agg, under which `output`'s `zip` over
        // `self.accs` and reading `self.accs[0]` for every agg are identical —
        // measured before this test existed: making only the **read** side use
        // accumulator 0 left the whole suite green. The differential layer
        // cannot close the hole either: `enumerate` in
        // `crates/ivmlite-test/src/query.rs` produces only `[Sum(i)]` or
        // `[Sum(i), Count]`, with SUM always at index 0. But `lower` accepts
        // `[Count, Sum]` and `AggState::new` is public, so "accumulator indices
        // line up with agg indices" is guarded here, and otherwise only by the
        // tests built on the `[Count, Sum]` join fixture `join_on_k` (see the
        // matching row in docs/mutation-gates.md). Reading the wrong index
        // makes SUM silently NULL — exactly the class §6.1 names.
        let mut s = AggState::new(
            vec![0],
            vec![
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
            ],
            Box::new(MemArrangement::new()),
        );
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), int(7)]), 1),
        ]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(2), int(12)]), 1)]),
            "COUNT=2, SUM=12; reading the wrong accumulator index would make SUM silently NULL"
        );
    }

    #[test]
    fn groups_are_independent() {
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("b"), int(1)]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), -1),
                (row(vec![txt("a"), int(2)]), 1),
            ]),
            "only group a is affected; group b must not appear in the delta"
        );
    }

    #[test]
    fn a_null_group_key_is_a_group_like_any_other() {
        // NULL as a group key forms a group of its own under SQL's GROUP BY
        // (unlike WHERE's three-valued logic). NULL is frequent in the
        // differential tests' value domain, so this path is certain to be hit.
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
        assert_eq!(d, ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
    }

    #[test]
    fn emitted_output_weight_is_always_one() {
        // Spec §5.2: a group key maps to exactly one output row, and __w is
        // always 1 in the final output. Weights appear only in internal deltas
        // and operator state.
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 5)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(5)]), 1)]),
            "an input weight of 5 becomes one row with COUNT=5, emitted with weight 1, not 5"
        );
    }

    fn groups_id() -> ArrangementId {
        ArrangementId {
            node: 0,
            role: ArrangementRole::AggregateGroups,
        }
    }

    fn sum_state_on(state: Box<dyn Arrangement>) -> AggState {
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
            state,
        )
    }

    #[test]
    fn a_state_rebuilt_from_its_arrangement_retracts_what_the_old_one_emitted() {
        // The old state emits (a, 100). A new AggState built from nothing but
        // the arrangement's contents must, on the next batch, retract (a, 100)
        // — a row it never emitted itself. This works only if the emitted row
        // is derived from the stored accumulators rather than kept on the side
        // (the plan's Ruling 1).
        let mirrors = Mirrors::default();
        let mut old = sum_state_on(mirrors.arrangement(groups_id()));
        old.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let mut rebuilt = sum_state_on(Box::new(mirrors.snapshot(groups_id())));
        let d = rebuilt.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ])
        );
    }

    #[test]
    fn the_stored_state_is_one_value_per_group() {
        // Each group's state is a single value of weight 1: updating a group
        // must retract its old value, not add a second one.
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(2)]), 1)]));
        let stored: Vec<(Row, Row, i64)> = mirrors.snapshot(groups_id()).scan().collect();
        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(stored[0].0, row(vec![txt("a")]));
        assert_eq!(stored[0].2, 1);
    }

    #[test]
    fn a_group_that_empties_leaves_no_state_behind() {
        // Spec §5.1: no zombie entries. A group whose rows are all deleted must
        // disappear from the arrangement (and from the future shadow table).
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(mirrors.snapshot(groups_id()).scan().count(), 0);
    }

    #[test]
    #[should_panic(expected = "corrupted aggregate state")]
    fn a_group_with_two_stored_values_is_reported_as_corrupted() {
        // In memory this cannot happen; from a persisted table it can (M1b
        // Phase 3 turns this panic into an error, per the plan's Ruling 3).
        let mut state = MemArrangement::new();
        let key = row(vec![txt("a")]);
        state.update(&key, &row(vec![int(1), int(5), int(1)]), 1);
        state.update(&key, &row(vec![int(1), int(6), int(1)]), 1);
        let mut s = sum_state_on(Box::new(state));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
    }

    #[test]
    #[should_panic(expected = "corrupted aggregate state")]
    fn a_group_whose_one_stored_value_has_weight_two_is_reported_as_corrupted() {
        // One value, but with weight 2: `store` only ever writes a group's
        // state with weight 1, so this too is corrupted state, not a group
        // whose state counts twice. The previous test cannot see this case:
        // it has two values, and `load` rejects it for that alone.
        let mut state = MemArrangement::new();
        state.update(&row(vec![txt("a")]), &row(vec![int(1), int(5), int(1)]), 2);
        let mut s = sum_state_on(Box::new(state));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
    }
}
