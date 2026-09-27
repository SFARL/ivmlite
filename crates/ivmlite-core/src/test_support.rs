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
