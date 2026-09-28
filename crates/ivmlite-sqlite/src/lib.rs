//! ivmlite as a SQLite loadable extension (spec §4.3; Phase 3a spec): the
//! `ivm` virtual-table module.
//!
//! ```sql
//! CREATE VIRTUAL TABLE revenue USING ivm('SELECT region, SUM(amount) FROM orders GROUP BY region');
//! INSERT INTO revenue(revenue) VALUES ('refresh');
//! SELECT * FROM revenue;
//! DROP TABLE revenue;
//! ```
//!
//! A view cannot be renamed (spec §7): its shadow tables, triggers and
//! metadata all carry its name, so `ALTER TABLE v RENAME TO ...` is refused.

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

/// The `ivm` module (Phase 3b spec §7): rusqlite's writable-table module
/// plus an `xRename`, which rusqlite 0.40 does not expose. A `static`,
/// because SQLite keeps the module pointer for as long as the module is
/// registered.
static IVM_MODULE: ffi::sqlite3_module = {
    const BASE: Module<'static, vtab::IvmTab> = Module::update_module();
    // SAFETY: rusqlite 0.40 (pinned by this crate's Cargo.lock) declares
    // `Module` `#[repr(transparent)]` over `ffi::sqlite3_module`, so the two
    // have the same layout; `transmute` checks their sizes at compile time.
    let mut module: ffi::sqlite3_module = unsafe { std::mem::transmute(BASE) };
    module.xRename = Some(vtab::refuse_rename);
    module
};

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
    // SAFETY: `db`, `err` and `api` are exactly what SQLite passed to this
    // `sqlite3_ivmlite_init` call (the entry point's own contract, see its
    // doc comment above); `extension_init2`'s contract is that `init` must do
    // nothing but register features, and the closure below only calls
    // `create_module`.
    unsafe {
        Connection::extension_init2(db, err, api, |conn| {
            let registration = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: `conn` wraps the handle SQLite is initializing; the
                // module is a `static`, and no client data is passed. Already
                // inside this function's outer `unsafe` block, so no nested
                // block is needed (one would be flagged as unused).
                let rc = ffi::sqlite3_create_module_v2(
                    conn.handle(),
                    c"ivm".as_ptr(),
                    &IVM_MODULE,
                    std::ptr::null_mut(),
                    None,
                );
                if rc != ffi::SQLITE_OK {
                    return Err(rusqlite::Error::SqliteFailure(
                        ffi::Error::new(rc),
                        Some("registering the ivm module".to_string()),
                    ));
                }
                Ok(())
            }))
            .unwrap_or_else(|_| {
                Err(rusqlite::Error::ModuleError(
                    "ivmlite internal error while registering the ivm module".to_string(),
                ))
            });
            registration?;
            Ok(false)
        })
    }
}
