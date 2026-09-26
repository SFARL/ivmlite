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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ivmlite_core::{Arrangement, Row, StateError};
use rusqlite::Connection;

use crate::encode::{decode, encode};
use crate::names::quote;

/// An arrangement's pending weight changes, by key then value; shared with
/// the refresh that stages them once the operator tree is done.
pub type Pending = Rc<RefCell<BTreeMap<Row, BTreeMap<Row, i64>>>>;

pub struct BufferedArrangement {
    conn: Rc<Connection>,
    get_sql: String,
    scan_sql: String,
    pending: Pending,
}

impl BufferedArrangement {
    pub fn new(conn: Rc<Connection>, table: &str, pending: Pending) -> Self {
        let t = quote(table);
        BufferedArrangement {
            conn,
            get_sql: format!("SELECT val, w FROM {t} WHERE key = ?1"),
            scan_sql: format!("SELECT key, val, w FROM {t}"),
            pending,
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
            values.insert(decode(&val)?, w);
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
                .insert(decode(&val)?, w);
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
}
