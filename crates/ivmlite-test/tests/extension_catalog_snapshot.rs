//! M1b Phase 6 spec §4: a connection reads `main.sqlite_schema` once per
//! schema version and shares the snapshot, and each base table's capture
//! verdict, among its views; every broken-view message stays as it was.

mod common;
use common::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ivmlite_test::open_with_extension;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::Connection;

/// The views `one_scan_serves_every_view_after_ddl` creates on `orders`.
const VIEWS: usize = 20;

fn filtered(k: usize) -> String {
    format!("SELECT region, SUM(amount), COUNT(*) FROM orders WHERE amount > {k} GROUP BY region")
}

/// Count, from now on, the authorizer's `Read` callbacks on `main`'s
/// `sqlite_schema` (SQLite may name it either way). The authorizer is
/// consulted whenever a statement is prepared, once per column it reads.
fn count_schema_reads(c: &Connection) -> Arc<AtomicUsize> {
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = reads.clone();
    c.authorizer(Some(move |ctx: AuthContext<'_>| {
        if let AuthAction::Read {
            table_name: "sqlite_schema" | "sqlite_master",
            ..
        } = ctx.action
        {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        Authorization::Allow
    }))
    .unwrap();
    reads
}

fn assert_broken(err: rusqlite::Error, expected: &str, what: &str) {
    let message = err.to_string();
    assert!(
        message.contains("cannot be maintained") && message.contains(expected),
        "{what}: {message}"
    );
}

/// Spec §4: after a schema change, the next refresh of every view on the
/// connection rebuilds one snapshot with one scan of `sqlite_schema`, and
/// `orders`' capture verdict is computed once for all twenty views. Counted
/// in authorizer `Read` callbacks: the post-DDL refresh of every view makes
/// at most twice what one snapshot scan makes (the scan, plus the one read
/// of `orders`' own `CREATE` statement behind its verdict), where every view
/// checking on its own made about ten lookups each.
#[test]
fn one_scan_serves_every_view_after_ddl() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    for v in 0..VIEWS {
        create(&c, &format!("v{v}"), &filtered(v % 4)).unwrap();
    }
    for v in 0..VIEWS {
        refresh(&c, &format!("v{v}")).unwrap();
    }
    c.execute_batch("CREATE TABLE unrelated(x)").unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 3), ('c', 9)")
        .unwrap();
    let reads = count_schema_reads(&c);
    // One snapshot scan's worth, measured on its own text (spec §4: the
    // type, name and tbl_name of every row).
    c.prepare("SELECT type, name, tbl_name FROM \"main\".sqlite_schema")
        .unwrap();
    let one_scan = reads.swap(0, Ordering::Relaxed);
    assert!(one_scan > 0, "the authorizer saw the scan");
    for v in 0..VIEWS {
        refresh(&c, &format!("v{v}")).unwrap();
    }
    let after_ddl = reads.load(Ordering::Relaxed);
    assert!(
        after_ddl <= 2 * one_scan,
        "the refresh of {VIEWS} views after DDL made {after_ddl} sqlite_schema reads; \
         one scan makes {one_scan}"
    );
    c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    for v in 0..VIEWS {
        assert_matches_oracle(&c, &format!("v{v}"), &filtered(v % 4));
    }
}

/// Spec §4: a broken capture is part of the table's verdict, which every
/// view of the table shares: both views' next refreshes fail, with today's
/// message.
#[test]
fn a_broken_capture_breaks_every_view_of_the_table() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "big", &filtered(1)).unwrap();
    refresh(&c, "sums").unwrap();
    refresh(&c, "big").unwrap();
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_del")
        .unwrap();
    for view in ["sums", "big"] {
        let err = refresh(&c, view).expect_err(view);
        assert_broken(
            err,
            "its capture trigger __ivm_trig_orders_del is missing",
            view,
        );
    }
}

/// Spec §4: verdicts are kept per table. A join reads `orders` and
/// `regions`; with either table's capture trigger dropped, the join's next
/// refresh names that table's trigger, whichever table the view lists
/// first.
#[test]
fn a_broken_capture_on_either_table_of_a_join_breaks_it() {
    for table in ["orders", "regions"] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        create(&c, "j", JOIN).unwrap();
        refresh(&c, "j").unwrap();
        c.execute_batch(&format!("DROP TRIGGER __ivm_trig_{table}_upd"))
            .unwrap();
        let err = refresh(&c, "j").expect_err(table);
        assert_broken(
            err,
            &format!("its capture trigger __ivm_trig_{table}_upd is missing"),
            table,
        );
    }
}

/// Spec §4: a snapshot and its verdicts hold for one schema version only.
/// An unrelated table first makes a refresh build the snapshot and
/// `orders`' verdict. Then `orders`, dropped and recreated with the same
/// columns on the same connection, has the same shape but none of its
/// capture triggers, and the next refresh says so.
#[test]
fn a_recreated_base_table_is_seen() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("CREATE TABLE unrelated(x)").unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch(
        "DROP TABLE orders;
         CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;",
    )
    .unwrap();
    let err = refresh(&c, "sums").expect_err("the capture triggers went with the table");
    assert_broken(
        err,
        "its capture trigger __ivm_trig_orders_ins is missing",
        "recreated",
    );
}

/// Spec §4: each connection keeps its own snapshot. Two extension
/// connections to one database each refresh views on `orders`, so both
/// snapshots are built. A harmless `CREATE INDEX` on B leaves A's views
/// correct; a capture trigger dropped on A breaks B's next refresh.
#[test]
fn each_connection_keeps_its_own_snapshot() {
    let file = TempFile::new("snapshot-two-connections");
    let a = open_with_extension(Some(file.path())).unwrap();
    setup(&a);
    create(&a, "sums", SUMS).unwrap();
    create(&a, "big", &filtered(1)).unwrap();
    let b = open_with_extension(Some(file.path())).unwrap();
    for c in [&a, &b] {
        refresh(c, "sums").unwrap();
        refresh(c, "big").unwrap();
    }
    b.execute_batch("CREATE INDEX orders_region ON orders(region)")
        .unwrap();
    a.execute_batch("INSERT INTO orders VALUES ('b', 7), ('d', 2)")
        .unwrap();
    for view in ["sums", "big"] {
        refresh(&a, view).unwrap();
    }
    assert_matches_oracle(&a, "sums", SUMS);
    assert_matches_oracle(&a, "big", &filtered(1));
    a.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
        .unwrap();
    for view in ["sums", "big"] {
        let err = refresh(&b, view).expect_err(view);
        assert_broken(
            err,
            "its capture trigger __ivm_trig_orders_ins is missing",
            view,
        );
    }
}

/// Spec §4: closing a connection frees its snapshot, and a connection opened
/// afterwards, whose handle may reuse the address, starts from a new one.
/// The freeing itself is not observable here; that the next connection's
/// views are correct is.
#[test]
fn the_snapshot_is_freed_with_its_connection() {
    let file = TempFile::new("snapshot-freed");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        refresh(&c, "sums").unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('e', 1)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
        .unwrap();
    let err = refresh(&c, "sums").expect_err("the new snapshot sees the drop");
    assert_broken(
        err,
        "its capture trigger __ivm_trig_orders_ins is missing",
        "reopened",
    );
}

/// Spec §4: rolling back a schema change can bring `schema_version` back to
/// a value the snapshot was read at, with other objects. SQLite then resets
/// the connection's schema and every view connects again, and the connect
/// reads the snapshot afresh. Here the application keeps its refresh
/// statement prepared, as statement caches do, so the view's old virtual
/// table, and with it the old snapshot, outlives the reset: the cookie goes
/// V → V+1 (`scratch`), a refresh passes at V+1, the rollback returns it to
/// V, and dropping a capture trigger moves it to V+1 again. The next refresh
/// must see the missing trigger.
#[test]
fn a_rolled_back_schema_is_not_reused_by_a_held_statement() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    let mut command = c
        .prepare("INSERT INTO sums(sums) VALUES ('refresh')")
        .unwrap();
    command.execute([]).unwrap();
    let before = count(&c, "PRAGMA schema_version");
    c.execute_batch("BEGIN; CREATE TABLE scratch(x INTEGER) STRICT;")
        .unwrap();
    command.execute([]).unwrap(); // passes at V+1
    c.execute_batch("ROLLBACK").unwrap();
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
        .unwrap();
    assert_eq!(count(&c, "PRAGMA schema_version"), before + 1);
    let err = command.execute([]).expect_err("the trigger is missing");
    assert_broken(
        err,
        "its capture trigger __ivm_trig_orders_ins is missing",
        "held statement",
    );
}

/// Spec §4: a verdict says whether a capture is broken; failing to read the
/// catalog says nothing about it, so it is never kept. After an unrelated
/// schema change, an authorizer refuses the `pragma_table_xinfo` read behind
/// `orders`' verdict and the refresh fails. Once the authorizer is gone, the
/// next refresh, at the same schema version, reads the catalog again and
/// succeeds.
#[test]
fn a_failure_to_read_the_catalog_is_not_kept() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("CREATE TABLE unrelated(x)").unwrap();
    let version = count(&c, "PRAGMA schema_version");
    c.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Read {
            table_name: "pragma_table_xinfo",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }))
    .unwrap();
    let err = refresh(&c, "sums").expect_err("the catalog read is refused");
    assert!(err.to_string().contains("pragma_table_xinfo"), "{err}");
    c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert_eq!(count(&c, "PRAGMA schema_version"), version);
    c.execute_batch("INSERT INTO orders VALUES ('b', 1)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}
