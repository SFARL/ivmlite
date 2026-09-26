//! ivmlite as a SQLite loadable extension (spec §4.3; Phase 3a spec): the
//! `ivm` virtual-table module.
//!
//! ```sql
//! CREATE VIRTUAL TABLE revenue USING ivm('SELECT region, SUM(amount) FROM orders GROUP BY region');
//! INSERT INTO revenue(revenue) VALUES ('refresh');
//! SELECT * FROM revenue;
//! DROP TABLE revenue;
//! ```

mod catalog;
mod encode;
mod names;
mod state;
mod view;
mod vtab;

use std::ffi::{c_char, c_int};
use std::panic::AssertUnwindSafe;

use rusqlite::vtab::Module;
use rusqlite::{ffi, Connection};

/// The extension's entry point; load it with
/// `load_extension('<path>', 'sqlite3_ivmlite_init')`.
///
/// # Safety
/// Called by SQLite with a live connection handle and API table.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_ivmlite_init(
    db: *mut ffi::sqlite3,
    err: *mut *mut c_char,
    api: *mut ffi::sqlite3_api_routines,
) -> c_int {
    Connection::extension_init2(db, err, api, |conn| {
        // The `ivm` module: a writable virtual table, `INSERT` being its
        // command channel. A `const` is promoted to the `'static` the
        // registration needs.
        const IVM: Module<'static, vtab::IvmTab> = Module::update_module();
        std::panic::catch_unwind(AssertUnwindSafe(|| conn.create_module(c"ivm", &IVM, None)))
            .unwrap_or_else(|_| {
                Err(rusqlite::Error::ModuleError(
                    "ivmlite internal error while registering the ivm module".to_string(),
                ))
            })?;
        Ok(false)
    })
}
