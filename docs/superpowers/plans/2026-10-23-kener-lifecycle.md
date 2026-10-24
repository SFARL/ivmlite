# Kener migration, uninstall and restart implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development with independent spec and quality reviews.

**Goal:** Safely remove/rebuild the experimental sidecar, retain original rows,
and prove recovery around process death before and after a migration commit.

**Architecture:** Add a separate lifecycle module; preserve measured adapter,
reader and benchmark source byte-for-byte. Use explicit caller connections and
Knex transactions/savepoints. Run migrations inside the real Kener worker scope,
then reinstall/backfill before the outer commit. Acceptance uses isolated scratch
databases and independently expected base/projection/public-view facts.

**Stack:** Existing Node/Knex/better-sqlite3 runtime, pinned Kener source, unchanged
extension, default SQLite rollback journal. No database-format upgrade, upstream
application changes, implicit retry, service or release.

## Contract and files

- New `scripts/kener-native/lifecycle.mjs` exports `uninstallAdapter(connection)`
  and `migrateAdapter(connection, runtime, work)`. The first requires the intact
  known adapter footprint and atomically drops its public view, three host
  capture triggers and ordinary projection. It leaves the original table, its
  rows/indexes and unrelated objects unchanged. Do not manually remove private
  IVM metadata: use public DROP and let the extension manage shared state.
- Check the five expected objects against the current installer's explicit SQL
  definitions (normalizing harmless whitespace only), refusing absent/drifted
  footprints. After transactional public-view/host-trigger removal, conservatively
  reject remaining schema SQL that references either owned table before dropping
  the projection. Other maintained views depending on the projection, ordinary
  views/indexes/triggers and foreign-key dependents must not be silently broken.
  Such rejection rolls back every tentative DDL change. Unrelated maintained
  views over the original source must survive. This is an exclusive-sidecar
  ownership contract, not a general SQL dependency resolver or repair facility.
- `migrateAdapter` uses the existing `kenerTransaction` so the actual upstream
  worker context is bound to the explicit connection. Uninstall, call
  `work({connection,repository})`, reinstall/backfill with unchanged installer,
  and refresh before commit. Return the callback value. A caller, schema or
  reinstall error rolls back schema, source data and maintained state. Passing
  an outer transaction uses a savepoint; no ambient connection adoption.
- New `scripts/kener-native/lifecycle-acceptance.mjs`: export runner only; use the
  existing guarded CLI as the sole evidence-writing entry point.
- New bounded child-process fixture, e.g. `lifecycle-worker.mjs`: run an actual
  migration/write, signal readiness before outer commit or after successful
  commit while its DB handle remains open. Parent kills it with SIGKILL, awaits
  exit, then reopens. No reliance on finally/clean shutdown for recovery proof.
- `acceptance.mjs`: fifth serial `lifecycle` section. Existing native CI inherits
  it. README/report plus a new result directory record source/byte provenance.

## Tasks and acceptance

- [x] Implement lifecycle functions and native controls. A second uninstall or a
  missing/modified object is refused without collateral changes. Verify successful
  uninstall with pending writes preserves base facts, removes owned objects and
  leaves the original public schema. Open a fresh bare Knex/better-sqlite3 pool
  **without loading the extension**, exercise actual repository writes and reads,
  close it, then reinstall/backfill all rows correctly.
- [x] Test successful migration adding an ordinary unreferenced column and
  source-row/key changes, return value, worker context restoration, repeated
  rebuild and nested outer rollback. Test callback failure and forced reinstall
  failure: exact old schema/state restoration, then valid write/refresh succeeds.
- [x] Test unrelated maintained-view preservation and refusal of foreign
  dependents on projection/public view; do not weaken ownership checks to make
  cleanup succeed. All operations use scratch copies, no real deployment DB.
- [x] Add deterministic precommit/postcommit child crash tests around migration
  rebuild. Reopen verifies old schema/state before commit, new schema/state after
  commit, independently expected raw rows and grouped results, and clean next
  write/refresh. Bound readiness/exit waits and clean up every child/handle after
  errors. A focused real-source mutation control must fail the intended lifecycle
  assertion and an intact counterpart pass; avoid unrelated setup-error reds.
- [x] Reject TEMP source/owned-name shadows before any work and before reinstall;
  compare exact main+TEMP snapshots and independently expected main facts after
  refusal. Existing and callback-created shadows preserve data and context.
- [x] Complete independent spec/quality/final review, run the full five sections
  serially, preserve historical evidence and archive exact new JSON/log/source
  hashes. Check staged diff including new files.
- [x] Commit specific files, push PR #7, inspect latest Linux/macOS core/native
  checks. No merge, tag or release.

## Validation

Use the bundled Node (currently 24.22.1; CI 24.19.0), exact owned external runtime
and pinned clean upstream; keep output/scratch outside both repositories. The
current release extension is unchanged; native test ownership stays with one
agent. Run syntax checks for each changed module and the full guarded command:

```sh
node scripts/kener-native/acceptance.mjs --upstream /external/kener/upstream --runtime /external/kener/runtime --extension /absolute/path/libivmlite_sqlite.dylib --scratch /external/kener/lifecycle --output /external/kener/lifecycle/acceptance.json
git diff 312742e --check
```

Expected: all five serial sections pass. Lifecycle proof is bounded to the
rollback journal, local processes and compatible schema rebuilds. Queue/cache/UI
lifecycle and database format migrations remain separate host work.

Local completion: all five native sections passed with lifecycle counts 69 base,
59 projection/view, 15 schema, 20 complete main+TEMP comparisons, 27 context,
8 dependency refusals, 6 ownership refusals, 3 TEMP refusals and 2 actual SIGKILL
recoveries. Seven focused mutation controls detect their intended regressions;
the eight runtime ownership checks pass. Independent spec, quality and final
evidence reviews approved; staged new files pass whitespace checks. Complete
snapshot comparisons run in memory; raw archives retain captured SQL, recovery
facts/schema and counters. Historical evidence and adapter/benchmark/runtime/host
source remain unchanged.

The reviewed implementation and local evidence were pushed as `7188729` to
[PR #7](https://github.com/SFARL/ivmlite/pull/7). The
[current PR checks](https://github.com/SFARL/ivmlite/pull/7/checks) track core and
five-section native acceptance on both Linux and macOS for each pushed revision.
The final review corrected the archive wording to distinguish compared in-memory
snapshots from captured raw evidence. No application queue/cache lifecycle,
extension format upgrade, merge or release is included in this checkpoint.
