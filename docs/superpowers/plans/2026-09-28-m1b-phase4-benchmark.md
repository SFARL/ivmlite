# M1b Phase 4: Benchmarking the SQLite Extension — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the ivmlite extension as a fourth engine in `ivmlite-bench` under a sound measurement protocol, fix its two measured hot spots (the retraction full scan and the four-scan latch), measure each fix's effect, measure write amplification including UNIQUE-key REPLACE, and publish an honest slice of spec §10.4's surface.

**Architecture:** `ivmlite-bench` gains an engine abstraction shared by four engines and four modes (`matrix`, `confirm`, `ablation`, `write-amp`). It loads the release extension through a library-locating helper shared with `ivmlite-test`. Workload parameters stay in TOML files read by `ivmlite-workload`. The two fixes are local to `crates/ivmlite-sqlite/src/view.rs`.

**Tech Stack:** Rust 1.95, rusqlite 0.40 (`bundled` + `load_extension` in the workspace; `loadable_extension` + `vtab` in the extension), `toml`, `serde`, `rand` 0.10.

**Spec:** `docs/superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md` (binding). Background: the parent spec §10, and Phase 3b's spec §6.2 for the latch.

## Global Constraints

- **English only** in every file, comment, error message, doc and commit message.
- The extension's conventions (Phase 3a §4–§5, Phase 3b) still hold:
  - every identifier is quoted;
  - statements outside trigger bodies are qualified with `"main"`; names inside trigger bodies stay unqualified;
  - the arming UPDATE is the last statement of every apply;
  - every callback runs inside `guard`;
  - a broken view still connects and can be dropped;
  - no trigger SQL may use aggregate `ORDER BY`, `FILTER`, window functions or `group_concat`: databases must stay readable by SQLite < 3.44.
- No new dependencies beyond enabling `rusqlite`'s `load_extension` feature for `ivmlite-bench` and letting `ivmlite-bench` depend on `ivmlite-test` (for the library locator).
- **Timing protocol (spec §3.2):** bootstrap → verify initial → prepare → timed apply → timed maintain → verify final. Nothing runs between the two timed regions. A verification mismatch aborts; no numbers are published for a wrong engine.
- **Matrix order (spec §3.3):** cells in the outer loop, engines in the inner loop. Cell `i` runs `ENGINES` rotated left by `i % 4`.
- **Workload portability (spec §8):** parameters that select what is measured live in workload TOML files. The protocol constants live in the runner: the confirmation rule, `K = 5` and the rotation.
- **Build and test commands:**
  - The extension builds with `scripts/build-extension.sh` (debug) or `cargo build --release --manifest-path crates/ivmlite-sqlite/Cargo.toml`.
  - After editing `crates/ivmlite-sqlite`, `crates/ivmlite-core` or `crates/ivmlite-sql`, rebuild before running `ivmlite-test` tests.
  - Build no copy of the repository into the repository's `target/` (use a separate `CARGO_TARGET_DIR`).
- **Before every commit**, all of these must pass:
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
  - `scripts/test-all.sh`
- **Mutation gates:** every new check in the extension gets a verified row in `docs/mutation-gates.md` under `## ivmlite-sqlite`, in the existing format:
  1. Mutate the code.
  2. Run `scripts/test-all.sh` and record `N passed / M failed (baseline X/0)` and which tests went red.
  3. Restore the code and rebuild.
  4. Update the count line so that `python3 scripts/count-mutation-gates.py` prints `consistent`.
- **Git:**
  - stage specific files only, never `git add -A`;
  - never `--no-verify`;
  - commit messages end with the implementer's own model's `Co-Authored-By` line (e.g. `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`);
  - do not touch other branches or untracked files that are not the task's own.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/ivmlite-test/src/extension.rs` | `extension_library_for(profile)`: locate a debug or release build and check that it is fresh | 1 |
| `crates/ivmlite-bench/Cargo.toml` | the `load_extension` feature, and `ivmlite-test` as a dependency | 1 |
| `crates/ivmlite-bench/src/engine.rs` (new) | the `Engine` enum, the per-cell protocol (§3.2), space sampling and verification | 1 |
| `crates/ivmlite-bench/src/baseline.rs` | the existing SQL helpers, reused by `engine.rs` | 1, 5 |
| `crates/ivmlite-bench/src/main.rs` | mode dispatch, the matrix loop with rotation, CSV output | 1, 2, 5 |
| `crates/ivmlite-bench/src/confirm.rs` (new) | the selection rule and K repeats | 2 |
| `crates/ivmlite-bench/src/ablation.rs` (new) | the ablation cells × K repeats against an explicit library | 2 |
| `crates/ivmlite-bench/src/write_amp.rs` (new) | the write-amp runner | 5 |
| `crates/ivmlite-bench/src/plot.rs` | charts with an `ivmlite` series | 6 |
| `crates/ivmlite-workload/src/lib.rs` | the `[ablation]` section, `WriteAmpWorkload`, and its trace | 2, 5 |
| `workloads/m0-baseline.toml` | the `[ablation]` section | 2 |
| `workloads/write-amp.toml` (new) | the write-amp workload | 5 |
| `scripts/bench.sh`, `scripts/bench-ablation.sh` (new) | build in release mode and run | 1, 2 |
| `crates/ivmlite-sqlite/src/view.rs`, `src/names.rs` | the output index (§4) and the one-scan latch (§5) | 3, 4 |
| `crates/ivmlite-test/tests/extension_*.rs` | tests for the extension fixes | 3, 4 |
| `docs/bench/*`, `docs/README.md`, the specs | data and analysis | 6 |

---

### Task 1: The `ivmlite` engine and the measurement protocol

**Files:**
- Modify:
  - `crates/ivmlite-test/src/extension.rs`, `crates/ivmlite-test/src/lib.rs`
  - `crates/ivmlite-bench/Cargo.toml`, `crates/ivmlite-bench/src/main.rs`, `crates/ivmlite-bench/src/baseline.rs`
- Create:
  - `crates/ivmlite-bench/src/engine.rs`
  - `scripts/bench.sh`

**Interfaces:**
- Produces, in `ivmlite-test`:

  ```rust
  pub enum Profile { Debug, Release }
  /// The extension library of `profile` under crates/ivmlite-sqlite/target/,
  /// or why it cannot be used (missing, or older than any source, manifest or
  /// lock file of ivmlite-sqlite, ivmlite-core or ivmlite-sql).
  pub fn extension_library_for(profile: Profile) -> Result<PathBuf, String>;
  ```

  `extension_library()` becomes `extension_library_for(Profile::Debug).unwrap_or_else(|why| panic!("{why}"))`, with the same messages as today.
  - For the release profile, the "run the script" hint in the error names `scripts/bench.sh`.
  - Also export `open_with_extension_at(path: Option<&Path>, lib: &Path) -> rusqlite::Result<Connection>`. `open_with_extension` calls it with the debug library.
- Produces, in `ivmlite-bench/src/engine.rs`:

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Engine { NoMaintenance, HandWrittenTrigger, NaiveRecompute, Ivmlite }
  pub const ENGINES: [Engine; 4] = [Engine::NoMaintenance, Engine::HandWrittenTrigger, Engine::NaiveRecompute, Engine::Ivmlite];
  impl Engine { pub fn label(self) -> &'static str }   // "no_maintenance", "hand_written_trigger", "naive_recompute", "ivmlite"
  /// ENGINES rotated left by `cell_index % 4` (spec §3.3).
  pub fn engine_order(cell_index: usize) -> [Engine; 4];
  #[derive(Debug, Clone, Copy, Default, PartialEq)]
  pub struct Space { pub page_count: i64, pub freelist_count: i64 }
  #[derive(Debug, Clone)]
  pub struct Measurement {
      pub engine: Engine, pub bootstrap_ms: f64, pub apply_ms: f64, pub maintain_ms: f64,
      pub page_size: i64, pub base: Space, pub bootstrapped: Space, pub written: Space, pub maintained: Space,
  }
  /// Run spec §3.2's sequence for one engine on one cell, in a fresh in-memory database.
  /// `lib` is the extension library, used only for Engine::Ivmlite.
  pub fn run_cell(engine: Engine, cell: &Workload, lib: &Path) -> Result<Measurement, String>;
  ```

  `Baseline` in `baseline.rs` is replaced by `Engine`: `baseline.rs` keeps its SQL helpers, and `main.rs` stops using `Baseline`.

- [ ] **Step 1: Write the failing tests**

In `crates/ivmlite-bench/src/engine.rs`, under `#[cfg(test)] mod tests`:

```rust
#[test]
fn engine_order_rotates_by_cell_index() {
    assert_eq!(engine_order(0), ENGINES);
    assert_eq!(
        engine_order(1),
        [Engine::HandWrittenTrigger, Engine::NaiveRecompute, Engine::Ivmlite, Engine::NoMaintenance]
    );
    assert_eq!(engine_order(4), ENGINES);
    assert_eq!(engine_order(7), engine_order(3));
}

/// Spec §9: a tiny cell per engine against the debug extension, with both
/// verifications, so the workspace suite exercises the whole protocol.
#[test]
fn every_engine_runs_a_tiny_cell_and_verifies_its_views() {
    let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
        .unwrap_or_else(|why| panic!("{why}"));
    let mut w = Workload::load(&repo_path("workloads/m0-baseline.toml")).unwrap();
    w.data.base_rows = 500;
    w.data.group_cardinality = 20;
    w.updates.batch_size = 30;
    w.views.truncate(2);
    for engine in ENGINES {
        let m = run_cell(engine, &w, &lib).unwrap_or_else(|e| panic!("{}: {e}", engine.label()));
        assert!(m.apply_ms >= 0.0 && m.maintain_ms >= 0.0 && m.bootstrap_ms >= 0.0);
        assert!(m.page_size > 0 && m.base.page_count > 0);
        if engine == Engine::Ivmlite {
            assert!(m.bootstrapped.page_count > m.base.page_count, "views add pages");
            assert!(m.maintain_ms > 0.0);
        }
    }
}

/// A wrong engine must never produce numbers (spec §3.2 step 7).
#[test]
fn a_view_that_disagrees_with_the_oracle_aborts_the_cell() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
         INSERT INTO orders VALUES (1, 'a', 5);
         CREATE TABLE mv_0(k TEXT NOT NULL PRIMARY KEY, s INTEGER NOT NULL, c INTEGER NOT NULL) STRICT;
         INSERT INTO mv_0 VALUES ('a', 6, 1);",
    )
    .unwrap();
    let view = ivmlite_workload::ViewSpec { id: 0, threshold: 0 };
    let err = verify_view(&conn, "orders", &view, "mv_0").expect_err("6 is not SUM(amount) = 5");
    assert!(err.contains("mv_0"), "{err}");
}
```

(`repo_path(rel)` is a test helper that joins `env!("CARGO_MANIFEST_DIR")/../..` with `rel`. `verify_view(conn, table, view, relation) -> Result<(), String>` compares `SELECT * FROM relation` with SQLite's evaluation of `view.sql(table)`, both as multisets. It is `pub(crate)`, and `run_cell` uses it for both engines that keep views.)

- [ ] **Step 2: Run them to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-bench --locked`.

Expected: they fail to compile, because `engine.rs`, `extension_library_for` and the other new names do not exist yet.

- [ ] **Step 3: Implement**

1. **`ivmlite-test`:** implement `Profile`, `extension_library_for` and `open_with_extension_at`, reusing today's `check_library` and `sources` unchanged. The existing `FakeBuild` tests must still pass. Add one test: `extension_library_for(Profile::Release)` returns `Err` naming `scripts/bench.sh` when the path does not exist. Use a helper that takes the directory, as `check_library` already does.
2. **`ivmlite-bench/Cargo.toml`:**

   ```toml
   ivmlite-test = { path = "../ivmlite-test" }
   rusqlite = { workspace = true, features = ["load_extension"] }
   ```
3. **`run_cell`**, in this order, exactly (spec §3.2):
   1. Open a fresh in-memory database. For `Ivmlite`, open it with `open_with_extension_at(None, lib)`.
   2. `seed_base`, then sample `page_size` and the `base` space: `PRAGMA page_size`, `PRAGMA page_count`, `PRAGMA freelist_count`.
   3. **Bootstrap**, timed as `bootstrap_ms`:
      - `HandWrittenTrigger`: `install_trigger_view` for every view;
      - `Ivmlite`: for every view, `CREATE VIRTUAL TABLE "mv_<id>" USING ivm('<view.sql(table)>')`, with `'` doubled;
      - the other engines record `0.0`.

      Then sample `bootstrapped`.
   4. **Verify the initial state:** `verify_view` against `mv_<id>` for `HandWrittenTrigger` and `Ivmlite`.
      - The hand-written summary table's columns are `(k, s, c)`, and the view's SQL yields `(region, SUM, COUNT)`. Compare values positionally, not by column name.
      - The hand-written table deletes a group when `c = 0`, as the view does.
   5. Build the trace and prepare the `ApplyStatements`, plus the `RecomputeStatements` for `NaiveRecompute`. For `Ivmlite`, prepare one `INSERT INTO "mv_i"("mv_i") VALUES ('refresh')` per view.
   6. **Timed `apply`**, via the existing `apply()`. Then sample `written`.
   7. **Timed `maintain`:**
      - `NaiveRecompute`: `recompute_all`;
      - `Ivmlite`: run each prepared refresh;
      - the other engines record `0.0`.

      Then sample `maintained`.
   8. **Verify the final state:** `verify_view` for `HandWrittenTrigger` and `Ivmlite`. For `NaiveRecompute`, check that its evaluation equals the oracle, which is the same query, so this is a sanity check only.
   9. Return the `Measurement`.
4. **`main.rs`:**
   - Parse the mode from `std::env::args()`: no argument or `matrix` runs the matrix. Tasks 2 and 5 add `confirm`, `ablation` and `write-amp`. `--extension <path>` overrides the library; otherwise use `extension_library_for(Profile::Release)` and exit with its message on `Err`.
   - The matrix loop is `for (i, cell) in cells.iter().enumerate() { for engine in engine_order(i) { … } }`.
   - Print the CSV to stdout with the header:

     `engine,views,base_rows,batch_size,group_cardinality,bootstrap_ms,apply_ms,maintain_ms,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free`

     Rows are sorted by `(engine label, views, base_rows, batch_size, group_cardinality)` before printing, so the file diffs stably.
   - The M0 chart writing moves to Task 6. Remove it from `main.rs` now, and keep `plot.rs` compiling (it still reads M0 `Record`s in its tests).
   - The existing test `cells_reproduce_exactly_the_published_csv_matrix` stays unchanged: it checks the M0 CSV against `cells()`.
5. **`scripts/bench.sh`** (make it executable):

   ```bash
   #!/usr/bin/env bash
   # Build the extension and the bench in release mode, then run the bench.
   # Arguments are passed to ivmlite-bench (e.g. `matrix`, `confirm`, …).
   set -euo pipefail
   cd "$(dirname "$0")/.."
   cargo build --release --manifest-path crates/ivmlite-sqlite/Cargo.toml
   cargo run --release -p ivmlite-bench --locked -- "$@"
   ```

- [ ] **Step 4: Run to verify**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-bench -p ivmlite-test --locked`. Expected: all pass.

Then run a release smoke test of the whole matrix path on a reduced workload. Copy `workloads/m0-baseline.toml` to a scratch file with `base_rows = [10000]`, `batch_sizes = [10]`, `view_counts = [1]` and `group_cardinalities = [10]`, and let `main.rs` accept `--workload <path>`. Run `scripts/bench.sh matrix --workload <scratch>`. It must print a header and 8 rows: 2 cells × 4 engines.

- [ ] **Step 5: Full checks and commit**

```bash
git add crates/ivmlite-test/src/extension.rs crates/ivmlite-test/src/lib.rs crates/ivmlite-bench/Cargo.toml crates/ivmlite-bench/src/main.rs crates/ivmlite-bench/src/baseline.rs crates/ivmlite-bench/src/engine.rs scripts/bench.sh Cargo.lock
git commit -m "feat(bench): ivmlite as a fourth engine under Phase 4's measurement protocol"
```

---

### Task 2: Confirmation and ablation modes

**Files:**
- Create: `crates/ivmlite-bench/src/confirm.rs`, `crates/ivmlite-bench/src/ablation.rs`, `scripts/bench-ablation.sh`
- Modify: `crates/ivmlite-bench/src/main.rs`, `crates/ivmlite-workload/src/lib.rs`, `workloads/m0-baseline.toml`

**Interfaces:**
- Consumes: Task 1's `Engine`, `engine_order`, `run_cell` and `Measurement`, and its CSV header.
- Produces, in `confirm.rs`:

  ```rust
  pub const REPEATS: usize = 5;
  /// One exploration row, parsed back from the CSV.
  pub struct Row { pub engine: String, pub views: usize, pub base_rows: usize, pub batch_size: usize,
                   pub group_cardinality: usize, pub apply_ms: f64, pub maintain_ms: f64 }
  pub fn parse_csv(text: &str) -> Result<Vec<Row>, String>;
  /// Spec §3.5: cells whose speedup is in [0.7, 1.4] or [1.4, 2.8], or where
  /// ivmlite's apply+maintain is below hand_written_trigger's. Returns the
  /// cell keys (views, base_rows, batch_size, group_cardinality), sorted, deduplicated.
  pub fn select(rows: &[Row]) -> Vec<(usize, usize, usize, usize)>;
  ```
- Produces, in `ivmlite-workload`: `pub struct AblationSpec { pub cells: Vec<AblationCell>, pub repeats: usize }` with `pub struct AblationCell { base_rows, group_cardinality, views, batch_size }`, and `Workload::ablation: Option<AblationSpec>` (serde `default`). Add `Workload::with_cell(&self, base_rows, card, views, batch) -> Workload`, which derives a cell with the matrix's view-threshold formula.
  - Refactor `cells()` to use `with_cell`, so the two can never disagree.
  - `cells_reproduce_exactly_the_published_csv_matrix` must stay green.

- [ ] **Step 1: Write the failing tests** (in `confirm.rs`)

```rust
fn row(engine: &str, key: (usize, usize, usize, usize), apply: f64, maintain: f64) -> Row {
    Row { engine: engine.into(), views: key.0, base_rows: key.1, batch_size: key.2,
          group_cardinality: key.3, apply_ms: apply, maintain_ms: maintain }
}

#[test]
fn selection_takes_the_near_1x_and_near_2x_cells_and_every_claim_of_beating_triggers() {
    let near_one = (10, 100_000, 100, 1_000);
    let near_two = (10, 100_000, 1_000, 1_000);
    let far = (10, 1_000_000, 1_000, 10);
    let beats_triggers = (200, 100_000, 1_000, 10);
    let rows = vec![
        row("naive_recompute", near_one, 1.0, 99.0), row("ivmlite", near_one, 10.0, 90.0),   // 1.0x
        row("naive_recompute", near_two, 1.0, 199.0), row("ivmlite", near_two, 10.0, 90.0),  // 2.0x
        row("naive_recompute", far, 1.0, 999.0), row("ivmlite", far, 1.0, 9.0),              // 100x
        row("hand_written_trigger", far, 50.0, 0.0),                                         // ivmlite 10 < 50
        row("naive_recompute", beats_triggers, 1.0, 999.0), row("ivmlite", beats_triggers, 5.0, 5.0),
        row("hand_written_trigger", beats_triggers, 9.0, 0.0),                               // 10 > 9: no claim
    ];
    assert_eq!(select(&rows), vec![near_one, near_two, far]);
}

#[test]
fn parse_csv_round_trips_the_task_1_header() {
    let text = "engine,views,base_rows,batch_size,group_cardinality,bootstrap_ms,apply_ms,maintain_ms,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free\n\
                ivmlite,10,1000,10,10,1.0,2.5,3.5,4096,1,0,2,0,3,0,3,0\n";
    let rows = parse_csv(text).unwrap();
    assert_eq!((rows[0].views, rows[0].apply_ms, rows[0].maintain_ms), (10, 2.5, 3.5));
    assert!(parse_csv("wrong,header\n").is_err());
}
```

In `ivmlite-workload`, add a test that `with_cell` reproduces every cell `cells()` returns.

- [ ] **Step 2: Run to verify they fail**

Run `cargo test -p ivmlite-bench -p ivmlite-workload --locked`. It fails to compile.

- [ ] **Step 3: Implement**

- **`confirm` mode**, `ivmlite-bench confirm --from docs/bench/m1b-phase4.csv [--workload …]`:
  - parse the CSV and `select` the cells;
  - for each cell (via `with_cell`) and each repetition `r` in `0..REPEATS`, run all four engines in `engine_order(r + cell_index)`, each with `run_cell`;
  - print the Task 1 CSV with one extra leading column, `repeat`.
- **`[ablation]` section** in `workloads/m0-baseline.toml`: exactly the four cells of spec §6, with `repeats = 5`.
- **`ablation` mode**, `ivmlite-bench ablation --extension <lib> --label <name>`:
  - for each ablation cell and each repetition, run `Engine::NaiveRecompute` and `Engine::Ivmlite` in the order given by rotating them by the repetition index;
  - print CSV rows with a leading `label,repeat,` prefix before Task 1's columns.
- **`scripts/bench-ablation.sh <label>=<rev> …`** (executable):
  1. build the release bench once, from the current tree;
  2. for each `label=rev`: `git worktree add` a detached checkout of `rev` under `${TMPDIR:-/tmp}/ivmlite-ablation-<label>`, then build its extension with `CARGO_TARGET_DIR` set to that worktree's own `target`;
  3. run `target/release/ivmlite-bench ablation --extension <that lib> --label <label>`, appending the rows to the output file (default `docs/bench/m1b-phase4-ablation.csv`, header written once);
  4. remove the worktree with `git worktree remove --force` on exit, via a trap.

  The script never touches the main checkout's `target/` for the extension.
- **The M0 `plot.rs` `Record`:** leave as is.

- [ ] **Step 4: Run to verify**

Run `cargo test -p ivmlite-bench -p ivmlite-workload --locked`; all tests must pass.

Then smoke-test the ablation script with one label against HEAD, using a scratch workload whose `[ablation]` has one tiny cell and `repeats = 1`: `scripts/bench-ablation.sh head=HEAD`. It must write a header and 2 rows, then remove its worktree (`git worktree list` shows only the main one).

- [ ] **Step 5: Full checks and commit**

```bash
git add crates/ivmlite-bench/src/confirm.rs crates/ivmlite-bench/src/ablation.rs crates/ivmlite-bench/src/main.rs crates/ivmlite-workload/src/lib.rs workloads/m0-baseline.toml scripts/bench-ablation.sh
git commit -m "feat(bench): confirmation and ablation modes"
```

Record this commit's hash in the task report: it is the ablation's "before §4" build.

---

### Task 3: Index the output table (spec §4)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs`, `crates/ivmlite-sqlite/src/names.rs`
- Test: `crates/ivmlite-test/tests/extension_lifecycle.rs`, and unit tests in `view.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces:
  - `names::out_index(view: &str) -> String`, returning `__ivm_outidx_<view>`;
  - `view::retraction_lookup(out: &str, cols: &[String], operand: impl Fn(usize) -> String) -> String`, which returns `SELECT rowid FROM <out> WHERE <c0> IS <operand(0)> AND …`.
  - `FORMAT` becomes `3`.

- [ ] **Step 1: Write the failing tests**

(a) A unit test in `view.rs`, which examines the exact lookup the trigger uses:

```rust
#[test]
fn the_retraction_lookup_searches_the_output_index() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE \"__ivm_out_v\"(\"k\" TEXT, \"s\" INTEGER, __w INTEGER NOT NULL);
         CREATE INDEX \"__ivm_outidx_v\" ON \"__ivm_out_v\"(\"k\", \"s\");",
    )
    .unwrap();
    let cols = vec![quote("k"), quote("s")];
    let lookup = retraction_lookup(&quote("__ivm_out_v"), &cols, |i| format!("?{}", i + 1));
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {lookup}"))
        .unwrap()
        .query_map([Option::<String>::None, None], |r| r.get::<_, String>(3))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(
        plan.iter().any(|d| d.contains("SEARCH") && d.contains("__ivm_outidx_v")),
        "{plan:?}"
    );
}
```

(b) End-to-end tests in `extension_lifecycle.rs`:

```rust
/// Phase 4 spec §4: a NULL group key and a SUM over only NULLs are retracted
/// through the output index.
#[test]
fn null_output_values_are_retracted() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, x INTEGER) STRICT;
         INSERT INTO t VALUES (NULL, 1), ('a', NULL), ('b', 2);",
    )
    .unwrap();
    let q = "SELECT k, SUM(x), COUNT(*) FROM t GROUP BY k";
    create(&c, "v", q).unwrap();
    c.execute_batch("INSERT INTO t VALUES (NULL, 5), ('a', NULL); DELETE FROM t WHERE k = 'b';")
        .unwrap();
    refresh(&c, "v").unwrap();
    assert_matches_oracle(&c, "v", q);
}

/// Phase 4 spec §4: one staged out- removes exactly one of two identical
/// output rows. v0 cannot produce duplicates through SQL, so this writes the
/// output and stage tables directly (white-box).
#[test]
fn one_retraction_removes_one_copy_of_a_duplicated_output_row() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);")
        .unwrap();
    create(&c, "v", "SELECT k, COUNT(*) FROM t GROUP BY k").unwrap();
    refresh(&c, "v").unwrap(); // empties the stage
    c.execute_batch(
        "INSERT INTO __ivm_out_v SELECT * FROM __ivm_out_v;
         DELETE FROM __ivm_stage_v;
         INSERT INTO __ivm_stage_v(op, c0, c1) VALUES ('out-', 'a', 1);
         UPDATE __ivm_stage_v SET armed = 1;",
    )
    .unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM __ivm_out_v"), 1);
}
```

Also:
- add `__ivm_outidx_<view>` to the TEMP-shadow test's list of TEMP objects;
- assert that after `DROP TABLE v`, no object named `__ivm_outidx_v` remains (`objects()` already covers this once the index exists);
- change the format test's expected text to "format 3", and its tampering to set `2`.

- [ ] **Step 2: Run to verify they fail**

Run `cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked`: the unit test fails to compile, since `retraction_lookup` does not exist yet.

Run the end-to-end tests after `scripts/build-extension.sh`. Record which ones fail. The NULL test may already pass: `IS` handles NULL even with a full scan. That is fine, because it pins the semantics.

- [ ] **Step 3: Implement**

- `names.rs`: add `out_index`, with a doc comment.
- `create_out_table`: after the `CREATE TABLE`, run `CREATE INDEX {main_qualified(out_index(name))} ON {quote(out_table(name))}(<every output column, quoted>)`. It must be non-unique; add a comment citing spec §4, which explains why.
- In `create_stage`: build `same` once from `retraction_lookup(&out, &cols, |i| format!("NEW.c{i}"))`, and use it in both places:
  - the RAISE check becomes `… WHERE NEW.op = 'out-' AND NOT EXISTS ({lookup})`;
  - the DELETE becomes `… AND rowid = ({lookup} LIMIT 1)`.
- `FORMAT = 3`. Update its doc comment, citing Phase 4 spec §4.
- `verify` must also require the output index. Add `out_index(name)` to its `needed` list, checked as an index. Add an `index_exists` helper, or generalize `table_exists`, so that a dropped index reports "its shadow index … is missing" and the view is broken but droppable. Add that case to `a_broken_view_reports_why_and_can_still_be_dropped`.

- [ ] **Step 4: Run to verify they pass**

Run `scripts/test-all.sh`. All tests must pass.

- [ ] **Step 5: Gate rows**

First add one end-to-end test to `extension_lifecycle.rs`, `a_view_creates_its_output_index`. After `create`, `sqlite_schema` must hold an index named `__ivm_outidx_<view>` on `__ivm_out_<view>` whose `sql` names every output column. The plan unit test builds its own index, so it cannot see the extension's `CREATE INDEX`.

Then add and verify three rows:
1. Delete the `CREATE INDEX` in `create_out_table`: `a_view_creates_its_output_index` goes red.
2. Make the lookup defeat the index, e.g. make `retraction_lookup` compare the first column as `+<col> IS <operand>`: the plan unit test goes red. Name the exact mutation in the row.
3. `FORMAT = 2`: the format test goes red.

- [ ] **Step 6: Full checks and commit**

```bash
git add crates/ivmlite-sqlite/src/view.rs crates/ivmlite-sqlite/src/names.rs crates/ivmlite-test/tests/extension_lifecycle.rs crates/ivmlite-test/tests/extension_sharing.rs docs/mutation-gates.md
git commit -m "perf(sqlite): index the output table so a retraction is a search, not a scan"
```

Record this commit's hash in the report: it is the ablation's "after §4" build.

---

### Task 4: One scan for the latch (spec §5)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs`
- Test: `crates/ivmlite-test/tests/extension_replace.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- `Fingerprint::changed(&self, table: &str) -> String` keeps its signature and meaning: a condition that holds when the capture differs.

- [ ] **Step 1: Write the failing tests**

In `extension_replace.rs`:

```rust
/// Phase 4 spec §5: the latch is one aggregate over sqlite_schema. With the
/// table renamed away the scan finds no rows, and the latch must still fire
/// (count() is 0 on empty input where sum() would be NULL).
#[test]
fn the_latch_fires_when_the_scan_finds_no_row_for_the_table() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x INTEGER) STRICT; INSERT INTO t VALUES ('a', 1);")
        .unwrap();
    create(&c, "ks", KS).unwrap();
    c.execute_batch("ALTER TABLE t RENAME TO u; INSERT INTO u VALUES ('b', 2); ALTER TABLE u RENAME TO t;")
        .unwrap();
    let err = refresh(&c, "ks").expect_err("the write while renamed away latched");
    assert!(err.to_string().contains("changed after its capture was generated"), "{err}");
}

/// Phase 4 spec §5: one scan of sqlite_schema per written row, not four.
#[test]
fn the_latch_reads_sqlite_schema_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT UNIQUE, x INTEGER) STRICT;").unwrap();
    create(&c, "ks", KS).unwrap();
    let body: String = c
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = '__ivm_trig_t_preins'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body.matches("sqlite_schema").count(), 1, "{body}");
}
```

Also extend `ivmlite_triggers_use_no_aggregate_order_by` to reject `FILTER (` and ` OVER (`, case-insensitively.

- [ ] **Step 2: Run to verify they fail**

After `scripts/build-extension.sh`, run `cargo test -p ivmlite-test --test extension_replace --locked -- latch`. `the_latch_reads_sqlite_schema_once` fails: the body contains 4 or more occurrences.

- [ ] **Step 3: Implement**

Rewrite `Fingerprint::changed` as a single aggregate. `<t>` is `literal(table)`, the trigger names are the five `literal(&trigger(table, event))` strings, and `<n>` is `self.unique_indexes.len()`:

```sql
(SELECT
   count(CASE WHEN type = 'table' AND name = <t> COLLATE NOCASE THEN 1 END) <> 1
   OR count(CASE WHEN type = 'table' AND name = <t> COLLATE NOCASE AND sql IS <table literal> THEN 1 END) <> 1
   OR count(CASE WHEN type = 'trigger' AND name IN (<five names>) THEN 1 END) <> 5
   OR count(CASE WHEN type = 'index' AND sql LIKE 'CREATE UNIQUE INDEX%' THEN 1 END) <> <n>
   OR count(CASE WHEN type = 'index' AND sql LIKE 'CREATE UNIQUE INDEX%' AND sql IN (<recorded>) THEN 1 END) <> <n>
 FROM sqlite_schema WHERE tbl_name = <t> COLLATE NOCASE)
```

- When `<n> = 0`, leave out the last line: `IN ()` is invalid.
- `count(CASE … END)` returns 0 on empty input, so a renamed-away table makes the first line true.
- The result is a single boolean expression. Keep the existing doc comment's reasoning, and add a paragraph on the single scan and the empty-input rule.
- `table_row`, `unique_index_rows` and `capture_trigger_rows` stay for `Fingerprint::read`, which runs at track time. Remove any helper left unused.

- [ ] **Step 4: Run to verify they pass**

Run `scripts/test-all.sh`. Every Phase 3b latch, decoy and swap test, and the new ones, must pass.

Then repeat the manual old-SQLite check. With `/usr/local/bin/python3.11` (SQLite 3.42), open a database that the current extension created (with a view and a unique index), then read and write the base table without the extension. Record the exact command and its output in the task report. If that interpreter is missing, say so in the report.

- [ ] **Step 5: Gate rows**

Re-verify every existing latch gate row against the new query, editing each row's mutation text to name the new code. Add new rows:
- turn `count(CASE …)` for the table row into `sum(CASE … THEN 1 ELSE 0 END)` wrapped so that an empty input yields NULL (e.g. `sum(CASE … THEN 1 END)`): `the_latch_fires_when_the_scan_finds_no_row_for_the_table` goes red;
- revert to the four-subquery form (the Phase 3b code): `the_latch_reads_sqlite_schema_once` goes red.

- [ ] **Step 6: Full checks and commit**

```bash
git add crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/tests/extension_replace.rs docs/mutation-gates.md
git commit -m "perf(sqlite): read sqlite_schema once per written row in the latch"
```

Record this commit's hash: it is the ablation's "after §5" build.

---

### Task 5: The write-amplification workload and runner (spec §7)

**Files:**
- Modify: `crates/ivmlite-workload/src/lib.rs`, `crates/ivmlite-bench/src/main.rs`, `crates/ivmlite-bench/src/baseline.rs`
- Create: `workloads/write-amp.toml`, `crates/ivmlite-bench/src/write_amp.rs`

**Interfaces:**
- Produces, in `ivmlite-workload`:

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum WriteOpKind { Insert, Delete, Update, ReplaceRowid, ReplaceUnique, ReplaceTwo }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum WriteOp {
      Insert { id: i64, email: String, handle: String, region: String, amount: i64 },
      Delete { id: i64 },
      Update { id: i64, region: String, amount: i64 },
      /// INSERT OR REPLACE of a full row; `expect_removed` rows are removed by conflict.
      Replace { id: i64, email: String, handle: String, region: String, amount: i64, expect_removed: usize },
  }
  pub struct WriteAmpWorkload { pub name: String, pub seed: u64, pub schema: WorkloadSchema,
      pub base_rows: usize, pub group_cardinality: usize, pub amount_max: i64,
      pub view_counts: Vec<usize>, pub ops: Vec<WriteOpKind>, pub rows_per_op: usize,
      pub tx_rows: usize, pub recursive_triggers: Vec<bool>, pub view_threshold_stride: i64,
      pub view_threshold_modulus: i64 }
  impl WriteAmpWorkload {
      pub fn load(path: &Path) -> Result<Self, WorkloadError>;
      /// Base rows: id i, email "e{i}@x", handle "h{i}", region "r{i % card}", amount seeded.
      pub fn rows(&self) -> impl Iterator<Item = (i64, String, String, String, i64)> + '_;
      /// The deterministic trace for one op kind, against the base rows only (each
      /// kind's trace starts from a freshly seeded table).
      pub fn trace(&self, kind: WriteOpKind) -> Vec<WriteOp>;
      pub fn views(&self, n: usize) -> Vec<ViewSpec>;  // m0 shape over the accounts table
  }
  ```

  `ViewSpec::sql` stays m0-shaped (`region`, `amount`), and `accounts` has both columns.
- The traces, for `rows_per_op = M`: each targets distinct existing rows, drawn with the seeded RNG without replacement, so every op hits a live row.
  - **`Insert`:** new ids from `base_rows`, with fresh emails and handles.
  - **`Delete` and `Update`:** distinct existing ids.
  - **`ReplaceRowid`:** an existing id `a`, with a fresh email and handle; `expect_removed = 1`.
  - **`ReplaceUnique`:** a new id, and the email of an existing row `a`; `expect_removed = 1`.
  - **`ReplaceTwo`:** a new id, the email of existing row `a` and the handle of a different existing row `b`; `expect_removed = 2`.

  No two ops in one trace touch the same existing row.

- [ ] **Step 1: Write the failing tests** (in `ivmlite-workload`)

```rust
fn tiny() -> WriteAmpWorkload {
    WriteAmpWorkload::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/write-amp.toml"))
        .unwrap()
        .with_sizes(1_000, 50)   // base_rows, rows_per_op: a test-only helper
}

#[test]
fn traces_are_deterministic_and_hit_distinct_live_rows() {
    let w = tiny();
    for kind in [WriteOpKind::Delete, WriteOpKind::Update, WriteOpKind::ReplaceRowid,
                 WriteOpKind::ReplaceUnique, WriteOpKind::ReplaceTwo] {
        let t = w.trace(kind);
        assert_eq!(t, w.trace(kind), "{kind:?} is not deterministic");
        assert_eq!(t.len(), 50);
        let mut touched = std::collections::BTreeSet::new();
        for op in &t {
            let hit: Vec<i64> = match op {
                WriteOp::Delete { id } | WriteOp::Update { id, .. } => vec![*id],
                WriteOp::Replace { id, email, handle, expect_removed, .. } => {
                    let mut ids = Vec::new();
                    if (*id as usize) < 1_000 { ids.push(*id); }
                    if let Some(a) = email.strip_prefix('e').and_then(|s| s.strip_suffix("@x")) { ids.push(a.parse().unwrap()); }
                    if let Some(b) = handle.strip_prefix('h') { ids.push(b.parse().unwrap()); }
                    ids.dedup();
                    assert_eq!(ids.len(), *expect_removed, "{op:?}");
                    ids
                }
                WriteOp::Insert { .. } => vec![],
            };
            for id in hit {
                assert!(id < 1_000 && touched.insert(id), "{kind:?} reuses or misses row {id}");
            }
        }
    }
}
```

(A fresh email or handle must never parse as an existing row, e.g. use `n{id}@x` and `m{id}`.)

In `ivmlite-bench/src/write_amp.rs`: add a test that runs a tiny workload (`base_rows = 300`, `rows_per_op = 20`, views `[0, 2]`, both modes) for all three engines against the debug extension. It asserts every per-op effect check passes, and that `ivmlite`'s views match the oracle after the refresh.

- [ ] **Step 2: Run to verify they fail**

`cargo test -p ivmlite-workload -p ivmlite-bench --locked` fails to compile.

- [ ] **Step 3: Implement**

- **`workloads/write-amp.toml`:**
  - `schema.ddl` is `accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT`;
  - `base_rows = 100000`, `group_cardinality = 1000`, `amount_max = 200`;
  - `view_counts = [0, 1, 10, 50, 200]`;
  - `ops = ["insert", "delete", "update", "replace_rowid", "replace_unique", "replace_two"]`;
  - `rows_per_op = 1000`, `tx_rows = 1000`;
  - `recursive_triggers = [false, true]`;
  - a comment block stating the refresh policy (refresh every view once after each op trace, untimed).
- **`baseline.rs`:** add `install_trigger_view_with_update(conn, table, view)`, the M0 hand-written view plus an `AFTER UPDATE` trigger that retracts OLD and adds NEW. `write_amp.rs` uses it; the matrix keeps using `install_trigger_view` unchanged.
- **`write_amp.rs`:** for each `(views, kind, mode, engine)`, with engines ordered by `engine_order` over `[NoMaintenance, HandWrittenTrigger, Ivmlite]` and rotated by the combination index:
  1. open a fresh in-memory database, loading the extension for `Ivmlite`;
  2. seed the base rows and sample space;
  3. bootstrap the views (timed) and sample space;
  4. prepare one statement per `WriteOp` variant, then `PRAGMA recursive_triggers = <mode>`;
  5. **timed apply:** execute the trace in transactions of `tx_rows`. After each op, check its effect using `changes()` and a row count kept outside the timer only when cheap:
     - for `Delete` and `Update`, `execute` must return 1;
     - for a `Replace`, check once per trace, after the timer, that the table's `count(*)` equals the expected net total, `base − Σ expect_removed + M`.

     Sample space.
  6. **untimed:** refresh every view (`Ivmlite` only), verify every view against the oracle (`Ivmlite` and `HandWrittenTrigger` for insert, delete and update; for the replace ops, `HandWrittenTrigger` is cost-only, since its triggers do not see REPLACE deletions with recursive_triggers OFF; say so in a comment), and sample space.
- **CSV** to stdout: `engine,views,op,recursive_triggers,rows,apply_ms,us_per_row,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free`.
- **`main.rs`:** add the `write-amp [--workload workloads/write-amp.toml]` mode, plus `--views <list>` to override the view counts. The ablation script (Task 6) uses that override.

- [ ] **Step 4: Run to verify**

Run `cargo test -p ivmlite-workload -p ivmlite-bench --locked`. Then run the release smoke test: `scripts/bench.sh write-amp --views 0,1` on a scratch copy with `base_rows = 5000` and `rows_per_op = 100`. It must complete, with every check passing.

- [ ] **Step 5: Full checks and commit**

```bash
git add crates/ivmlite-workload/src/lib.rs workloads/write-amp.toml crates/ivmlite-bench/src/write_amp.rs crates/ivmlite-bench/src/baseline.rs crates/ivmlite-bench/src/main.rs
git commit -m "feat(bench): a write-amplification workload with UNIQUE-key REPLACE, UPDATE and both recursive_triggers modes"
```

---

### Task 6: Run everything, chart, and write the analysis

**Files:**
- Modify: `scripts/bench-ablation.sh`, which also runs `write-amp --views 10,200` for each build into `docs/bench/m1b-phase4-ablation-write-amp.csv`
- Modify: `crates/ivmlite-bench/src/plot.rs` and `main.rs`, for the charts
- Create:
  - the data files: `docs/bench/m1b-phase4.csv`, `m1b-phase4-confirm.csv`, `m1b-phase4-ablation.csv`, `m1b-phase4-ablation-write-amp.csv`, `m1b-phase4-write-amp.csv`;
  - the charts: `docs/bench/m1b-phase4-card{10,1000,100000}.svg`, and a speedup-surface chart per group cardinality.
- Modify: `docs/bench/README.md`, `docs/README.md`, the Phase 4 spec (a dated amendment with any deviation), and the parent spec §10 (a short amendment pointing to the results).

**Interfaces:**
- Consumes the modes from Tasks 1, 2 and 5, and the three commit hashes that Tasks 2, 3 and 4 recorded.

- [ ] **Step 1: Charts**

Extend `plot.rs` to read the Phase 4 CSV:
- a line per engine of `apply_ms + maintain_ms` against base rows, at `views = 10, batch = 100`, one chart per cardinality, as M0 did;
- and a speedup chart per cardinality: `naive_recompute(apply+maintain) / ivmlite(apply+maintain)` against base rows, one line per batch size, with horizontal reference lines at 1× and 2×.

Keep the existing M0 plot tests. Add a test that the speedup chart contains the 1× and 2× lines.

- [ ] **Step 2: Run the measurements**

Run them in one session, in this order, on an otherwise idle machine. Record the machine, the OS and each run's wall time.
1. `scripts/bench.sh matrix > docs/bench/m1b-phase4.csv`
2. `scripts/bench.sh confirm --from docs/bench/m1b-phase4.csv > docs/bench/m1b-phase4-confirm.csv`
3. `scripts/bench-ablation.sh before=<Task 2 hash> index=<Task 3 hash> latch=<Task 4 hash>`, which writes both ablation CSVs.
4. `scripts/bench.sh write-amp > docs/bench/m1b-phase4-write-amp.csv`
5. Generate the charts, via the bench's `plot --from …` mode.

If a run aborts on a verification mismatch, **stop and report BLOCKED with the cell**: that is a correctness bug, not a benchmark result.

- [ ] **Step 3: Write the analysis**

Add an "M1b Phase 4" section to `docs/bench/README.md`. It must cover:
- **The setup:** machine, SQLite version (bundled), release builds, the protocol (spec §3.2 and §3.3), and the refresh policy caveat (a slice at one refresh per batch).
- **The ablation table:** the median and min–max for each build and cell, plus the latch's effect on µs per written row at 10 and 200 views, from the ablation write-amp CSV.
- **The §10.2 bars:**
  - where ivmlite ≪ recompute holds, and where it does not;
  - how close it is to hand-written triggers;
  - whether it beats them anywhere, citing confirmed medians and ranges only.
- **The §10.4 test:** state explicitly whether a region with speedup > 2 exists, and exactly where, backed by confirmed cells. Report the regions where ivmlite loses just as plainly.
- **Write amplification,** per op and mode: the multiple over `no_maintenance`, and the extra µs per row. Include the REPLACE rows and how the cost grows with views.
- **Space amplification:** bytes at each sample point, against the base.
- **Limits:**
  - one exploration run per cell;
  - only the cells listed are confirmed;
  - uniform data;
  - in-memory databases.

Add a dated amendment to the parent spec §10 ("Amended 2026-09-28, M1b Phase 4: results in docs/bench/README.md, M1b Phase 4 section"). Add the Phase 4 spec, the plan and the data to `docs/README.md`.

- [ ] **Step 4: Checks and commit**

Run the Global Constraints checks, `python3 scripts/count-mutation-gates.py`, and the CJK grep (`LC_ALL=en_US.UTF-8 git grep -nP '[\x{4e00}-\x{9fff}]' -- crates docs scripts .github ':!docs/cases'`, which must print nothing).

```bash
git add scripts/bench-ablation.sh crates/ivmlite-bench/src/plot.rs crates/ivmlite-bench/src/main.rs docs/bench/ docs/README.md docs/superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md docs/superpowers/specs/2026-09-18-ivmlite-design.md
git commit -m "docs(bench): M1b Phase 4 results — the extension against the M0 baselines"
```
