# Kener worker transaction integration

Continue I01 on `codex/selective-reads`, using the pinned Kener source and native
runtime from the completed October 22 experiment. This is a bounded host
integration experiment, not an upstream application deployment or generic SDK.

## Contract

The actual upstream `BaseRepository.knex` prefers `getWorkerKnex()` over its
constructor connection. Export the pinned context functions through `loadRuntime`
and add a small explicit host transaction entry point in a new module. Its input
connection is the caller-owned Knex pool or transaction; never silently adopt an
ambient connection. Scope repository work with the actual upstream
`runWithWorkerKnex(trx, ...)`. Provide the callback a repository, transaction,
reader (strict by default), and explicit refresh function. Refresh before commit;
the caller must refresh before reading their own pending writes. Nesting passes
the outer transaction explicitly and uses Knex savepoints. Preserve the existing
adapter and measured benchmark code unchanged.

## Tasks

- [x] Implement the host entry point and runtime context exports. Add executable
  native acceptance using actual upstream context: ambient pool precedence,
  success, caller failure, refresh failure, nested savepoint rollback/commit,
  async context isolation/restoration, and reopen/load failure coverage. Use
  independent base/projection/public view assertions. Keep native test runs
  serial. Include the acceptance in the existing CI entry point.
- [x] Complete independent spec review, then quality review; fix any findings.
- [x] Run the integrated native acceptance; record new evidence separately from
  historical benchmark manifests. Update usage/report documentation with the
  tested boundary and remaining application lifecycle work. Independently check
  evidence hashes.
- [x] Commit specific files, push PR #7, and inspect CI.

## Completion checkpoint

Source and local evidence are committed as `41bd49f` and pushed to
[PR #7](https://github.com/SFARL/ivmlite/pull/7). Independent spec, quality and
integrated evidence reviews approved the change. The complete serial native
acceptance passed: existing 5,362 exact result comparisons plus worker 13
comparisons per state layer, 37 context checks and 95 SQL connection identity
checks across 15 transactions. Four focused mutants were detected, with intact
counterparts passing. Historical benchmark artifacts and measured adapter code
remain unchanged. The PR's checks track the Linux/macOS core and native workflows
for each pushed revision; the worker checks run through the existing native
acceptance command.

## Validation and provenance

Use Node >=24.14, exact external owned runtime, pinned clean Kener checkout,
existing release extension (Rust unchanged). Historical source-scale timings
remain tied to commit `3a17bc5`; this work does not rerun or rebind those timings.
The bundled local Node is now 24.22.1 (ABI 137); CI remains pinned to 24.19.0.
No merge, release, tag, or upstream comment is part of this task.
