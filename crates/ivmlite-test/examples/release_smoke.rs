//! Exercise the exact library extracted from a release archive.
//! Usage: cargo run --locked -p ivmlite-test --example release_smoke -- /path/to/library

use std::error::Error;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ivmlite_test::open_with_extension_at;
use rusqlite::types::Value;
use rusqlite::Connection;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Rows = Vec<Vec<Value>>;

const VIEWS: [(&str, &str); 3] = [
    (
        "a_sums",
        "SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region",
    ),
    (
        "b_counts",
        "SELECT region, COUNT(*) FROM orders GROUP BY region",
    ),
    (
        "c_totals",
        "SELECT amount, COUNT(*) FROM orders GROUP BY amount",
    ),
];

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ivmlite-release-smoke-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rows(c: &Connection, sql: &str) -> Result<Rows> {
    let mut stmt = c.prepare(sql)?;
    let columns = stmt.column_count();
    let mut result: Rows = stmt
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())?
        .collect::<rusqlite::Result<_>>()?;
    result.sort_by_cached_key(|row| format!("{row:?}"));
    Ok(result)
}

fn oracle(c: &Connection, name: &str, sql: &str) -> Result<()> {
    assert_eq!(
        rows(c, &format!("SELECT * FROM {name}"))?,
        rows(c, sql)?,
        "{name}"
    );
    Ok(())
}

fn all_oracles(c: &Connection) -> Result<()> {
    for (name, sql) in VIEWS {
        oracle(c, name, sql)?;
    }
    Ok(())
}

// Include stage tables and watermarks: checking output alone misses partial refreshes.
fn snapshot(c: &Connection) -> Result<Vec<(String, Rows)>> {
    let mut stmt =
        c.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    names
        .into_iter()
        .map(|name| {
            let data = rows(
                c,
                &format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")),
            )?;
            Ok((name, data))
        })
        .collect()
}

fn refresh(c: &Connection, mode: &str) -> rusqlite::Result<Value> {
    c.query_row("SELECT ivm_refresh_all(?1)", [mode], |row| row.get(0))
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let library =
        PathBuf::from(args.next().ok_or("expected one extension library path")?).canonicalize()?;
    if args.next().is_some() {
        return Err("expected exactly one extension library path".into());
    }
    let temp = TempDir::new()?;
    let database = temp.0.join("smoke.db");
    let c = open_with_extension_at(Some(&database), &library)?;
    c.execute_batch(
        "CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
         INSERT INTO orders VALUES ('a', 1), ('a', 2), ('b', 5), (NULL, 4);",
    )?;
    let schema_sql = "SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'";
    let base_schema = rows(&c, schema_sql)?;
    for (name, sql) in VIEWS {
        c.execute_batch(&format!("CREATE VIRTUAL TABLE {name} USING ivm('{sql}')"))?;
    }
    all_oracles(&c)?;

    let stale = rows(&c, "SELECT * FROM a_sums")?;
    c.execute_batch(
        "INSERT INTO orders VALUES ('c', 3);
         UPDATE orders SET amount = 9 WHERE amount = 1;
         DELETE FROM orders WHERE amount = 5;",
    )?;
    assert_eq!(
        rows(&c, "SELECT * FROM a_sums")?,
        stale,
        "view refreshed eagerly"
    );
    assert_ne!(
        rows(&c, VIEWS[0].1)?,
        stale,
        "fixture must change the oracle"
    );
    assert_eq!(refresh(&c, "atomic")?, Value::Integer(3));
    all_oracles(&c)?;

    // Fail the middle view, so atomic and continue must handle views on both sides.
    c.execute_batch(
        "CREATE TRIGGER fault BEFORE INSERT ON __ivm_out_b_counts
         BEGIN SELECT RAISE(ABORT, 'release-smoke-fault'); END;
         INSERT INTO orders VALUES ('d', 7);",
    )?;
    let before = snapshot(&c)?;
    let failed_progress = rows(&c, "SELECT * FROM __ivm_progress WHERE view = 'b_counts'")?;
    let error = refresh(&c, "atomic").expect_err("injected failure must abort atomic refresh");
    assert!(
        error.to_string().contains("no view was refreshed"),
        "{error}"
    );
    assert_eq!(
        snapshot(&c)?,
        before,
        "atomic failure changed persistent state"
    );
    let report = refresh(&c, "continue")?;
    assert!(
        matches!(report, Value::Text(ref text)
        if text.starts_with("refreshed 2, skipped 0, failed 1: b_counts:")
            && text.contains("release-smoke-fault")),
        "{report:?}"
    );
    oracle(&c, VIEWS[0].0, VIEWS[0].1)?;
    oracle(&c, VIEWS[2].0, VIEWS[2].1)?;
    let after = snapshot(&c)?;
    for (name, data) in &before {
        if name.starts_with("__ivm_state_b_counts_")
            || name == "__ivm_out_b_counts"
            || name == "__ivm_stage_b_counts"
        {
            assert_eq!(
                after.iter().find(|(n, _)| n == name).map(|(_, rows)| rows),
                Some(data),
                "{name}"
            );
        }
    }
    assert_eq!(
        rows(&c, "SELECT * FROM __ivm_progress WHERE view = 'b_counts'")?,
        failed_progress
    );
    c.execute_batch("DROP TRIGGER fault")?;
    assert_eq!(refresh(&c, "atomic")?, Value::Integer(1));
    all_oracles(&c)?;

    let before = snapshot(&c)?;
    c.execute_batch("BEGIN; INSERT INTO orders VALUES ('rollback', 99);")?;
    assert_eq!(refresh(&c, "atomic")?, Value::Integer(3));
    all_oracles(&c)?;
    c.execute_batch("ROLLBACK")?;
    assert!(c.is_autocommit());
    assert_eq!(snapshot(&c)?, before, "rollback changed persistent state");
    drop(c);

    let c = open_with_extension_at(Some(&database), &library)?;
    all_oracles(&c)?;
    c.execute_batch("INSERT INTO orders VALUES ('reopened', 11)")?;
    assert_eq!(
        refresh(&c, "continue")?,
        Value::Text("refreshed 3, skipped 0, failed 0".into())
    );
    all_oracles(&c)?;
    for (name, _) in VIEWS {
        c.execute_batch(&format!("DROP TABLE {name}"))?;
    }
    assert_eq!(
        rows(&c, schema_sql)?,
        base_schema,
        "DROP left extension objects"
    );
    c.execute_batch("INSERT INTO orders VALUES ('after-drop', 1)")?;
    println!("release artifact smoke passed: {}", library.display());
    Ok(())
}
