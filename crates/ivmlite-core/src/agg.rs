use std::collections::{BTreeMap, BTreeSet};

use crate::{Agg, AggFn, Row, Value, ZSet};

/// One agg's accumulator.
///
/// `Sum` must keep both `sum` and `non_null`: spec §6.1 states that an
/// implementation keeping only the running sum outputs `0` when "the group is
/// non-empty but the column is entirely NULL", where SQLite outputs `NULL` — and
/// that the disagreement is silent.
#[derive(Debug, Clone, Default)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// One group's state.
#[derive(Debug, Clone)]
struct Group {
    /// The total weight of the group's rows. `COUNT(*)` outputs exactly this; at zero the group leaves the output.
    rows: i64,
    accs: Vec<Acc>,
    /// The row this group last emitted. Spec §6.2: an aggregate must remember
    /// what it emitted in order to retract it — the real reason aggregation
    /// needs state.
    emitted: Option<Row>,
}

/// Spec §6.2's aggregate operator state.
///
/// **This type must survive between batches**: `emitted` records "what was
/// last emitted", and only by keeping it across batches can the second batch
/// retract the row the first one emitted. Creating a fresh `AggState` per batch
/// (or taking a temporary copy of it in `Node::delta`) would emit only `+1` per
/// batch and never retract — the biggest source of bugs §6.2 speaks of. Pinned
/// by `aggregate_retracts_across_two_batches_through_the_same_node` in `node.rs`.
///
/// `groups` is a `BTreeMap` rather than a `HashMap`, but **not** because its
/// iteration order reaches the delta stream — measured, today it does not:
/// `absorb` returns a `ZSet`, itself a `BTreeMap`, and `groups` is never
/// iterated, only accessed by key through `entry` / `get_mut` / `remove`.
/// Replacing it (and `absorb`'s `touched` set) with hash containers leaves the
/// whole suite green. The ordered containers are kept so spec §9.4 still holds
/// once a downstream consumer takes an **ordered** delta sequence instead of a
/// `ZSet` — see the comment at `absorb`'s emit loop and the matching n/a row in
/// docs/mutation-gates.md.
#[derive(Debug)]
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    groups: BTreeMap<Row, Group>,
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState {
        AggState {
            group_by,
            aggs,
            groups: BTreeMap::new(),
        }
    }

    /// Absorb one batch of input deltas, returning the delta this operator **emits**.
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // First merge all of this batch's changes into the group state,
        // recording which groups were touched; emit everything afterwards. The
        // two phases are necessary: several rows in one batch can touch the same
        // group, and emitting per row would send out a string of retraction
        // pairs for intermediate states, when only the batch's net change should
        // be visible.
        //
        // `touched` is a `BTreeSet`, not a `Vec` with a `contains` check: the
        // linear scan made a batch touching N distinct groups cost O(N^2)
        // (measured in release: 67ms / 256ms / 1004ms for 10k / 20k / 40k groups).
        // The set also yields the keys in sorted order, which the emit loop needs.
        let mut touched: BTreeSet<Row> = BTreeSet::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            touched.insert(key.clone());
            let g = self.groups.entry(key).or_insert_with(|| Group {
                rows: 0,
                accs: vec![Acc::default(); self.aggs.len()],
                emitted: None,
            });
            g.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                // `lower` rejects a SUM without a column
                // (`sum_without_a_column_is_rejected_at_the_boundary`), so a
                // tree built through `Node::build` never reaches this `expect`.
                // `AggState::new` is public, and constructing one directly while
                // bypassing `lower` can still panic here — but that is the
                // caller skipping the boundary check, not a panic path the
                // engine leaves open after create_view.
                let col = agg
                    .column
                    .expect("SUM must have a column; a tree built through lower was checked at the boundary");
                if let Value::Int(v) = row.get(col) {
                    g.accs[i].sum += v * w;
                    g.accs[i].non_null += w;
                }
                // A NULL input goes into neither sum nor non_null — this is
                // where the "all NULL outputs NULL" contract lands in the state.
            }
        }

        let mut out = ZSet::new();
        // Emit in group-key order rather than arrival order (spec §9.4). The
        // order comes from iterating `touched`, a `BTreeSet`.
        //
        // **This ordering is unobservable today, measured**: `absorb` returns a
        // `ZSet`, which is a `BTreeMap` internally, so the order of `update`
        // calls on distinct rows does not affect its contents; and two distinct
        // groups always produce distinct output rows, because an output row
        // begins with its group key. Replacing the ordered containers with hash
        // containers leaves the whole suite green across repeated separate
        // process runs (the counts are in the gate row). The ordering is kept
        // because the moment a downstream consumer
        // takes an *ordered* delta sequence instead of a `ZSet`, it reaches the
        // output — see the matching n/a row in docs/mutation-gates.md.
        for key in touched {
            let Some(g) = self.groups.get_mut(&key) else {
                continue;
            };
            let new_out = if g.rows > 0 {
                let mut vals: Vec<Value> = key.0.clone();
                for (i, agg) in self.aggs.iter().enumerate() {
                    vals.push(match agg.func {
                        AggFn::Count => Value::Int(g.rows),
                        AggFn::Sum => {
                            if g.accs[i].non_null == 0 {
                                Value::Null
                            } else {
                                Value::Int(g.accs[i].sum)
                            }
                        }
                    });
                }
                Some(Row::new(vals))
            } else {
                None
            };

            // Emit nothing when `new_out == g.emitted`.
            //
            // **This test is unobservable today too, measured**: making it
            // always true leaves the whole suite green. When the output did not
            // change, the row retracted and the row re-emitted are **the same
            // row**, and `ZSet::update` cancels the `-1` and `+1` exactly and
            // removes the entry, so the extra pair leaves no trace in the return
            // value. In the current shape it is an optimization (it saves two
            // `BTreeMap` operations), not falsifiable semantics — see the
            // matching n/a row in docs/mutation-gates.md. Failing to emit when
            // something *should* be emitted is another matter entirely: that is
            // guarded by the `if let Some(old)` retraction below, whose removal
            // reddens several tests, among them node.rs's
            // `aggregate_retracts_across_two_batches_through_the_same_node`.
            if new_out != g.emitted {
                if let Some(old) = &g.emitted {
                    out.update(old.clone(), -1);
                }
                if let Some(new) = &new_out {
                    out.update(new.clone(), 1);
                }
                g.emitted = new_out;
            }

            // Spec §5.1: leave no zombie state once a group is completely empty.
            if g.rows == 0 && g.emitted.is_none() {
                self.groups.remove(&key);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggFn, Value, ZSet};

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
        )
    }

    fn count_state() -> AggState {
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
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
        // `ZSet::from_rows`, so `absorb` would see no input at all, `touched`
        // would be empty and the emit loop would never run — the test would
        // pass, for a reason unrelated to what it claims to guard.
        //
        // **This test does not guard the `new_out != emitted` check** (measured:
        // with that check always true this test stays green — the row retracted
        // and the row re-emitted are the same, and `ZSet::update` cancels them
        // exactly). What it really pins is that `COUNT(*)` equals the group's
        // total weight: with `g.rows += w` changed to `g.rows += 1` this batch's
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
        // Every test above has a single agg, under which `g.accs[i]` and
        // `g.accs[0]` are identical — measured: changing only the **read** side's
        // `g.accs[i]` to `g.accs[0]` left the whole suite green. The
        // differential layer cannot close the hole either: `enumerate` in
        // `crates/ivmlite-test/src/query.rs` produces only `[Sum(i)]` or
        // `[Sum(i), Count]`, with SUM always at index 0. But `lower` accepts
        // `[Count, Sum]` and `AggState::new` is public, so "accumulator indices
        // line up with agg indices" is guarded by this test alone. Reading the
        // wrong index makes SUM silently NULL — exactly the class §6.1 names.
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
}
