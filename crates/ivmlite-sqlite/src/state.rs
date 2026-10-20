//! `Arrangement` over a `__ivm_state_<view>_<node>_<role>` table (spec §6.3,
//! §7) that **reads** the table and **buffers** its writes.
//!
//! A refresh must change state, output and watermarks together or not at all,
//! and it runs inside the `INSERT INTO v(v)` statement, where SQLite forbids
//! a `SAVEPOINT` and does not roll back the callback's own writes when it
//! fails inside an explicit transaction (measured, Phase 3a). So nothing is
//! written while the operator tree runs: each arrangement overlays its pending
//! changes on the table's rows, and `view.rs` applies every pending change
//! afterwards in one statement.
//!
//! A MIN/MAX arrangement stores each value's sort key raw (`ValueCodec`), so
//! its extreme is one ordered read of the primary key (M2c spec §4).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use ivmlite_core::{Arrangement, ArrangementId, ArrangementRole, Row, StateError, Value};
use rusqlite::Connection;

use crate::encode::{decode, encode};
use crate::names::main_qualified;

/// An arrangement's pending weight changes, by key then value; shared with
/// the refresh that stages them once the operator tree is done.
pub type Pending = Rc<RefCell<BTreeMap<Row, BTreeMap<Row, i64>>>>;

/// How an arrangement's values are stored in its table's `val` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCodec {
    /// `encode.rs`'s row encoding.
    Row,
    /// A MIN/MAX value's sort key, stored raw, so that `ORDER BY val` is
    /// spec §2's order (M2c spec §4). Its value rows are `[Value::Blob(key)]`.
    SortKey,
}

/// The codec of the arrangement `id`'s values.
pub fn codec(id: ArrangementId) -> ValueCodec {
    match id.role {
        ArrangementRole::AggregateExtremum(_) => ValueCodec::SortKey,
        ArrangementRole::JoinLeft
        | ArrangementRole::JoinRight
        | ArrangementRole::AggregateGroups => ValueCodec::Row,
    }
}

impl ValueCodec {
    /// The bytes stored in `val` for the value row `val`.
    ///
    /// # Errors
    /// If a `SortKey` value row is not one sort key: core's `AggState` only
    /// writes `[Value::Blob(key)]` rows to a MIN/MAX arrangement.
    pub fn encode_val(self, val: &Row) -> Result<Vec<u8>, StateError> {
        match (self, val.0.as_slice()) {
            (ValueCodec::Row, _) => Ok(encode(val)),
            (ValueCodec::SortKey, [Value::Blob(key)]) => Ok(key.clone()),
            (ValueCodec::SortKey, other) => Err(StateError(format!(
                "broken invariant: a MIN/MAX value row is {other:?}, not one sort key"
            ))),
        }
    }

    /// The value row stored as `bytes`. A sort key is checked when it is
    /// decoded to a value (`from_sort_key`), not here.
    pub fn decode_val(self, bytes: &[u8]) -> Result<Row, StateError> {
        match self {
            ValueCodec::Row => decode(bytes),
            ValueCodec::SortKey => Ok(Row::new(vec![Value::Blob(bytes.to_vec())])),
        }
    }
}

/// M2c spec §4: the indexed read of a MIN (ascending) or a MAX (descending).
/// It has no `LIMIT 1`: `first` steps it until it finds a value that the
/// refresh's pending changes leave live (amendment 3).
pub fn first_sql(table: &str, descending: bool) -> String {
    format!(
        "SELECT val, w FROM {} WHERE key = ?1 ORDER BY val{}",
        main_qualified(table),
        if descending { " DESC" } else { "" }
    )
}

pub struct BufferedArrangement {
    conn: Rc<Connection>,
    get_sql: String,
    scan_sql: String,
    first_asc_sql: String,
    first_desc_sql: String,
    pending: Pending,
    codec: ValueCodec,
}

impl BufferedArrangement {
    pub fn new(conn: Rc<Connection>, table: &str, pending: Pending, codec: ValueCodec) -> Self {
        let t = main_qualified(table);
        BufferedArrangement {
            conn,
            get_sql: format!("SELECT val, w FROM {t} WHERE key = ?1"),
            scan_sql: format!("SELECT key, val, w FROM {t}"),
            first_asc_sql: first_sql(table, false),
            first_desc_sql: first_sql(table, true),
            pending,
            codec,
        }
    }
}

fn state_error(e: rusqlite::Error) -> StateError {
    StateError(format!("operator state table: {e}"))
}

/// Add `changes` to `stored` and drop what reaches zero (spec §5.1).
fn overlay(stored: &mut BTreeMap<Row, i64>, changes: Option<&BTreeMap<Row, i64>>) {
    for (val, dw) in changes.into_iter().flatten() {
        let w = stored.entry(val.clone()).or_insert(0);
        *w += dw;
        if *w == 0 {
            stored.remove(val);
        }
    }
}

impl Arrangement for BufferedArrangement {
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        let mut stmt = self
            .conn
            .prepare_cached(&self.get_sql)
            .map_err(state_error)?;
        let rows = stmt
            .query_map([encode(key)], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(state_error)?;
        let mut values = BTreeMap::new();
        for row in rows {
            let (val, w) = row.map_err(state_error)?;
            values.insert(self.codec.decode_val(&val)?, w);
        }
        overlay(&mut values, self.pending.borrow().get(key));
        Ok(values.into_iter().collect())
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError> {
        if weight_delta == 0 {
            return Ok(());
        }
        let mut pending = self.pending.borrow_mut();
        let vals = pending.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                pending.remove(key);
            }
        }
        Ok(())
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        let mut stmt = self
            .conn
            .prepare_cached(&self.scan_sql)
            .map_err(state_error)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(state_error)?;
        let mut all: BTreeMap<Row, BTreeMap<Row, i64>> = BTreeMap::new();
        for row in rows {
            let (key, val, w) = row.map_err(state_error)?;
            all.entry(decode(&key)?)
                .or_default()
                .insert(self.codec.decode_val(&val)?, w);
        }
        let pending = self.pending.borrow();
        for key in pending.keys() {
            all.entry(key.clone()).or_default();
        }
        let mut out = Vec::new();
        for (key, mut values) in all {
            overlay(&mut values, pending.get(&key));
            out.extend(values.into_iter().map(|(val, w)| (key.clone(), val, w)));
        }
        Ok(out)
    }

    /// M2c spec §4, amendment 3: the stored values in order, stepped lazily
    /// and stopped at the first one the pending changes leave live, then
    /// compared with the values only this refresh added. In the steady state
    /// it reads one row.
    fn first(&self, key: &Row, descending: bool) -> Result<Option<Row>, StateError> {
        let pending = self.pending.borrow();
        let changes = pending.get(key);
        let change = |val: &Row| changes.and_then(|c| c.get(val)).copied().unwrap_or(0);
        let sql = if descending {
            &self.first_desc_sql
        } else {
            &self.first_asc_sql
        };
        let mut stmt = self.conn.prepare_cached(sql).map_err(state_error)?;
        let mut rows = stmt.query([encode(key)]).map_err(state_error)?;
        // Stored values passed over because a pending change retracted them.
        let mut passed: BTreeSet<Row> = BTreeSet::new();
        let mut stored = None;
        while let Some(r) = rows.next().map_err(state_error)? {
            let val = self
                .codec
                .decode_val(&r.get::<_, Vec<u8>>(0).map_err(state_error)?)?;
            let w: i64 = r.get(1).map_err(state_error)?;
            if w + change(&val) > 0 {
                stored = Some(val);
                break;
            }
            passed.insert(val);
        }
        // Dropping `rows` resets the cached statement before the callback
        // returns (Phase 6 spec §3).
        drop(rows);
        let earlier = |v: &Row| match &stored {
            None => true,
            Some(s) => {
                if descending {
                    v > s
                } else {
                    v < s
                }
            }
        };
        // A value only this refresh added, ordered before the stored
        // candidate. A pending value that is also stored and was passed over
        // has a total weight of at most 0, so `passed` excludes it; a stored
        // value after the candidate cannot win.
        let added = changes
            .into_iter()
            .flatten()
            .filter(|(v, &w)| w > 0 && !passed.contains(v) && earlier(v))
            .map(|(v, _)| v.clone());
        let added = if descending { added.max() } else { added.min() };
        Ok(added.or(stored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extreme_read_is_one_ordered_primary_key_search() {
        assert_eq!(
            first_sql("__ivm_state_m_0_agg_extremum_1", true),
            "SELECT val, w FROM \"main\".\"__ivm_state_m_0_agg_extremum_1\" WHERE key = ?1 ORDER BY val DESC"
        );
        assert_eq!(
            first_sql("t", false),
            "SELECT val, w FROM \"main\".\"t\" WHERE key = ?1 ORDER BY val"
        );
    }
}
