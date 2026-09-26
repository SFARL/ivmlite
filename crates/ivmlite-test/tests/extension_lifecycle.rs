//! M1b Phase 3a spec §6, scenarios 2–6, against the real loaded extension:
//! capture from any connection, failure and retry, atomic create, rejections,
//! broken views, and drop.

use std::path::{Path, PathBuf};

use ivmlite_test::open_with_extension;
use rusqlite::types::Value;
use rusqlite::Connection;

const SUMS: &str = "SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region";
const JOIN: &str = "SELECT r.manager, SUM(o.amount) FROM orders o JOIN regions r \
                    ON o.region = r.name GROUP BY r.manager";

fn setup(c: &Connection) {
    c.execute_batch(
        "CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
         CREATE TABLE regions(name TEXT, manager TEXT) STRICT;
         INSERT INTO orders VALUES ('a', 1), ('a', 2), ('b', 5), (NULL, 4);
         INSERT INTO regions VALUES ('a', 'ann'), ('b', 'bob'), ('c', 'bob');",
    )
    .unwrap();
}

fn create(c: &Connection, name: &str, sql: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {name} USING ivm('{}')",
        sql.replace('\'', "''")
    ))
}

fn refresh(c: &Connection, name: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!("INSERT INTO {name}({name}) VALUES ('refresh')"))
}

/// Every row `sql` returns, sorted, so results compare as multisets.
fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = c.prepare(sql).unwrap();
    let n = stmt.column_count();
    let mut out: Vec<Vec<Value>> = stmt
        .query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    out.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    out
}

/// The view agrees with SQLite computing its SELECT from scratch.
fn assert_matches_oracle(c: &Connection, name: &str, sql: &str) {
    assert_eq!(
        rows(c, &format!("SELECT * FROM {name}")),
        rows(c, sql),
        "view {name}"
    );
}

/// The names of the database's objects other than SQLite's own.
fn objects(c: &Connection) -> Vec<Vec<Value>> {
    rows(
        c,
        "SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
    )
}

/// Everything a refresh may change: every state table, the output table and
/// the watermarks.
fn durable_state(c: &Connection, name: &str) -> Vec<Vec<Vec<Value>>> {
    let tables: Vec<String> = rows(
        c,
        &format!(
            "SELECT name FROM sqlite_schema WHERE type = 'table' \
             AND (name LIKE '__ivm_state_{name}_%' OR name = '__ivm_out_{name}')"
        ),
    )
    .into_iter()
    .map(|r| match &r[0] {
        Value::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    })
    .collect();
    let mut all: Vec<Vec<Vec<Value>>> = tables
        .iter()
        .map(|t| rows(c, &format!("SELECT * FROM \"{t}\"")))
        .collect();
    all.push(rows(c, "SELECT * FROM __ivm_progress"));
    all
}

struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ivmlite-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        TempFile(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

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
    assert!(err.to_string().contains("__w"), "{err}");
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
        ("CREATE TABLE t(k TEXT) STRICT; CREATE VIRTUAL TABLE a USING ivm('SELECT k, COUNT(*) FROM t GROUP BY k')", q, "already tracked"),
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
        // A table that declares REPLACE conflict resolution: SQLite fires no
        // DELETE trigger for the rows REPLACE removes unless the writing
        // connection has PRAGMA recursive_triggers ON, so every plain INSERT
        // could silently lose a retraction (final review, Critical 1).
        ("CREATE TABLE t(k TEXT UNIQUE ON CONFLICT REPLACE, v INTEGER) STRICT", q, "ON CONFLICT REPLACE"),
        ("CREATE TABLE t(id INTEGER PRIMARY KEY ON CONFLICT REPLACE, k TEXT) STRICT", q, "ON CONFLICT REPLACE"),
        ("CREATE TABLE t(k TEXT, v INTEGER, UNIQUE(k) on /* spaced */ conflict\n replace) STRICT", q, "ON CONFLICT REPLACE"),
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
/// conflict and a UNIQUE conflict are both captured and the view stays equal
/// to the oracle (final review, Critical 1; a known v0 limitation otherwise,
/// Phase 3a spec §5).
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
