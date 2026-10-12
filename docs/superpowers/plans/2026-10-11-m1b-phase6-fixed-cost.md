# M1b Phase 6: The Fixed Cost of a Refresh — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove two fixed costs from every refresh, and fix one O(views) scan. Make the benchmark measure the steady state, then measure the phase and prepare `v0.1.0-alpha.4`.
- Statements are recompiled on every refresh.
- Every view re-reads the catalog after any DDL.
- The O(views) scan is in garbage collection.

**Architecture:**
- **Extension changes** are in `crates/ivmlite-sqlite/src/`:
  - `vtab.rs`: each view owns its `Connection` and its share of the snapshot;
  - `view.rs`: cached statements, fixed staging shapes, checks against the snapshot, and the progress index;
  - a new `snapshot.rs`: the per-connection catalog snapshot and its registry.
- **Benchmark changes** are in `crates/ivmlite-workload` (multi-round traces) and `crates/ivmlite-bench` (a cell runs one warm-up batch and five measured batches).

**Tech Stack:** Rust 1.95, rusqlite 0.40 (the extension crate builds it with `loadable_extension` only), and SQLite ≥ 3.45 in the test host. The trigger SQL must still parse on SQLite < 3.44.

**Spec:** `docs/superpowers/specs/2026-10-11-m1b-phase6-fixed-cost-design.md` (binding). It builds on the Phase 5 spec §3–§6 and the Phase 4 spec §3–§4.

## Global Constraints

- **English only** in every file, comment, error message, doc and commit message.
- **Extension conventions.** Phase 3a §4–§5 and Phase 3b still hold:
  - every identifier is quoted;
  - statements outside trigger bodies are qualified with `"main"`, and names inside trigger bodies stay unqualified;
  - the arming statement is the last statement of every refresh apply;
  - every callback runs inside `guard`;
  - a broken view still connects and can be dropped.
- **Trigger SQL compatibility.** Trigger SQL never uses aggregate `ORDER BY`, `FILTER`, window functions or `group_concat`. A database stays readable by SQLite < 3.44.
- **Cached-statement rules (spec §3).**
  - A cached statement is reset before its callback returns.
  - No cached statement, including any step of the apply trigger's body, reads or writes an ivmlite virtual table.
  - The refresh path creates and drops no TEMP object.
- **Error messages.** Every existing broken-view message keeps its exact text (spec §4).
- **No new dependencies.** rusqlite features may be enabled only where the test crate already does: `hooks` in `ivmlite-test`.
- **Building and testing.**
  - Run `scripts/build-extension.sh` before any `ivmlite-test` test.
  - Never build a copy of the repository into the repository's `target/`.
  - SQL-executing tests live in `crates/ivmlite-test`, because the extension crate cannot open a `Connection`.
- **Before every commit**, run all of these:
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
  - `scripts/test-all.sh`
- **Mutation gates.**
  - Every new check or behaviour gets a row in `docs/mutation-gates.md`. To fill it: mutate, run `scripts/test-all.sh`, record `N passed / M failed (baseline X/0)` and the red tests, restore, then rebuild.
  - `python3 scripts/count-mutation-gates.py` must print `consistent`.
  - Comments cite spec sections, never ledger rulings.
- **Git.**
  - Stage specific files only, and never use `--no-verify`.
  - End each commit with the implementer's own model's `Co-Authored-By` line.
  - Never merge or push.

## Review Focus

These failure modes are implied by the spec but not exercised by any task's main tests. Each one has a test in the task named after it.

1. **Closing after a failed or unrefreshed view.** A connection where a view was created but never refreshed, or where a `CREATE VIRTUAL TABLE` failed, must still close with `sqlite3_close` (v1) returning `SQLITE_OK`. (Task 2)
2. **A refresh that fails part way.** After a refresh raises, for example on a broken invariant, the next refresh still works, and `DROP TABLE` of that view still succeeds. Its cached statements were left reset. (Task 2)
3. **A lowered `SQLITE_LIMIT_VARIABLE_NUMBER` after statements are cached.** Lowering the limit between two refreshes gives smaller chunk shapes, and the view still matches the oracle. (Task 2)
4. **A base table dropped and recreated on the same connection between refreshes.** The snapshot is rebuilt, because the version moved. The next refresh reports today's broken-view message rather than reusing a stale verdict. (Task 3)
5. **Several views on one table across two connections.** Connection A's views and connection B's views on the same base table each see the other connection's DDL on their next refresh. Each connection keeps its own snapshot. (Task 3)

---

### Task 1: Steady-state benchmark (spec §6)

**Files:**
- Modify:
  - `crates/ivmlite-workload/src/lib.rs`: multi-round traces and the rounds check;
  - `crates/ivmlite-bench/src/engine.rs`: `run_cell`, `Measurement`;
  - `crates/ivmlite-bench/src/main.rs`: CSV header and rows;
  - `crates/ivmlite-bench/src/confirm.rs`: selection uses the steady total;
  - `crates/ivmlite-bench/src/ablation.rs`: if it reads `Measurement`;
  - `crates/ivmlite-bench/src/plot.rs`: reads the old and new CSVs;
  - `docs/bench/phase4_tables.py` and `docs/bench/phase5_tables.py`: tolerate the new columns;
  - `docs/mutation-gates.md`.
- Test: unit tests beside the code in `ivmlite-workload` and `ivmlite-bench`.

**Interfaces:**
- **Produces**, for the measurement task:
  - `ivmlite_workload::MEASURED_ROUNDS: usize = 5`;
  - `Workload::update_rounds(&self, rounds: usize) -> Vec<Vec<TraceOp>>`;
  - `Workload::check_rounds(&self, rounds: usize) -> Result<(), WorkloadError>`.
- **`Measurement` gains** `first_refresh_ms: f64` and `apply_plus_maintain_ms: f64`.
  - `apply_ms` and `maintain_ms` become medians over the measured rounds.
  - `apply_plus_maintain_ms` is the median of the per-round sums.
- **CSV:** `first_refresh_ms,apply_plus_maintain_ms` are appended at the end of `CSV_HEADER`.

- [ ] **Step 1: Multi-round traces (workload).**
  - `update_rounds(rounds)` continues one generator across rounds. Write it as `update_trace`'s body with the `next_id` counter and the `deleted` set carried across rounds, each round taking `batch_size` ops.
  - Keep `update_trace()` and make it return `update_rounds(1).remove(0)`, so every existing test and trace is unchanged.
  - `check_rounds(rounds)` refuses a cell when `rounds × batch_size × delete_ratio` exceeds `base_rows`. The message names the cell's base rows, batch size and the rounds requested.

  Tests:
  ```rust
  #[test]
  fn rounds_continue_one_trace_and_touch_disjoint_rows() {
      let w = Workload::load(&repo_path("workloads/m0-baseline.toml")).unwrap();
      let rounds = w.update_rounds(6);
      assert_eq!(rounds.len(), 6);
      assert!(rounds.iter().all(|r| r.len() == w.updates.batch_size));
      assert_eq!(rounds[0], w.update_trace());
      let mut seen = std::collections::BTreeSet::new();
      for op in rounds.iter().flatten() {
          let id = match op { TraceOp::Insert { id, .. } | TraceOp::Delete { id } => *id };
          assert!(seen.insert(id), "id {id} touched twice");
      }
  }
  ```
  Also add:
  - every delete targets a base row, or a row inserted in an earlier round, that is live at that point;
  - `check_rounds` accepts every shipped matrix cell at 6 rounds and refuses a hand-made cell with `base_rows = 10, batch_size = 100, delete_ratio = 0.33`.

- [ ] **Step 2: The cell runner (bench).** `run_cell` does this, in order:
  1. Seed, bootstrap and verify, as today.
  2. Prepare every timed statement once, as today.
  3. Run `check_rounds(1 + MEASURED_ROUNDS)`.
  4. **Warm-up round:** apply round 0 untimed, then maintain it timed as `first_refresh_ms`, then verify.
  5. **Measured rounds** 1..=5: each with a timed apply and a timed maintain, and the `assert_not_reprepared` checks after each.
  6. Verify after the last round.

  The space snapshots `written` and `maintained` are taken after the last round's apply and maintain. Medians use `demo_bench`'s rule; the bench cannot depend on `ivmlite-test`'s module, so copy the small median helper or move it to `ivmlite-workload`. The rule:
  - per step, the median over rounds;
  - for the total, the median of the per-round `apply + maintain`;
  - for an even count, the mean of the two middle values.

  Tests:
  - `run_cell` on a 1,000-row cell yields `first_refresh_ms > 0` for ivmlite and `naive_recompute`, and `0` for `no_maintenance`;
  - `apply_plus_maintain_ms` equals the median of the per-round sums. Add a pure unit test of the median helper with rounds whose sum of medians differs from the median of sums, for example apply `[1,9,2,8,3]` and maintain `[9,1,8,2,7]`.

- [ ] **Step 3: CSV consumers.**
  - `confirm`, `plot`, `ablation`, `phase4_tables.py` and `phase5_tables.py` must read old CSVs without the two columns and new CSVs with them.
  - Confirmation selection and the trigger comparison use `apply_plus_maintain_ms` when present, and `apply_ms + maintain_ms` otherwise.
  - Regenerate the Phase 4 and Phase 5 charts into a scratch directory, and check they are byte-identical to the committed ones.
  - Run both tables scripts and check their output is unchanged, by diffing before and after.

- [ ] **Step 4: Gate rows.**
  - Generate one round instead of continuing the generator: the disjointness test goes red.
  - Take the sum of the medians: the median test goes red.
  - Drop `check_rounds`: its refusal test goes red.

- [ ] **Step 5: Checks and commit.** Run the Global Constraints checks and the gate count, then:
  `git commit -m "bench: time a cell's steady state over five batches after a warm-up"`

---

### Task 2: Statement cache (spec §3)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/vtab.rs`, `crates/ivmlite-sqlite/src/view.rs`, `docs/mutation-gates.md`
- Test: create `crates/ivmlite-test/tests/extension_statement_cache.rs`

**Interfaces:**
- **Consumes:** nothing from Task 1.
- **Produces:**
  - `IvmTab` gains a field `conn: Rc<Connection>`, made in `open_view` and used by every later callback of that view.
  - `view::statement_capacity(view: &CompiledView) -> usize`.
  - `Stager::finish` stages the remainder in binary-digit chunks.

- [ ] **Step 1: Failing tests** in `extension_statement_cache.rs`. Reuse `open_with_extension`, `setup` and the `SUMS` helpers from the existing extension tests. Copy the minimal helpers, as other test files do.

  ```rust
  use std::cell::Cell;
  use std::rc::Rc;
  use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

  /// Spec §3: after the cache is warm, a refresh prepares no statement.
  /// The authorizer is consulted whenever SQLite prepares a statement, so
  /// counting its calls counts prepares. Installing it expires every
  /// statement, so the first two refreshes re-warm the cache.
  #[test]
  fn a_steady_refresh_prepares_no_statement() {
      let c = open_with_extension(None).unwrap();
      setup(&c);
      create(&c, "sums", SUMS).unwrap();
      let calls = Rc::new(Cell::new(0usize));
      let counter = calls.clone();
      c.authorizer(Some(move |_: AuthContext<'_>| {
          counter.set(counter.get() + 1);
          Authorization::Allow
      }));
      // Warm the cache: the first two refreshes re-prepare what the
      // authorizer expired.
      for rows in [3usize, 5] {
          insert_rows(&c, rows);
          refresh(&c, "sums").unwrap();
      }
      for rows in [1usize, 37, 130] {
          insert_rows(&c, rows);
          let at = calls.get();
          refresh(&c, "sums").unwrap();
          assert_eq!(calls.get(), at, "refresh after {rows} new rows prepared a statement");
      }
      assert_matches_oracle(&c, "sums", SUMS);
  }
  ```

  `insert_rows(&c, n)` inserts `n` rows that hit `n` distinct groups, so the stage holds about `n` state rows plus `n` output changes. It prepares its INSERT once, outside the counted window, or counts its own prepares and subtracts them. Write it so the assertion counts only the refresh.

  Also write:
  - `remainders_are_staged_in_push_order`: at 0, 1, 63, 64, 65 and 129 staged state rows, read the stage right after the refresh. The stage is kept full until the next apply. Its `state` rows must appear in rowid order exactly as computed, and the view must match the oracle.
  - `a_refresh_after_ddl_is_correct`: refresh after each of these, and match the oracle each time:
    - `CREATE INDEX` on the base table's `amount` (a non-unique index, which the capture accepts);
    - `CREATE TABLE unrelated(x)`;
    - a `CREATE TABLE` made by a second connection to the same file (use a temp-file database).
  - `a_view_drops_while_others_hold_cached_statements`: two views on one base table, both refreshed; `DROP TABLE` of the first succeeds; the second still refreshes and matches the oracle; dropping it removes `__ivm_delta_<t>`.
  - `close_v1_succeeds_after_refreshes`. This is Review Focus 1. Open a raw handle with `rusqlite::ffi`, load the extension, then cover four cases:
    - create views and refresh them;
    - create views and never refresh;
    - attempt a `CREATE VIRTUAL TABLE` that fails, for example with a bad SQL argument;
    - after each, `sqlite3_close` (not `_v2`) returns `SQLITE_OK`.

    Use `Connection::open` and then `ffi::sqlite3_close(c.handle())` through `std::mem::forget` of the `Connection`, or open with `ffi::sqlite3_open_v2` directly. Match how `ivmlite_test::open_with_extension_at` loads the library.
  - `a_failed_refresh_leaves_the_cache_usable`. This is Review Focus 2. Inject a `RAISE(ABORT)` on `__ivm_out_sums` insert and refresh: it fails. Then drop the fault trigger: the next refresh succeeds, matches the oracle, and `DROP TABLE sums` succeeds.
  - `a_lowered_variable_limit_after_caching_still_refreshes`. This is Review Focus 3. Refresh twice with the default limit. Then lower `SQLITE_LIMIT_VARIABLE_NUMBER` to 100 with `ffi::sqlite3_limit`. Insert 130 rows into distinct groups and refresh; the view matches the oracle.

- [ ] **Step 2: Run them; they fail** against today's per-callback `Connection`. The prepare-count test fails first.

- [ ] **Step 3: `vtab.rs`.**
  - In `open_view`, keep the `Rc<Connection>` that `make` used. Set `conn.set_prepared_statement_cache_capacity(view::statement_capacity(&compiled))` when the view compiled; a broken view keeps rusqlite's default.
  - Store it in `IvmTab.conn`.
  - `insert`'s refresh and `destroy` use `self.conn`.
  - The `IvmCursor` can keep using its own path: `open` copies `self.db`. If it prepares SQL, it may use `self.conn` through a clone of the `Rc`. Check the cursor's `filter` and keep it correct.
  - Delete the free `connection` function, or confine it to `open_view`.
  - Update the comment on `apply`'s arming statement: the cache now lasts the view's lifetime.

- [ ] **Step 4: `view.rs`, caching the fixed statements.**
  - Route every steady-path statement through `prepare_cached`:
    - `schema_version`;
    - `check_latch`;
    - the watermark read in `refresh`;
    - `read_deltas`: `prepare` → `prepare_cached`;
    - `apply`'s `DELETE FROM stage`, sentinel `INSERT` and progress `INSERT`. These use `exec` or `conn.execute` today, which prepares and finalizes each time.
  - Catalog checks stay uncached (spec §3).
  - Add:

  ```rust
  /// Spec §3: the fixed statements one view's refresh prepares, so the
  /// cache never evicts one of them for another.
  pub fn statement_capacity(view: &CompiledView) -> usize {
      const STAGING_SHAPES_PER_KIND: usize = 7; // the full chunk, then 32, 16, 8, 4, 2, 1
      const MARGIN: usize = 4;
      let tables = view.tables.len();
      let arrangements = arrangement_ids(&view.plan).len();
      let fixed = 1 // schema_version
          + 1 // latch (one text, bound per table)
          + 1 // watermark read
          + tables // delta reads
          + 2 * arrangements // BufferedArrangement get and scan
          + 3 // stage delete, sentinel insert, progress insert
          + 2 * STAGING_SHAPES_PER_KIND
          + 1; // arming UPDATE
      fixed + MARGIN
  }
  ```

  Check `state.rs`'s SQL count per arrangement against the `2 * arrangements` term and correct the term if it differs.

- [ ] **Step 5: `Stager::finish`, remainder shapes.** Replace the single remainder statement:

  ```rust
  /// Stage the rows left over, fewer than one chunk, in chunks of the binary
  /// digits of their count (32, 16, 8, 4, 2, 1 rows), front first, so every
  /// staging statement has one of a few cached shapes (spec §3) and the rows
  /// keep their push order.
  fn finish(mut self) -> Result<()> {
      let mut rows = self.values.len() / self.width;
      while rows > 0 {
          let take = 1usize << (usize::BITS - 1 - rows.leading_zeros());
          let sql = self.sql(take);
          execute_staging(self.conn, &sql, &mut self.values, take * self.width)?;
          rows -= take;
      }
      Ok(())
  }
  ```

  - `execute_staging` gains a `count` parameter and drains `values.drain(..count)`.
  - The full-chunk call passes the whole buffer.
  - Remainders are always smaller than `chunk`, so `take < chunk ≤ 64`.

- [ ] **Step 6: Run the tests; they pass.** Then run `scripts/test-all.sh`. Every Phase 5 atomicity and fault test stays green.

- [ ] **Step 7: Gate rows.**
  - Wrap the handle afresh on each refresh: `insert` uses `connection(self.db)?` again. The prepare-count test goes red.
  - Prepare the remainder as one uncached statement, restoring the old `finish`. The prepare-count test goes red.
  - Make `statement_capacity` return 2: the prepare-count test goes red.
  - Stage remainder chunks back to front: the push-order test goes red.

- [ ] **Step 8: Checks and commit.** Run the Global Constraints checks and the gate count, then:
  `git commit -m "perf(sqlite): keep each view's statements prepared across refreshes"`

---

### Task 3: Shared catalog snapshot (spec §4)

**Files:**
- Create: `crates/ivmlite-sqlite/src/snapshot.rs`
- Modify:
  - `crates/ivmlite-sqlite/src/lib.rs` (the `mod`);
  - `crates/ivmlite-sqlite/src/view.rs` (`verify`, `check_capture`, `check_output_index`, `check_table_capture`, `checked_schemas`, `object_exists`, `check_trigger`);
  - `crates/ivmlite-sqlite/src/vtab.rs` (`IvmTab` holds its snapshot);
  - `docs/mutation-gates.md`.
- Test: create `crates/ivmlite-test/tests/extension_catalog_snapshot.rs`, and keep `extension_refresh_cache.rs` green.

**Interfaces:**
- **Consumes:** Task 2's `IvmTab.conn`.
- **Produces** `snapshot.rs`:

  ```rust
  /// One connection's view of `main.sqlite_schema` at one schema version
  /// (spec §4), and the capture verdicts computed against it.
  pub struct Snapshot {
      version: i64,
      /// (type, name) → tbl_name, names exact as `sqlite_schema` stores them.
      objects: HashMap<(String, String), String>,
      /// base table → its capture verdict (shape, capture triggers, probe,
      /// delta and pend tables; the latch is never part of it).
      verdicts: HashMap<String, Result<Schema, String>>,
  }
  pub type Shared = Arc<Mutex<Option<Snapshot>>>;
  /// The snapshot every view on the connection `handle` shares.
  pub fn for_connection(handle: *mut ffi::sqlite3) -> Shared;
  impl Snapshot {
      /// Rebuilt with one scan when `version` differs from the held one.
      pub fn current<'a>(slot: &'a mut Option<Snapshot>, conn: &Connection, version: i64) -> Result<&'a mut Snapshot>;
      pub fn object(&self, kind: &str, name: &str) -> Option<&str>; // tbl_name
      pub fn verdict(&mut self, conn: &Connection, table: &str) -> Result<Schema>; // computed once per version
  }
  ```

  - **The registry** is `static REGISTRY: Mutex<HashMap<usize, Weak<Mutex<Option<Snapshot>>>>>`, keyed by the handle's address.
  - `for_connection` upgrades the `Weak`, or creates a new snapshot when the `Weak` is dead or absent. It prunes dead entries while it holds the lock.

- [ ] **Step 1: Failing tests** in `extension_catalog_snapshot.rs`:
  - `one_scan_serves_every_view_after_ddl`. Create 20 views on one base table and refresh each once. Then run `CREATE TABLE unrelated(x)`, install an authorizer and count its `Read` actions on `sqlite_schema` during the next refresh of all 20 views. Fewer than 20 × 10 must happen; assert at most 2 scans' worth, one rebuild. Write the authorizer filter to count `AuthAction::Read { table_name: "sqlite_schema" | "sqlite_master", .. }`. Every view must still match the oracle.
  - `a_broken_capture_breaks_every_view_of_the_table`. Two views read `orders`; `DROP TRIGGER` of one capture trigger; both views' next refreshes fail with today's message (`… its capture trigger … is missing …`).
  - `a_recreated_base_table_is_seen`. This is Review Focus 4. Refresh. Then `DROP TABLE orders` and recreate it with the same columns on the same connection. The next refresh fails with today's broken-view message for a dropped capture trigger.
  - `each_connection_keeps_its_own_snapshot`. This is Review Focus 5. Use two extension connections to one temp-file database, each holding views on `orders`. DDL on A, `DROP TRIGGER` of a capture trigger, breaks B's next refresh. DDL on B, `CREATE INDEX`, leaves A's views correct.
  - `the_snapshot_is_freed_with_its_connection`. Open, create, refresh and close a connection, then open a new connection to the same file. Its first refresh is correct. It is a fresh snapshot: the old handle's `Weak` is dead. Assert correctness, which is observable; the freeing itself is covered by the gate rows.

- [ ] **Step 2: Implement `snapshot.rs`.**
  - `current` reads `SELECT type, name, tbl_name FROM "main".sqlite_schema` once, and clears `verdicts`.
  - `verdict` computes `check_table_capture` without its latch call. It reads the shape and base schema through `SqliteCatalog` (pragmas, as today), and the triggers, probe, delta and pend tables through `object`/`tbl_name` lookups that keep `check_trigger`'s exact messages. It caches `Err` too, so a broken table is reported identically to every view.

- [ ] **Step 3: Route the checks through it.**
  - In `view.rs`:
    - `object_exists`, `table_exists` and `check_trigger` gain a `&Snapshot` variant used by the checks. Keep the SQL versions only where no snapshot exists yet, for example inside `create` before the view is registered.
    - `check_table_capture` becomes the snapshot's `verdict` plus the latch.
    - `verify`, `check_capture` and `check_output_index` take `&mut Snapshot`.
    - The `__ivm_dep` lookup stays a data read, uncached as a check (spec §3).
  - `checked_schemas` locks the view's `Shared`:
    1. reads the version;
    2. on a mismatch, calls `Snapshot::current` and runs the view's checks against it;
    3. on a hit, reads only the latches, as in Phase 5.
  - `IvmTab` holds `snapshot: snapshot::Shared` from `open_view` (`for_connection(handle)`).
  - Keep `Checked` as the per-view record of the version it passed at.

- [ ] **Step 4: Run all tests.** Every `extension_refresh_cache.rs` test, the Phase 3b capture tests and the new tests pass, with today's messages.

- [ ] **Step 5: Gate rows.**
  - Never rebuild on a version change (compare against a stale version): `one_scan…` stays green, but the DDL tests go red. Record which.
  - Keep verdicts across versions (do not clear `verdicts`): `a_recreated_base_table_is_seen` goes red.
  - Key verdicts by a constant instead of the table name: a two-table join test goes red. Use the existing join tests, or add one with two base tables where only one table's capture trigger is dropped.
  - Make the registry return a new snapshot every call: `one_scan…` goes red.

- [ ] **Step 6: Checks and commit.** Run the Global Constraints checks and the gate count, then:
  `git commit -m "perf(sqlite): share one catalog snapshot per connection and schema version"`

---

### Task 4: Progress index, format 5 (spec §5)

**Files:**
- Modify:
  - `crates/ivmlite-sqlite/src/view.rs` (`FORMAT`, `create_global_tables`, the snapshot checks);
  - `crates/ivmlite-sqlite/src/names.rs` (`PROGRESS_INDEX`);
  - `crates/ivmlite-test/tests/extension_lifecycle.rs` (the format test);
  - `docs/mutation-gates.md`.
- Test: `crates/ivmlite-test/tests/extension_lifecycle.rs` and `extension_gc.rs`.

**Interfaces:**
- **Consumes:** Task 3's `Snapshot::object`.
- **Produces:** `names::PROGRESS_INDEX = "__ivm_progress_tbl"` and `FORMAT = 5`.

- [ ] **Step 1: Failing tests.**
  - **`the_gc_and_watermark_update_search_the_progress_index`.** Read the stored `__ivm_apply_<view>` SQL from `sqlite_schema`. Extract its GC statement and its watermark `UPDATE`, the same way `the_retraction_lookup_searches_the_output_index` extracts the lookup. Substitute literal values for `NEW`-free text where needed. `EXPLAIN QUERY PLAN` must show `SEARCH __ivm_progress USING … INDEX __ivm_progress_tbl` for the `MIN(applied_seq)` subquery, and must not show a `SCAN __ivm_progress`.
  - **`dropping_the_progress_index_breaks_the_next_refresh`.** The message names `__ivm_progress_tbl`, in the same shape as the output-index message (`its shadow index … is missing`).
  - **The format test.** Tamper the stored format to 4. The view opens broken with "format 4 … reads format 5" and can be dropped.

- [ ] **Step 2: Implement.**
  - `create_global_tables` adds `CREATE INDEX IF NOT EXISTS "main"."__ivm_progress_tbl" ON "__ivm_progress"(tbl, applied_seq);`.
  - `FORMAT = 5`, with its doc citing Phase 6 §5.
  - Every view's check, through the snapshot, requires the index.

- [ ] **Step 3: Run all tests.** Adapt any test that pins `FORMAT` or the global-table DDL.

- [ ] **Step 4: Gate rows.**
  - Drop the index from `create_global_tables`: the plan test goes red.
  - Skip its check: the dropping test goes red.

- [ ] **Step 5: Checks and commit.**
  `git commit -m "perf(sqlite): index __ivm_progress for the apply's garbage collection (format 5)"`

---

### Task 5: Measure, report, and prepare the release (spec §7)

**Files:**
- Create:
  - `docs/bench/m1b-phase6.csv`, `m1b-phase6-confirm.csv`, `m1b-phase6-ablation.csv`, `m1b-phase6-ablation-builds.csv`, `m1b-phase6-demos.csv`, `m1b-phase6-demos-builds.csv`;
  - the charts;
  - `docs/bench/phase6_tables.py`;
  - `docs/releases/v0.1.0-alpha.4.md`.
- Modify:
  - `docs/bench/README.md` (a new "M1b Phase 6" section, and the Phase 4/5 sections' note that they measured the first refresh);
  - `docs/cases/evaluation/README.md` and its CSV (re-run);
  - `CHANGELOG.md` (`[Unreleased]`);
  - `docs/README.md` (index);
  - the parent spec §10 (a dated amendment);
  - the Phase 5 spec §3 (a dated amendment pointing to Phase 6 §4).

- [ ] **Step 1: Run.** Do it in one session on an awake, preferably idle machine. Wrap every command in `caffeinate -i`, and record the wall time and the load average at the start and the end of each run.
  1. `OUT=docs/bench/m1b-phase6-ablation.csv WRITE_AMP_OUT=<scratch> scripts/bench-ablation.sh before=ebd716b stmtcache=<Task 2> snapshot=<Task 3> progidx=<Task 4>`.
     - The script builds the bench once from this tree, so every label is measured with Task 1's steady protocol.
     - Write `m1b-phase6-ablation-builds.csv` (label → commit) by hand, as in Phase 4 and 5.
     - Write-amp is not published, since the write path did not change.
  2. `scripts/bench.sh matrix > docs/bench/m1b-phase6.csv`.
  3. `scripts/bench.sh confirm --from docs/bench/m1b-phase6.csv > docs/bench/m1b-phase6-confirm.csv`.
  4. `OUT=docs/bench/m1b-phase6-demos.csv scripts/bench-demos.sh before=ebd716b after=<Task 4>`.
  5. `real_case_bench` at the settings documented in `docs/cases/evaluation/README.md`. Update the README's numbers.
  6. The charts, with `plot --from` the Phase 6 CSVs.
  7. Memory: SQLite's memory use after refreshing 200 views, `before` against `progidx`, using a throwaway script that calls `sqlite3_memory_used` through `ctypes`. Report it in the README; the script is not committed.

  If a run aborts on a verification mismatch, stop and report BLOCKED with the cell.

- [ ] **Step 2: Apply the acceptance rule (spec §7).**
  - From the ablation, check each change against its bar:
    - §3: steady `maintain_ms` at least 5% lower at 10 and 200 views, batch 1;
    - §4: `first_refresh_ms` at least 5% lower at 200 views;
    - §5: steady `maintain_ms` at least 5% lower at 200 views, batch 1.
  - A change that misses its bar is reported BLOCKED to the controller, with the numbers. The controller decides on the revert and the spec amendment.

- [ ] **Step 3: `phase6_tables.py`.** Write it like `phase5_tables.py`: standard library only, run from the repo root. It covers:
  - the ablation per change, both steady and first refresh, as medians and ranges;
  - the speedup surface against recomputation;
  - the confirmed cells;
  - the §10.2 bars against hand-written triggers, in the steady state;
  - the Phase 5 → Phase 6 change per cell, with the caveat that Phase 5 measured the first refresh;
  - the demo before and after;
  - the drift control: the engines no build changes.

- [ ] **Step 4: README "M1b Phase 6" section.** Every number comes from the script. The section covers:
  - the setup, including the load averages;
  - the protocol change and why the old numbers measured the first refresh;
  - the ablation;
  - the §10.2 bars, plainly: how close ivmlite now is to hand-written triggers, and where;
  - what still loses and why: the apply trigger's body and compute;
  - anything that got worse, including memory;
  - the limits.

- [ ] **Step 5: Docs and release prep.**
  - **CHANGELOG `[Unreleased]`:**
    - format 5 (breaking: alpha.3 views must be dropped and recreated; they open as broken views);
    - the statement cache, the snapshot and the progress index;
    - the steady-state protocol;
    - the headline numbers.
  - **`docs/releases/v0.1.0-alpha.4.md`:** in alpha.3's style. Leave the version bump to the controller.
  - **`docs/README.md` index:** the spec, the plan and the tables script.
  - **Dated amendments:** the parent spec §10, and the Phase 5 spec §3.

- [ ] **Step 6: Checks and commits.** Run the Global Constraints checks, the gate count and the CJK grep. Commit the data, the scripts and the docs separately.
