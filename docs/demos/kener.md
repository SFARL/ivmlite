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

This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 4.1 million
base rows, a 500-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 10.38 at the start and 2.88 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md).

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.166 (0.163–0.169) ms | 0 ms | 0 ms | 0.166 (0.163–0.169) ms | 0.166 (0.163–0.169) ms | 100,940 KiB |
| unindexed recompute | 0 ms | 1,689 (1,651–1,726) ms | 0.192 (0.190–0.198) ms | 1,669 (1,646–1,674) ms | 0 ms | 1,670 (1,646–1,674) ms | 1,670 (1,646–1,674) ms | 100,940 KiB |
| indexed recompute | 1,637 (1,625–2,134) ms | 347 (345–366) ms | 1.25 (1.21–1.27) ms | 347 (345–349) ms | 0 ms | 348 (346–350) ms | 348 (346–350) ms | 200,596 KiB |
| ivmlite | 8,851 (8,509–9,057) ms | 14.4 (4.78–18.6) ms | 4.24 (4.18–4.36) ms | 2.41 (2.15–2.46) ms | 144 (143–145) ms | 6.65 (6.33–6.82) ms | 150 (150–152) ms | 255,432 KiB |

The refresh itself was about 140 times faster than indexed recomputation, the
headline baseline, and about 690 times faster than unindexed recomputation.
Counting the capture cost inside the writes as well, apply + maintain was about
52 times faster than indexed recomputation. Both figures omit reading the large
materialized result. Including the read, the
adapted ivmlite path was about 2.3 times faster than indexed recomputation and
about 11 times faster than unindexed recomputation, and used about 2.5 times
the base table's SQLite page allocation (the covering index used about 2.0
times). High result cardinality therefore remains a material read and space
cost even when maintenance is cheap.

ivmlite's first refresh after `CREATE` took 14.4 ms, about 6 times the
steady-state refresh of 2.41 ms, with a wide range over the repetitions
(4.78–18.6 ms). Since format 4, bootstrap empties its own stage, so this is not
the leftover stage; its cause has not been measured. In the M1b Phase 5
before/after run, the same build's first refresh was 5.37 ms.

See the [raw results](results/2026-10-10-kener-macos-m2-pro.txt).

## Boundaries

Current ivmlite cannot maintain the source's computed time expression,
conditional aggregates, `AVG`, `MIN` or `MAX`; it also does not push monitor or
time constraints into a virtual-table scan. This benchmark drains all 550,000
groups, unlike the page-specific upstream query. It does not test response
compression, JSON padding, viewer-timezone day composition or concurrent
writers. PR #842 rebuilds affected buckets in the application; that
source-specific baseline is not yet implemented here, so these numbers do not
claim ivmlite is faster than the upstream solution.
