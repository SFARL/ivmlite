use crate::{Arrangement, Row, Value, ZSet};

/// Spec §6.1's bilinear operator: a two-table inner equi-join.
///
/// Each side's current contents live in an `Arrangement` keyed by the join
/// key. **The key is a one-column `Row` holding the key value; the value is the
/// whole input row.** Both arrangements are passed in rather than created
/// here, so rebuilding a join from persisted state is a matter of passing
/// non-empty ones (M1a Phase 3, Ruling 2).
///
/// A row whose key is NULL is neither stored nor probed: SQL's `NULL = NULL`
/// is UNKNOWN, so such a row can never match (measured in SQLite; see the
/// plan's Ruling 4).
pub struct JoinState {
    left_key: usize,
    right_key: usize,
    left: Box<dyn Arrangement>,
    right: Box<dyn Arrangement>,
}

impl JoinState {
    pub fn new(
        left_key: usize,
        right_key: usize,
        left: Box<dyn Arrangement>,
        right: Box<dyn Arrangement>,
    ) -> Self {
        Self {
            left_key,
            right_key,
            left,
            right,
        }
    }

    /// Absorb one call's deltas from both inputs and return the join's output delta.
    ///
    /// Spec §6.1: `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS`. This computes it in two
    /// steps: `ΔR` is joined against `S` as it stood before this call and then
    /// folded into the left arrangement; `ΔS` is then joined against the
    /// **updated** left side, `R + ΔR`, which contributes both `R⋈ΔS` and
    /// `ΔR⋈ΔS`. Folding `ΔR` in before probing with `ΔS` is what carries the
    /// cross term — doing it afterwards silently drops it.
    ///
    /// The same argument makes the result independent of which table the
    /// engine refreshes first: across two calls, one per table, the two steps
    /// are exactly the two calls, in either order.
    pub fn absorb(&mut self, left_delta: &ZSet, right_delta: &ZSet) -> ZSet {
        let mut out = ZSet::new();

        // ΔR ⋈ S, against S from before this call.
        for (l, &wl) in left_delta.iter() {
            let Some(key) = join_key(l, self.left_key) else {
                continue;
            };
            for (r, wr) in self.right.get(&key) {
                out.update(concat(l, &r), wl * wr);
            }
        }
        for (l, &wl) in left_delta.iter() {
            if let Some(key) = join_key(l, self.left_key) {
                self.left.update(&key, l, wl);
            }
        }

        // (R + ΔR) ⋈ ΔS: R⋈ΔS and ΔR⋈ΔS together.
        for (r, &wr) in right_delta.iter() {
            let Some(key) = join_key(r, self.right_key) else {
                continue;
            };
            for (l, wl) in self.left.get(&key) {
                out.update(concat(&l, r), wl * wr);
            }
        }
        for (r, &wr) in right_delta.iter() {
            if let Some(key) = join_key(r, self.right_key) {
                self.right.update(&key, r, wr);
            }
        }

        out
    }
}

impl std::fmt::Debug for JoinState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Arrangement` is not `Debug`, and adding it as a supertrait
        // would impose it on M1b's shadow-table implementation for no benefit.
        f.debug_struct("JoinState")
            .field("left_key", &self.left_key)
            .field("right_key", &self.right_key)
            .finish_non_exhaustive()
    }
}

/// The arrangement key for `row`'s join column, or `None` when that column is
/// NULL — a NULL key never matches (SQL three-valued equality).
fn join_key(row: &Row, column: usize) -> Option<Row> {
    match row.get(column) {
        Value::Null => None,
        value => Some(Row::new(vec![value.clone()])),
    }
}

/// The joined row: the left row's columns, then the right row's.
fn concat(left: &Row, right: &Row) -> Row {
    let mut values = left.0.clone();
    values.extend(right.0.iter().cloned());
    Row::new(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemArrangement, Row, Value, ZSet};

    /// A `(k TEXT, v INTEGER)` row; `k = None` is a NULL key.
    fn kv(k: Option<&str>, v: i64) -> Row {
        Row::new(vec![
            k.map_or(Value::Null, |s| Value::Text(s.into())),
            Value::Int(v),
        ])
    }

    fn joined(left: &Row, right: &Row) -> Row {
        let mut values = left.0.clone();
        values.extend(right.0.iter().cloned());
        Row::new(values)
    }

    fn z(rows: &[(Row, i64)]) -> ZSet {
        ZSet::from_rows(rows.iter().cloned())
    }

    fn empty_join() -> JoinState {
        JoinState::new(
            0,
            0,
            Box::new(MemArrangement::new()),
            Box::new(MemArrangement::new()),
        )
    }

    #[test]
    fn delta_on_the_left_joins_against_earlier_right_state() {
        // The ΔR⋈S term: S arrived in an earlier call, ΔR arrives now.
        let mut j = empty_join();
        assert!(j
            .absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]))
            .is_empty());
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn delta_on_the_right_joins_against_earlier_left_state() {
        // The R⋈ΔS term.
        let mut j = empty_join();
        assert!(j
            .absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new())
            .is_empty());
        let out = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn both_deltas_in_one_call_include_the_delta_cross_term() {
        // Spec §6.1: Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS. Starting empty, the first
        // two terms are empty, so the only output can come from ΔR⋈ΔS. An
        // implementation that joins both deltas against the state from before
        // the call drops this term.
        let mut j = empty_join();
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &z(&[(kv(Some("a"), 10), 1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn null_keys_never_match() {
        // SQL: NULL = NULL is UNKNOWN, so a NULL key matches nothing — not even
        // another NULL. Measured in SQLite: an inner join on two NULL keys
        // yields no row.
        let mut j = empty_join();
        assert!(j.absorb(&ZSet::new(), &z(&[(kv(None, 10), 1)])).is_empty());
        assert!(j.absorb(&z(&[(kv(None, 1), 1)]), &ZSet::new()).is_empty());
        assert!(j
            .absorb(&z(&[(kv(None, 2), 1)]), &z(&[(kv(None, 20), 1)]))
            .is_empty());
    }

    #[test]
    fn output_weight_is_the_product_of_input_weights() {
        // Z-set join: a row of weight 2 matched with a row of weight 3 is 6
        // joined rows. The retraction of one left copy is then -3.
        let mut j = empty_join();
        j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 3)]));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 2)]), &ZSet::new());
        let row = joined(&kv(Some("a"), 1), &kv(Some("a"), 10));
        assert_eq!(out, z(&[(row.clone(), 6)]));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), -1)]), &ZSet::new());
        assert_eq!(out, z(&[(row, -3)]));
    }

    #[test]
    fn a_key_with_several_rows_on_the_other_side_matches_each() {
        // Spec §6.3: each side of a join is key → many rows, which is why
        // `Arrangement::get` returns an iterator rather than an `Option`.
        let mut j = empty_join();
        j.absorb(
            &ZSet::new(),
            &z(&[(kv(Some("a"), 10), 1), (kv(Some("a"), 20), 1)]),
        );
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[
                (joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1),
                (joined(&kv(Some("a"), 1), &kv(Some("a"), 20)), 1),
            ])
        );
    }

    #[test]
    fn output_row_is_left_columns_then_right_columns_on_both_paths() {
        // Every operator above the join indexes the joined row as "left
        // columns, then right columns", on the ΔR⋈S path and on the R⋈ΔS path.
        let mut j = empty_join();
        let from_right = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        assert!(from_right.is_empty());
        let from_left = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        let later_right = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 20), 1)]));
        assert_eq!(
            from_left,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
        assert_eq!(
            later_right,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 20)), 1)])
        );
    }

    #[test]
    fn retracting_a_row_retracts_its_join_results() {
        let mut j = empty_join();
        j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        let out = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), -1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), -1)])
        );
        // After the retraction the right side is empty again: a new left row
        // for key "a" matches nothing.
        assert!(j
            .absorb(&z(&[(kv(Some("a"), 2), 1)]), &ZSet::new())
            .is_empty());
    }

    /// A `(v INTEGER, k TEXT)` row: the key at column 1, not column 0.
    fn vk(v: i64, k: &str) -> Row {
        Row::new(vec![Value::Int(v), Value::Text(k.into())])
    }

    #[test]
    fn each_side_reads_its_own_key_column() {
        // Left rows `(k, v)` keyed at column 0, right rows `(v, k)` keyed at
        // column 1. Every other test joins column 0 to column 0, where reading
        // either side's key with the other side's index goes unnoticed. The
        // right row `(1, "b")` would match the left row `("a", 1)` if its key
        // were read from column 0 (`1 = 1`), so it pins that mistake too.
        let mut j = JoinState::new(
            0,
            1,
            Box::new(MemArrangement::new()),
            Box::new(MemArrangement::new()),
        );
        let from_right = j.absorb(
            &z(&[(kv(Some("a"), 1), 1)]),
            &z(&[(vk(10, "a"), 1), (vk(1, "b"), 1)]),
        );
        assert_eq!(
            from_right,
            z(&[(joined(&kv(Some("a"), 1), &vk(10, "a")), 1)])
        );
        let from_left = j.absorb(&z(&[(kv(Some("b"), 2), 1)]), &ZSet::new());
        assert_eq!(from_left, z(&[(joined(&kv(Some("b"), 2), &vk(1, "b")), 1)]));
        let later_right = j.absorb(&ZSet::new(), &z(&[(vk(20, "b"), 1)]));
        assert_eq!(
            later_right,
            z(&[(joined(&kv(Some("b"), 2), &vk(20, "b")), 1)])
        );
    }

    #[test]
    fn a_join_uses_the_arrangements_it_is_given() {
        // Ruling 2: the arrangements are passed in, so "rebuild from persisted
        // state" is only a matter of passing a non-empty one. The key layout is
        // part of that contract: a one-column Row holding the key value, with
        // the whole input row as the value.
        let mut right = MemArrangement::new();
        right.update(
            &Row::new(vec![Value::Text("a".into())]),
            &kv(Some("a"), 10),
            1,
        );
        let mut j = JoinState::new(0, 0, Box::new(MemArrangement::new()), Box::new(right));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }
}
