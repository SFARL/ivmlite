# Kener Native Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute unchanged Kener repository methods through its locked native SQLite driver and determine whether a bounded ivmlite adapter helps its actual indexed workload.

**Architecture:** Keep Kener commit `5bc658b771d44f48b61a514cfdb5037468c5aa36` unchanged in an external checkout. An ivmlite-owned Node harness imports its real repository, creates scratch databases using upstream migrations, and compares the existing covering-index query against an optional transactional projection and grouped view. Preserve TEXT tags, nullable REAL latency, arbitrary request boundaries, and explicit refresh semantics; record negative fit and numerical differences honestly.

**Tech Stack:** Node >=24.14, upstream-locked Knex 3.1.0 / better-sqlite3 12.6.2 / dotenv, Node scoped resolution hooks, SQLite loadable extension. All local native commands execute serially.

## File map and boundaries

- `scripts/kener-native/prepare.mjs`: validate pinned upstream checkout, derive minimal dependency closure from its lock, install an isolated runtime without changing upstream.
- `scripts/kener-native/runtime.mjs`: scoped TS/module resolution, native connection lifecycle, import the real repository and selected migrations, record runtime/source identities.
- `scripts/kener-native/adapter.mjs`: optional sidecar facts/triggers, fixed buckets and grouped view, request reader with safe fallback, transactional refresh.
- `scripts/kener-native/acceptance.mjs`: actual method replay, independent baseline database/base-row comparisons, failure/rollback/reopen checks, result and floating point evidence.
- `scripts/kener-native/benchmark.mjs`: deterministic seeded workload, setup outside timings, current indexed baseline, native TEXT-tag adapter read/write/storage comparison.
- `scripts/kener-native/README.md`: exact preparation/run commands, scope and limits.
- `docs/demos/kener-native.md`, `docs/demos/results/2026-10-22-kener-native/`: compact reproducible report and machine-readable evidence.
- `.github/workflows/kener-native.yml`: Linux/macOS real-driver acceptance on pinned Node/source/dependencies, separate from performance measurements.

External checkout/runtime/databases/logs belong under `/Users/weijianfeng/.codex/artifacts/ivmlite-kener-integration-2026-10-22`. No node_modules, native addons or DB files enter Git. No core Rust changes, release/tag, upstream edits or community messages are authorized by this plan.

## Task 1: Locked real-driver and upstream-module execution

- [x] Validate source commit and clean checkout; reject a different commit. Walk every locked dependency from knex, better-sqlite3 and dotenv using npm nested resolution, preserving version/resolved/integrity. Install into an external runtime with Node 24.19.0 and `npm ci`, not a newly resolved lock.
- [x] First demonstrate the missing integration with a failing load/import smoke, then implement `runtime.mjs`. Scope `.js` to `.ts`, extensionless and `$lib/` resolution to pinned upstream parents only. Route their bare dependencies to the isolated runtime. Set `DATABASE_URL` to scratch storage before dynamic import. Do not import Kener's application database singleton.
- [x] Open the native driver through Knex, pool size one, loading on every new connection:

```js
afterCreate(connection, done) {
  try {
    connection.loadExtension(extensionPath, 'sqlite3_ivmlite_init');
    done(null, connection);
  } catch (error) {
    done(error, connection);
  }
}
```

- [x] Execute original migration `up()` functions for initial schema, monitoring indexes, error_message/raw_status additions and covering index, accounting for unrelated tables required by those migrations. Instantiate `new MonitoringRepository(knex)`. Call original insert/upsert and both interval methods, capture actual query SQL/bindings, and run `EXPLAIN QUERY PLAN` on that SQL. Assert shipped covering index presence and native `typeof()` values (TEXT tag/status, REAL fractional latency, INTEGER timestamp, NULL).
- [x] Check explicit load failure and close/reopen load on new connection. Emit JSON with upstream SHA/source hashes, Node/addon/SQLite versions, extension hash, schema, emitted query and plan. Upstream `git status --porcelain` must remain empty.
- [x] Run `node scripts/kener-native/acceptance.mjs --upstream .../upstream --runtime .../runtime --extension .../libivmlite_sqlite.dylib --output .../driver-smoke.json` under the bundled Node path. Review spec, then quality; commit only the task's scripts and plan progress.

## Task 2: Bounded adapter and executable acceptance

- [x] Add a 900-second ordinary sidecar projection maintained by INSERT/UPDATE/DELETE triggers on the migration-created monitoring_data. Compute negative timestamp floor as `timestamp - ((timestamp % 900 + 900) % 900)`. Keep native TEXT tags/statuses, nullable REAL latency, and native row identity. Backfill explicitly, then maintain `COUNT(*), SUM(latency), COUNT(latency), MIN(latency), MAX(latency)` with SELECT/GROUP BY order `(bucket_ts,monitor_tag,status)`. This application-owned layout lets numeric leading window bounds use the existing output index while preserving TEXT identifiers; full-retention windows can still cover all tags. Include setup/storage/write cost.
- [x] Use whole fixed buckets only for safe integer positive requests whose start and interval align to 900 seconds. For other boundaries, invoke the original repository method and report raw fallback. Preserve single tag, merged tag-array and per-monitor grouping, duplicate/empty/missing tags, unknown/null status existence, all types, sort and camelCase mapping. No invented integer tag translation. Default strict JSON mode uses raw fallback for floating-point differences; expose a clearly named experimental numerical mode and report exact versus tolerance comparisons separately.
- [x] Replay the same original calls against independent baseline and adapter databases: insert/upsert, inclusive range update, both confirmation branches, filtered/open-ended/all deletes and retention. Compare ordered base rows after each batch and compare all result fields. Use integer and dyadic fixtures for exact acceptance, plus 0.1/0.2/0.3 and rounding-boundary decimal fixtures to expose numerical limitations. Include zero/negative/NULL latency and SIGNAL second timestamps.
- [x] Exercise transaction visibility, explicit refresh, repeated refresh, caller rollback, nested savepoint rollback, forced refresh failure and reopen. Refresh happens before the same transaction commits; failures must not silently expose stale results. Verify original response computation where import boundaries permit without singleton evaluation; if only repository results are exercised, state that explicitly and do not claim endpoint/UI parity.
- [x] Add discriminating checks: disable an update/delete projection trigger and ensure replay fails; skip refresh and ensure stale-result test fails. Record these small focused mutations separately from the full Rust mutation ledger.
- [x] Run acceptance serially with actual driver and extension. Review spec then quality; fix all material findings before the next task. Commit only task files and updated plan.

Task 2 implementation evidence: `adapter-acceptance.json` nests the retained Task 1
driver smoke and independent adapter acceptance. Local native replay, forced SUM
overflow rollback, reopen and three red/green focused mutants pass. Strict mode
uses the original query; status-split 0.1/0.2/0.3 exposes a real exact AVG
difference in experimental mode. Endpoint/response-builder parity is explicitly
outside scope. Local Node syntax checks, eight ownership regressions, 24 base/
projection/view comparisons and 5,362 exact result comparisons pass; all three
focused mutants are killed with green counterparts. Task 2 spec and quality
reviews are complete; Task 3 proceeds with the approved bounded contract.

## Task 3: Native workload measurement and report

- [x] Add deterministic TEXT-tag workload; include nonnumeric and numeric-looking tags. Time original methods with covering index, adapter numerical reads, strict fallback, base writes and adapter writes plus refresh separately. Prepare data and requests outside each timed interval; actual repository method construction remains part of each call. Use at least three fresh repeats and state row/tag/group counts and journal/synchronous/cache settings. Capture EQP and raw timings; make no endpoint/cache/adoption claim.
- [x] Report bootstrap/backfill time, base/projection/IVM allocated size and retention/correction batch cost. A TEXT-bound scan or disappointing result is a valid finding; do not change core affinity semantics to manufacture a win. Include the actual earlier INTEGER benchmark boundary.
- [x] Write exact commands, pinned source/dependency/extension identities, acceptance matrix, numeric differences, lifecycle evidence and next capability priorities in the report. Source evidence and unchanged-upstream boundary must be independently reviewable.
- [x] Add a Linux/macOS native-driver workflow: checkout ivmlite, Node 24.19.0, Rust 1.95, build the locked extension, fetch/checkout the pinned Kener commit into runner temporary storage, prepare the exact locked runtime, then run ownership tests and acceptance. Upload acceptance evidence; omit benchmarks from CI. Run Node syntax/acceptance checks, `git diff --check`, inspect measurements and upstream cleanliness. Rust core unchanged: retain PR #7's passing Linux/macOS suite as existing core validation, and distinguish pending native-driver CI from completed local evidence.
- [ ] Complete final Task 3 spec/evidence review, then quality review; fix any material findings. Push the authorized feature branch, update PR scope and inspect the new native workflow result. No merge/release/tag or upstream post is authorized.

Task 3 local evidence is complete: three audited fresh repeats each seeded
4,147,200 rows and built 337,379 groups. Paired batch read ratios are 6.04x
merged history, 6.36x grouped history and 3.66x recent grouped day; one-tag reads
lose to the covering index. Bootstrap takes 35.58–39.54 s, live dbstat allocation
is 1.524 GB and the file is 2.323 GB; one-day retention costs 6.43x baseline by
paired median. Every mutation/retention has an additional untimed exact changed-
window audit. The interrupted pre-audit run and both pilots are diagnostic only.
Final October 23 checks pass seven syntax checks, eight ownership checks and full
native acceptance. Readable summaries/rounds and 13 lossless gzip archives bind
raw bytes, code, source and dependency identities through a SHA-256 manifest.
Task 3 reviews, push and remote native-driver CI remain pending.

## Completion criteria

The original upstream repository and real locked native driver run successfully; adapter acceptance and failure/reopen behavior have executable evidence; performance conclusions include native TEXT tags and shipped indexes; limitations determine the next concrete work. Alpha.7 remains preparation only until explicitly authorized for publication.
