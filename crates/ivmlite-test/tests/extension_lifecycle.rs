//! M1b Phase 3a spec §6, scenarios 2–6, against the real loaded extension:
//! capture from any connection, failure and retry, atomic create, rejections,
//! broken views, and drop.

mod common;
use common::*;

use ivmlite_test::open_with_extension;
use rusqlite::types::Value;
use rusqlite::Connection;

#[test]
fn writes_from_any_connection_are_captured_and_survive_a_reopen() {
    let file = TempFile::new("capture");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        assert_matches_oracle(&c, "sums", SUMS);
    }
    {
        // A connection that never loaded the extension: its INSERT, DELETE
        // and UPDATE are captured by the triggers (spec §8.1).
        let plain = Connection::open(file.path()).unwrap();
        plain
            .execute_batch(
                "INSERT INTO orders VALUES ('a', 10), ('c', 7);
                 DELETE FROM orders WHERE region = 'b';
                 UPDATE orders SET amount = 100 WHERE amount = 1;
                 UPDATE orders SET region = 'c' WHERE region IS NULL;",
            )
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_join_view_is_maintained_across_both_tables() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "managers", JOIN).unwrap();
    assert_matches_oracle(&c, "managers", JOIN);
    c.execute_batch(
        "INSERT INTO orders VALUES ('c', 3); UPDATE regions SET manager = 'cy' WHERE name = 'c';
         DELETE FROM regions WHERE name = 'a'; INSERT INTO regions VALUES ('a', 'ann');",
    )
    .unwrap();
    refresh(&c, "managers").unwrap();
    assert_matches_oracle(&c, "managers", JOIN);
}

/// Spec §5 and §6 scenario 3: a refresh that fails part way changes nothing
/// durable — in autocommit mode and inside an explicit transaction, whose
/// earlier statements must survive — and a retry then succeeds.
#[test]
fn a_failed_refresh_changes_nothing_and_a_retry_succeeds() {
    for explicit_transaction in [false, true] {
        for fault_on in ["__ivm_out_managers", "__ivm_state_managers_2_join_left"] {
            let c = open_with_extension(None).unwrap();
            setup(&c);
            create(&c, "managers", JOIN).unwrap();
            c.execute_batch(&format!(
                "CREATE TRIGGER fault BEFORE INSERT ON \"{fault_on}\" \
                 BEGIN SELECT RAISE(ABORT, 'injected fault'); END;"
            ))
            .unwrap();
            if explicit_transaction {
                c.execute_batch("BEGIN").unwrap();
            }
            c.execute_batch("INSERT INTO orders VALUES ('c', 3), ('a', 9)")
                .unwrap();
            let before = durable_state(&c, "managers");
            let err = refresh(&c, "managers").expect_err("the fault must fail the refresh");
            assert!(err.to_string().contains("injected fault"), "{err}");
            assert_eq!(
                durable_state(&c, "managers"),
                before,
                "a failed refresh changed durable state (transaction: {explicit_transaction}, fault on {fault_on})"
            );
            if explicit_transaction {
                assert!(!c.is_autocommit(), "the transaction must still be open");
            }
            c.execute_batch("DROP TRIGGER fault").unwrap();
            refresh(&c, "managers").unwrap();
            if explicit_transaction {
                c.execute_batch("COMMIT").unwrap();
            }
            assert_matches_oracle(&c, "managers", JOIN);
            assert_eq!(
                rows(&c, "SELECT count(*) FROM orders WHERE region = 'c'"),
                vec![vec![Value::Integer(1)]]
            );
        }
    }
}

/// Spec §6 scenario 4.
#[test]
fn creating_a_view_is_atomic() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let user_objects = objects(&c);

    c.execute_batch("BEGIN").unwrap();
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        objects(&c),
        user_objects,
        "a rolled-back create left objects behind"
    );

    // A create that fails after creating shadow tables (the output column
    // name `__w` is reserved and is checked after them), inside a
    // transaction: nothing is left, and the transaction's earlier work is.
    c.execute_batch("BEGIN; INSERT INTO orders VALUES ('z', 1);")
        .unwrap();
    let err = create(
        &c,
        "bad",
        "SELECT region, COUNT(*) AS __w FROM orders GROUP BY region",
    )
    .expect_err("__w is reserved");
    // ivmlite's own message, not SQLite's "duplicate column name: __w",
    // which the output table's CREATE would also raise without the check.
    assert!(
        err.to_string()
            .contains("a result column is named __w, which ivmlite reserves"),
        "{err}"
    );
    assert_eq!(objects(&c), user_objects);
    c.execute_batch("COMMIT").unwrap();
    assert_eq!(
        rows(&c, "SELECT count(*) FROM orders WHERE region = 'z'"),
        vec![vec![Value::Integer(1)]]
    );

    // Bootstrap over tables that already hold rows.
    create(&c, "sums", SUMS).unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Spec §6 scenario 5: each rejection names its problem.
#[test]
fn what_v0_cannot_maintain_is_rejected_by_name() {
    let q = "SELECT k, COUNT(*) FROM t GROUP BY k";
    let cases = [
        ("CREATE TABLE t(k TEXT, v INTEGER)", q, "not STRICT"),
        ("CREATE TABLE t(k ANY, v INTEGER) STRICT", q, "type ANY"),
        ("CREATE TABLE t(k TEXT COLLATE NOCASE, v INTEGER) STRICT", q, "COLLATE"),
        ("CREATE TABLE t(k TEXT, v REAL) STRICT", q, "type REAL"),
        ("PRAGMA encoding = 'UTF-16le'; CREATE TABLE t(k TEXT) STRICT", q, "UTF-8"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k, COUNT(*) FROM nope GROUP BY k", "no such table: nope"),
        ("CREATE TABLE t(k TEXT) STRICT; CREATE VIEW vv AS SELECT * FROM t", "SELECT k, COUNT(*) FROM vv GROUP BY k", "view"),
        ("CREATE TABLE __ivm_x(k TEXT) STRICT", "SELECT k, COUNT(*) FROM __ivm_x GROUP BY k", "ivmlite's own"),
        // A virtual table (here another ivmlite view): pragma_table_list
        // reports its type as `virtual`.
        ("CREATE TABLE t(k TEXT) STRICT; CREATE VIRTUAL TABLE a USING ivm('SELECT k, COUNT(*) FROM t GROUP BY k')", "SELECT k, COUNT(*) FROM a GROUP BY k", "is a virtual"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k, COUNT(*) AS w FROM t GROUP BY k", "like the view"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k FROM t", "GROUP BY"),
        // A result column named like SQLite's rowid aliases is rejected by
        // name, whichever alias and whatever its case (Task 3 controller
        // ruling: exercised end to end, not only through `declaration`).
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k AS rowid, COUNT(*) FROM t GROUP BY k", "rowid"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k AS OID, COUNT(*) FROM t GROUP BY k", "OID"),
        // A base column whose name starts with `__ivm_` is refused: the
        // prefix is reserved for ivmlite's own shadow columns.
        ("CREATE TABLE t(k TEXT, __ivm_x INTEGER) STRICT", "SELECT k, COUNT(*) FROM t GROUP BY k", "__ivm_"),
        // Phase 3b spec §6.1: what REPLACE capture cannot look up.
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX uk ON t(k COLLATE NOCASE)", q, "collation NOCASE"),
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX up ON t(v) WHERE v > 0", q, "partial"),
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX ue ON t(v + 1)", q, "expression"),
        ("CREATE TABLE t(k TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP UNIQUE, v INTEGER) STRICT", q, "default CURRENT_TIMESTAMP"),
        ("CREATE TABLE t(k TEXT, RowId INTEGER) STRICT", q, "RowId"),
        // A generated column as a unique key, through an explicit index or
        // a UNIQUE constraint's autoindex (final review, minor 3: this used
        // to fail with "reading the catalog: Query returned no rows").
        (
            "CREATE TABLE t(k TEXT, v INTEGER, g INTEGER GENERATED ALWAYS AS (v * 2) VIRTUAL) STRICT; \
             CREATE UNIQUE INDEX ug ON t(g)",
            q,
            "unique index ug has the generated column g as a key",
        ),
        (
            "CREATE TABLE t(k TEXT, v INTEGER, g INTEGER GENERATED ALWAYS AS (v * 2) STORED UNIQUE) STRICT",
            q,
            "has the generated column g as a key",
        ),
    ];
    for (setup_sql, sql, expected) in cases {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(setup_sql).unwrap();
        let err = create(&c, "w", sql).expect_err(setup_sql);
        assert!(
            err.to_string().contains(expected),
            "{setup_sql} / {sql}: {err}"
        );
    }
    // A view named with ivmlite's own prefix, in any case, would collide
    // with (or be mistaken for) a shadow table (final review, Minor 7).
    for view in ["__ivm_meta", "__IVM_out_x", "__Ivm_x"] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch("CREATE TABLE t(k TEXT) STRICT").unwrap();
        let err = create(&c, view, q).expect_err(view);
        assert!(
            err.to_string()
                .contains(&format!("view name {view} starts with __ivm_")),
            "{view}: {err}"
        );
    }
}

#[test]
fn a_view_accepts_only_the_refresh_command() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    for (sql, expected) in [
        (
            "INSERT INTO sums(sums) VALUES ('rebuild')",
            "unknown ivmlite command",
        ),
        ("INSERT INTO sums(region) VALUES ('x')", "read-only"),
        ("UPDATE sums SET region = 'x'", "read-only"),
        ("DELETE FROM sums", "read-only"),
    ] {
        let err = c.execute_batch(sql).expect_err(sql);
        assert!(err.to_string().contains(expected), "{sql}: {err}");
    }
    // Extra values: a refresh command carrying a non-NULL value in an output
    // column is an error, not silently dropped.
    let err = c
        .execute_batch("INSERT INTO sums(sums, region) VALUES ('refresh', 1)")
        .expect_err("a refresh with an extra column value must be rejected");
    assert!(err.to_string().contains("no other column value"), "{err}");
}

/// Spec §6 scenarios 5 and 6: a view whose stored plan no longer matches, or
/// whose state table is missing, reports why on every use — and can still
/// be dropped, leaving the base tables writable.
#[test]
fn a_broken_view_reports_why_and_can_still_be_dropped() {
    for (breakage, expected) in [
        ("UPDATE __ivm_view SET plan = 'tampered'", "different plan"),
        ("DROP TABLE __ivm_state_sums_0_agg_groups", "is missing"),
        // Phase 3b spec §6.3: without its pend table, every write to
        // `orders` fails, and REPLACE capture has nowhere to record.
        (
            "DROP TABLE __ivm_pend_orders",
            "its shadow table __ivm_pend_orders is missing",
        ),
    ] {
        let file = TempFile::new("broken");
        {
            let c = open_with_extension(Some(file.path())).unwrap();
            setup(&c);
            create(&c, "sums", SUMS).unwrap();
        }
        Connection::open(file.path())
            .unwrap()
            .execute_batch(breakage)
            .unwrap();
        let c = open_with_extension(Some(file.path())).unwrap();
        for sql in [
            "SELECT * FROM sums",
            "INSERT INTO sums(sums) VALUES ('refresh')",
        ] {
            let err = c.execute_batch(sql).expect_err(sql);
            assert!(
                err.to_string().contains(expected),
                "{breakage} / {sql}: {err}"
            );
        }
        c.execute_batch("DROP TABLE sums").unwrap();
        assert_eq!(
            objects(&c),
            rows(
                &c,
                "SELECT type, name FROM sqlite_schema WHERE name IN ('orders', 'regions')"
            )
        );
        c.execute_batch("INSERT INTO orders VALUES ('a', 1)")
            .unwrap();
    }
}

/// Final review, I2: two views share `orders`' capture, and the user drops
/// one of its shared shadow tables — the delta table, or `__ivm_tracked`.
/// Both views report why, and both still drop, in either order; after the
/// last drop only the user's objects remain and `orders` is writable. The
/// first drop used to fail with "SQL logic error" in `collect_garbage`
/// (no delta table), and the last one in `untrack` (no `__ivm_tracked`).
#[test]
fn two_views_sharing_a_broken_capture_can_both_still_be_dropped() {
    let counts = "SELECT region, COUNT(*) FROM orders GROUP BY region";
    for breakage in ["DROP TABLE __ivm_delta_orders", "DROP TABLE __ivm_tracked"] {
        for order in [["sums", "counts"], ["counts", "sums"]] {
            let c = open_with_extension(None).unwrap();
            setup(&c);
            let user_objects = objects(&c);
            create(&c, "sums", SUMS).unwrap();
            create(&c, "counts", counts).unwrap();
            c.execute_batch(breakage).unwrap();
            for view in order {
                let err = refresh(&c, view).expect_err(breakage);
                assert!(
                    err.to_string().contains("cannot be maintained"),
                    "{breakage} / {view}: {err}"
                );
            }
            for view in order {
                c.execute_batch(&format!("DROP TABLE {view}"))
                    .unwrap_or_else(|e| panic!("{breakage} / DROP TABLE {view}: {e}"));
            }
            assert_eq!(
                objects(&c),
                user_objects,
                "{breakage}: DROP TABLE left objects"
            );
            c.execute_batch("INSERT INTO orders VALUES ('a', 1)")
                .unwrap();
        }
    }
}

/// A view whose capture no longer works — a base table dropped and recreated
/// (its triggers go with it), recreated with another shape, altered, or the
/// view's apply trigger dropped — is a broken view (spec §5): a refresh on
/// the connection that made the change, and a read and a refresh on a fresh
/// connection, fail with the reason; and the view can still be dropped
/// (final review, Important 2). Without these checks every refresh
/// succeeded against a stale view, or silently applied nothing.
#[test]
fn a_view_whose_capture_is_broken_reports_why_and_can_still_be_dropped() {
    for (breakage, expected) in [
        (
            "DROP TABLE orders; CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;",
            "__ivm_trig_orders_ins is missing",
        ),
        (
            "DROP TABLE orders; CREATE TABLE orders(region INTEGER, amount INTEGER) STRICT;",
            "changed shape",
        ),
        // Adding a column changes the scan's column list, so the plan check
        // reports it first (a v0 limitation, Phase 3a spec §5).
        ("ALTER TABLE orders ADD COLUMN note TEXT", "different plan"),
        (
            "DROP TRIGGER __ivm_apply_sums",
            "__ivm_apply_sums is missing",
        ),
    ] {
        let file = TempFile::new("capture-broken");
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        let user_objects = objects(&c);
        create(&c, "sums", SUMS).unwrap();
        c.execute_batch(breakage).unwrap();
        // A NULL region fits either recreated shape.
        c.execute_batch("INSERT INTO orders(amount) VALUES (7)")
            .unwrap();
        let err = refresh(&c, "sums").expect_err(breakage);
        assert!(
            err.to_string().contains(expected) && err.to_string().contains("drop and recreate"),
            "{breakage} / same connection: {err}"
        );
        drop(c);

        let c = open_with_extension(Some(file.path())).unwrap();
        for sql in [
            "SELECT * FROM sums",
            "INSERT INTO sums(sums) VALUES ('refresh')",
        ] {
            let err = c.execute_batch(sql).expect_err(sql);
            assert!(
                err.to_string().contains(expected),
                "{breakage} / {sql}: {err}"
            );
        }
        c.execute_batch("DROP TABLE sums").unwrap();
        assert_eq!(
            objects(&c),
            user_objects,
            "{breakage}: DROP TABLE left objects"
        );
        c.execute_batch("INSERT INTO orders(amount) VALUES (1)")
            .unwrap();
    }
}

/// `ALTER TABLE t RENAME` carries `t`'s capture triggers to the new name
/// but leaves the trigger names alone, so a new `t` with the same shape
/// passes a check by name. The check must also verify which table each
/// trigger is on. Otherwise the view keeps following the old table, with
/// no error. Nothing writes the renamed table here: that would run its
/// capture triggers, whose latch reports the move first (Phase 3b spec
/// §6.2; `extension_replace`'s decoy tests), and this test isolates the
/// check of where the triggers are.
#[test]
fn a_capture_trigger_left_on_a_renamed_table_breaks_the_view() {
    let file = TempFile::new("renamed-base");
    let c = open_with_extension(Some(file.path())).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch(
        "ALTER TABLE orders RENAME TO old_orders;
         CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
         INSERT INTO orders VALUES ('b', 10);",
    )
    .unwrap();
    let expected = "__ivm_trig_orders_ins is on table old_orders";
    let err = refresh(&c, "sums").expect_err("the view no longer captures orders");
    assert!(
        err.to_string().contains(expected) && err.to_string().contains("drop and recreate"),
        "same connection: {err}"
    );
    drop(c);

    let c = open_with_extension(Some(file.path())).unwrap();
    for sql in [
        "SELECT * FROM sums",
        "INSERT INTO sums(sums) VALUES ('refresh')",
    ] {
        let err = c.execute_batch(sql).expect_err(sql);
        assert!(err.to_string().contains(expected), "{sql}: {err}");
    }
    c.execute_batch("DROP TABLE sums").unwrap();
    assert_eq!(
        rows(
            &c,
            "SELECT name FROM sqlite_schema WHERE name LIKE '\\_\\_ivm\\_%' ESCAPE '\\'"
        ),
        Vec::<Vec<Value>>::new(),
        "DROP TABLE left shadow objects"
    );
    c.execute_batch("INSERT INTO old_orders VALUES ('a', 1)")
        .unwrap();
}

/// Once the arming `UPDATE` has applied a refresh, nothing may fail the
/// refresh: in an explicit transaction a failure would report an error for
/// changes that stay applied. Here the stage table refuses to be emptied.
/// The refresh that fills the stage must still succeed. The next refresh,
/// which empties the stage before staging anything, fails with nothing
/// changed.
#[test]
fn nothing_after_the_apply_can_fail_a_refresh() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    // The bootstrap leaves its changes staged; a refresh with nothing to
    // apply empties the stage and stages nothing.
    refresh(&c, "sums").unwrap();
    c.execute_batch(
        "CREATE TRIGGER fault BEFORE DELETE ON __ivm_stage_sums \
         BEGIN SELECT RAISE(ABORT, 'injected fault'); END;
         BEGIN;
         INSERT INTO orders VALUES ('a', 1);",
    )
    .unwrap();
    refresh(&c, "sums").expect("the apply succeeded, so the refresh must too");
    assert_matches_oracle(&c, "sums", SUMS);

    c.execute_batch("INSERT INTO orders VALUES ('b', 1)")
        .unwrap();
    let before = durable_state(&c, "sums");
    let err = refresh(&c, "sums").expect_err("the stage cannot be emptied");
    assert!(err.to_string().contains("injected fault"), "{err}");
    assert_eq!(durable_state(&c, "sums"), before);
    assert!(!c.is_autocommit(), "the transaction must still be open");

    c.execute_batch("DROP TRIGGER fault").unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("COMMIT").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Every row of every table in `names`, read from the TEMP schema.
fn temp_contents(c: &Connection, names: &[&str]) -> Vec<Vec<Vec<Value>>> {
    names
        .iter()
        .map(|t| rows(c, &format!("SELECT * FROM temp.\"{t}\"")))
        .collect()
}

/// On the connection that owns the view, a TEMP table named like a base
/// table is neither read by the catalog, the bootstrap or a refresh, nor
/// given the capture triggers: SQLite resolves an unqualified name against
/// `temp` before `main` (Phase 3a spec §4; final review, Important 3).
#[test]
fn a_temp_table_named_like_a_base_table_is_never_read_or_captured() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    // Another column order and an extra column: reading its shape would
    // change the plan's column positions.
    c.execute_batch(
        "CREATE TEMP TABLE orders(amount INTEGER, region TEXT, note TEXT) STRICT;
         INSERT INTO temp.orders VALUES (1000, 'a', 'x'), (2000, 'temp', 'y');",
    )
    .unwrap();
    let main_sums = SUMS.replace("FROM orders", "FROM main.orders");
    create(&c, "sums", SUMS).unwrap();
    assert_matches_oracle(&c, "sums", &main_sums);
    c.execute_batch(
        "INSERT INTO main.orders VALUES ('a', 5), ('d', 6);
         DELETE FROM main.orders WHERE region = 'b';
         UPDATE main.orders SET amount = 50 WHERE amount = 1;
         INSERT INTO temp.orders VALUES (3000, 'temp', 'z');
         DELETE FROM temp.orders WHERE amount = 1000;",
    )
    .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", &main_sums);
    assert_eq!(
        rows(
            &c,
            "SELECT name FROM sqlite_temp_schema WHERE type = 'trigger'"
        ),
        Vec::<Vec<Value>>::new(),
        "a capture trigger landed on the TEMP table"
    );
    c.execute_batch("DROP TABLE sums").unwrap();
    assert_eq!(
        rows(&c, "SELECT count(*) FROM temp.orders"),
        vec![vec![Value::Integer(2)]]
    );
}

/// On the connection that owns the view, TEMP tables named like every table
/// ivmlite creates — the global tables, the delta, pend, output, stage and state
/// tables — are never read, written or dropped by a create, a refresh, a
/// read or a drop (final review, Important 3 and Minor 5: a TEMP
/// `__ivm_out_<view>` used to make `SELECT * FROM v` read the TEMP rows).
#[test]
fn temp_tables_named_like_the_shadow_tables_are_never_touched() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let shadows = [
        "__ivm_meta",
        "__ivm_view",
        "__ivm_dep",
        "__ivm_progress",
        "__ivm_tracked",
        "__ivm_probe",
        "__ivm_delta_orders",
        "__ivm_pend_orders",
        "__ivm_out_sums",
        "__ivm_stage_sums",
        "__ivm_state_sums_0_agg_groups",
    ];
    // Each TEMP table has the columns its namesake gets, so a statement
    // that resolved to it would succeed silently rather than fail; each
    // holds a row the real one never would.
    c.execute_batch(
        "CREATE TEMP TABLE __ivm_meta(key TEXT PRIMARY KEY, value);
         INSERT INTO temp.__ivm_meta VALUES ('format', 1), ('temp', 1);
         CREATE TEMP TABLE __ivm_view(name TEXT PRIMARY KEY, sql TEXT NOT NULL,
             plan TEXT NOT NULL, declaration TEXT NOT NULL, format INTEGER NOT NULL);
         INSERT INTO temp.__ivm_view VALUES ('temp', 'x', 'x', 'x', 1);
         CREATE TEMP TABLE __ivm_dep(view TEXT NOT NULL, tbl TEXT NOT NULL,
             shape TEXT NOT NULL, PRIMARY KEY(view, tbl));
         INSERT INTO temp.__ivm_dep VALUES ('temp', 'x', 'x');
         CREATE TEMP TABLE __ivm_progress(view TEXT NOT NULL, tbl TEXT NOT NULL,
             applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl));
         INSERT INTO temp.__ivm_progress VALUES ('temp', 'x', 0);
         CREATE TEMP TABLE __ivm_tracked(tbl TEXT PRIMARY KEY, shape TEXT NOT NULL,
             broken TEXT);
         INSERT INTO temp.__ivm_tracked VALUES ('temp', 'x', NULL);
         CREATE TEMP TABLE __ivm_probe(n INTEGER NOT NULL);
         INSERT INTO temp.__ivm_probe VALUES (1000);
         CREATE TEMP TABLE __ivm_delta_orders(__ivm_seq INTEGER PRIMARY KEY,
             __ivm_w INTEGER NOT NULL, region TEXT, amount INTEGER);
         INSERT INTO temp.__ivm_delta_orders VALUES (1000, 1, 'temp', 1000);
         CREATE TEMP TABLE __ivm_pend_orders(__ivm_rid INTEGER, region TEXT, amount INTEGER);
         INSERT INTO temp.__ivm_pend_orders VALUES (1000, 'temp', 1000);
         CREATE TEMP TABLE __ivm_out_sums AS
             SELECT region, SUM(amount), COUNT(*), 1 AS __w FROM orders WHERE 0 GROUP BY region;
         INSERT INTO temp.__ivm_out_sums VALUES ('temp', 1000, 1000, 1);
         CREATE TEMP TABLE __ivm_stage_sums(op TEXT NOT NULL, arr INTEGER, key BLOB, val BLOB,
             w INTEGER, tbl TEXT, seq INTEGER, c0 TEXT, c1 INTEGER, c2 INTEGER,
             armed INTEGER NOT NULL DEFAULT 0);
         INSERT INTO temp.__ivm_stage_sums(op) VALUES ('temp');
         CREATE TEMP TABLE __ivm_state_sums_0_agg_groups(key BLOB NOT NULL, val BLOB NOT NULL,
             w INTEGER NOT NULL, PRIMARY KEY(key, val)) WITHOUT ROWID;
         INSERT INTO temp.__ivm_state_sums_0_agg_groups VALUES (x'00', x'00', 1);",
    )
    .unwrap();
    let temp_before = temp_contents(&c, &shadows);

    create(&c, "sums", SUMS).unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
    c.execute_batch(
        "INSERT INTO orders VALUES ('a', 5), ('d', 6);
         DELETE FROM orders WHERE region = 'b';
         UPDATE orders SET amount = 50 WHERE amount = 1;",
    )
    .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
    assert_eq!(temp_contents(&c, &shadows), temp_before);

    c.execute_batch("DROP TABLE sums").unwrap();
    assert_eq!(
        temp_contents(&c, &shadows),
        temp_before,
        "DROP TABLE touched a TEMP table"
    );
    assert_eq!(
        rows(
            &c,
            "SELECT name FROM main.sqlite_schema WHERE name LIKE '\\_\\_ivm\\_%' ESCAPE '\\'"
        ),
        Vec::<Vec<Value>>::new(),
        "DROP TABLE left a shadow object in main"
    );
}

/// On the connection that owns the view, TEMP tables named like the shadow
/// tables REPLACE capture writes — `__ivm_probe`, `__ivm_pend_<t>`,
/// `__ivm_delta_<t>` and `__ivm_tracked` — are never touched, on a table
/// whose unique key makes every conflicting write run the probe and fill
/// the pend table (final review, minor 2: `orders` above has no unique key,
/// so its writes never had a candidate to probe or record). A trigger body
/// that resolved to the TEMP `__ivm_probe` would count its extra row and
/// record nothing; one that resolved to the TEMP `__ivm_tracked` would fail
/// on its missing `broken` column.
#[test]
fn temp_tables_named_like_the_capture_tables_are_never_touched_by_replace_capture() {
    let q = "SELECT k, v, COUNT(*) FROM u GROUP BY k, v";
    let by_v = "SELECT v, COUNT(*) FROM u GROUP BY v";
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE u(id INTEGER PRIMARY KEY, k TEXT UNIQUE, v INTEGER) STRICT;
         INSERT INTO u VALUES (1, 'a', 1), (2, 'b', 1), (3, 'c', 2);
         CREATE TEMP TABLE __ivm_probe(n INTEGER NOT NULL);
         INSERT INTO temp.__ivm_probe VALUES (1000);
         CREATE TEMP TABLE __ivm_pend_u(__ivm_rid INTEGER, id INTEGER, k TEXT, v INTEGER);
         INSERT INTO temp.__ivm_pend_u VALUES (1000, 1000, 'temp', 1000);
         CREATE TEMP TABLE __ivm_delta_u(__ivm_seq INTEGER PRIMARY KEY,
             __ivm_w INTEGER NOT NULL, id INTEGER, k TEXT, v INTEGER);
         INSERT INTO temp.__ivm_delta_u VALUES (1000, 1, 1000, 'temp', 1000);
         CREATE TEMP TABLE __ivm_tracked(tbl TEXT PRIMARY KEY, shape TEXT NOT NULL);
         INSERT INTO temp.__ivm_tracked VALUES ('u', 'temp');",
    )
    .unwrap();
    let shadows = [
        "__ivm_probe",
        "__ivm_pend_u",
        "__ivm_delta_u",
        "__ivm_tracked",
    ];
    let temp_before = temp_contents(&c, &shadows);
    create(&c, "everything", q).unwrap();
    create(&c, "by_v", by_v).unwrap();
    c.execute_batch(
        "PRAGMA recursive_triggers = OFF;
         INSERT OR REPLACE INTO u VALUES (4, 'a', 5);
         UPDATE OR REPLACE u SET k = 'c' WHERE k = 'b';
         REPLACE INTO u VALUES (2, 'z', 1);",
    )
    .unwrap();
    for (view, sql) in [("everything", q), ("by_v", by_v)] {
        refresh(&c, view).unwrap();
        assert_matches_oracle(&c, view, sql);
    }
    assert_eq!(temp_contents(&c, &shadows), temp_before);
    for view in ["everything", "by_v"] {
        c.execute_batch(&format!("DROP TABLE {view}")).unwrap();
    }
    assert_eq!(
        temp_contents(&c, &shadows),
        temp_before,
        "DROP TABLE touched a TEMP table"
    );
}

/// A SUM past `i64::MAX` panics in the core engine in a debug build
/// (`agg.rs` adds with overflow checks on). Every extension callback runs
/// inside `guard`, so create and refresh fail with an SQLite error rather
/// than unwinding across the FFI boundary and aborting the host process
/// (final review, Important 4). A release build wraps instead — within the
/// parent spec's "overflow is undefined" (§6.1).
#[test]
fn an_overflowing_sum_is_an_sqlite_error_not_a_crash() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE big(k TEXT, x INTEGER) STRICT;
         INSERT INTO big VALUES ('a', 9223372036854775807), ('a', 1);",
    )
    .unwrap();
    let q = "SELECT k, SUM(x) FROM big GROUP BY k";
    let err = create(&c, "big_sums", q).expect_err("the bootstrap overflows");
    assert!(err.to_string().contains("overflow"), "create: {err}");

    c.execute_batch("DELETE FROM big WHERE x = 1").unwrap();
    create(&c, "big_sums", q).unwrap();
    c.execute_batch("INSERT INTO big VALUES ('a', 1)").unwrap();
    let err = refresh(&c, "big_sums").expect_err("the refresh overflows");
    assert!(err.to_string().contains("overflow"), "refresh: {err}");
    // The failed refresh changed nothing, and the view can still be dropped.
    assert_eq!(
        rows(&c, "SELECT * FROM big_sums"),
        vec![vec![Value::Text("a".into()), Value::Integer(i64::MAX)]]
    );
    c.execute_batch("DROP TABLE big_sums").unwrap();
}

/// Spec §6 scenario 6 (M-1 scenario 9).
#[test]
fn drop_removes_every_shadow_object_and_the_base_tables_stay_writable() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let user_objects = objects(&c);
    create(&c, "managers", JOIN).unwrap();
    c.execute_batch("DROP TABLE managers").unwrap();
    assert_eq!(objects(&c), user_objects);
    c.execute_batch("INSERT INTO orders VALUES ('a', 1); INSERT INTO regions VALUES ('d', 'dee');")
        .unwrap();
}

/// A view written `FROM ORDERS` over a table declared `orders` (and the
/// reverse case, a table declared `ORDERS` over `FROM orders`) is created and
/// maintained correctly: `pragma_table_list`'s lookup is `COLLATE NOCASE`
/// (Task 3 controller ruling).
#[test]
fn table_name_case_does_not_prevent_a_view_from_being_created_or_maintained() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let upper = "SELECT region, SUM(amount), COUNT(*) FROM ORDERS GROUP BY region";
    create(&c, "sums_upper", upper).unwrap();
    assert_matches_oracle(&c, "sums_upper", SUMS);
    c.execute_batch("INSERT INTO orders VALUES ('a', 42)")
        .unwrap();
    refresh(&c, "sums_upper").unwrap();
    assert_matches_oracle(&c, "sums_upper", SUMS);

    let c2 = open_with_extension(None).unwrap();
    c2.execute_batch(
        "CREATE TABLE ORDERS(region TEXT, amount INTEGER) STRICT;
         INSERT INTO ORDERS VALUES ('a', 1), ('a', 2), ('b', 5), (NULL, 4);",
    )
    .unwrap();
    let lower = "SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region";
    create(&c2, "sums_lower", lower).unwrap();
    assert_matches_oracle(&c2, "sums_lower", lower);
    c2.execute_batch("INSERT INTO ORDERS VALUES ('c', 9)")
        .unwrap();
    refresh(&c2, "sums_lower").unwrap();
    assert_matches_oracle(&c2, "sums_lower", lower);
}

/// The COLLATE refusal matches the keyword as a token, not as a substring: a
/// column named `collateral` (or `x_collate`) is an ordinary column (final
/// review, Minor 6). A conflict clause other than REPLACE is accepted too.
#[test]
fn a_column_named_collateral_is_not_a_collate_clause() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE loans(collateral TEXT, x_collate INTEGER UNIQUE ON CONFLICT IGNORE) STRICT;
         INSERT INTO loans VALUES ('house', 1), ('car', 2), ('house', 3);",
    )
    .unwrap();
    let q = "SELECT collateral, SUM(x_collate), COUNT(*) FROM loans GROUP BY collateral";
    create(&c, "by_collateral", q).unwrap();
    assert_matches_oracle(&c, "by_collateral", q);
    c.execute_batch("INSERT INTO loans VALUES ('car', 4)")
        .unwrap();
    refresh(&c, "by_collateral").unwrap();
    assert_matches_oracle(&c, "by_collateral", q);
}

/// Statement-level REPLACE conflict resolution (`INSERT OR REPLACE`,
/// `REPLACE INTO`, `UPDATE OR REPLACE`) removes rows without an explicit
/// DELETE. SQLite fires the DELETE triggers for them only when the writing
/// connection has `PRAGMA recursive_triggers = ON` (measured: with it off,
/// only the new row's +1 reaches the delta table). With it on, a rowid
/// conflict and a UNIQUE conflict are both captured by those DELETE triggers
/// and the view stays equal to the oracle (final review, Critical 1). With it
/// off, Phase 3b's REPLACE capture records them instead (spec §6.2;
/// `extension_replace.rs`).
#[test]
fn replace_conflicts_are_captured_when_the_writer_has_recursive_triggers_on() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE kv(id INTEGER PRIMARY KEY, k TEXT UNIQUE, g TEXT, x INTEGER) STRICT;
         INSERT INTO kv VALUES (1, 'a', 'p', 10), (2, 'b', 'p', 20), (3, 'c', 'q', 30);",
    )
    .unwrap();
    let q = "SELECT g, SUM(x), COUNT(*) FROM kv GROUP BY g";
    create(&c, "kv_sums", q).unwrap();
    c.execute_batch("PRAGMA recursive_triggers = ON").unwrap();
    for write in [
        // A rowid conflict (and the same UNIQUE key): row 1 is replaced.
        "INSERT OR REPLACE INTO kv VALUES (1, 'a', 'q', 100)",
        // A UNIQUE conflict on k: row 2 is removed, row 4 inserted.
        "REPLACE INTO kv VALUES (4, 'b', 'q', 5)",
        // An UPDATE whose new k collides with row 3: row 3 is removed.
        "UPDATE OR REPLACE kv SET k = 'c' WHERE id = 4",
        // A rowid conflict with a new UNIQUE key.
        "INSERT OR REPLACE INTO kv VALUES (1, 'z', 'p', 7)",
    ] {
        c.execute_batch(write).unwrap();
        refresh(&c, "kv_sums").unwrap();
        assert_matches_oracle(&c, "kv_sums", q);
    }
}

/// An upsert (`INSERT … ON CONFLICT(col) DO UPDATE`) runs an ordinary UPDATE
/// on the conflicting row, so its UPDATE trigger fires without
/// recursive_triggers (final review, Critical 1).
#[test]
fn an_upsert_is_captured_without_recursive_triggers() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE kv(k TEXT UNIQUE, g TEXT, x INTEGER) STRICT;
         INSERT INTO kv VALUES ('a', 'p', 10), ('b', 'p', 20), ('c', 'q', 30);",
    )
    .unwrap();
    let q = "SELECT g, SUM(x), COUNT(*) FROM kv GROUP BY g";
    create(&c, "kv_sums", q).unwrap();
    c.execute_batch(
        "INSERT INTO kv VALUES ('a', 'q', 1) ON CONFLICT(k) DO UPDATE SET g = excluded.g, x = x + excluded.x;
         INSERT INTO kv VALUES ('d', 'p', 4) ON CONFLICT(k) DO UPDATE SET x = x + excluded.x;
         INSERT INTO kv VALUES ('b', 'p', 2) ON CONFLICT(k) DO NOTHING;",
    )
    .unwrap();
    refresh(&c, "kv_sums").unwrap();
    assert_matches_oracle(&c, "kv_sums", q);
}

/// Phase 3b spec §7: renaming a view fails, and leaves it working and droppable.
#[test]
fn a_view_cannot_be_renamed() {
    for explicit_transaction in [false, true] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        let user_objects = objects(&c);
        create(&c, "sums", SUMS).unwrap();
        if explicit_transaction {
            c.execute_batch("BEGIN").unwrap();
        }
        let err = c
            .execute_batch("ALTER TABLE sums RENAME TO totals")
            .expect_err("rename");
        assert!(
            err.to_string().contains("ivmlite views cannot be renamed"),
            "{err}"
        );
        if explicit_transaction {
            c.execute_batch("COMMIT").unwrap();
        }
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'totals'"
            ),
            0
        );
        c.execute_batch("INSERT INTO orders VALUES ('a', 5)")
            .unwrap();
        refresh(&c, "sums").unwrap();
        assert_matches_oracle(&c, "sums", SUMS);
        c.execute_batch("DROP TABLE sums").unwrap();
        assert_eq!(objects(&c), user_objects);
    }
}

/// A base table with columns named `w` and `seq` is maintained correctly: the
/// delta table's own columns are `__ivm_seq` / `__ivm_w`, so they do not
/// shadow same-named base columns (Task 3 controller ruling).
#[test]
fn base_columns_named_w_and_seq_are_not_shadowed_by_the_delta_table() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, w INTEGER, seq INTEGER) STRICT;
         INSERT INTO t VALUES ('a', 1, 10), ('a', 2, 20), ('b', 5, 30);",
    )
    .unwrap();
    let q = "SELECT k, SUM(w), SUM(seq), COUNT(*) FROM t GROUP BY k";
    create(&c, "sums", q).unwrap();
    assert_matches_oracle(&c, "sums", q);

    c.execute_batch(
        "INSERT INTO t VALUES ('c', 7, 70);
         DELETE FROM t WHERE k = 'b';
         UPDATE t SET w = 100 WHERE k = 'a' AND w = 1;",
    )
    .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", q);
}
