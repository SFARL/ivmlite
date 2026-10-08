# Kener quarter-hour rollup slice

This demo evaluates the high-cardinality rollup proposed in
[Kener issue #840](https://github.com/rajnandan1/kener/issues/840) and implemented
upstream in the still-open
[PR #842](https://github.com/rajnandan1/kener/pull/842). It is a synthetic,
adapted workload and does not imply Kener adoption or endorsement.

## Public evidence

The issue reports 215 monitors and 5.5 million SQLite rows. A 90-day request for
the largest page scanned 4.4 million rows and spent 5,631 ms in the database.
The proposed 15-minute rollup measured about 550,000 buckets and 23 MB for 4.1
million raw rows, versus 577 MB raw, with a roughly 4.4-second backfill. The
reported expected 470 ms read belongs to Kener's implementation and is not an
ivmlite result.

## Adapted query

The demo precomputes the quarter-hour bucket, stores integer latency and turns
conditional status counts into a grouping dimension:

```sql
SELECT monitor_id, bucket_ts, status, SUM(latency_ms), COUNT(*)
FROM monitoring_facts
GROUP BY monitor_id, bucket_ts, status;
```

At the source-scale setting, the deterministic fixture produces 4.1 million
rows and exactly 550,000 initial result groups across 215 monitors. The mixed
batch covers append, historical status/monitor rewrite, latency correction and
delete. The application could pivot the status dimension and derive an average
from sum and count.

## Run and first result

The source-scale run uses substantial memory and takes about a minute on the
recorded machine:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- kener_quarter_hour_rollup 4100000 500 3
```

Three-run medians on an Apple M2 Pro were:

| Mode | Bootstrap | Apply | Maintain/recompute | Read | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0.240 ms | 0 ms | 0 ms | 0.240 ms | 100,900 KiB |
| full adapted recompute | 0 ms | 0.211 ms | 1,683.307 ms | 0 ms | 1,683.518 ms | 100,900 KiB |
| ivmlite | 10,230.130 ms | 3.914 ms | 9.567 ms | 155.736 ms | 169.217 ms | 256,048 KiB |

The refresh itself is about 176 times faster than full recomputation, but that
number omits reading the large materialized result. Including the read, the
adapted ivmlite path was about 10 times faster and used roughly 2.5 times the
SQLite page allocation. High result cardinality therefore remains a material
read and space cost even when maintenance is cheap.

See the [raw transcript](results/2026-10-08-kener-macos-m2-pro.txt).

## Boundaries

Current ivmlite cannot maintain the source's computed time expression,
conditional aggregates, `AVG`, `MIN` or `MAX`; it also does not push monitor or
time constraints into a virtual-table scan. This benchmark drains all 550,000
groups, unlike the page-specific upstream query. It does not test response
compression, JSON padding, viewer-timezone day composition or concurrent
writers. PR #842 rebuilds affected buckets in the application; that
source-specific baseline is not yet implemented here, so these numbers do not
claim ivmlite is faster than the upstream solution.
