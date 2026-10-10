# M1b Phase 5: Refresh Performance — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the extension's refresh cost in three ways:
- skip the catalog checks when the schema is unchanged;
- apply each refresh with set-based statements fired once (format 4);
- empty the stage after bootstrap.

Then make one profile-driven change, and measure everything with Phase 4's harness.

**Architecture:** All extension changes are in `crates/ivmlite-sqlite/src/{view.rs,vtab.rs}`. The measurement reuses `ivmlite-bench` and `scripts/bench-ablation.sh` unchanged, apart from one new ablation cell and a Phase 5 tables script.

**Tech Stack:** Rust 1.95, rusqlite 0.40, SQLite ≥ 3.45 in the test host. The trigger SQL must still parse on SQLite < 3.44.

**Spec:** `docs/superpowers/specs/2026-10-08-m1b-phase5-refresh-performance-design.md` (binding). The protocol comes from `docs/superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md` §3.

## Global Constraints

- **English only** in every file, comment, error message, doc and commit message.
- **Extension conventions.** Phase 3a §4–§5 and Phase 3b still hold:
  - every identifier is quoted;
  - statements outside trigger bodies are qualified with `"main"`, while names inside trigger bodies stay unqualified;
  - the arming statement is the last statement of every refresh apply;
  - every callback runs inside `guard`;
  - a broken view still connects and can be dropped.
- **Trigger SQL compatibility.** No trigger SQL may use an aggregate `ORDER BY`, `FILTER`, window functions or `group_concat`. Databases must stay readable by SQLite < 3.44.
- **Tables generated with a reserved alias.** Any generated subquery that could be captured by a user's column or table name uses a reserved `__ivm_` alias.
- **No new dependencies.**
- **Building and testing.**
  - Rebuild the extension (`scripts/build-extension.sh`) before running any `ivmlite-test` test.
  - Never build a copy of the repository into the repository's `target/`.
  - The extension crate cannot open a `rusqlite::Connection` in its own tests. Put SQL-executing tests in `crates/ivmlite-test`.
- **Before every commit**, run all of these:
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
  - `scripts/test-all.sh`
- **Mutation gates.** Every new check or behavior gets a row in `docs/mutation-gates.md`, in the right section. For each row: mutate the code, run `scripts/test-all.sh`, record `N passed / M failed (baseline X/0)` and the red tests, restore the code, and rebuild. Afterwards `python3 scripts/count-mutation-gates.py` must print `consistent`. Comments cite spec sections, never ledger rulings.
- **Git.**
  - Stage specific files only.
  - Never use `--no-verify`.
  - End each commit message with the implementer's own model's `Co-Authored-By` line.
  - Never merge or push.

---

### Task 1: Schema-version-gated checks (spec §3)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/vtab.rs`, `crates/ivmlite-sqlite/src/view.rs`
- Test: `crates/ivmlite-test/tests/extension_lifecycle.rs`, or a new `crates/ivmlite-test/tests/extension_refresh_cache.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces, in `view.rs`:

  ```rust
  /// What a view's last passing catalog check saw (spec §3): the schema
  /// cookie, and its base tables' schemas in `view.tables` order.
  pub struct Checked { pub schema_version: i64, pub schemas: Vec<Schema> }
  pub fn schema_version(conn: &Connection) -> Result<i64>;   // PRAGMA schema_version
  /// `refresh` now takes the per-connection cache and updates it.
  pub fn refresh(conn: &Rc<Connection>, name: &str, view: &CompiledView, checked: &mut Option<Checked>) -> Result<()>;
  ```

- `IvmTab` gains a field `checked: Option<Checked>`:
  - `create` fills it after bootstrap;
  - `connect` fills it when `verify` passes;
  - a broken view leaves it `None`.

- [ ] **Step 1: Write the failing tests**

```rust
/// Phase 5 spec §3: schema changes made after the last check still break the
/// next refresh, whichever connection makes them, and with or without the
/// extension loaded on it.
#[test]
fn a_schema_change_from_any_connection_breaks_the_next_refresh() {
    for (who, ddl, expected) in [
        ("same", "DROP TRIGGER __ivm_trig_orders_ins", "__ivm_trig_orders_ins is missing"),
        ("other", "DROP INDEX __ivm_outidx_sums", "__ivm_outidx_sums is missing"),
        ("plain", "CREATE UNIQUE INDEX uq ON orders(amount)", "changed"),
    ] {
        let file = TempFile::new(&format!("cache-{who}"));
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        refresh(&c, "sums").unwrap(); // the cache is warm
        match who {
            "same" => c.execute_batch(ddl).unwrap(),
            "other" => open_with_extension(Some(file.path())).unwrap().execute_batch(ddl).unwrap(),
            _ => Connection::open(file.path()).unwrap().execute_batch(ddl).unwrap(),
        }
        let err = refresh(&c, "sums").expect_err(ddl);
        assert!(err.to_string().contains(expected), "{who} / {ddl}: {err}");
    }
}

/// The latch is data, not schema: a latched table breaks the next refresh
/// even though the schema cookie did not move.
#[test]
fn a_latch_set_without_a_schema_change_breaks_the_next_refresh() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("UPDATE __ivm_tracked SET broken = 'test latch' WHERE tbl = 'orders'").unwrap();
    let err = refresh(&c, "sums").expect_err("latched");
    assert!(err.to_string().contains("test latch"), "{err}");
}
```

(The latch test writes `__ivm_tracked` directly. That is white-box, and the comment says so. It is the only way to set the latch without also changing the schema.)

- [ ] **Step 2: Run the tests to verify they fail, or to see which already pass**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --locked -- a_schema_change_from_any_connection a_latch_set_without`

Expected: both pass today, since the checks run on every refresh. They pin the behavior the cache must keep. Record that they pass. Then add the test that observes the cache itself, which must fail today:

```rust
/// Phase 5 spec §3: with the schema cookie unchanged, a refresh skips the
/// catalog checks. Observed deterministically through a data-only tampering
/// that only those checks would notice: deleting the view's `__ivm_dep` row
/// (white-box). It goes unnoticed while the schema is unchanged, and the
/// first refresh after any schema change, even a harmless one, reports it.
#[test]
fn an_unchanged_schema_skips_the_catalog_checks() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch("DELETE FROM __ivm_dep WHERE view = 'sums'").unwrap();
    refresh(&c, "sums").expect("the cache hit skips the dependency check");
    c.execute_batch("CREATE TABLE scratch(x INTEGER) STRICT").unwrap();
    let err = refresh(&c, "sums").expect_err("a schema change re-runs the checks");
    assert!(err.to_string().contains("dependency on table orders is not recorded"), "{err}");
}
```

Today the second `refresh` fails, because the checks always run.

- [ ] **Step 3: Implement**

- In `view.rs`, add `Checked` and `schema_version`. `verify` and `create` return the schemas they read.
- `refresh`:

```rust
pub fn refresh(conn: &Rc<Connection>, name: &str, view: &CompiledView, checked: &mut Option<Checked>) -> Result<()> {
    let version = schema_version(conn)?;
    let schemas = match checked {
        Some(c) if c.schema_version == version => c.schemas.clone(),
        _ => {
            *checked = None;
            check_capture(conn, name, view).map_err(|why| broken(name, &why))?;
            check_output_index(conn, name).map_err(|why| broken(name, &why))?;
            let schemas: Vec<Schema> = view.tables.iter().map(|t| base_schema(conn, t)).collect::<Result<_>>()?;
            *checked = Some(Checked { schema_version: version, schemas: schemas.clone() });
            schemas
        }
    };
    for table in &view.tables {
        check_latch(conn, table).map_err(|why| broken(name, &why))?;  // __ivm_tracked.broken, always
    }
    // … the existing delta read, compute and apply, using `schemas` …
}
```

  `check_latch` is the `broken` read that `check_table_capture` already performs, factored out so both use it. A latched table must still report the same message.
- In `vtab.rs`:
  - add `checked: Option<Checked>` to `IvmTab`;
  - in `open_view`, fill it from `create`/`connect`;
  - pass `&mut self.checked` to `view::refresh`.
- `Schema` must be `Clone`. It already is in ivmlite-core; check this, and if it is not, add `#[derive(Clone)]` there.

- [ ] **Step 4: Run to verify**

Run `scripts/test-all.sh`. Every existing broken-view, latch, decoy and rename test must stay green; they are the main guard here.

- [ ] **Step 5: Gate rows**
  - Compare `<=` instead of `==` (or always hit the cache): the DDL tests go red.
  - Skip `check_latch` when the cache hits: the latch test goes red.
  - Keep a stale cache after a failed check: if no test catches it, add one. The case is a failed check, the user fixes nothing, and the second refresh must fail again.

- [ ] **Step 6: Full checks and commit**

`git commit -m "perf(sqlite): skip the catalog checks while the schema cookie is unchanged"`

---

### Task 2: Set-based apply, format 4 (spec §4)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs`
- Test: `crates/ivmlite-test/tests/extension_lifecycle.rs`, `extension_gc.rs`, `extension_sharing.rs`, the format test
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- `create_stage(conn, name, view, ids)` and `apply(conn, name, view, changes)` keep their signatures.
- `FORMAT = 4`.
- `retraction_lookup(out, cols, operand)` is unchanged; its operand becomes `__ivm_s.c{i}`.

- [ ] **Step 1: Adapt and add tests (failing first)**

1. **`the_retraction_lookup_searches_the_output_index`.** Re-target it to the new trigger. It must:
   - extract the lookup from the stored `__ivm_apply_<view>` SQL, from both the RAISE's `NOT EXISTS (…)` and the DELETE's `SELECT (… LIMIT 1)`;
   - assert that the two are byte-identical;
   - substitute `__ivm_s.cN` with `?N+1`, highest `N` first;
   - require a SEARCH that covers every output column.
2. **New test, `one_refresh_fires_the_apply_body_once`.** Before refreshing, install `CREATE TEMP TABLE fired(n INTEGER); CREATE TRIGGER temp.count_apply AFTER INSERT ON main.__ivm_stage_sums WHEN NEW.op = 'apply' BEGIN INSERT INTO fired VALUES (1); END;`. TEMP triggers on main tables are allowed, and the comment says so. Refresh a batch that stages many rows, then assert that `fired` holds exactly 1 row and the view matches the oracle.
3. **Format test.** The stored format tampered to 3 must be refused, with the message naming "format 3"; the broken view still drops.

Today: test 2 fails, because the arming statement is an UPDATE and no 'apply' row exists. Test 1 fails on the new extraction. Test 3 fails because FORMAT is 3.

- [ ] **Step 2: Implement `create_stage`**

Drop `armed` from the stage DDL. The trigger:

```rust
let s = "__ivm_s"; // reserved alias for the stage inside every subquery (spec §4)
let lookup = retraction_lookup(&out, &cols, |i| format!("{s}.c{i}"));
let mut body = Vec::new();
for (i, id) in ids.iter().enumerate() {
    let t = quote(&state_table(name, *id));
    body.push(format!(
        "INSERT INTO {t}(key, val, w) SELECT key, val, w FROM {stage} \
         WHERE op = 'state' AND arr = {i} AND true \
         ON CONFLICT(key, val) DO UPDATE SET w = w + excluded.w;"));
    body.push(format!(
        "DELETE FROM {t} WHERE w = 0 AND (key, val) IN \
         (SELECT key, val FROM {stage} WHERE op = 'state' AND arr = {i});"));
}
body.push(format!(
    "SELECT RAISE(ABORT, 'ivmlite broken invariant: the view retracted a row its output table does not hold') \
     WHERE EXISTS (SELECT 1 FROM {stage} AS {s} WHERE {s}.op = 'out-' AND NOT EXISTS ({lookup}));"));
body.push(format!(
    "DELETE FROM {out} WHERE rowid IN \
     (SELECT ({lookup} LIMIT 1) FROM {stage} AS {s} WHERE {s}.op = 'out-');"));
body.push(format!(
    "INSERT INTO {out}({}, {WEIGHT}) SELECT {}, 1 FROM {stage} WHERE op = 'out+';",
    cols.join(", "), (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>().join(", ")));
body.push(format!(
    "UPDATE {progress} SET applied_seq = \
     (SELECT seq FROM {stage} WHERE op = 'progress' AND tbl = {progress}.tbl) \
     WHERE view = {} AND tbl IN (SELECT tbl FROM {stage} WHERE op = 'progress');",
    literal(name)));
for t in &view.tables {
    body.push(format!(
        "DELETE FROM {delta} WHERE {DELTA_SEQ} <= (SELECT MIN(applied_seq) FROM {progress} WHERE tbl = {lit}) \
         AND EXISTS (SELECT 1 FROM {stage} WHERE op = 'progress' AND tbl = {lit});",
        delta = quote(&delta_table(t)), lit = literal(t)));
}
// CREATE TABLE stage(op TEXT NOT NULL, arr INTEGER, key BLOB, val BLOB, w INTEGER, tbl TEXT, seq INTEGER, c0 …);
// CREATE TRIGGER "main"."__ivm_apply_<view>" AFTER INSERT ON <stage> WHEN NEW.op = 'apply' BEGIN … END;
```

Keep the comments: the unqualified trigger body, the order of statements, and why each one is set-based. Add a comment on the duplicate-retraction impossibility (spec §4).

- [ ] **Step 3: Implement `apply`**

- Replace the final `UPDATE {stage} SET armed = 1` with `INSERT INTO {stage}(op) VALUES ('apply')`, keeping it the last statement.
- Update the doc comment: still one statement, still atomic, and the stage keeps its rows, now including the `apply` row, until the next apply empties it.
- `FORMAT = 4`, with the doc citing Phase 5 spec §4.
- Update the Phase 3a/3b spec references in comments where they describe the arming `UPDATE`.

- [ ] **Step 4: Run to verify**

Run `scripts/test-all.sh`. Every atomicity test must pass unchanged:
- `a_failed_refresh_changes_nothing_and_a_retry_succeeds`;
- `nothing_after_the_apply_can_fail_a_refresh`;
- `a_failed_gc_changes_nothing_and_a_retry_succeeds`;
- the broken-view tests;
- the differential sweeps, including the reopen and sibling modes.

- [ ] **Step 5: Gate rows**
  - Remove the trigger's `WHEN`: `one_refresh_fires_the_apply_body_once` goes red, and possibly every test does, since data rows would apply.
  - Drop the state `DELETE … w = 0`: some state test or differential goes red. Name it.
  - Drop the RAISE: the missing-retraction invariant test goes red. Find or add the test that stages an `out-` with no matching row, white-box.
  - The alias: write the lookup's operand as unaliased `c{i}`, and a test with an output column named like a stage column goes red. Add a test with result columns named `op`, `key` and `c0` if none exists.
  - Set `FORMAT = 3`: the format test goes red.

- [ ] **Step 6: Full checks and commit**

`git commit -m "perf(sqlite): apply a refresh with set-based statements fired once (format 4)"`

---

### Task 3: Bootstrap empties its stage (spec §5)

**Files:** `crates/ivmlite-sqlite/src/view.rs`, `crates/ivmlite-test/tests/extension_lifecycle.rs`, `docs/mutation-gates.md`

- [ ] **Step 1: Failing tests**

```rust
/// Phase 5 spec §5: bootstrap leaves no staged rows behind.
#[test]
fn bootstrap_leaves_the_stage_empty() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM __ivm_stage_sums"), 0);
}

// A failing CREATE is rolled back as a whole; `creating_a_view_is_atomic` (Phase 3a)
// already pins that, so the new DELETE needs only its gate row (Step 3).
```

- [ ] **Step 2: Implement**

At the end of `create`, after `apply(…)?`, add:

```rust
// Spec §5: unlike a refresh, a failing CREATE VIRTUAL TABLE is rolled back as a
// whole — it writes sqlite_schema — so emptying the stage after the bootstrap's
// apply cannot leave an applied-but-reported-failed state (Phase 3a §5).
exec(conn, &format!("DELETE FROM {}", main_qualified(&stage_table(name))))?;
```

Correct every comment and doc that says bootstrap leaves the stage full. This includes the test comment in `nothing_after_the_apply_can_fail_a_refresh`, whose setup refresh may now be unnecessary; keep the test's meaning intact.

- [ ] **Step 3: Verify, gate, commit**

- **Gate:** delete the new `DELETE`, and `bootstrap_leaves_the_stage_empty` goes red.
- **Checks:** run the full Global Constraints checks.
- **Commit:** `git commit -m "perf(sqlite): empty the stage at the end of bootstrap"`

---

### Task 4: One profile-driven change (spec §6)

**Files:**
- Create: `scripts/profile-refresh.sh`, and a small Python or Rust driver the script runs
- Modify: depends on the finding, `crates/ivmlite-sqlite/src/{view.rs,state.rs}`
- Modify: the Phase 5 spec, with a dated §6 amendment

- [ ] **Step 1: Profile**

Write `scripts/profile-refresh.sh`. It must:
1. build the release extension;
2. run a steady refresh workload: 10 views, 100,000 base rows, 1,000 groups, refreshed after every 1,000-row insert, for 30 s;
3. sample it for 10 s with macOS `sample`, or `perf` on Linux;
4. print the inclusive sample counts of the `ivmlite_sqlite::`/`ivmlite_core::` functions under `view::refresh`.

The spike that motivated this phase did exactly this; its method is in spec §2. Run it, and record the top entries in your report.

- [ ] **Step 2: Decide by spec §6's rule**

The single largest remaining cost of at least 20% of `view::refresh` selects the remedy:
- **Stage inserts** (`view::apply` outside the arming statement): multi-row VALUES inserts with a fixed chunk of 64 rows, plus a remainder statement, all prepared and cached.
- **`BufferedArrangement::get`:** prefetch a batch's keys per arrangement with one `SELECT … WHERE key IN (…)` query, chunked to a bounded parameter count. Overlay semantics must not change.
- **Anything else, or nothing at 20% or more:** make no code change. Write the amendment and stop.

- [ ] **Step 3: Implement (if a remedy applies), with tests and gates**

Existing tests guard correctness; the change is pure performance. Add one targeted test for the new code's edge: a chunk boundary, i.e. exactly 64 and 65 staged rows, or a prefetch over more keys than one chunk holds. Add a gate row that breaks the edge.

- [ ] **Step 4: Amend the spec and commit**

Add a dated amendment to Phase 5 spec §6 with the profile's top entries, the choice, and its reason. Commit the script, any code, and the amendment.

---

### Task 5: Measure, report, and prepare the release (spec §7)

**Files:**
- Modify: `workloads/m0-baseline.toml` (`[ablation]`: add the cell views=200, base_rows=100000, batch=1, group_cardinality=1000)
- Create:
  - `docs/bench/m1b-phase5.csv`, `docs/bench/m1b-phase5-confirm.csv`, `docs/bench/m1b-phase5-ablation.csv`, `docs/bench/m1b-phase5-ablation-builds.csv`;
  - `docs/bench/m1b-phase5-demos.csv`, `scripts/bench-demos.sh`;
  - `docs/bench/phase5_tables.py`;
  - the charts.
- Modify: `docs/bench/README.md`, `docs/README.md`, the Phase 5 spec (dated amendment for any deviation), `CHANGELOG.md`
- Create: `docs/releases/v0.1.0-alpha.3.md`

- [ ] **Step 1: The ablation cell.** Add the cell, and check that `AblationSpec::validate` accepts it. If the published Phase 4 test of ablation cells pins exactly four, update it and say why.

- [ ] **Step 2: Run, in one session on an idle machine.** Record the machine, the OS and each wall time.
  1. `scripts/bench-ablation.sh before=<master at Phase 5 start> schemaver=<Task 1> setapply=<Task 2> stage=<Task 3> [profiled=<Task 4>]`, writing the Phase 5 ablation CSVs. The builds CSV records label → commit.
  2. `scripts/bench.sh matrix > docs/bench/m1b-phase5.csv`.
  3. `scripts/bench.sh confirm --from docs/bench/m1b-phase5.csv > docs/bench/m1b-phase5-confirm.csv`.
  4. `scripts/bench-demos.sh before=<master at Phase 5 start> after=<last reviewed Phase 5 commit>`, writing `docs/bench/m1b-phase5-demos.csv`. The script builds each rev's extension in its own worktree and runs the current tree's `fluxflow_demo` and `demand_case_bench` against it through `--extension`, at each demo's documented scale with its default repeats.
  5. Regenerate the charts with `plot --from` into Phase 5 file names.

  If any run aborts on a verification mismatch, stop and report BLOCKED with the cell.

- [ ] **Step 3: `phase5_tables.py`.** It computes every Phase 5 table from the CSVs, as `phase4_tables.py` does, and prints them. It covers:
  - the ablation per change, as medians and ranges;
  - the speedup surface, as counts above 2x and below 1x per base-row size;
  - the confirmed cells;
  - the demo before/after, per demo and mode: steady-state refresh, end to end, and the speedup against `indexed_recompute`;
  - the §10.2 bars against hand-written triggers;
  - **the Phase 4 → Phase 5 change per cell**, read from both matrices.

- [ ] **Step 4: README "M1b Phase 5" section.** Every measured number comes from the script's output. The section has:
  - the setup;
  - the ablation, saying which changes matter and which are indistinguishable;
  - the §10.2 bars again, plainly: does ivmlite now come close to hand-written triggers, where, and where not;
  - what still loses and why;
  - anything that got worse;
  - the limits.

- [ ] **Step 5: Docs and release prep.**
  - **CHANGELOG `[Unreleased]`:** format 4 (breaking: alpha.2 views must be dropped and recreated; they still open as broken views), the refresh changes, and the headline numbers.
  - **`docs/releases/v0.1.0-alpha.3.md`:** written in the style of alpha.2's notes. Leave the version bump to the controller.
  - **`docs/README.md` index:** the Phase 5 spec, the plan, the tables script, and the profiling script.
  - **Parent spec §10:** a dated amendment pointing to the Phase 5 results.

- [ ] **Step 6: Checks and commit.** Run the Global Constraints checks, the gate count and the CJK grep. Then commit the data, the scripts and the docs as separate commits.
