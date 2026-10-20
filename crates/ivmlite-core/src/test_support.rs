//! Test-only helpers for ivmlite-core's unit tests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::{
    Agg, AggFn, Arrangement, ArrangementId, Column, ColumnType, MemArrangement, Predicate, Row,
    Schema, StateError, Value, ViewQuery,
};

/// An arrangement provider that records every update, by `ArrangementId`.
///
/// Each arrangement it hands out behaves like a fresh `MemArrangement` and
/// mirrors every `update` into a shared record — exactly what a provider backed
/// by `__ivm_state_<view>_<op>` tables would have written. `snapshot` then
/// returns a copy of one arrangement's contents, so a test can build a second
/// tree from nothing but those contents.
#[derive(Clone, Default)]
pub(crate) struct Mirrors(Rc<RefCell<BTreeMap<ArrangementId, MemArrangement>>>);

impl Mirrors {
    pub(crate) fn arrangement(
        &self,
        id: ArrangementId,
    ) -> Result<Box<dyn Arrangement>, StateError> {
        Ok(Box::new(Mirrored {
            id,
            inner: MemArrangement::new(),
            mirrors: self.clone(),
        }))
    }

    /// A copy of what the arrangement `id` holds now; empty if it was never updated.
    pub(crate) fn snapshot(&self, id: ArrangementId) -> MemArrangement {
        self.0.borrow().get(&id).cloned().unwrap_or_default()
    }
}

struct Mirrored {
    id: ArrangementId,
    inner: MemArrangement,
    mirrors: Mirrors,
}

impl Arrangement for Mirrored {
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        self.inner.get(key)
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError> {
        self.inner.update(key, val, weight_delta)?;
        self.mirrors
            .0
            .borrow_mut()
            .entry(self.id)
            .or_default()
            .update(key, val, weight_delta)
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        self.inner.scan()
    }
}

/// An arrangement whose every read and write fails, as a shadow table that
/// SQLite cannot read would (M1b Phase 3a).
pub(crate) struct Failing;

impl Arrangement for Failing {
    fn get(&self, _key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        Err(StateError("injected read failure".into()))
    }

    fn update(&mut self, _key: &Row, _val: &Row, _weight_delta: i64) -> Result<(), StateError> {
        Err(StateError("injected write failure".into()))
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        Err(StateError("injected read failure".into()))
    }
}

/// `(k TEXT, v INTEGER)`, the harness's two-column shape.
pub(crate) fn kv(name: &str) -> Schema {
    Schema {
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
    }
}

pub(crate) fn kv_row(k: &str, v: i64) -> Row {
    Row::new(vec![Value::Text(k.into()), Value::Int(v)])
}

/// `SELECT t0.k, COUNT(*), SUM(t1.v) FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t0.k`
pub(crate) fn join_on_k() -> ViewQuery {
    ViewQuery {
        group_by: vec![0],
        aggs: vec![
            Agg {
                func: AggFn::Count,
                column: None,
            },
            Agg {
                func: AggFn::Sum,
                column: Some(3),
            },
        ],
        predicate: Predicate::None,
        join: Some(crate::Join {
            right: "t1".into(),
            left_column: 0,
            right_column: 0,
        }),
    }
}

/// A deterministic random source for property tests (Steele, Lea and Flood's
/// SplitMix64). Hand-written: `rand` is not a dependency of ivmlite-core.
pub(crate) struct SplitMix64(pub u64);

impl SplitMix64 {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, `n > 0` (the modulo bias is irrelevant here).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}
