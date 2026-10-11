//! The catalog snapshot every view on a connection shares (Phase 6 spec §4):
//! `main.sqlite_schema` read once per schema version, and each base table's
//! capture verdict computed once against it, so that after a schema change a
//! connection's views make one scan between them, not about ten each.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, TryLockError, Weak};

use ivmlite_core::Schema;
use rusqlite::{ffi, Connection};

use crate::view;

type Result<T> = std::result::Result<T, String>;

/// One connection's view of `main.sqlite_schema` at one schema version
/// (spec §4), and the capture verdicts computed against it.
pub struct Snapshot {
    version: i64,
    /// (type, name) → tbl_name, names exact as `sqlite_schema` stores them.
    objects: HashMap<(String, String), String>,
    /// base table → its capture verdict (shape, capture triggers, probe,
    /// delta and pend tables; the latch is never part of it).
    verdicts: HashMap<String, Result<Schema>>,
}

pub type Shared = Arc<Mutex<Option<Snapshot>>>;

/// Spec §4: each connection's snapshot, by the address of its handle. A
/// `Mutex`, because SQLite may call one connection's callbacks from different
/// threads over its life. Only `Weak` references: the views hold the strong
/// ones, so the snapshot is freed when the last view of its connection
/// disconnects, and a later connection that reuses the address finds the
/// dead `Weak` and gets a new snapshot.
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A handle's address → its connection's snapshot, while any view holds it.
type Registry = HashMap<usize, Weak<Mutex<Option<Snapshot>>>>;

/// The snapshot every view on the connection `handle` shares.
pub fn for_connection(handle: *mut ffi::sqlite3) -> Shared {
    // A panic while the registry was held cannot leave it inconsistent: each
    // entry is replaced whole.
    let mut registry = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    registry.retain(|_, snapshot| snapshot.strong_count() > 0);
    let key = handle as usize;
    if let Some(shared) = registry.get(&key).and_then(Weak::upgrade) {
        return shared;
    }
    let shared: Shared = Arc::new(Mutex::new(None));
    registry.insert(key, Arc::downgrade(&shared));
    shared
}

/// Run `body` against `shared` brought to `version` (spec §4, at refresh):
/// rebuilt only when it was read at another version.
pub fn with_current<T>(
    shared: &Shared,
    conn: &Connection,
    version: i64,
    body: impl FnOnce(&mut Snapshot) -> Result<T>,
) -> Result<T> {
    with_slot(shared, |slot| body(Snapshot::current(slot, conn, version)?))
}

/// Run `body` against `shared` read afresh at `version`, whatever version it
/// held (spec §4, at connect). A connect follows the first use of a view on
/// its connection or a reset of the connection's schema, and SQLite resets
/// the schema whenever it rolls back a schema change, which can bring
/// `schema_version` back to a value the snapshot was read at with other
/// objects. The view's own record of what it passed at is new with its
/// connect too.
pub fn with_rebuilt<T>(
    shared: &Shared,
    conn: &Connection,
    version: i64,
    body: impl FnOnce(&mut Snapshot) -> Result<T>,
) -> Result<T> {
    with_slot(shared, |slot| {
        *slot = None;
        body(Snapshot::current(slot, conn, version)?)
    })
}

/// Lock `shared` for `body`. A connection's callbacks never run at the same
/// time, so a locked snapshot means a callback running inside one that holds
/// it, on this same thread, where waiting would deadlock: that callback gets
/// a snapshot of its own instead, read afresh. A panic while the snapshot was
/// held may have left it half rebuilt, so it is dropped and read again.
fn with_slot<T>(
    shared: &Shared,
    body: impl FnOnce(&mut Option<Snapshot>) -> Result<T>,
) -> Result<T> {
    match shared.try_lock() {
        Ok(mut slot) => body(&mut slot),
        Err(TryLockError::Poisoned(poisoned)) => {
            let mut slot = poisoned.into_inner();
            *slot = None;
            shared.clear_poison();
            body(&mut slot)
        }
        Err(TryLockError::WouldBlock) => body(&mut None),
    }
}

impl Snapshot {
    /// The one scan (spec §4), with plain `prepare`: the catalog checks never
    /// enter the view's statement cache (Phase 6 spec §3).
    fn read(conn: &Connection, version: i64) -> Result<Snapshot> {
        let mut stmt = conn
            .prepare("SELECT type, name, tbl_name FROM \"main\".sqlite_schema")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))
            .map_err(|e| e.to_string())?;
        let objects = rows
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?;
        Ok(Snapshot {
            version,
            objects,
            verdicts: HashMap::new(),
        })
    }

    /// Rebuilt with one scan when `version` differs from the held one.
    pub fn current<'a>(
        slot: &'a mut Option<Snapshot>,
        conn: &Connection,
        version: i64,
    ) -> Result<&'a mut Snapshot> {
        if slot.as_ref().is_none_or(|held| held.version != version) {
            *slot = Some(Snapshot::read(conn, version)?);
        }
        Ok(slot.as_mut().expect("filled just above"))
    }

    /// The `tbl_name` of the object of `kind` (`table`, `index`, `trigger`)
    /// named exactly `name`, if `main` holds one.
    pub fn object(&self, kind: &str, name: &str) -> Option<&str> {
        self.objects
            .get(&(kind.to_string(), name.to_string()))
            .map(String::as_str)
    }

    /// `table`'s capture verdict, computed the first time any view of this
    /// connection asks for it at this version and reused after that, a
    /// broken one included, so every view of the table reports it alike. A
    /// failure to read the catalog is not a verdict (an interrupt, say), so
    /// it is returned but not kept.
    pub fn verdict(&mut self, conn: &Connection, table: &str) -> Result<Schema> {
        if let Some(verdict) = self.verdicts.get(table) {
            return verdict.clone();
        }
        let verdict = view::capture_verdict(conn, self, table)?;
        self.verdicts.insert(table.to_string(), verdict.clone());
        verdict
    }
}
