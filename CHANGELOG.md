# Changelog

All notable changes to ivmlite will be documented in this file.

The project uses semantic versioning after `0.1.0`; prereleases may change the
SQL subset and persisted shadow-table format without migration.

## [Unreleased]

## [0.1.0-alpha.6] - 2026-10-19

### Breaking

- Shadow-table format 6 refuses alpha.5 format-5 state. Old views open as
  broken views that can be dropped. Drop every old view and recreate it from
  its SELECT query; there is no automatic state migration.

### Added

- Non-STRICT base tables, every declared type by SQLite affinity, STRICT ANY,
  and all five storage classes through capture, persisted state and output.
- `COUNT(column)`, `TOTAL`, `AVG`, `MIN` and `MAX`; exact REAL `SUM`, with
  retraction-safe indexed extremum state and order-independent aggregation.
- REAL and BLOB literals with comparison affinity, and same-affinity joins
  with canonical numeric equality (`1` matches `1.0`).
- Mixed-type differential tests with an independent exact reference, REAL
  order-independence and persisted-extremum checks, and artifact smoke coverage
  for new aggregates, storage classes, retractions, rollback and reopen.
- An [After M2c census](docs/census/2026-10-17/README.md#after-m2c)
  compatibility projection: ten adapted application candidates, including
  low-confidence fits; this is not upstream execution or adoption.

### Changed

- SUM accepts any column. Integer SUM overflow is judged on the final exact
  result. REAL SUM/TOTAL/AVG round once and may differ from SQLite's scan-order
  results, including finite exact results after SQLite intermediate overflow.
- Group keys use SQLite numeric equality while preserving REAL output when
  a live REAL key exists. MIN prefers INTEGER and MAX REAL in numeric ties.
- FluxFlow uses native REAL amounts; Kener uses REAL latency with AVG/MIN/MAX.
  The supported SQL boundaries and numerical differences are documented in the
  [release notes](docs/releases/v0.1.0-alpha.6.md).

## [0.1.0-alpha.5] - 2026-10-15

### Fixed

- Catalog validation now qualifies pragma table-valued functions with `main`,
  so a TEMP object with the same name cannot shadow the catalog checks.
- A refused savepoint close in `ivm_refresh_all` now rolls back the call
  inside the application's transaction too. Cleanup guards remain armed
  until closing succeeds, and continue mode uses a distinct per-view
  savepoint name. If the host persistently refuses rollback, the error
  reports that refreshes may remain rather than claiming none occurred.
- Benchmark workload preflight checks the actual deterministic trace,
  rejecting deletes after the base rows are exhausted even when the
  expected delete count fits. Valid traces are unchanged.

### Added

- Five additional demand-shaped rollup demos: Zenod, Nicti, Postio, Phoenix
  and PocketBase. Each runs mixed mutations against a SQLite multiset oracle
  and compares ivmlite with ordinary/covering-index recomputation and local
  SQL triggers. The saved [results](docs/demos/results/2026-10-15-summary.md)
  include losses and bootstrap/space costs; fixtures are synthetic, and
  public reports establish workload shapes rather than adoption. These runs
  used the pre-bump alpha.4 package version at commit `1c61d6e`.
- `ivm_refresh_all([mode])` refreshes every view of the database in one call,
  in `name` order. It must be called from a `SELECT`.
  - `ivm_refresh_all()`, the same as `ivm_refresh_all('atomic')`, refreshes
    every view under one savepoint, or none. It returns the number of views
    refreshed, not counting the views it skipped because they had nothing
    new since their last refresh (their checks still run). On a failure it rolls every view back and fails with
    `ivm_refresh_all: view <name>: <message>; no view was refreshed`.
  - `ivm_refresh_all('continue')` refreshes each view under its own
    savepoint, rolls back only the views that fail, and returns
    `refreshed <r>, skipped <s>, failed <f>`, then `: <name>: <message>` for
    each failed view.
  - From a statement that writes, it fails with
    `ivm_refresh_all must be called from a SELECT, not from a statement that writes`.
    It is registered `SQLITE_DIRECTONLY`, so SQLite refuses it in the
    database schema's triggers, views and `CHECK` constraints. Inside
    `BEGIN … COMMIT` it nests in the transaction.
  - In autocommit it runs inside `BEGIN IMMEDIATE` … `COMMIT`, so it waits
    for another writer under `busy_timeout`; from `SELECT … FROM t`, whose
    read is already under way, it cannot wait and fails with
    `database is locked`. If its commit is refused, for example with
    `SQLITE_BUSY`, it rolls the call back whole.
  - A nested failure that rolls back the whole transaction, such as
    `RAISE(ROLLBACK)`, fails the call in either mode with the view's name
    and message, and ends the application's own transaction.
  - A `__ivm_view` row that does not name an `ivm` virtual table fails as
    `not an ivmlite view`, and nothing is written to the table it names.
  - Any other argument fails with `ivm_refresh_all takes 'atomic' or 'continue'`.
- Published M1b Phase 7 results in [`docs/bench/README.md`](docs/bench/README.md),
  from a new driver, [`scripts/bench-refresh-all.py`](scripts/bench-refresh-all.py),
  with [`phase7_tables.py`](docs/bench/phase7_tables.py) deriving every figure:
  - the function costs 4.0–5.4 µs per view per call more than an
    application's loop of refresh statements it keeps prepared, with one-row
    or empty batches: 8.965 against 8.157 ms for 200 views at batch 1
    (1.10x), and 1.99x when every view is idle. Its nested statements are
    prepared on every call, because a cached statement that names an
    ivmlite view would keep the connection from closing (Phase 6 §3). With
    1,000-row batches it was 2.3–4.2% faster than the loop, ranges
    overlapping;
  - its value is atomicity and one call, not speed.

### Changed

- A refresh with nothing new since its last refresh returns after its
  catalog checks, before it computes, stages or applies anything. The checks
  still run, so a base table recreated without its capture triggers still
  breaks the view instead of leaving it silently stale. An idle view's
  refresh fell from 18.7 to 4.3 µs: refreshing 200 idle views takes 0.866 ms
  instead of 3.748 ms (4.33x), and is 1.33x faster when half of them are
  idle (in every round, with overlapping ranges). The shadow-table format is unchanged (5).

## [0.1.0-alpha.4] - 2026-10-12

### Fixed

- The M1b Phase 5 Kener result published with `0.1.0-alpha.3` was wrong
  (corrected 2026-10-10). Its steady-state refresh is 1.22x faster than in
  alpha.2 (2.872 → 2.354 ms median), not 0.80x to 0.90x slower. The earlier
  figures came from timing one refresh per repeat. With ten measured batches
  per run, steady-state refresh is 1.19–1.47x faster in all four demos
  ([`docs/bench/README.md`](docs/bench/README.md), "M1b Phase 5").
- A demo run's `apply_plus_maintain_ms` and `end_to_end_ms` are now the
  medians over its measured batches of each batch's own sum. They were sums
  of the per-step medians, which may come from different batches. The demo
  results and the Phase 5 demo before/after were re-run; the first
  correction above had given Kener 2.772 → 2.235 ms and 1.14–1.45x.
- `real_case_bench` prepares every view's refresh and its transaction's
  `BEGIN IMMEDIATE` and `COMMIT` before the timers that run them; they were
  compiled inside them. Its `total_ms` is the median of each repeat's own
  `apply + maintain`, not the sum of the two medians. `docs/cases/evaluation/`
  was re-run with the `0.1.0-alpha.3` extension, and its claim that statement
  preparation was untimed is corrected. With that extension all three slices
  cross over; the Org-roam join slice, slower than recomputation at both
  points in alpha.1, is now 31.99x faster at 250k base rows with 100 inserted.
- A workload file whose list field repeats an entry (`view_counts`,
  `ablation_view_counts`, `ops`, `recursive_triggers`, the four `[matrix]`
  lists, or two equal `[[ablation.cells]]`) is rejected at load. A repeated
  entry ran a cell twice, and the write-amplification CSV could then pair one
  run's rows with the other run's `no_maintenance` baseline.

### Changed

- **Breaking: shadow-table format 5.** The global tables gain an index,
  `__ivm_progress_tbl` on `__ivm_progress(tbl, applied_seq)`, so the garbage
  collection inside a view's refresh searches it instead of scanning one
  watermark row per view of the database. Databases created by
  `0.1.0-alpha.3` (format 4) are refused. Their views still open as broken
  views, so `DROP TABLE <view>` works with this release loaded: drop every
  alpha.3 view, then create it again. A missing index breaks every view's
  next refresh with a message naming it; creating any view recreates it. The
  supported SQL is unchanged.
- Each view keeps the statements of its refresh prepared for as long as it is
  open, through one connection wrapper per view, instead of compiling them on
  every refresh. Rows are staged in full chunks plus a remainder split into
  chunks of 32, 16, 8, 4, 2 and 1 rows, so every staging statement has one of
  a few cached shapes and a steady refresh prepares nothing. `CREATE` flushes
  the cache as its last step, so bootstrap's large staging shapes are not
  kept. A statement made stale by a schema change re-prepares itself, and
  every refresh runs its catalog checks before any statement that reads the
  view's shadow tables.
- After a schema change, the catalog checks read one snapshot of
  `main.sqlite_schema` per connection, keyed by `schema_version` and shared by
  that connection's views, instead of each view looking objects up in
  `sqlite_schema`. Each base table's capture verdict is computed once per
  connection and version. Every connect rebuilds the snapshot, because a
  rolled-back schema change can bring back an earlier version with different
  objects. Broken-view messages are unchanged.
- The benchmark's matrix, confirmation and ablation cells measure the steady
  state: one warm-up batch, reported as `first_refresh_ms`, then five measured
  batches, each touching rows no other batch touches. `apply_ms` and
  `maintain_ms` are medians over the five, and the new
  `apply_plus_maintain_ms` is the median of each batch's own sum. Earlier
  results timed the first refresh after `CREATE`, so they are not comparable
  column for column. A workload cell that cannot fit six disjoint batches is
  refused. `confirm`, `plot` and the tables scripts still read the older CSVs.
- The demo benchmarks (`crates/ivmlite-test/src/demo_bench.rs`) time ten
  steady-state batches per run after the warm-up, instead of one, and report
  each of `apply_ms`, `maintain_ms` and `read_ms` as its median over them;
  a single refresh is too noisy to resolve a change of about 10%. The demo
  results in `docs/demos/` and the Phase 5 demo before/after were re-run with
  it, and the single-refresh Kener recheck CSV was removed.

### Added

- Published M1b Phase 6 results in [`docs/bench/README.md`](docs/bench/README.md),
  with [`phase6_tables.py`](docs/bench/phase6_tables.py) deriving every figure:
  - with one-row batches, a steady refresh of every view is 3.50x faster than
    in alpha.3 at 200 views (23.547 → 6.730 ms, about 34 µs per view) and
    3.05x at 10 views; the first refresh after creating 200 views is 9.63x
    faster (220.637 → 22.919 ms). With batches of 100 or more, refresh moved
    by 0.92–1.07x, about the run's noise floor;
  - each of the three changes met its 5% acceptance bar; the progress index
    met it narrowly (5.9%);
  - the speedup over full recomputation is above 2x in 62 of 68 cells,
    including all 48 at 100,000 rows and more;
  - steady-state refresh in the four demos is 1.06–1.37x faster.

### Known limitations

- ivmlite still does not come close to hand-written triggers on apply +
  refresh time: 1.56x slower at best (10 groups, batch 1,000) and 6.65–8.01x
  slower in the other confirmed cells, in the steady state. With one-row
  batches the exploration's median is 22.53x.
- SQLite memory use grows by about 40 KB per open view for its cached
  statements: 38.3 → 46.3 MB at 200 views in one probe.
- Some measurements got slower than in alpha.3, near the run's noise floor:
  steady refresh at 10 views with 1,000-row batches (0.96x and 0.92x),
  bootstrap (0.923–1.002x), and the first refresh after `CREATE` in three
  demos (noop 0.93x, Zcash 0.96x, Kener 0.67x, a noisy figure). Writes were not re-measured with the
  write-amplification workload; no change touched the write path's code.
- Reconnecting many views at once, after a schema change from another
  connection or a rolled-back one, still takes about 2.5 s at 200 views over
  20,000 rows; the work grows about quadratically with the view count.
- A user trigger on an `__ivm_` table that reads an ivmlite view keeps that
  view's cached statements alive, so `sqlite3_close` returns `SQLITE_BUSY`.
  Triggers on reserved tables are out of scope.

## [0.1.0-alpha.3] - 2026-10-09

> **Correction (2026-10-10):** the Kener entry under "Known limitations" below
> was wrong, and the demo figures under "Added" came from the single-refresh
> protocol; see "Fixed" under [0.1.0-alpha.4]. The text below is as released.

### Changed

- **Breaking: shadow-table format 4.** A refresh now applies its staged
  changes with set-based statements that fire once, armed by updating one
  sentinel row of the stage, instead of a trigger that fired once per staged
  row. Databases created by `0.1.0-alpha.2` (format 3) are refused. Their
  views still open as broken views, so `DROP TABLE <view>` works with this
  release loaded: drop every alpha.2 view, then create it again. The
  supported SQL is unchanged.
- A refresh skips its catalog checks (base-table shape, unique indexes,
  capture triggers, output index, apply trigger) while `PRAGMA
  schema_version` is unchanged. It still reads each base table's latch, and
  any schema change, from any connection, makes the next refresh check
  everything again.
- Bootstrap empties its stage table, so a new view no longer holds a second
  copy of its initial state until its first refresh.
- A refresh stages its rows with multi-row `INSERT … VALUES` statements of
  up to 64 rows, bounded by the connection's live
  `SQLITE_LIMIT_VARIABLE_NUMBER` and by 999 parameters.
- `ivmlite-bench plot --from` now names its charts after the exploration
  CSV's file stem, so `m1b-phase5.csv` yields `m1b-phase5-*.svg` beside
  Phase 4's charts instead of overwriting them.

### Added

- Four demand-backed demo benchmarks — FluxFlow, noop, Zcash and Kener — share
  one protocol runner, `crates/ivmlite-test/src/demo_bench.rs`: a checked
  release extension (or an explicit `--extension`), every statement prepared
  before its timer, a warm-up batch reported apart as `first_refresh_ms`, an
  indexed-recomputation baseline, and multiset verification against SQLite.
  FluxFlow lives in `examples/fluxflow_demo.rs`; noop, Zcash and Kener share
  `examples/demand_case_bench.rs`; `real_case_bench` is refactored onto the
  same helpers. `docs/demos/` documents the shared protocol, each demo, and
  its results.
- `scripts/bench-demos.sh`, which runs the demo benchmarks against the
  extension built at several commits; a batch=1, 200-view ablation cell; and
  `scripts/profile-refresh.sh`, which profiles refresh on macOS.
- Published M1b Phase 5 results in [`docs/bench/README.md`](docs/bench/README.md):
  - in the five-build ablation, apply + refresh is 1.35–1.53x faster than in
    alpha.2 in four cells, and 3.52x in the fifth (200 views, 100,000 groups),
    where most of the gain is the first refresh after `CREATE` no longer
    draining the bootstrap stage; the benchmark times that first refresh;
  - the speedup over full recomputation is above 2x in 57 of 68 cells,
    including all 48 at 100,000 rows and more;
  - steady-state refresh in the FluxFlow, noop and Zcash demos is 1.19–1.44x
    faster.

### Known limitations

- ivmlite still does not come close to hand-written triggers on apply +
  refresh time: 1.81x slower at best (10 groups), and 6.64–17.55x slower in
  the other confirmed cells with batches of 100 or more, and thousands of
  times slower with one-row batches at 200 views.
- The Kener demo's steady-state refresh is slower than in alpha.2, by 0.80x
  to 0.90x of its speed in two runs (2.547 → 3.179 ms and 2.512 → 2.794 ms
  medians), although its end to end is unchanged; the cause is not yet
  known.
- Writes to a tracked table still cost more as views are added (about
  59–71 µs per inserted or updated row at 200 views).

## [0.1.0-alpha.2] - 2026-10-08

### Changed

- **Shadow-table format 3.** Each view's output table now carries a
  non-unique index over its output columns, so retracting an output row is an
  index search instead of a full scan of the output (the decisive Phase 4 fix:
  one 100,000-group cell went from 17.6 s to 0.13 s per batch). Databases
  created by `0.1.0-alpha.1` (format 2) are refused; drop and recreate their
  views. Every refresh also checks that the index still exists.
- The REPLACE-capture latch reads `sqlite_schema` once per written row instead
  of four times, cutting write cost by about 1.4–1.5x at 200 views. The trigger
  SQL still parses on SQLite older than 3.44.

### Added

- `ivmlite` as a fourth engine in `ivmlite-bench`, with a fixed timing
  protocol, oracle verification of every view, rotated engine order, and
  `confirm`, `ablation`, `write-amp` and `plot` modes
  (`scripts/bench.sh`, `scripts/bench-ablation.sh`).
- A write-amplification workload (`workloads/write-amp.toml`) with UNIQUE-key
  REPLACE, UPDATE, and both `recursive_triggers` modes.
- Published M1b Phase 4 results in [`docs/bench/README.md`](docs/bench/README.md):
  ivmlite beats full recomputation by more than 2x in 55 of 68 cells (17–909x
  at one million rows), loses in the small-table corner, does not come close to
  hand-written triggers on total apply + refresh time, and has lower write cost
  than hand-written triggers from 50 views up.

### Fixed

- The release workflow now publishes the hand-written notes in
  `docs/releases/<tag>.md`; it looked for the file without the leading `v`, so
  `v0.1.0-alpha.1` was published with generated notes instead.

### Known limitations

- Writes to a tracked table still cost more as views are added (about
  60–70 µs extra per row at 200 views): the latch scans `sqlite_schema`, which
  grows with every view.
- Bootstrap leaves its stage table full until the first refresh, roughly
  doubling the database right after `CREATE VIRTUAL TABLE`.

## [0.1.0-alpha.1] - 2026-09-29

First public technical preview.

### Added

- SQLite loadable extension with `CREATE VIRTUAL TABLE ... USING ivm(...)`.
- Explicit, transactional refresh through the virtual-table command channel.
- Incremental grouped `COUNT(*)` and integer `SUM` with one fixed predicate.
- One two-table inner equi-join, with changes captured from either table.
- Insert, update, delete, REPLACE, bootstrap, reopen, garbage collection, and
  shared base-table capture across multiple views.
- SQL front end with named diagnostics for unsupported queries and schemas.
- Differential tests against SQLite full recomputation and persisted-state
  lifecycle tests.
- Source-backed Datasette, Org-roam, and Taproot Assets case slices with an
  honest full-recompute comparison.
- Linux x86_64 and macOS arm64 release archives with SHA-256 checksums.

### Known limitations

- The supported SQL subset is intentionally narrow; see the README.
- Refresh is explicit rather than automatic.
- Base tables must be `STRICT` and use the supported types and collation.
- The shadow-table format has no migration path during the alpha series.
- The Org-roam join/count case is slower than full recomputation at both
  measured configurations.

[Unreleased]: https://github.com/SFARL/ivmlite/compare/v0.1.0-alpha.6...HEAD
[0.1.0-alpha.6]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.6
[0.1.0-alpha.5]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.5
[0.1.0-alpha.4]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.1
