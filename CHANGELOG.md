# Changelog

All notable changes to ivmlite will be documented in this file.

The project uses semantic versioning after `0.1.0`; prereleases may change the
SQL subset and persisted shadow-table format without migration.

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
