# Kener native SQLite repository experiment

This optional ivmlite-owned adapter runs unchanged Kener 4.1.8 repository methods
through its exact locked Knex 3.1.0/better-sqlite3 12.6.2/dotenv 17.2.4 dependency
closure. It measures native TEXT tags and nullable REAL latency against Kener's
shipped covering index. It is experimental, SQLite-only, and carries no upstream
endorsement or adoption claim. Alpha.7 remains preparation; this work publishes
no release or tag.

The source is pinned to
[`5bc658b771d44f48b61a514cfdb5037468c5aa36`](https://github.com/rajnandan1/kener/tree/5bc658b771d44f48b61a514cfdb5037468c5aa36).
The actual
[interval methods](https://github.com/rajnandan1/kener/blob/5bc658b771d44f48b61a514cfdb5037468c5aa36/src/lib/server/db/repositories/monitoring.ts#L513-L664)
and
[covering-index migration](https://github.com/rajnandan1/kener/blob/5bc658b771d44f48b61a514cfdb5037468c5aa36/migrations/20260617120000_add_monitoring_data_covering_index.ts)
are executed directly. The harness imports five original migrations sufficient
for the monitoring schema, including unrelated tables needed by those migrations;
it does not run the entire application migration history. Source hashes, emitted
SQL, bindings and exact plans are in the evidence. The upstream checkout remains
clean. No upstream file, package installation, issue or community post is changed.

## What the measurements mean

**Native batch reads improve, while single-monitor reads lose to the covering index.**
Across three fresh 90-day repeats, paired median elapsed ratios were 6.04x for
batched merged history, 6.36x for grouped history and 3.66x for a recent grouped
day. Single-monitor experimental reads were 3.11x slower for full history and
2.45x slower for a recent day. Strict mode always executes the original SQL.

Local measurements used Apple M2 Pro (10 logical CPUs, 16 GiB), macOS/Darwin
27.0.0 arm64, Node 24.19.0 and SQLite 3.51.2. Observed pragmas on the
migration-created databases were journal `delete`, synchronous `2`, cache size `-16000`, page size `4096`,
`temp_store=0`, and `mmap_size=0`. No benchmark-specific pragma tuning was applied.

Read cells are the median of nine timed calls in milliseconds, followed by the
range of the three repeat medians. The last column pairs original/experimental
medians within each repeat, then shows the median and range of those three
ratios; values below 1 mean experimental is slower. Strict timing differences
reflect execution variance of the same original SQL.

| Repository request | Original ms [repeat range] | Strict ms [repeat range] | Experimental ms [repeat range] | Paired ratio [range] |
| --- | ---: | ---: | ---: | ---: |
| One tag, full 90 days | 43.88 [42.87–44.12] | 45.78 [44.78–45.96] | 135.51 [134.93–137.10] | 0.322x [0.318–0.325] |
| One tag, recent day | 0.576 [0.564–0.598] | 0.594 [0.555–0.624] | 1.44 [1.36–1.46] | 0.409x [0.391–0.425] |
| 32 tags merged, 90 days | 2040.90 [1902.90–2137.02] | 1913.48 [1906.89–2073.31] | 353.79 [318.37–366.21] | 6.040x [5.196–6.411] |
| 32 tags grouped, 90 days | 2491.02 [2413.55–2845.64] | 2270.36 [2183.91–2794.73] | 394.34 [387.64–403.84] | 6.356x [6.226–7.047] |
| 32 tags grouped, recent day | 21.02 [20.80–22.25] | 21.11 [20.54–22.33] | 5.92 [5.33–6.08] | 3.659x [3.514–3.946] |

Full bootstrap/backfill produced **337,379 groups** and took a median **35.71 s**
(35.58–39.54 s). Migration setup took 23.60 ms, native template seeding 15.32 s,
and copying each independent baseline/adapter pair 0.94–0.99 s. These setup
costs are excluded from the read/write results. Separate no-op host refresh
had a 0.086 ms median across 45 calls (0.053–1.880 ms range).

| Actual repository batch | Baseline write body ms | Adapter write body ms | Adapter refresh ms | Baseline write/commit/read ms | Adapter write/refresh/commit/read ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Late correction + new row | 2.59 [1.07–3.75] | 2.52 [1.67–2.83] | 4.75 [4.37–4.83] | 23.97 [23.58–25.45] | 13.60 [13.57–13.85] |
| Inclusive 15-row range | 1.90 [0.767–2.02] | 1.09 [0.939–3.02] | 2.14 [2.07–2.73] | 26.75 [24.55–52.68] | 11.75 [10.38–12.38] |
| Confirmed DOWN → UP recovery | 0.853 [0.344–0.913] | 0.611 [0.527–0.667] | 0.522 [0.447–1.82] | 23.41 [23.17–23.71] | 8.74 [7.36–9.27] |

These are three observations per batch, one in each fresh repeat; brackets are
the observed ranges. The short write-body samples do not establish an isolated
write speed benefit. The end-to-end experiment uses a grouped recent-day read;
its read saving can offset the added refresh for these small writes. Every batch
also passes an untimed all-history-plus-next-day result audit, independent
changed-row expectations and whole-base identity checks.

Actual background retention deleted **46,080 rows per repeat**. Baseline
delete/commit took 111.03 ms (100.16–114.36); adapter delete/refresh/commit took
660.13 ms (644.21–750.87), including a 522.94 ms median refresh
(493.53–568.79). The paired adapter/baseline transaction ratio was **6.43x**
(5.95–6.57x). Retention is a material maintenance cost, separate from read gains.
All three final databases contained 4,101,121 identical native rows with ordered
SHA-256 `f51ee962ef627acd3871604834bd5ebda1f842d3cba0f27a3b50d29f17d22dea`.

| Allocation at bootstrap | Before adapter MiB (pages) | After adapter MiB (pages) |
| --- | ---: | ---: |
| Native base + indexes | 654.88 (167,650) | 654.88 (167,650) |
| Ordinary projection | 0.00 (0) | 140.21 (35,894) |
| IVM diagnostic state + indexes | 0.00 (0) | 658.22 (168,505) |
| Other schema objects | 0.09 (22) | 0.10 (26) |
| Total allocated | 654.97 (167,672) | 1,453.42 (372,075) |
| File size, including free pages | 654.97 | 2,215.64 |

Bootstrap live `dbstat` allocation is 1,524,019,200 bytes, **2.22x** baseline;
the 2,323,263,488-byte file is **3.38x** baseline, with
195,127 free pages (762.21 MiB) after bootstrap. After mutations and retention,
allocated categories and freelist are retained in JSON; the file remains
2,215.64 MiB without vacuum. Storage figures are identical across the three
fresh repeats.

The fixture has 32 tags, including `api/east`, Unicode, `002` and `2`, and one
REALTIME check per minute for 90 days: **4,147,200 rows**. A seeded integer mixer
produces mostly UP, occasional DOWN/DEGRADED/MAINTENANCE/UNKNOWN/null statuses,
nullable latency, and dyadic fractions. Status counts are recorded in JSON.
There are no SIGNAL rows in the timed fixture; second-precision SIGNAL is covered
by acceptance. This fixed workload illustrates a repository tradeoff, not the
distribution or performance of a production Kener deployment.

Three independent baseline/adapter database pairs are copied from a closed,
migration-created seeded template. Native prepared-statement bulk seeding is
setup and never an application write result. Every read case runs one warmup per
path and three timed rounds per repeat, reversing path order between rounds and
repeats. Exact comparisons, plans, hashing, bootstrap and file copying are outside
read timers. OS cache is shared and not evicted; results are warm repository
calls on a shared desktop, not cold storage or endpoint latency. Native jobs run
serially; this does not establish system-wide quiescence. Original SQL must show the covering
index in every captured baseline plan. Both interval methods run unchanged in
the original arm and in the strict fallback arm.

No-op host refresh cost is recorded separately from reads. Application writes
use the actual original repository insert/upsert, inclusive 15-row range update
and confirmation recovery. Adapter capture and refresh run in the same Knex
transaction before commit. End-to-end results include that transaction's commit
and a grouped recent-day read. Each batch checks independently constructed
changed native rows, both query results, and streaming ordered whole-base SHA-256
identities. An additional untimed all-history-plus-next-day query observes old
corrections, recovery and the new boundary row after every batch and retention;
the timed recent-day request excludes some of those changes. Frozen-clock actual
`background(90)` at seed start + 91 days deletes
exactly the first day's **46,080** rows; cutoff rows remain. This is actual
background retention, not an unlabelled synthetic delete. Retention runs once
per fresh repeat, with write, refresh and total transaction times separated.
Allocated `dbstat` pages include all table/index allocations and separate
base, projection, IVM diagnostic state and other schema objects; free pages and
file size are also retained. Reading diagnostic allocation names does not expose
private shadow contents as an application query API.

## The TEXT-tag access-path finding

The 7-day diagnostic pilot used 322,560 rows and 26,275 groups. Plain
`monitor_tag IN (...)` caused SQLite's public VDBE to loop an ephemeral tag list
around `VFilter(sr1:0=ge,0=lt,1=eq)` and `VNext`. The extension's conservative
TEXT candidate handling preserves coercion correctness; here the candidate scan
repeats for each tag. In the first pilot, batch merged full history cost
321.87 ms for plain IN versus 20.85 ms for unary-plus membership, while the
original indexed repository cost 119.34 ms. Grouped full history was
315.84/24.54/176.89 ms respectively. These are pilot medians, separate from the
final source-scale results.

The adapter now uses ordinary outer SQL `+monitor_tag IN (...)`. The public plan
passes only the numeric bucket window to xFilter; TEXT membership is checked
once per emitted candidate by SQLite. Unary plus preserves the virtual column's
no-affinity native TEXT comparison. Both methods, numeric-looking/Unicode/empty
tags, duplicate/missing selections and exact result shapes pass durable
acceptance. This is a small host SQL choice with retained before/after pilot
EQP/VDBE and raw timings. No Rust affinity rule or private output-table access
was changed. Bucket-first grouping remains the default; no tag-layout matrix
was needed to resolve this measured issue. The superseded first pilot retains
its original code hashes and is explicitly diagnostic; its original storage
category labels had an `_ivm`/`__ivm_` prefix accounting bug and must not be used
for storage conclusions. The optimized pilot and source-scale run fix that
classification and assert category totals.

## Correctness and host contract

`createReader()` defaults to **strict**, which always invokes the original raw
repository method and preserves exact JSON-number behavior. Maintaining the
adapter still adds setup/write/storage cost in strict mode. The explicit
`experimental-numeric` option only uses whole 900-second buckets for safe integer
aligned windows; unaligned/unsafe/unsupported requests use the original method.
Queries alone never refresh. The host must bind the repository and refresh to
the same transaction and load the extension on every new connection. Installing
capture state makes extension availability a dependency of future writes, even
when reads fall back. Deployment, migration and connection ownership need an
explicit host policy.

The original standalone benchmark has no active worker `AsyncLocalStorage`
context. In Kener background jobs,
[the repository getter](https://github.com/rajnandan1/kener/blob/5bc658b771d44f48b61a514cfdb5037468c5aa36/src/lib/server/db/repositories/base.ts#L38-L56)
prefers the worker context over the constructor's Knex argument. Passing `trx`
to the constructor alone is insufficient there. The subsequent
[`kenerTransaction` host entry point](../../scripts/kener-native/host.mjs)
binds the actual upstream `runWithWorkerKnex(trx, ...)` for repository work,
uses the caller-selected connection, and refreshes before commit. It exposes
an explicit refresh for reads of pending writes and keeps strict reads as the
default. Nested calls pass the outer transaction explicitly to use a savepoint.
It does not start Kener's application singleton, queue processors, cache or UI;
their deployment and lifecycle remain outside this experiment.

| Acceptance boundary | Local evidence |
| --- | --- |
| Native migrations, TEXT/REAL/NULL, shipped index, source/runtime ownership | Actual Knex/better-sqlite3 driver, eight ownership checks |
| Independent native mutation replay | 24 base, 24 projection and 24 grouped-view comparisons |
| Repository result checks | 5,362 exact comparisons; 2,856 strict and 2,856 experimental calls |
| Focused controls | UPDATE trigger, DELETE trigger and skipped refresh: all three killed; intact counterparts pass |
| Transaction lifecycle | Explicit refresh/visibility, repeated refresh, caller rollback, nested savepoint rollback, forced real SUM-overflow atomic failure and reopen |
| Numeric limitation | Actual raw AVG `0.19999999999999998` versus merged `0.20000000000000004`; strict remains raw |
| Final access path | Both batched methods assert numeric-window-only xFilter plus outer TEXT membership |
| Platform coverage | Local macOS native driver; Linux x64 and macOS ARM64 native CI passed at source commit `3a17bc5` |

The October 23 worker acceptance adds 13 independent comparisons of each state
layer, 37 context checks and 95 native query identity checks across 15 host
transactions. It exercises caller and real precommit refresh failure, both
savepoint rollback directions, controlled overlapping async contexts, explicit
numeric read freshness and native reopen. Removing the actual worker binding
routes a repository write to the ambient database; the independent state oracle
detects that mutant, and the intact counterpart passes. Complete serial driver,
adapter and worker acceptance passed locally on Node 24.22.1/SQLite 3.51.2.
The new [worker evidence](results/2026-10-23-kener-worker/) has its own source and
byte hashes, separate from the historical benchmark artifacts.

The connection policy adds an optional expected extension SHA-256 and explicit
per-handle SQLite `busy_timeout`. A supplied hash rejects an unexpected artifact
before loading; without it, loading remains unpinned. This is exact file identity,
not semantic-version/ABI negotiation or database format migration. The host owns
an immutable approved artifact path for the process lifetime and a process restart
when deploying another build. Initialization failures close the native
handle before pool handoff. SQLite lock waiting and Knex pool acquisition use
separate timeouts, and the host transaction helper never retries a work callback.
The synchronous driver blocks its Node event loop while waiting for a lock;
independent-process writers are needed to test release during that wait. These
checks keep SQLite's default rollback journal and do not claim concurrent writer
commits, WAL coverage or general retry safety.

The [connection acceptance evidence](results/2026-10-23-kener-connection/)
records 26 invalid-policy checks, 11 native timeout checks, 13 comparisons per
state layer, 10 context checks and 28 SQL connection identity checks. Independent
processes share one database: a held writer causes actual `SQLITE_BUSY` and a
clean rollback; an explicit new caller transaction succeeds after release. A
child timer armed by the parent's actual INSERT permits a waiting writer to
succeed. Same-key corrections, pool recreation, per-handle artifact replacement
rejection, subsequent refresh and reopen preserve independently expected facts.
Both new source-mutation controls are detected, with intact counterparts passing.
The complete four-section local acceptance also retains the previous adapter and
worker results. Elapsed lock-wait observations are diagnostic, not latency bounds.

The finite decimal acceptance bound `1e-12 * max(1,abs(expected))` applies only
to fixture sums/averages; timestamps, counts and extrema stay exact. It is not a
general guarantee for arbitrary REAL magnitude, cancellation, overflow or
reassociation. Timed dyadic fixtures require exact equality for every measured
result. Unknown/null status groups and all non-null latency values contribute,
matching upstream's repository behavior. No status/type filter is invented.

The harness does not execute the endpoint, response builder, application
singleton, Redis cache or UI. Kener already has
[batched client requests/caching](https://github.com/rajnandan1/kener/blob/5bc658b771d44f48b61a514cfdb5037468c5aa36/src/lib/client/monitor-bar-client.ts)
and a
[60-second grouped controller cache](https://github.com/rajnandan1/kener/blob/5bc658b771d44f48b61a514cfdb5037468c5aa36/src/lib/server/controllers/monitorsController.ts#L854-L874).
Repository gains do not establish an end-user page gain. Latest status, raw day
details, latency charts and alert lookbacks retain different sequence/filter
semantics and remain outside this adapter. Earlier synthetic INTEGER-tag results
(including 131x/1077x figures) in [the selective-read demo](selective-reads.md) are not native Kener
performance claims.

## Reproduction and next work

Exact preparation, ownership, acceptance, pilot and source-scale commands are in
[`scripts/kener-native/README.md`](../../scripts/kener-native/README.md). Use
Node 24.19.0, an extension built for the host, a pinned clean upstream checkout
and external runtime/scratch/evidence directories. The checked-in
[evidence directory](results/2026-10-22-kener-native/) contains readable summary
and rounds metadata, plus losslessly compressed acceptance, ownership, pilot and
full raw-round JSON/logs with identities and provenance. The
benchmark records actual SQLite, Node, dependency, addon, extension, source and
script identities. The manifest binds both compressed bytes and exact
uncompressed bytes to their SHA-256 and size. Raw evidence retains machine paths
as execution provenance; use the generic reproduction paths in the README.

Those measurements and their historical manifests remain pinned to source
commit `3a17bc5`, including its runtime/helper hashes. The later host transaction
entry point has separate acceptance evidence and has not been timed by that
source-scale benchmark. Changing the current loader does not rebind historical
measurements to the new source.

```sh
gzip -dc docs/demos/results/2026-10-22-kener-native/benchmark.json.gz > /external/kener/benchmark-decoded.json
gzip -dc docs/demos/results/2026-10-22-kener-native/acceptance.json.gz > /external/kener/acceptance-decoded.json
shasum -a 256 /external/kener/benchmark-decoded.json
```

The separate [native workflow](../../.github/workflows/kener-native.yml) builds
the locked extension with Rust 1.95.0 and tests Node 24.19.0 on ubuntu-24.04 and
macos-14. It fetches the exact upstream commit into runner temporary storage,
installs only the owned locked dependency closure, runs ownership plus full
acceptance, and uploads JSON/logs on success or failure. Benchmarks remain local;
path filters exclude documentation, while a PR's changed-file comparison may
still include earlier code changes. The unchanged Rust core uses
PR #7's Linux/macOS 876-test validation; this work adds no new claim of an
unchanged-core local rerun. At source commit `3a17bc5`, both
[native driver jobs](https://github.com/SFARL/ivmlite/actions/runs/39961750299) and
both [core regression jobs](https://github.com/SFARL/ivmlite/actions/runs/39961750396)
passed. [CI provenance](results/2026-10-22-kener-native/ci.json) records the exact
commit, jobs, runtime identities and assertions; the two platforms' complete
acceptance and ownership JSONs are archived losslessly with manifest hashes.

The measured host burden makes **I01 host integration** the immediate practical
priority. The experimental loader and host entry point cover connection loading
and transaction-final refresh at the repository boundary. Application queue
startup/shutdown, deployment/migration policy and cache invalidation still need
host integration. **F02 computed expressions** would remove
some application-owned bucket projection work only after the supported floor,
nullable REAL and grouping semantics are proved. **F01 composite filters** are a
separate maintained-query capability; outer SQLite CASE/IN/AND used here already
works, so this adapter does not establish that expanding F01 is required for
this slice. These IDs follow the separately maintained capability matrix; this
report does not silently extend the core SQL contract. General REAL parity and
single-monitor performance remain material limits before deeper application
integration or release positioning.
