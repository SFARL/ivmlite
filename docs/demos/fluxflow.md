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
three counterparty kinds, integer satoshi amounts and about 180 day buckets at
the reported scale.

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

The mixed batch contains 60% inserts, 20% updates that move a row between
groups and change its value, and 20% deletes representing reorg rollback.
Every materialized result is compared with SQLite recomputing the query.

## Run

Build the release extension, then run a quick probe:

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test \
  --example fluxflow_demo -- 250000 100 5
```

Run at the source-reported scale with fewer repetitions first:

```sh
cargo run --release --locked -p ivmlite-test \
  --example fluxflow_demo -- 1500000 500 3
```

The output is CSV and includes four modes:

- `no_maintenance`: base-write floor;
- `full_recompute`: base writes followed by SQLite draining the complete query;
- `handwritten_trigger`: a correct rollup table maintained inside each base
  write transaction, using FluxFlow's subtract-old/add-new strategy for this
  one adapted view rather than copying both upstream rollup levels;
- `ivmlite`: capture triggers during writes followed by explicit batch refresh.

Seeding and extension loading are outside timed regions. `bootstrap_ms` measures
creating and backfilling the materialized state over an existing base table.
`apply_ms` includes each mode's write-side overhead. For ivmlite,
`maintain_ms` is refresh; for full recomputation, it is the complete aggregate
read; hand-written trigger maintenance is already inside `apply_ms`. `read_ms`
measures draining the small materialized result for the two materialized modes.
`database_kib` is SQLite's total in-memory page allocation after the batch, not
an on-disk file-size claim.

## First source-scale result

The first checked-in run used an Apple M2 Pro, macOS 26.2, Rust 1.95.0, an
in-memory database, 1.5 million base rows, a 500-operation mixed batch and three
independent repetitions. These are development measurements of the adapted
fixture, not FluxFlow production results:

| Mode | Bootstrap | Apply | Maintain/recompute | Materialized read | Apply + maintain |
|---|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0.227 ms | 0 ms | 0 ms | 0.227 ms |
| full recompute | 0 ms | 0.230 ms | 1,289.994 ms | 0 ms | 1,290.224 ms |
| hand-written trigger | 1,326.856 ms | 1.428 ms | 0 ms | 0.524 ms | 1.428 ms |
| ivmlite | 2,884.475 ms | 4.082 ms | 2.676 ms | 2.833 ms | 6.758 ms |

For this run, ivmlite's batch write plus refresh was about 191 times faster
than recomputing the aggregate, but about 4.7 times slower than the specialized
trigger. Its bootstrap was also about 2.2 times slower than the trigger
backfill. That is the intended comparison: ivmlite wins reusable machinery and
explicit batching, while a workload-specific trigger remains the performance
bar.

The [raw command transcript](results/2026-10-08-fluxflow-macos-m2-pro.txt)
contains the environment and CSV output. More machines, on-disk runs, batch
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
