//! The real SQLite extension as an engine under test (M1b Phase 3a).
//!
//! The extension is built separately (see `crates/ivmlite-sqlite/Cargo.toml`),
//! so this module loads its dynamic library from one fixed path and refuses
//! to run against a library older than its sources: a stale or missing
//! library fails the test with an instruction, and is never skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use ivmlite_core::{Database, Row, Value, ZSet};
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;

use crate::{create_table_sql, view_query_to_sql, Engine, EngineError, ViewQuery};

/// The view every engine instance creates.
/// Not `v`: the harness's tables have a column `v`, and a view may not share its
/// name with a result column (the name is its command column).
const VIEW: &str = "ivm_view";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The extension's dynamic library, checked to be at least as new as every
/// source file it is built from.
///
/// # Panics
/// If the library is missing or stale; run `scripts/build-extension.sh`.
pub fn extension_library() -> PathBuf {
    let name = if cfg!(target_os = "macos") {
        "libivmlite_sqlite.dylib"
    } else {
        "libivmlite_sqlite.so"
    };
    let lib = repo().join("crates/ivmlite-sqlite/target/debug").join(name);
    let crates: Vec<PathBuf> = [
        "crates/ivmlite-sqlite",
        "crates/ivmlite-core",
        "crates/ivmlite-sql",
    ]
    .iter()
    .map(|dir| repo().join(dir))
    .collect();
    if let Err(why) = check_library(&lib, &crates) {
        panic!("{why}");
    }
    lib
}

/// `Ok` if `lib` exists and is at least as new as every source file of
/// `crates`; otherwise why not, with the instruction to rebuild. A missing
/// or stale library is an error, never a skip: a skipped extension test
/// would be a false green (Phase 3a spec §2).
fn check_library(lib: &Path, crates: &[PathBuf]) -> Result<(), String> {
    let built = std::fs::metadata(lib)
        .and_then(|m| m.modified())
        .map_err(|_| {
            format!(
                "the ivmlite extension is not built at {}; run scripts/build-extension.sh",
                lib.display()
            )
        })?;
    let newest = crates
        .iter()
        .flat_map(|dir| sources(dir))
        .max()
        .ok_or_else(|| "the ivmlite extension has no source files".to_string())?;
    if built < newest {
        return Err(format!(
            "the ivmlite extension at {} is older than its sources; run scripts/build-extension.sh",
            lib.display()
        ));
    }
    Ok(())
}

/// The modification times of a crate's manifest, its lock file if it has
/// one (the extension builds outside the workspace, with its own
/// `Cargo.lock`), and every file under `src/`.
fn sources(dir: &Path) -> Vec<SystemTime> {
    let mut out = Vec::new();
    let mut stack = vec![dir.join("src")];
    for file in ["Cargo.toml", "Cargo.lock"] {
        if let Ok(m) = std::fs::metadata(dir.join(file)).and_then(|m| m.modified()) {
            out.push(m);
        }
    }
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                out.push(m);
            }
        }
    }
    out
}

/// Open `path` (or an in-memory database) with the extension loaded.
pub fn open_with_extension(path: Option<&Path>) -> rusqlite::Result<Connection> {
    let conn = match path {
        Some(p) => Connection::open(p)?,
        None => Connection::open_in_memory()?,
    };
    // SAFETY: loading our own library, whose entry point only registers the
    // `ivm` module.
    unsafe {
        conn.load_extension_enable()?;
        conn.load_extension(extension_library(), Some("sqlite3_ivmlite_init"))?;
        conn.load_extension_disable()?;
    }
    Ok(conn)
}

fn err(e: rusqlite::Error) -> EngineError {
    EngineError(e.to_string())
}

fn sql_value(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Int(n) => SqlValue::Integer(*n),
        Value::Text(s) => SqlValue::Text(s.clone()),
    }
}

/// A database file under the system temp directory, removed when dropped.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        TempDb(std::env::temp_dir().join(format!("ivmlite-ext-{}-{n}.db", std::process::id())))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Drives the loaded extension through SQL: base-table writes are captured by
/// its triggers, `refresh` is the command channel, `materialize` reads the
/// virtual table.
pub struct SqliteExtensionEngine {
    /// Close and reopen the database before every refresh, so each refresh
    /// starts from persisted state alone (Phase 3a spec §6, scenario 2).
    reopen: bool,
    file: Option<TempDb>,
    conn: Option<Connection>,
    db: Option<Database>,
}

impl SqliteExtensionEngine {
    pub fn new() -> Self {
        SqliteExtensionEngine {
            reopen: false,
            file: None,
            conn: None,
            db: None,
        }
    }

    /// An engine that reopens its database file before every refresh.
    pub fn reopening() -> Self {
        SqliteExtensionEngine {
            reopen: true,
            ..SqliteExtensionEngine::new()
        }
    }

    fn conn(&self) -> Result<&Connection, EngineError> {
        self.conn
            .as_ref()
            .ok_or_else(|| EngineError("create_view has not been called".into()))
    }

    fn columns(&self, table: &str) -> Result<Vec<String>, EngineError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| EngineError("no database".into()))?;
        let schema = db
            .get(table)
            .ok_or_else(|| EngineError(format!("unknown table {table}")))?;
        Ok(schema
            .columns
            .iter()
            .map(|c| format!("\"{}\"", c.name))
            .collect())
    }

    fn insert(&self, table: &str, row: &Row) -> Result<(), EngineError> {
        let cols = self.columns(table)?;
        let params: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let values: Vec<SqlValue> = row.0.iter().map(sql_value).collect();
        self.conn()?
            .prepare_cached(&format!(
                "INSERT INTO \"{table}\"({}) VALUES ({})",
                cols.join(", "),
                params.join(", ")
            ))
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(err)?;
        Ok(())
    }

    fn delete_one(&self, table: &str, row: &Row) -> Result<(), EngineError> {
        let cols = self.columns(table)?;
        let same: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c} IS ?{}", i + 1))
            .collect();
        let values: Vec<SqlValue> = row.0.iter().map(sql_value).collect();
        let changed = self
            .conn()?
            .prepare_cached(&format!(
                "DELETE FROM \"{table}\" WHERE rowid = (SELECT rowid FROM \"{table}\" WHERE {} LIMIT 1)",
                same.join(" AND ")
            ))
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(err)?;
        if changed != 1 {
            return Err(EngineError(format!(
                "delete of {row:?} from {table} matched no row"
            )));
        }
        Ok(())
    }
}

impl Default for SqliteExtensionEngine {
    fn default() -> Self {
        SqliteExtensionEngine::new()
    }
}

impl Engine for SqliteExtensionEngine {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        if self.reopen {
            self.file = Some(TempDb::new());
        }
        self.conn =
            Some(open_with_extension(self.file.as_ref().map(|f| f.0.as_path())).map_err(err)?);
        self.db = Some(db.clone());
        for schema in db.tables() {
            self.conn()?
                .execute_batch(&create_table_sql(schema))
                .map_err(err)?;
            let rows = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!("table {} has no initial state", schema.table))
            })?;
            for (row, &w) in rows.iter() {
                for _ in 0..w {
                    self.insert(&schema.table, row)?;
                }
            }
        }
        let sql = view_query_to_sql(query, db).replace('\'', "''");
        self.conn()?
            .execute_batch(&format!("CREATE VIRTUAL TABLE {VIEW} USING ivm('{sql}')"))
            .map_err(err)
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        for (row, w) in raw {
            for _ in 0..w.unsigned_abs() {
                if *w > 0 {
                    self.insert(table, row)?;
                } else {
                    self.delete_one(table, row)?;
                }
            }
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        if self.reopen {
            drop(self.conn.take());
            self.conn =
                Some(open_with_extension(self.file.as_ref().map(|f| f.0.as_path())).map_err(err)?);
        }
        self.conn()?
            .execute_batch(&format!("INSERT INTO {VIEW}({VIEW}) VALUES ('refresh')"))
            .map_err(err)
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {VIEW}"))
            .map_err(err)?;
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                let mut values = Vec::with_capacity(n);
                for i in 0..n {
                    values.push(match r.get::<_, SqlValue>(i)? {
                        SqlValue::Null => Value::Null,
                        SqlValue::Integer(v) => Value::Int(v),
                        SqlValue::Text(s) => Value::Text(s),
                        other => Value::Text(format!("unexpected {other:?}")),
                    });
                }
                Ok(Row::new(values))
            })
            .map_err(err)?;
        let mut z = ZSet::new();
        for row in rows {
            z.update(row.map_err(err)?, 1);
        }
        Ok(z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A fake crate directory — `Cargo.toml`, `Cargo.lock`, `src/lib.rs` —
    /// and a fake library next to it, each with a chosen modification time
    /// (seconds after an arbitrary epoch); removed when dropped.
    struct FakeBuild {
        dir: PathBuf,
    }

    impl FakeBuild {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ivmlite-libcheck-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("crate/src")).unwrap();
            FakeBuild { dir }
        }

        fn krate(&self) -> PathBuf {
            self.dir.join("crate")
        }

        fn lib(&self) -> PathBuf {
            self.dir.join("libivmlite_sqlite.dylib")
        }

        /// Create (or overwrite) `path` with modification time `secs`.
        fn touch(&self, path: &Path, secs: u64) {
            let file = std::fs::File::create(path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs))
                .unwrap();
        }

        /// Every source at time 10; the manifest's lock file at `lock`.
        fn sources_at(&self, lock: u64) {
            self.touch(&self.krate().join("Cargo.toml"), 10);
            self.touch(&self.krate().join("src/lib.rs"), 10);
            self.touch(&self.krate().join("Cargo.lock"), lock);
        }
    }

    impl Drop for FakeBuild {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_missing_library_is_an_error_not_a_skip() {
        let b = FakeBuild::new("missing");
        b.sources_at(10);
        let err = check_library(&b.lib(), &[b.krate()]).unwrap_err();
        assert!(err.contains("is not built"), "{err}");
        assert!(err.contains("scripts/build-extension.sh"), "{err}");
    }

    #[test]
    fn a_library_older_than_a_source_file_is_an_error() {
        let b = FakeBuild::new("stale-src");
        b.sources_at(10);
        b.touch(&b.lib(), 20);
        b.touch(&b.krate().join("src/lib.rs"), 30);
        let err = check_library(&b.lib(), &[b.krate()]).unwrap_err();
        assert!(err.contains("older than its sources"), "{err}");
    }

    /// The extension's own `Cargo.lock` decides which dependency versions it
    /// is built with, so a newer lock file makes the library stale too
    /// (final review, Minor 11).
    #[test]
    fn a_library_older_than_the_lock_file_is_an_error() {
        let b = FakeBuild::new("stale-lock");
        b.sources_at(30);
        b.touch(&b.lib(), 20);
        let err = check_library(&b.lib(), &[b.krate()]).unwrap_err();
        assert!(err.contains("older than its sources"), "{err}");
    }

    #[test]
    fn a_library_newer_than_every_source_is_accepted() {
        let b = FakeBuild::new("fresh");
        b.sources_at(10);
        b.touch(&b.lib(), 20);
        assert_eq!(check_library(&b.lib(), &[b.krate()]), Ok(()));
    }
}
