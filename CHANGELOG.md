# Changelog

All notable changes to ivmlite will be documented in this file.

The project uses semantic versioning after `0.1.0`; prereleases may change the
SQL subset and persisted shadow-table format without migration.

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

[0.1.0-alpha.1]: https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.1
