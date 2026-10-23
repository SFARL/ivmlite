# Native Kener repository harness

This optional, ivmlite-owned experiment executes unchanged Kener 4.1.8 source
at `5bc658b771d44f48b61a514cfdb5037468c5aa36`. It imports the actual
`MonitoringRepository` through scoped Node resolution hooks and native type
stripping. It does not run Kener's database singleton, endpoint/response builder,
Redis cache or UI. The driver smoke establishes driver compatibility and the indexed repository
baseline. The adapter acceptance independently replays native repository calls
and tests derived state and lifecycle behavior. Neither establishes a performance
benefit or upstream adoption.

Use Node >=24.14, npm, Git, an extension built for the host, and a clean upstream
checkout at the pinned commit. Keep upstream, the npm runtime, evidence and
scratch databases outside the ivmlite checkout. Substitute absolute paths below;
the harness commits no local environment paths. The first npm run can download
or build the native better-sqlite3 addon (a compiler toolchain may be needed).

```sh
export PATH="/absolute/path/to/node/bin:$PATH"
node --version
git clone https://github.com/rajnandan1/kener.git /external/kener/upstream
git -C /external/kener/upstream checkout --detach 5bc658b771d44f48b61a514cfdb5037468c5aa36
node scripts/kener-native/prepare.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime
node scripts/kener-native/acceptance.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --extension /absolute/path/to/libivmlite_sqlite.dylib \
  --scratch /external/kener/scratch \
  --output /external/kener/adapter-acceptance.json
node scripts/kener-native/prepare.test.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --output /external/kener/ownership-tests.json
```

Use the host's `.so`/`.dylib` extension path. `prepare.mjs` derives the exact
dependency closure of upstream's locked Knex 3.1.0, better-sqlite3 12.6.2 and
dotenv, preserving complete package records including nested dependencies,
integrity, resolved URLs and metadata. It uses `npm ci` in the owned external
runtime. Repeating preparation accepts an empty directory or a runtime with a
valid ownership marker, matching manifest and exact lock; other nonempty
directories are rejected before writing or installing. Repeating preparation
reinstalls that runtime; it never installs into
upstream. Peer database drivers are optional and are not required by SQLite.
The run checks installed package versions and rejects manifest/lock drift.

The acceptance command makes a fresh directory per run (`--scratch` is its
parent, defaulting to the OS temporary directory) and retains databases for
inspection. It runs five original migration `up()` functions, including the
initial migration's unrelated tables required by later migrations. Its evidence
contains source hashes, dependency/addon/extension identities, SQLite versions,
native schema/storage classes, original SQL and bindings, covering-index plans,
upsert results, failed extension loading and load-on-reopen checks. Both interval
methods are called without copying their SQL. Migrations swallowing duplicate
index errors are compensated by schema and actual plan assertions.

Run native preparation/acceptance serially. The JSON records failure and the
command exits nonzero if a check fails. The SQLite runtime uses one Knex pool
connection and loads `sqlite3_ivmlite_init` on every newly opened connection;
load failure closes the native handle and rejects connection creation. The
ownership regression command checks byte-preserving rejection of unowned and
corrupt directories, installs an empty runtime, then reinstalls the valid owned
runtime. `runtime.mjs` exports `loadRuntime`
and migration/covering-index constants for further independent replay tests.

### Connection policy

The loader accepts an explicit fourth argument on `createDatabase`:

```js
const db = runtime.createDatabase(filename, extension, lifecycle, {
  expectedExtensionSHA256: approvedExtensionSHA256,
  busyTimeoutMs: 5000,
});
```

`approvedExtensionSHA256` comes from the host's trusted deployment artifact
manifest, using the hash for its platform. If supplied, it must be a lowercase
64-digit SHA-256. Invalid policy values and unknown keys are rejected before
creating the pool. Each new native connection checks the extension file against
it before loading. A mismatch rejects initialization and closes that native
handle before it reaches the pool. Omitting the hash preserves unpinned loading.
The extension has no semantic-version query function: this policy enforces exact
artifact identity, not version-range/ABI negotiation or database format migration.
Use an immutable approved artifact path for the process lifetime and restart to
deploy another build. A path hash does not protect against concurrent file
replacement during native loading or attest an already loaded native image.

`busyTimeoutMs` is SQLite's per-connection lock wait, a nonnegative integer up
to 2147483647, defaulting to 5000 ms. Initialization sets and verifies
`PRAGMA busy_timeout` on every native handle; zero means no waiting. The separate
Knex `acquireConnectionTimeout` remains 2000 ms and governs waiting for a pool
connection. Neither setting retries a failed host callback. A busy error must
roll back the transaction; any new attempt is an explicit caller decision.
SQLite still serializes writers. With this synchronous driver, lock waiting
blocks the calling Node event loop, so another connection's timer on that same
event loop cannot release a lock while the call waits.


The acceptance JSON contains `driverSmoke` (the original compatibility checks),
`adapter` (independent replay, lifecycle, numeric and focused mutation results),
`worker` (the actual upstream async context and host transaction boundary), and
`connection` (artifact policy and independent-process writer contention).
Run this command serially after preparation/ownership checks:

```sh
node --check scripts/kener-native/adapter.mjs
node --check scripts/kener-native/adapter-acceptance.mjs
node --check scripts/kener-native/acceptance.mjs
node scripts/kener-native/acceptance.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --extension /absolute/path/to/libivmlite_sqlite.dylib \
  --scratch /external/kener/scratch \
  --output /external/kener/adapter-acceptance.json
```

`installAdapter(db)` creates an ordinary projection keyed by the original SQLite
rowid, backfills existing rows, installs three AFTER triggers and creates the
public `kener_ivm_900` virtual table in one transaction. It preserves native TEXT
tags/statuses and nullable REAL latency; it includes every sample type. The
ordinary INTEGER bucket uses true floor for negative timestamps. UPDATE deletes
the OLD projection fact then inserts NEW, including rowid or composite-key
changes. It uses no REPLACE. The view groups `(bucket_ts, monitor_tag, status)`
and explicitly aliases sample count, latency sum/non-null count and extrema.
Bucket comes first to allow the existing numeric window access path. Application
reads query the public virtual table, never private IVM shadow tables.
Installation is deliberate and one-time: a second installation fails atomically.
Reopening loads the extension and reuses persisted state without rebootstrap.

`createReader(db, originalRepository)` defaults to `mode: 'strict'`. Both
repository-shaped methods return exactly the original row shape and always call
the original repository in this mode, including for aligned requests. For
provenance call `reader.read(grouped, tags, start, interval, points)` and inspect
its separate `{ rows, provenance }` result, or provide an `onRead` callback.
There is no general exact-JSON-number guarantee for merged REAL aggregates.
Installing maintained state still adds write/storage/refresh cost even when
strict reads use raw fallback.

The explicit opt-in `{ mode: 'experimental-numeric' }` uses whole fixed buckets
only for safe-integer start, positive interval, nonnegative points, safe span/end,
and start/interval divisible by 900. Unsupported tag input or any failed window
check invokes the actual original method with the unchanged arguments; the
provenance explains `unsafe-or-invalid-window`, `unaligned-window` or
`unsupported-tags`. Empty/missing/duplicate tags retain original semantics.
Single-method arrays merge monitors while the grouped method keeps monitors
separate. The outer SQL pivots four known statuses but retains other/null status
groups for existence and latency, weights average by non-null latency count,
preserves extrema, omits missing buckets, sorts and applies `Number(...) || 0`
as upstream does. Unaligned anchors, short intervals and arbitrary API windows
are raw fallbacks. Timezones with :30/:45 offsets are tested as aligned offsets;
this does not replace upstream timezone/day-boundary logic.

The host must explicitly refresh. Queries alone do not refresh and can return
stale experimental results after a write. For Kener worker code, use the host
entry point with the context functions returned by `loadRuntime()`:

```js
import { kenerTransaction } from './host.mjs';
await kenerTransaction(db, runtime, async ({ repository, reader, refresh }) => {
  await repository.insertMonitoringData(sample);
  await refresh(); // Required before reading pending writes through the IVM view.
  const rows = await reader.getStatusCountsByInterval(tags, start, interval, points);
  return rows;
}); // Also refreshes inside this transaction before commit; strict reader is default.
```

Upstream `BaseRepository.knex` prefers `getWorkerKnex()` over its constructor
argument. Inside a real background job, passing `trx` to the constructor alone
does not bind its queries to that transaction. `kenerTransaction` scopes work
with the actual upstream `runWithWorkerKnex(trx, ...)`, overriding an ambient
worker pool for the callback and restoring it afterwards. Its first argument
explicitly selects the caller-owned pool or transaction; it never silently
adopts the ambient pool. Pass `{ mode: 'experimental-numeric' }` as the fourth
argument to opt in to maintained reads and their numeric limits. Await all work
inside the callback; do not retain its transaction-bound objects for later use.
The callback also exposes `connection`; pass it as the first argument to a nested
`kenerTransaction` call to create a savepoint.

The lower-level `adapterTransaction` remains available for callers without a
worker context. It only passes `trx` to the constructor and does not bind an
ambient Kener context. The original benchmark uses that standalone path. The
host acceptance uses the pinned upstream async context, without starting the
application singleton or BullMQ queue lifecycle.

For explicit control use `refreshAdapter(trx)` inside the caller's transaction;
read-after-write experimental queries need that call before reading. Never
refresh through the pool while holding a transaction connection. Caller errors,
repository errors and refresh errors propagate and roll back. There is no silent
stale-data/failure fallback. Every connection must load the extension before
accessing tracked state. A separate max-INTEGER SUM view, sorted after the adapter view, forces a real
`ivm_refresh_all()` overflow in acceptance: atomic refresh fails and restores the already-refreshed adapter view inside
the still-open transaction, then the enclosing transaction rolls back base/projection/pending IVM state, and the next refresh
succeeds. Other checks cover pre/post-refresh visibility, repeated refresh,
caller rollback, Knex nested-savepoint rollback after refresh followed by
continued writes/refresh/commit, and close/reopen.

Acceptance uses separate baseline/adapter databases and independently executes
the original insert/upsert, inclusive range update (deviation zero), both status
confirmation branches, filtered/open-ended/all-monitor/all-data deletion and
actual `background()` retention. It compares every ordered native base row,
projection fact and grouped view after each batch, plus both repository result
methods. A fixed clock only during the two actual retention calls prevents
minute-rollover flakes and tests strict less-than-cutoff behavior. An additional
SQL probe covers changes to native rowid and composite keys because upstream
has no method for that operation. Fixtures include all eight sample types,
second-precision SIGNAL, dyadic REALs, negative/zero/NULL latency, all-NULL
groups, unknown/null statuses, sparse buckets, negative timestamps, empty and
Unicode/numeric-looking TEXT tags.

Dyadic fixtures require exact ordered equality. Decimal fixtures include
0.1/0.2/0.3 and decimal rounding boundaries. The native run must expose at least
one exact number mismatch; JSON records original, strict and experimental rows
and every numeric difference. For example the status-split decimal fixture
produces raw AVG `0.19999999999999998` versus merged `0.20000000000000004` with
this locked driver. Experimental acceptance bounds finite fixture error only for sums/averages by
`1e-12 * max(1, abs(expected))`; timestamps, counts and extrema require exact
equality; this is a test bound, not a guarantee for arbitrary
input magnitude, cancellation or REAL data. Comparator guards reject unequal
NaN/Infinity (only `Object.is` identical exceptional values are accepted).
Strict results remain exactly equal to the original decimal result.

Three durable focused mutation checks disable the actual UPDATE trigger, disable
the DELETE trigger, and omit refresh in separate scratch databases. Each must
fail the intended projection/freshness assertion, and an intact counterpart must
pass. The JSON includes actual red failure messages and green outcomes, separate
from the Rust mutation ledger. No endpoint, response builder, cache or UI is
executed: the original response builder imports the application singleton and
remains outside this repository-only experiment. Setup/storage/write costs and
performance measurement are a separate task.

## Native measurement

Run the small pilot before the source-scale run. The checked-in configurations
declare 7 days/one repeat and 90 days/three repeats, respectively; both use 32
native TEXT tags, nullable dyadic REAL latency and three timed rounds per read
case. The source-scale template has 4,147,200 REALTIME rows. SIGNAL second rows
are covered by acceptance and are not added to this performance fixture.

```sh
node --check scripts/kener-native/benchmark.mjs
node scripts/kener-native/benchmark.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --extension /absolute/path/to/libivmlite_sqlite.dylib \
  --scratch /external/kener/scratch \
  --output /external/kener/pilot.json \
  --config scripts/kener-native/pilot.json
node scripts/kener-native/benchmark.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --extension /absolute/path/to/libivmlite_sqlite.dylib \
  --scratch /external/kener/scratch \
  --output /external/kener/benchmark.json \
  --config scripts/kener-native/source-scale.json
```

All commands run serially. The template is created by actual migrations with the
shipped covering index, seeded in a single native prepared-statement transaction
and closed. Seeding is setup, not an application write benchmark. Fresh baseline
and adapter database copies are made for every repeat. Adapter installation
includes projection backfill, trigger creation and full IVM bootstrap. Copying,
setup, query preparation/capture, warmup, exact comparisons and base hashes stay
outside measured read intervals. Each path is warmed once; timed path order
reverses across rounds/repeats. OS cache is shared and is not evicted. Results
describe warm repository calls without application caches or HTTP work.

The original repository executes the baseline and strict paths. Experimental
queries use the public virtual table with numeric bucket-window constraints;
`+monitor_tag IN (...)` keeps native TEXT membership in outer SQLite SQL and
avoids repeated xFilter candidate scans per tag. The pilot records both plain
and unary-plus SQL, EQP, VDBE and raw timings. No internal IVM contents are used
for application reads. `dbstat` reports allocated pages by base/index,
projection and internal diagnostic state; this is storage accounting only.

Write measurements use actual original insert/upsert, inclusive range update and
confirmation methods; adapter writes capture and refresh in the same Knex
transaction before commit. End-to-end timings include commit and a grouped
recent-day read. Every batch verifies independently constructed changed rows,
query results and whole-base ordered SHA-256 identities. An additional untimed
all-history-plus-next-day query checks the changed old/new aggregate windows,
which are deliberately outside the timed recent-day request. Actual `background()`
retention runs with a frozen clock at seed start + days + 1, deleting one day
through the upstream strict-less-than cutoff. It reports repository deletion,
host refresh and total transaction times separately. No-op refresh is reported
separately from warm reads. This benchmark is not a general REAL-number parity
test; its dyadic fixture requires exact equality, while decimal limitations are
recorded by acceptance.

See [the native report](../../docs/demos/kener-native.md) for measured tradeoffs.
The October 22 benchmark evidence remains pinned to source commit `3a17bc5` and
its recorded helper hashes. The subsequent worker integration has separate
acceptance evidence; those timings do not measure the worker entry point.
The separate `kener-native` Linux/macOS workflow executes ownership and full
real-driver acceptance with Node 24.19.0/Rust 1.95.0, uploads JSON/logs on every
outcome, and deliberately omits benchmarks. Its path filters select code,
configuration and workflow files, excluding documentation paths. GitHub's PR
changed-file comparison can still include earlier code changes in the PR.
