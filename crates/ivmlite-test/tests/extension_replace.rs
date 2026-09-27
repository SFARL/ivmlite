//! M1b Phase 3b spec §6: REPLACE conflict resolution, its schema rules and
//! (Task 4) its capture.

mod common;

use common::*;
use ivmlite_test::open_with_extension;

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
