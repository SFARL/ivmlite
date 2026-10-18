use std::collections::BTreeMap;

use crate::{
    numeric_term, Agg, AggFn, Arrangement, ExactSum, NumericTerm, Row, StateError, Value, ZSet,
    EXACT_BYTES,
};

/// M2c spec §4: the refresh fails with this when a SUM over integers only
/// ends outside `i64`.
pub const SUM_OVERFLOW: &str = "integer overflow in SUM";

/// The bytes of a stored `Approx`: count, +Inf count, -Inf count, then the limbs.
const APPROX_BYTES: usize = 24 + EXACT_BYTES;

/// The bytes of a stored `int_sum` that does not fit `i64`.
const WIDE_INT_BYTES: usize = 16;

/// One SUM/TOTAL/AVG argument's non-integer inputs (M2c spec §4), present
/// only while `count != 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Approx {
    /// The spec's `approx`: inputs classified as non-integer, by weight.
    count: i64,
    /// ±Inf inputs, by weight; not in `frac` (M2c spec amendment 5).
    pos_inf: i64,
    neg_inf: i64,
    /// The spec's `frac`: the exact sum of the finite non-integer inputs.
    frac: ExactSum,
}

impl Approx {
    fn zero() -> Approx {
        Approx {
            count: 0,
            pos_inf: 0,
            neg_inf: 0,
            frac: ExactSum::zero(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(APPROX_BYTES);
        for n in [self.count, self.pos_inf, self.neg_inf] {
            out.extend_from_slice(&n.to_be_bytes());
        }
        out.extend_from_slice(&self.frac.to_be_bytes());
        out
    }

    /// The inverse of `to_bytes`; `None` for any other length or limbs no sum
    /// of finite terms reaches.
    fn from_bytes(bytes: &[u8]) -> Option<Approx> {
        if bytes.len() != APPROX_BYTES {
            return None;
        }
        let int =
            |i: usize| i64::from_be_bytes(bytes[8 * i..8 * i + 8].try_into().expect("8 bytes"));
        Some(Approx {
            count: int(0),
            pos_inf: int(1),
            neg_inf: int(2),
            frac: ExactSum::from_be_bytes(&bytes[24..])?,
        })
    }
}

/// One aggregate's accumulator (M2c spec §4). `COUNT(*)` uses none of it
/// (it is the group's `rows`); `COUNT(x)` uses only `non_null`.
///
/// SUM must keep `non_null` beside its sums: spec §6.1 states that an
/// implementation keeping only the running sum outputs `0` when "the group is
/// non-empty but the column is entirely NULL", where SQLite outputs `NULL` —
/// and that the disagreement is silent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    non_null: i64,
    /// The spec's `int_sum`: inputs classified as integers, exactly.
    int_sum: i128,
    /// Boxed, so an integer-only accumulator stays small.
    approx: Option<Box<Approx>>,
}

fn accumulator_overflow() -> StateError {
    StateError("aggregate accumulator overflow: the exact integer sum left the i128 range".into())
}

impl Acc {
    /// Fold one SUM/TOTAL/AVG input of weight `w` into a delta.
    fn absorb_term(&mut self, value: &Value, w: i64) -> Result<(), StateError> {
        // A NULL input counts nowhere — where the "all NULL outputs NULL"
        // contract lands in the state.
        let Some(term) = numeric_term(value) else {
            return Ok(());
        };
        self.non_null += w;
        match term {
            NumericTerm::Integer(n) => {
                self.int_sum = self
                    .int_sum
                    .checked_add(i128::from(n) * i128::from(w))
                    .ok_or_else(accumulator_overflow)?;
            }
            NumericTerm::Approx(x) => {
                let a = self.approx.get_or_insert_with(|| Box::new(Approx::zero()));
                a.count += w;
                // `ExactSum` holds finite values only (M2c spec amendment 5).
                if x == f64::INFINITY {
                    a.pos_inf += w;
                } else if x == f64::NEG_INFINITY {
                    a.neg_inf += w;
                } else {
                    a.frac.add(x, w);
                }
            }
        }
        Ok(())
    }

    fn plus(&self, d: &Acc) -> Result<Acc, StateError> {
        let int_sum = self
            .int_sum
            .checked_add(d.int_sum)
            .ok_or_else(accumulator_overflow)?;
        let approx = match (&self.approx, &d.approx) {
            (None, None) => None,
            (a, b) => {
                let mut sum = a.clone().unwrap_or_else(|| Box::new(Approx::zero()));
                if let Some(b) = b {
                    sum.count += b.count;
                    sum.pos_inf += b.pos_inf;
                    sum.neg_inf += b.neg_inf;
                    sum.frac.add_sum(&b.frac);
                }
                // M2c spec §4: with every non-integer input retracted, frac is
                // exactly zero and is dropped.
                if sum.count == 0 {
                    if sum.pos_inf != 0 || sum.neg_inf != 0 || !sum.frac.is_zero() {
                        return Err(StateError(
                            "corrupted aggregate state: no non-integer input is live, but \
                             their sum is not zero"
                                .into(),
                        ));
                    }
                    None
                } else {
                    Some(sum)
                }
            }
        };
        Ok(Acc {
            non_null: self.non_null + d.non_null,
            int_sum,
            approx,
        })
    }

    /// The REAL result of `finish` over the exact sum `int_sum + frac`, after
    /// the infinities (M2c spec amendment 5).
    fn approx_value(&self, finish: impl Fn(&ExactSum) -> f64) -> Value {
        let (pos, neg) = self
            .approx
            .as_ref()
            .map_or((0, 0), |a| (a.pos_inf, a.neg_inf));
        match (pos > 0, neg > 0) {
            // +Inf + -Inf is NaN, which SQLite returns as NULL.
            (true, true) => Value::Null,
            (true, false) => Value::Real(f64::INFINITY),
            (false, true) => Value::Real(f64::NEG_INFINITY),
            (false, false) => {
                let mut exact = self
                    .approx
                    .as_ref()
                    .map_or_else(ExactSum::zero, |a| a.frac.clone());
                exact.add_int(self.int_sum);
                Value::Real(finish(&exact))
            }
        }
    }
}

/// How many values `agg`'s accumulator stores (M2c spec §4): none for
/// `COUNT(*)`, `[non_null]` for `COUNT(x)`, `[non_null, int_sum, approx]`
/// for SUM, TOTAL and AVG.
fn width(agg: &Agg) -> usize {
    match (agg.func, agg.column) {
        (AggFn::Count, None) => 0,
        (AggFn::Count, Some(_)) => 1,
        (AggFn::Sum | AggFn::Total | AggFn::Avg, _) => 3,
    }
}

/// The stored form of `int_sum`: an `Int` when it fits `i64`, else 16
/// big-endian two's-complement bytes. Only one form per value, so the
/// encoding stays canonical.
fn int_sum_value(n: i128) -> Value {
    match i64::try_from(n) {
        Ok(n) => Value::Int(n),
        Err(_) => Value::Blob(n.to_be_bytes().to_vec()),
    }
}

fn decode_int_sum(value: &Value) -> Option<i128> {
    match value {
        Value::Int(n) => Some(i128::from(*n)),
        Value::Blob(bytes) if bytes.len() == WIDE_INT_BYTES => {
            let n = i128::from_be_bytes(bytes.as_slice().try_into().expect("16 bytes"));
            i64::try_from(n).is_err().then_some(n)
        }
        _ => None,
    }
}

/// The stored form of `approx`: NULL while no non-integer input is live.
fn approx_value(approx: Option<&Approx>) -> Value {
    approx.map_or(Value::Null, |a| Value::Blob(a.to_bytes()))
}

fn decode_approx(value: &Value) -> Option<Option<Box<Approx>>> {
    match value {
        Value::Null => Some(None),
        Value::Blob(bytes) => {
            let a = Approx::from_bytes(bytes)?;
            (a.count != 0).then(|| Some(Box::new(a)))
        }
        _ => None,
    }
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
    /// One count per group-by column, in order: the total weight of the
    /// group's rows whose key value in that column was a `Real` (M2c spec §2,
    /// "The type a group key is output as"). Kept for every column, since core
    /// knows no affinity; a TEXT column's count simply stays 0.
    real_keys: Vec<i64>,
    /// One accumulator per entry of `aggs`, in order.
    accs: Vec<Acc>,
}

impl GroupState {
    fn empty(keys: usize, aggs: usize) -> Self {
        GroupState {
            rows: 0,
            real_keys: vec![0; keys],
            accs: vec![Acc::default(); aggs],
        }
    }

    fn is_empty(&self) -> bool {
        self.rows == 0
            && self.real_keys.iter().all(|&c| c == 0)
            && self.accs.iter().all(|a| *a == Acc::default())
    }

    fn plus(&self, delta: &GroupState) -> Result<GroupState, StateError> {
        Ok(GroupState {
            rows: self.rows + delta.rows,
            real_keys: self
                .real_keys
                .iter()
                .zip(&delta.real_keys)
                .map(|(a, d)| a + d)
                .collect(),
            accs: self
                .accs
                .iter()
                .zip(&delta.accs)
                .map(|(a, d)| a.plus(d))
                .collect::<Result<_, _>>()?,
        })
    }

    /// The stored form: `[rows, real_keys, …]`, then each aggregate's values
    /// in order (`width`): nothing for `COUNT(*)`, `[non_null]` for
    /// `COUNT(x)`, `[non_null, int_sum, approx]` for SUM, TOTAL and AVG, with
    /// `int_sum` as `int_sum_value` and `approx` as `approx_value` write them.
    /// The layout is fixed and every value has one form, so the encoding is
    /// canonical — the same state always encodes to the same row (spec §7).
    fn encode(&self, aggs: &[Agg]) -> Row {
        let mut values = Vec::with_capacity(2 + aggs.iter().map(width).sum::<usize>());
        values.push(Value::Int(self.rows));
        values.push(real_keys_value(&self.real_keys));
        for (acc, agg) in self.accs.iter().zip(aggs) {
            match width(agg) {
                0 => {}
                1 => values.push(Value::Int(acc.non_null)),
                _ => {
                    values.push(Value::Int(acc.non_null));
                    values.push(int_sum_value(acc.int_sum));
                    values.push(approx_value(acc.approx.as_deref()));
                }
            }
        }
        Row::new(values)
    }

    /// The inverse of `encode`, for `keys` group-by columns and `aggs`.
    ///
    /// # Errors
    /// If `row` is not a row `encode` can produce for that shape. The
    /// in-memory engine only ever stores rows `encode` produced; a state loaded
    /// from a persisted table may not be (M1b Phase 3a). That includes a
    /// second encoding of one state: a `real_keys` BLOB of all-zero counts, a
    /// 16-byte `int_sum` that fits `i64`, or an `approx` whose count is 0.
    fn decode(row: &Row, keys: usize, aggs: &[Agg]) -> Result<GroupState, StateError> {
        let expected = 2 + aggs.iter().map(width).sum::<usize>();
        if row.len() != expected {
            return Err(StateError(format!(
                "corrupted aggregate state: expected {expected} values, found {row:?}"
            )));
        }
        let int = |i: usize| match row.get(i) {
            Value::Int(n) => Ok(*n),
            other => Err(StateError(format!(
                "corrupted aggregate state: value {i} is {other:?}, not an Int"
            ))),
        };
        let mut accs = Vec::with_capacity(aggs.len());
        let mut next = 2;
        for agg in aggs {
            let mut acc = Acc::default();
            match width(agg) {
                0 => {}
                1 => acc.non_null = int(next)?,
                _ => {
                    acc.non_null = int(next)?;
                    acc.int_sum = decode_int_sum(row.get(next + 1)).ok_or_else(|| {
                        StateError(format!(
                            "corrupted aggregate state: value {} is {:?}, not an integer sum",
                            next + 1,
                            row.get(next + 1)
                        ))
                    })?;
                    acc.approx = decode_approx(row.get(next + 2)).ok_or_else(|| {
                        StateError(format!(
                            "corrupted aggregate state: value {} is {:?}, not a non-integer sum",
                            next + 2,
                            row.get(next + 2)
                        ))
                    })?;
                }
            }
            next += width(agg);
            accs.push(acc);
        }
        Ok(GroupState {
            rows: int(0)?,
            real_keys: decode_real_keys(row.get(1), keys)?,
            accs,
        })
    }

    /// The row the group emits in this state, or `None` when it has no rows.
    ///
    /// # Errors
    /// `SUM_OVERFLOW` when a SUM over integers only ends outside `i64` (M2c
    /// spec §4).
    fn output(&self, key: &Row, aggs: &[Agg]) -> Result<Option<Row>, StateError> {
        if self.rows <= 0 {
            return Ok(None);
        }
        let mut values: Vec<Value> = key
            .0
            .iter()
            .zip(&self.real_keys)
            .map(|(value, &reals)| match value {
                // M2c spec §2: SQLite outputs an arbitrary row's key; a group
                // with any REAL key row outputs REAL.
                Value::Int(i) if reals > 0 => Value::Real(*i as f64),
                other => other.clone(),
            })
            .collect();
        for (acc, agg) in self.accs.iter().zip(aggs) {
            values.push(match agg.func {
                AggFn::Count => match agg.column {
                    None => Value::Int(self.rows),
                    Some(_) => Value::Int(acc.non_null),
                },
                AggFn::Sum if acc.non_null == 0 => Value::Null,
                AggFn::Sum => match &acc.approx {
                    // The `approx` switch (M2c spec §4): integers only, so an INTEGER.
                    None => Value::Int(
                        i64::try_from(acc.int_sum).map_err(|_| StateError(SUM_OVERFLOW.into()))?,
                    ),
                    Some(_) => acc.approx_value(ExactSum::round),
                },
                AggFn::Total if acc.non_null == 0 => Value::Real(0.0),
                AggFn::Total => acc.approx_value(ExactSum::round),
                AggFn::Avg if acc.non_null == 0 => Value::Null,
                AggFn::Avg if acc.non_null < 0 => {
                    return Err(StateError(format!(
                        "corrupted aggregate state: AVG's non-NULL count is {}",
                        acc.non_null
                    )))
                }
                AggFn::Avg => acc.approx_value(|s| s.div_round(acc.non_null)),
            });
        }
        Ok(Some(Row::new(values)))
    }
}

/// The stored form of `GroupState::real_keys`: NULL when no key row was REAL,
/// so integer workloads pay one byte, else one big-endian `i64` per group-by
/// column (M2c spec §2, "The type a group key is output as").
fn real_keys_value(counts: &[i64]) -> Value {
    if counts.iter().all(|&c| c == 0) {
        Value::Null
    } else {
        Value::Blob(counts.iter().flat_map(|c| c.to_be_bytes()).collect())
    }
}

/// The inverse of `real_keys_value` for `keys` group-by columns. Only the two
/// forms it writes are accepted, so the stored state stays canonical.
fn decode_real_keys(value: &Value, keys: usize) -> Result<Vec<i64>, StateError> {
    let corrupted = || {
        StateError(format!(
            "corrupted aggregate state: the REAL key counts are {value:?}"
        ))
    };
    match value {
        Value::Null => Ok(vec![0; keys]),
        Value::Blob(bytes) if bytes.len() == 8 * keys => {
            let counts: Vec<i64> = bytes
                .chunks_exact(8)
                .map(|chunk| i64::from_be_bytes(chunk.try_into().expect("an 8-byte chunk")))
                .collect();
            if counts.iter().all(|&c| c == 0) {
                Err(corrupted())
            } else {
                Ok(counts)
            }
        }
        _ => Err(corrupted()),
    }
}

/// Spec §6.2's aggregate operator.
///
/// Its whole state lives in `state`, an `Arrangement` keyed by the
/// **canonical** group key (`Value::canonical`, M2c spec §2: SQLite's GROUP BY
/// equality, so `1` and `1.0` are one group), whose
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
    pub fn absorb(&mut self, input: &ZSet) -> Result<ZSet, StateError> {
        // Phase one: fold the whole batch into one delta per touched group.
        // Emitting per input row would send out retraction pairs for
        // intermediate states, when only the batch's net change should be
        // visible. A `BTreeMap` rather than a `Vec` with a linear search keeps
        // a batch touching N groups at O(N log N) (external review P2-2), and
        // yields the groups in key order for the emit loop.
        let mut deltas: BTreeMap<Row, GroupState> = BTreeMap::new();
        for (row, &w) in input.iter() {
            let key = Row::new(
                self.group_by
                    .iter()
                    .map(|&c| row.get(c).canonical())
                    .collect(),
            );
            let d = deltas
                .entry(key)
                .or_insert_with(|| GroupState::empty(self.group_by.len(), self.aggs.len()));
            d.rows += w;
            for (j, &c) in self.group_by.iter().enumerate() {
                if matches!(row.get(c), Value::Real(_)) {
                    d.real_keys[j] += w;
                }
            }
            for (i, agg) in self.aggs.iter().enumerate() {
                // `lower` gives every aggregate but COUNT(*) a column.
                let Some(col) = agg.column else { continue };
                let value = row.get(col);
                match agg.func {
                    AggFn::Count => {
                        if *value != Value::Null {
                            d.accs[i].non_null += w;
                        }
                    }
                    AggFn::Sum | AggFn::Total | AggFn::Avg => d.accs[i].absorb_term(value, w)?,
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
            let old = self.load(&key)?;
            let new = old.plus(&delta)?;
            let old_out = old.output(&key, &self.aggs)?;
            let new_out = new.output(&key, &self.aggs)?;
            if new_out != old_out {
                if let Some(row) = old_out {
                    out.update(row, -1);
                }
                if let Some(row) = new_out {
                    out.update(row, 1);
                }
            }
            self.store(&key, &old, &new)?;
        }
        Ok(out)
    }

    /// The group's stored state, or the empty state if it has none.
    fn load(&self, key: &Row) -> Result<GroupState, StateError> {
        match self.state.get(key)?.as_slice() {
            [] => Ok(GroupState::empty(self.group_by.len(), self.aggs.len())),
            [(value, 1)] => GroupState::decode(value, self.group_by.len(), &self.aggs),
            values => Err(StateError(format!(
                "corrupted aggregate state for group {key:?}: expected at most one value \
                 of weight 1, found {values:?}"
            ))),
        }
    }

    /// Replace the group's stored state `old` with `new`. A group whose state
    /// is all zero is not stored at all (spec §5.1: no zombie entries).
    ///
    /// The retraction and the insertion are two writes; if the second fails,
    /// the arrangement is left without this group's state. Callers that
    /// persist state must therefore make a failed batch roll back as a whole —
    /// `ivmlite-sqlite`'s refresh runs inside one savepoint (M1b Phase 3a).
    fn store(&mut self, key: &Row, old: &GroupState, new: &GroupState) -> Result<(), StateError> {
        if old == new {
            return Ok(());
        }
        if !old.is_empty() {
            self.state.update(key, &old.encode(&self.aggs), -1)?;
        }
        if !new.is_empty() {
            self.state.update(key, &new.encode(&self.aggs), 1)?;
        }
        Ok(())
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
        let first = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]))
            .unwrap();
        assert_eq!(first, ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let second = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]))
            .unwrap();
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
        ]))
        .unwrap();
        // Swap one of the group's rows: the rows change but their count does not, so COUNT's output does not change.
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(3)]), 1),
                (row(vec![txt("a"), int(1)]), -1),
            ]))
            .unwrap();
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
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .unwrap();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]))
            .unwrap();
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
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), Value::Null]), 1),
                (row(vec![txt("a"), Value::Null]), 1),
            ]))
            .unwrap();
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
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), 1),
                (row(vec![txt("a"), int(-5)]), 1),
            ]))
            .unwrap();
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
        ]))
        .unwrap();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]))
            .unwrap();
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
        let first = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), 3)]))
            .unwrap();
        assert_eq!(
            first,
            ZSet::from_rows([(row(vec![txt("a"), int(15)]), 1)]),
            "a row of weight 3 contributes 5*3=15, not 5"
        );

        let second = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]))
            .unwrap();
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
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), 1),
                (row(vec![txt("a"), int(7)]), 1),
            ]))
            .unwrap();
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
        ]))
        .unwrap();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .unwrap();
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
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]))
            .unwrap();
        assert_eq!(d, ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
    }

    #[test]
    fn emitted_output_weight_is_always_one() {
        // Spec §5.2: a group key maps to exactly one output row, and __w is
        // always 1 in the final output. Weights appear only in internal deltas
        // and operator state.
        let mut s = count_state();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 5)]))
            .unwrap();
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
        let mut old = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
        old.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]))
            .unwrap();

        let mut rebuilt = sum_state_on(Box::new(mirrors.snapshot(groups_id())));
        let d = rebuilt
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]))
            .unwrap();
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
        let mut s = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .unwrap();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(2)]), 1)]))
            .unwrap();
        let stored: Vec<(Row, Row, i64)> = mirrors.snapshot(groups_id()).scan().unwrap();
        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(stored[0].0, row(vec![txt("a")]));
        assert_eq!(stored[0].2, 1);
    }

    #[test]
    fn a_group_that_empties_leaves_no_state_behind() {
        // Spec §5.1: no zombie entries. A group whose rows are all deleted must
        // disappear from the arrangement (and from the future shadow table).
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .unwrap();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]))
            .unwrap();
        assert_eq!(mirrors.snapshot(groups_id()).scan().unwrap().len(), 0);
    }

    #[test]
    fn a_group_with_two_stored_values_is_reported_as_corrupted() {
        // In memory this cannot happen; from a persisted table it can, so it
        // is a `StateError`, not a panic (M1b Phase 3a).
        let mut state = MemArrangement::new();
        let key = row(vec![txt("a")]);
        state
            .update(
                &key,
                &row(vec![int(1), Value::Null, int(1), int(5), Value::Null]),
                1,
            )
            .unwrap();
        state
            .update(
                &key,
                &row(vec![int(1), Value::Null, int(1), int(6), Value::Null]),
                1,
            )
            .unwrap();
        let mut s = sum_state_on(Box::new(state));
        let err = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .expect_err("two stored values for one group is corrupted state");
        assert!(err.0.contains("corrupted aggregate state"), "{err}");
    }
    #[test]
    fn a_group_whose_one_stored_value_has_weight_two_is_reported_as_corrupted() {
        // One value, but with weight 2: `store` only ever writes a group's
        // state with weight 1, so this too is corrupted state, not a group
        // whose state counts twice. The previous test cannot see this case:
        // it has two values, and `load` rejects it for that alone.
        let mut state = MemArrangement::new();
        state
            .update(
                &row(vec![txt("a")]),
                &row(vec![int(1), Value::Null, int(1), int(5), Value::Null]),
                2,
            )
            .unwrap();
        let mut s = sum_state_on(Box::new(state));
        let err = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .expect_err("a stored value of weight 2 is corrupted state");
        assert!(err.0.contains("corrupted aggregate state"), "{err}");
    }
    #[test]
    fn a_stored_value_of_the_wrong_shape_is_reported_as_corrupted() {
        // What a shadow table written by another plan, or damaged, could hold:
        // too few values, a TEXT where an accumulator should be, or REAL key
        // counts that are not one big-endian i64 per group-by column. All-zero
        // counts are stored as NULL, so a BLOB of zeros or an Int(0) in their
        // place is not a second encoding of the same state but corruption.
        // Likewise an `int_sum` that fits i64 is stored as an Int, and an
        // `approx` whose count is 0 is not stored at all (M2c spec §4), so a
        // 16-byte `int_sum` of 5 or an all-zero `approx` is corruption too.
        // The SUM layout is [rows, real_keys, non_null, int_sum, approx].
        for bad in [
            row(vec![int(1), Value::Null, int(1), int(5)]),
            row(vec![int(1), Value::Null, txt("x"), int(5), Value::Null]),
            row(vec![
                int(1),
                Value::Null,
                int(1),
                Value::Blob(5i128.to_be_bytes().to_vec()),
                Value::Null,
            ]),
            row(vec![
                int(1),
                Value::Null,
                int(1),
                int(5),
                Value::Blob(vec![0; 304]),
            ]),
            row(vec![
                int(1),
                Value::Blob(vec![1, 2, 3]),
                int(1),
                int(5),
                Value::Null,
            ]),
            row(vec![
                int(1),
                Value::Blob(vec![0; 8]),
                int(1),
                int(5),
                Value::Null,
            ]),
            row(vec![int(1), int(0), int(1), int(5), Value::Null]),
        ] {
            let mut state = MemArrangement::new();
            state.update(&row(vec![txt("a")]), &bad, 1).unwrap();
            let mut s = sum_state_on(Box::new(state));
            let err = s
                .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
                .expect_err("a malformed stored value is corrupted state");
            assert!(err.0.contains("corrupted aggregate state"), "{err}");
        }
    }

    fn real(x: f64) -> Value {
        Value::Real(x)
    }

    #[test]
    fn integral_reals_and_ints_share_a_group() {
        // Spec §2: 1 and 1.0 are one group; -0.0 and 0 are one group; a group
        // with a REAL key row outputs its key as REAL.
        let mut s = count_state();
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![int(1), int(0)]), 1),
                (row(vec![real(1.0), int(0)]), 1),
                (row(vec![real(-0.0), int(0)]), 1),
                (row(vec![int(0), int(0)]), 1),
                (row(vec![real(1.5), int(0)]), 1),
                (row(vec![txt("1"), int(0)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![real(1.0), int(2)]), 1),
                (row(vec![real(0.0), int(2)]), 1),
                (row(vec![real(1.5), int(1)]), 1),
                (row(vec![txt("1"), int(1)]), 1),
            ])
        );
    }

    #[test]
    fn a_group_without_real_key_rows_outputs_an_int_key() {
        let mut s = count_state();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![int(1), int(0)]), 2)]))
            .unwrap();
        assert_eq!(d, ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]));
    }

    #[test]
    fn the_key_turns_back_to_int_when_its_last_real_row_leaves() {
        // The count follows weights: two REAL rows in, one out, still REAL.
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![int(1), int(0)]), 1),
            (row(vec![real(1.0), int(0)]), 2),
        ]))
        .unwrap();
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![real(1.0), int(0)]), -1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![real(1.0), int(3)]), -1),
                (row(vec![real(1.0), int(2)]), 1),
            ])
        );
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![real(1.0), int(0)]), -1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![real(1.0), int(2)]), -1),
                (row(vec![int(1), int(1)]), 1),
            ])
        );
    }

    #[test]
    fn the_stored_state_counts_real_keys_and_omits_them_when_zero() {
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
        s.absorb(&ZSet::from_rows([(row(vec![int(1), int(5)]), 1)]))
            .unwrap();
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        assert_eq!(
            stored[0].1,
            row(vec![int(1), Value::Null, int(1), int(5), Value::Null])
        );
        s.absorb(&ZSet::from_rows([(row(vec![real(1.0), int(5)]), 1)]))
            .unwrap();
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, row(vec![int(1)]), "keyed by the canonical key");
        assert_eq!(
            stored[0].1,
            row(vec![
                int(2),
                Value::Blob(1i64.to_be_bytes().to_vec()),
                int(2),
                int(10),
                Value::Null
            ])
        );
    }

    #[test]
    fn a_state_rebuilt_from_its_arrangement_keeps_its_real_keys() {
        let mirrors = Mirrors::default();
        let mut old = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
        old.absorb(&ZSet::from_rows([(row(vec![real(2.0), int(1)]), 1)]))
            .unwrap();
        let mut rebuilt = sum_state_on(Box::new(mirrors.snapshot(groups_id())));
        let d = rebuilt
            .absorb(&ZSet::from_rows([(row(vec![int(2), int(1)]), 1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![real(2.0), int(1)]), -1),
                (row(vec![real(2.0), int(2)]), 1),
            ])
        );
    }

    #[test]
    fn real_key_counts_are_kept_per_group_by_column() {
        // Two group-by columns, only the second ever REAL: the first is output
        // as Int and the second as Real, and the stored counts are [0, 1] in
        // column order.
        let mirrors = Mirrors::default();
        let mut s = AggState::new(
            vec![0, 1],
            vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            mirrors.arrangement(groups_id()).unwrap(),
        );
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![int(1), real(2.0)]), 1),
                (row(vec![int(1), int(2)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![int(1), real(2.0), int(2)]), 1)])
        );
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        assert_eq!(stored[0].0, row(vec![int(1), int(2)]));
        let counts: Vec<u8> = [0i64, 1].iter().flat_map(|c| c.to_be_bytes()).collect();
        // COUNT(*) stores nothing of its own: it is the group's `rows`.
        assert_eq!(stored[0].1, row(vec![int(2), Value::Blob(counts)]));
    }

    fn agg(func: AggFn, column: usize) -> Agg {
        Agg {
            func,
            column: Some(column),
        }
    }

    fn state_with(aggs: Vec<Agg>) -> AggState {
        AggState::new(vec![0], aggs, Box::new(MemArrangement::new()))
    }

    #[test]
    fn sum_of_integers_stays_integer_and_a_real_makes_it_real() {
        let mut s = state_with(vec![
            agg(AggFn::Sum, 1),
            agg(AggFn::Total, 1),
            agg(AggFn::Avg, 1),
        ]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), 1),
                (row(vec![txt("a"), int(2)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(3), real(3.0), real(1.5)]), 1)])
        );
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), 1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(3), real(3.0), real(1.5)]), -1),
                (
                    row(vec![txt("a"), real(3.5), real(3.5), real(3.5 / 3.0)]),
                    1
                ),
            ])
        );
        // Retracting the only REAL input switches back to INTEGER (the `approx` switch).
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), -1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (
                    row(vec![txt("a"), real(3.5), real(3.5), real(3.5 / 3.0)]),
                    -1
                ),
                (row(vec![txt("a"), int(3), real(3.0), real(1.5)]), 1),
            ])
        );
    }

    #[test]
    fn sum_outside_i64_is_an_error_and_total_and_avg_are_not() {
        let mut s = state_with(vec![agg(AggFn::Total, 1), agg(AggFn::Avg, 1)]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(i64::MAX)]), 1),
                (row(vec![txt("a"), int(1)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(
                row(vec![
                    txt("a"),
                    real(9_223_372_036_854_775_808.0),
                    real(4_611_686_018_427_387_904.0)
                ]),
                1
            )])
        );
        let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
        let e = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(i64::MAX)]), 1),
                (row(vec![txt("a"), int(1)]), 1),
            ]))
            .expect_err("spec §4: the final exact sum is outside i64");
        assert_eq!(e.0, SUM_OVERFLOW);
    }

    #[test]
    fn only_the_final_sum_is_judged_for_overflow() {
        // Spec §4: SQLite fails on this prefix; ivmlite judges the final sum.
        let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(i64::MAX)]), 1),
                (row(vec![txt("a"), int(1)]), 1),
                (row(vec![txt("a"), int(-1)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 1)])
        );
    }

    #[test]
    fn count_of_a_column_skips_nulls_and_follows_weights() {
        let mut s = state_with(vec![
            agg(AggFn::Count, 1),
            Agg {
                func: AggFn::Count,
                column: None,
            },
        ]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), 3),
                (row(vec![txt("a"), Value::Null]), 2),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(3), int(5)]), 1)])
        );
    }

    #[test]
    fn avg_divides_by_the_non_null_count() {
        let mut s = state_with(vec![agg(AggFn::Avg, 1)]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), 1),
                (row(vec![txt("a"), Value::Null]), 1),
                (row(vec![txt("a"), int(2)]), 1),
            ]))
            .unwrap();
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), real(1.5)]), 1)]));
    }

    #[test]
    fn avg_is_the_exact_mean_rounded_once() {
        // 0.1 + 0.2 + 0.3, exactly, divided by 3, is closer to 0.2 than to
        // 0.19999999999999998, which is what rounding the sum first gives.
        let mut s = state_with(vec![agg(AggFn::Avg, 1), agg(AggFn::Sum, 1)]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), real(0.1)]), 1),
                (row(vec![txt("a"), real(0.2)]), 1),
                (row(vec![txt("a"), real(0.3)]), 1),
            ]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), real(0.2), real(0.6)]), 1)])
        );
    }

    #[test]
    fn nothing_non_null_gives_null_sum_and_avg_and_zero_total() {
        let mut s = state_with(vec![
            agg(AggFn::Sum, 1),
            agg(AggFn::Total, 1),
            agg(AggFn::Avg, 1),
            agg(AggFn::Count, 1),
        ]);
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([(
                row(vec![txt("a"), Value::Null, real(0.0), Value::Null, int(0)]),
                1
            )])
        );
    }

    #[test]
    fn opposite_infinities_sum_to_null() {
        // M2c spec amendment 5: +Inf + -Inf is NaN, which SQLite returns as NULL.
        let mut s = state_with(vec![agg(AggFn::Sum, 1), agg(AggFn::Avg, 1)]);
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), real(f64::INFINITY)]), 1),
            (row(vec![txt("a"), real(1.0)]), 1),
        ]))
        .unwrap();
        let d = s
            .absorb(&ZSet::from_rows([(
                row(vec![txt("a"), real(f64::NEG_INFINITY)]),
                1,
            )]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (
                    row(vec![txt("a"), real(f64::INFINITY), real(f64::INFINITY)]),
                    -1
                ),
                (row(vec![txt("a"), Value::Null, Value::Null]), 1),
            ])
        );
    }

    #[test]
    fn text_that_reads_as_an_integer_sums_as_an_integer() {
        let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
        let d = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), txt("12")]), 1),
                (row(vec![txt("a"), txt(" 3 ")]), 1),
            ]))
            .unwrap();
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(15)]), 1)]));
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), txt("abc")]), 1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(15)]), -1),
                (row(vec![txt("a"), real(15.0)]), 1)
            ])
        );
    }

    #[test]
    fn the_state_forgets_frac_when_approx_returns_to_zero() {
        let mirrors = Mirrors::default();
        let mut s = AggState::new(
            vec![0],
            vec![agg(AggFn::Sum, 1)],
            mirrors.arrangement(groups_id()).unwrap(),
        );
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), real(0.5)]), 1),
        ]))
        .unwrap();
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        assert!(
            matches!(stored[0].1.get(4), Value::Blob(b) if b.len() == 304),
            "{stored:?}"
        );
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), -1)]))
            .unwrap();
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        // [rows, real_keys, non_null, int_sum, approx]
        assert_eq!(
            stored[0].1,
            row(vec![int(1), Value::Null, int(1), int(1), Value::Null])
        );
    }

    #[test]
    fn the_stored_state_does_not_depend_on_the_order_of_inputs() {
        let values = [0.1, 1e300, -1e300, 3.25, 7e-310];
        let stored = |order: &[usize]| {
            let mirrors = Mirrors::default();
            let mut s = AggState::new(
                vec![0],
                vec![agg(AggFn::Sum, 1)],
                mirrors.arrangement(groups_id()).unwrap(),
            );
            for &i in order {
                s.absorb(&ZSet::from_rows([(
                    row(vec![txt("a"), real(values[i])]),
                    1,
                )]))
                .unwrap();
            }
            mirrors.snapshot(groups_id()).scan().unwrap()
        };
        assert_eq!(stored(&[0, 1, 2, 3, 4]), stored(&[4, 2, 0, 3, 1]));
    }

    #[test]
    fn an_int_sum_outside_i64_is_stored_as_sixteen_bytes() {
        let mirrors = Mirrors::default();
        let mut s = AggState::new(
            vec![0],
            vec![agg(AggFn::Total, 1)],
            mirrors.arrangement(groups_id()).unwrap(),
        );
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 2)]))
            .unwrap();
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        let want = (i128::from(i64::MAX) * 2).to_be_bytes().to_vec();
        assert_eq!(stored[0].1.get(3), &Value::Blob(want));
        // Read back from the 16 bytes, and an Int again once it fits.
        let d = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), -1)]))
            .unwrap();
        assert_eq!(
            d,
            ZSet::from_rows([
                (
                    row(vec![txt("a"), real(2.0 * 9_223_372_036_854_775_808.0)]),
                    -1
                ),
                (row(vec![txt("a"), real(9_223_372_036_854_775_808.0)]), 1),
            ])
        );
        let stored = mirrors.snapshot(groups_id()).scan().unwrap();
        assert_eq!(stored[0].1.get(3), &int(i64::MAX));
    }

    #[test]
    fn an_int_sum_past_i128_is_an_error_not_a_panic() {
        // Each product fits i128, and the weights sum to 4, but the exact
        // integer sum, about 2^127, does not. (Rows are ordered Int before
        // Text, so the row count never leaves i64 on the way.)
        let mut s = state_with(vec![agg(AggFn::Total, 1)]);
        let e = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), int(i64::MIN)]), -i64::MAX),
                (row(vec![txt("a"), int(i64::MAX)]), i64::MAX),
                (row(vec![txt("a"), txt("9223372036854775807")]), 4),
            ]))
            .expect_err("the exact integer sum leaves i128");
        assert!(e.0.contains("i128"), "{e}");
    }

    #[test]
    fn a_non_integer_sum_left_without_inputs_is_reported_as_corrupted() {
        // M2c spec §4: once no non-integer input is live, their exact sum is
        // zero. A retraction of a REAL that was never inserted breaks that.
        let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
        let e = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), real(0.5)]), 1),
                (row(vec![txt("a"), real(0.25)]), -1),
            ]))
            .expect_err("no live non-integer input, but a non-zero sum");
        assert!(e.0.contains("corrupted aggregate state"), "{e}");
    }

    #[test]
    fn avg_over_a_negative_count_is_reported_as_corrupted() {
        // A non-NULL count below zero only comes from retracting rows that
        // were never inserted; AVG reports it instead of dividing by it.
        let mut s = state_with(vec![agg(AggFn::Avg, 1)]);
        let e = s
            .absorb(&ZSet::from_rows([
                (row(vec![txt("a"), Value::Null]), 2),
                (row(vec![txt("a"), int(1)]), -1),
            ]))
            .expect_err("AVG over a negative count");
        assert!(e.0.contains("corrupted aggregate state"), "{e}");
    }
}
