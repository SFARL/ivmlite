//! `ivm_refresh_all([mode])` (M1b Phase 7 spec §2–§4): refresh every view of
//! the database in one call, atomically or view by view.
//!
//! ```sql
//! SELECT ivm_refresh_all();            -- 'atomic': every view, or none
//! SELECT ivm_refresh_all('continue');  -- each view on its own
//! ```
//!
//! A view's own refresh, `INSERT INTO v(v)`, runs inside a writing
//! statement, where SQLite refuses `SAVEPOINT`. A function called from a
//! top-level `SELECT` can open one, so this function is what makes many
//! views' refreshes atomic together (spec §1).

use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::{Value, ValueRef};
use rusqlite::Connection;

use crate::names::{delta_table, main_qualified, quote, DELTA_SEQ, DEPS, PROGRESS, VIEWS};
use crate::vtab::guard;

/// The function's savepoint (spec §2); reserved, and always quoted.
pub const SAVEPOINT: &str = "__ivm_refresh_all";
pub const BAD_ARGUMENT: &str = "ivm_refresh_all takes 'atomic' or 'continue'";
pub const NOT_FROM_A_WRITE: &str =
    "ivm_refresh_all must be called from a SELECT, not from a statement that writes";
/// Spec §2, "Only ivm views are refreshed": the failure of a `__ivm_view`
/// row that names no ivm virtual table.
pub const NOT_AN_IVM_VIEW: &str = "not an ivmlite view";

type Result<T> = std::result::Result<T, String>;

fn sql_error(e: rusqlite::Error) -> String {
    e.to_string()
}

/// Run `sql` with statements prepared and finalized here, never cached
/// (spec §3).
fn exec(conn: &Connection, sql: &str) -> Result<()> {
    conn.execute_batch(sql).map_err(sql_error)
}

/// Phase 7 spec §2: register `ivm_refresh_all([mode])` on `conn`.
///
/// `SQLITE_DIRECTONLY` keeps it out of the database schema's triggers, views
/// and CHECK constraints (not TEMP ones), as SQLite recommends for a function with side effects. It
/// is not deterministic. Its errors carry a message, so SQLite reports
/// them as `SQLITE_ERROR` (`sqlite3_result_error` sets that code), never
/// a code such as `SQLITE_FULL` that would roll back the caller's whole
/// transaction after the function has already undone its own work.
pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "ivm_refresh_all",
        -1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DIRECTONLY,
        |ctx| {
            guard(|| {
                let mode = mode(ctx)?;
                // SAFETY: the handle of the statement calling this function,
                // live for the whole call. The `ConnectionRef` never closes
                // it, stays on this thread and dies before the call returns,
                // and every statement prepared on it is finalized before then
                // (spec §3); nothing here resets or finalizes the calling
                // statement.
                let conn = unsafe { ctx.get_connection() }.map_err(sql_error)?;
                run(&conn, mode)
            })
        },
    )
}

#[derive(Clone, Copy)]
enum Mode {
    Atomic,
    Continue,
}

/// Spec §2: no argument means `'atomic'`; anything else but `'atomic'` or
/// `'continue'`, or more than one argument, is refused.
fn mode(ctx: &Context<'_>) -> Result<Mode> {
    match ctx.len() {
        0 => Ok(Mode::Atomic),
        1 => match ctx.get_raw(0) {
            ValueRef::Text(b"atomic") => Ok(Mode::Atomic),
            ValueRef::Text(b"continue") => Ok(Mode::Continue),
            _ => Err(BAD_ARGUMENT.to_string()),
        },
        _ => Err(BAD_ARGUMENT.to_string()),
    }
}

/// An open `SAVEPOINT "__ivm_refresh_all"` (spec §2). `release` and
/// `roll_back` close it and report their errors. `Drop` is the backstop for
/// a path that leaves it open, an early `?` or a panic's unwinding: it rolls
/// back and releases, and cannot report.
struct Savepoint<'c> {
    conn: &'c Connection,
    open: bool,
}

impl<'c> Savepoint<'c> {
    fn open(conn: &'c Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(&format!("SAVEPOINT {}", quote(SAVEPOINT)))?;
        Ok(Savepoint { conn, open: true })
    }

    fn release(mut self) -> Result<()> {
        self.open = false;
        exec(self.conn, &format!("RELEASE {}", quote(SAVEPOINT)))
    }

    fn roll_back(mut self) -> Result<()> {
        self.open = false;
        roll_back_and_release(self.conn).map_err(sql_error)
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = roll_back_and_release(self.conn);
        }
    }
}

/// Undo everything since the savepoint, then close it. A failed
/// `ROLLBACK TO` stops the batch before the `RELEASE`, which would otherwise
/// keep the writes it failed to undo, so it leaves the savepoint open (spec
/// §2, "When closing the savepoint fails"). In practice the I/O or memory
/// error that makes `ROLLBACK TO` fail has already made SQLite roll back the
/// whole transaction, closing every savepoint with it.
fn roll_back_and_release(conn: &Connection) -> rusqlite::Result<()> {
    let name = quote(SAVEPOINT);
    conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"))
}

/// Spec §2: open the call's savepoint, refresh, then release it, or roll
/// it back if the call fails. Only the `SAVEPOINT` statement's own error is
/// read as a call from a writing statement.
///
/// The mode returns the cause of a failed call, and every failure reads
/// `ivm_refresh_all: <cause>; no view was refreshed` (`failure`). In
/// autocommit the outermost `RELEASE` is the commit, and it can fail, for
/// example with `SQLITE_BUSY` while another connection reads in a
/// rollback-journal database; SQLite then keeps the savepoint and a write
/// transaction open. So when closing the savepoint fails and the call began
/// that transaction, it runs `ROLLBACK` (spec §2, "When closing the
/// savepoint fails"); see `close_failure` for the message.
///
/// When outermost, the call first runs `BEGIN IMMEDIATE` (spec §2, "Waiting
/// for other writers"). A transaction that reads before it writes cannot
/// wait for the write lock: SQLite refuses it with `SQLITE_BUSY` without
/// calling the busy handler. `BEGIN IMMEDIATE` takes the lock first, waiting
/// under `busy_timeout`, and the call ends with `COMMIT`, or `ROLLBACK` on
/// any failure. The calling statement's own reads count too: from
/// `SELECT ivm_refresh_all() FROM t`, which holds a read transaction on `t`,
/// `BEGIN IMMEDIATE` cannot wait either, and a held write lock fails the
/// call at once (measured).
fn run(conn: &Connection, mode: Mode) -> Result<Value> {
    let outermost = conn.is_autocommit();
    if outermost {
        exec(conn, "BEGIN IMMEDIATE").map_err(|e| failure(&e))?;
    }
    let call = match Savepoint::open(conn) {
        Ok(call) => call,
        Err(_) => {
            // From a writing statement in autocommit, `BEGIN` succeeds and
            // the savepoint is refused; the transaction is ours to end.
            if outermost {
                exec(conn, "ROLLBACK").map_err(|rollback| {
                    format!("{NOT_FROM_A_WRITE} (and its ROLLBACK failed: {rollback})")
                })?;
            }
            return Err(NOT_FROM_A_WRITE.to_string());
        }
    };
    let outcome = match mode {
        Mode::Atomic => atomic(conn),
        Mode::Continue => continuing(conn),
    };
    let closed = match outcome {
        Ok(_) => call.release(),
        Err(_) => call.roll_back(),
    }
    .and_then(|()| match (outermost, &outcome) {
        (false, _) => Ok(()),
        (true, Ok(_)) => exec(conn, "COMMIT"),
        (true, Err(_)) => exec(conn, "ROLLBACK"),
    });
    match (outcome, closed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(cause), Ok(())) => Err(failure(&cause)),
        (outcome, Err(close)) => {
            if outermost && !conn.is_autocommit() {
                exec(conn, "ROLLBACK").map_err(|rollback| {
                    format!("ivm_refresh_all: {close}; its ROLLBACK failed too: {rollback}")
                })?;
            }
            Err(close_failure(outcome.err().as_deref(), &close))
        }
    }
}

/// Spec §2: the message of a failed call, `ivm_refresh_all: <cause>; no view
/// was refreshed`.
fn failure(cause: &str) -> String {
    format!("ivm_refresh_all: {cause}; no view was refreshed")
}

/// Spec §2, "When closing the savepoint fails": the call is rolled back
/// whole and fails with `ivm_refresh_all: <close>; no view was refreshed`.
/// When the call had already failed, its cause comes first and the close's
/// error follows: `ivm_refresh_all: <cause>; no view was refreshed (and
/// closing its savepoint failed: <close>)`.
fn close_failure(cause: Option<&str>, close: &str) -> String {
    match cause {
        None => failure(close),
        Some(cause) => format!(
            "{} (and closing its savepoint failed: {close})",
            failure(cause)
        ),
    }
}

/// Spec §2, `'atomic'`: the first failure fails the call, and `run` rolls
/// every view back. Returns the number of views refreshed, skips excluded;
/// a failed view's cause is `view <name>: <its refresh's message>`.
fn atomic(conn: &Connection) -> Result<Value> {
    let mut refreshed = 0;
    for view in views(conn)? {
        match refresh_counted(conn, &view) {
            Ok(Outcome::Refreshed) => refreshed += 1,
            Ok(Outcome::Skipped) => {}
            Err(message) => return Err(format!("view {}: {message}", view.name)),
        }
    }
    Ok(Value::Integer(refreshed))
}

/// Spec §2, `'continue'`: each view inside a savepoint of its own, rolled
/// back when that view fails; the report names every failed view.
fn continuing(conn: &Connection) -> Result<Value> {
    let (mut refreshed, mut skipped) = (0, 0);
    let mut failed = Vec::new();
    for view in views(conn)? {
        let one = Savepoint::open(conn).map_err(sql_error)?;
        match refresh_counted(conn, &view) {
            Ok(outcome) => {
                one.release()?;
                match outcome {
                    Outcome::Refreshed => refreshed += 1,
                    Outcome::Skipped => skipped += 1,
                }
            }
            Err(message) => {
                // A failure that rolled back the whole transaction, such as
                // `RAISE(ROLLBACK)`, took this savepoint with it, so its
                // roll-back fails. The call then fails with this view's own
                // message; the call's close fails for the same reason and
                // `run` adds that error.
                if one.roll_back().is_err() {
                    return Err(format!("view {}: {message}", view.name));
                }
                failed.push((view.name, message));
            }
        }
    }
    Ok(Value::Text(report(refreshed, skipped, &failed)))
}

/// Spec §2: `refreshed <r>, skipped <s>, failed <f>`, then, when `f > 0`,
/// `: <name>: <message>; <name>: <message>` in view order.
fn report(refreshed: usize, skipped: usize, failed: &[(String, String)]) -> String {
    let mut text = format!(
        "refreshed {refreshed}, skipped {skipped}, failed {}",
        failed.len()
    );
    if !failed.is_empty() {
        let each: Vec<String> = failed
            .iter()
            .map(|(name, message)| format!("{name}: {message}"))
            .collect();
        text.push_str(": ");
        text.push_str(&each.join("; "));
    }
    text
}

struct View {
    /// The name as stored, or, for a row whose name is not TEXT, its
    /// `quote()`.
    name: String,
    tables: Vec<String>,
    /// Spec §2, "Only ivm views are refreshed": `false` when `"main"`'s
    /// schema does not list `name` as a virtual table using `ivm`. Such a
    /// row fails with `NOT_AN_IVM_VIEW` and is never written to.
    is_ivm: bool,
}

enum Outcome {
    Refreshed,
    Skipped,
}

/// The views of `main`, by name (spec §3), each with its base tables; none
/// when `__ivm_view` is absent, so a database with no views gets no global
/// tables.
fn views(conn: &Connection) -> Result<Vec<View>> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM \"main\".sqlite_schema \
             WHERE type = 'table' AND name = ?1)",
            [VIEWS],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if !exists {
        return Ok(Vec::new());
    }
    // The stored DDL of the virtual table each row names, if any; a name
    // that is not TEXT (a tampered row) is reported by its `quote()`.
    let rows: Vec<(String, Option<String>)> = conn
        .prepare(&format!(
            "SELECT CASE WHEN typeof(v.name) = 'text' THEN v.name ELSE quote(v.name) END, \
             (SELECT s.sql FROM \"main\".sqlite_schema AS s \
              WHERE s.type = 'table' AND s.name = v.name AND typeof(v.name) = 'text') \
             FROM {} AS v ORDER BY v.name",
            main_qualified(VIEWS)
        ))
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(sql_error)?;
    let mut deps = conn
        .prepare(&format!(
            "SELECT tbl FROM {} WHERE view = ?1 ORDER BY tbl",
            main_qualified(DEPS)
        ))
        .map_err(sql_error)?;
    rows.into_iter()
        .map(|(name, sql)| {
            let is_ivm = sql.as_deref().is_some_and(uses_ivm);
            let tables = if is_ivm {
                deps.query_map([&name], |r| r.get(0))
                    .and_then(|rows| rows.collect())
                    .map_err(sql_error)?
            } else {
                Vec::new()
            };
            Ok(View {
                name,
                tables,
                is_ivm,
            })
        })
        .collect()
}

/// Whether `sql`, a table's DDL as `sqlite_schema` stores it, creates a
/// virtual table using module `ivm`. SQLite stores a virtual table's DDL as
/// `CREATE VIRTUAL TABLE `, then the name as written, then the rest of the
/// statement as written: `USING <module>`, with any whitespace, comments
/// and quoting the statement had.
fn uses_ivm(sql: &str) -> bool {
    let Some(rest) = sql.strip_prefix("CREATE VIRTUAL TABLE ") else {
        return false;
    };
    let Some((_name, rest)) = identifier(rest) else {
        return false;
    };
    let rest = skip_space(rest);
    let Some(rest) = rest
        .get(..5)
        .filter(|word| word.eq_ignore_ascii_case("USING"))
        .map(|_| &rest[5..])
        .filter(|after| !after.starts_with(is_word_char))
    else {
        return false;
    };
    identifier(skip_space(rest)).is_some_and(|(module, _)| module.eq_ignore_ascii_case("ivm"))
}

/// SQLite's identifier characters: ASCII letters, digits, `_`, `$`, and
/// every non-ASCII character.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
}

/// `text` without its leading whitespace and comments.
fn skip_space(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if let Some(comment) = trimmed.strip_prefix("--") {
            text = comment.find('\n').map_or("", |end| &comment[end..]);
        } else if let Some(comment) = trimmed.strip_prefix("/*") {
            text = comment.find("*/").map_or("", |end| &comment[end + 2..]);
        } else {
            return trimmed;
        }
    }
}

/// The identifier at the start of `text`, dequoted, and the text after it:
/// a bare word, or one quoted with `"…"`, `` `…` ``, `'…'` (each doubling
/// its quote to escape it) or `[…]`.
fn identifier(text: &str) -> Option<(String, &str)> {
    let first = text.chars().next()?;
    let close = match first {
        '"' | '`' | '\'' => first,
        '[' => ']',
        c if is_word_char(c) => {
            let end = text.find(|c: char| !is_word_char(c)).unwrap_or(text.len());
            return Some((text[..end].to_string(), &text[end..]));
        }
        _ => return None,
    };
    let mut name = String::new();
    let mut chars = text[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        if c != close {
            name.push(c);
            continue;
        }
        let after = &text[1 + i + c.len_utf8()..];
        if close != ']' && after.starts_with(close) {
            name.push(close);
            chars.next();
        } else {
            return Some((name, after));
        }
    }
    None
}

/// Spec §4: whether any of `view`'s base tables has a delta past the
/// view's watermark. Reads `__ivm_progress` and the delta tables only.
fn behind(conn: &Connection, view: &View) -> Result<bool> {
    for table in &view.tables {
        let applied: i64 = conn
            .query_row(
                &format!(
                    "SELECT applied_seq FROM {} WHERE view = ?1 AND tbl = ?2",
                    main_qualified(PROGRESS)
                ),
                [&view.name, table],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        let latest: Option<i64> = conn
            .query_row(
                &format!(
                    "SELECT MAX({}) FROM {}",
                    quote(DELTA_SEQ),
                    main_qualified(&delta_table(table))
                ),
                [],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        if latest.is_some_and(|seq| seq > applied) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Refresh `view`, counting it as skipped when nothing was past its
/// watermark (spec §4). The refresh runs either way: its checks have the
/// final word, so a broken view fails with its usual message (spec §3),
/// even one whose skip check could not read its tables.
fn refresh_counted(conn: &Connection, view: &View) -> Result<Outcome> {
    if !view.is_ivm {
        return Err(NOT_AN_IVM_VIEW.to_string());
    }
    let behind = behind(conn, view);
    refresh_one(conn, &view.name)?;
    Ok(if behind? {
        Outcome::Refreshed
    } else {
        Outcome::Skipped
    })
}

/// One view's nested refresh, prepared and finalized here (spec §3). Its
/// error is the text SQLite reports for the nested `INSERT`, as a direct
/// refresh would report it.
fn refresh_one(conn: &Connection, name: &str) -> Result<()> {
    let sql = format!(
        "INSERT INTO {}({}) VALUES ('refresh')",
        main_qualified(name),
        quote(name)
    );
    conn.execute(&sql, []).map(|_| ()).map_err(sql_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_close_keeps_the_call_s_own_failure() {
        assert_eq!(
            close_failure(None, "database is locked"),
            "ivm_refresh_all: database is locked; no view was refreshed"
        );
        assert_eq!(
            close_failure(Some("view v: boom"), "database is locked"),
            "ivm_refresh_all: view v: boom; no view was refreshed \
             (and closing its savepoint failed: database is locked)"
        );
    }

    #[test]
    fn only_ivm_virtual_tables_are_ivm_views() {
        for ivm in [
            "CREATE VIRTUAL TABLE v USING ivm('SELECT 1')",
            "CREATE VIRTUAL TABLE \"my view\" using IVM ('SELECT 1')",
            "CREATE VIRTUAL TABLE v USING \"ivm\"('SELECT 1')",
            "CREATE VIRTUAL TABLE [v w] using ivm /* c */ ('SELECT 1')",
            "CREATE VIRTUAL TABLE \"q\"\"x\" -- c\n USING ivm('SELECT 1')",
        ] {
            assert!(uses_ivm(ivm), "{ivm}");
        }
        for other in [
            "CREATE TABLE victim(victim)",
            "CREATE VIRTUAL TABLE v USING fts5(x)",
            "CREATE VIRTUAL TABLE v USING ivmx('SELECT 1')",
            "CREATE VIRTUAL TABLE v USINGivm('SELECT 1')",
            "CREATE VIRTUAL TABLE \"USING ivm\" USING fts5(x)",
            "CREATE VIRTUAL TABLE \"unterminated USING ivm(",
        ] {
            assert!(!uses_ivm(other), "{other}");
        }
    }

    #[test]
    fn the_report_lists_failures_in_order() {
        assert_eq!(report(3, 1, &[]), "refreshed 3, skipped 1, failed 0");
        let failed = [
            ("a".to_string(), "x".to_string()),
            ("b c".to_string(), "y".to_string()),
        ];
        assert_eq!(
            report(1, 0, &failed),
            "refreshed 1, skipped 0, failed 2: a: x; b c: y"
        );
    }
}
