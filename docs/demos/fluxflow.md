# FluxFlow grouped-flow rollup slice

This demo evaluates one stable grouped aggregate shaped by
[FluxFlow issue #3](https://github.com/2ndtlmining/fluxflow/issues/3) and the
hand-written rollups merged in
[PR #45](https://github.com/2ndtlmining/fluxflow/pull/45). It is an adapted
workload, not a reproduction of the complete application and not evidence that
FluxFlow uses or endorses ivmlite.

## Public evidence

Issue #3 reports about 700 ms per `getStats()` call on a synthetic database
with 1.5 million `flow_events`. A browser tab could indirectly trigger about
five full-scan calls every five seconds while synchronization also wrote in
batches.

PR #45 later added a deterministic upstream benchmark generator. Its default
six-month run creates 518,400 blocks and, because of the generator's
Poisson-shaped loop, about 1.7 million flows. In the PR's results, **`6M` means
six months, not six million rows**. The reported six-month summary improved
from 100 ms to 2.8 ms after the application added hand-written hourly/daily
trigger rollups and caching. That result belongs to FluxFlow, not ivmlite.

The pinned upstream sources are:

- [benchmark generator](https://github.com/2ndtlmining/fluxflow/blob/ed26a17f5dbf3fcc0a5e2f1f7b9536877198ff3c/scripts/bench-api.ts)
- [schema and rollup triggers](https://github.com/2ndtlmining/fluxflow/blob/ed26a17f5dbf3fcc0a5e2f1f7b9536877198ff3c/src/lib/server/db/migrations.ts)

No production database is public. This repository therefore creates a
deterministic synthetic fixture with the same important categorical shape:
approximately 45% buying, 44% selling and 11% p2p, seven exchange values,
three counterparty kinds and native REAL amounts in the current adaptation. Each day bucket holds
10,000 flows, so the 1.5-million-row run has 150 day buckets.

## Adaptation

The upstream `flows` table is not `STRICT` and contains a `REAL confidence`
column. M2c accepts this table class and REAL values. The demo still uses a
narrow fact table and precomputes the day and counterparty grouping dimensions:

```sql
CREATE TABLE flow_facts (
  id INTEGER PRIMARY KEY,
  flow_type TEXT NOT NULL,
  day_bucket INTEGER NOT NULL,
  counterparty_kind TEXT NOT NULL,
  exchange_key TEXT NOT NULL,
  amount REAL NOT NULL,
  confidence REAL
);
```

It maintains exactly this query:

```sql
SELECT
  flow_type,
  day_bucket,
  counterparty_kind,
  exchange_key,
  SUM(amount),
  COUNT(*)
FROM flow_facts
GROUP BY flow_type, day_bucket, counterparty_kind, exchange_key;
```

Upstream, a p2p flow has no exchange. Here `exchange_key` is `NOT NULL`, and
the empty string `''` stands in for that NULL on every p2p row, so all p2p
flows of a day and counterparty kind share one group.

The mixed batch contains 60% inserts, 20% updates that move a row between
groups and change its value, and 20% deletes representing reorg rollback. As a
reorg would, the updates and deletes hit the newest base rows. At the
documented batch sizes all of those rows lie in the last day bucket, and the
updates move them into the next day, where the inserts also land. The batch
therefore touches only the groups of two day buckets. That is realistic for a
reorg, and it is favourable to refresh, whose cost grows with the groups a
batch touches; a batch spread across history would touch more. Every
materialized result is compared with SQLite recomputing the query.

## Run

Build the release extension, then run a quick probe:

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test \
  --example fluxflow_demo -- 250000 100 5
```

Run at the source-reported scale:

```sh
cargo run --release --locked -p ivmlite-test \
  --example fluxflow_demo -- 1500000 500 5
```

The output is CSV, preceded by `#` provenance lines, and includes five modes:
`no_maintenance`, `unindexed_recompute`, `indexed_recompute`,
`handwritten_trigger` and `ivmlite`. The protocol, the modes and every column
are described in the [demos README](README.md#benchmark-protocol). Here, the
covering index of `indexed_recompute` is
`flow_facts(flow_type, day_bucket, counterparty_kind, exchange_key, amount)`, and
`handwritten_trigger` is a correct rollup table maintained inside each base
write transaction, using FluxFlow's subtract-old/add-new strategy for this one
adapted view rather than copying both upstream rollup levels.

## Historical alpha.3 source-scale result (2026-10-10)

The following unchanged historical table measures the earlier STRICT integer
`sat` adaptation, not the current REAL query. This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 1.5 million
base rows, a 500-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 5.21 at the start and 4.51 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md). These are
development measurements of the adapted fixture, not FluxFlow production
results:

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.198 (0.192–0.200) ms | 0 ms | 0 ms | 0.198 (0.192–0.200) ms | 0.198 (0.192–0.200) ms | 60,840 KiB |
| unindexed recompute | 0 ms | 1,244 (1,198–2,112) ms | 0.233 (0.226–0.239) ms | 1,229 (1,218–1,264) ms | 0 ms | 1,229 (1,218–1,264) ms | 1,229 (1,218–1,264) ms | 60,840 KiB |
| indexed recompute | 1,312 (1,287–1,347) ms | 135 (132–136) ms | 1.12 (1.06–1.14) ms | 135 (134–135) ms | 0 ms | 136 (135–138) ms | 136 (135–138) ms | 121,668 KiB |
| hand-written trigger | 1,238 (1,190–1,385) ms | 0 ms | 1.33 (1.31–1.37) ms | 0 ms | 0.468 (0.457–0.477) ms | 1.33 (1.31–1.37) ms | 1.80 (1.76–1.84) ms | 61,144 KiB |
| ivmlite | 2,826 (2,673–3,607) ms | 2.68 (2.31–3.65) ms | 4.12 (4.05–4.26) ms | 2.38 (2.21–2.46) ms | 2.61 (2.58–2.68) ms | 6.59 (6.31–6.61) ms | 9.27 (8.86–9.30) ms | 63,268 KiB |

Against the headline baseline, recomputing the aggregate over its covering
index, ivmlite's batch write plus refresh was about 21 times faster than that
recomputation. It was about 190 times faster than the secondary baseline,
unindexed recomputation; the covering index alone made recomputation about 9
times faster than the unindexed scan, at about 2.0 times the page allocation.
ivmlite was about 5 times slower than the specialized trigger, and its
bootstrap took about 2.3 times the trigger backfill's. That is the intended
comparison: ivmlite wins reusable machinery and explicit batching, while a
workload-specific trigger remains the performance bar.

ivmlite's first refresh after `CREATE` took 2.68 ms, against a steady-state
refresh of 2.38 ms; their ranges overlap. Since format 4, bootstrap empties its
own stage, so the first refresh no longer drains it.

The [raw results](results/2026-10-10-fluxflow-macos-m2-pro.txt) contain the
environment, provenance and CSV output. More machines, on-disk runs, batch
sizes and mutation mixes are required before making a release-level performance
claim.

Correctness is also covered by:

```sh
scripts/build-extension.sh
cargo test --locked -p ivmlite-test \
  fluxflow_grouped_rollup_tracks_insert_update_and_reorg_delete
```

## M2c source-scale result (2026-10-20)

The current non-STRICT fixture stores `amount = ((hash >> 24) % 2^32) / 256`
and `confidence = (hash % 1000) / 1000`. Amount corrections add 1.0, and the
covering index includes `amount`. Amounts are exact dyadic fractions (multiples
of 1/256); group sums remain exactly representable at this recorded scale.
The SQLite multiset oracle therefore checks exact results here despite the
more general exact-summation differences documented in the
[alpha.6 numerical notes](../releases/v0.1.0-alpha.6.md#differences-from-sqlite).
Confidence is captured but is not aggregated by this query.

This October 20 run used an Apple M2 Pro (10 cores, 16 GiB), macOS 27.0.1,
Rust 1.95.0, bundled SQLite 3.53.2, 1.5 million base rows and 500-operation
mixed batches. Five independent runs each measured ten steady batches after
warm-up. Values below are medians [minimum, maximum] over runs; combined times
are measured batch sums. The after extension is `84e4c07`, with clean harness
`aba0fd7`; the same-day before capture used `b647bcb` and its original integer
workload. Full source hashes and commands are in the
[build CSV](../bench/m2c-demos-builds.csv) and
[capture provenance](../bench/m2c-capture/README.md).

| Mode | Bootstrap (ms) | First refresh (ms) | Apply (ms) | Maintain (ms) | Read (ms) | Apply + maintain (ms) | End to end (ms) | SQLite pages (KiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no_maintenance | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | 0.197 [0.193, 0.201] | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | 0.198 [0.193, 0.201] | 0.198 [0.193, 0.201] | 77,224 |
| unindexed_recompute | 0.000 [0.000, 0.000] | 1217.760 [1206.021, 1237.943] | 0.222 [0.215, 0.232] | 1210.346 [1201.575, 1233.281] | 0.000 [0.000, 0.000] | 1210.569 [1201.801, 1233.507] | 1210.569 [1201.801, 1233.507] | 77,224 |
| indexed_recompute | 1297.668 [1279.306, 1335.844] | 134.839 [134.469, 140.226] | 1.168 [1.137, 1.254] | 134.743 [134.605, 136.052] | 0.000 [0.000, 0.000] | 135.890 [135.769, 137.335] | 135.890 [135.769, 137.335] | 141,028 |
| handwritten_trigger | 1200.204 [1191.149, 1231.629] | 0.000 [0.000, 0.000] | 1.286 [1.276, 1.299] | 0.000 [0.000, 0.000] | 0.475 [0.474, 0.475] | 1.286 [1.276, 1.299] | 1.760 [1.754, 1.775] | 77,544 |
| ivmlite | 2834.756 [2640.757, 3013.083] | 3.989 [3.720, 6.244] | 3.921 [3.893, 4.082] | 2.128 [2.097, 2.288] | 2.301 [2.295, 2.354] | 6.052 [6.015, 6.368] | 8.347 [8.304, 8.710] | 83,872 |

Current ivmlite write plus refresh is 22.45x faster than indexed recomputation
and 200.03x faster than unindexed recomputation, but 4.71x slower than the
specialized trigger. Including reads, the respective ivmlite/trigger times
are 8.347 and 1.760 ms. Bootstrap costs 2,834.756 ms and total SQLite database
page allocation is 83,872 KiB, versus the covering index's 1,297.668 ms and
141,028 KiB. Page allocation includes the base table and all database
structures; it is neither RSS nor pure ivmlite state.

The October 20 old-shape ivmlite capture had maintain 1.739 [1.720, 1.839] ms
and combined 5.619 [5.584, 5.901] ms, versus the current 2.128 [2.097, 2.288]
and 6.052 [6.015, 6.368] ms. The schema, values and capture payload changed;
these ratios describe the changed adaptation's cost, not an engine speedup
or an integer-workload regression check. The
[raw before/after results](results/2026-10-20-fluxflow-m2c.txt),
[complete comparison](../bench/m2c-report.md) and
[matching integer comparisons](../bench/README.md#m2c-column-types-and-aggregates)
retain those distinctions. The captures ran 09:41–09:46 EDT (13:41–13:46 UTC),
with one-minute load 2.883 → 2.882 for before and 2.882 → 2.250 for after;
the host was not isolated. These are synthetic measurements, not production
or adoption evidence.

## Boundaries

This demo does not reproduce FluxFlow's blocks table, HTTP polling, ETags,
leaderboards, time-window stitching, page query, or response cache. It cannot
reuse the PR's 2.8 ms result as an ivmlite baseline; all comparisons must be
rerun on the same host with this executable.

FluxFlow deliberately keeps historical rollups when retention deletes raw
rows. Current ivmlite instead retracts a deleted row's contribution. Retention
is therefore disabled in this positive demo and remains a separate unsupported
sealing requirement.
