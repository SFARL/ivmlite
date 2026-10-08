//! M1b Phase 5 spec §3: a refresh skips the catalog checks while
//! `PRAGMA schema_version` is unchanged since they last passed, and any
//! schema change, from any connection, makes the next refresh run them again.

mod common;
use common::*;

use ivmlite_test::open_with_extension;
use rusqlite::Connection;

/// `setup`, plus a unique index on `orders(amount)`, so an
/// `ALTER COLUMN amount SET NOT NULL` changes the shape the capture was
/// generated from.
fn setup_with_unique_amount(c: &Connection) {
    setup(c);
    c.execute_batch("CREATE UNIQUE INDEX orders_amount ON orders(amount)")
        .unwrap();
}

/// Phase 5 spec §3: schema changes made after the last check still break the
/// next refresh, with today's messages, whichever connection makes them, and
/// with or without the extension loaded on it.
#[test]
fn a_schema_change_from_any_connection_breaks_the_next_refresh() {
    let changes = [
        (
            "DROP TRIGGER __ivm_trig_orders_ins",
            "its capture trigger __ivm_trig_orders_ins is missing",
        ),
        (
            "CREATE UNIQUE INDEX uq ON orders(amount)",
            "base table orders changed shape",
        ),
        (
            "DROP INDEX __ivm_outidx_sums",
            "its shadow index __ivm_outidx_sums is missing",
        ),
        (
            "DROP TABLE orders;
             CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
             CREATE UNIQUE INDEX orders_amount ON orders(amount);",
            "its capture trigger __ivm_trig_orders_ins is missing",
        ),
        (
            "ALTER TABLE orders ALTER COLUMN amount SET NOT NULL",
            "base table orders changed shape",
        ),
    ];
    for who in ["same", "other", "plain"] {
        for (ddl, expected) in changes {
            let file = TempFile::new(&format!("cache-{who}"));
            let c = open_with_extension(Some(file.path())).unwrap();
            setup_with_unique_amount(&c);
            create(&c, "sums", SUMS).unwrap();
            refresh(&c, "sums").unwrap(); // the cache is warm
            match who {
                "same" => c.execute_batch(ddl).unwrap(),
                "other" => open_with_extension(Some(file.path()))
                    .unwrap()
                    .execute_batch(ddl)
                    .unwrap(),
                _ => Connection::open(file.path())
                    .unwrap()
                    .execute_batch(ddl)
                    .unwrap(),
            }
            let err = refresh(&c, "sums").expect_err(ddl);
            assert!(
                err.to_string().contains("cannot be maintained")
                    && err.to_string().contains(expected),
                "{who} / {ddl}: {err}"
            );
        }
    }
}

/// The latch is data, not schema: a latched table breaks the next refresh
/// even though the schema cookie did not move. White-box: the test writes
/// `__ivm_tracked` directly, the only way to set the latch without also
/// changing the schema.
#[test]
fn a_latch_set_without_a_schema_change_breaks_the_next_refresh() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("UPDATE __ivm_tracked SET broken = 'test latch' WHERE tbl = 'orders'")
        .unwrap();
    let err = refresh(&c, "sums").expect_err("latched");
    assert!(err.to_string().contains("test latch"), "{err}");
}

/// As above, for every base table of a join: on a cache hit each table's
/// latch is read, not only the first table's. Each of `orders` and `regions`
/// is latched in turn, so whichever one the view lists second is covered.
#[test]
fn a_latch_on_any_table_of_a_join_breaks_the_next_refresh() {
    for table in ["orders", "regions"] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        create(&c, "j", JOIN).unwrap();
        refresh(&c, "j").unwrap(); // the cache is warm
        c.execute_batch(&format!(
            "UPDATE __ivm_tracked SET broken = 'test latch on {table}' WHERE tbl = '{table}'"
        ))
        .unwrap();
        let err = refresh(&c, "j").expect_err(table);
        assert!(
            err.to_string().contains(&format!("test latch on {table}")),
            "{table}: {err}"
        );
    }
}

/// Phase 5 spec §3: with the schema cookie unchanged, a refresh skips the
/// catalog checks. Observed deterministically through a data-only tampering
/// that only those checks would notice: deleting the view's `__ivm_dep` row
/// (white-box). It goes unnoticed while the schema is unchanged, and the
/// first refresh after any schema change, even a harmless one, reports it.
#[test]
fn an_unchanged_schema_skips_the_catalog_checks() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("DELETE FROM __ivm_dep WHERE view = 'sums'")
        .unwrap();
    refresh(&c, "sums").expect("the cache hit skips the dependency check");
    c.execute_batch("CREATE TABLE scratch(x INTEGER) STRICT")
        .unwrap();
    let err = refresh(&c, "sums").expect_err("a schema change re-runs the checks");
    assert!(
        err.to_string()
            .contains("dependency on table orders is not recorded"),
        "{err}"
    );
}

/// Phase 5 spec §3: the cache never outlives a failed check. A schema change
/// breaks a refresh; the user fixes nothing, makes no further schema change,
/// and every later refresh still fails.
#[test]
fn a_failed_check_keeps_failing_until_the_schema_is_fixed() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
        .unwrap();
    for attempt in 0..3 {
        let err = refresh(&c, "sums").expect_err("the trigger is still missing");
        assert!(
            err.to_string().contains("__ivm_trig_orders_ins is missing"),
            "attempt {attempt}: {err}"
        );
    }
}

/// Phase 5 spec §3: a check that passed inside a transaction that is then
/// rolled back says nothing about the schema afterwards, even when a later
/// change brings `PRAGMA schema_version` back to the value it passed at.
/// Here the cookie moves V → V+1 (`scratch`), a refresh passes at V+1, the
/// rollback returns it to V, and dropping a capture trigger moves it to V+1
/// again: the next refresh must still see the missing trigger. Both a
/// whole-transaction `ROLLBACK` and a `ROLLBACK TO` a savepoint.
#[test]
fn a_check_from_a_rolled_back_transaction_is_not_reused() {
    for (begin, undo) in [
        ("BEGIN", "ROLLBACK"),
        ("SAVEPOINT s", "ROLLBACK TO s; RELEASE s"),
    ] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        refresh(&c, "sums").unwrap();
        let before = count(&c, "PRAGMA schema_version");
        c.execute_batch(&format!("{begin}; CREATE TABLE scratch(x INTEGER) STRICT;"))
            .unwrap();
        refresh(&c, "sums").unwrap(); // passes at V+1
        c.execute_batch(undo).unwrap();
        assert_eq!(count(&c, "PRAGMA schema_version"), before, "{undo}");
        c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
            .unwrap();
        assert_eq!(count(&c, "PRAGMA schema_version"), before + 1, "{undo}");
        let err = refresh(&c, "sums").expect_err(undo);
        assert!(
            err.to_string().contains("__ivm_trig_orders_ins is missing"),
            "{undo}: {err}"
        );
    }
}

/// Phase 5 spec §3: the checks `create` and `connect` run fill the cache, so
/// even the first refresh after them skips the catalog checks. Observed, as
/// above, through deleting the view's `__ivm_dep` row (white-box), which only
/// those checks would notice.
#[test]
fn the_checks_at_create_and_at_connect_warm_the_cache() {
    let file = TempFile::new("cache-warm");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        c.execute_batch("DELETE FROM __ivm_dep WHERE view = 'sums'")
            .unwrap();
        refresh(&c, "sums").expect("create filled the cache");
        c.execute_batch("INSERT INTO __ivm_dep(view, tbl) VALUES ('sums', 'orders')")
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM sums"), 3); // connects
    c.execute_batch("DELETE FROM __ivm_dep WHERE view = 'sums'")
        .unwrap();
    refresh(&c, "sums").expect("connect filled the cache");
}
