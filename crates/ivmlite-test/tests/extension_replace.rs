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

/// Write `t` at random through two connections — one with the extension,
/// one without — and after every refresh compare a view holding `t`'s full
/// contents with SQLite's own evaluation (spec §8 scenario 4).
///
/// `reentry_kind` names `reentry` in the database file's name, with the
/// variant and the seed: both differential tests run at once in this test
/// binary over overlapping seeds, and no two running cases may share a file.
fn differential(
    reentry_kind: &str,
    variant: usize,
    reentry: &str,
    recursive: Recursive,
    seeds: std::ops::Range<u64>,
) {
    let v = &VARIANTS[variant];
    let group = v.cols.join(", ");
    let everything = format!("SELECT {group}, COUNT(*) FROM t GROUP BY {group}");
    for seed in seeds {
        let file = TempFile::new(&format!("replace-{reentry_kind}-{variant}-{seed}"));
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch(v.ddl).unwrap();
        c.execute_batch(reentry).unwrap();
        create(&c, "everything", &everything).unwrap();
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
                refresh(&c, "everything")
                    .unwrap_or_else(|e| panic!("{} seed {seed} step {step}: {e}", v.name));
                assert_eq!(
                    rows(&c, "SELECT * FROM everything"),
                    rows(&c, &everything),
                    "{} seed {seed} step {step} after: {sql}",
                    v.name
                );
            }
        }
    }
}

#[test]
fn replace_is_captured_without_reentry_whatever_the_writers_recursive_triggers() {
    for variant in 0..VARIANTS.len() {
        differential("none", variant, "", Recursive::RandomPerStatement, 0..60);
    }
}

/// Spec §6.4: with recursive_triggers ON, capture stays exact even when
/// user triggers write the same table again.
#[test]
fn replace_is_captured_under_reentry_when_the_writer_has_recursive_triggers_on() {
    for variant in 0..VARIANTS.len() {
        for (kind, reentry) in [
            ("conflicting", REENTRY_CONFLICTING),
            ("non-conflicting", REENTRY_NON_CONFLICTING),
        ] {
            differential(kind, variant, reentry, Recursive::AlwaysOn, 0..30);
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
