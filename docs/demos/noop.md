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

This run used an Apple M2 Pro, macOS 26.2, Rust 1.95.0, SQLite 3.53.2, an
in-memory database, 518,400 base rows, a 200-operation mixed batch and five
independent repetitions. Each cell is the median, with the min–max over the
repetitions in parentheses. Other builds and tests were running on the machine
at the time, so these timings are noisier than an idle run and only the shape
of each comparison should be read from them; the authoritative before/after
comparison of M1b Phase 5 comes from its own measurement run.

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.209 (0.204–0.273) ms | 0 ms | 0 ms | 0.209 (0.204–0.274) ms | 0.209 (0.204–0.274) ms | 15,012 KiB |
| unindexed recompute | 0 ms | 181 (168–187) ms | 0.261 (0.243–0.279) ms | 181 (180–187) ms | 0 ms | 181 (181–187) ms | 181 (181–187) ms | 15,012 KiB |
| indexed recompute | 157 (155–161) ms | 17.7 (17.4–18.3) ms | 0.458 (0.379–0.461) ms | 17.6 (17.5–18.3) ms | 0 ms | 18.1 (17.8–18.8) ms | 18.1 (17.8–18.8) ms | 20,808 KiB |
| ivmlite | 219 (213–230) ms | 1.15 (1.14–1.23) ms | 2.10 (2.09–2.13) ms | 1.25 (1.22–1.27) ms | 0.021 (0.020–0.021) ms | 3.35 (3.34–3.40) ms | 3.37 (3.37–3.42) ms | 15,120 KiB |

For this adapted grouped-count query, ivmlite's write, refresh and read were
about 5 times faster than indexed recomputation, the headline baseline, and
about 54 times faster than unindexed recomputation. ivmlite's first refresh
after `CREATE`, which also drains the stage the bootstrap left behind, took
1.15 ms: within run-to-run noise of the steady-state refresh of 1.25 ms. This
does not reproduce the issue's 2,251 ms device result: the benchmark does not
run Room, sixty separate range queries, fifteen repeated passes, timezone
conversion or the indexed max lookup.

See the [raw results](results/2026-10-08-noop-macos-m2-pro.txt).

## Boundaries

The stored day must be computed with the application's real timezone and DST
rules. Explicit refresh creates a stale window, and count plus max must be read
from a consistent snapshot. The demo does not address iOS/Android extension
registration or cross-compilation. A production integration would also need to
keep the shadow row and source sample atomic.
