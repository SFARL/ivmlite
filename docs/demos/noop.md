# noop gravity-witness count slice

This demo evaluates the expensive count half of the gravity witness described
in [noop issue #2314](https://github.com/ryanbr/noop/issues/2314). It is an
adapted synthetic workload, not noop production data and not evidence that noop
uses or endorses ivmlite.

## Public evidence

The issue reports a real-device pass where 60 daily `(COUNT(*), MAX(ts))`
reads took 2,251 ms, about 97% of the store-probe budget. At a ten-second
cadence, sixty days contain about 518,000 samples per device. `MAX(ts)` can use
an index seek, while `COUNT(*)` walks every row. A historical backfill changes
the count without changing the maximum timestamp, so max alone is insufficient.

The issue also reports Python in-memory comparisons. Those timings are useful
for query-shape analysis but are not device measurements or ivmlite baselines.
No production database is public.

## Adapted query

The demo stores the resolved local-day bucket as a bare integer and maintains:

```sql
SELECT device_id, day_bucket, COUNT(*)
FROM gravity_witness
GROUP BY device_id, day_bucket;
```

The deterministic fixture contains 518,400 rows across sixty days. Its mixed
batch includes current-day append, historical backfill, deletion and a
correction that moves a row to another bucket. Every ivmlite result is compared
with SQLite recomputing the same adapted SQL.

Current ivmlite cannot maintain the original timezone expression or `MAX`.
An application would combine the maintained count with a separate indexed
`MAX(ts)` lookup. The demo's integer `device_id` and narrow shadow table also
omit the source table's sensor payload.

## Run and result

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- noop_gravity_witness 518400 200 5
```

The fixture holds at most 518,400 rows, one device's sixty days of ten-second
samples; the runner refuses more, which would repeat a `(device_id, ts)` pair.
The covering index of `indexed_recompute` is `gravity_witness(device_id,
day_bucket)`. The protocol and columns are in the [demos
README](README.md#benchmark-protocol).

This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 518,400
base rows, a 200-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 4.51 at the start and 7.34 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md).

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.195 (0.185–0.210) ms | 0 ms | 0 ms | 0.195 (0.185–0.210) ms | 0.195 (0.185–0.210) ms | 15,048 KiB |
| unindexed recompute | 0 ms | 196 (178–270) ms | 0.299 (0.257–0.328) ms | 188 (178–199) ms | 0 ms | 188 (178–200) ms | 188 (178–200) ms | 15,048 KiB |
| indexed recompute | 159 (150–181) ms | 17.6 (17.2–18.5) ms | 0.439 (0.324–0.578) ms | 18.0 (17.1–23.6) ms | 0 ms | 18.4 (17.4–24.5) ms | 18.4 (17.4–24.5) ms | 20,852 KiB |
| ivmlite | 233 (211–278) ms | 0.809 (0.766–0.964) ms | 1.98 (1.87–2.03) ms | 0.710 (0.647–0.760) ms | 0.017 (0.017–0.019) ms | 2.70 (2.51–2.79) ms | 2.72 (2.52–2.81) ms | 15,160 KiB |

For this adapted grouped-count query, ivmlite's write, refresh and read were
about 7 times faster than indexed recomputation, the headline baseline, and
about 69 times faster than unindexed recomputation. ivmlite's first refresh
after `CREATE` took 0.809 ms, about 1.1 times the steady-state refresh of
0.710 ms, with their ranges just apart (0.766–0.964 against 0.647–0.760 ms;
the earlier run's ranges overlapped). Since format 4, bootstrap empties its own stage, so the
first refresh no longer drains it. This does not reproduce the issue's
2,251 ms device result: the benchmark does not run Room, sixty separate range
queries, fifteen repeated passes, timezone conversion or the indexed max
lookup.

See the [raw results](results/2026-10-10-noop-macos-m2-pro.txt).

## Boundaries

The stored day must be computed with the application's real timezone and DST
rules. Explicit refresh creates a stale window, and count plus max must be read
from a consistent snapshot. The demo does not address iOS/Android extension
registration or cross-compilation. A production integration would also need to
keep the shadow row and source sample atomic.
