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
three counterparty kinds and integer satoshi amounts. Each day bucket holds
10,000 flows, so the 1.5-million-row run has 150 day buckets.

## Adaptation

The upstream `flows` table is not `STRICT` and contains a `REAL confidence`
column. Current ivmlite accepts only `STRICT` base tables whose columns are
`INTEGER`, `TEXT` or nullable variants. The demo uses a narrow fact table and
precomputes the day and counterparty grouping dimensions:

```sql
CREATE TABLE flow_facts (
  id INTEGER PRIMARY KEY,
  flow_type TEXT NOT NULL,
  day_bucket INTEGER NOT NULL,
  counterparty_kind TEXT NOT NULL,
  exchange_key TEXT NOT NULL,
  sat INTEGER NOT NULL
) STRICT;
```

It maintains exactly this query:

```sql
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
`flow_facts(flow_type, day_bucket, counterparty_kind, exchange_key, sat)`, and
`handwritten_trigger` is a correct rollup table maintained inside each base
write transaction, using FluxFlow's subtract-old/add-new strategy for this one
adapted view rather than copying both upstream rollup levels.

## Source-scale result

This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 1.5 million
base rows, a 500-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 2.71 at the start and 16.06 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md). These are
development measurements of the adapted fixture, not FluxFlow production
results:

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.205 (0.196–0.208) ms | 0 ms | 0 ms | 0.205 (0.196–0.208) ms | 0.205 (0.196–0.208) ms | 60,840 KiB |
| unindexed recompute | 0 ms | 1,282 (1,203–1,404) ms | 0.237 (0.231–0.253) ms | 1,281 (1,239–1,335) ms | 0 ms | 1,281 (1,239–1,335) ms | 1,281 (1,239–1,335) ms | 60,840 KiB |
| indexed recompute | 1,490 (1,306–1,979) ms | 139 (131–217) ms | 1.17 (1.13–1.45) ms | 136 (132–155) ms | 0 ms | 137 (133–156) ms | 137 (133–156) ms | 121,668 KiB |
| hand-written trigger | 1,318 (1,191–1,456) ms | 0 ms | 1.36 (1.31–1.37) ms | 0 ms | 0.479 (0.460–0.495) ms | 1.36 (1.31–1.37) ms | 1.84 (1.77–1.87) ms | 61,144 KiB |
| ivmlite | 2,942 (2,922–3,489) ms | 2.81 (2.56–3.13) ms | 4.19 (4.05–4.50) ms | 2.33 (2.23–2.79) ms | 2.64 (2.55–3.00) ms | 6.53 (6.29–7.28) ms | 9.17 (8.84–10.3) ms | 63,268 KiB |

Against the headline baseline, recomputing the aggregate over its covering
index, ivmlite's batch write plus refresh was about 21 times faster than that
recomputation. It was about 200 times faster than the secondary baseline,
unindexed recomputation; the covering index alone made recomputation about 9
times faster than the unindexed scan, at about 2.0 times the page allocation.
ivmlite was about 5 times slower than the specialized trigger, and its
bootstrap took about 2.2 times the trigger backfill's. That is the intended
comparison: ivmlite wins reusable machinery and explicit batching, while a
workload-specific trigger remains the performance bar.

ivmlite's first refresh after `CREATE` took 2.81 ms, against a steady-state
refresh of 2.33 ms; their ranges overlap. Since format 4, bootstrap empties its
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

## Boundaries

This demo does not reproduce FluxFlow's blocks table, HTTP polling, ETags,
leaderboards, time-window stitching, page query, or response cache. It cannot
reuse the PR's 2.8 ms result as an ivmlite baseline; all comparisons must be
rerun on the same host with this executable.

FluxFlow deliberately keeps historical rollups when retention deletes raw
rows. Current ivmlite instead retracts a deleted row's contribution. Retention
is therefore disabled in this positive demo and remains a separate unsupported
sealing requirement.
