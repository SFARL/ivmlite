# Real-world demo roadmap

Research snapshot: 2026-10-08.

This document ranks public workload reports that could become ivmlite demos.
They are demand evidence, not customer interviews, endorsements, adoption, or
proof that the reporters want to use ivmlite. A demo may adapt a workload to
the current SQL subset, but it must keep that adaptation visible.

## Evidence levels

Use these labels consistently:

- **Public database:** a downloadable database containing the reported
  workload. None of the candidates below currently has one.
- **Public generator:** upstream code can deterministically create a
  source-shaped database at the reported scale. FluxFlow has the strongest
  example.
- **Public source/query:** schema and query code are public, but production
  data is not. A synthetic fixture is still required.
- **Issue-reported numbers only:** scale and timings come from the reporter;
  there is no public large fixture with which to reproduce them.

Synthetic data can validate behavior and scaling. It cannot be presented as a
reproduction of a production deployment.

## Priority

| Priority | Candidate | Evidence available | Current fit | Decision |
|---|---|---|---|---|
| P0 | [FluxFlow #3](https://github.com/2ndtlmining/fluxflow/issues/3) / [PR #45](https://github.com/2ndtlmining/fluxflow/pull/45) | Public generator and source; no public production DB | A normalized grouped `SUM`/`COUNT` slice fits; retention semantics do not | Implemented |
| P0 | [Taproot Assets #642](https://github.com/lightninglabs/taproot-assets/issues/642) | Public source/query and an existing synthetic ivmlite fixture; no production DB | Existing split filtered join/count slice fits | Package and extend existing demo |
| P1 | [noop #2314](https://github.com/ryanbr/noop/issues/2314) | Public source/query and issue measurements; no production DB or reusable large fixture captured here | Per-device/day `COUNT(*)` fits after storing the day bucket | Implemented adapted slice |
| P1 | [Zcash #2476](https://github.com/zcash/librustzcash/issues/2476) | Public source/query and issue measurements; no production DB or matching large generator | Per-account integer `SUM` fits only after eligibility is denormalized | Implemented adapted slice |
| P2 | [Kener #840](https://github.com/rajnandan1/kener/issues/840) / [PR #842](https://github.com/rajnandan1/kener/pull/842) | Issue-reported production numbers and an upstream rollup implementation; no production DB | Status-dimensional counts and integer latency sum fit; full rollup does not | Implemented high-cardinality slice |
| P3 | [Bifrost #7460](https://github.com/maximhq/bifrost/issues/7460) | Issue-reported production numbers; no public DB or complete generator | Requires aggregates to survive raw deletion, which ivmlite does not support | Keep as a negative demo and sealing acceptance case |
| Defer | [Claude Monitor #305](https://github.com/hoangsonww/Claude-Code-Agent-Monitor/issues/305) | Public proposal and small application fixtures; no row count, DB size, or timing in the issue | Also requires rollup-before-delete/sealing | Revisit after sealing exists |

## P0: FluxFlow first

### What the public evidence establishes

The issue reports a synthetic six-month database with **1.5 million
`flow_events`** and about **700 ms** per `getStats()` call. One open browser tab
could indirectly cause about five full-scan calls every five seconds while
background synchronization was also writing data.

Here, **`6M` means six months, not six million rows**.

The merged PR includes `scripts/bench-api.ts`, a public synthetic generator. Its
reported fixture contains 518,400 blocks and about 1.7 million flows. The PR
reports the following before/after results for the application's hand-written
trigger rollups and response cache:

| Query | Before | Application rollup/cache |
|---|---:|---:|
| Six-month summary | 100 ms | 2.8 ms |
| 30-day summary | 11 ms | 0.8 ms |
| Six-month events, first page | 518 ms | 0.1 ms |

The original 700 ms `getStats()` result and the PR's 100 ms summary result are
different measurements. Do not combine them into one before/after benchmark.
The project solved its immediate problem without ivmlite.

### Executable slice

Create a typed fact table that stores the grouping keys needed by one stable
summary, for example:

```sql
CREATE TABLE flow_facts (
  id INTEGER PRIMARY KEY,
  flow_type TEXT NOT NULL,
  day_bucket INTEGER NOT NULL,
  counterparty_kind TEXT NOT NULL,
  exchange_key TEXT NOT NULL,
  sat INTEGER NOT NULL
) STRICT;

SELECT
  flow_type,
  day_bucket,
  counterparty_kind,
  exchange_key,
  SUM(sat),
  COUNT(*)
FROM flow_facts
GROUP BY flow_type, day_bucket, counterparty_kind, exchange_key;
```

This is an adaptation. It does not reproduce every `getStats()` query, the
application's time-window stitching, leaderboards, HTTP cache, or UI polling.

Run three honest baselines over the same generated changes:

1. indexed SQLite full recomputation;
2. a correct hand-written trigger for the same adapted view, using the
   application's subtract-old/add-new strategy without handicapping it;
3. ivmlite capture plus explicit batch refresh.

Measure bootstrap, base writes, refresh, result reads, total write-plus-refresh,
state size, and correctness. Include small and large bases, multiple batch
sizes, and INSERT, corrected-label UPDATE, reorg DELETE, and reinsert traces.
Compare every checkpoint with SQLite recomputation.

FluxFlow deliberately keeps rollups after retention deletes raw rows. Current
ivmlite retracts contributions when source rows are deleted. Disable retention
in the positive demo and add a separate failing/unsupported retention check.

### What success would mean

The demo can show whether ivmlite is competitive for the compatible grouped
aggregate under FluxFlow-shaped scale and mutations. It cannot show that
ivmlite replaces PR #45, because that PR also changes polling, status snapshots,
ETags, indexes, window composition, and retention behavior.

## P0: Taproot Assets

The issue reports that the event log becomes problematic above roughly ten
million rows and identifies repeated joins and aggregation over a
non-materialized view. The repository contains the pinned upstream migrations
and consumer SQL, but no production database, event distribution, or upstream
timing suitable for replay. See the [source archive and evidence gaps](../cases/taproot-assets/README.md).

An executable synthetic slice already exists. The original query has one join
and two `COUNT(CASE ...)` aggregates; current ivmlite instead maintains two
filtered join/count views (`SYNC` and `NEW_PROOF`) and the caller merges them.
This omits original groups that contain neither event type.

The saved Apple M2 Pro measurements show a useful crossover rather than a
universal win:

| Synthetic workload | ivmlite | full recomputation |
|---|---:|---:|
| 25k base + 1k inserts | 42.806 ms | 9.685 ms |
| 250k base + 100 inserts | 26.155 ms | 121.853 ms |

Extend this demo with on-disk runs, 1M and larger fixtures where practical,
mixed INSERT/UPDATE/DELETE traces, group-cardinality variation, bootstrap time,
state size, and write amplification. Keep the split-view semantic gap explicit.

## P1: noop gravity witness

The issue reports a real-device pass in which 60 per-day witness reads took
2,251 ms, about 97% of the store-probe budget. At a ten-second cadence, sixty
days contain about 518,000 rows per device. The reporter isolates `COUNT(*)` as
the cost: `MAX(ts)` can seek to the end of an index range, while count must walk
the range. A backfill can change count without changing max, so max alone is not
a correct replacement.

Build a synthetic typed table with a stored day bucket and maintain:

```sql
SELECT device_id, day_bucket, COUNT(*)
FROM gravity_witness
GROUP BY device_id, day_bucket;
```

Combine that result with indexed `MAX(ts)` in the demo application. Exercise
steady-state reads, append, historical backfill, deletion, and correction.
The stored day bucket is an adaptation because current ivmlite cannot group by
the original timezone/day expression. The issue's Python timings are
indicative; only the cited store-probe timings came from the device.

The executable [noop demo](noop.md) runs this slice at 518,400 rows and checks
append, historical backfill, delete and correction against SQLite recomputation.

## P1: Zcash transparent balance

The issue reports about **197 ms warm-cache at 500,000 UTXOs** for a
high-frequency wallet-summary path. The original query scans transparent
outputs, joins transactions/addresses/accounts, materializes spentness, and may
run twice when `min_confirmations > 0`.

For a current-version demo, denormalize query eligibility into a fact table and
maintain:

```sql
SELECT account_uuid, SUM(value_zat)
FROM transparent_outputs
WHERE eligible = 1
GROUP BY account_uuid;
```

Model receive as INSERT, spend as `eligible: 1 -> 0`, and rewind as
`eligible: 0 -> 1`. This is valuable because it exercises retractions, not just
insert throughput. It does not reproduce confirmation-height logic, coinbase
maturity, the original multi-table query, or wallet correctness. The fixture
must be described as synthetic and shaped only by public scale/query evidence.

The executable [Zcash demo](zcash.md) runs this adaptation at 500,000 rows with
receive, spend, rewind and correction batches.

## P2: Kener status rollups

The issue reports 215 monitors and 5.5 million SQLite rows. Its largest page has
63 monitors; a 90-day request scans 4.4 million rows, spends 5,631 ms in the
database, and returns 847 KB. The proposed 15-minute rollup is reported as
550,000 buckets and 23 MB for 4.1 million raw rows, versus 577 MB raw, with a
4.4-second backfill and an expected read near 470 ms.

A current ivmlite slice can pre-store `bucket_ts` and integer latency, then use
status as a grouping dimension alongside `SUM(latency_ms)` and `COUNT(*)`.
It cannot reproduce the full query because `MIN`, `MAX`, `AVG`, computed time
buckets, compound predicates, and multiple conditional aggregates are
unsupported.

The executable [Kener demo](kener.md) tests 4.1 million rows, exactly 550,000
initial result groups, historical rewrites and deletes. It also records the
large materialized-read and state-size costs. Do not quote the issue's expected
470 ms as an observed ivmlite result.

## P3: Bifrost sealing acceptance case

The report describes a single-user SQLite deployment with about 2,300 requests
per day: 26,500 rows produced a 17.7 GB database in 17 days because payloads
were large. A seven-day stats query reportedly took 200 seconds before a manual
`ANALYZE`; afterward it ranged from 0.4 to 15 seconds. Raw-log retention then
deletes the historical token/cost totals the user still needs.

The core requirement is **finalize aggregates, verify them, and only then delete
raw rows while keeping the finalized result**. Current ivmlite instead retracts
deleted source contributions. This case must therefore be a negative test or a
future sealing acceptance test, not a positive v0 demo. It must also keep the
query-planner problem separate: `ANALYZE`/`PRAGMA optimize` is a simpler remedy
for the reported bad SQLite plan.

## Deferred: Claude Monitor tiered retention

The proposal asks for hot raw data, warm rows without payloads, and cold daily
rollups that survive raw deletion. It gives a useful target schema and safety
requirements, but no row count, database size, query timing, or large public
fixture. The repository's small development data cannot support a performance
claim.

Revisit this as a second sealing scenario after Bifrost's core semantics are
implemented. Until then it adds no distinct positive capability demo.

## Claims policy

These cases do **not** currently support claims that:

- any project, maintainer, or reporter tested, adopted, requested, or endorsed
  ivmlite;
- ivmlite caused any upstream fix;
- synthetic results reproduce production behavior;
- issue-reported historical timings describe current upstream releases;
- a compatible slice solves the complete application problem;
- ivmlite preserves aggregates after raw-row deletion;
- ivmlite is always faster than recomputation or hand-written triggers.

Every published demo should name the upstream source, evidence level, exact
adaptation, unsupported behavior, data generator, revision, machine, SQLite and
ivmlite versions, complete command, raw output, and correctness oracle.
