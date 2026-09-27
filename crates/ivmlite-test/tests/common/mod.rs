//! Helpers shared by the extension integration tests (each `tests/*.rs` file
//! is its own crate, so they are included with `mod common;`).
#![allow(dead_code)] // each test crate uses a different subset

use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::Connection;

pub const SUMS: &str = "SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region";
pub const JOIN: &str = "SELECT r.manager, SUM(o.amount) FROM orders o JOIN regions r \
                    ON o.region = r.name GROUP BY r.manager";

pub fn setup(c: &Connection) {
    c.execute_batch(
        "CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
         CREATE TABLE regions(name TEXT, manager TEXT) STRICT;
         INSERT INTO orders VALUES ('a', 1), ('a', 2), ('b', 5), (NULL, 4);
         INSERT INTO regions VALUES ('a', 'ann'), ('b', 'bob'), ('c', 'bob');",
    )
    .unwrap();
}

pub fn create(c: &Connection, name: &str, sql: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {name} USING ivm('{}')",
        sql.replace('\'', "''")
    ))
}

pub fn refresh(c: &Connection, name: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!("INSERT INTO {name}({name}) VALUES ('refresh')"))
}

/// Every row `sql` returns, sorted, so results compare as multisets.
pub fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
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
pub fn assert_matches_oracle(c: &Connection, name: &str, sql: &str) {
    assert_eq!(
        rows(c, &format!("SELECT * FROM {name}")),
        rows(c, sql),
        "view {name}"
    );
}

/// The names of the database's objects other than SQLite's own. The
/// `NOT LIKE 'sqlite_%'` filter also hides `sqlite_sequence`: the delta
/// tables' AUTOINCREMENT creates it, and SQLite never drops it, so it stays
/// after the last view is dropped (Phase 3a spec §6, scenario 6).
pub fn objects(c: &Connection) -> Vec<Vec<Value>> {
    rows(
        c,
        "SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
    )
}

/// Everything a refresh may change: every state table, the output table and
/// the watermarks.
pub fn durable_state(c: &Connection, name: &str) -> Vec<Vec<Vec<Value>>> {
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

pub struct TempFile(PathBuf);

impl TempFile {
    pub fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ivmlite-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        TempFile(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The single integer `sql` returns.
pub fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}
