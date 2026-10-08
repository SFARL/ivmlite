# Kener quarter-hour rollup slice

This demo evaluates the high-cardinality rollup proposed in
[Kener issue #840](https://github.com/rajnandan1/kener/issues/840) and implemented
upstream in
[PR #842](https://github.com/rajnandan1/kener/pull/842), still open at the
2026-10-08 snapshot, whose head was then
[`891f07d`](https://github.com/rajnandan1/kener/pull/842/commits/891f07d28d95b5df76df815115ec453a4c682179)
(last updated 2026-09-06). It is a synthetic,
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

## Run and result

The source-scale run uses substantial memory and takes a few minutes on the
recorded machine:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- kener_quarter_hour_rollup 4100000 500 5
```

The covering index of `indexed_recompute` is `monitoring_facts(monitor_id,
bucket_ts, status, latency_ms)`. The protocol and columns are in the [demos
README](README.md#benchmark-protocol).

This run used an Apple M2 Pro, macOS 26.2, Rust 1.95.0, SQLite 3.53.2, an
in-memory database, 4.1 million base rows, a 500-operation mixed batch and five
independent repetitions. Each cell is the median, with the min–max over the
repetitions in parentheses. Other builds and tests were running on the machine
at the time, so these timings are noisier than an idle run and only the shape
of each comparison should be read from them; the authoritative before/after
comparison of M1b Phase 5 comes from its own measurement run.

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.158 (0.155–0.163) ms | 0 ms | 0 ms | 0.158 (0.155–0.163) ms | 0.158 (0.155–0.163) ms | 100,904 KiB |
| unindexed recompute | 0 ms | 1,683 (1,586–1,760) ms | 0.182 (0.181–0.199) ms | 1,654 (1,636–1,707) ms | 0 ms | 1,654 (1,637–1,708) ms | 1,654 (1,637–1,708) ms | 100,904 KiB |
| indexed recompute | 1,663 (1,590–1,766) ms | 353 (339–356) ms | 1.19 (1.18–1.27) ms | 352 (343–354) ms | 0 ms | 354 (344–356) ms | 354 (344–356) ms | 199,960 KiB |
| ivmlite | 9,836 (9,534–10,084) ms | 8.82 (8.50–9.03) ms | 4.31 (4.15–4.47) ms | 2.86 (2.76–2.98) ms | 147 (142–149) ms | 7.23 (6.91–7.36) ms | 154 (149–156) ms | 256,048 KiB |

The refresh itself was about 120 times faster than indexed recomputation, the
headline baseline, and about 580 times faster than unindexed recomputation.
Counting the capture cost inside the writes as well, apply + maintain was about
49 times faster than indexed recomputation. Both figures omit reading the large
materialized result. Including the read, the
adapted ivmlite path was about 2.3 times faster than indexed recomputation and
about 11 times faster than unindexed recomputation, and used about 2.5 times
the base table's SQLite page allocation (the covering index used about 2.0
times). High result cardinality therefore remains a material read and space
cost even when maintenance is cheap.

ivmlite's first refresh after `CREATE`, which also drains the stage the
bootstrap left behind, took 8.82 ms: about 3 times the steady-state refresh of
2.86 ms.

See the [raw results](results/2026-10-08-kener-macos-m2-pro.txt).

## Boundaries

Current ivmlite cannot maintain the source's computed time expression,
conditional aggregates, `AVG`, `MIN` or `MAX`; it also does not push monitor or
time constraints into a virtual-table scan. This benchmark drains all 550,000
groups, unlike the page-specific upstream query. It does not test response
compression, JSON padding, viewer-timezone day composition or concurrent
writers. PR #842 rebuilds affected buckets in the application; that
source-specific baseline is not yet implemented here, so these numbers do not
claim ivmlite is faster than the upstream solution.
