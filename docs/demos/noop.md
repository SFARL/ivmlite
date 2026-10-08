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

## Run and first result

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- noop_gravity_witness 518400 200 3
```

On an Apple M2 Pro with the in-memory database, three-run medians were:

| Mode | Bootstrap | Apply | Maintain/recompute | Read | End to end |
|---|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0.254 ms | 0 ms | 0 ms | 0.254 ms |
| full adapted recompute | 0 ms | 0.293 ms | 185.489 ms | 0 ms | 185.781 ms |
| ivmlite | 224.505 ms | 2.125 ms | 1.026 ms | 0.024 ms | 3.175 ms |

For this adapted grouped-count query, ivmlite was about 59 times faster end to
end after the batch. This does not reproduce the issue's 2,251 ms device result:
the benchmark does not run Room, sixty separate range queries, fifteen repeated
passes, timezone conversion or the indexed max lookup.

See the [raw transcript](results/2026-10-08-noop-macos-m2-pro.txt).

## Boundaries

The stored day must be computed with the application's real timezone and DST
rules. Explicit refresh creates a stale window, and count plus max must be read
from a consistent snapshot. The demo does not address iOS/Android extension
registration or cross-compilation. A production integration would also need to
keep the shadow row and source sample atomic.
