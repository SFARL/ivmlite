//! M1b Phase 7 spec §2–§5: `ivm_refresh_all([mode])` refreshes every view of
//! the database in one call, atomically or view by view, only from a
//! statement that does not write, and never leaves its savepoint open.

mod common;
use common::*;

use std::ffi::{c_int, CStr, CString};
use std::time::{Duration, Instant};

use ivmlite_test::{extension_library, open_with_extension};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
use rusqlite::types::Value;
use rusqlite::{ffi, Connection};

const NOT_FROM_A_WRITE: &str =
    "ivm_refresh_all must be called from a SELECT, not from a statement that writes";
const BAD_ARGUMENT: &str = "ivm_refresh_all takes 'atomic' or 'continue'";

/// Three views, in name order: two on `orders`, one on `regions`. The
/// middle one is where a fault goes, between two healthy views.
const FIRST: (&str, &str) = ("a_sums", SUMS);
const MIDDLE: (&str, &str) = (
    "b_counts",
    "SELECT region, COUNT(*) FROM orders GROUP BY region",
);
const LAST: (&str, &str) = (
    "c_managers",
    "SELECT manager, COUNT(*) FROM regions GROUP BY manager",
);
const VIEWS: [(&str, &str); 3] = [FIRST, MIDDLE, LAST];

/// The three views, created out of name order, so a call that followed
/// creation order instead of name order (spec §3) would report differently.
fn three_views(c: &Connection) {
    setup(c);
    for (name, sql) in [LAST, FIRST, MIDDLE] {
        create(c, name, sql).unwrap();
    }
}

fn write_both(c: &Connection) {
    c.execute_batch(
        "INSERT INTO orders VALUES ('c', 3), ('a', 9); DELETE FROM orders WHERE amount = 5;
         UPDATE regions SET manager = 'cy' WHERE name = 'c'; INSERT INTO regions VALUES ('d', 'dee');",
    )
    .unwrap();
}

fn write_orders(c: &Connection) {
    c.execute_batch("INSERT INTO orders VALUES ('c', 3), ('a', 9)")
        .unwrap();
}

fn all_match_the_oracle(c: &Connection) {
    for (name, sql) in VIEWS {
        assert_matches_oracle(c, name, sql);
    }
}

/// `name` as an SQL identifier.
fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `text` as an SQL string literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Everything a refresh of `name` may change: its output, state and stage
/// tables, and its watermarks.
fn view_state(c: &Connection, name: &str) -> Vec<Vec<Vec<Value>>> {
    let tables: Vec<String> = c
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND (name = ?1 OR name = ?2 \
             OR substr(name, 1, length(?3)) = ?3) ORDER BY name",
        )
        .unwrap()
        .query_map(
            [
                format!("__ivm_out_{name}"),
                format!("__ivm_stage_{name}"),
                format!("__ivm_state_{name}_"),
            ],
            |r| r.get(0),
        )
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(tables.len() >= 3, "{name}: {tables:?}");
    let mut all: Vec<Vec<Vec<Value>>> = tables
        .iter()
        .map(|t| rows(c, &format!("SELECT * FROM {}", ident(t))))
        .collect();
    all.push(rows(
        c,
        &format!(
            "SELECT * FROM __ivm_progress WHERE view = {}",
            literal(name)
        ),
    ));
    all
}

fn every_view_state(c: &Connection) -> Vec<Vec<Vec<Vec<Value>>>> {
    VIEWS.iter().map(|(name, _)| view_state(c, name)).collect()
}

/// `SELECT ivm_refresh_all(<args>)`.
fn call(c: &Connection, args: &str) -> rusqlite::Result<Value> {
    c.query_row(&format!("SELECT ivm_refresh_all({args})"), [], |r| r.get(0))
}

fn text(v: Value) -> String {
    match v {
        Value::Text(t) => t,
        other => panic!("expected TEXT, got {other:?}"),
    }
}

/// A `RAISE(ABORT)` on every insert into `view`'s output table, so its
/// apply fails.
fn inject_fault(c: &Connection, view: &str) {
    c.execute_batch(&format!(
        "CREATE TRIGGER fault BEFORE INSERT ON {} BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        ident(&format!("__ivm_out_{view}"))
    ))
    .unwrap();
}

/// The message a direct refresh of `view` fails with.
fn direct_failure(c: &Connection, view: &str) -> String {
    c.execute_batch(&format!(
        "INSERT INTO {0}({0}) VALUES ('refresh')",
        ident(view)
    ))
    .expect_err("the direct refresh must fail too")
    .to_string()
}

#[test]
fn atomic_refreshes_every_view_and_counts_them() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    write_both(&c);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
    all_match_the_oracle(&c);
    write_both(&c);
    assert_eq!(call(&c, "'atomic'").unwrap(), Value::Integer(3));
    all_match_the_oracle(&c);
    assert!(c.is_autocommit(), "the call left its savepoint open");
}

/// Spec §2: one failed refresh rolls every view back, the healthy ones
/// refreshed before it included, in autocommit and inside a transaction,
/// which stays usable and commits.
#[test]
fn atomic_failure_refreshes_no_view() {
    for explicit_transaction in [false, true] {
        let c = open_with_extension(None).unwrap();
        three_views(&c);
        inject_fault(&c, MIDDLE.0);
        if explicit_transaction {
            c.execute_batch("BEGIN").unwrap();
        }
        write_both(&c);
        let before = every_view_state(&c);
        let err = call(&c, "").expect_err("the fault must fail the call");
        // `SQLITE_ERROR` (a function error with a message always is), not a
        // code that makes SQLite roll back the whole transaction.
        match &err {
            rusqlite::Error::SqliteFailure(e, _) => assert_eq!(e.extended_code, ffi::SQLITE_ERROR),
            other => panic!("{other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains("view b_counts: ") && message.contains("no view was refreshed"),
            "{message}"
        );
        assert_eq!(
            every_view_state(&c),
            before,
            "a failed atomic call changed a view (transaction: {explicit_transaction})"
        );
        if explicit_transaction {
            assert!(!c.is_autocommit(), "the transaction must still be open");
            c.execute_batch("COMMIT").unwrap();
            assert_eq!(every_view_state(&c), before);
        }
        assert!(c.is_autocommit(), "the call left its savepoint open");
        // Last: a direct refresh that fails inside a transaction leaves its
        // stage half-written (Phase 3a spec §5), so it runs after the checks.
        assert_eq!(
            message,
            format!(
                "ivm_refresh_all: view b_counts: {}; no view was refreshed",
                direct_failure(&c, MIDDLE.0)
            )
        );
        c.execute_batch("DROP TRIGGER fault").unwrap();
        assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
        all_match_the_oracle(&c);
    }
}

/// Spec §2: a failed view is rolled back to its own savepoint, stage
/// included, and reported with the message a direct refresh gives; the
/// healthy views are refreshed.
#[test]
fn continue_reports_the_failed_view_and_refreshes_the_rest() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    // A first refresh leaves rows in the middle view's stage, so a stage
    // left half-written by the failed apply below differs from it.
    write_both(&c);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
    inject_fault(&c, MIDDLE.0);
    write_both(&c);
    let before = view_state(&c, MIDDLE.0);
    let report = text(call(&c, "'continue'").unwrap());
    assert_eq!(view_state(&c, MIDDLE.0), before, "the failed view changed");
    assert_matches_oracle(&c, FIRST.0, FIRST.1);
    assert_matches_oracle(&c, LAST.0, LAST.1);
    assert_eq!(
        report,
        format!(
            "refreshed 2, skipped 0, failed 1: b_counts: {}",
            direct_failure(&c, MIDDLE.0)
        )
    );
    assert!(c.is_autocommit(), "the call left a savepoint open");
}

/// Spec §3: a broken view (a dropped capture trigger) fails the atomic
/// call and is reported failed by the continue call, with its usual
/// message.
#[test]
fn a_broken_view_fails_atomic_and_is_reported_by_continue() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    write_both(&c);
    c.execute_batch("DROP TRIGGER __ivm_trig_regions_ins")
        .unwrap();
    let before = every_view_state(&c);
    let direct = direct_failure(&c, LAST.0);
    assert!(direct.contains("cannot be maintained"), "{direct}");
    let err = call(&c, "").expect_err("a broken view fails the atomic call");
    assert_eq!(
        err.to_string(),
        format!("ivm_refresh_all: view c_managers: {direct}; no view was refreshed")
    );
    assert_eq!(every_view_state(&c), before);
    let report = text(call(&c, "'continue'").unwrap());
    assert_eq!(
        report,
        format!("refreshed 2, skipped 0, failed 1: c_managers: {direct}")
    );
    assert_matches_oracle(&c, FIRST.0, FIRST.1);
    assert_matches_oracle(&c, MIDDLE.0, MIDDLE.1);
}

/// Review Focus 1: the skip check of a view whose delta table is gone
/// fails, and so does its refresh, whose checks report the view broken with
/// their usual message; the call closes its savepoint either way.
#[test]
fn a_skip_check_that_cannot_read_reports_the_refresh_message() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    write_both(&c);
    c.execute_batch("DROP TABLE __ivm_delta_regions").unwrap();
    let before = every_view_state(&c);
    let direct = direct_failure(&c, LAST.0);
    assert!(direct.contains("cannot be maintained"), "{direct}");
    let err = call(&c, "").expect_err("the view is broken");
    assert_eq!(
        err.to_string(),
        format!("ivm_refresh_all: view c_managers: {direct}; no view was refreshed")
    );
    assert_eq!(every_view_state(&c), before);
    assert!(c.is_autocommit(), "the call left its savepoint open");
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        format!("refreshed 2, skipped 0, failed 1: c_managers: {direct}")
    );
    assert!(c.is_autocommit(), "the call left a savepoint open");
}

/// Spec §2: inside a writing statement SQLite refuses the savepoint, and
/// the function refuses the call, changing nothing.
#[test]
fn called_from_a_writing_statement_is_refused() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    c.execute_batch("CREATE TABLE t(x)").unwrap();
    write_both(&c);
    let before = every_view_state(&c);
    for args in ["", "'continue'"] {
        let err = c
            .execute_batch(&format!("INSERT INTO t SELECT ivm_refresh_all({args})"))
            .expect_err("a writing statement must be refused");
        assert_eq!(err.to_string(), NOT_FROM_A_WRITE);
        assert_eq!(every_view_state(&c), before);
        assert_eq!(count(&c, "SELECT count(*) FROM t"), 0);
        assert!(c.is_autocommit());
    }
}

/// Spec §2: `SQLITE_DIRECTONLY` keeps the function out of trigger bodies.
#[test]
fn called_from_a_trigger_is_refused() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    write_both(&c);
    let before = every_view_state(&c);
    c.execute_batch(
        "CREATE TABLE t(x);
         CREATE TRIGGER calls AFTER INSERT ON t BEGIN SELECT ivm_refresh_all(); END;",
    )
    .unwrap();
    let err = c
        .execute_batch("INSERT INTO t VALUES (1)")
        .expect_err("SQLite must refuse the function in a trigger");
    assert!(
        err.to_string().contains("unsafe use of ivm_refresh_all()"),
        "{err}"
    );
    assert_eq!(every_view_state(&c), before);
}

#[test]
fn a_bad_argument_is_refused() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    write_both(&c);
    let before = every_view_state(&c);
    for args in [
        "'x'",
        "1, 2",
        "NULL",
        "1",
        "'ATOMIC'",
        "'atomic', 'continue'",
    ] {
        let err = call(&c, args).expect_err(args);
        assert_eq!(err.to_string(), BAD_ARGUMENT, "{args}");
    }
    assert_eq!(every_view_state(&c), before);
    assert!(c.is_autocommit());
}

/// Spec §2: the savepoint nests in the caller's transaction, so its
/// `ROLLBACK` undoes the call.
#[test]
fn inside_a_rolled_back_transaction_nothing_is_kept() {
    for args in ["", "'continue'"] {
        let c = open_with_extension(None).unwrap();
        three_views(&c);
        write_both(&c);
        let before = every_view_state(&c);
        c.execute_batch("BEGIN").unwrap();
        call(&c, args).unwrap();
        assert_ne!(every_view_state(&c), before, "{args}: the call refreshed");
        c.execute_batch("ROLLBACK").unwrap();
        assert_eq!(every_view_state(&c), before, "{args}");
    }
}

/// Spec §3: no views, no work, and no global tables created.
#[test]
fn no_views_returns_zero_and_creates_nothing() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x)").unwrap();
    let schema = || rows(&c, "SELECT * FROM sqlite_schema");
    let before = schema();
    assert_eq!(call(&c, "").unwrap(), Value::Integer(0));
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 0, skipped 0, failed 0"
    );
    assert_eq!(schema(), before);
    assert!(c.is_autocommit());
}

/// Spec §4: a view none of whose base tables has a delta past its
/// watermark is counted as skipped, in both modes.
#[test]
fn views_with_nothing_new_are_skipped() {
    // A join view, last in name order, is behind as soon as either of its
    // tables is: a write to `orders` alone or to `regions` alone refreshes it.
    let joined = ("d_joined", JOIN);
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    create(&c, joined.0, joined.1).unwrap();
    write_orders(&c);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
    all_match_the_oracle(&c);
    assert_matches_oracle(&c, joined.0, joined.1);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(0));
    c.execute_batch("UPDATE regions SET manager = 'zed' WHERE name = 'a'")
        .unwrap();
    assert_eq!(call(&c, "").unwrap(), Value::Integer(2));
    all_match_the_oracle(&c);
    assert_matches_oracle(&c, joined.0, joined.1);

    let c = open_with_extension(None).unwrap();
    three_views(&c);
    create(&c, joined.0, joined.1).unwrap();
    write_orders(&c);
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 3, skipped 1, failed 0"
    );
    all_match_the_oracle(&c);
    assert_matches_oracle(&c, joined.0, joined.1);
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 0, skipped 4, failed 0"
    );
    c.execute_batch("UPDATE regions SET manager = 'zed' WHERE name = 'a'")
        .unwrap();
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 2, skipped 2, failed 0"
    );
    assert_matches_oracle(&c, joined.0, joined.1);
}

/// Spec §4: a recreated base table has lost its capture triggers and has
/// no deltas; its views still run their checks and are reported broken,
/// never skipped.
#[test]
fn a_recreated_base_table_is_reported_not_skipped() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    c.execute_batch(
        "DROP TABLE regions;
         CREATE TABLE regions(name TEXT, manager TEXT) STRICT;",
    )
    .unwrap();
    let direct = direct_failure(&c, LAST.0);
    assert!(
        direct.contains("its capture trigger __ivm_trig_regions_ins is missing"),
        "{direct}"
    );
    let err = call(&c, "").expect_err("the recreated table breaks its view");
    assert_eq!(
        err.to_string(),
        format!("ivm_refresh_all: view c_managers: {direct}; no view was refreshed")
    );
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        format!("refreshed 0, skipped 2, failed 1: c_managers: {direct}")
    );
}

/// A raw handle on `path` with the extension loaded, so the test can close
/// it with `sqlite3_close` itself (as in `extension_statement_cache.rs`).
fn open_raw(path: &std::path::Path) -> *mut ffi::sqlite3 {
    let name = CString::new(path.to_str().unwrap()).unwrap();
    let mut db = std::ptr::null_mut();
    // SAFETY: a fresh handle, opened read-write and created if missing.
    let rc = unsafe {
        ffi::sqlite3_open_v2(
            name.as_ptr(),
            &mut db,
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
            std::ptr::null(),
        )
    };
    assert_eq!(rc, ffi::SQLITE_OK, "open");
    with_raw(db, |c| {
        // SAFETY: loading our own library, whose entry point only registers
        // the `ivm` module and the `ivm_refresh_all` function.
        unsafe {
            c.load_extension_enable().unwrap();
            c.load_extension(extension_library(), Some("sqlite3_ivmlite_init"))
                .unwrap();
            c.load_extension_disable().unwrap();
        }
    });
    db
}

/// Run `body` on a non-owning `Connection` over `db`.
fn with_raw(db: *mut ffi::sqlite3, body: impl FnOnce(&Connection)) {
    // SAFETY: `db` is a live handle this test opened and has not closed.
    let c = unsafe { Connection::from_handle(db) }.unwrap();
    body(&c);
}

/// `sqlite3_close`, not `close_v2`: it fails with `SQLITE_BUSY` while any
/// statement of the connection is unfinalized.
fn close_v1(db: *mut ffi::sqlite3) -> c_int {
    // SAFETY: `db` is live, and nothing uses it after this call.
    unsafe { ffi::sqlite3_close(db) }
}

type CloseCase<'a> = (&'a str, &'a dyn Fn(&Connection));

/// Spec §3, §5: the function finalizes every statement it prepared, its
/// nested refreshes included, and closes its savepoint, so `sqlite3_close`
/// succeeds after any kind of call.
#[test]
fn close_v1_succeeds_after_every_kind_of_call() {
    let file = TempFile::new("refresh-all-close");
    let cases: [CloseCase; 5] = [
        ("a successful call", &|c| {
            three_views(c);
            write_both(c);
            assert_eq!(call(c, "").unwrap(), Value::Integer(3));
            assert_eq!(
                text(call(c, "'continue'").unwrap()),
                "refreshed 0, skipped 3, failed 0"
            );
        }),
        ("an atomic failure", &|c| {
            inject_fault(c, MIDDLE.0);
            write_both(c);
            call(c, "").expect_err("the fault fails the call");
        }),
        ("continue with a failure", &|c| {
            write_both(c);
            let report = text(call(c, "'continue'").unwrap());
            assert!(
                report.starts_with("refreshed 2, skipped 0, failed 1"),
                "{report}"
            );
        }),
        ("refused from a writing statement", &|c| {
            c.execute_batch("CREATE TABLE t(x)").unwrap();
            c.execute_batch("INSERT INTO t SELECT ivm_refresh_all()")
                .expect_err("refused");
        }),
        ("a bad argument", &|c| {
            call(c, "'x'").expect_err("refused");
        }),
    ];
    for (case, body) in cases {
        let db = open_raw(file.path());
        with_raw(db, |c| {
            body(c);
            assert!(c.is_autocommit(), "{case}: a savepoint was left open");
        });
        assert_eq!(close_v1(db), ffi::SQLITE_OK, "{case}");
    }
}

/// Review Focus 2: a view dropped by another connection after this one read
/// `__ivm_view`. Another connection cannot drop a view while this one's
/// call is under way, because the call holds the write lock from its
/// `BEGIN IMMEDIATE` on, so two cases are tested, both on a WAL database:
/// 1. A holds a read statement, so its snapshot still lists the view when
///    B drops it. A's call cannot take the write lock from a snapshot that
///    is no longer the latest, so its `BEGIN IMMEDIATE` fails at once, in
///    both modes, and nothing is changed.
/// 2. Once A's statement ends, the next call sees B's drop: the dropped view
///    is not listed and the rest refresh.
#[test]
fn a_stale_snapshot_of_a_dropped_view_fails_the_call_as_locked() {
    let file = TempFile::new("refresh-all-dropped");
    let a = open_with_extension(Some(file.path())).unwrap();
    let mode: String = a
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    three_views(&a);
    write_both(&a);
    let b = open_with_extension(Some(file.path())).unwrap();

    let mut held = a.prepare("SELECT name FROM __ivm_view").unwrap();
    let mut listed = held.query([]).unwrap();
    assert!(listed.next().unwrap().is_some());
    b.execute_batch(&format!("DROP TABLE {}", MIDDLE.0))
        .unwrap();

    let before = every_view_state(&a);
    let started = Instant::now();
    for args in ["", "'continue'"] {
        let err = call(&a, args).expect_err("A's snapshot is stale, so it cannot write");
        assert_eq!(
            err.to_string(),
            "ivm_refresh_all: database is locked; no view was refreshed",
            "{args}"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the calls waited {:?}",
        started.elapsed()
    );
    assert_eq!(every_view_state(&a), before);
    drop(listed);
    drop(held);
    assert!(a.is_autocommit());

    assert_eq!(call(&a, "").unwrap(), Value::Integer(2));
    assert_matches_oracle(&a, FIRST.0, FIRST.1);
    assert_matches_oracle(&a, LAST.0, LAST.1);
    assert_eq!(
        count(
            &a,
            "SELECT count(*) FROM __ivm_view WHERE name = 'b_counts'"
        ),
        0
    );
}

/// Spec §2, "When closing the savepoint fails": in a rollback-journal
/// database, the call's `COMMIT` fails with `SQLITE_BUSY` while another
/// connection reads. The call then rolls back whole and returns the
/// connection to autocommit, after a successful atomic call and a continue
/// call. An atomic call that had already failed ends with `ROLLBACK`, which
/// a reader cannot refuse, and fails with its own message only.
#[test]
fn a_commit_refused_by_a_reader_rolls_the_call_back_whole() {
    let file = TempFile::new("refresh-all-busy");
    let cases = [
        ("", false, None),
        ("'continue'", false, None),
        ("", true, Some(MIDDLE.0)),
    ];
    for (args, with_fault, failing) in cases {
        let _ = std::fs::remove_file(file.path());
        let db = open_raw(file.path());
        with_raw(db, |a| {
            let mode: String = a
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "delete");
            three_views(a);
            if with_fault {
                inject_fault(a, MIDDLE.0);
            }
            write_both(a);
            a.busy_timeout(Duration::ZERO).unwrap();
            let b = Connection::open(file.path()).unwrap();
            let before = every_view_state(&b);
            let mut held = b.prepare("SELECT * FROM orders").unwrap();
            let mut reading = held.query([]).unwrap();
            assert!(reading.next().unwrap().is_some());

            let err = call(a, args).expect_err("the commit must be refused");
            assert!(a.is_autocommit(), "{args}: a transaction was left open");
            drop(reading);
            drop(held);
            assert_eq!(every_view_state(&b), before, "{args}: a view changed");
            let stray = a
                .execute_batch("RELEASE \"__ivm_refresh_all\"")
                .expect_err("no savepoint may be left");
            assert_eq!(stray.to_string(), "no such savepoint: __ivm_refresh_all");
            let expected = match failing {
                None => "ivm_refresh_all: database is locked; no view was refreshed".to_string(),
                Some(view) => format!(
                    "ivm_refresh_all: view {view}: {}; no view was refreshed",
                    direct_failure(a, view)
                ),
            };
            assert_eq!(err.to_string(), expected, "{args}");
        });
        assert_eq!(close_v1(db), ffi::SQLITE_OK, "{args}");
    }
}

/// Refuse one RELEASE, allowing the cleanup that follows. Even inside the
/// application's transaction, a failed call must undo every refresh, close
/// its own savepoints, and preserve the caller's writes and savepoint.
fn assert_refused_release_rolls_back_call(args: &str, refused_release: usize, keep_refusing: bool) {
    use std::cell::Cell;

    for explicit_transaction in [true, false] {
        let c = open_with_extension(None).unwrap();
        three_views(&c);
        if explicit_transaction {
            c.execute_batch("BEGIN; SAVEPOINT application").unwrap();
        }
        write_both(&c);
        let before = every_view_state(&c);
        let tables = [
            "orders",
            "regions",
            "__ivm_delta_orders",
            "__ivm_delta_regions",
            "sqlite_sequence",
        ];
        let base_and_capture = || {
            tables
                .iter()
                .map(|table| rows(&c, &format!("SELECT * FROM {}", ident(table))))
                .collect::<Vec<_>>()
        };
        let before_base_and_capture = base_and_capture();
        let releases = Cell::new(0);
        c.authorizer(Some(move |ctx: AuthContext<'_>| {
            if let AuthAction::Savepoint {
                operation: TransactionOperation::Release,
                savepoint_name: "__ivm_refresh_all" | "__ivm_refresh_all_view",
            } = ctx.action
            {
                releases.set(releases.get() + 1);
                if releases.get() == refused_release
                    || (keep_refusing && releases.get() > refused_release)
                {
                    return Authorization::Deny;
                }
            }
            Authorization::Allow
        }))
        .unwrap();
        let err = call(&c, args).expect_err("the authorizer must refuse RELEASE");
        c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let expected = if keep_refusing {
            "ivm_refresh_all: not authorized; no view was refreshed \
             (and closing its savepoint failed: not authorized)"
        } else {
            "ivm_refresh_all: not authorized; no view was refreshed"
        };
        assert_eq!(err.to_string(), expected);
        assert_eq!(every_view_state(&c), before, "{args}: a view changed");
        assert_eq!(base_and_capture(), before_base_and_capture);
        if keep_refusing && explicit_transaction {
            // The host forbade closing any savepoint. Once that policy is
            // lifted, the caller can close the rolled-back outer savepoint.
            c.execute_batch("RELEASE __ivm_refresh_all").unwrap();
        } else {
            let stray = c
                .execute_batch("RELEASE __ivm_refresh_all")
                .expect_err("the call must leave no savepoint");
            assert_eq!(stray.to_string(), "no such savepoint: __ivm_refresh_all");
        }
        let stray = c
            .execute_batch("RELEASE __ivm_refresh_all_view")
            .expect_err("the call must leave no per-view savepoint");
        assert_eq!(
            stray.to_string(),
            "no such savepoint: __ivm_refresh_all_view"
        );
        assert_eq!(c.is_autocommit(), !explicit_transaction);
        if explicit_transaction {
            c.execute_batch("RELEASE application; COMMIT").unwrap();
            assert_eq!(every_view_state(&c), before);
            assert_eq!(base_and_capture(), before_base_and_capture);
        }
        assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
        all_match_the_oracle(&c);
        c.close().unwrap();
    }
}

#[test]
fn atomic_refused_release_rolls_the_call_back_whole() {
    assert_refused_release_rolls_back_call("", 1, false);
}

#[test]
fn continue_refused_release_rolls_the_call_back_whole() {
    // A per-view close can fail before or after a successful view; the last
    // RELEASE closes the outer call after all three views succeeded.
    for refused_release in [1, 2, 4] {
        assert_refused_release_rolls_back_call("'continue'", refused_release, false);
    }
}

#[test]
fn continue_refused_release_cleanup_still_rolls_back_the_outer_call() {
    // The second view's RELEASE and all cleanup RELEASEs are refused. The
    // outer rollback must still undo the first view, not just the second.
    assert_refused_release_rolls_back_call("'continue'", 2, true);
}

#[test]
fn a_refused_rollback_is_retried_and_never_claims_an_unconfirmed_rollback() {
    use std::cell::Cell;

    for keep_refusing in [false, true] {
        let c = open_with_extension(None).unwrap();
        three_views(&c);
        inject_fault(&c, MIDDLE.0);
        c.execute_batch("BEGIN; SAVEPOINT application").unwrap();
        write_both(&c);
        let before = every_view_state(&c);
        let rollbacks = Cell::new(0);
        c.authorizer(Some(move |ctx: AuthContext<'_>| {
            if let AuthAction::Savepoint {
                operation: TransactionOperation::Rollback,
                savepoint_name: "__ivm_refresh_all",
            } = ctx.action
            {
                rollbacks.set(rollbacks.get() + 1);
                if rollbacks.get() == 1 || keep_refusing {
                    return Authorization::Deny;
                }
            }
            Authorization::Allow
        }))
        .unwrap();
        let err = call(&c, "").expect_err("the fault must fail the call");
        c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let message = err.to_string();
        assert!(message.contains("view b_counts:"), "{message}");
        assert!(message.contains("closing its savepoint failed: not authorized"));
        if keep_refusing {
            assert!(message.contains("refreshed views may remain"), "{message}");
            assert!(!message.contains("no view was refreshed"), "{message}");
            assert_ne!(view_state(&c, FIRST.0), before[0]);
            // Once the host permits cleanup, it can undo just this call.
            c.execute_batch("ROLLBACK TO __ivm_refresh_all; RELEASE __ivm_refresh_all")
                .unwrap();
        } else {
            assert!(message.contains("no view was refreshed"), "{message}");
            let stray = c
                .execute_batch("RELEASE __ivm_refresh_all")
                .expect_err("the retry must close the call's savepoint");
            assert_eq!(stray.to_string(), "no such savepoint: __ivm_refresh_all");
        }
        assert_eq!(every_view_state(&c), before);
        assert!(!c.is_autocommit());
        c.execute_batch("RELEASE application; COMMIT").unwrap();
        assert_eq!(every_view_state(&c), before);
        c.execute_batch("DROP TRIGGER fault").unwrap();
        assert_eq!(call(&c, "").unwrap(), Value::Integer(3));
        all_match_the_oracle(&c);
        c.close().unwrap();
    }
}

/// Spec §3: failed views are reported in name order, not creation order.
#[test]
fn failures_are_reported_in_name_order() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    for view in [LAST.0, FIRST.0] {
        c.execute_batch(&format!(
            "CREATE TRIGGER {} BEFORE INSERT ON {} BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            ident(&format!("fault_{view}")),
            ident(&format!("__ivm_out_{view}"))
        ))
        .unwrap();
    }
    write_both(&c);
    let report = text(call(&c, "'continue'").unwrap());
    let err = call(&c, "").expect_err("the faults fail the call");
    assert_eq!(
        report,
        format!(
            "refreshed 1, skipped 0, failed 2: a_sums: {}; c_managers: {}",
            direct_failure(&c, FIRST.0),
            direct_failure(&c, LAST.0)
        )
    );
    assert!(
        err.to_string()
            .starts_with("ivm_refresh_all: view a_sums: "),
        "{err}"
    );
}

/// Spec §2, "Waiting for other writers": in autocommit the call takes the
/// write lock with `BEGIN IMMEDIATE`, so it waits under `busy_timeout` while
/// another connection briefly holds it, as a direct refresh does. From
/// `SELECT ivm_refresh_all() FROM t`, whose own read transaction is open,
/// SQLite cannot wait, and the call fails at once.
#[test]
fn the_call_waits_for_another_writer() {
    const HELD: Duration = Duration::from_millis(300);
    for journal in ["delete", "wal"] {
        for args in ["", "'continue'"] {
            let file = TempFile::new(&format!("refresh-all-wait-{journal}"));
            let a = open_with_extension(Some(file.path())).unwrap();
            let mode: String = a
                .query_row(&format!("PRAGMA journal_mode = {journal}"), [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(mode, journal);
            three_views(&a);
            write_both(&a);
            a.busy_timeout(Duration::from_secs(5)).unwrap();
            let (locked, holding) = std::sync::mpsc::channel();
            let path = file.path().to_path_buf();
            let writer = std::thread::spawn(move || {
                let b = Connection::open(path).unwrap();
                b.execute_batch("BEGIN IMMEDIATE; INSERT INTO regions VALUES ('w', 'wes');")
                    .unwrap();
                locked.send(()).unwrap();
                std::thread::sleep(HELD);
                b.execute_batch("COMMIT").unwrap();
            });
            holding.recv().unwrap();
            let started = Instant::now();
            let outcome = call(&a, args);
            let waited = started.elapsed();
            writer.join().unwrap();
            let outcome = outcome.unwrap_or_else(|e| panic!("{journal} {args}: {e}"));
            assert!(
                waited >= HELD / 2 && waited < Duration::from_secs(4),
                "{journal} {args}: returned after {waited:?}"
            );
            match args {
                "" => assert_eq!(outcome, Value::Integer(3)),
                _ => assert_eq!(text(outcome), "refreshed 3, skipped 0, failed 0"),
            }
            all_match_the_oracle(&a);

            // The calling statement's own read transaction: no wait.
            let (locked, holding) = std::sync::mpsc::channel();
            let path = file.path().to_path_buf();
            let writer = std::thread::spawn(move || {
                let b = Connection::open(path).unwrap();
                b.execute_batch("BEGIN IMMEDIATE; INSERT INTO regions VALUES ('x', 'xan');")
                    .unwrap();
                locked.send(()).unwrap();
                std::thread::sleep(HELD);
                b.execute_batch("COMMIT").unwrap();
            });
            holding.recv().unwrap();
            let started = Instant::now();
            let err = a
                .query_row(
                    &format!("SELECT ivm_refresh_all({args}) FROM orders LIMIT 1"),
                    [],
                    |r| r.get::<_, Value>(0),
                )
                .expect_err("a read transaction cannot wait for the write lock");
            let waited = started.elapsed();
            writer.join().unwrap();
            assert_eq!(
                err.to_string(),
                "ivm_refresh_all: database is locked; no view was refreshed"
            );
            assert!(waited < HELD, "{journal} {args}: waited {waited:?}");
            assert!(a.is_autocommit());
        }
    }
}

/// Spec §2: a nested failure that rolls back the whole transaction, here
/// `RAISE(ROLLBACK)`, takes the call's savepoints with it. Both modes fail
/// the call with the view's own message and the savepoint's error, nothing
/// is kept, and inside the application's transaction that transaction ends.
#[test]
fn a_failure_that_rolls_back_the_transaction_fails_the_call() {
    for explicit_transaction in [false, true] {
        for args in ["", "'continue'"] {
            let c = open_with_extension(None).unwrap();
            three_views(&c);
            c.execute_batch(
                "CREATE TRIGGER fault BEFORE INSERT ON __ivm_out_b_counts \
                 BEGIN SELECT RAISE(ROLLBACK, 'rolled back'); END;",
            )
            .unwrap();
            write_both(&c);
            let before = every_view_state(&c);
            if explicit_transaction {
                c.execute_batch("BEGIN").unwrap();
            }
            let err = call(&c, args).expect_err("the fault fails the call");
            assert!(c.is_autocommit(), "the transaction was rolled back");
            assert_eq!(every_view_state(&c), before);
            assert_eq!(
                err.to_string(),
                format!(
                    "ivm_refresh_all: view b_counts: {}; no view was refreshed \
                     (and closing its savepoint failed: no such savepoint: __ivm_refresh_all)",
                    direct_failure(&c, MIDDLE.0)
                ),
                "{args} (transaction: {explicit_transaction})"
            );
        }
    }
}

/// Spec §2, "Only ivm views are refreshed": a `__ivm_view` row naming an
/// ordinary table, or a name that is not TEXT, fails as `not an ivmlite
/// view`, and nothing is written to the table it names.
#[test]
fn a_forged_view_row_is_never_refreshed() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    c.execute_batch(
        "CREATE TABLE victim(victim TEXT);
         INSERT INTO __ivm_view(name, sql, plan, declaration, format)
             SELECT 'victim', sql, plan, declaration, format FROM __ivm_view WHERE name = 'a_sums';",
    )
    .unwrap();
    write_both(&c);
    let before = every_view_state(&c);
    let err = call(&c, "").expect_err("the forged row fails the atomic call");
    assert_eq!(
        err.to_string(),
        "ivm_refresh_all: view victim: not an ivmlite view; no view was refreshed"
    );
    assert_eq!(every_view_state(&c), before);
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 3, skipped 0, failed 1: victim: not an ivmlite view"
    );
    all_match_the_oracle(&c);
    assert_eq!(count(&c, "SELECT count(*) FROM victim"), 0);

    c.execute_batch(
        "INSERT INTO __ivm_view(name, sql, plan, declaration, format)
             SELECT NULL, sql, plan, declaration, format FROM __ivm_view WHERE name = 'a_sums';
         INSERT INTO __ivm_view(name, sql, plan, declaration, format)
             SELECT X'00', sql, plan, declaration, format FROM __ivm_view WHERE name = 'a_sums';",
    )
    .unwrap();
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 0, skipped 3, failed 3: NULL: not an ivmlite view; \
         victim: not an ivmlite view; X'00': not an ivmlite view"
    );
    assert_eq!(count(&c, "SELECT count(*) FROM victim"), 0);
}

/// Review Focus 3: names that need quoting refresh through the function,
/// and the continue report prints a failed view's name as stored.
#[test]
fn quoted_view_names_refresh() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let views = [("my view", SUMS), ("q\"x", MIDDLE.1)];
    for (name, sql) in views {
        c.execute_batch(&format!(
            "CREATE VIRTUAL TABLE {} USING ivm({})",
            ident(name),
            literal(sql)
        ))
        .unwrap();
    }
    write_orders(&c);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(2));
    for (name, sql) in views {
        assert_matches_oracle(&c, &ident(name), sql);
    }
    inject_fault(&c, "q\"x");
    write_orders(&c);
    let report = text(call(&c, "'continue'").unwrap());
    assert_eq!(
        report,
        format!(
            "refreshed 1, skipped 0, failed 1: q\"x: {}",
            direct_failure(&c, "q\"x")
        )
    );
    assert_matches_oracle(&c, &ident("my view"), SUMS);
}

/// Review Focus 4: a second call in the same transaction opens its own
/// savepoint inside it, and the transaction commits both.
#[test]
fn two_calls_in_one_transaction() {
    let c = open_with_extension(None).unwrap();
    three_views(&c);
    c.execute_batch("BEGIN").unwrap();
    write_orders(&c);
    assert_eq!(call(&c, "").unwrap(), Value::Integer(2));
    c.execute_batch("INSERT INTO regions VALUES ('e', 'eve')")
        .unwrap();
    assert_eq!(
        text(call(&c, "'continue'").unwrap()),
        "refreshed 1, skipped 2, failed 0"
    );
    assert!(!c.is_autocommit());
    c.execute_batch("COMMIT").unwrap();
    all_match_the_oracle(&c);
}

/// A user function, not `SQLITE_DIRECTONLY`, that runs
/// `SELECT ivm_refresh_all()` on its own connection and fails with that
/// call's error.
unsafe extern "C" fn reenter(
    ctx: *mut ffi::sqlite3_context,
    _argc: c_int,
    _argv: *mut *mut ffi::sqlite3_value,
) {
    let outcome = std::panic::catch_unwind(|| {
        // SAFETY: the handle of the statement calling this function; the
        // non-owning `Connection` never closes it and dies within the call.
        let c = unsafe { Connection::from_handle(ffi::sqlite3_context_db_handle(ctx)) }
            .map_err(|e| e.to_string())?;
        call(&c, "").map(|_| ()).map_err(|e| e.to_string())
    });
    let message = match outcome {
        Ok(Ok(())) => {
            // SAFETY: `ctx` is this call's live context.
            unsafe { ffi::sqlite3_result_int(ctx, 0) };
            return;
        }
        Ok(Err(message)) => message,
        Err(_) => "reenter panicked".to_string(),
    };
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: `ctx` is this call's live context; SQLite copies the message.
    unsafe { ffi::sqlite3_result_error(ctx, message.as_ptr(), -1) };
}

fn register_reenter(c: &Connection) {
    let name: &CStr = c"reenter";
    // SAFETY: `c`'s own live handle; `reenter` uses no user data.
    let rc = unsafe {
        ffi::sqlite3_create_function_v2(
            c.handle(),
            name.as_ptr(),
            0,
            ffi::SQLITE_UTF8,
            std::ptr::null_mut(),
            Some(reenter),
            None,
            None,
            None,
        )
    };
    assert_eq!(rc, ffi::SQLITE_OK);
}

/// Review Focus 4: the function nested inside itself. A trigger on a view's
/// output table that calls it is refused by `SQLITE_DIRECTONLY`; one that
/// reaches it through a user function runs it inside the view's nested
/// refresh, a writing statement, where its savepoint is refused. Either way
/// the outer call reports that view as failed and refreshes the others.
#[test]
fn a_call_nested_in_a_refresh_is_refused_and_reported() {
    let cases = [
        (
            "SELECT ivm_refresh_all()",
            "unsafe use of ivm_refresh_all()",
        ),
        ("SELECT reenter()", NOT_FROM_A_WRITE),
    ];
    for (body, expected) in cases {
        let c = open_with_extension(None).unwrap();
        register_reenter(&c);
        three_views(&c);
        c.execute_batch(&format!(
            "CREATE TRIGGER nested BEFORE INSERT ON __ivm_out_b_counts BEGIN {body}; END;"
        ))
        .unwrap();
        write_both(&c);
        let before = view_state(&c, MIDDLE.0);
        let report = text(call(&c, "'continue'").unwrap());
        assert!(
            report.starts_with("refreshed 2, skipped 0, failed 1: b_counts: "),
            "{body}: {report}"
        );
        assert!(report.contains(expected), "{body}: {report}");
        assert_eq!(view_state(&c, MIDDLE.0), before, "{body}");
        assert_matches_oracle(&c, FIRST.0, FIRST.1);
        assert_matches_oracle(&c, LAST.0, LAST.1);
        assert!(c.is_autocommit(), "{body}");
    }
}
