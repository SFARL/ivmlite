# M1b Phase 7: `ivm_refresh_all()` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a SQL function that refreshes every view in one call, either atomically or continuing past failures and reporting them by name. Let any refresh with nothing new return early, after its checks.

**Architecture:**
- **The early return** lives in `view::refresh` (`crates/ivmlite-sqlite/src/view.rs`).
- **The function** lives in a new `crates/ivmlite-sqlite/src/refresh_all.rs`, registered from `lib.rs`. It drives each view's existing refresh through one nested `INSERT INTO v(v) VALUES('refresh')` per view, inside savepoints it opens itself.
- **The measurement** is a committed Python driver, plus a tables script and a README section.

**Tech Stack:** Rust 1.95, rusqlite 0.40 (the extension crate may enable rusqlite's `functions` feature), SQLite ≥ 3.45 in the test host. `SQLITE_DIRECTONLY` needs SQLite 3.30.

**Spec:** `docs/superpowers/specs/2026-10-12-m1b-phase7-refresh-all-design.md` (binding). Phase 6 spec §3's cached-statement rules still bind.

## Global Constraints

- **Language.** English only in every file, comment, error message, doc and commit message.
- **Extension conventions.** Phase 3a §4–§5, 3b, 5 and 6 all hold:
  - every identifier is quoted;
  - statements outside trigger bodies are `"main"`-qualified;
  - the arming statement is the single last statement of every apply;
  - every callback and the function body run inside `guard` (or an equivalent `catch_unwind`), so a panic becomes an SQL error;
  - a broken view still connects and can be dropped.
- **Cached statements (Phase 6 §3).**
  - No cached statement names an ivmlite virtual table, so the function's nested `INSERT INTO v(v)` statements are prepared and finalized per call.
  - Every cached statement is reset before its callback returns.
- **Messages.** Every existing message keeps its exact text. New messages are exactly the spec §2 texts.
- **Dependencies.** No new dependencies. Enabling rusqlite's `functions` feature on `crates/ivmlite-sqlite` is allowed.
- **Build and test.**
  - Run `scripts/build-extension.sh` before any `ivmlite-test` test.
  - Never build a copy into the repository's `target/`.
  - Put SQL-executing tests in `crates/ivmlite-test`.
- **Before every commit**, run:
  - `cargo fmt --all -- --check`
  - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
  - `scripts/test-all.sh`
- **Mutation gates.**
  - Every new check or behaviour gets a row in `docs/mutation-gates.md`: mutate, run `scripts/test-all.sh`, record `N passed / M failed (baseline X/0)` and the red tests, restore, rebuild.
  - `python3 scripts/count-mutation-gates.py` must print `consistent`.
  - Comments cite spec sections, never ledger rulings.
- **Git.**
  - Stage specific files, and never use `--no-verify`.
  - End each commit with the implementer's own model's `Co-Authored-By` line.
  - Never merge or push.

## Review Focus

1. **A panic or error between `SAVEPOINT` and `RELEASE`.** Every path closes the function's savepoint, including a refresh that fails, a skip check that fails to read a delta table, and an out-of-memory formatting the message. After any failure, a following `BEGIN` / `COMMIT` and an `sqlite3_close` (v1) succeed. (Task 2)
2. **A view dropped by another connection between the function reading `__ivm_view` and refreshing that view.** The nested `INSERT` fails with "no such table". Atomic mode fails the call; continue mode reports that view as failed; nothing hangs. (Task 2)
3. **View names that need quoting.** A view named `my view` or `"q"` refreshes through the function, and the continue report prints the name as stored. (Task 2)
4. **The function called twice in one transaction, or nested inside itself through a user function.** A second call inside `BEGIN … COMMIT` works. A call whose nested refresh fires a user trigger that calls `ivm_refresh_all` is refused by `SQLITE_DIRECTONLY`, and the outer call reports that as a failure. (Task 2)
5. **An early return leaves no trace.** A refresh with nothing new returns early, and the next real refresh applies correctly, even though the stage still holds the previous apply's rows. (Task 1)

---

### Task 1: A refresh with nothing new returns early (spec §4)

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs` (`refresh`), `docs/mutation-gates.md`
- Test: `crates/ivmlite-test/tests/extension_lifecycle.rs`, or a new `extension_refresh_skip.rs`

**Interfaces:**
- Produces: no new API. `view::refresh` returns `Ok(())` early when no base table has a delta past the view's watermark.

- [ ] **Step 1: Failing tests**
  - `a_refresh_with_nothing_new_changes_nothing`:
    1. Create a view and refresh it.
    2. Record `total_changes()` and dump `__ivm_stage_<v>` with `quote()` for every column.
    3. Refresh again with no writes in between.
    4. `total_changes()` is unchanged and the stage dump is identical.

    Today's code re-stages the sentinel and re-arms it, so the test fails.
  - `a_skipped_refresh_still_runs_its_checks`. After a first refresh, drop and recreate the base table with the same columns. This drops its capture triggers. The next refresh, with no deltas, fails with today's broken-view message.
  - `the_refresh_after_a_skip_applies_correctly`. Covers Review Focus 5:
    1. Refresh with no writes, so it is skipped.
    2. Write rows.
    3. Refresh again; the view matches the oracle.

- [ ] **Step 2: Implement.** In `refresh`, after the per-table loop has read every table's deltas:

  ```rust
  // Phase 7 spec §4: nothing past any watermark, so nothing to compute,
  // stage or apply. The checks above (`checked_schemas`) already ran, so a
  // view whose capture was lost is reported broken here, never skipped.
  if progress.is_empty() && batches.iter().all(|(_, delta)| delta.is_empty()) {
      return Ok(());
  }
  ```

  Check `ZSet::is_empty`, or the equivalent, in `ivmlite-core`. A delta can be consolidated to empty while its watermark still moves: a row inserted and then deleted. In that case `progress` is non-empty, and the apply must still run so the watermark and GC advance. The condition above covers this, because it needs both.

- [ ] **Step 3: Run the tests**, and the full suite. Any test that counted applies or stage rows after an idle refresh needs adapting; keep its meaning.

- [ ] **Step 4: Gate rows.**
  - Remove the early return: `a_refresh_with_nothing_new_changes_nothing` goes red.
  - Return early before `checked_schemas`, moving the check to the top: `a_skipped_refresh_still_runs_its_checks` goes red.
  - Drop `progress.is_empty() &&`: a test with an insert and a delete of the same row between two refreshes must go red, because the watermark would stop moving. Add that test, `a_consolidated_empty_delta_still_moves_the_watermark`. It checks `__ivm_progress.applied_seq`, and that GC empties the delta table.

- [ ] **Step 5: Commit**
  `git commit -m "perf(sqlite): return early from a refresh with nothing new, after its checks"`

---

### Task 2: `ivm_refresh_all()` (spec §2–§4)

**Files:**
- Create: `crates/ivmlite-sqlite/src/refresh_all.rs`
- Modify:
  - `crates/ivmlite-sqlite/src/lib.rs`: register the function after the module, and add the `mod`;
  - `crates/ivmlite-sqlite/Cargo.toml`: rusqlite `functions`, if that route is used;
  - `crates/ivmlite-sqlite/Cargo.lock`, if the feature changes it;
  - `docs/mutation-gates.md`.
- Test: create `crates/ivmlite-test/tests/extension_refresh_all.rs`.

**Interfaces:**
- **Consumes:** Task 1's early return. Names come from `names.rs`: `VIEWS`, `DEPS`, `PROGRESS`, `delta_table`, `DELTA_SEQ`, `quote`, `main_qualified`.
- **Produces:** the SQL function `ivm_refresh_all`, with 0 or 1 argument. The constants:

  ```rust
  pub const SAVEPOINT: &str = "__ivm_refresh_all";
  pub const BAD_ARGUMENT: &str = "ivm_refresh_all takes 'atomic' or 'continue'";
  pub const NOT_FROM_A_WRITE: &str =
      "ivm_refresh_all must be called from a SELECT, not from a statement that writes";
  ```

- [ ] **Step 1: Failing tests** in `extension_refresh_all.rs`. Use the helpers of the other extension tests: `open_with_extension`, a base table with two views on it, and one view on a second table.

  **Modes:**
  - `atomic_refreshes_every_view_and_counts_them`: after writes to both tables, `SELECT ivm_refresh_all()` returns the number of views refreshed. Every view matches the oracle.
  - `atomic_failure_refreshes_no_view`:
    - Install a `BEFORE INSERT ON __ivm_out_<middle view>` trigger that does `RAISE(ABORT, 'injected')`.
    - The call fails. Its message contains `view <middle>: ` and `no view was refreshed`.
    - Every view's output and `__ivm_progress` are unchanged.
    - Run the test in autocommit, and inside `BEGIN … COMMIT`. After a failure inside the transaction, the transaction still commits.
  - `continue_reports_the_failed_view_and_refreshes_the_rest`:
    - Use the same fault.
    - The returned text equals `refreshed 2, skipped 0, failed 1: <middle>: <its message>`; compare the exact text with the message captured from a direct refresh.
    - The healthy views match the oracle, and the failed one is unchanged.
  - `a_broken_view_fails_atomic_and_is_reported_by_continue`: drop one capture trigger.

  **Where the function can be called:**
  - `called_from_a_writing_statement_is_refused`: `INSERT INTO t SELECT ivm_refresh_all()` fails with `NOT_FROM_A_WRITE`, and no view changed.
  - `called_from_a_trigger_is_refused`: a trigger body calling it fails, which is SQLite's `DIRECTONLY` error.
  - `a_bad_argument_is_refused`: `ivm_refresh_all('x')` and `ivm_refresh_all(1, 2)`.
  - `inside_a_rolled_back_transaction_nothing_is_kept`.
  - `no_views_returns_zero_and_creates_nothing`: a fresh database with the extension loaded and no views returns `0`, and `sqlite_schema` is unchanged.

  **Skipping:**
  - `views_with_nothing_new_are_skipped`: write only to table 1. Atomic returns 2. Continue returns `refreshed 2, skipped 1, failed 0`.
  - `a_recreated_base_table_is_reported_not_skipped`.

  **Lifetime and the Review Focus items:**
  - `close_v1_succeeds_after_every_kind_of_call`: success, atomic failure, continue with a failure, the refused write. Use the raw-handle pattern from `extension_statement_cache.rs`.
  - `a_view_dropped_by_another_connection_is_a_failure` (Review Focus 2):
    - Use a temp-file database.
    - Connection B drops a view while A's call is under way. Do this with a user function that A's first nested refresh triggers. If that is not deterministic, drop the view between A reading `__ivm_view` and A refreshing it, by injecting through a test-only hook. If neither is possible, test the simpler case: the view is dropped by B, then A calls. Say which one you tested.
    - The call does not hang, and reports correctly.
  - `quoted_view_names_refresh` (Review Focus 3): views named `my view` and `q"x`.
  - `two_calls_in_one_transaction` (Review Focus 4).

- [ ] **Step 2: Implement `refresh_all.rs`.** Shape:

  ```rust
  /// Phase 7 spec §2: `ivm_refresh_all([mode])`.
  pub fn register(conn: &Connection) -> rusqlite::Result<()> {
      conn.create_scalar_function(
          "ivm_refresh_all",
          -1,
          FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DIRECTONLY,
          |ctx| { /* guard → run(ctx) */ },
      )
  }

  enum Mode { Atomic, Continue }

  struct View { name: String, tables: Vec<String> }

  /// The views of `main`, by name (spec §3); none when `__ivm_view` is absent.
  fn views(conn: &Connection) -> Result<Vec<View>>;

  /// Spec §4: whether any of `view`'s base tables has a delta past its watermark.
  fn behind(conn: &Connection, view: &View) -> Result<bool>;

  /// One view's nested refresh; prepared and finalized here (spec §3).
  fn refresh_one(conn: &Connection, name: &str) -> Result<()> {
      let sql = format!("INSERT INTO {}({}) VALUES ('refresh')", main_qualified(name), quote(name));
      conn.execute(&sql, []).map(|_| ()).map_err(sql_error)
  }
  ```

  - **Opening the savepoint.** `run` opens `SAVEPOINT "__ivm_refresh_all"` and maps a refusal, any error from it, to `NOT_FROM_A_WRITE`.
  - **Atomic mode:**
    1. Loop over the views. For each one, count it as skipped when it is not `behind`, then call `refresh_one`.
    2. On the first error, run `ROLLBACK TO` then `RELEASE`, and return `format!("ivm_refresh_all: view {name}: {message}; no view was refreshed")`.
    3. Otherwise run `RELEASE` and return the refreshed count as an integer.
  - **Continue mode:** open and release the savepoint per view, rolling it back on that view's error. Collect `(name, message)` and build the spec §2 text.
  - **Never return with the savepoint open.** Structure the code so the release runs on every path. One way: a small guard struct whose `Drop` runs `ROLLBACK TO` + `RELEASE` unless disarmed. `Drop` cannot return errors, so the explicit paths still run them and check their results; the `Drop` is the backstop for `?` and panics.
  - **Messages.** The refresh's own message is the text SQLite reports for the nested `INSERT`. Strip nothing, so it reads exactly as a direct refresh would.
  - **The connection** inside the function is `ctx.get_connection()` (unsafe; document the safety contract). It is the handle of the statement calling the function.

  In `lib.rs`, call `refresh_all::register(conn)?` inside the existing `catch_unwind` closure, after the module.

- [ ] **Step 3: Run the tests and the full suite.**

- [ ] **Step 4: Gate rows.**
  - Drop the `ROLLBACK TO` on the atomic failure path: `atomic_failure_refreshes_no_view` goes red.
  - Drop the per-view rollback in continue mode: the continue test goes red. Its failed view keeps partial or committed writes; the apply is one statement, so look at what actually stays and make the test see it. If the single-statement apply leaves nothing to see, record the row as n/a and say why.
  - Remove `SQLITE_DIRECTONLY`: the trigger test goes red.
  - Skip the `NOT_FROM_A_WRITE` mapping, so the raw SQLite error passes through: the writing-statement test goes red.
  - Count every view as refreshed, with no skip: the skip test goes red.
  - Return early without closing the savepoint on one error path: `close_v1…` or the transaction test goes red.

- [ ] **Step 5: Commit**
  `git commit -m "feat(sqlite): ivm_refresh_all(), atomic or continuing, in one call"`

---

### Task 3: Measure, document, prepare the release (spec §6)

**Files:**
- Create:
  - `scripts/bench-refresh-all.py`;
  - `docs/bench/m1b-phase7-refresh-all.csv` and `docs/bench/m1b-phase7-refresh-all-builds.csv`;
  - `docs/bench/phase7_tables.py`;
  - `docs/releases/v0.1.0-alpha.5.md`.
- Modify:
  - `docs/bench/README.md` (new "M1b Phase 7" section);
  - the root `README.md` (usage section: `ivm_refresh_all`);
  - `CHANGELOG.md` (`[Unreleased]`);
  - `docs/README.md` (index);
  - the parent spec §11 M3 (a dated amendment: `ivm_refresh_all()` shipped).

- [ ] **Step 1: The driver.** `scripts/bench-refresh-all.py <before.dylib> <after.dylib> [--rounds N]`.
  - Python 3.12+ and the standard library, using `sqlite3` with `load_extension(entrypoint=...)`.
  - For each build, in a fresh in-memory database per case: 100,000 rows in 1,000 groups.
  - **Cases:**
    - views 10 and 200, at batch 1 and batch 1,000;
    - the three shapes of spec §6: app-cached per-view refresh, re-prepared per-view refresh, and `SELECT ivm_refresh_all()`;
    - the skip cases: 200 views over 2 base tables, writing one table, then writing none.
  - **Timing.** One warm-up and five measured batches per case. Report the median and min–max of the time to refresh all views. Verify every view against its query after the last batch.
  - **Ordering.** Interleave the builds, so that `before` and `after` alternate per case.
  - **The `before` build** has no `ivm_refresh_all`, so its function rows are absent.
  - **Output.** Write the CSV with a header, and a builds CSV mapping label to commit.
  - Record the load average at the start and the end.

- [ ] **Step 2: Run it.** Build the extension at master's Phase 7 start (`before`) and at Task 2's commit (`after`), each in a scratch `CARGO_TARGET_DIR`. Run under `caffeinate -i`.

- [ ] **Step 3: `phase7_tables.py`.** Standard library only. It prints:
  - per case, the three shapes side by side;
  - `ivm_refresh_all` against the app-cached loop;
  - the skip savings;
  - the per-view cost of re-preparing the nested statements.

  Its label self-check fails with `sys.exit(1)`.

- [ ] **Step 4: Docs.** Every number comes from the script.
  - **The bench README "M1b Phase 7" section** gives the setup, the shapes, what the function costs or saves, the skip effect, and the limits:
    - it is a different driver from the matrix;
    - the nested statements are re-prepared, and why;
    - the machine load.
  - **The root README usage section.** Show `SELECT ivm_refresh_all();` and `SELECT ivm_refresh_all('continue');` with their return values. State that it must be called from a `SELECT`, and the atomic/continue semantics.
  - **CHANGELOG `[Unreleased]`.** Added: `ivm_refresh_all`. Changed: an idle refresh returns early. Include the headline numbers.
  - **`docs/releases/v0.1.0-alpha.5.md`.** Write it in alpha.4's style, and leave the version bump to the controller. The format is unchanged (5), so it carries no breaking note.
  - **Dated amendments.** Index the spec, the plan, the driver and the tables script in `docs/README.md`, and add a dated amendment to the parent spec §11 M3.

- [ ] **Step 5: Checks and commits.**
  - Run the Global Constraints checks, the gate count and the CJK grep.
  - Commit the data and scripts separately from the docs.
