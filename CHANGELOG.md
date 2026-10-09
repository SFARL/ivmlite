# Changelog

All notable changes to ivmlite will be documented in this file.

The project uses semantic versioning after `0.1.0`; prereleases may change the
SQL subset and persisted shadow-table format without migration.

## [Unreleased]

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

[0.1.0-alpha.2]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.1
