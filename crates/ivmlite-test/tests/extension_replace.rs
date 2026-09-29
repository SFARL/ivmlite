//! M1b Phase 3b spec §6: REPLACE conflict resolution, its schema rules and
//! (Task 4) its capture.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use rusqlite::Connection;

const KS: &str = "SELECT k, COUNT(*) FROM t GROUP BY k";

#[test]
fn literal_defaults_and_non_unique_collations_are_accepted() {
    for ddl in [
        "CREATE TABLE t(k TEXT NOT NULL DEFAULT 'd' UNIQUE, v INTEGER) STRICT",
        "CREATE TABLE t(k TEXT NOT NULL DEFAULT 'x''y' UNIQUE, v INTEGER NOT NULL DEFAULT -5 UNIQUE) STRICT",
        "CREATE TABLE t(k TEXT, v INTEGER NOT NULL DEFAULT (5) UNIQUE) STRICT",
        "CREATE TABLE t(k TEXT, v INTEGER NOT NULL DEFAULT 0x10, UNIQUE(k, v)) STRICT",
        "CREATE TABLE t(k TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, v INTEGER UNIQUE) STRICT",
        "CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE INDEX ik ON t(k COLLATE NOCASE)",
        "CREATE TABLE t(k TEXT PRIMARY KEY, v INTEGER) STRICT, WITHOUT ROWID",
    ] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(ddl).unwrap();
        create(&c, "ks", KS).unwrap_or_else(|e| panic!("{ddl}: {e}"));
    }
}

/// Spec §3: a unique index added after the view was created changes the
/// table's shape, so every view of it reports the change and can still
/// be dropped.
#[test]
fn a_unique_index_added_after_create_breaks_the_view() {
    for (index, expected) in [
        ("CREATE UNIQUE INDEX uv ON t(v)", "changed shape"),
        (
            "CREATE UNIQUE INDEX uk ON t(k COLLATE NOCASE)",
            "collation NOCASE",
        ),
    ] {
        let file = TempFile::new("unique-added");
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE t(k TEXT, v INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);")
            .unwrap();
        create(&c, "ks", KS).unwrap();
        c.execute_batch(index).unwrap();
        let err = refresh(&c, "ks").expect_err(index);
        assert!(
            err.to_string().contains(expected) || err.to_string().contains("changed shape"),
            "{index}: {err}"
        );
        drop(c);
        let c = open_with_extension(Some(file.path())).unwrap();
        let err = c.execute_batch("SELECT * FROM ks").expect_err(index);
        assert!(
            err.to_string().contains(expected),
            "{index} / reopen: {err}"
        );
        c.execute_batch("DROP TABLE ks").unwrap();
    }
}

/// What every view of `t` reports once its capture triggers have run
/// against a definition, unique-index set or capture-trigger placement of
/// `t` other than the one they were generated with (spec §6.3, the latch).
const LATCHED: &str = "the definition, unique indexes or capture triggers of t changed \
                       after its capture was generated";

/// How many tracked tables have latched a change.
const LATCHES: &str = "SELECT count(*) FROM __ivm_tracked WHERE broken IS NOT NULL";

/// `ks` over `t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT)`, in a file, with
/// one row; the connection writes with recursive_triggers OFF, so a row
/// REPLACE removes is captured only through the candidate lookup.
fn transient_index_setup(file: &TempFile) -> Connection {
    let c = open_with_extension(Some(file.path())).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT) STRICT;
         INSERT INTO t VALUES (1, 5, 'a');
         PRAGMA recursive_triggers = OFF;",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c
}

/// Every use of a latched view fails with `LATCHED`, on the connection that
/// wrote and after a reopen; no new view of `t` can be created; and the view
/// still drops, leaving `t` writable and uncaptured.
fn assert_latched_and_droppable(c: Connection, file: &TempFile, case: &str) {
    let err = refresh(&c, "ks").expect_err(case);
    assert!(
        err.to_string().contains(LATCHED) && err.to_string().contains("drop and recreate"),
        "{case}: {err}"
    );
    drop(c);
    let c = open_with_extension(Some(file.path())).unwrap();
    for sql in ["SELECT * FROM ks", "INSERT INTO ks(ks) VALUES ('refresh')"] {
        let err = c.execute_batch(sql).expect_err(sql);
        assert!(err.to_string().contains(LATCHED), "{case} / {sql}: {err}");
    }
    let err = create(&c, "other", "SELECT v, COUNT(*) FROM t GROUP BY v").expect_err(case);
    assert!(err.to_string().contains(LATCHED), "{case} / create: {err}");
    c.execute_batch("DROP TABLE ks").unwrap();
    assert_eq!(
        objects(&c),
        rows(
            &c,
            "SELECT type, name FROM sqlite_schema WHERE tbl_name = 't' AND type IN ('table', 'index')"
        ),
        "{case}: DROP TABLE left objects"
    );
    c.execute_batch("INSERT INTO t VALUES (3, 7, 'c')").unwrap();
}

/// Final review, C1: the capture triggers are generated from the unique
/// indexes that exist when `t` is first tracked, and the shape check only
/// compares snapshots. A unique index added, used by a REPLACE (whose
/// removed row the triggers never looked up) and dropped again before the
/// next refresh left the shape unchanged, and the refresh succeeded with a
/// wrong count. The BEFORE triggers now latch the change (spec §6.3).
#[test]
fn a_unique_index_added_and_dropped_between_refreshes_breaks_the_view() {
    let file = TempFile::new("transient-unique");
    let c = transient_index_setup(&file);
    c.execute_batch(
        "CREATE UNIQUE INDEX i ON t(k);
         INSERT OR REPLACE INTO t VALUES (2, 5, 'b');
         DROP INDEX i;",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "transient unique index");
}

/// Final review, C1 (reviewer's h9): once a refresh has reported the added
/// unique index, dropping it again must not make the view maintainable:
/// the REPLACE it let through is still missing from the deltas.
#[test]
fn a_view_that_reported_an_added_unique_index_stays_broken_after_it_is_dropped() {
    let file = TempFile::new("reverted-unique");
    let c = transient_index_setup(&file);
    c.execute_batch(
        "CREATE UNIQUE INDEX i ON t(k);
         INSERT OR REPLACE INTO t VALUES (2, 5, 'b');",
    )
    .unwrap();
    let err = refresh(&c, "ks").expect_err("the unique index exists");
    assert!(err.to_string().contains("cannot be maintained"), "{err}");
    c.execute_batch("DROP INDEX i").unwrap();
    assert_latched_and_droppable(c, &file, "reverted unique index");
}

/// Of `t`'s indexes, the latch covers the explicit unique ones only, and
/// only while a write sees them: a non-unique index added and dropped around writes,
/// and a unique index added and dropped with no write in between, leave
/// the triggers exactly as correct as before, and the view keeps working.
#[test]
fn indexes_that_never_change_what_replace_removes_keep_the_view_working() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT) STRICT;
         INSERT INTO t VALUES (1, 5, 'a');
         PRAGMA recursive_triggers = OFF;",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch(
        "CREATE INDEX n ON t(k);
         INSERT OR REPLACE INTO t VALUES (2, 5, 'b');
         UPDATE t SET k = 6 WHERE id = 1;
         DROP INDEX n;
         CREATE UNIQUE INDEX u ON t(v);
         DROP INDEX u;
         INSERT OR REPLACE INTO t VALUES (1, 5, 'c');",
    )
    .unwrap();
    refresh(&c, "ks").unwrap();
    assert_matches_oracle(&c, "ks", KS);
    assert_eq!(count(&c, LATCHES), 0);
}

/// Final review, minor 2: dropping a unique index the triggers were
/// generated with changes the shape, so the view reports it; a write after
/// the drop also sets the latch (the index set differs either way, and the
/// fingerprint compares it exactly), which stays set when the same index is
/// created again. Both are "broken", never silently wrong.
#[test]
fn dropping_a_unique_index_the_capture_was_generated_with_breaks_the_view() {
    let file = TempFile::new("dropped-unique");
    let c = open_with_extension(Some(file.path())).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT) STRICT;
         CREATE UNIQUE INDEX uk ON t(k);
         INSERT INTO t VALUES (1, 5, 'a');
         PRAGMA recursive_triggers = OFF;",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch("DROP INDEX uk").unwrap();
    let err = refresh(&c, "ks").expect_err("the unique index is gone");
    assert!(err.to_string().contains("changed shape"), "{err}");
    assert_eq!(count(&c, LATCHES), 0);
    c.execute_batch(
        "INSERT INTO t VALUES (2, 5, 'b');
         DELETE FROM t WHERE id = 2;
         CREATE UNIQUE INDEX uk ON t(k);",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "dropped and recreated unique index");
}

/// `ks` over `t(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT)` with a
/// unique index on the nullable `k`, in a file, with one row; the connection
/// writes with recursive_triggers OFF. The capture triggers are generated
/// for a nullable `k`, so they never substitute its default for a NULL.
fn nullable_default_key_setup(file: &TempFile) -> Connection {
    let c = open_with_extension(Some(file.path())).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT) STRICT;
         CREATE UNIQUE INDEX i ON t(k);
         INSERT INTO t VALUES (1, 5, 'a');
         PRAGMA recursive_triggers = OFF;",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c
}

/// Final re-review, residual (rr2 R1): `ALTER COLUMN k SET NOT NULL` makes
/// a REPLACE of a NULL `k` substitute the default 5 and remove row 1, which
/// the triggers never looked up; `DROP NOT NULL` then restored the shape,
/// and the refresh succeeded with a wrong view. The latch fingerprints t's
/// own `CREATE TABLE` text too, so the write latches the change.
#[test]
fn a_not_null_set_used_by_a_replace_and_dropped_again_breaks_the_view() {
    let file = TempFile::new("transient-not-null");
    let c = nullable_default_key_setup(&file);
    c.execute_batch(
        "ALTER TABLE t ALTER COLUMN k SET NOT NULL;
         INSERT OR REPLACE INTO t VALUES (2, NULL, 'b');
         ALTER TABLE t ALTER COLUMN k DROP NOT NULL;",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "transient NOT NULL");
}

/// Final re-review, residual: once a refresh has reported the NOT NULL
/// constraint as a changed shape, dropping it again must not make the view
/// maintainable: the REPLACE it let through is still missing from the deltas.
#[test]
fn a_view_that_reported_a_set_not_null_stays_broken_after_it_is_dropped() {
    let file = TempFile::new("reverted-not-null");
    let c = nullable_default_key_setup(&file);
    c.execute_batch(
        "ALTER TABLE t ALTER COLUMN k SET NOT NULL;
         INSERT OR REPLACE INTO t VALUES (2, NULL, 'b');",
    )
    .unwrap();
    let err = refresh(&c, "ks").expect_err("k is NOT NULL");
    assert!(err.to_string().contains("cannot be maintained"), "{err}");
    c.execute_batch("ALTER TABLE t ALTER COLUMN k DROP NOT NULL")
        .unwrap();
    assert_latched_and_droppable(c, &file, "reverted NOT NULL");
}

/// The latch reads t's definition only when a write runs: a NOT NULL set
/// and dropped again with no write in between leaves the triggers exactly
/// as correct as before, and the view keeps working.
#[test]
fn a_not_null_set_and_dropped_with_no_write_between_keeps_the_view_working() {
    let file = TempFile::new("unused-not-null");
    let c = nullable_default_key_setup(&file);
    c.execute_batch(
        "ALTER TABLE t ALTER COLUMN k SET NOT NULL;
         ALTER TABLE t ALTER COLUMN k DROP NOT NULL;
         INSERT OR REPLACE INTO t VALUES (2, 5, 'b');
         INSERT INTO t VALUES (3, NULL, 'c');",
    )
    .unwrap();
    refresh(&c, "ks").unwrap();
    assert_matches_oracle(&c, "ks", KS);
    assert_eq!(count(&c, LATCHES), 0);
}

/// `ks` over `"t"(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT)`, in a
/// file, with one row, then `extra` (DDL run before the view is created);
/// the connection writes with recursive_triggers OFF. The name is quoted in
/// the DDL, so a rename round trip restores `t`'s text exactly.
fn quoted_table_setup(file: &TempFile, extra: &str) -> Connection {
    let c = open_with_extension(Some(file.path())).unwrap();
    c.execute_batch(&format!(
        "CREATE TABLE \"t\"(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT) STRICT;
         {extra}
         INSERT INTO t VALUES (1, 5, 'a');
         PRAGMA recursive_triggers = OFF;"
    ))
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c
}

/// Final re-review, Ruling 17 (P7): the fingerprint looked `t` up by name.
/// `t` renamed to `u`, a decoy `t` with `t`'s exact text in its place, a
/// unique index added to `u` and used by a REPLACE, then everything undone
/// and `u` renamed back: the shape and the looked-up text were equal again,
/// and the refresh succeeded with a wrong view. The fingerprint now also
/// names the capture triggers on `t`, which sit on `u` while it is renamed
/// away, so the write latches.
#[test]
fn a_unique_index_used_while_t_is_renamed_behind_a_decoy_breaks_the_view() {
    let file = TempFile::new("decoy-unique");
    let c = quoted_table_setup(&file, "");
    c.execute_batch(
        "ALTER TABLE t RENAME TO u;
         CREATE TABLE \"t\"(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT) STRICT;
         CREATE UNIQUE INDEX j ON u(k);
         INSERT OR REPLACE INTO u VALUES (2, 5, 'b');
         DROP INDEX j;
         DROP TABLE t;
         ALTER TABLE u RENAME TO t;",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "unique index behind a decoy");
}

/// Final re-review, Ruling 17 (P7d): the same round trip, with `t`'s own
/// unique index moved onto the decoy and a NOT NULL set on `u`, used by a
/// REPLACE of a NULL key and dropped again.
#[test]
fn a_not_null_used_while_t_is_renamed_behind_a_decoy_breaks_the_view() {
    let file = TempFile::new("decoy-not-null");
    let c = quoted_table_setup(&file, "CREATE UNIQUE INDEX \"i\" ON \"t\"(k);");
    c.execute_batch(
        "ALTER TABLE t RENAME TO u;
         DROP INDEX i;
         CREATE TABLE \"t\"(id INTEGER PRIMARY KEY, k INTEGER DEFAULT 5, v TEXT) STRICT;
         CREATE UNIQUE INDEX \"i\" ON \"t\"(k);
         CREATE UNIQUE INDEX i2 ON u(k);
         ALTER TABLE u ALTER COLUMN k SET NOT NULL;
         INSERT OR REPLACE INTO u VALUES (2, NULL, 'b');
         ALTER TABLE u ALTER COLUMN k DROP NOT NULL;
         DROP INDEX i2;
         DROP TABLE t;
         ALTER TABLE u RENAME TO t;
         CREATE UNIQUE INDEX \"i\" ON \"t\"(k);",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "NOT NULL behind a decoy");
}

/// The capture triggers are looked up only when a write runs: `t` renamed
/// away and back with no write in between is back exactly as generated,
/// and the view keeps working.
#[test]
fn a_rename_round_trip_with_no_write_between_keeps_the_view_working() {
    let file = TempFile::new("rename-round-trip");
    let c = quoted_table_setup(&file, "CREATE UNIQUE INDEX \"i\" ON \"t\"(k);");
    c.execute_batch(
        "ALTER TABLE t RENAME TO u;
         ALTER TABLE u RENAME TO t;
         INSERT OR REPLACE INTO t VALUES (2, 5, 'b');
         UPDATE t SET v = 'c' WHERE id = 2;",
    )
    .unwrap();
    refresh(&c, "ks").unwrap();
    assert_matches_oracle(&c, "ks", KS);
    assert_eq!(count(&c, LATCHES), 0);
}

/// Spec §6.2: the latch compares the unique-index set, not only its size.
/// `t`'s unique index on `k` swapped for one on `v` keeps the count at one,
/// and a REPLACE through `v` removes a row the triggers never looked up;
/// the original index recreated with its exact text restores the shape.
#[test]
fn a_unique_index_swapped_for_another_breaks_the_view() {
    let file = TempFile::new("swapped-unique");
    let c = quoted_table_setup(&file, "CREATE UNIQUE INDEX \"i\" ON \"t\"(k);");
    c.execute_batch(
        "DROP INDEX i;
         CREATE UNIQUE INDEX j ON t(v);
         INSERT OR REPLACE INTO t VALUES (2, 6, 'a');
         DROP INDEX j;
         CREATE UNIQUE INDEX \"i\" ON \"t\"(k);",
    )
    .unwrap();
    assert_latched_and_droppable(c, &file, "swapped unique index");
}

/// Final re-review, Ruling 18: the capture triggers live in the user's
/// schema, so every SQLite that opens the database parses them, with the
/// extension loaded or not. An aggregate `ORDER BY` (SQLite 3.44) in the
/// latch made the file unreadable to SQLite 3.42 ("malformed database
/// schema"). No ivmlite trigger may use one, nor `group_concat` at all.
#[test]
fn ivmlite_triggers_use_no_aggregate_order_by() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT) STRICT;
         CREATE UNIQUE INDEX i ON t(k);",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    let triggers: Vec<(String, String)> = c
        .prepare(
            "SELECT name, sql FROM sqlite_schema
             WHERE type = 'trigger' AND name LIKE '\\_\\_ivm\\_%' ESCAPE '\\'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    // Five capture triggers, the probe step and the view's apply trigger.
    assert_eq!(triggers.len(), 7, "{triggers:?}");
    for (name, sql) in &triggers {
        let sql = sql.to_ascii_lowercase();
        assert!(
            !sql.contains("group_concat") && !sql.contains("order by"),
            "{name}: {sql}"
        );
        // Phase 4 spec §5: nor an aggregate `FILTER` (3.30) or a window
        // function (3.25), which the single-scan latch might be tempted to use.
        assert!(
            !sql.contains("filter (") && !sql.contains(" over ("),
            "{name}: {sql}"
        );
    }
}

/// Phase 4 spec §5: the latch is one aggregate over sqlite_schema. With the
/// table renamed away the scan finds no rows, and the latch must still fire
/// (count() is 0 on empty input where sum() would be NULL).
#[test]
fn the_latch_fires_when_the_scan_finds_no_row_for_the_table() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);")
        .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch(
        "ALTER TABLE t RENAME TO u; INSERT INTO u VALUES ('b', 2); ALTER TABLE u RENAME TO t;",
    )
    .unwrap();
    let err = refresh(&c, "ks").expect_err("the write while renamed away latched");
    assert!(
        err.to_string()
            .contains("changed after its capture was generated"),
        "{err}"
    );
}

/// Phase 4 spec §5: one scan of sqlite_schema per written row, not four.
#[test]
fn the_latch_reads_sqlite_schema_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT UNIQUE, x INTEGER) STRICT;")
        .unwrap();
    create(&c, "ks", KS).unwrap();
    let body: String = c
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = '__ivm_trig_t_preins'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body.matches("sqlite_schema").count(), 1, "{body}");
}

/// Spec §2's three table variants: each has a `NOT NULL DEFAULT 'd'` unique
/// key, a nullable unique column and a composite unique key.
struct Variant {
    name: &'static str,
    ddl: &'static str,
    cols: &'static [&'static str],
}

const VARIANTS: [Variant; 3] = [
    Variant {
        name: "integer primary key",
        ddl: "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT NOT NULL DEFAULT 'd' UNIQUE, u TEXT UNIQUE, a INTEGER, b INTEGER, x INTEGER, UNIQUE(a, b)) STRICT",
        cols: &["id", "k", "u", "a", "b", "x"],
    },
    Variant {
        name: "rowid",
        ddl: "CREATE TABLE t(k TEXT NOT NULL DEFAULT 'd' UNIQUE, u TEXT UNIQUE, a INTEGER, b INTEGER, x INTEGER, UNIQUE(a, b)) STRICT",
        cols: &["k", "u", "a", "b", "x"],
    },
    Variant {
        name: "without rowid",
        ddl: "CREATE TABLE t(k TEXT NOT NULL DEFAULT 'd' PRIMARY KEY, u TEXT UNIQUE, a INTEGER, b INTEGER, x INTEGER, UNIQUE(a, b)) STRICT, WITHOUT ROWID",
        cols: &["k", "u", "a", "b", "x"],
    },
];

/// User triggers that write `t` again from inside a write to `t` (spec §6.4).
const REENTRY_CONFLICTING: &str = "
    CREATE TRIGGER user_after AFTER INSERT ON t WHEN NEW.x = 3 BEGIN
      INSERT OR REPLACE INTO t(k, u, x) VALUES ('side' || NEW.k, NEW.u, 4); END;
    CREATE TRIGGER user_before BEFORE INSERT ON t WHEN NEW.x = 5 BEGIN
      INSERT OR IGNORE INTO t(k, a, b, x) VALUES (COALESCE(NEW.k, 'd'), NEW.b, NEW.a, 6); END;
    CREATE TRIGGER user_upd AFTER UPDATE ON t WHEN NEW.x = 7 BEGIN
      REPLACE INTO t(k, x) VALUES ('p', 8); END;";
const REENTRY_NON_CONFLICTING: &str = "
    CREATE TRIGGER user_after AFTER INSERT ON t WHEN NEW.x = 3 BEGIN
      INSERT INTO t(k, x) VALUES ('side' || hex(randomblob(8)), 4); END;
    CREATE TRIGGER user_upd AFTER UPDATE ON t WHEN NEW.x = 7 BEGIN
      UPDATE t SET x = 9 WHERE k = NEW.k; END;";

fn int(r: &mut StdRng) -> String {
    match r.random_range(0..10) {
        0 | 1 => "NULL".to_string(),
        // Text into an INTEGER column: STRICT converts it.
        2 => format!("'{}'", r.random_range(0..10)),
        _ => r.random_range(0..10).to_string(),
    }
}

fn text(r: &mut StdRng, pool: &[&str]) -> String {
    if r.random_bool(0.2) {
        "NULL".to_string()
    } else {
        format!("'{}'", pool[r.random_range(0..pool.len())])
    }
}

fn value(r: &mut StdRng, column: &str) -> String {
    match column {
        "id" => {
            if r.random_bool(0.5) {
                "NULL".to_string()
            } else {
                r.random_range(1..13).to_string()
            }
        }
        "k" => text(r, &["d", "p", "q"]),
        "u" => text(r, &["p", "q", "r", "s", "t"]),
        "x" if r.random_bool(0.3) => ["3", "5", "7"][r.random_range(0..3)].to_string(),
        _ => int(r),
    }
}

fn row(r: &mut StdRng, cols: &[&str]) -> String {
    let values: Vec<String> = cols.iter().map(|c| value(r, c)).collect();
    format!("({})", values.join(", "))
}

/// One of spec §2's 14 statement kinds.
fn statement(r: &mut StdRng, cols: &[&str]) -> String {
    let col = cols[r.random_range(0..cols.len())];
    let val = value(r, col);
    let wh = [
        "x IS NULL",
        "x > 4",
        "a = b",
        "k = 'p'",
        "u IS NOT NULL",
        "1",
    ][r.random_range(0..6)];
    let plain: Vec<&str> = cols.iter().copied().filter(|c| *c != "id").collect();
    let swapped: Vec<&str> = plain
        .iter()
        .map(|c| match *c {
            "a" => "b",
            "b" => "a",
            c => c,
        })
        .collect();
    let (r1, r2) = (row(r, cols), row(r, cols));
    match r.random_range(0..14) {
        0 => format!("INSERT INTO t VALUES {r1}"),
        1 => format!("INSERT OR REPLACE INTO t VALUES {r1}"),
        2 => format!("REPLACE INTO t VALUES {r1}, {r2}"),
        3 => format!("INSERT OR IGNORE INTO t VALUES {r1}"),
        4 => format!("INSERT INTO t VALUES {r1} ON CONFLICT(u) DO UPDATE SET x = excluded.x"),
        5 => format!("INSERT INTO t VALUES {r1} ON CONFLICT DO NOTHING"),
        6 => format!("UPDATE t SET {col} = {val} WHERE {wh}"),
        7 => format!("UPDATE OR REPLACE t SET {col} = {val} WHERE {wh}"),
        8 => format!("UPDATE OR IGNORE t SET {col} = {val} WHERE {wh}"),
        9 => format!("DELETE FROM t WHERE {wh}"),
        10 => format!(
            "INSERT OR REPLACE INTO t({}) SELECT {} FROM t WHERE {wh}",
            plain.join(", "),
            swapped.join(", ")
        ),
        11 => format!("UPDATE OR REPLACE t SET a = b, b = a, x = x + 1 WHERE {wh}"),
        // k omitted: its default 'd' may conflict.
        12 => format!(
            "INSERT OR REPLACE INTO t(u, a) VALUES ({}, {})",
            value(r, "u"),
            int(r)
        ),
        // NOT NULL k set to NULL: REPLACE substitutes 'd'.
        _ => format!("UPDATE OR REPLACE t SET k = NULL WHERE {wh}"),
    }
}

#[derive(Clone, Copy)]
enum Recursive {
    RandomPerStatement,
    AlwaysOn,
}

/// When a test installs its user triggers relative to the view's capture
/// triggers. SQLite does not specify the order in which several triggers on
/// one event fire (spec §2). In practice (measured, SQLite 3.53) the trigger
/// created most recently fires first: user triggers created before the view
/// fire after ivmlite's, and ones created after it fire before. Each order
/// puts the user's triggers on the other side of ivmlite's.
#[derive(Clone, Copy)]
enum UserTriggers {
    BeforeView,
    AfterView,
}

impl UserTriggers {
    const BOTH: [UserTriggers; 2] = [UserTriggers::BeforeView, UserTriggers::AfterView];

    fn tag(self) -> &'static str {
        match self {
            UserTriggers::BeforeView => "before-view",
            UserTriggers::AfterView => "after-view",
        }
    }
}

/// Write `t` at random through two connections — one with the extension,
/// one without — and after every refresh compare two views with SQLite's
/// own evaluation (spec §8 scenario 4): one holding `t`'s full contents, and
/// a coarse one grouping by `x` alone. In the first, a row retracted twice
/// vanishes just as a row retracted once does; in the second it shares a
/// group with other rows, so the extra retraction shows as a wrong count.
///
/// `reentry_kind` names `reentry` in the database file's name, with the
/// trigger order, the variant and the seed: both differential tests run at
/// once in this test binary over overlapping seeds, and no two running cases
/// may share a file. `order` installs `reentry` before or after the view.
fn differential(
    reentry_kind: &str,
    order: UserTriggers,
    variant: usize,
    reentry: &str,
    recursive: Recursive,
    seeds: std::ops::Range<u64>,
) {
    let v = &VARIANTS[variant];
    let group = v.cols.join(", ");
    let everything = format!("SELECT {group}, COUNT(*) FROM t GROUP BY {group}");
    let by_x = "SELECT x, COUNT(*) FROM t GROUP BY x";
    let views = [("everything", everything.as_str()), ("by_x", by_x)];
    for seed in seeds {
        let file = TempFile::new(&format!(
            "replace-{reentry_kind}-{}-{variant}-{seed}",
            order.tag()
        ));
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch(v.ddl).unwrap();
        if let UserTriggers::BeforeView = order {
            c.execute_batch(reentry).unwrap();
        }
        for (name, q) in views {
            create(&c, name, q).unwrap();
        }
        if let UserTriggers::AfterView = order {
            c.execute_batch(reentry).unwrap();
        }
        let plain = Connection::open(file.path()).unwrap();
        let mut r = StdRng::seed_from_u64(seed);
        for step in 0..40 {
            let writer = if r.random_bool(0.25) { &plain } else { &c };
            let on = match recursive {
                Recursive::RandomPerStatement => r.random_bool(0.5),
                Recursive::AlwaysOn => true,
            };
            writer
                .execute_batch(&format!(
                    "PRAGMA recursive_triggers = {}",
                    if on { "ON" } else { "OFF" }
                ))
                .unwrap();
            let sql = statement(&mut r, v.cols);
            // Constraint failures are part of the space; they change nothing.
            let _ = writer.execute_batch(&sql);
            if r.random_bool(0.3) || step == 39 {
                for (name, q) in views {
                    refresh(&c, name).unwrap_or_else(|e| {
                        panic!(
                            "{} {} seed {seed} step {step}: {name}: {e}",
                            v.name,
                            order.tag()
                        )
                    });
                    assert_eq!(
                        rows(&c, &format!("SELECT * FROM {name}")),
                        rows(&c, q),
                        "{} {} seed {seed} step {step} view {name} after: {sql}",
                        v.name,
                        order.tag()
                    );
                }
            }
        }
    }
}

#[test]
fn replace_is_captured_without_reentry_whatever_the_writers_recursive_triggers() {
    for variant in 0..VARIANTS.len() {
        differential(
            "none",
            UserTriggers::BeforeView,
            variant,
            "",
            Recursive::RandomPerStatement,
            0..60,
        );
    }
}

/// Spec §6.4: with recursive_triggers ON, capture stays exact even when
/// user triggers write the same table again, whichever side of ivmlite's
/// triggers they fire on.
#[test]
fn replace_is_captured_under_reentry_when_the_writer_has_recursive_triggers_on() {
    for variant in 0..VARIANTS.len() {
        for (kind, reentry) in [
            ("conflicting", REENTRY_CONFLICTING),
            ("non-conflicting", REENTRY_NON_CONFLICTING),
        ] {
            for order in UserTriggers::BOTH {
                differential(kind, order, variant, reentry, Recursive::AlwaysOn, 0..30);
            }
        }
    }
}

/// Phase 3a's deterministic REPLACE writes, now with recursive_triggers OFF.
#[test]
fn replace_conflicts_are_captured_without_recursive_triggers() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE kv(id INTEGER PRIMARY KEY, k TEXT UNIQUE, g TEXT, x INTEGER) STRICT;
         INSERT INTO kv VALUES (1, 'a', 'p', 10), (2, 'b', 'p', 20), (3, 'c', 'q', 30);",
    )
    .unwrap();
    let q = "SELECT g, SUM(x), COUNT(*) FROM kv GROUP BY g";
    create(&c, "kv_sums", q).unwrap();
    c.execute_batch("PRAGMA recursive_triggers = OFF").unwrap();
    for write in [
        "INSERT OR REPLACE INTO kv VALUES (1, 'a', 'q', 100)",
        "REPLACE INTO kv VALUES (4, 'b', 'q', 5)",
        "UPDATE OR REPLACE kv SET k = 'c' WHERE id = 4",
        "INSERT OR REPLACE INTO kv VALUES (1, 'z', 'p', 7)",
    ] {
        c.execute_batch(write).unwrap();
        refresh(&c, "kv_sums").unwrap();
        assert_matches_oracle(&c, "kv_sums", q);
    }
}

/// Spec §6.1: `ON CONFLICT REPLACE` in the DDL is accepted now, and its
/// implicit deletions are captured.
#[test]
fn a_table_declaring_on_conflict_replace_is_maintained() {
    for (ddl, first, conflicting, q) in [
        (
            "CREATE TABLE t(k TEXT UNIQUE ON CONFLICT REPLACE, v INTEGER) STRICT",
            "INSERT INTO t VALUES ('a', 1)",
            "INSERT INTO t VALUES ('a', 2)",
            "SELECT k, v, COUNT(*) FROM t GROUP BY k, v",
        ),
        (
            "CREATE TABLE t(id INTEGER PRIMARY KEY ON CONFLICT REPLACE, k TEXT) STRICT",
            "INSERT INTO t VALUES (1, 'a')",
            "INSERT INTO t VALUES (1, 'b')",
            "SELECT id, k, COUNT(*) FROM t GROUP BY id, k",
        ),
    ] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(ddl).unwrap();
        c.execute_batch(first).unwrap();
        create(&c, "everything", q).unwrap();
        c.execute_batch("PRAGMA recursive_triggers = OFF").unwrap();
        c.execute_batch(conflicting).unwrap();
        refresh(&c, "everything").unwrap();
        assert_matches_oracle(&c, "everything", q);
    }
}

/// Spec §6.3: without its recursive-triggers probe, REPLACE capture cannot
/// tell whether SQLite's own DELETE trigger will fire, so every view is
/// broken — and can still be dropped.
#[test]
fn a_missing_probe_trigger_breaks_the_view() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT UNIQUE, v INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch("DROP TRIGGER __ivm_probe_step").unwrap();
    let err = refresh(&c, "ks").expect_err("the probe trigger is gone");
    assert!(
        err.to_string().contains("__ivm_probe_step is missing"),
        "{err}"
    );
    c.execute_batch("DROP TABLE ks").unwrap();
}

/// Spec §6.2: in a BEFORE INSERT without an explicit rowid, `NEW.rowid` is
/// −1, so a real row at rowid −1 becomes a candidate of every such insert.
/// It is not replaced, and it is still in `t` afterwards, so it must never
/// be confirmed: the confirmation checks that a candidate is gone.
#[test]
fn a_row_at_rowid_minus_one_is_not_taken_for_a_replaced_row() {
    for (ddl, seed_rows, q) in [
        (
            "CREATE TABLE t(k TEXT, v INTEGER) STRICT",
            "INSERT INTO t(rowid, k, v) VALUES (-1, 'neg', 1), (5, 'five', 2)",
            "SELECT k, v, COUNT(*) FROM t GROUP BY k, v",
        ),
        (
            "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT) STRICT",
            "INSERT INTO t VALUES (-1, 'neg'), (5, 'five')",
            "SELECT id, k, COUNT(*) FROM t GROUP BY id, k",
        ),
    ] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(ddl).unwrap();
        c.execute_batch(seed_rows).unwrap();
        create(&c, "everything", q).unwrap();
        c.execute_batch("PRAGMA recursive_triggers = OFF").unwrap();
        c.execute_batch("INSERT INTO t(k) VALUES ('new')").unwrap();
        refresh(&c, "everything").unwrap();
        assert_matches_oracle(&c, "everything", q);
    }
}

/// Spec §6.2: a row deleted between a BEFORE and its AFTER trigger is
/// retracted once, by the DELETE trigger, which also forgets its candidate.
/// Here a user BEFORE INSERT trigger deletes the row a plain INSERT would
/// conflict with, so that INSERT never replaces it. This pins behavior
/// slightly inside spec §6.4's re-entry region (the trigger writes `t`
/// again), with recursive_triggers OFF. When ivmlite's BEFORE fires first,
/// it records the row as a candidate, and without the forget the AFTER
/// would confirm it and count the deletion twice; in the other order there
/// is no candidate to record.
#[test]
fn a_row_a_user_trigger_deletes_before_the_insert_is_retracted_once() {
    // `b` shares `a`'s group, so a second retraction of `a` shows as a
    // wrong count rather than as a group that vanishes either way.
    let q = "SELECT v, COUNT(*) FROM t GROUP BY v";
    for order in UserTriggers::BOTH {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(
            "CREATE TABLE t(k TEXT UNIQUE, v INTEGER) STRICT;
             INSERT INTO t VALUES ('a', 1), ('b', 1);",
        )
        .unwrap();
        let user_clear = "CREATE TRIGGER user_clear BEFORE INSERT ON t BEGIN
             DELETE FROM t WHERE k = NEW.k; END;";
        if let UserTriggers::BeforeView = order {
            c.execute_batch(user_clear).unwrap();
        }
        create(&c, "everything", q).unwrap();
        if let UserTriggers::AfterView = order {
            c.execute_batch(user_clear).unwrap();
        }
        c.execute_batch("PRAGMA recursive_triggers = OFF").unwrap();
        c.execute_batch("INSERT INTO t VALUES ('a', 3)").unwrap();
        refresh(&c, "everything").unwrap_or_else(|e| panic!("user triggers {}: {e}", order.tag()));
        assert_eq!(
            rows(&c, "SELECT * FROM everything"),
            rows(&c, q),
            "user triggers {}",
            order.tag()
        );
    }
}

/// Spec §6.3: dropping the probe table drops its trigger with it, so every
/// view reports the missing probe — and can still be dropped (the global
/// drop at the last view must not require the probe table to exist).
#[test]
fn a_missing_probe_table_breaks_the_view_and_it_still_drops() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT UNIQUE, v INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch("DROP TABLE __ivm_probe").unwrap();
    let err = refresh(&c, "ks").expect_err("the probe table is gone");
    assert!(
        err.to_string().contains("__ivm_probe_step is missing"),
        "{err}"
    );
    c.execute_batch("DROP TABLE ks").unwrap();
    // Capture is gone with the last view, so the table is writable again.
    c.execute_batch("INSERT INTO t VALUES ('b', 2)").unwrap();
}

/// The generated trigger SQL names the base table inside subqueries next to
/// `NEW`, `OLD` and the pend table's alias. A base table named like one of
/// those (in any case) must not capture the reference: a table `p` used to
/// make the confirmation's `p."k"` read the base table, and a table `old`
/// made the BEFORE UPDATE exclusion's `OLD."k"` read it, so REPLACE
/// deletions went uncaptured (task review, Ruling 9).
#[test]
fn base_tables_named_like_the_trigger_aliases_are_captured_exactly() {
    // Every divergent case is collected, so a failure lists them all.
    let mut diverged = Vec::new();
    for name in ["p", "P", "old", "OLD", "new", "New"] {
        for (layout, ddl) in [
            ("rowid", format!("CREATE TABLE \"{name}\"(k TEXT UNIQUE, u TEXT UNIQUE, v INTEGER) STRICT")),
            (
                "without rowid",
                format!("CREATE TABLE \"{name}\"(k TEXT PRIMARY KEY, u TEXT UNIQUE, v INTEGER) STRICT, WITHOUT ROWID"),
            ),
        ] {
            let c = open_with_extension(None).unwrap();
            c.execute_batch(&ddl).unwrap();
            c.execute_batch(&format!(
                "INSERT INTO \"{name}\" VALUES ('a', 'x', 1), ('b', 'y', 2), ('c', 'z', 3), ('d', 'w', 1)"
            ))
            .unwrap();
            let views = [
                ("everything", format!("SELECT k, u, v, COUNT(*) FROM \"{name}\" GROUP BY k, u, v")),
                ("by_v", format!("SELECT v, COUNT(*) FROM \"{name}\" GROUP BY v")),
            ];
            for (view, q) in &views {
                create(&c, view, q).unwrap_or_else(|e| panic!("{layout} {name}: {e}"));
            }
            c.execute_batch("PRAGMA recursive_triggers = OFF").unwrap();
            for write in [
                // Replaces `a` (same k) and `b` (same u).
                format!("INSERT OR REPLACE INTO \"{name}\" VALUES ('a', 'y', 10)"),
                // Replaces `c` through k.
                format!("UPDATE OR REPLACE \"{name}\" SET k = 'c' WHERE k = 'a'"),
                // Replaces `c` (now holding u = 'y') through u.
                format!("UPDATE OR REPLACE \"{name}\" SET u = 'y', v = 1 WHERE k = 'd'"),
            ] {
                c.execute_batch(&write).unwrap();
                for (view, q) in &views {
                    refresh(&c, view).unwrap();
                    if rows(&c, &format!("SELECT * FROM {view}")) != rows(&c, q) {
                        diverged.push(format!("{layout} table {name}, view {view}, after: {write}"));
                    }
                }
            }
        }
    }
    assert!(diverged.is_empty(), "diverged:\n{}", diverged.join("\n"));
}

/// A self-referencing foreign key whose action writes the same table is a
/// same-table re-entry (spec §6.4). `ON DELETE CASCADE` only deletes, and
/// each cascaded delete fires the DELETE trigger, so a REPLACE that removes
/// a parent is captured exactly with `recursive_triggers` OFF and ON alike.
#[test]
fn a_self_referencing_on_delete_cascade_is_captured_exactly() {
    for recursive in ["OFF", "ON"] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT UNIQUE,
                 parent INTEGER REFERENCES t(id) ON DELETE CASCADE) STRICT;
             INSERT INTO t VALUES (1, 'a', NULL), (2, 'b', 1), (3, 'c', 2);",
        )
        .unwrap();
        create(&c, "ks", KS).unwrap();
        c.execute_batch(&format!(
            "PRAGMA recursive_triggers = {recursive};
             INSERT OR REPLACE INTO t VALUES (4, 'a', NULL);"
        ))
        .unwrap();
        refresh(&c, "ks").unwrap();
        assert_matches_oracle(&c, "ks", KS);
    }
}

/// `ON DELETE SET NULL` on a self-referencing foreign key UPDATEs the child
/// row inside the REPLACE that removes its parent: a same-table re-entry,
/// supported only with `recursive_triggers` ON (spec §6.4). With it OFF the
/// child's BEFORE UPDATE empties the candidate table and the parent's −1 is
/// lost (external review of bc0c891, reproduced); that case stays a
/// documented limitation, not a test.
#[test]
fn a_self_referencing_on_delete_set_null_is_captured_with_recursive_triggers_on() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT UNIQUE,
             parent INTEGER REFERENCES t(id) ON DELETE SET NULL) STRICT;
         INSERT INTO t VALUES (1, 'a', NULL), (2, 'b', 1);",
    )
    .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch(
        "PRAGMA recursive_triggers = ON;
         INSERT OR REPLACE INTO t VALUES (3, 'a', NULL);",
    )
    .unwrap();
    refresh(&c, "ks").unwrap();
    assert_matches_oracle(&c, "ks", KS);
}
