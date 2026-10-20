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

The demo uses a non-STRICT table with `latency_ms REAL NOT NULL`, precomputes
the quarter-hour bucket, and turns conditional status counts into a grouping
dimension:

```sql
SELECT monitor_id, bucket_ts, status,
       AVG(latency_ms), MIN(latency_ms), MAX(latency_ms), COUNT(*)
FROM monitoring_facts
GROUP BY monitor_id, bucket_ts, status;
```

At the source-scale setting, the deterministic fixture produces 4.1 million
rows and exactly 550,000 initial result groups across 215 monitors. The mixed
batch covers append, historical status/monitor rewrite, latency correction and
delete. The application can still pivot the status dimension; AVG/MIN/MAX are now
maintained directly. Latencies are seeded as
`20 + id % 980 + 0.25 * (id % 4)`, and corrections add 25.0. These dyadic
values have exactly representable sums at the recorded scale, so the SQLite
multiset oracle checks the current aggregates exactly. General REAL inputs
retain the [documented numerical differences](../releases/v0.1.0-alpha.6.md#differences-from-sqlite).

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

### Historical alpha.3 result (2026-10-10)

The following unchanged historical table measures the earlier STRICT integer
SUM/COUNT adaptation, not the current REAL AVG/MIN/MAX query. This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 4.1 million
base rows, a 500-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 6.99 at the start and 5.57 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md).

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.167 (0.164–0.184) ms | 0 ms | 0 ms | 0.168 (0.164–0.184) ms | 0.168 (0.164–0.184) ms | 100,940 KiB |
| unindexed recompute | 0 ms | 1,738 (1,609–1,744) ms | 0.205 (0.192–0.207) ms | 1,717 (1,602–1,770) ms | 0 ms | 1,718 (1,602–1,770) ms | 1,718 (1,602–1,770) ms | 100,940 KiB |
| indexed recompute | 1,703 (1,588–1,848) ms | 345 (338–350) ms | 1.21 (1.17–1.28) ms | 345 (341–353) ms | 0 ms | 346 (342–354) ms | 346 (342–354) ms | 200,596 KiB |
| ivmlite | 8,662 (8,207–9,645) ms | 18.5 (4.98–35.2) ms | 4.20 (4.10–4.44) ms | 2.38 (2.33–2.81) ms | 143 (140–166) ms | 6.90 (6.81–7.64) ms | 150 (148–177) ms | 255,432 KiB |

The refresh itself was about 145 times faster than indexed recomputation, the
headline baseline, and about 720 times faster than unindexed recomputation.
Counting the capture cost inside the writes as well, apply + maintain was about
50 times faster than indexed recomputation. Both figures omit reading the large
materialized result. Including the read, the
adapted ivmlite path was about 2.3 times faster than indexed recomputation and
about 11 times faster than unindexed recomputation, and used about 2.5 times
the base table's SQLite page allocation (the covering index used about 2.0
times). High result cardinality therefore remains a material read and space
cost even when maintenance is cheap.

ivmlite's first refresh after `CREATE` took 18.5 ms, about 8 times the
steady-state refresh of 2.38 ms, with a wide range over the repetitions
(4.98–35.2 ms). Since format 4, bootstrap empties its own stage, so this is not
the leftover stage; its cause has not been measured. In the M1b Phase 5
before/after run, the same build's first refresh was 4.92 ms.

See the [raw results](results/2026-10-10-kener-macos-m2-pro.txt).

## M2c source-scale result (2026-10-20)

This October 20 run used a 10-core Apple M2 Pro with 16 GiB, macOS 27.0.1,
Rust 1.95.0, bundled SQLite 3.53.2, an in-memory database, 4.1 million base rows,
550,000 initial result groups and 500-operation mixed batches. Five independent
runs each measured ten steady batches after warm-up. Values are medians
[minimum, maximum] over runs; combined times are measured batch sums. The
after extension is `84e4c07`, with clean harness `aba0fd7`; the same-day before
capture used `b647bcb` and its original integer SUM/COUNT query. Exact hashes
and commands are in the [build CSV](../bench/m2c-demos-builds.csv) and
[capture provenance](../bench/m2c-capture/README.md).

| Mode | Bootstrap (ms) | First refresh (ms) | Apply (ms) | Maintain (ms) | Read (ms) | Apply + maintain (ms) | End to end (ms) | SQLite pages (KiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no_maintenance | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | 0.167 [0.162, 0.170] | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | 0.167 [0.162, 0.170] | 0.167 [0.162, 0.170] | 119,500 |
| unindexed_recompute | 0.000 [0.000, 0.000] | 1912.466 [1895.986, 1922.866] | 0.192 [0.189, 0.209] | 1908.265 [1901.148, 1914.990] | 0.000 [0.000, 0.000] | 1908.596 [1901.472, 1915.313] | 1908.596 [1901.472, 1915.313] | 119,500 |
| indexed_recompute | 1637.267 [1628.107, 1700.497] | 572.818 [570.149, 576.866] | 1.207 [1.197, 1.229] | 571.278 [570.310, 574.859] | 0.000 [0.000, 0.000] | 572.612 [571.511, 576.074] | 572.612 [571.511, 576.074] | 237,728 |
| ivmlite | 30638.833 [29687.721, 31928.525] | 17.006 [16.310, 20.899] | 4.057 [4.042, 4.072] | 7.010 [6.988, 8.574] | 155.472 [155.102, 159.344] | 11.072 [11.033, 12.648] | 166.546 [166.160, 171.359] | 1,760,784 |

Write plus refresh is 51.72x faster than indexed recomputation and 172.38x
faster than unindexed recomputation. Reading all groups costs 155.472 ms;
including that read, the comparisons narrow to 3.44x and 11.46x. There is no
specialized affected-bucket application baseline here.

These benefits have substantial bootstrap and space costs. Bootstrap is
30,638.833 [29,687.721, 31,928.525] ms, versus current indexed recomputation's
1,637.267 [1,628.107, 1,700.497] ms. Total database page allocation is
**1,760,784 KiB**, versus 237,728 KiB for indexed recomputation and 119,500 KiB
for the base-only database. Retractable MIN/MAX retain live value
multiplicities. These page counts cover base tables, indexes, captured deltas
and shadow state; they measure neither process RSS nor pure operator-state
size.

In the same-day old-shape capture, ivmlite bootstrap was 6,016.495
[5,900.298, 6,221.211] ms and page allocation was 255,436 KiB: the current
figures are 5.092x and **6.893x** larger. Maintenance changed from 1.651
[1.643, 1.663] to 7.010 [6.988, 8.574] ms; combined cost from 6.158
[6.103, 6.191] to 11.072 [11.033, 12.648] ms. The query and value types changed,
so these are adaptation costs, not an engine speedup or integer regression
gate. The [complete report](../bench/m2c-report.md) retains every before/after
mode; [matching integer comparisons](../bench/README.md#m2c-column-types-and-aggregates)
use unchanged shapes.

The [raw results](results/2026-10-20-kener-m2c.txt) ran 09:46–09:56 EDT
(13:46–13:56 UTC); one-minute load was 2.250 → 1.896 for before and
1.896 → 2.150 for after. The host was not isolated. These measurements remain
synthetic and do not establish production performance or adoption.

## Boundaries

Current ivmlite maintains AVG/MIN/MAX but still cannot maintain the source's
computed time expression or conditional aggregates; stored time buckets and
status grouping remain adaptations. It also does not push monitor or time
constraints into a virtual-table scan. This benchmark drains all 550,000
groups, unlike the page-specific upstream query. It does not test response
compression, JSON padding, viewer-timezone day composition or concurrent
writers. PR #842 rebuilds affected buckets in the application; that
source-specific baseline is not yet implemented here, so these numbers do not
claim ivmlite is faster than the upstream solution.
