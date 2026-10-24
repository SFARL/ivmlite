//! Native Rust lifecycle with an explicitly selected SQLite extension library.
//! Usage: cargo run --locked -p ivmlite-test --example selective_read_driver -- /absolute/path/to/library

use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ivmlite_test::open_with_extension_at;
use rusqlite::{params, Connection};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type RollupRow = (i64, i64, i64);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ivmlite-selective-read-driver-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Connections are dropped first, including when an assertion unwinds.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn library_argument(mut arguments: impl Iterator<Item = OsString>) -> Result<PathBuf> {
    let usage = "usage: selective_read_driver /absolute/path/to/library";
    let path = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?,
    );
    if !path.is_absolute() || arguments.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, usage).into());
    }
    Ok(path)
}

fn assert_rollup(connection: &Connection, total: i64) -> Result<()> {
    let mut selective = connection.prepare(
        "SELECT monitor_id, bucket_ts, total FROM roll
         WHERE monitor_id = ?1 AND bucket_ts >= ?2 AND bucket_ts < ?3
         ORDER BY monitor_id, bucket_ts",
    )?;
    let selected = selective
        .query_map(params![7, 10, 11], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<RollupRow>>>()?;
    assert_eq!(selected, vec![(7, 10, total)]);

    let mut full = connection
        .prepare("SELECT monitor_id, bucket_ts, total FROM roll ORDER BY monitor_id, bucket_ts")?;
    let rows = full
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<RollupRow>>>()?;
    assert_eq!(rows, vec![(7, 10, total), (7, 11, 20), (8, 10, 30)]);
    Ok(())
}

fn assert_committed_facts(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT monitor_id, bucket_ts, value FROM facts
         ORDER BY monitor_id, bucket_ts, value",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<RollupRow>>>()?;
    assert_eq!(rows, vec![(7, 10, 3), (7, 10, 5), (7, 11, 20), (8, 10, 30)]);
    Ok(())
}

fn run_driver(library: &Path) -> Result<String> {
    let temporary = TempDir::new()?;
    let database = temporary.0.join("selective-reads.db");
    let mut connection = open_with_extension_at(Some(&database), library)?;
    let sqlite_version = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    connection.execute_batch(
        "CREATE TABLE facts(monitor_id INTEGER, bucket_ts INTEGER, value INTEGER);
         INSERT INTO facts VALUES (7, 10, 5), (7, 11, 20), (8, 10, 30);
         CREATE VIRTUAL TABLE roll USING ivm(
             'SELECT monitor_id, bucket_ts, SUM(value) AS total
              FROM facts GROUP BY monitor_id, bucket_ts');",
    )?;
    assert_rollup(&connection, 5)?;

    let transaction = connection.transaction()?;
    transaction.execute("INSERT INTO facts VALUES (?1, ?2, ?3)", params![7, 10, 3])?;
    // Reading the view never refreshes it: the inserted 3 is still pending.
    assert_rollup(&transaction, 5)?;
    transaction.execute_batch("INSERT INTO roll(roll) VALUES ('refresh')")?;
    assert_rollup(&transaction, 8)?;
    transaction.commit()?;
    assert_rollup(&connection, 8)?;
    assert_committed_facts(&connection)?;

    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE facts SET value = ?1 WHERE monitor_id = ?2 AND bucket_ts = ?3",
        params![100, 7, 10],
    )?;
    assert_rollup(&transaction, 8)?;
    transaction.execute_batch("INSERT INTO roll(roll) VALUES ('refresh')")?;
    assert_rollup(&transaction, 200)?;
    transaction.rollback()?;
    assert_rollup(&connection, 8)?;
    assert_committed_facts(&connection)?;
    connection.close().map_err(|(_, error)| error)?;

    let reopened = open_with_extension_at(Some(&database), library)?;
    assert_rollup(&reopened, 8)?;
    assert_committed_facts(&reopened)?;
    reopened.close().map_err(|(_, error)| error)?;
    Ok(sqlite_version)
}

fn main() -> Result<()> {
    let library = library_argument(std::env::args_os().skip(1))?;
    let sqlite_version = run_driver(&library)?;
    println!("selective-read lifecycle passed: {}", library.display());
    println!("SQLite {sqlite_version}; totals: initial/stale=5, committed=8, refreshed=200, rolled-back/reopened=8");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_file_backed_lifecycle() {
        super::run_driver(&ivmlite_test::extension_library()).unwrap();
    }
}
