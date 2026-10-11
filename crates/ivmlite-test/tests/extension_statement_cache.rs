//! M1b Phase 6 spec §3: each view keeps its statements prepared across
//! refreshes, in one `Connection` that lives as long as the view's virtual
//! table, and the cache never makes a refresh, a drop or a close wrong.

mod common;
use common::*;

use std::ffi::{c_int, CString};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ivmlite_core::{Row, Value as CoreValue};
use ivmlite_test::{extension_library, open_with_extension};
use rusqlite::hooks::{AuthContext, Authorization};
use rusqlite::{ffi, Connection};

/// A second view over `orders`, sharing its capture with `SUMS`.
const COUNTS: &str = "SELECT region, COUNT(*) FROM orders GROUP BY region";

/// Insert `n` rows into `orders`, each in a region no earlier call used, so
/// the next refresh of `SUMS` stages `n` state rows and `n` output rows.
fn insert_rows(c: &Connection, n: usize) {
    let base = count(c, "SELECT count(*) FROM orders");
    c.execute(
        "WITH RECURSIVE i(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM i WHERE n < ?1)
         INSERT INTO orders SELECT printf('g%06d', ?2 + n), n FROM i WHERE ?1 > 0",
        rusqlite::params![n as i64, base],
    )
    .unwrap();
}

fn limit_variables(c: &Connection, n: c_int) {
    // SAFETY: `c`'s own live handle; `sqlite3_limit` only sets one of its
    // run-time limits.
    unsafe {
        ffi::sqlite3_limit(c.handle(), ffi::SQLITE_LIMIT_VARIABLE_NUMBER, n);
    }
}

/// Spec §3: after the cache is warm, a refresh prepares no statement. The
/// authorizer is consulted whenever SQLite prepares a statement, the
/// extension's own included (it runs on the same handle), so counting its
/// calls counts prepares. Installing it expires every statement, so the
/// warm-up refreshes re-prepare them: the first stages 127 rows of each
/// kind, a full chunk of 64 and then every remainder shape (32, 16, 8, 4,
/// 2, 1). The refresh command itself is prepared once, before the counted
/// window, which then covers only the extension's work.
#[test]
fn a_steady_refresh_prepares_no_statement() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    c.authorizer(Some(move |_: AuthContext<'_>| {
        counter.fetch_add(1, Ordering::Relaxed);
        Authorization::Allow
    }))
    .unwrap();
    let mut command = c
        .prepare("INSERT INTO sums(sums) VALUES ('refresh')")
        .unwrap();
    for rows in [127usize, 5] {
        insert_rows(&c, rows);
        command.execute([]).unwrap();
    }
    for rows in [1usize, 37, 130] {
        insert_rows(&c, rows);
        let at = calls.load(Ordering::Relaxed);
        command.execute([]).unwrap();
        assert_eq!(
            calls.load(Ordering::Relaxed),
            at,
            "refresh after {rows} new rows prepared a statement"
        );
    }
    drop(command);
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Decode one state-table blob (Phase 3a spec §4): per value a tag byte,
/// then `0x00` NULL; `0x01` INTEGER, 8 bytes big-endian; `0x02` TEXT, a
/// 4-byte big-endian length, then the UTF-8 bytes.
fn decode(bytes: &[u8]) -> Row {
    let mut values = Vec::new();
    let mut rest = bytes;
    while let Some((&tag, tail)) = rest.split_first() {
        rest = tail;
        match tag {
            0x00 => values.push(CoreValue::Null),
            0x01 => {
                let (n, tail) = rest.split_first_chunk::<8>().expect("an INTEGER");
                values.push(CoreValue::Int(i64::from_be_bytes(*n)));
                rest = tail;
            }
            0x02 => {
                let (len, tail) = rest.split_first_chunk::<4>().expect("a TEXT length");
                let (text, tail) = tail.split_at(u32::from_be_bytes(*len) as usize);
                values.push(CoreValue::Text(String::from_utf8(text.to_vec()).unwrap()));
                rest = tail;
            }
            other => panic!("unknown tag {other}"),
        }
    }
    Row(values)
}

/// The stage's `state` rows in rowid order, as `(arr, key, val)`.
fn staged_state(c: &Connection) -> Vec<(i64, Row, Row)> {
    let mut stmt = c
        .prepare("SELECT arr, key, val FROM __ivm_stage_sums WHERE op = 'state' ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            decode(&r.get::<_, Vec<u8>>(1)?),
            decode(&r.get::<_, Vec<u8>>(2)?),
        ))
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}

/// Spec §3: the remainder after the last full chunk is staged in chunks of
/// its binary digits, front first, so the rows keep their push order. A
/// view's rows are pushed per arrangement in key order, then value order
/// (`apply` walks each pending `BTreeMap`), so in push order the stage's
/// `state` rows, read by rowid, are sorted by `(arr, key, val)`. The stage
/// keeps its rows until the next apply, so it can be read after the refresh.
/// 63 rows are six remainder chunks, so chunks staged in any other order
/// break the sort.
#[test]
fn remainders_are_staged_in_push_order() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    for rows in [0usize, 1, 63, 64, 65, 129] {
        insert_rows(&c, rows);
        refresh(&c, "sums").unwrap();
        let staged = staged_state(&c);
        assert_eq!(
            staged.len(),
            rows,
            "state rows staged for {rows} new groups"
        );
        let mut sorted = staged.clone();
        sorted.sort();
        assert!(staged == sorted, "{rows} rows: not staged in push order");
        assert_matches_oracle(&c, "sums", SUMS);
    }
}

/// Spec §3: a cached statement that SQLite finds stale re-prepares against
/// the current schema, so a refresh after any schema change is correct:
/// a non-unique index on a base table (which the capture accepts), an
/// unrelated table, and a table another connection creates.
#[test]
fn a_refresh_after_ddl_is_correct() {
    let file = TempFile::new("stmtcache-ddl");
    let c = open_with_extension(Some(file.path())).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    insert_rows(&c, 3);
    refresh(&c, "sums").unwrap(); // the cache is warm
    let other = Connection::open(file.path()).unwrap();
    let changes: [(&str, &Connection); 3] = [
        ("CREATE INDEX orders_amount ON orders(amount)", &c),
        ("CREATE TABLE unrelated(x)", &c),
        ("CREATE TABLE from_another_connection(x)", &other),
    ];
    for (ddl, on) in changes {
        on.execute_batch(ddl).unwrap();
        insert_rows(&c, 5);
        c.execute_batch("UPDATE orders SET amount = amount + 1 WHERE region = 'a'")
            .unwrap();
        refresh(&c, "sums").unwrap_or_else(|e| panic!("{ddl}: {e}"));
        assert_matches_oracle(&c, "sums", SUMS);
    }
}

/// Spec §3: a cached statement is reset before its callback returns, so it
/// never blocks a `DROP TABLE`. One view is dropped while another on the
/// same base table holds cached statements; the other still refreshes, and
/// dropping it, the last reader, removes the delta table.
#[test]
fn a_view_drops_while_others_hold_cached_statements() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "counts", COUNTS).unwrap();
    insert_rows(&c, 3);
    refresh(&c, "sums").unwrap();
    refresh(&c, "counts").unwrap();
    c.execute_batch("DROP TABLE sums").unwrap();
    insert_rows(&c, 4);
    refresh(&c, "counts").unwrap();
    assert_matches_oracle(&c, "counts", COUNTS);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM sqlite_schema WHERE name = '__ivm_delta_orders'"
        ),
        1
    );
    c.execute_batch("DROP TABLE counts").unwrap();
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM sqlite_schema WHERE name = '__ivm_delta_orders'"
        ),
        0
    );
}

/// A raw handle on `path` with the extension loaded the way
/// `ivmlite_test::open_with_extension_at` loads it, so the test can close it
/// with `sqlite3_close` itself. rusqlite's own close also uses
/// `sqlite3_close`, but its `Drop` only logs a failure.
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
        // the `ivm` module.
        unsafe {
            c.load_extension_enable().unwrap();
            c.load_extension(extension_library(), Some("sqlite3_ivmlite_init"))
                .unwrap();
            c.load_extension_disable().unwrap();
        }
    });
    db
}

/// Run `body` on a non-owning `Connection` over `db`. Dropping it finalizes
/// the statements its own cache holds and leaves the handle open.
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

/// One case of `close_v1_succeeds_after_refreshes`: its name, and what it
/// runs on the raw handle before the close.
type CloseCase<'a> = (&'a str, &'a dyn Fn(&Connection));

/// Spec §3: SQLite calls xDisconnect before it checks for unfinalized
/// statements at close, and dropping a view's `Connection` there finalizes
/// its cached statements, so `sqlite3_close` succeeds: after refreshes,
/// after creates with no refresh, after a create that failed, and after a
/// view reopened by xConnect was refreshed.
#[test]
fn close_v1_succeeds_after_refreshes() {
    let file = TempFile::new("stmtcache-close");
    let cases: [CloseCase; 4] = [
        ("created and refreshed", &|c| {
            setup(c);
            create(c, "sums", SUMS).unwrap();
            create(c, "counts", COUNTS).unwrap();
            for _ in 0..3 {
                insert_rows(c, 5);
                refresh(c, "sums").unwrap();
                refresh(c, "counts").unwrap();
            }
        }),
        ("reopened and refreshed", &|c| {
            insert_rows(c, 5);
            refresh(c, "sums").unwrap();
            refresh(c, "counts").unwrap();
            assert_matches_oracle(c, "sums", SUMS);
        }),
        ("created, never refreshed", &|c| {
            create(c, "joined", JOIN).unwrap();
        }),
        ("a create that failed", &|c| {
            create(c, "bad", "SELECT nonsense FROM nowhere").expect_err("bad SQL");
        }),
    ];
    for (case, body) in cases {
        let db = open_raw(file.path());
        with_raw(db, body);
        assert_eq!(close_v1(db), ffi::SQLITE_OK, "{case}");
    }
}

/// Spec §3: a failed refresh returns its statements to the cache reset, so
/// the next refresh, once the fault is gone, succeeds, and the view drops.
#[test]
fn a_failed_refresh_leaves_the_cache_usable() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    for rows in [3usize, 5] {
        insert_rows(&c, rows);
        refresh(&c, "sums").unwrap();
    }
    c.execute_batch(
        "CREATE TRIGGER fault BEFORE INSERT ON __ivm_out_sums \
         BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
    )
    .unwrap();
    insert_rows(&c, 7);
    let err = refresh(&c, "sums").expect_err("the fault aborts the apply");
    assert!(err.to_string().contains("injected fault"), "{err}");
    c.execute_batch("DROP TRIGGER fault").unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
    c.execute_batch("DROP TABLE sums").unwrap();
}

/// Spec §3 with Phase 5 spec §6: the chunk is derived from the live variable
/// limit on every refresh, so lowering it after the 64-row shapes were
/// cached (each binds 256 parameters) still stages every row within it.
#[test]
fn a_lowered_variable_limit_after_caching_still_refreshes() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    for rows in [130usize, 70] {
        insert_rows(&c, rows);
        refresh(&c, "sums").unwrap();
    }
    limit_variables(&c, 100);
    insert_rows(&c, 130);
    refresh(&c, "sums").unwrap();
    for op in ["state", "out+"] {
        assert_eq!(
            count(
                &c,
                &format!("SELECT count(*) FROM __ivm_stage_sums WHERE op = '{op}'")
            ),
            130,
            "{op} rows staged"
        );
    }
    assert_matches_oracle(&c, "sums", SUMS);
}
