# M2a Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish M2a, the bounded non-uniform workload validation, from the in-progress implementation already on this branch, then run it and publish the results.

**Architecture:** The workload, trace, generator, preflight, correctness/state replay, timed adapters, space capture and a resumable runner binary already exist (merged from `codex/m2a-validation`). An audit against the spec (2026-10-17) found 47 of 69 requirements done. This plan closes the remaining 22: one spec amendment, the targeted fixture suite, loss accounting, plan and provenance gates, workload-file parameters, the derived report, the confirmation workflow and mutation-gate registration. It then runs the matrix and publishes.

**Tech Stack:** Rust 2021 workspace (`ivmlite-workload`, `ivmlite-bench`), rusqlite, serde/serde_json/sha2 (bench-only), the release `ivmlite-sqlite` extension.

**Spec:** `docs/superpowers/specs/2026-10-15-m2a-nonuniform-validation-design.md`

## Global Constraints

- M2a validates the current engine. Add no SQL operators, leave shadow-table format 5 unchanged, and make no performance optimization (spec §1, §6 last paragraph). A bug that is found gets its own regression test and rerun.
- Do not modify `crates/ivmlite-sqlite`, `crates/ivmlite-core` or `crates/ivmlite-sql` source.
- Existing TOML workloads, deterministic M1b traces and legacy CSV readers stay valid (spec §1).
- Everything in the repo is English: code, comments, error strings, docs, commit messages.
- serde, serde_json and sha2 stay dependencies of `ivmlite-bench` (and `ivmlite-test` for serde_json) only; the shipped extension never links them.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Stage specific files, never `git add -A`. Never `--no-verify`.
- **Test commands.** Use `CARGO_INCREMENTAL=0` (the incremental cache gave a reproducible link error for the `m2a` test binary). If extension tests panic "older than its sources", run `cargo clean -p ivmlite-sqlite --manifest-path crates/ivmlite-sqlite/Cargo.toml` and then `scripts/build-extension.sh`; it is a stale-mtime artefact, not a failure.
- Gates before every commit: `CARGO_INCREMENTAL=0 cargo test -p ivmlite-bench -p ivmlite-workload --locked`, `cargo clippy -p ivmlite-bench -p ivmlite-workload --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`. The last task runs `scripts/test-all.sh`.
- Abbreviations used below for existing files: `W` = `crates/ivmlite-workload/src/m2a.rs`, `R` = `crates/ivmlite-bench/src/m2a_runner.rs`, `S` = `crates/ivmlite-bench/src/m2a_state.rs`, `SP` = `crates/ivmlite-bench/src/m2a_space.rs`, `TC` = `crates/ivmlite-bench/src/m2a_tracecheck.rs`, `B` = `crates/ivmlite-bench/src/bin/m2a.rs`. Line numbers are as of commit 48cc3d8 and will drift.

## Rulings already made (controller)

- **Stage rows after refresh (spec §5).** Format 5 empties the stage at the start of the next apply, not at the end of the current one (`crates/ivmlite-sqlite/src/view.rs` `apply`, Phase 5 §4 and Phase 6). M2a cannot change format 5. So the spec is amended (Task 1): pending deltas must be empty, and the stage may hold exactly the last applied batch, marked by one `apply` sentinel; its row count is reported as retained space. `validate_consumed` already implements this; Task 1 pins it with a test and reports the count.
- **M2b preflight evidence and community docs** that arrived with the merge stay on the branch untouched; M2a does not depend on them.

---

### Task 1: Stage-row rule — spec amendment, test and retained-space field

**Files:**
- Modify: `docs/superpowers/specs/2026-10-15-m2a-nonuniform-validation-design.md` (§5, paragraph beginning "One view has consumed every delta")
- Modify: `S` (`validate_consumed`, the `Snapshot` doc comment)
- Modify: `SP` if the space checkpoint does not already carry stage rows
- Test: `S` tests module

**Interfaces:**
- Produces: every state/space checkpoint record has `stage_rows: u64` and `retained_stage_rows: u64` (equal after a refresh, the latter 0 when no batch has been applied since the last emptying). Task 6 reads `retained_stage_rows`.

- [ ] **Step 1: Amend the spec.** Replace "pending deltas and stage rows must be empty;" with: "pending deltas must be empty. The stage table may hold exactly the last applied batch, marked by one `apply` sentinel: format 5 empties the stage at the start of the next apply (Phase 5 §4), and M2a does not change the format. Its row count is reported as retained space beside the other state, never as a pending delta." Add a dated line `Amendment 2026-10-17: stage rows after refresh (see §5).` under the Status line.
- [ ] **Step 2: Write failing tests** in `S`'s test module:
  - `stage_holds_exactly_the_last_applied_batch_after_refresh`: on a tiny join fixture, apply a batch of 3 operations, refresh, take a snapshot; assert `pending` is all zero, `stage_operation_counts["apply"] == 1`, and `retained_stage_rows == stage_rows > 0`; refresh again with no writes and assert `retained_stage_rows` did not grow.
  - `validate_consumed_rejects_a_stage_without_an_apply_sentinel`: build a `Snapshot` by hand with `stage_rows = 2`, `stage_operation_counts = {"state": 2}` and assert `validate_consumed` returns an error containing `not already applied`.
  - `validate_consumed_rejects_pending_deltas`: a `Snapshot` with one pending entry with `delta_rows = 1` must fail.
- [ ] **Step 3: Run** `CARGO_INCREMENTAL=0 cargo test -p ivmlite-bench --locked stage_ validate_consumed` and confirm the first test fails to compile or fails (no `retained_stage_rows`).
- [ ] **Step 4: Implement** `retained_stage_rows` on the snapshot and in the space/state JSON checkpoint output. Update the `validate_consumed` doc comment to cite the amended §5.
- [ ] **Step 5: Run the tests and gates**, then commit: `git add <the files> && git commit -m "m2a: pin the retained-stage rule and report retained stage rows"`.

### Task 2: Targeted correctness fixture suite (spec §3, last paragraph; AC1)

**Files:**
- Create: `crates/ivmlite-bench/src/m2a_fixtures.rs` (test-only module: `#[cfg(test)] mod m2a_fixtures;` in `crates/ivmlite-bench/src/lib.rs`)
- Read for reuse: `S` (snapshot and validation helpers), `R` (connection setup with the extension), `crates/ivmlite-bench/src/demo_bench.rs` (`sorted_rows`, `check_same_multiset`)

**Interfaces:**
- Consumes: the existing helpers that open an in-memory connection with the debug extension, create the M2a single/join schema and view, run a refresh, and take/validate a `Snapshot` (`validate_snapshot`/`validate_idle` or their current names in `S`).

Each case runs on the join schema of spec §2 (`lhs(id, k, g)`, `rhs(id, k, amount)`, both STRICT, ordinary indexes on `k`, view `SELECT lhs.g, COUNT(*), SUM(rhs.amount) FROM lhs JOIN rhs ON lhs.k = rhs.k GROUP BY lhs.g`) unless it says single. NULL-key cases use a variant schema without `NOT NULL` on `k`. For every case, after every refresh, assert three things: (a) the view's rows equal the **explicit expected rows written in the test**, sorted; (b) they equal SQLite's full recomputation of the view SQL as a multiset; (c) the state snapshot validates (decoded arrangements equal the live bases, pending empty). Write a helper `fn check(conn, expected: &[(i64, i64, Option<i64>)])` doing all three.

- [ ] **Step 1: Write the cases**, each a `#[test]` with its seed rows, writes and expected rows spelled out:
  1. `lhs_join_key_update_moves_matches` — lhs `(1,10,1)`, rhs `(1,10,5),(2,20,7)`; expected `[(1,1,5)]`; `UPDATE lhs SET k=20 WHERE id=1`; expected `[(1,1,7)]`.
  2. `rhs_join_key_update_moves_matches` — lhs `(1,10,1),(2,20,2)`, rhs `(1,10,5)`; expected `[(1,1,5)]`; `UPDATE rhs SET k=20 WHERE id=1`; expected `[(2,1,5)]`.
  3. `duplicate_join_keys_multiply` — lhs `(1,10,1),(2,10,1)`, rhs `(1,10,3),(2,10,4)`; expected `[(1,4,14)]`; delete rhs id 2; expected `[(1,2,6)]`.
  4. `simultaneous_left_and_right_deltas_count_the_delta_product_once` — empty start except lhs `(1,10,1)`, rhs `(1,10,2)`; expected `[(1,1,2)]`; in ONE transaction insert lhs `(2,10,1)` and rhs `(2,10,5)`; expected `[(1,4,14)]` (2 lhs × 2 rhs; sums 2+5+2+5). A delta×delta double count gives `(1,5,19)`; a missing term gives `(1,3,9)`.
  5. `orphan_becomes_matched` — lhs `(1,10,1)`, rhs empty; expected `[]`; insert rhs `(1,10,4)`; expected `[(1,1,4)]`.
  6. `matched_becomes_orphan` — lhs `(1,10,1)`, rhs `(1,10,4)`; expected `[(1,1,4)]`; `UPDATE rhs SET k=99 WHERE id=1`; expected `[]`, and the output table has no row for g=1.
  7. `null_join_keys_never_match` (NULL schema) — lhs `(1,NULL,1),(2,10,1)`, rhs `(1,NULL,3),(2,10,4)`; expected `[(1,1,4)]`; `UPDATE lhs SET k=NULL WHERE id=2`; expected `[]`.
  8. `group_appears_and_vanishes` (single schema `events(id, g, amount)`) — rows `(1,1,5)`; expected `[(1,1,5)]`; insert `(2,2,3)`; expected `[(1,1,5),(2,1,3)]`; delete id 2; expected `[(1,1,5)]`; delete id 1; expected `[]`.
  9. `positive_count_zero_sum_stays_visible` (single) — rows `(1,1,5),(2,1,-5)`; expected `[(1,2,0)]`.
  10. `rollback_leaves_view_state_and_pending_unchanged` — lhs `(1,10,1)`, rhs `(1,10,4)`; refresh; snapshot A; `BEGIN; INSERT rhs (2,10,6); ROLLBACK;`; snapshot B equals A in pending, state and output; refresh; expected still `[(1,1,4)]`.
  11. `repeated_idle_refresh_changes_nothing` — after case 1's final state, refresh twice more; rows and every snapshot count are unchanged; retained rows do not grow.
- [ ] **Step 2: Run** `CARGO_INCREMENTAL=0 cargo test -p ivmlite-bench --locked m2a_fixtures`. All should pass against the current engine. A case that fails is either a wrong expected row (recheck by hand against SQLite) or a real engine bug: if the latter, stop and report BLOCKED with the case; do not change the engine in this task.
- [ ] **Step 3: Prove the explicit rows are not decorative.** Temporarily change case 4's expected row to `(1,5,19)`, confirm red, restore. Note it in the report.
- [ ] **Step 4: Gates and commit** `m2a: targeted correctness fixtures with explicit expected rows`.

### Task 3: Continue past failures and account for losses by cell (AC4, audit R4)

**Files:**
- Modify: `B` (`run_inner` around the per-case loop, `record_failure`, `register_selection`, `Args`/`parse`)
- Test: `B` tests module

**Interfaces:**
- Produces: in the output dir, `selection.json` gains `summary: { planned: u64, complete: u64, failed: Vec<{cell, seed, repeat, phase, error}> }`. Each failed case dir keeps its `failure.json` and has no `completed.json`. Flag `--retry-failed` (default off): with `--resume`, failed cases are skipped unless it is set. Process exit status is non-zero iff `failed` is non-empty. Task 6 reads `summary` and treats any case without `completed.json` as incomplete.

- [ ] **Step 1: Write failing tests.**
  - `a_failing_case_is_recorded_and_the_rest_still_run`: drive `run_inner` (or the smallest seam around the loop) with three smoke cases where the second is forced to fail through a test-only hook (e.g. an injected cell filter that makes `execute_case` return `Err("injected")` for one key). Assert cases 1 and 3 have `completed.json`, case 2 has `failure.json` with phase and error, `summary.complete == 2`, `summary.failed.len() == 1`, and the function returns an error that names the failed count.
  - `resume_skips_failed_cases_unless_retry_failed_is_set`: after the run above, `--resume` without the flag executes nothing new; with `--retry-failed` and the hook removed, case 2 completes and `summary.failed` is empty.
- [ ] **Step 2: Run them and confirm red.**
- [ ] **Step 3: Implement.** Replace the `return Err(error)` after `record_failure` with collection into the summary; write the summary after the loop; return `Err(format!("{n} of {planned} cases failed; see selection.json"))` when any failed. Keep `record_failure` for setup failures that happen before any case runs (those still abort).
- [ ] **Step 4: Gates and commit** `m2a: keep running after a failed case and record losses by cell`.

### Task 4: Plan gates and provenance fixes (spec §2, §3, §4; audit #13, #20, #27, #29, #35, #41)

**Files:**
- Modify: `R` (plan capture/assertion near the covering-plan check, apply path where `changed != 1` is checked, connection setup with pragmas)
- Modify: `B` (`Manifest`, `provenance`, `record_failure`)
- Modify: `crates/ivmlite-bench/build.rs` (create if absent) for the compile-time toolchain
- Test: `R` and `B` test modules

**Interfaces:**
- Produces in `manifest.json`: `pragmas: BTreeMap<String, String>` read back from the live connection; `target_selection: "group_then_row"`; `runner_rustc: String` (compile-time `rustc -vV`); `plans: { view_sql_reference: String, ordinary_recompute: String, covering_recompute: String, dml: BTreeMap<String, String> }`. In `targets.json` per operation: `rows_changed: u64`. On an oracle mismatch: `failure-rows.json` with `{expected_only: [...], actual_only: [...]}` (the symmetric multiset difference, full, sorted).

- [ ] **Step 1: Write failing tests.**
  - `ordinary_recompute_plan_uses_a_join_key_index`: on a tiny join fixture, the captured ordinary-recompute plan text contains `USING INDEX lhs_k` or `USING INDEX rhs_k` (or `USING COVERING INDEX`); with both `lhs_k` and `rhs_k` dropped before capture the assertion function returns an error.
  - `dml_plans_use_rowid_lookups`: the EXPLAIN QUERY PLAN of each prepared UPDATE and DELETE contains `USING INTEGER PRIMARY KEY`; the assertion errors on a statement rewritten to `WHERE amount = ?`.
  - `manifest_pragmas_are_read_back_from_the_connection`: the manifest's pragma map has `foreign_keys`, `recursive_triggers`, `journal_mode`, `synchronous`, `page_size`, `cache_size`, and changing one pragma on the connection before capture changes the map (proves it is read, not copied).
  - `mismatch_writes_the_full_symmetric_difference`: compare `[(1,1,5),(2,1,3)]` against `[(1,1,5),(3,1,3),(3,1,3)]` and assert `expected_only == [(2,1,3)]`, `actual_only == [(3,1,3),(3,1,3)]`.
  - `targets_record_rows_changed_per_operation`: every operation in a tiny trace records `rows_changed == 1`.
- [ ] **Step 2: Run and confirm red.**
- [ ] **Step 3: Implement.** Assert the plans before timing (a failing plan assertion is a case failure in phase `plan`). Store the view-SQL plan only under `view_sql_reference`, not under the `ivmlite`/`no_maintenance` labels. Emit `cargo:rustc-env=M2A_RUSTC_VV=<output of rustc -vV>` from `build.rs` and read it with `env!`. Read pragmas with `PRAGMA <name>` after setup and fail the case if any differs from the pinned value set in `R`.
- [ ] **Step 4: Gates and commit** `m2a: assert recompute and DML plans and read provenance back from the connection`.

### Task 5: Churn and smoke parameters in the workload file; smoke-before-full gate; exact float round trip (spec §3, AC7; audit #17, R5)

**Files:**
- Modify: `workloads/m2a.toml`, `W` (`M2aConfig`, validation), `B` (`select_cells`, smoke and churn selection), `crates/ivmlite-bench/Cargo.toml` and `crates/ivmlite-workload/Cargo.toml` if serde_json is used there (enable serde_json's `float_roundtrip` feature)
- Test: `W` and `B` test modules

**Interfaces:**
- Produces: `M2aConfig.churn: ChurnConfig { profiles: Vec<String>, fanout: u32, batch: u32, seed: u64, rounds: u32 }` and `M2aConfig.smoke: SmokeConfig { single_rows: u64, single_groups: u64, join_rows: u64, join_groups: u64, batch_sizes: Vec<u32> }`. TOML:

```toml
[churn]
profiles = ["uniform_uniform", "zipf_hot"]
fanout = 64
batch = 1000
seed = 42
rounds = 100

[smoke]
single_rows = 4096
single_groups = 64
join_rows = 4096
join_groups = 64
batch_sizes = [10, 100]
```

(Use the values the runner constants hold today; if they differ from the above, keep the runner's values and say so in the report.)

- [ ] **Step 1: Write failing tests.**
  - `churn_and_smoke_come_from_the_workload_file`: load a TOML copy with `rounds = 7` and `single_rows = 512`; the planned churn cases have 7 rounds and the smoke fixture has 512 single rows.
  - `invalid_churn_is_rejected`: an unknown churn profile, `fanout` not in `fanouts`, `rounds = 0` each fail `load` with a message naming the field.
  - `full_run_refuses_without_a_matching_completed_smoke`: a full `run` into an empty output dir errors with `smoke`; after a completed smoke selection with the same source/extension/runner hashes it proceeds; `--skip-smoke-gate` proceeds and is recorded in provenance as `smoke_gate: "skipped"`.
  - `fixture_floats_round_trip_exactly`: a config with `zipf_exponent = 1.1700000000000002` and `hot_group_fraction = 0.30000000000000004` round-trips through `save_fixture` and reload bit-exactly.
- [ ] **Step 2: Run and confirm red.**
- [ ] **Step 3: Implement**, removing the runner constants so the workload file is the only source.
- [ ] **Step 4: Gates and commit** `m2a: churn and smoke parameters live in the workload file; smoke gates the full run`.

### Task 6: `m2a report` — derived tables from the raw artifacts (spec §4, §5, §6.4–6.5)

**Files:**
- Create: `crates/ivmlite-bench/src/m2a_report.rs`
- Modify: `crates/ivmlite-bench/src/lib.rs` (`pub mod m2a_report;`), `B` (new action `report --out DIR --report DIR`)
- Test: `m2a_report.rs` tests with synthetic artifact dirs written by the test

**Interfaces:**
- Consumes: case dirs with `manifest.json`, `timing.json`, `space.json`, `targets.json`, `correctness.json`, `completed.json` (Tasks 1 and 3 fields included); `selection.json` `summary`.
- Produces: `pub fn report(out: &Path, dest: &Path) -> Result<ReportSummary>` writing to `dest`: `timing.csv`, `ratios.csv`, `confirm-candidates.json`, `space.csv`, `targets.csv`, `churn.csv`, `losses.csv` and `summary.md`. `ReportSummary { complete: usize, incomplete: usize, candidates: Vec<CaseKey>, best_ratio: Option<f64> }`. Task 7 reads `confirm-candidates.json` (a list of `{cell, seed}` objects).

Definitions (state them in a doc comment and in `summary.md`):
- A run's total for a mode is the **median over the five measured rounds of that round's apply+maintain+read sum** (never a sum of component medians). Also report apply+maintain.
- **Fastest complete recompute** for a run = the smaller of `ordinary_recompute` and `covering_recompute` totals in that same run. `no_maintenance` is never a denominator or numerator of a speedup; its apply time is reported as the write lower bound.
- **Ratio** = fastest-recompute total / ivmlite total, per run (seed). Per cell: min, median, max across the three seeds.
- **Confirmation candidate**: a cell whose [min, max] ratio range contains 1.0 or 2.0, or whose median is within 10% of 1.0 or 2.0.
- **Space**: per checkpoint, ivmlite / base-only and covering / base-only allocated-byte ratios at the same checkpoint key, plus live-page proxies, logical state rows and `retained_stage_rows`.
- **Targets**: top-10% group row share of the initial fixture, realized hot target share, matched-group hot share per batch.
- **Churn**: page_count, freelist_count and logical state rows per round, per profile.
- A case without `completed.json` is listed in `losses.csv` with its phase/error and contributes to no ratio.

- [ ] **Step 1: Write failing tests** on a synthetic dir with two cells × three seeds:
  - `ratio_uses_the_faster_recompute_mode_per_run`: in seed 1 ordinary is faster, in seed 42 covering is faster; assert each ratio's denominator.
  - `totals_are_median_of_round_sums`: rounds `(apply, maintain, read)` = `(1,1,1),(1,1,10),(10,1,1),(1,10,1),(2,2,2)`: sums 3,12,12,12,6, median 12; the median-of-components sum (1+1+1=3) must not appear.
  - `no_maintenance_is_never_in_a_speedup`: make no_maintenance the fastest mode; the ratio is unchanged.
  - `candidate_rule_selects_straddles_and_near_misses`: seed ratios `[0.9,1.1,1.0]` → candidate; `[1.95,2.05,2.0]` → candidate; `[3,3.1,3.2]` → not; `[1.08,1.09,1.09]` → candidate (within 10% of 1).
  - `incomplete_case_is_a_loss_not_a_ratio`: delete one seed's `completed.json`; it appears in `losses.csv` and the cell's ratio range uses only two seeds and is marked `partial`.
  - `space_ratio_pairs_same_checkpoint_keys`.
- [ ] **Step 2: Run, confirm red, implement, run green.**
- [ ] **Step 3: Gates and commit** `m2a: derive ratios, candidates, space and churn tables from raw artifacts`.

### Task 7: Confirmation workflow from the report's candidates (spec §4 last paragraph; audit #50)

**Files:**
- Modify: `B` (`confirm` action), possibly `R` (a timing-only entry point)
- Test: `B` tests

**Interfaces:**
- Consumes: `confirm-candidates.json` from Task 6.
- Produces: `confirm --candidates FILE --repeats 5 --out DIR` runs, for each candidate cell, 5 paired timing-only repeats at seed 42 into `DIR/confirm/<cell>/repeat-N/` (fixture reused from the initial run's case dir; correctness and space replay run once per cell, not per repeat). Each repeat's manifest records `load_before` and `load_after` (1-minute load average). `--max-load L` (default 2.0) refuses to start a repeat while load exceeds L, waiting up to 10 minutes, then records the repeat as a loss `host_not_idle`. `m2a report` (Task 6) gains a `confirmations.csv` with each candidate's repeat ratios and full range, separate from the seed table.

- [ ] **Step 1: Write failing tests:** `confirm_reads_candidates_and_runs_timing_only_repeats` (smoke-size candidate, 2 repeats; correctness.json written once per cell, timing.json per repeat); `confirm_records_host_not_idle_when_load_stays_high` (inject a load reader returning 9.0 with a 0-second wait cap); `report_lists_confirmations_separately`.
- [ ] **Step 2: Confirm red, implement, green, gates, commit** `m2a: confirm candidate cells with timing-only paired repeats on an idle host`.

### Task 8: Mutation-gate registration (AC3)

**Files:**
- Modify: `docs/mutation-gates.md` (new rows at the end of the `## ivmlite-workload / ivmlite-bench` table; totals via script)
- Modify: any test file where a mutation survives (add the missing gate first)

For each invariant below: apply the mutation, run `CARGO_INCREMENTAL=0 cargo test -p ivmlite-bench -p ivmlite-workload --locked`, record the red test name(s) and counts, restore, confirm green. Use the existing row format: `| M2a §<ref>: <invariant> | <mutation> | <test that goes red> | **verified** — compiles; <passed>/<failed>, exactly <test> red |`. Only write **verified** for mutations actually run.

1. RePrepare is gated at apply, maintain and read call sites (three rows).
2. Totals are paired round sums (report and runner).
3. no_maintenance has N/A maintain/read and no speedup.
4. Fastest-recompute denominator (Task 6).
5. Confirmation candidate rule (Task 6).
6. Hot locality: hot target share falls to uniform if the hot policy is replaced by uniform selection.
7. Fanout per checkpoint: f rows per key per side.
8. Trace prefix validation (`TC`).
9. Decoded arrangement equality (ghost, missing, wrong weight).
10. Idle refresh adds no retained rows.
11. Inverse replay restores bootstrap state.
12. Covering plan, ordinary recompute plan, DML rowid plan (Task 4).
13. Completion hashes bind artifacts.
14. **Skipped oracle check:** delete the oracle comparison call in the correctness replay (`R` `correctness`, per batch). If nothing goes red, first add `correctness_replay_fails_on_a_wrong_view_result` — inject a view result off by one row through a test hook and assert the replay errors — then register it.
15. **Skipped state check:** delete the per-batch `validate_snapshot` call in the correctness replay. Same procedure.
16. **Skipped performance verification:** delete the final-result `verify()` in the timed replay. Same procedure.
17. Retained stage rule (Task 1), explicit fixture rows (Task 2, case 4), continue-on-failure (Task 3), smoke gate (Task 5), host-not-idle (Task 7).

- [ ] **Step 1:** Run each mutation; add missing gates as described; register rows.
- [ ] **Step 2:** `python3 scripts/count-mutation-gates.py --fix`, then confirm it exits 0 without `--fix`.
- [ ] **Step 3:** Gates and commit `m2a: register the M2a invariants in the mutation-gate table`.

### Task 9: Run, report and publish (AC1, AC4–AC7)

Run by the controller (long-running commands). No code changes except a bug fix with its own regression test, committed separately.

- [ ] **Step 1:** `CARGO_INCREMENTAL=0 scripts/test-all.sh`; both fmt checks; clippy for the workspace and for `crates/ivmlite-sqlite/Cargo.toml` with `--all-targets -- -D warnings`.
- [ ] **Step 2:** `scripts/build-extension.sh` (release), then `cargo run --release -p ivmlite-bench --bin m2a -- run --smoke --out target/m2a`, then `run --out target/m2a`, then `churn --out target/m2a`, then `report --out target/m2a --report target/m2a/report`, then `confirm --candidates target/m2a/report/confirm-candidates.json --out target/m2a`, then `report` again. Use the exact flags the binary supports after Tasks 3–7.
- [ ] **Step 3:** Publish under `docs/bench/m2a/`: the derived CSV/JSON tables, `summary.md`, the manifests and a compressed archive or a sampled subset of raw rounds if the full raw set exceeds 20 MB (state which, and keep the full set's SHA-256 list in `docs/bench/m2a/raw-sha256.txt`). Write `docs/bench/m2a.md`: the question, the matrix, the >2× answer by region (profile × batch × fanout), where the advantage fails and how hotspots change it, state growth and space beside timing, churn retention, every loss, and the stage-row amendment. A falsifying result is a complete result. Link it from `docs/bench/README.md` and `docs/README.md`.
- [ ] **Step 4:** Commit `m2a: publish the non-uniform validation results`.
