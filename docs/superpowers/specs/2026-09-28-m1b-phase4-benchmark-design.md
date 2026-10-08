# M1b Phase 4: benchmarking the SQLite extension

**Status:** approved 2026-09-28. **Parent specs:** [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md) §10 (benchmark), [2026-09-27-m1b-phase3b-shared-capture-design.md](2026-09-27-m1b-phase3b-shared-capture-design.md) (the extension as it stands). Where they disagree, this document is newer and wins for Phase 4.

> **Results amendment, 2026-09-30:** the full protocol completed without a
> verification mismatch. The confirmed data finds a broad region above the 2x
> bar and a small-table, large-batch losing region; ivmlite did not beat the
> hand-written trigger. Full results and limits are in
> [`docs/bench/README.md`](../../bench/README.md), section “M1b Phase 4: the
> SQLite extension.” There was no measurement-protocol deviation. The data
> deliverable list in §9 is corrected to include
> `m1b-phase4-ablation-write-amp.csv`, which §7 and the implementation plan
> already required.

> **Review amendment, 2026-10-07:** the post-merge review of Phase 4 changed no
> published measurement; it corrected the README's analysis and tightened the
> runner. This document is amended where it disagreed with the code:
> §3.4 (which CSV carries the derived write-amplification column), §6 and §8
> (the ablation's K and view counts live in the workload files; the refresh
> policy is a documented constant of the runner), and §7 (REPLACE is checked
> per op, and the `recursive_triggers` mode is read back after each timed
> apply).

## 1. Scope

1. Add the extension, as the engine `ivmlite`, to `ivmlite-bench`, next to the three M0 baselines (§10.2): `no_maintenance`, `hand_written_trigger` and `naive_recompute`. It runs on the same workload file and the same matrix cells.
2. Fix the two hot spots the §2 measurements found:
   - the retraction full scan of the output table;
   - the latch, which scans `sqlite_schema` four times per written row.
3. Measure the effect of each fix separately, using an ablation over three builds.
4. Measure write amplification with a workload that exercises UNIQUE candidate lookups, UPDATE, REPLACE, and both `recursive_triggers` modes.
5. Publish the result as a **slice** of §10.4's surface under one refresh policy, **one refresh after every batch**. Phase 4 does not cover the refresh-frequency dimension, and the README must not claim it does.

Not in Phase 4: Turso, Nexmark, Zipf data, update locality, and the bare `dbsp` ceiling. All remain M2 (§11, §10.6).

## 2. Evidence (throwaway spike, 2026-09-28)

These numbers motivate §4 and §5. **They are not the "before" data:** §6's ablation, run through the real harness, supplies that. The spike script and its output are kept in `docs/spikes/2026-09-28-m1b-phase4-cost-spike.md`, so they can be reproduced.

Setup: release build of the extension, Python 3.13 with SQLite 3.53, in-memory database, the m0 `orders` shape, views of the m0 shape `SELECT region, SUM(amount), COUNT(*) FROM orders WHERE amount > θ GROUP BY region`. Each cell was run once.

| base rows | groups | views | batch | refresh, all views (ms) | full recompute, all views (ms) |
|---|---|---|---|---|---|
| 100k | 1k | 10 | 1000 | 164 | 263 |
| 100k | 100k | 10 | 1000 | 11 056 | 690 |
| 1M | 1k | 10 | 1000 | 172 | 2 784 |
| 1M | 100k | 10 | 1000 | 27 365 | 3 914 |

Refresh cost per view, 100k base rows, 10 views:

| groups (rows in each output) | no delta | 1-row insert | 100-row insert |
|---|---|---|---|
| 10 | 0.20 ms | 0.26 ms | 0.43 ms |
| 1 000 | 0.21 ms | 0.30 ms | 3.84 ms |
| 100 000 | 0.75 ms | 1.54 ms | 95.9 ms |

The fixed cost of a refresh is small. The cost grows with the size of the output, which is the retraction full scan: each retracted output row scans the whole output table (Phase 3a's known limitation).

Insert cost per row, by number of views:

| views | `sqlite_schema` rows | µs per inserted row |
|---|---|---|
| 0 (no extension) | 1 | 1.0 |
| 1 | 26 | 10.1 |
| 10 | 71 | 14.2 |
| 50 | 271 | 32.1 |

That is about 9 µs of fixed trigger work, plus about 0.09 µs per `sqlite_schema` row. The latch reads `sqlite_schema` four times per row. The table has no index, so the cost grows with every view created anywhere in the database.

## 3. Engine and runner

### 3.1 Loading the extension

- **Build and library path.** `ivmlite-bench` gains `rusqlite`'s `load_extension` feature. It loads the **release** build `crates/ivmlite-sqlite/target/release/libivmlite_sqlite.{dylib,so}`, and refuses to run if that library is missing or older than any source, manifest or lock file of `ivmlite-sqlite`, `ivmlite-core` or `ivmlite-sql`. This is the same rule the test host applies to the debug build.
- **Overriding the path.** `--extension <path>` overrides the library path. The ablation (§6) uses it; the staleness check is skipped for an explicit path.
- **The script.** `scripts/bench.sh` builds the release extension and the release bench, then runs the bench.

### 3.2 One cell, one engine

Each engine gets its own fresh in-memory database for each cell, and follows this sequence:

1. **Seed** the base table (untimed).
2. **Bootstrap** (timed on its own, as `bootstrap_ms`):
   - `ivmlite` creates the N views;
   - `hand_written_trigger` creates and fills its summary tables and installs its triggers;
   - the other two engines do nothing here.
3. **Verify the initial state** (untimed). Every view equals SQLite's own evaluation of its SQL. For `ivmlite`, read the view; for `hand_written_trigger`, read the summary table. `no_maintenance` and `naive_recompute` have nothing to verify.
4. **Prepare** every statement the timed regions will run (as M0 does, so compilation is never timed).
5. **Timed `apply`:** run the batch's writes. This is the total write time, including every trigger the engine installed.
6. **Timed `maintain`:**
   - `ivmlite` refreshes every view;
   - `naive_recompute` re-runs every view's SQL and reads the results;
   - the other two record 0.
7. **Verify the final state** (untimed), as in step 3, for `ivmlite` and `hand_written_trigger`. `naive_recompute`'s results are compared with the same oracle.

   A mismatch aborts the run with the cell and the difference. **No numbers are published from an engine that computed a wrong answer.**

Nothing runs between steps 5 and 6. In particular no oracle runs there: `ivmlite`'s views are not refreshed yet, and a full scan there would warm the cache for step 6.

The space samples of §3.4 are taken outside the timers, after steps 1, 2, 5 and 6.

### 3.3 Matrix order

- **Cell order.** The runner iterates **cells in the outer loop and engines in the inner loop**. The M0 runner had it the other way round (one engine through the whole matrix, then the next), which let machine drift across a long session bias one engine against another.
- **Engine order within a cell.** It rotates with the cell index: cell *i* runs the engines in the order `ENGINES` rotated left by `i mod 4`.

### 3.4 Metrics

Per cell and engine, the CSV `docs/bench/m1b-phase4.csv` records:

| Group | Columns |
|---|---|
| The cell (as M0) | `engine`, `views`, `base_rows`, `batch_size`, `group_cardinality` |
| Times | `bootstrap_ms`, `apply_ms`, `maintain_ms` |
| Space, once | `page_size` |
| Space, at each of the four sample points (`base`, `bootstrapped`, `written`, `maintained`) | `page_count` and `freelist_count` |

The four space sample points are:
- only the base table;
- after bootstrap;
- after the writes, before `maintain`;
- after `maintain`, which includes `ivmlite`'s GC.

Derived in the README, never in the runner (this applies to the matrix CSV;
the write-amplification CSV of §7 carries its own `extra_us_per_row` column,
computed by the runner):

- **Speedup**, the main chart's ratio:

  `(naive_recompute.apply_ms + naive_recompute.maintain_ms) / (ivmlite.apply_ms + ivmlite.maintain_ms)`

  Values above 1 mean `ivmlite` is faster.
- **Write amplification**, for `ivmlite` and `hand_written_trigger`:
  - as a multiple of `no_maintenance.apply_ms`;
  - as extra µs per written row: `(engine.apply_ms − no_maintenance.apply_ms) × 1000 / batch_size`.
- **Space amplification:** `ivmlite`'s `(page_count − freelist_count) × page_size` at each sample point, against the base-only sample.

### 3.5 Exploration, then confirmation

- **Exploration.** The full matrix runs **once**: 68 cells × 4 engines (the M0 derivation, `Workload::cells()`, unchanged). It shows the shape of the surface. On its own, it proves no single point.
- **Confirmation.** `ivmlite-bench confirm` reads the exploration CSV and selects:
  - every cell whose speedup lies in **[0.7, 1.4]** (near the 1× crossover);
  - every cell whose speedup lies in **[1.4, 2.8]** (near §10.4's 2× falsification bar);
  - every cell where `ivmlite`'s `apply_ms + maintain_ms` is **below** `hand_written_trigger`'s (a claim of beating hand-written triggers).
- **Repeats.** Each selected cell is re-run **K = 5** times, each time independently: a fresh database, and a rotated engine order. The runner writes `docs/bench/m1b-phase4-confirm.csv`, with each repetition as a row.
- **What the README may claim.** A statement about a crossover, the 2× bar, or beating hand-written triggers must cite the **median and min–max over the repeats**. The M0 README's "differences under 1.3× are not findings" is **not** a general error bound, and Phase 4 does not rely on it.

## 4. Fix: index the output table

- **The index.** Each view's output table gets a **non-unique** index over all its output columns, `__ivm_outidx_<view>`. It is created with the table and dropped with it.
  - It must be non-unique. v0's root aggregate makes output rows distinct, but the apply trigger's semantics are per-copy: one `out-` removes one row. A non-unique index keeps that exact.
- **What uses the index.** The retraction's existence check (`RAISE … WHERE NOT EXISTS …`) and its `DELETE … WHERE rowid = (SELECT rowid … LIMIT 1)` both use it.
  - One function builds the lookup SQL. It takes the operand of each column's `IS` comparison as a parameter: `NEW.cN` inside the trigger, `?N` in the test. The query-plan test therefore examines **the very query the trigger runs**, not a simplified copy.
- **Format.** It becomes **3**. A database in format 2 is refused, as before; no release ever wrote format 2.
- **Tests:**
  - `EXPLAIN QUERY PLAN` of the generated lookup shows `SEARCH` on `__ivm_outidx_<view>`, not `SCAN`.
  - An output value that is NULL is retracted correctly: a NULL group key, and a SUM over only NULLs.
  - A white-box test: two identical rows in the output table, plus one staged `out-`, leave exactly one row. v0 views cannot produce duplicates through SQL, so the test writes the output and stage tables directly.
  - The TEMP-shadow test includes a TEMP table named `__ivm_outidx_<view>`. Dropping the view leaves no index behind.
- **A gate row:** remove the index, and the plan test goes red.

## 5. Fix: one scan for the latch

The latch's four comparisons (Phase 3b §6.2) all concern rows of `sqlite_schema` whose `tbl_name` is `t`: the table itself, its capture triggers and its unique indexes. They become **one** aggregate query over `sqlite_schema WHERE tbl_name = <t> COLLATE NOCASE`, so each written row takes one pass instead of four. The latch fires unless all of these hold:

- exactly one row with `type = 'table'`, and its `sql` equals the literal;
- exactly 5 rows with `type = 'trigger'` and `name IN (<the five names>)`;
- exactly *n* explicit unique indexes (`sql LIKE 'CREATE UNIQUE INDEX%'`), all of whose `sql` is in the literal list.

Rules:

- **Only NULL-safe aggregates.** Counting uses `count(CASE WHEN … THEN 1 END)` (0 on empty input), never `sum(…)` (NULL on empty input). If `t` has been renamed away, the scan finds no rows at all; the latch must still fire. A test covers that case, and the case of a table with no unique index.
- **Old SQLite must still parse the triggers.** No aggregate `ORDER BY`, no `FILTER`, no window function, no `group_concat`. The existing text test is extended to reject `FILTER (` and ` OVER (`. The manual check with SQLite 3.42 is repeated, and recorded with the command used.
- **Behavior is unchanged.** Every Phase 3b latch test, decoy test and gate row stays green: missing table, missing trigger, rename round trip, transient unique index, `ALTER COLUMN`, swapped index. The latch's gate rows are re-verified against the new query.

## 6. Ablation: the effect of each fix

`ivmlite-bench ablation --extension <lib> --label <name>` runs a fixed set of representative cells for the `ivmlite` and `naive_recompute` engines. Each cell repeats K = 5 times, with the same protocol as §3.2. This K is a workload parameter, `[ablation] repeats` in the m0 workload file (§8), not a runner constant like the confirmation's K.

| base rows | groups | views | batch |
|---|---|---|---|
| 100 000 | 1 000 | 10 | 1 000 |
| 100 000 | 100 000 | 10 | 1 000 |
| 100 000 | 1 000 | 200 | 1 000 |
| 100 000 | 100 000 | 200 | 100 |

These cover low and high group cardinality, and few and many views. Their parameters live in the workload file (§8).

`scripts/bench-ablation.sh` builds the extension at each of three commits: the one before §4, the one after §4, and the one after §5. Each build goes into a separate `git worktree` with its own `CARGO_TARGET_DIR`. The script runs the ablation mode against each build and writes `docs/bench/m1b-phase4-ablation.csv`. The README's before/after table reports medians and min–max per build. This, not §2, is Phase 4's formal before-and-after comparison.

## 7. Write amplification: its own workload

§3's matrix inserts and deletes only through the `INTEGER PRIMARY KEY`, so it cannot answer what REPLACE's UNIQUE candidate lookup costs. A second workload file, `workloads/write-amp.toml`, is loaded by a new `ivmlite_workload::WriteAmpWorkload`. It defines:

- **The schema:**

  `accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT`

  It has two ordinary UNIQUE keys besides the rowid.
- **The data:** base rows and group cardinality, with a fixed seed.
- **The view counts:** `[0, 1, 10, 50, 200]`, with views of the m0 shape over `accounts`.
- **The operations.** Each operation kind has its own trace of *M* rows, and the trace is replayable and deterministic:

| op | what each row does | verified per row |
|---|---|---|
| `insert` | plain INSERT of a new id, email and handle | 1 row inserted |
| `delete` | DELETE by id of an existing row | exactly 1 row changed |
| `update` | UPDATE `amount` and `region` by id | exactly 1 row changed |
| `replace_rowid` | INSERT OR REPLACE whose id conflicts with one existing row | net row count unchanged |
| `replace_unique` | INSERT OR REPLACE with a new id whose email conflicts with one existing row | net row count unchanged |
| `replace_two` | INSERT OR REPLACE with a new id whose email conflicts with row A and whose handle conflicts with a different row B | net row count −1 |

- **The modes:** `recursive_triggers` OFF and ON.
- **The transaction batch:** a fixed number of rows per transaction.
- **The refresh policy:** refresh every view once after each op trace, untimed; `apply_ms` is the timed quantity. This is a constant of the runner, documented in the workload file's comments (§8).

The engines are `no_maintenance`, `hand_written_trigger` (INSERT, DELETE and UPDATE triggers, measured for cost only) and `ivmlite`. The runner verifies each row's effect as the table says. After refresh, it also verifies that `ivmlite`'s views match SQLite's own evaluation.

The per-row check of a REPLACE is made after the trace, untimed: every REPLACE's own row must be present as written, and every existing row it claims to conflict with must be gone. Together with the net row count, this is exact per op. After each timed apply the runner also reads `PRAGMA recursive_triggers` back and refuses a row whose mode differs from the requested one.

Output: `docs/bench/m1b-phase4-write-amp.csv`, with one row per (engine, views, op, mode). Each row records `apply_ms` and µs per row, the extra µs per row over `no_maintenance`, and the space samples of §3.4.

The ablation's commits also run this workload for `ivmlite` at 10 and 200 views, so the latch fix's effect on write cost is measured, not assumed. Those view counts are the workload file's `ablation_view_counts`, selected with `ivmlite-bench write-amp --ablation-views`.

## 8. Workload files stay the single source

- **Portable workloads (§10.3 item 7).** Every parameter above that selects what gets measured lives in a workload file, not in the runner. That covers the ablation cells and repeats, the write-amp ops, the view counts (including the ablation's), the transaction size and the modes. The refresh policy (§1 item 5, §7) is the one exception: it is a documented constant of the runner, since Phase 4 measures only one.
- **The m0 file** gains an `[ablation]` section, and `write-amp.toml` is new.
- **The confirmation rule, the confirmation's K and the engine rotation** are properties of the measurement protocol, not of the workload. They are constants in the runner, stated in this spec and in the README. The ablation's K is not among them: the workload file already carries it (§6).

## 9. Deliverables and acceptance

- **Code.** The four engines, with the modes `matrix` (the default), `confirm`, `ablation` and `write-amp`, and the scripts `scripts/bench.sh` and `scripts/bench-ablation.sh`.
- **Tests.** Unit tests for:
  - the engine rotation;
  - the confirmation selection rule;
  - CSV round trips;
  - trace verification;
  - the write-amp trace generator, which must be deterministic and produce the conflicts it claims.
- **A smoke test in the workspace suite.** It runs one tiny cell per engine against the debug extension, verifying the states, so `scripts/test-all.sh` exercises the harness without a release build.
- **Data.** `docs/bench/m1b-phase4.csv`, `-confirm.csv`, `-ablation.csv`,
  `-ablation-write-amp.csv` and `-write-amp.csv`, plus charts. The M0 charts
  are regenerated with an `ivmlite` series, under new file names. The M0 CSV
  and its test stay as the historical M0 record.
- **Analysis.** `docs/bench/README.md` gains an "M1b Phase 4" section:
  - **The three bars of §10.2**, each with where it holds and where it does not.
  - **The falsification test of §10.4:** is there a region with speedup > 2? Where exactly is it, backed by confirmed cells?
  - **Write amplification per op and mode,** including the REPLACE lookups.
  - **Space amplification.**
  - **The ablation table.**
  - **The refresh-policy caveat.**

  Findings that go against ivmlite are reported as plainly as the ones in its favor.
- **Gates.** Every new check in the extension (§4, §5) gets a verified row in `docs/mutation-gates.md`.
