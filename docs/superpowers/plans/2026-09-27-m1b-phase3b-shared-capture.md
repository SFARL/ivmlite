# M1b Phase 3b: Shared Capture, Delta GC, REPLACE Capture, Rename Refusal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let any number of ivmlite views share a base table's capture, garbage-collect consumed deltas, capture REPLACE deletions without `recursive_triggers`, and refuse `ALTER TABLE v RENAME`.

**Architecture:** All work is in the `ivmlite-sqlite` cdylib (`crates/ivmlite-sqlite/src/{names,catalog,view,vtab,lib}.rs`) plus tests and one harness mode in `ivmlite-test`. Capture objects become per base table (tracked in a new `__ivm_tracked` table), GC is one more statement in each view's one-statement apply trigger, and REPLACE capture adds a candidate table, two BEFORE triggers and a global recursive-triggers probe. The module is re-registered from a `static` `ffi::sqlite3_module` so it can carry an `xRename`.

**Tech Stack:** Rust 1.95, rusqlite 0.40 (`loadable_extension` + `vtab` in the extension; `bundled` + `load_extension` in the test host), SQLite ≥ 3.45, `rand` (already an `ivmlite-test` dependency).

**Spec:** `docs/superpowers/specs/2026-09-27-m1b-phase3b-shared-capture-design.md` (binding). Also read the Phase 3a spec `docs/superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md` §4–§5: its conventions all still hold.

## Global Constraints

- **English only** in every file, comment, error message, doc and commit message.
- Every identifier the extension issues is double-quoted (`names::quote`). Every statement **outside** a trigger body that names a base table or an object ivmlite owns qualifies it to `"main"` (`names::main_qualified`). Names **inside** a trigger body, and a trigger's `ON` table, stay unqualified (SQLite forbids qualification there); the trigger's own name is `main_qualified`.
- The arming `UPDATE __ivm_stage_<view> SET armed = 1` is the **last** statement of every apply; nothing may run after it (Phase 3a §5).
- Every virtual-table callback's body runs inside `vtab::guard`.
- A broken view still connects, so it can be dropped. Every read and refresh of it fails with `view <name> cannot be maintained: <why>; drop and recreate it`.
- No new dependencies. `rusqlite` stays at 0.40 (the extension's `Cargo.lock` pins 0.40.2).
- The extension is built by `scripts/build-extension.sh`. `scripts/test-all.sh` builds it, then runs its unit tests and `cargo test --workspace --locked --no-fail-fast`. Extension tests fail, never skip, when the library is missing or stale. **After editing anything under `crates/ivmlite-sqlite`, `crates/ivmlite-core` or `crates/ivmlite-sql`, rebuild before running `ivmlite-test` tests.**
- Before every commit, run and pass:
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
  - `scripts/test-all.sh`
- **Mutation gates.** Every new check or behavior a task adds gets a row in `docs/mutation-gates.md`, under `## ivmlite-sqlite`, in that table's four-column format: `| Spec requirement | Mutation | Test that goes red | Verified |`. To verify a row:
  1. Apply the mutation.
  2. Run `scripts/test-all.sh`, summing `passed`/`failed` over both cargo invocations.
  3. Record `**verified**: compiles; scripts/test-all.sh gives N passed / M failed (baseline X/0), red: <test names>`.
  4. Restore the code **and rebuild the extension**.

  Then update the count line (`The table has **R** rows: …`) so `python3 scripts/count-mutation-gates.py` prints `consistent`.
- Git:
  - stage specific files, never `git add -A`, and never touch the untracked `docs/cases/`;
  - never `--no-verify`;
  - every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/ivmlite-sqlite/src/names.rs` | shadow-object names: `TRACKED`, `PROBE`, `PROBE_STEP`, `pend_table`, `CAPTURE_EVENTS` | 1, 4 |
| `crates/ivmlite-sqlite/src/catalog.rs` | `SqliteCatalog::capture` (unique-key metadata), REPLACE schema rules, `is_literal_default` | 3, 4 |
| `crates/ivmlite-sqlite/src/view.rs` | format 2, tracking, watermark, drop, GC, shape, capture triggers, checks | 1–4 |
| `crates/ivmlite-sqlite/src/vtab.rs` | `refuse_rename` | 5 |
| `crates/ivmlite-sqlite/src/lib.rs` | static module with `xRename`, raw registration | 5 |
| `crates/ivmlite-test/src/extension.rs` | `SqliteExtensionEngine` sibling mode | 1 |
| `crates/ivmlite-test/tests/common/mod.rs` | helpers shared by the extension integration tests (moved out of `extension_lifecycle.rs`) | 1 |
| `crates/ivmlite-test/tests/extension_lifecycle.rs` | existing lifecycle tests; rejection list, TEMP list, rename | 1, 3, 4, 5 |
| `crates/ivmlite-test/tests/extension_sharing.rs` | multi-view lifecycle | 1 |
| `crates/ivmlite-test/tests/extension_gc.rs` | GC | 2 |
| `crates/ivmlite-test/tests/extension_replace.rs` | REPLACE differential and rules | 3, 4 |
| `crates/ivmlite-test/tests/extension_differential.rs` | sibling-view sweeps | 1 |
| `docs/…` | specs, README, gate table | every task (gates), 6 |

---

### Task 1: Format 2, shared capture, drop with readers remaining, sibling-view harness

**Files:**
- Modify: `crates/ivmlite-sqlite/src/names.rs`, `crates/ivmlite-sqlite/src/view.rs`
- Modify: `crates/ivmlite-test/src/extension.rs`, `crates/ivmlite-test/tests/extension_lifecycle.rs`, `crates/ivmlite-test/tests/extension_differential.rs`
- Create: `crates/ivmlite-test/tests/common/mod.rs`, `crates/ivmlite-test/tests/extension_sharing.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces (in `names.rs`):
  - `pub const TRACKED: &str = "__ivm_tracked";`
  - `pub const CAPTURE_EVENTS: [&str; 3] = ["ins", "del", "upd"];` — every capture trigger event. Task 4 grows it to 5. **Every** loop over capture triggers must use this constant.
- Produces (in `view.rs`, private):
  - `fn is_tracked(conn: &Connection, table: &str) -> Result<bool>`
  - `fn track(conn: &Connection, schema: &Schema) -> Result<()>` — creates the delta table and capture triggers (the old `create_delta_table`) and inserts the `__ivm_tracked` row.
  - `fn check_table_capture(conn: &Connection, table: &str) -> Result<()>` — stored shape equals current shape, and every `CAPTURE_EVENTS` trigger exists on `table`.
  - `fn readers(conn: &Connection, table: &str) -> Result<Vec<String>>` — views with a `__ivm_dep` row for `table`, sorted.
  - `fn high_watermark(conn: &Connection, table: &str) -> Result<i64>`.
  - `fn untrack(conn: &Connection, table: &str) -> Result<()>` — drops the triggers, then the delta table, then the `__ivm_tracked` row.
- Produces (harness): `SqliteExtensionEngine::with_siblings()` and `SqliteExtensionEngine::reopening_with_siblings()`, both `-> SqliteExtensionEngine`.
- Produces (tests): `tests/common/mod.rs` with `pub` versions of today's helpers in `extension_lifecycle.rs` (`SUMS`, `JOIN`, `setup`, `create`, `refresh`, `rows`, `assert_matches_oracle`, `objects`, `durable_state`, `TempFile`), plus `pub fn count(c: &Connection, sql: &str) -> i64`.

- [ ] **Step 1: Move the shared test helpers**

Create `crates/ivmlite-test/tests/common/mod.rs`. Move into it, unchanged except for adding `pub`:
- the constants `SUMS` and `JOIN`;
- `setup`, `create`, `refresh`, `rows`, `assert_matches_oracle`, `objects`, `durable_state`;
- `struct TempFile` with its `impl`s.

Begin the file with:

```rust
//! Helpers shared by the extension integration tests (each `tests/*.rs` file
//! is its own crate, so they are included with `mod common;`).
#![allow(dead_code)] // each test crate uses a different subset

use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::Connection;
```

and add:

```rust
/// The single integer `sql` returns.
pub fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}
```

In `extension_lifecycle.rs`, delete the moved items and add `mod common;` and `use common::*;` after the module doc comment. Keep `use ivmlite_test::open_with_extension;`, `use rusqlite::types::Value;` and `use rusqlite::Connection;` only where the file still uses them.

Run: `cargo test -p ivmlite-test --test extension_lifecycle --locked`. Expected: all pass, the same count as before.

- [ ] **Step 2: Write the failing sharing tests**

Create `crates/ivmlite-test/tests/extension_sharing.rs`:

```rust
//! M1b Phase 3b spec §4 and §8 scenario 2: several views over one base
//! table share its capture.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rusqlite::Connection;

const COUNTS: &str = "SELECT region, COUNT(*) FROM orders GROUP BY region";

/// Capture triggers per base table (spec §3; Task 4 raises it to 5).
const CAPTURE_TRIGGERS: i64 = 3;

fn capture_objects(c: &Connection, table: &str) -> i64 {
    count(
        c,
        &format!(
            "SELECT count(*) FROM sqlite_schema WHERE name = '__ivm_delta_{table}' \
             OR (type = 'trigger' AND tbl_name = '{table}' AND name LIKE '\\_\\_ivm\\_trig\\_%' ESCAPE '\\')"
        ),
    )
}

#[test]
fn views_over_one_table_share_its_capture_and_each_matches_its_oracle() {
    let file = TempFile::new("sharing");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        create(&c, "counts", COUNTS).unwrap();
        create(&c, "managers", JOIN).unwrap();
        // One delta table and one trigger per capture event, however many views read it.
        assert_eq!(capture_objects(&c, "orders"), 1 + CAPTURE_TRIGGERS);
        assert_eq!(capture_objects(&c, "regions"), 1 + CAPTURE_TRIGGERS);
        c.execute_batch(
            "INSERT INTO orders VALUES ('a', 10), ('c', 7); DELETE FROM orders WHERE region = 'b';
             UPDATE regions SET manager = 'cy' WHERE name = 'c';",
        )
        .unwrap();
        refresh(&c, "counts").unwrap();
        assert_matches_oracle(&c, "counts", COUNTS);
        c.execute_batch("UPDATE orders SET amount = 100 WHERE amount = 1")
            .unwrap();
        refresh(&c, "sums").unwrap();
        refresh(&c, "managers").unwrap();
        refresh(&c, "counts").unwrap();
        for (view, sql) in [("sums", SUMS), ("counts", COUNTS), ("managers", JOIN)] {
            assert_matches_oracle(&c, view, sql);
        }
    }
    {
        let plain = Connection::open(file.path()).unwrap();
        plain
            .execute_batch("INSERT INTO orders VALUES ('c', 1); DELETE FROM regions WHERE name = 'a';")
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    for (view, sql) in [("managers", JOIN), ("sums", SUMS), ("counts", COUNTS)] {
        refresh(&c, view).unwrap();
        assert_matches_oracle(&c, view, sql);
    }
}

#[test]
fn dropping_views_in_any_order_leaves_the_others_correct_and_the_last_leaves_nothing() {
    let views = [("sums", SUMS), ("counts", COUNTS), ("managers", JOIN)];
    for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        let user_objects = objects(&c);
        for (view, sql) in views {
            create(&c, view, sql).unwrap();
        }
        let mut alive: Vec<usize> = vec![0, 1, 2];
        for (step, &gone) in order.iter().enumerate() {
            c.execute_batch(&format!("DROP TABLE {}", views[gone].0)).unwrap();
            alive.retain(|&i| i != gone);
            c.execute_batch(&format!(
                "INSERT INTO orders VALUES ('a', {step}), ('z', 1); UPDATE regions SET manager = 'm{step}' WHERE name = 'b';"
            ))
            .unwrap();
            for &i in &alive {
                refresh(&c, views[i].0).unwrap();
                assert_matches_oracle(&c, views[i].0, views[i].1);
            }
        }
        assert_eq!(objects(&c), user_objects, "drop order {order:?}");
        c.execute_batch("INSERT INTO orders VALUES ('a', 1); INSERT INTO regions VALUES ('q', 'q');")
            .unwrap();
    }
}

/// Spec §4: a view created while another view's deltas are unconsumed starts
/// from the table's current sequence number; replaying the older rows would
/// count them twice (they are already in the bootstrap's scan).
#[test]
fn a_view_created_later_starts_from_the_current_sequence() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7); DELETE FROM orders WHERE region = 'b';")
        .unwrap();
    create(&c, "counts", COUNTS).unwrap();
    let seq = count(&c, "SELECT seq FROM sqlite_sequence WHERE name = '__ivm_delta_orders'");
    assert!(seq > 0);
    assert_eq!(
        count(&c, "SELECT applied_seq FROM __ivm_progress WHERE view = 'counts' AND tbl = 'orders'"),
        seq
    );
    refresh(&c, "counts").unwrap();
    assert_matches_oracle(&c, "counts", COUNTS);
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_tracked_table_whose_capture_is_broken_refuses_a_new_view() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins").unwrap();
    let err = create(&c, "counts", COUNTS).expect_err("orders is no longer captured");
    let err = err.to_string();
    assert!(err.contains("__ivm_trig_orders_ins is missing") && err.contains("sums"), "{err}");
}

#[test]
fn a_database_in_format_1_is_refused_by_create_and_by_connect() {
    let file = TempFile::new("format1");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        c.execute_batch("UPDATE __ivm_meta SET value = 1; UPDATE __ivm_view SET format = 1;")
            .unwrap();
        let err = create(&c, "counts", COUNTS).expect_err("format 1");
        assert!(err.to_string().contains("format 1"), "{err}");
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    let err = c.execute_batch("SELECT * FROM sums").expect_err("format 1");
    assert!(err.to_string().contains("format 1"), "{err}");
    c.execute_batch("DROP TABLE sums").unwrap();
}
```

In `extension_lifecycle.rs`, delete the `what_v0_cannot_maintain_is_rejected_by_name` case whose expected text is `"already tracked"`.

- [ ] **Step 3: Run the new tests to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_sharing --locked`.

Expected failures:
- the multi-view tests fail at the second `create`, with "already tracked by another ivmlite view";
- the format test fails because this build is format 1.

- [ ] **Step 4: Implement format 2 and shared capture in `view.rs`**

In `names.rs`, add the two constants from **Interfaces**, with doc comments.

In `view.rs`:

1. `pub const FORMAT: i64 = 2;`
2. In `create_global_tables`:
   - add `CREATE TABLE IF NOT EXISTS {tracked}(tbl TEXT PRIMARY KEY, shape TEXT NOT NULL);`, with `tracked = main_qualified(TRACKED)`;
   - change `__ivm_dep` to `(view TEXT NOT NULL, tbl TEXT NOT NULL, PRIMARY KEY(view, tbl))`.
3. Rename `create_delta_table` to `track`. Loop over `CAPTURE_EVENTS` to build the three triggers. At its end, insert the row:

```rust
    conn.execute(
        &format!("INSERT INTO {}(tbl, shape) VALUES (?1, ?2)", main_qualified(TRACKED)),
        params![t, shape(schema)],
    )
    .map_err(sql_error)?;
```

4. Add:

```rust
fn is_tracked(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        &format!("SELECT 1 FROM {} WHERE tbl = ?1", main_qualified(TRACKED)),
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(sql_error)
}

/// The views that read `table`, by name.
fn readers(conn: &Connection, table: &str) -> Result<Vec<String>> {
    conn.prepare(&format!(
        "SELECT view FROM {} WHERE tbl = ?1 ORDER BY view",
        main_qualified(DEPS)
    ))
    .and_then(|mut s| s.query_map([table], |r| r.get(0))?.collect())
    .map_err(sql_error)
}

/// The highest sequence number `table`'s delta table has ever handed out
/// (spec §4). Not `MAX(__ivm_seq)`: GC can empty the delta table, and
/// AUTOINCREMENT never reuses a number it handed out.
fn high_watermark(conn: &Connection, table: &str) -> Result<i64> {
    conn.query_row(
        "SELECT seq FROM \"main\".sqlite_sequence WHERE name = ?1",
        [delta_table(table)],
        |r| r.get(0),
    )
    .optional()
    .map(|seq| seq.unwrap_or(0))
    .map_err(sql_error)
}

/// `table`'s capture as every view of it relies on: the shape its triggers
/// were generated from, and every capture trigger on `table` itself.
fn check_table_capture(conn: &Connection, table: &str) -> Result<()> {
    let recorded: Option<String> = conn
        .query_row(
            &format!("SELECT shape FROM {} WHERE tbl = ?1", main_qualified(TRACKED)),
            [table],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    let recorded = recorded.ok_or_else(|| format!("table {table} is not tracked"))?;
    let now = shape(&base_schema(conn, table)?);
    if now != recorded {
        return Err(format!(
            "base table {table} changed shape since it was first tracked \
             (was ({recorded}), now ({now}))"
        ));
    }
    for event in CAPTURE_EVENTS {
        check_trigger(conn, &trigger(table, event), table)
            .map_err(|why| format!("its capture trigger {why}"))?;
    }
    Ok(())
}

/// Stop capturing `table`: triggers first, so it stays writable.
fn untrack(conn: &Connection, table: &str) -> Result<()> {
    for event in CAPTURE_EVENTS {
        exec(conn, &format!("DROP TRIGGER IF EXISTS {}", main_qualified(&trigger(table, event))))?;
    }
    exec(conn, &format!("DROP TABLE IF EXISTS {}", main_qualified(&delta_table(table))))?;
    conn.execute(
        &format!("DELETE FROM {} WHERE tbl = ?1", main_qualified(TRACKED)),
        [table],
    )
    .map_err(sql_error)?;
    Ok(())
}
```

5. In `create`, replace the "already tracked" loop and the `create_delta_table` loop with:

```rust
    for schema in &schemas {
        let t = &schema.table;
        if is_tracked(conn, t)? {
            check_table_capture(conn, t).map_err(|why| {
                format!(
                    "table {t} is tracked but its capture is broken: {why}; drop the views \
                     that read it ({}) and create this view again",
                    readers(conn, t).unwrap_or_default().join(", ")
                )
            })?;
        } else {
            track(conn, schema)?;
        }
    }
```

   - The `__ivm_dep` insert loses its `shape` column.
   - The progress insert uses `high_watermark(conn, &schema.table)?` instead of `0`.
   - Replace the bootstrap comment with one citing spec §4: the watermark and the base scan share the statement's write transaction.
   - The error message must contain `"<trigger name> is missing"` and the reader names; the test asserts both.

6. `check_capture` becomes:

```rust
fn check_capture(conn: &Connection, name: &str, view: &CompiledView) -> Result<()> {
    for table in &view.tables {
        let recorded: Option<i64> = conn
            .query_row(
                &format!("SELECT 1 FROM {} WHERE view = ?1 AND tbl = ?2", main_qualified(DEPS)),
                params![name, table],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if recorded.is_none() {
            return Err(format!("its dependency on table {table} is not recorded"));
        }
        check_table_capture(conn, table)?;
    }
    check_trigger(conn, &apply_trigger(name), &stage_table(name))
        .map_err(|why| format!("its apply trigger {why}"))
}
```

   Update its doc comment: the shape now lives in `__ivm_tracked`.

7. `destroy` becomes, keeping its doc comment and extending it with spec §4's drop steps:

```rust
pub fn destroy(conn: &Connection, name: &str) -> Result<()> {
    let tables: Vec<String> = conn
        .prepare(&format!("SELECT tbl FROM {} WHERE view = ?1", main_qualified(DEPS)))
        .and_then(|mut s| s.query_map([name], |r| r.get(0))?.collect())
        .map_err(sql_error)?;
    for table in [VIEWS, DEPS, PROGRESS] {
        let column = if table == VIEWS { "name" } else { "view" };
        conn.execute(
            &format!("DELETE FROM {} WHERE {column} = ?1", main_qualified(table)),
            [name],
        )
        .map_err(sql_error)?;
    }
    // … the existing state-table, output-table and stage-table drops, unchanged …
    for t in &tables {
        if readers(conn, t)?.is_empty() {
            untrack(conn, t)?;
        }
    }
    let left: i64 = /* the existing count(*) of __ivm_view */;
    if left == 0 {
        for table in [META, VIEWS, TRACKED, DEPS, PROGRESS] {
            exec(conn, &format!("DROP TABLE {}", main_qualified(table)))?;
        }
    }
    Ok(())
}
```

   Task 2 adds the "readers remain → GC" branch here.

- [ ] **Step 5: Run the sharing and lifecycle tests**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_sharing --test extension_lifecycle --locked`

Expected: all pass. If a lifecycle test that reads `__ivm_dep.shape`, or that expects a message naming it, now fails, update the test to the new layout; its assertion must keep the same strength. The "broken capture" test's messages ("__ivm_trig_orders_ins is missing", "changed shape", "different plan", "__ivm_apply_sums is missing") must still hold.

- [ ] **Step 6: Write the sibling-view harness mode**

In `crates/ivmlite-test/src/extension.rs`:
- add the fields `siblings_enabled: bool`, `siblings: Vec<Sibling>` and `refreshes: usize` to `SqliteExtensionEngine`, defaulting to `false`, empty and `0` in `new()`;
- add the two constructors and the `Sibling` struct;
- factor the body of `materialize` into `fn read_view(&self, name: &str) -> Result<ZSet, EngineError>`, and make `materialize` call `self.read_view(VIEW)`.

```rust
/// A view created beside the one under test (Phase 3b spec §8 scenario 1).
/// It shares the base tables' capture, refreshes at every second harness
/// refresh so it lags behind, and is checked against SQLite evaluating its
/// own SQL each time it refreshes.
struct Sibling {
    name: String,
    db: Database,
    query: ViewQuery,
}

impl SqliteExtensionEngine {
    /// Beside the view under test: a sibling with the same SQL (dropped
    /// after the second refresh, so a view is dropped while others still
    /// read its tables) and one single-table sibling per base table.
    pub fn with_siblings() -> Self {
        SqliteExtensionEngine { siblings_enabled: true, ..SqliteExtensionEngine::new() }
    }

    pub fn reopening_with_siblings() -> Self {
        SqliteExtensionEngine { siblings_enabled: true, ..SqliteExtensionEngine::reopening() }
    }

    fn create_siblings(&mut self, db: &Database, query: &ViewQuery) -> Result<(), EngineError> {
        let main_sql = view_query_to_sql(query, db);
        let pick = main_sql.bytes().map(usize::from).sum::<usize>();
        let mut siblings = vec![Sibling {
            name: "ivm_sib_same".to_string(),
            db: db.clone(),
            query: query.clone(),
        }];
        for (i, schema) in db.tables().iter().enumerate() {
            let queries = enumerate(schema);
            siblings.push(Sibling {
                name: format!("ivm_sib_{i}"),
                db: Database::new(vec![schema.clone()]),
                query: queries[(pick + i) % queries.len()].clone(),
            });
        }
        for s in &siblings {
            let sql = view_query_to_sql(&s.query, &s.db).replace('\'', "''");
            self.conn()?
                .execute_batch(&format!("CREATE VIRTUAL TABLE {} USING ivm('{sql}')", s.name))
                .map_err(err)?;
        }
        self.siblings = siblings;
        Ok(())
    }

    /// Every row of `schema`'s table as SQLite holds it now.
    fn read_table(&self, schema: &Schema) -> Result<ZSet, EngineError> { /* SELECT every column, like read_view */ }

    fn refresh_siblings(&mut self) -> Result<(), EngineError> {
        for s in &self.siblings {
            self.conn()?
                .execute_batch(&format!("INSERT INTO {0}({0}) VALUES ('refresh')", s.name))
                .map_err(err)?;
            let mut bases = BTreeMap::new();
            for schema in s.db.tables() {
                bases.insert(schema.table.clone(), self.read_table(schema)?);
            }
            let want = recompute_via_sqlite(&s.db, &s.query, &bases)?;
            let got = self.read_view(&s.name)?;
            if got != want {
                return Err(EngineError(format!(
                    "sibling view {} disagrees with the oracle\n  query: {}\n  view: {got:?}\n  oracle: {want:?}",
                    s.name,
                    view_query_to_sql(&s.query, &s.db)
                )));
            }
        }
        Ok(())
    }
}
```

`read_table` returns a `ZSet` of the table's rows, each with weight 1 per occurrence. Build it like `read_view`: `SELECT "c1", "c2", … FROM "<table>"`, using `self.columns(table)`.

Wire the mode into the `Engine` implementation:
- at the end of `create_view`: `if self.siblings_enabled { self.create_siblings(db, query)?; }`;
- in `refresh`, after the view's own refresh:

```rust
        self.refreshes += 1;
        if self.siblings_enabled && self.refreshes % 2 == 0 {
            self.refresh_siblings()?;
        }
        if self.siblings_enabled && self.refreshes == 2 {
            self.conn()?.execute_batch("DROP TABLE ivm_sib_same").map_err(err)?;
            self.siblings.retain(|s| s.name != "ivm_sib_same");
        }
```

Import `enumerate`, `recompute_via_sqlite` and `Schema` as the module needs them.

- [ ] **Step 7: Add the sibling sweeps**

In `extension_differential.rs`, extend the module doc with "and, Phase 3b, with sibling views sharing the capture", and add:

```rust
/// Phase 3b spec §8 scenario 1: sibling views share each base table's
/// capture, lag behind the view under test, and one is dropped mid-case.
#[test]
fn sibling_views_share_capture_across_the_single_table_space() {
    let db = gen_database(2);
    sweep(&db, &enumerate(&db.tables()[0]), 2, SqliteExtensionEngine::with_siblings);
}

#[test]
fn sibling_views_share_capture_across_the_join_space() {
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 10, SqliteExtensionEngine::with_siblings);
    }
}

#[test]
fn sibling_views_resume_from_persisted_state() {
    let db = gen_database(2);
    sweep(&db, &enumerate(&db.tables()[0]), 6, SqliteExtensionEngine::reopening_with_siblings);
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 50, SqliteExtensionEngine::reopening_with_siblings);
    }
}
```

Run: `cargo test -p ivmlite-test --test extension_differential --locked -- sibling`.

Expected: PASS. Record each sweep's wall time in your report. If one takes more than 60 s, double its stride, and say so in the report.

- [ ] **Step 8: Gate rows**

Add and verify one row each:

| Spec requirement | Mutation | Expected red |
|---|---|---|
| spec §4 watermark from `sqlite_sequence` | in `create`, use `0` instead of `high_watermark(...)?` | `a_view_created_later_starts_from_the_current_sequence` |
| spec §4 drop keeps the capture of a table other views read | in `destroy`, call `untrack` unconditionally | `dropping_views_in_any_order_…` and the sibling sweeps |
| spec §4 create checks a tracked table's capture | delete the `check_table_capture(...)` call in `create` | `a_tracked_table_whose_capture_is_broken_refuses_a_new_view` |
| spec §3 format 2 | `FORMAT = 1` (and the test's `UPDATE`s would then be no-ops) | `a_database_in_format_1_…` |

Run `python3 scripts/count-mutation-gates.py`: it must print `consistent`.

- [ ] **Step 9: Full check and commit**

Run the Global Constraints checks. Then commit:

```bash
git add crates/ivmlite-sqlite/src/names.rs crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/src/extension.rs crates/ivmlite-test/tests/common/mod.rs crates/ivmlite-test/tests/extension_lifecycle.rs crates/ivmlite-test/tests/extension_sharing.rs crates/ivmlite-test/tests/extension_differential.rs docs/mutation-gates.md
git commit -m "feat(sqlite): share a base table's capture among views (format 2)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Delta GC

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs`
- Create: `crates/ivmlite-test/tests/extension_gc.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: Task 1's `readers`, `high_watermark`, the `destroy` structure, and `tests/common` (`count`, `durable_state`, `TempFile`, …).
- Produces: `fn collect_garbage(conn: &Connection, table: &str) -> Result<()>` in `view.rs`.

- [ ] **Step 1: Write the failing GC tests**

Create `crates/ivmlite-test/tests/extension_gc.rs`:

```rust
//! M1b Phase 3b spec §5 and §8 scenario 3: delta rows every reader has
//! consumed are deleted, atomically with the watermark that consumed them.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rusqlite::Connection;

const COUNTS: &str = "SELECT region, COUNT(*) FROM orders GROUP BY region";

fn deltas(c: &Connection) -> i64 {
    count(c, "SELECT count(*) FROM __ivm_delta_orders")
}

fn watermark(c: &Connection, view: &str) -> i64 {
    count(c, &format!("SELECT applied_seq FROM __ivm_progress WHERE view = '{view}' AND tbl = 'orders'"))
}

#[test]
fn deltas_stay_until_every_reader_has_consumed_them() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "counts", COUNTS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7); UPDATE orders SET amount = 3 WHERE amount = 1;")
        .unwrap();
    let written = deltas(&c);
    assert_eq!(written, 4);
    refresh(&c, "sums").unwrap();
    // counts has not read them yet.
    assert_eq!(deltas(&c), written);
    refresh(&c, "counts").unwrap();
    assert_eq!(deltas(&c), 0);
    // A lagging reader keeps exactly the rows after its watermark.
    c.execute_batch("INSERT INTO orders VALUES ('d', 1)").unwrap();
    refresh(&c, "counts").unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('e', 2), ('f', 3)").unwrap();
    refresh(&c, "counts").unwrap();
    let lag = watermark(&c, "sums");
    assert_eq!(count(&c, &format!("SELECT count(*) FROM __ivm_delta_orders WHERE __ivm_seq <= {lag}")), 0);
    assert_eq!(deltas(&c), 3);
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 0);
    assert_matches_oracle(&c, "sums", SUMS);
    assert_matches_oracle(&c, "counts", COUNTS);
}

#[test]
fn dropping_the_slowest_reader_collects_what_only_it_held() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "counts", COUNTS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7)").unwrap();
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 2);
    c.execute_batch("DROP TABLE counts").unwrap();
    assert_eq!(deltas(&c), 0);
    c.execute_batch("INSERT INTO orders VALUES ('a', 1)").unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Spec §4: once GC has emptied the delta table, `MAX(__ivm_seq)` is NULL,
/// but a new view must still start from the sequence AUTOINCREMENT will
/// continue from.
#[test]
fn a_view_created_over_an_emptied_delta_table_starts_from_sqlite_sequence() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7)").unwrap();
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 0);
    let seq = count(&c, "SELECT seq FROM sqlite_sequence WHERE name = '__ivm_delta_orders'");
    assert_eq!(seq, 2);
    create(&c, "counts", COUNTS).unwrap();
    assert_eq!(watermark(&c, "counts"), seq);
    c.execute_batch("INSERT INTO orders VALUES ('c', 1)").unwrap();
    refresh(&c, "counts").unwrap();
    assert_matches_oracle(&c, "counts", COUNTS);
}

/// Spec §5: GC runs inside the one arming statement, so a failed deletion
/// rolls back state, output, watermarks and the delta table together.
#[test]
fn a_failed_gc_changes_nothing_and_a_retry_succeeds() {
    for explicit_transaction in [false, true] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        create(&c, "counts", COUNTS).unwrap();
        c.execute_batch(
            "CREATE TRIGGER fault BEFORE DELETE ON __ivm_delta_orders \
             BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        )
        .unwrap();
        if explicit_transaction {
            c.execute_batch("BEGIN").unwrap();
        }
        c.execute_batch("INSERT INTO orders VALUES ('c', 3), ('a', 9)").unwrap();
        // sums goes first: counts still holds the rows, so nothing is deleted.
        refresh(&c, "sums").unwrap();
        let before = (durable_state(&c, "counts"), deltas(&c));
        let err = refresh(&c, "counts").expect_err("GC deletes, and the fault aborts it");
        assert!(err.to_string().contains("injected fault"), "{err}");
        assert_eq!((durable_state(&c, "counts"), deltas(&c)), before, "transaction: {explicit_transaction}");
        if explicit_transaction {
            assert!(!c.is_autocommit(), "the transaction must still be open");
        }
        c.execute_batch("DROP TRIGGER fault").unwrap();
        refresh(&c, "counts").unwrap();
        if explicit_transaction {
            c.execute_batch("COMMIT").unwrap();
        }
        assert_eq!(deltas(&c), 0);
        assert_matches_oracle(&c, "counts", COUNTS);
        assert_matches_oracle(&c, "sums", SUMS);
    }
}
```

`durable_state(c, view)` already includes every row of `__ivm_progress`, so it covers the watermarks too.

- [ ] **Step 2: Run to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_gc --locked`

Expected: every `deltas(&c), 0` assertion fails (nothing is ever deleted). The fault test fails because `refresh(counts)` succeeds.

- [ ] **Step 3: Implement GC**

In `create_stage`, directly after the `progress` `UPDATE` pushed onto `body`, add:

```rust
    // Spec §5: once this view's watermark for `t` moves, delete every delta
    // row of `t` that every reader has consumed. Only on the stage row that
    // moves `t`'s watermark, inside the one arming statement.
    for t in &view.tables {
        body.push(format!(
            "DELETE FROM {delta} WHERE NEW.op = 'progress' AND NEW.tbl = {lit} \
             AND {DELTA_SEQ} <= (SELECT MIN(applied_seq) FROM {progress} WHERE tbl = {lit});",
            delta = quote(&delta_table(t)),
            lit = literal(t),
        ));
    }
```

Add:

```rust
/// Delete `table`'s delta rows that every reader has consumed (spec §5).
/// Used when a view is dropped while others still read `table`: the dropped
/// view may have been the slowest.
fn collect_garbage(conn: &Connection, table: &str) -> Result<()> {
    conn.execute(
        &format!(
            "DELETE FROM {} WHERE {DELTA_SEQ} <= (SELECT MIN(applied_seq) FROM {} WHERE tbl = ?1)",
            main_qualified(&delta_table(table)),
            main_qualified(PROGRESS)
        ),
        [table],
    )
    .map(|_| ())
    .map_err(sql_error)
}
```

In `destroy`, change the per-table loop to `if readers(conn, t)?.is_empty() { untrack(conn, t)?; } else { collect_garbage(conn, t)?; }`.

- [ ] **Step 4: Run to verify they pass**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_gc --test extension_sharing --test extension_differential --locked`

Expected: PASS.

- [ ] **Step 5: Gate rows**

Add and verify:

| Spec requirement | Mutation | Expected red |
|---|---|---|
| spec §5 GC after the watermark moves | delete the GC `body.push` loop | `deltas_stay_until_…`, `a_failed_gc_…`, `a_view_created_over_an_emptied_…` |
| spec §5 GC follows the slowest reader | `MIN(applied_seq)` → `MAX(applied_seq)` in the trigger body | `deltas_stay_until_…` (sums lagging loses its rows) and the sibling sweeps |
| spec §4 drop collects what the dropped view held | delete the `else { collect_garbage(...) }` branch | `dropping_the_slowest_reader_…` |
| spec §4 watermark is `sqlite_sequence`, not `MAX` | `high_watermark` → `SELECT COALESCE(MAX(__ivm_seq), 0) FROM <delta>` | `a_view_created_over_an_emptied_…` |

- [ ] **Step 6: Full check and commit**

```bash
git add crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/tests/extension_gc.rs docs/mutation-gates.md
git commit -m "feat(sqlite): collect consumed deltas in the apply and at drop

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Unique-key metadata, REPLACE schema rules, the extended shape

**Files:**
- Modify: `crates/ivmlite-sqlite/src/catalog.rs`, `crates/ivmlite-sqlite/src/view.rs`
- Create: `crates/ivmlite-test/tests/extension_replace.rs`
- Modify: `crates/ivmlite-test/tests/extension_lifecycle.rs`, `docs/mutation-gates.md`

**Interfaces:**
- Produces (in `catalog.rs`, `pub`):

```rust
/// One key column of a unique index, as `pragma_index_xinfo` and
/// `pragma_table_info` report it.
pub struct KeyColumn {
    pub name: String,
    pub collation: String,
    pub not_null: bool,
    /// `pragma_table_info.dflt_value`: the default's SQL text, if any.
    pub default: Option<String>,
}

/// A unique index: every `pragma_index_list` row with `unique = 1`
/// (origin `pk`, `u` or `c`).
pub struct UniqueKey {
    pub index: String,
    pub primary: bool,
    pub partial: bool,
    /// Some key is an expression (`pragma_index_xinfo.cid = -2`).
    pub expression: bool,
    /// The key columns in index order; an expression key is omitted.
    pub columns: Vec<KeyColumn>,
}

/// What the capture triggers depend on beyond the columns (spec §3, §6).
pub struct CaptureInfo {
    pub without_rowid: bool,
    pub unique_keys: Vec<UniqueKey>,
}

impl SqliteCatalog<'_> {
    /// `table`'s capture metadata; `table` is the declared name.
    pub fn capture(&self, table: &str) -> Result<CaptureInfo, String>;
}

/// Spec §6.1's literal default.
pub fn is_literal_default(text: &str) -> bool;
```

- Produces (in `view.rs`): `fn shape(schema: &Schema, capture: &CaptureInfo) -> String`. Every caller now has a `CaptureInfo`, from `SqliteCatalog { conn }.capture(&schema.table)?`.

- [ ] **Step 1: Write the failing tests**

(a) Unit tests at the end of `catalog.rs`'s `mod tests`:

```rust
    #[test]
    fn literal_defaults_are_numbers_strings_and_null() {
        for yes in ["NULL", "null", "5", "-5", "+3", "0x10", "-0X1f", "'d'", "'x''y'", "''"] {
            assert!(is_literal_default(yes), "{yes}");
        }
        for no in ["CURRENT_TIMESTAMP", "random()", "5.0", "1e3", "'a' || 'b'", "'unterminated", "x", "0x", "-", "(5)"] {
            assert!(!is_literal_default(no), "{no}");
        }
    }
```

(`(5)` is reported by SQLite as `5`, so a parenthesised text never reaches this check. Refusing it is the conservative answer.)

(b) Rejection cases. In `extension_lifecycle.rs`, add to the `cases` array of `what_v0_cannot_maintain_is_rejected_by_name`:

```rust
        // Phase 3b spec §6.1: what REPLACE capture cannot look up.
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX uk ON t(k COLLATE NOCASE)", q, "collation NOCASE"),
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX up ON t(v) WHERE v > 0", q, "partial"),
        ("CREATE TABLE t(k TEXT, v INTEGER) STRICT; CREATE UNIQUE INDEX ue ON t(v + 1)", q, "expression"),
        ("CREATE TABLE t(k TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP UNIQUE, v INTEGER) STRICT", q, "default CURRENT_TIMESTAMP"),
        ("CREATE TABLE t(k TEXT, RowId INTEGER) STRICT", q, "RowId"),
```

(c) Create `crates/ivmlite-test/tests/extension_replace.rs`:

```rust
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
        ("CREATE UNIQUE INDEX uk ON t(k COLLATE NOCASE)", "collation NOCASE"),
    ] {
        let file = TempFile::new("unique-added");
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE t(k TEXT, v INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);")
            .unwrap();
        create(&c, "ks", KS).unwrap();
        c.execute_batch(index).unwrap();
        let err = refresh(&c, "ks").expect_err(index);
        assert!(err.to_string().contains(expected) || err.to_string().contains("changed shape"), "{index}: {err}");
        drop(c);
        let c = open_with_extension(Some(file.path())).unwrap();
        let err = c.execute_batch("SELECT * FROM ks").expect_err(index);
        assert!(err.to_string().contains(expected), "{index} / reopen: {err}");
        c.execute_batch("DROP TABLE ks").unwrap();
    }
}
```

The same-connection refresh may report either message: SQLite does not always reconnect the virtual table after a schema change (Phase 3a gate-table note). After a reopen, the connect path recompiles, so the catalog's message is the one expected there.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked` — this fails to compile, since `is_literal_default` does not exist yet.

Then run `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_replace --test extension_lifecycle --locked`. The five new rejection cases fail (the view is created), and `a_unique_index_added_after_create_breaks_the_view` fails (the refresh succeeds).

- [ ] **Step 3: Implement `capture`, the rules and `is_literal_default`**

In `catalog.rs`, implement `SqliteCatalog::capture`:
- `without_rowid`: from `SELECT wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1`.
- For each `SELECT name, origin, partial FROM pragma_index_list(?1, 'main') WHERE "unique" = 1 ORDER BY name`:
  - read `SELECT cid, name, coll FROM pragma_index_xinfo(?1, 'main') WHERE key = 1 ORDER BY seqno`, passing the index name. The schema argument is required, as for `pragma_table_info` (Phase 3a §4).
  - `cid = -2` sets `expression = true` and adds no column.
  - otherwise add a `KeyColumn`, whose `not_null` and `default` come from `SELECT "notnull", dflt_value FROM pragma_table_info(?1, 'main') WHERE name = ?2`.
  - set `primary = origin == "pk"`.

Implement `is_literal_default` per spec §6.1, with a doc comment quoting its rule:

```rust
pub fn is_literal_default(text: &str) -> bool {
    if text.eq_ignore_ascii_case("NULL") {
        return true;
    }
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        // Every quote inside must be a doubled one.
        return !inner.replace("''", "").contains('\'');
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if let Some(hex) = unsigned.strip_prefix("0x").or_else(|| unsigned.strip_prefix("0X")) {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    !unsigned.is_empty() && unsigned.bytes().all(|b| b.is_ascii_digit())
}
```

In `SqliteCatalog::table`, after the column loop:

```rust
        const ROWID_ALIASES: [&str; 3] = ["rowid", "oid", "_rowid_"];
        if let Some(c) = columns.iter().find(|c| ROWID_ALIASES.iter().any(|a| c.name.eq_ignore_ascii_case(a))) {
            return refuse(&format!(
                "column {} is named like the rowid, which ivmlite's capture triggers address rows by (Phase 3b spec §6.1)",
                c.name
            ));
        }
        let capture = self.capture(&declared).map_err(CatalogError)?;
        for key in &capture.unique_keys {
            let index = &key.index;
            if key.partial {
                return refuse(&format!("unique index {index} is partial; ivmlite v0 needs full unique indexes to capture REPLACE (Phase 3b spec §6.1)"));
            }
            if key.expression {
                return refuse(&format!("unique index {index} has an expression key; ivmlite cannot look up what REPLACE would remove (Phase 3b spec §6.1)"));
            }
            for column in &key.columns {
                if !column.collation.eq_ignore_ascii_case("BINARY") {
                    return refuse(&format!(
                        "unique index {index} uses collation {} on column {}; v0 supports only BINARY (Phase 3b spec §6.1)",
                        column.collation, column.name
                    ));
                }
                if let (true, Some(default)) = (column.not_null, &column.default) {
                    if !is_literal_default(default) {
                        return refuse(&format!(
                            "column {} is NOT NULL in unique index {index} with default {default}, which is not a literal; \
                             REPLACE substitutes the default, and ivmlite cannot re-evaluate it (Phase 3b spec §6.1)",
                            column.name
                        ));
                    }
                }
            }
        }
```

Each message contains the asserted text: `partial`, `expression`, `collation NOCASE`, `default CURRENT_TIMESTAMP`, and the column's declared spelling (e.g. `RowId`).

- [ ] **Step 4: Extend the shape**

In `view.rs`:

```rust
/// Everything the capture triggers depend on (Phase 3b spec §3): the
/// columns' names and types in order, whether the table is WITHOUT ROWID,
/// and every unique index — its key columns with collation, `NOT NULL` and
/// default, and its partial and expression flags — in a canonical order,
/// so an index's name never matters.
fn shape(schema: &Schema, capture: &CaptureInfo) -> String {
    let columns: Vec<String> = schema.columns.iter().map(|c| format!("{} {}", quote(&c.name), sql_type(c))).collect();
    let mut keys: Vec<String> = capture
        .unique_keys
        .iter()
        .map(|k| {
            let cols: Vec<String> = k
                .columns
                .iter()
                .map(|c| {
                    let mut text = format!("{} {}", quote(&c.name), c.collation);
                    if c.not_null {
                        text.push_str(" NOT NULL");
                    }
                    if let Some(d) = &c.default {
                        text.push_str(&format!(" DEFAULT {d}"));
                    }
                    text
                })
                .collect();
            format!(
                "({}){}{}",
                cols.join(", "),
                if k.partial { " partial" } else { "" },
                if k.expression { " expression" } else { "" }
            )
        })
        .collect();
    keys.sort();
    format!(
        "{}; without rowid: {}; unique: [{}]",
        columns.join(", "),
        capture.without_rowid,
        keys.join("; ")
    )
}
```

Update both callers (`track` and `check_table_capture`) to pass `&SqliteCatalog { conn }.capture(&schema.table)?`.

- [ ] **Step 5: Run to verify they pass**

Run: `scripts/test-all.sh`. Expected: all pass.

- [ ] **Step 6: Gate rows**

One row each, verified:
- the partial refusal (`if key.partial` → `if false && key.partial`);
- the expression refusal;
- the collation refusal;
- the non-literal-default refusal;
- the rowid-named column refusal;
- `shape` includes the unique keys (replace `keys.join("; ")` with `""`) → `a_unique_index_added_after_create_breaks_the_view`;
- `is_literal_default` rejects a lone quote (make the string branch `return true;`) → the unit test.

- [ ] **Step 7: Full check and commit**

```bash
git add crates/ivmlite-sqlite/src/catalog.rs crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/tests/extension_replace.rs crates/ivmlite-test/tests/extension_lifecycle.rs docs/mutation-gates.md
git commit -m "feat(sqlite): read unique-key metadata, refuse what REPLACE capture cannot look up

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: REPLACE capture triggers

**Files:**
- Modify: `crates/ivmlite-sqlite/src/names.rs`, `crates/ivmlite-sqlite/src/view.rs`, `crates/ivmlite-sqlite/src/catalog.rs`
- Modify: `crates/ivmlite-test/tests/extension_replace.rs`, `crates/ivmlite-test/tests/extension_lifecycle.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: Task 3's `CaptureInfo`/`UniqueKey`/`KeyColumn`, and Task 1's `track`, `untrack`, `check_table_capture` and `CAPTURE_EVENTS`.
- Produces (in `names.rs`):
  - `pub fn pend_table(table: &str) -> String` → `__ivm_pend_<table>`;
  - `pub const PROBE: &str = "__ivm_probe";`
  - `pub const PROBE_STEP: &str = "__ivm_probe_step";`
  - `CAPTURE_EVENTS` becomes `["ins", "del", "upd", "preins", "preupd"]`, with a doc comment explaining that the events contain no `_`.

- [ ] **Step 1: Write the failing tests**

Append to `extension_replace.rs`:

```rust
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::Connection;

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
    match r.gen_range(0..10) {
        0 | 1 => "NULL".to_string(),
        // Text into an INTEGER column: STRICT converts it.
        2 => format!("'{}'", r.gen_range(0..10)),
        _ => r.gen_range(0..10).to_string(),
    }
}

fn text(r: &mut StdRng, pool: &[&str]) -> String {
    if r.gen_bool(0.2) {
        "NULL".to_string()
    } else {
        format!("'{}'", pool[r.gen_range(0..pool.len())])
    }
}

fn value(r: &mut StdRng, column: &str) -> String {
    match column {
        "id" => if r.gen_bool(0.5) { "NULL".to_string() } else { r.gen_range(1..13).to_string() },
        "k" => text(r, &["d", "p", "q"]),
        "u" => text(r, &["p", "q", "r", "s", "t"]),
        "x" if r.gen_bool(0.3) => ["3", "5", "7"][r.gen_range(0..3)].to_string(),
        _ => int(r),
    }
}

fn row(r: &mut StdRng, cols: &[&str]) -> String {
    let values: Vec<String> = cols.iter().map(|c| value(r, c)).collect();
    format!("({})", values.join(", "))
}

/// One of spec §2's 14 statement kinds.
fn statement(r: &mut StdRng, cols: &[&str]) -> String {
    let col = cols[r.gen_range(0..cols.len())];
    let val = value(r, col);
    let wh = ["x IS NULL", "x > 4", "a = b", "k = 'p'", "u IS NOT NULL", "1"][r.gen_range(0..6)];
    let plain: Vec<&str> = cols.iter().copied().filter(|c| *c != "id").collect();
    let swapped: Vec<&str> = plain.iter().map(|c| match *c { "a" => "b", "b" => "a", c => c }).collect();
    let (r1, r2) = (row(r, cols), row(r, cols));
    match r.gen_range(0..14) {
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
        10 => format!("INSERT OR REPLACE INTO t({}) SELECT {} FROM t WHERE {wh}", plain.join(", "), swapped.join(", ")),
        11 => format!("UPDATE OR REPLACE t SET a = b, b = a, x = x + 1 WHERE {wh}"),
        // k omitted: its default 'd' may conflict.
        12 => format!("INSERT OR REPLACE INTO t(u, a) VALUES ({}, {})", value(r, "u"), int(r)),
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
fn differential(variant: &Variant, reentry: &str, recursive: Recursive, seeds: std::ops::Range<u64>) {
    let group = variant.cols.join(", ");
    let everything = format!("SELECT {group}, COUNT(*) FROM t GROUP BY {group}");
    for seed in seeds {
        let file = TempFile::new(&format!("replace-{seed}"));
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch(variant.ddl).unwrap();
        c.execute_batch(reentry).unwrap();
        create(&c, "everything", &everything).unwrap();
        let plain = Connection::open(file.path()).unwrap();
        let mut r = StdRng::seed_from_u64(seed);
        for step in 0..40 {
            let writer = if r.gen_bool(0.25) { &plain } else { &c };
            let on = match recursive {
                Recursive::RandomPerStatement => r.gen_bool(0.5),
                Recursive::AlwaysOn => true,
            };
            writer
                .execute_batch(&format!("PRAGMA recursive_triggers = {}", if on { "ON" } else { "OFF" }))
                .unwrap();
            let sql = statement(&mut r, variant.cols);
            // Constraint failures are part of the space; they change nothing.
            let _ = writer.execute_batch(&sql);
            if r.gen_bool(0.3) || step == 39 {
                refresh(&c, "everything").unwrap_or_else(|e| panic!("{} seed {seed} step {step}: {e}", variant.name));
                assert_eq!(
                    rows(&c, "SELECT * FROM everything"),
                    rows(&c, &everything),
                    "{} seed {seed} step {step} after: {sql}",
                    variant.name
                );
            }
        }
    }
}

#[test]
fn replace_is_captured_without_reentry_whatever_the_writers_recursive_triggers() {
    for v in &VARIANTS {
        differential(v, "", Recursive::RandomPerStatement, 0..60);
    }
}

/// Spec §6.4: with recursive_triggers ON, capture stays exact even when
/// user triggers write the same table again.
#[test]
fn replace_is_captured_under_reentry_when_the_writer_has_recursive_triggers_on() {
    for v in &VARIANTS {
        for reentry in [REENTRY_CONFLICTING, REENTRY_NON_CONFLICTING] {
            differential(v, reentry, Recursive::AlwaysOn, 0..30);
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
```

Then:
- In `extension_sharing.rs`, set `CAPTURE_TRIGGERS` to `5`.
- In `extension_lifecycle.rs`, delete the three `"ON CONFLICT REPLACE"` rejection cases. They now live in `a_table_declaring_on_conflict_replace_is_maintained`.
- In `temp_tables_named_like_the_shadow_tables_are_never_touched`, add `__ivm_tracked`, `__ivm_probe` and `__ivm_pend_orders` (or the test's own base table) to the TEMP tables it creates and checks.

- [ ] **Step 2: Run to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --test extension_replace --locked`

Expected:
- `replace_is_captured_without_reentry_…` and `replace_conflicts_are_captured_without_recursive_triggers` fail, the view keeping removed rows;
- `a_table_declaring_on_conflict_replace_is_maintained` fails at `create`, with "declares ON CONFLICT REPLACE";
- the always-ON re-entry test passes already. That is spec §2's first row, the Phase 3a baseline.

- [ ] **Step 3: Implement**

`names.rs`: add `pend_table`, `PROBE` and `PROBE_STEP`, and grow `CAPTURE_EVENTS`, as in **Interfaces**.

`catalog.rs`: delete `declares_on_conflict_replace`, its call in `table` and its unit test. `tokens` stays; `declares_collate` still uses it.

`view.rs`, `create_global_tables`: add

```sql
CREATE TABLE IF NOT EXISTS {probe}(n INTEGER NOT NULL);
CREATE TRIGGER IF NOT EXISTS {probe_step} AFTER INSERT ON {probe_on} WHEN NEW.n < 2
BEGIN INSERT INTO {probe_body}(n) VALUES (NEW.n + 1); END;
```

with `probe = main_qualified(PROBE)`, `probe_step = main_qualified(PROBE_STEP)`, and `probe_on = probe_body = quote(PROBE)`. Add a comment citing spec §6.2: with `recursive_triggers` OFF, inserting a 0 leaves 2 rows; with it ON, 3. `destroy`'s final global drop gains `PROBE` (its trigger goes with it).

`view.rs`, `track`:
- create `__ivm_pend_<t>(__ivm_rid INTEGER, <same column defs as the delta table>)`;
- create the five triggers from spec §6.2, generated by the helpers below;
- `untrack` drops the pend table too.

```rust
/// How the capture triggers name a row of `t` (spec §6.2): its rowid, or
/// for a WITHOUT ROWID table its primary-key columns.
fn identity(capture: &CaptureInfo) -> Option<Vec<String>> {
    capture.without_rowid.then(|| {
        capture
            .unique_keys
            .iter()
            .find(|k| k.primary)
            .expect("a WITHOUT ROWID table has a primary key")
            .columns
            .iter()
            .map(|c| quote(&c.name))
            .collect()
    })
}

/// `a = b AND …` over the identity: `rowid`, or the primary-key columns.
fn same_row(pk: &Option<Vec<String>>, left: &str, right: &str) -> String {
    match pk {
        None => format!("{left}rowid = {right}rowid"),
        Some(cols) => cols.iter().map(|c| format!("{left}{c} = {right}{c}")).collect::<Vec<_>>().join(" AND "),
    }
}

/// The existing rows a new row could replace (spec §6.2), one `UNION`
/// branch per unique index plus the rowid, each able to use its own index.
/// `exclude_old`: in BEFORE UPDATE, never the row being updated.
fn candidates(schema: &Schema, capture: &CaptureInfo, exclude_old: bool) -> String {
    let base = quote(&schema.table);
    let cols = column_list(schema, "");
    let pk = identity(capture);
    let rid = if pk.is_some() { "NULL" } else { "rowid" };
    let mut branches = Vec::new();
    if pk.is_none() {
        branches.push("rowid = NEW.rowid".to_string());
    }
    for key in &capture.unique_keys {
        let terms: Vec<String> = key
            .columns
            .iter()
            .map(|c| {
                let q = quote(&c.name);
                match (&c.default, c.not_null) {
                    (Some(d), true) if !d.eq_ignore_ascii_case("NULL") => format!("{q} = COALESCE(NEW.{q}, {d})"),
                    _ => format!("{q} = NEW.{q}"),
                }
            })
            .collect();
        branches.push(terms.join(" AND "));
    }
    let exclude = match (&pk, exclude_old) {
        (_, false) => String::new(),
        (None, true) => " AND rowid <> OLD.rowid".to_string(),
        (Some(_), true) => format!(" AND NOT ({})", same_row(&pk, "", "OLD.")),
    };
    branches
        .iter()
        .map(|b| format!("SELECT {rid}, {cols} FROM {base} WHERE ({b}){exclude}"))
        .collect::<Vec<_>>()
        .join(" UNION ")
}
```

`d` is the `dflt_value` text that Task 3's catalog accepted as a literal, so it is safe to splice into SQL.

The trigger bodies. `delta`, `pend`, `probe` and `base` are the unqualified quoted names; `cols`, `new` and `old` are `column_list` with the prefixes `""`, `"NEW."` and `"OLD."`:

```rust
    let fill = |exclude_old: bool| {
        let c = candidates(schema, capture, exclude_old);
        format!(
            "DELETE FROM {pend};
             INSERT INTO {probe}(n) SELECT 0 WHERE EXISTS ({c});
             INSERT INTO {pend}(__ivm_rid, {cols}) SELECT * FROM ({c}) WHERE (SELECT count(*) FROM {probe}) = 2;
             DELETE FROM {probe};"
        )
    };
    let gone = match &pk {
        None => format!("p.__ivm_rid = NEW.rowid OR NOT EXISTS (SELECT 1 FROM {base} WHERE rowid = p.__ivm_rid)"),
        Some(_) => format!(
            "({}) OR NOT EXISTS (SELECT 1 FROM {base} WHERE {})",
            same_row(&pk, "p.", "NEW."),
            same_row(&pk, "", "p.")
        ),
    };
    let confirm = format!(
        "INSERT INTO {delta}({DELTA_W}, {cols}) SELECT -1, {p_cols} FROM {pend} AS p WHERE {gone};
         DELETE FROM {pend};"
    );
    let forget = match &pk {
        None => format!("DELETE FROM {pend} WHERE __ivm_rid = OLD.rowid;"),
        Some(_) => format!("DELETE FROM {pend} WHERE {};", same_row(&pk, "", "OLD.")),
    };
```

`p_cols` is `column_list(schema, "p.")`.

| trigger | event | body |
|---|---|---|
| `preins` | BEFORE INSERT | `fill(false)` |
| `preupd` | BEFORE UPDATE | `fill(true)` |
| `ins` | AFTER INSERT | `confirm`, then the +1 NEW insert |
| `upd` | AFTER UPDATE | `confirm`, then the −1 OLD and +1 NEW inserts |
| `del` | AFTER DELETE | `forget`, then the −1 OLD insert |

Keep Phase 3a's comments on qualification, and add one per trigger citing spec §6.2.

`check_table_capture`: loop over `CAPTURE_EVENTS` (now five). Also check:
- `check_trigger(conn, PROBE_STEP, PROBE)`, with the error prefixed `"its recursive-triggers probe "`;
- `table_exists(conn, &pend_table(table))?`, erroring with `"its shadow table {pend} is missing"`.

- [ ] **Step 4: Run to verify they pass**

Run: `scripts/test-all.sh`. Expected: all pass, including every Task 1–3 test. Report the wall time of the two differential tests; if either takes more than 60 s, halve its seed range and say so.

- [ ] **Step 5: Gate rows**

One row each, verified. Every mutation below must turn a test red. If one does not, report it, and do not weaken the row.

| Spec requirement | Mutation | Expected red |
|---|---|---|
| §6.2 candidates are confirmed only when gone | `WHERE {gone}` → `WHERE 1` | the no-re-entry differential |
| §6.2 a replaced row's −1 is recorded at all | delete the `INSERT … SELECT -1 … FROM pend` statement | both differentials and `replace_conflicts_are_captured_without_recursive_triggers` |
| §6.2 AFTER DELETE forgets its candidate | delete `forget` | the no-re-entry differential (writers with ON) |
| §6.2 the probe gates the candidates | `= 2` → `>= 2` | the always-ON re-entry differential |
| §6.2 NOT NULL defaults are substituted | drop the `COALESCE` arm | the no-re-entry differential |
| §6.2 rowid replacement (`p.__ivm_rid = NEW.rowid`) | delete that disjunct | the no-re-entry differential |
| §6.3 the probe trigger is checked | delete the probe `check_trigger` | a new small test in `extension_replace.rs`, `a_missing_probe_trigger_breaks_the_view` (`DROP TRIGGER __ivm_probe_step`; refresh fails with `"__ivm_probe_step is missing"`; the view still drops) — add it |

- [ ] **Step 6: Full check and commit**

```bash
git add crates/ivmlite-sqlite/src/names.rs crates/ivmlite-sqlite/src/view.rs crates/ivmlite-sqlite/src/catalog.rs crates/ivmlite-test/tests/extension_replace.rs crates/ivmlite-test/tests/extension_lifecycle.rs docs/mutation-gates.md
git commit -m "feat(sqlite): capture REPLACE deletions without recursive_triggers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Refuse renaming a view

**Files:**
- Modify: `crates/ivmlite-sqlite/src/lib.rs`, `crates/ivmlite-sqlite/src/vtab.rs`
- Modify: `crates/ivmlite-test/tests/extension_lifecycle.rs`, `docs/mutation-gates.md`

**Interfaces:**
- Produces: `pub unsafe extern "C" fn refuse_rename(vtab: *mut ffi::sqlite3_vtab, new_name: *const c_char) -> c_int` in `vtab.rs`, and `static IVM_MODULE: ffi::sqlite3_module` in `lib.rs`.

- [ ] **Step 1: Write the failing test**

Add to `extension_lifecycle.rs`:

```rust
/// Phase 3b spec §7: renaming a view fails, and leaves it working and droppable.
#[test]
fn a_view_cannot_be_renamed() {
    for explicit_transaction in [false, true] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        let user_objects = objects(&c);
        create(&c, "sums", SUMS).unwrap();
        if explicit_transaction {
            c.execute_batch("BEGIN").unwrap();
        }
        let err = c.execute_batch("ALTER TABLE sums RENAME TO totals").expect_err("rename");
        assert!(err.to_string().contains("ivmlite views cannot be renamed"), "{err}");
        if explicit_transaction {
            c.execute_batch("COMMIT").unwrap();
        }
        assert_eq!(count(&c, "SELECT count(*) FROM sqlite_schema WHERE name = 'totals'"), 0);
        c.execute_batch("INSERT INTO orders VALUES ('a', 5)").unwrap();
        refresh(&c, "sums").unwrap();
        assert_matches_oracle(&c, "sums", SUMS);
        c.execute_batch("DROP TABLE sums").unwrap();
        assert_eq!(objects(&c), user_objects);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ivmlite-test --test extension_lifecycle --locked -- a_view_cannot_be_renamed`. Expected: FAIL, because the rename succeeds.

- [ ] **Step 3: Implement**

In `vtab.rs`:

```rust
/// `xRename` (Phase 3b spec §7): an ivmlite view cannot be renamed — its
/// shadow tables, triggers and metadata all carry its name — so the rename
/// is refused and SQLite leaves the schema unchanged. Nothing here can
/// panic (no allocation through Rust, no indexing), so it needs no `guard`.
///
/// # Safety
/// Called by SQLite with the view's live `sqlite3_vtab`.
pub unsafe extern "C" fn refuse_rename(vtab: *mut ffi::sqlite3_vtab, _new_name: *const c_char) -> c_int {
    const MESSAGE: &[u8] = b"ivmlite views cannot be renamed; drop the view and create it again under the new name\0";
    // SAFETY: `vtab` is live for this call. SQLite frees `zErrMsg` with
    // `sqlite3_free`, so it must come from SQLite's allocator; an earlier
    // message is freed first.
    unsafe {
        if !(*vtab).zErrMsg.is_null() {
            ffi::sqlite3_free((*vtab).zErrMsg.cast());
        }
        let buf = ffi::sqlite3_malloc64(MESSAGE.len() as u64).cast::<c_char>();
        (*vtab).zErrMsg = buf;
        if buf.is_null() {
            return ffi::SQLITE_NOMEM;
        }
        std::ptr::copy_nonoverlapping(MESSAGE.as_ptr().cast::<c_char>(), buf, MESSAGE.len());
    }
    ffi::SQLITE_ERROR
}
```

In `lib.rs`, replace the `const IVM` and `create_module` call:

```rust
/// The `ivm` module (Phase 3b spec §7): rusqlite's writable-table module
/// plus an `xRename`, which rusqlite 0.40 does not expose. A `static`,
/// because SQLite keeps the module pointer for as long as the module is
/// registered.
static IVM_MODULE: ffi::sqlite3_module = {
    const BASE: Module<'static, vtab::IvmTab> = Module::update_module();
    // SAFETY: rusqlite 0.40 (pinned by this crate's Cargo.lock) declares
    // `Module` `#[repr(transparent)]` over `ffi::sqlite3_module`, so the two
    // have the same layout; `transmute` checks their sizes at compile time.
    let mut module: ffi::sqlite3_module = unsafe { std::mem::transmute(BASE) };
    module.xRename = Some(vtab::refuse_rename);
    module
};
```

In the `extension_init2` closure:

```rust
            // SAFETY: `conn` wraps the handle SQLite is initializing; the
            // module is a `static`, and no client data is passed.
            let rc = unsafe {
                ffi::sqlite3_create_module_v2(conn.handle(), c"ivm".as_ptr(), &IVM_MODULE, std::ptr::null_mut(), None)
            };
            if rc != ffi::SQLITE_OK {
                return Err(rusqlite::Error::SqliteFailure(
                    ffi::Error::new(rc),
                    Some("registering the ivm module".to_string()),
                ));
            }
            Ok(false)
```

Keep the `catch_unwind` around the registration, as today. Update the module doc of `lib.rs` to mention the rename refusal.

If the test still fails after this, **stop and report BLOCKED** with the observed behavior: spec §7 requires first confirming that SQLite calls `xRename`.

- [ ] **Step 4: Run to verify it passes**

Run: `scripts/test-all.sh`. Expected: all pass.

- [ ] **Step 5: Gate row, check, commit**

Gate: `module.xRename = Some(vtab::refuse_rename);` → `module.xRename = None;`, which turns `a_view_cannot_be_renamed` red. Verify it, then run the full check.

```bash
git add crates/ivmlite-sqlite/src/lib.rs crates/ivmlite-sqlite/src/vtab.rs crates/ivmlite-test/tests/extension_lifecycle.rs docs/mutation-gates.md
git commit -m "feat(sqlite): refuse ALTER TABLE RENAME of a view

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Documentation

**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md`, `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, `docs/README.md`, `docs/mutation-gates.md`

**Interfaces:** none (documentation only).

- [ ] **Step 1: Phase 3a spec**

Add a dated amendment line under its Status: *"Amended 2026-09-27: Phase 3b ([2026-09-27-m1b-phase3b-shared-capture-design.md](2026-09-27-m1b-phase3b-shared-capture-design.md)) supersedes §4's `__ivm_dep.shape` and one-view-per-table rule, §5 step 3, the REPLACE and rename limitations, and scenario 5's 'already tracked' and `ON CONFLICT REPLACE` rejections."* At each of those places, add a one-line pointer to the Phase 3b section that replaces it (§3, §4, §6, §7). Leave the Phase 3a text in place: it records what Phase 3a shipped.

- [ ] **Step 2: Parent spec**

In `2026-09-18-ivmlite-design.md`:
- §7.2 and §8.1: add a dated amendment pointing to Phase 3b §5 (GC inside the apply trigger) and §6 (the capture triggers), respectively;
- §13: update the items on REPLACE and rename to Phase 3b §6.4 and §7, and add the §6.4 residual as a known limitation.

Match each section's existing amendment style: search the file for "Phase 3a" to see it.

- [ ] **Step 3: README and gate table**

- `docs/README.md`: add the Phase 3b spec and this plan to the index, in the style of the Phase 3a entries.
- `docs/mutation-gates.md`: two older rows (the encode tag row and the state-table-suffix row) still claim that `scripts/test-all.sh` halts after the extension's unit tests, which has not been true since Phase 3a Task 5. Correct their wording to "both cargo invocations run", and leave their verified counts unchanged.

- [ ] **Step 4: Checks and commit**

Run:
- `python3 scripts/count-mutation-gates.py` → `consistent`;
- `git grep -nP '[\x{4e00}-\x{9fff}]' -- crates docs scripts .github ':!docs/cases'` → no output;
- the Global Constraints checks.

```bash
git add docs/superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md docs/superpowers/specs/2026-09-18-ivmlite-design.md docs/README.md docs/mutation-gates.md
git commit -m "docs: record M1b Phase 3b in the specs, the index and the gate table

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
