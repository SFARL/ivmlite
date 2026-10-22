//! Instrument actual nested shadow reads separately from latency measurements.
//!
//! SQLite permits one trace callback per connection. This scope owns that slot
//! and unregisters it before freeing callback state; do not install another
//! trace callback while it is alive. No statement pointer survives a callback
//! except as an opaque identity used to match STMT, ROW and PROFILE events.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

use rusqlite::{ffi, Connection};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct ReadWork {
    /// Parameterized SQL from sqlite3_sql, never a virtual-table plan label.
    pub sql: String,
    /// SQL with actual binds rendered by SQLite, for reproducible EQP.
    /// SQLite prefixes nested expanded SQL lines with `-- `; remove only
    /// those trace wrappers to recover the executable statement.
    pub expanded_sql: String,
    /// SQLite i32 counters; extremely large statements can overflow them.
    pub fullscan_steps: i32,
    pub vm_steps: i32,
    /// SQLITE_TRACE_ROW events for this nested statement, not outer results.
    pub rows_returned: usize,
}

#[derive(Default)]
struct State {
    active: HashMap<usize, ReadWork>,
    completed: Vec<ReadWork>,
}

#[derive(Default)]
struct CallbackState {
    state: RefCell<State>,
    panicked: Cell<bool>,
}

/// Collects STMT/ROW/PROFILE events for ivmlite's nested output SELECTs.
pub struct ReadTrace<'connection> {
    connection: &'connection Connection,
    state: Box<CallbackState>,
    registered: bool,
}

impl<'connection> ReadTrace<'connection> {
    pub fn start(connection: &'connection Connection) -> rusqlite::Result<Self> {
        let mut trace = Self {
            connection,
            state: Box::default(),
            registered: false,
        };
        // SAFETY: Box keeps callback state at a stable address; the connection
        // borrow prevents close, and Drop removes the callback before the Box.
        let rc = unsafe {
            ffi::sqlite3_trace_v2(
                connection.handle(),
                ffi::SQLITE_TRACE_STMT | ffi::SQLITE_TRACE_ROW | ffi::SQLITE_TRACE_PROFILE,
                Some(callback),
                (&mut *trace.state as *mut CallbackState).cast(),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(rusqlite::Error::SqliteFailure(ffi::Error::new(rc), None));
        }
        trace.registered = true;
        Ok(trace)
    }

    /// Unregister before inspecting state. Fully consume/finalize query rows
    /// first, so every nested read has delivered its PROFILE event.
    pub fn finish(mut self) -> Result<Vec<ReadWork>, String> {
        self.unregister();
        if self.state.panicked.get() {
            return Err("nested-read trace callback panicked".into());
        }
        let mut state = self.state.state.borrow_mut();
        if !state.active.is_empty() {
            return Err("nested-read trace ended before PROFILE".into());
        }
        Ok(std::mem::take(&mut state.completed))
    }

    fn unregister(&mut self) {
        if self.registered {
            // SAFETY: the borrowed connection and callback state are alive.
            unsafe {
                ffi::sqlite3_trace_v2(self.connection.handle(), 0, None, std::ptr::null_mut())
            };
            self.registered = false;
        }
    }
}

impl Drop for ReadTrace<'_> {
    fn drop(&mut self) {
        self.unregister();
    }
}

unsafe extern "C" fn callback(
    event: u32,
    context: *mut c_void,
    statement: *mut c_void,
    _detail: *mut c_void,
) -> i32 {
    // SAFETY: start passes this Box's address and unregisters before freeing it.
    let context = unsafe { &*context.cast::<CallbackState>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let statement = statement.cast::<ffi::sqlite3_stmt>();
        let identity = statement as usize;
        let mut state = context.state.borrow_mut();
        if event == ffi::SQLITE_TRACE_STMT {
            // SAFETY: SQLite supplies a live statement for each trace event.
            let raw = unsafe { ffi::sqlite3_sql(statement) };
            if raw.is_null() {
                return;
            }
            let sql = unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned();
            if !sql.starts_with("SELECT rowid, ") || !sql.contains("FROM \"main\".\"__ivm_out_") {
                return;
            }
            state.active.insert(
                identity,
                ReadWork {
                    sql,
                    expanded_sql: String::new(),
                    fullscan_steps: 0,
                    vm_steps: 0,
                    rows_returned: 0,
                },
            );
        } else if event == ffi::SQLITE_TRACE_ROW {
            if let Some(read) = state.active.get_mut(&identity) {
                read.rows_returned += 1;
            }
        } else if event == ffi::SQLITE_TRACE_PROFILE {
            if let Some(mut read) = state.active.remove(&identity) {
                let expanded = unsafe { ffi::sqlite3_expanded_sql(statement) };
                if !expanded.is_null() {
                    let sql = unsafe { CStr::from_ptr(expanded) }
                        .to_string_lossy()
                        .into_owned();
                    unsafe { ffi::sqlite3_free(expanded.cast()) };
                    // STMT expansion of nested statements can suppress binds.
                    // PROFILE runs after execution, when SQLite expands them.
                    read.expanded_sql = if sql.starts_with("-- ") {
                        sql.lines()
                            .map(|line| line.strip_prefix("-- ").unwrap_or(line))
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        sql
                    };
                } else {
                    context.panicked.set(true);
                }

                read.fullscan_steps = unsafe {
                    ffi::sqlite3_stmt_status(statement, ffi::SQLITE_STMTSTATUS_FULLSCAN_STEP, 0)
                };
                read.vm_steps = unsafe {
                    ffi::sqlite3_stmt_status(statement, ffi::SQLITE_STMTSTATUS_VM_STEP, 0)
                };
                state.completed.push(read);
            }
        }
    }));
    if result.is_err() {
        context.panicked.set(true);
    }
    0
}
