# M1b Phase 4: SQLite 3.42 compatibility check

**Date:** 2026-10-07. **Extension:** the release build of commit `ea87013`
(`cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml`),
whose extension source equals the Phase 4 ablation's `latch` build
(`9c6ff90`) except for one comment in `view.rs` and the crate version numbers
of the v0.1.0-alpha.1 release.

Phase 4 spec §5 requires that the triggers the extension writes stay readable
by SQLite older than 3.44: no aggregate `ORDER BY`, `FILTER`, window function
or `group_concat`. The test suite checks the trigger text; this check runs a
database the extension created through a real SQLite 3.42, with no extension
loaded, the way an older tool would open it.

## Tools

- **SQLite 3.53.0:** Homebrew's `/opt/homebrew/bin/python3` (3.14.4), which can
  load extensions. It creates the database and refreshes the view.
- **SQLite 3.42.0:** `/usr/local/bin/python3.11`. It opens the database with no
  extension loaded.

## Scripts

`make_db.py`:

```python
# Create a database with the extension (run with a Python whose SQLite can
# load extensions). Usage: make_db.py <db> <extension library>
import sqlite3, sys
db, lib = sys.argv[1], sys.argv[2]
c = sqlite3.connect(db, isolation_level=None)
c.enable_load_extension(True)
c.load_extension(lib, entrypoint="sqlite3_ivmlite_init")
c.executescript("""
CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT) STRICT;
CREATE UNIQUE INDEX i ON t(k);
INSERT INTO t VALUES (1, 5, 'a');
CREATE VIRTUAL TABLE ks USING ivm('SELECT v, COUNT(*) FROM t GROUP BY v');
""")
print("created with SQLite", sqlite3.sqlite_version)
print("view:", c.execute("SELECT * FROM ks").fetchall())
```

`old_sqlite_check.py`:

```python
# Open the database with an old SQLite and no extension loaded: the schema
# must parse, and every write must run the capture triggers, including the
# latch, without false-firing. Usage: old_sqlite_check.py <db>
import sqlite3, sys
c = sqlite3.connect(sys.argv[1], isolation_level=None)
print("SQLite", sqlite3.sqlite_version)
print("triggers:", [r[0] for r in c.execute(
    "SELECT name FROM sqlite_schema WHERE type = 'trigger' ORDER BY name")])
print("read:", c.execute("SELECT * FROM t ORDER BY id").fetchall())
c.execute("INSERT INTO t VALUES (2, 6, 'b')")
c.execute("INSERT OR REPLACE INTO t VALUES (3, 5, 'c')")  # replaces id 1 via k
c.execute("UPDATE t SET v = 'd' WHERE id = 2")
c.execute("DELETE FROM t WHERE id = 3")
print("after writes:", c.execute("SELECT * FROM t ORDER BY id").fetchall())
print("latched:", c.execute("SELECT tbl, broken FROM __ivm_tracked").fetchall())
print("delta rows:", c.execute("SELECT COUNT(*) FROM __ivm_delta_t").fetchone()[0])
```

`refresh_check.py`:

```python
# Refresh the view with the extension and compare it with SQLite's own
# evaluation. Usage: refresh_check.py <db> <extension library>
import sqlite3, sys
db, lib = sys.argv[1], sys.argv[2]
c = sqlite3.connect(db, isolation_level=None)
c.enable_load_extension(True)
c.load_extension(lib, entrypoint="sqlite3_ivmlite_init")
c.execute("INSERT INTO ks(ks) VALUES ('refresh')")
view = sorted(c.execute("SELECT * FROM ks").fetchall())
oracle = sorted(c.execute("SELECT v, COUNT(*) FROM t GROUP BY v").fetchall())
print("SQLite", sqlite3.sqlite_version, "view:", view, "oracle:", oracle)
```

`old_latch_check.py`:

```python
# With an old SQLite and no extension: a unique index added around a write
# must fire the latch. Usage: old_latch_check.py <db>
import sqlite3, sys
c = sqlite3.connect(sys.argv[1], isolation_level=None)
print("SQLite", sqlite3.sqlite_version)
c.execute("CREATE UNIQUE INDEX j ON t(v)")
c.execute("INSERT INTO t VALUES (4, 7, 'e')")
c.execute("DROP INDEX j")
print("latched:", c.execute("SELECT tbl, broken FROM __ivm_tracked").fetchall())
```

## Commands and output

From the repository root, with `$S` a scratch directory holding the scripts and
`LIB=crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib`:

```
$ /opt/homebrew/bin/python3 $S/make_db.py $S/old.db $LIB
created with SQLite 3.53.0
view: [('a', 1)]

$ /usr/local/bin/python3.11 $S/old_sqlite_check.py $S/old.db
SQLite 3.42.0
triggers: ['__ivm_apply_ks', '__ivm_probe_step', '__ivm_trig_t_del', '__ivm_trig_t_ins', '__ivm_trig_t_preins', '__ivm_trig_t_preupd', '__ivm_trig_t_upd']
read: [(1, 5, 'a')]
after writes: [(2, 6, 'd')]
latched: [('t', None)]
delta rows: 6

$ /opt/homebrew/bin/python3 $S/refresh_check.py $S/old.db $LIB
SQLite 3.53.0 view: [('d', 1)] oracle: [('d', 1)]

$ /usr/local/bin/python3.11 $S/old_latch_check.py $S/old.db
SQLite 3.42.0
latched: [('t', 'the definition, unique indexes or capture triggers of t changed after its capture was generated')]
```

## Result

SQLite 3.42 parses the schema, including all seven triggers. It runs the
one-scan latch on every write without false-firing, and the capture triggers
record all four writes, including the REPLACE that conflicts through the
UNIQUE index `i`. Refreshed by the extension afterwards, the view equals
SQLite's own evaluation. On 3.42 the latch still fires when a unique index is
added around a write. This repeats the check made during Phase 4 Task 4, with
the same output.
