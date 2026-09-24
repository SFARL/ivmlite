use std::collections::BTreeMap;

use crate::Row;

/// Spec §6.3: key → many (value, weight) pairs.
///
/// `get` returns an iterator rather than an `Option`: v0's group-by stores one
/// value per key and has no use for many, but each side of a join is key → many
/// rows (§6.3 names this as the main place the rule "M0 may make no decision
/// that forces rework for join" lands).
///
/// `Box<dyn Iterator>` rather than RPITIT is for object safety: operators need
/// to hold a `dyn Arrangement` (M1b's implementation comes from
/// `ivmlite-sqlite`), and RPITIT would make the trait not object-safe, forcing
/// type parameters to propagate through the whole operator tree.
pub trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}

/// M1a's in-memory implementation. M1b adds one backed by SQLite shadow tables.
///
/// Both levels are `BTreeMap`s: spec §9.4 requires every iteration order that
/// can affect output to be deterministic, and `scan()`'s order feeds straight
/// into the delta stream.
#[derive(Debug, Clone, Default)]
pub struct MemArrangement {
    inner: BTreeMap<Row, BTreeMap<Row, i64>>,
}

impl MemArrangement {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Arrangement for MemArrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_> {
        match self.inner.get(key) {
            Some(vals) => Box::new(vals.iter().map(|(v, &w)| (v.clone(), w))),
            None => Box::new(std::iter::empty()),
        }
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) {
        if weight_delta == 0 {
            return;
        }
        let vals = self.inner.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        // Spec §5.1: remove on reaching zero, leaving no zombie entry. Once a
        // key's values are empty, remove the key too. Leaving it would not make
        // scan()'s output any larger — scan() expands each key's value set with
        // flat_map, and an empty set contributes zero records whether or not
        // its key is still in `inner`. What grows with history (every key used
        // and then emptied) rather than with the current state is `inner`'s own
        // entry count: a memory-footprint problem, not an output-size one, but
        // still worth fixing, or `inner` accumulates empty shells that are
        // never read again and scan()'s traversal cost only ever grows.
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                self.inner.remove(key);
            }
        }
    }

    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_> {
        Box::new(
            self.inner
                .iter()
                .flat_map(|(k, vals)| vals.iter().map(move |(v, &w)| (k.clone(), v.clone(), w))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn r(vals: Vec<i64>) -> Row {
        Row::new(vals.into_iter().map(Value::Int).collect())
    }

    #[test]
    fn one_key_can_hold_multiple_values() {
        // Spec §6.3: get returns an iterator, not an Option, because each side
        // of a join is key → many rows. v0's group-by has no use for it, but
        // the shape must hold now, which `JoinState` relies on.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![1]), &r(vec![20]), 3);
        let mut got: Vec<(Row, i64)> = a.get(&r(vec![1])).collect();
        got.sort();
        assert_eq!(got, vec![(r(vec![10]), 1), (r(vec![20]), 3)]);
    }

    #[test]
    fn weights_accumulate_and_zero_removes_the_entry() {
        // Spec §5.1: a row whose weight reaches zero must be removed, leaving no zombie entry.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 2);
        a.update(&r(vec![1]), &r(vec![10]), -2);
        assert_eq!(
            a.get(&r(vec![1])).count(),
            0,
            "no entry may remain once the weight reaches zero"
        );
        assert_eq!(a.scan().count(), 0, "scan must not see it either");
    }

    #[test]
    fn a_key_with_no_values_left_disappears_from_scan() {
        // Removing the (key, val) is not enough — the key itself must not be
        // left behind as an empty shell, or `inner`'s entry count grows with
        // history rather than with the current state.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![2]), &r(vec![20]), 1);
        a.update(&r(vec![1]), &r(vec![10]), -1);
        let keys: Vec<Row> = a.scan().map(|(k, _, _)| k).collect();
        assert_eq!(keys, vec![r(vec![2])], "an emptied key must disappear");
        // White-box check: with only the
        // `if vals.is_empty() { self.inner.remove(key); }` line deleted, the
        // scan() assertion above does not go red — scan()'s flat_map yields
        // zero records for an empty inner BTreeMap whether or not its key is
        // still in `inner`. What grows with history is `inner`'s own entry
        // count (every key used and emptied would hold a slot forever as an
        // empty shell), and that can only be seen by inspecting internal state.
        assert!(
            !a.inner.contains_key(&r(vec![1])),
            "an emptied key must not stay in inner as an empty shell"
        );
    }

    #[test]
    fn negative_weights_are_representable() {
        // Negative weights are legal in intermediate deltas (spec §5.1); only the final materialized result must not have them.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), -5);
        assert_eq!(
            a.get(&r(vec![1])).collect::<Vec<_>>(),
            vec![(r(vec![10]), -5)]
        );
    }

    /// Spec §9.4: a failing case must replay exactly from its seed, so every
    /// iteration that can affect output must come from an ordered container.
    ///
    /// This is still only a statistical guard, not a proof: if `MemArrangement`
    /// replaced its outer `BTreeMap` with a `HashMap`, nothing outside the type
    /// could pin "the order is deterministic" as a certainty — `HashMap`'s
    /// `RandomState` re-seeds on every construction, so any run might happen to
    /// give the right order. The chance for 3 keys is 1/3! ≈ 16.7%, and a few
    /// separate process runs filter the coincidence out (see the matching row in
    /// `docs/mutation-gates.md`: 15 separate process runs, all red). The
    /// insertion order `[3, 1, 2]` is deliberately neither lexicographic nor its
    /// reverse, so an implementation that "looks order-preserving but sorts by
    /// value internally" is caught as well.
    #[test]
    fn scan_order_is_deterministic() {
        let build = || {
            let mut a = MemArrangement::new();
            for k in [3, 1, 2] {
                for v in [30, 10, 20] {
                    a.update(&r(vec![k]), &r(vec![v]), 1);
                }
            }
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
        let keys: Vec<Row> = build().into_iter().map(|(k, _, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "scan must be ordered by key");
    }

    #[test]
    fn scan_order_is_independent_of_update_history() {
        // Spec §9.4: `scan_order_is_deterministic` checks only that "the same
        // code path replayed twice agrees with itself" and that keys are sorted.
        // It cannot see something more fundamental: whether two
        // `MemArrangement`s that reach exactly the same final state by
        // different update orders (even one with a detour that inserts and then
        // retracts) give element-for-element identical `scan()` output. If they
        // do not, the delta stream cannot be replayed exactly from the final
        // state and a seed, because it secretly depends on how the state was
        // reached too.
        let ascending = {
            let mut a = MemArrangement::new();
            a.update(&r(vec![1]), &r(vec![10]), 1);
            a.update(&r(vec![1]), &r(vec![20]), 1);
            a.update(&r(vec![1]), &r(vec![30]), 1);
            a.scan().collect::<Vec<_>>()
        };
        let descending_with_a_detour = {
            let mut a = MemArrangement::new();
            a.update(&r(vec![1]), &r(vec![30]), 1);
            // Insert and then retract an unrelated value, so the two paths are
            // not merely reversed insertion sequences but genuinely different
            // histories (a container that appends in insertion order leaves or
            // frees a slot in the middle at the retraction, moving further from
            // the simple "just reversed" case).
            a.update(&r(vec![1]), &r(vec![5]), 1);
            a.update(&r(vec![1]), &r(vec![5]), -1);
            a.update(&r(vec![1]), &r(vec![20]), 1);
            a.update(&r(vec![1]), &r(vec![10]), 1);
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(
            ascending, descending_with_a_detour,
            "scan()'s output must not depend on the path taken to reach the state"
        );
    }

    #[test]
    fn get_on_a_missing_key_is_empty_not_a_panic() {
        let a = MemArrangement::new();
        assert_eq!(a.get(&r(vec![99])).count(), 0);
    }

    #[test]
    fn mem_arrangement_is_usable_as_a_trait_object() {
        // Spec §6.3's implementation note: the trait must be object-safe, or
        // type parameters are forced through the operator tree. This test is
        // that constraint's compile-time gate.
        let mut a: Box<dyn Arrangement> = Box::new(MemArrangement::new());
        a.update(&r(vec![1]), &r(vec![10]), 1);
        assert_eq!(a.get(&r(vec![1])).count(), 1);
    }
}
