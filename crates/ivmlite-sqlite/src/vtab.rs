//! The `ivm` virtual-table module: SQLite's callbacks, adapted to `view.rs`.
//!
//! Every callback runs inside `guard`, so a panic becomes an SQLite error
//! instead of unwinding across the FFI boundary (Phase 3a spec §5).

use std::borrow::Cow;
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use ivmlite_sql::CompiledView;
use rusqlite::types::Value as SqlValue;
use rusqlite::vtab::{
    dequote, Context, CreateVTab, Filters, IndexInfo, Inserts, UpdateVTab, Updates, VTab,
    VTabConnection, VTabCursor, VTabKind,
};
use rusqlite::{ffi, Connection, Error};

use crate::names::{main_qualified, out_table, quote};
use crate::snapshot;
use crate::view::{self, Checked};

/// Run a callback body, turning its error and any panic into an SQLite error.
fn guard<T>(body: impl FnOnce() -> Result<T, String>) -> rusqlite::Result<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(Error::ModuleError(message)),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".to_string());
            Err(Error::ModuleError(format!(
                "ivmlite internal error: {message}"
            )))
        }
    }
}

fn utf8<'a>(bytes: &'a [u8], what: &str) -> Result<&'a str, String> {
    std::str::from_utf8(bytes).map_err(|_| format!("the {what} is not UTF-8"))
}

/// What create or connect hands `IvmTab::open_view`: the table declaration,
/// the view or why it is broken, and what its passing checks saw.
type Opened = (String, Result<CompiledView, String>, Option<Checked>);

#[repr(C)]
pub struct IvmTab {
    /// Must come first: SQLite sees this struct as a `sqlite3_vtab`.
    base: ffi::sqlite3_vtab,
    /// The view's one non-owning `Connection` over the handle it was opened
    /// on (Phase 6 spec §3), used by every later callback of the view. Its
    /// statement cache lives as long as the view: dropping it in xDisconnect
    /// or xDestroy finalizes every cached statement, and SQLite calls
    /// xDisconnect before `sqlite3_close` checks for unfinalized statements.
    conn: Rc<Connection>,
    name: String,
    /// The compiled view, or why a reopened view can no longer be maintained.
    view: Result<CompiledView, String>,
    /// What the view's last passing catalog check on this connection saw
    /// (Phase 5 spec §3); `None` for a broken view.
    checked: Option<Checked>,
    /// The catalog snapshot every view on this connection shares (Phase 6
    /// spec §4). Holding it keeps it alive: it is freed when the last view of
    /// the connection disconnects.
    snapshot: snapshot::Shared,
}

impl IvmTab {
    /// Shared by create and connect: `make` returns the declaration, the
    /// view (or why it is broken) and what its passing checks saw. The
    /// connection's snapshot is found here, so connect's own checks already
    /// use it (Phase 6 spec §4).
    fn open_view(
        db: &mut VTabConnection,
        database: &[u8],
        table: &[u8],
        make: impl FnOnce(&Rc<Connection>, &snapshot::Shared, &str) -> Result<Opened, String>,
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        guard(|| {
            if database != b"main" {
                return Err("ivmlite v0 views live in the main database only".to_string());
            }
            let name = utf8(table, "view name")?.to_string();
            // SAFETY: the handle of the connection running this statement,
            // live as long as this view's virtual table; a `Connection` made
            // by `from_handle` never closes it.
            let conn = unsafe { Connection::from_handle(db.handle()) }
                .map(Rc::new)
                .map_err(|e| e.to_string())?;
            // SAFETY: only the handle's address is used, as the registry's key.
            let snapshot = snapshot::for_connection(unsafe { conn.handle() });
            let (declaration, view, checked) = make(&conn, &snapshot, &name)?;
            // A broken view keeps rusqlite's default: it never refreshes.
            if let Ok(compiled) = &view {
                conn.set_prepared_statement_cache_capacity(view::statement_capacity(compiled));
            }
            let declared = CString::new(declaration)
                .map(Cow::Owned)
                .map_err(|_| "a column name contains a NUL byte".to_string())?;
            Ok((
                declared,
                IvmTab {
                    base: ffi::sqlite3_vtab::default(),
                    conn,
                    name,
                    view,
                    checked,
                    snapshot,
                },
            ))
        })
    }

    fn view(&self) -> Result<&CompiledView, String> {
        self.view.as_ref().map_err(Clone::clone)
    }
}

// SAFETY: `IvmTab` is `#[repr(C)]` with `base: ffi::sqlite3_vtab` as its first
// field, as `VTab`'s safety contract requires.
unsafe impl<'vtab> VTab<'vtab> for IvmTab {
    type Aux = ();
    type Cursor = IvmCursor;

    fn connect(
        db: &mut VTabConnection,
        _aux: Option<&()>,
        _module: &[u8],
        database: &[u8],
        table: &[u8],
        _args: &[&[u8]],
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        IvmTab::open_view(db, database, table, |conn, snapshot, name| {
            let reopened = view::connect(conn, snapshot, name)?;
            Ok((reopened.declaration, reopened.view, reopened.checked))
        })
    }

    fn best_index(&self, info: &mut IndexInfo) -> rusqlite::Result<bool> {
        guard(|| {
            // Only full scans: v0 reads the whole output table.
            info.set_estimated_cost(1_000_000.0);
            Ok(true)
        })
    }

    fn open(&'vtab mut self) -> rusqlite::Result<IvmCursor> {
        // Copied out of `self` now, not borrowed: rusqlite's update glue can
        // hand this same `IvmTab` out as `&mut` (via `xUpdate`) while a
        // cursor opened from it is still alive (e.g. a `SELECT` that drives
        // an `UPDATE` via a trigger, or simply overlapping statements) —
        // holding `&'vtab IvmTab` here would be a live shared borrow
        // aliasing that later `&mut`, which is undefined behavior even
        // though the borrow checker cannot see across the FFI boundary.
        let cols = self
            .view()
            .map(|view| view.columns.iter().map(|c| quote(&c.name)).collect());
        Ok(IvmCursor {
            base: ffi::sqlite3_vtab_cursor::default(),
            conn: Rc::clone(&self.conn),
            out_table: out_table(&self.name),
            cols,
            rows: Vec::new(),
            at: 0,
        })
    }
}

impl CreateVTab<'_> for IvmTab {
    const KIND: VTabKind = VTabKind::Default;

    fn create(
        db: &mut VTabConnection,
        _aux: Option<&()>,
        _module: &[u8],
        database: &[u8],
        table: &[u8],
        args: &[&[u8]],
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        IvmTab::open_view(db, database, table, |conn, _snapshot, name| {
            let [arg] = args else {
                return Err(
                    "USING ivm takes one argument, the view's SELECT as a string literal"
                        .to_string(),
                );
            };
            let sql = dequote(utf8(arg, "view's SQL")?).into_owned();
            let (compiled, checked) = view::create(conn, name, &sql)?;
            Ok((
                view::declaration(name, &compiled)?,
                Ok(compiled),
                Some(checked),
            ))
        })
    }

    fn destroy(&self) -> rusqlite::Result<()> {
        guard(|| view::destroy(&self.conn, &self.name))
    }
}

impl UpdateVTab<'_> for IvmTab {
    fn delete(&mut self, _rowid: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<()> {
        Err(read_only())
    }

    fn insert(&mut self, args: &Inserts<'_>) -> rusqlite::Result<i64> {
        guard(|| {
            // argv: old rowid (NULL), new rowid, the output columns, then the
            // hidden command column. The field, not `self.view()`, so that
            // `self.checked` can be borrowed mutably alongside it.
            let view = self.view.as_ref().map_err(Clone::clone)?;
            let n = view.columns.len();
            let command_at = 2 + n;
            let command: Option<String> = args.get(command_at).map_err(|e| e.to_string())?;
            match command.as_deref() {
                Some("refresh") => {
                    // A refresh takes no other value: `INSERT INTO
                    // v(col, v) VALUES (1, 'refresh')` must fail, not
                    // silently drop the `1`.
                    for i in 0..n {
                        let value: SqlValue = args.get(2 + i).map_err(|e| e.to_string())?;
                        if value != SqlValue::Null {
                            return Err(
                                "INSERT INTO v(v) VALUES('refresh') takes no other column value"
                                    .to_string(),
                            );
                        }
                    }
                    view::refresh(
                        &self.conn,
                        &self.snapshot,
                        &self.name,
                        view,
                        &mut self.checked,
                    )?;
                    Ok(0)
                }
                Some(other) => Err(format!(
                    "unknown ivmlite command {other:?}; the only command is 'refresh'"
                )),
                None => Err(read_only().to_string()),
            }
        })
    }

    fn update(&mut self, _args: &Updates<'_>) -> rusqlite::Result<()> {
        Err(read_only())
    }
}

fn read_only() -> Error {
    Error::ModuleError(
        "an ivmlite view is read-only; bring it up to date with INSERT INTO v(v) VALUES('refresh')"
            .to_string(),
    )
}

/// `xRename` (Phase 3b spec §7): an ivmlite view cannot be renamed — its
/// shadow tables, triggers and metadata all carry its name — so the rename
/// is refused and SQLite leaves the schema unchanged. Nothing here can
/// panic (no allocation through Rust, no indexing), so it needs no `guard`.
///
/// # Safety
/// Called by SQLite with the view's live `sqlite3_vtab`.
pub unsafe extern "C" fn refuse_rename(
    vtab: *mut ffi::sqlite3_vtab,
    _new_name: *const c_char,
) -> c_int {
    const MESSAGE: &[u8] =
        b"ivmlite views cannot be renamed; drop the view and create it again under the new name\0";
    // SAFETY: `vtab` is live for this call. SQLite frees `zErrMsg` with
    // `sqlite3_free`, so it must come from SQLite's allocator; an earlier
    // message is freed first.
    unsafe {
        if !(*vtab).zErrMsg.is_null() {
            ffi::sqlite3_free((*vtab).zErrMsg.cast());
        }
        let buf = ffi::sqlite3_malloc64(MESSAGE.len() as u64).cast::<c_char>();
        (*vtab).zErrMsg = buf;
        if buf.is_null() {
            return ffi::SQLITE_NOMEM;
        }
        std::ptr::copy_nonoverlapping(MESSAGE.as_ptr().cast::<c_char>(), buf, MESSAGE.len());
    }
    ffi::SQLITE_ERROR
}

#[repr(C)]
pub struct IvmCursor {
    /// Must come first: SQLite sees this struct as a `sqlite3_vtab_cursor`.
    base: ffi::sqlite3_vtab_cursor,
    /// The view's `Connection`, shared rather than borrowed from `IvmTab`
    /// (see `open`). A read prepares its `SELECT` uncached each time: the
    /// statement cache covers the refresh path only (Phase 6 spec §3).
    conn: Rc<Connection>,
    out_table: String,
    /// The quoted output columns, or why the view is broken. Copied out of
    /// `IvmTab` at `open` (see its comment for why): the cursor owns
    /// everything it needs from here on and holds no reference into the
    /// table it was opened from.
    cols: Result<Vec<String>, String>,
    rows: Vec<(i64, Vec<SqlValue>)>,
    at: usize,
}

// SAFETY: `IvmCursor` is `#[repr(C)]` with `base: ffi::sqlite3_vtab_cursor` as
// its first field, as `VTabCursor`'s safety contract requires.
unsafe impl VTabCursor for IvmCursor {
    fn filter(
        &mut self,
        _idx: c_int,
        _idx_str: Option<&str>,
        _args: &Filters<'_>,
    ) -> rusqlite::Result<()> {
        guard(|| {
            let cols = self.cols.as_ref().map_err(|e| e.clone())?;
            let n = cols.len();
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT rowid, {} FROM {}",
                    cols.join(", "),
                    main_qualified(&self.out_table)
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    let mut values = Vec::with_capacity(n);
                    for i in 1..=n {
                        values.push(r.get::<_, SqlValue>(i)?);
                    }
                    Ok((r.get::<_, i64>(0)?, values))
                })
                .map_err(|e| e.to_string())?;
            self.rows = rows
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?;
            self.at = 0;
            Ok(())
        })
    }

    fn next(&mut self) -> rusqlite::Result<()> {
        self.at += 1;
        Ok(())
    }

    fn eof(&self) -> bool {
        self.at >= self.rows.len()
    }

    fn column(&self, ctx: &mut Context, i: c_int) -> rusqlite::Result<()> {
        guard(|| {
            let values = &self.rows[self.at].1;
            let value = usize::try_from(i).ok().and_then(|i| values.get(i));
            // Past the output columns is the hidden command column: NULL.
            ctx.set_result(value.unwrap_or(&SqlValue::Null))
                .map_err(|e| e.to_string())
        })
    }

    fn rowid(&self) -> rusqlite::Result<i64> {
        // Not `self.rows[self.at]`: unlike `column`, this callback is not
        // wrapped in `guard`, so an out-of-range index would panic across
        // the FFI boundary instead of becoming an SQLite error.
        self.rows
            .get(self.at)
            .map(|&(rowid, _)| rowid)
            .ok_or_else(|| {
                Error::ModuleError(format!(
                    "ivmlite internal error: cursor position {} out of range",
                    self.at
                ))
            })
    }
}
