# CPA Manager Plus: hourly core and red full-summary gates

[CPA Manager Plus #647](https://github.com/seakee/CPA-Manager-Plus/issues/647)
reports about 6.1 million `usage_events` in a roughly 94 GiB SQLite file.
An unfiltered 24-hour full `include.summary` request took 13–32 seconds in
reported production runs, sometimes reaching the panel's 30-second timeout;
the compact profile took about 0.5 seconds on the same host. These are
**upstream application measurements**, not ivmlite results. The issue remains
open without a maintainer decision on whether the full profile is supported
at this scale.

The [pinned service code](https://github.com/seakee/CPA-Manager-Plus/blob/64ce94e6aa1af691555429a3211e67c59c0480ad/apps/manager-server/internal/service/monitoring/service.go#L1246-L1304)
already uses an hourly snapshot where eligible for core totals. Full mode
also reads exact P95 latency/TTFT, task buckets, rolling 30-minute totals,
active days and zero-token models. Improving this demo's hourly core alone
would not solve the reported full-profile timeout.

## Green slice

`cpa_hourly_core` seeds synthetic events and maintains a fixed-hour/model
relation with `COUNT(*)` and integer sums of success, failure, input,
output and total tokens. The `STRICT` fixture stores `hour_bucket`,
`success_count` and `failure_count` at write time; constraints keep the two
status projections equal to the source's `SUM(CASE WHEN failed=... THEN 1...)`
semantics. The extension test checks every refreshed result against the
original conditional expression after each append batch. This is a **partial
core-total slice**, not the full JSON response or the source's schema.

Run the shared benchmark with a short fixture or the issue's event count:

```sh
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  cpa_hourly_core 100000 200 3
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  cpa_hourly_core 6100000 200 1
```

The benchmark compares no maintenance, ordinary grouped recomputation,
covering-index grouped recomputation and ivmlite on fresh in-memory databases.
Its result read drains all hour/model groups. It does not time the manager
API, its pre-existing hourly path, a disk-backed 94 GiB file, or an arbitrary
`[from,to)` request.

On this M2 Pro, the recorded [100k-row run](results/2026-10-18-cpa-hourly-core-100k-m2pro.txt)
used three repeats, one warm-up batch and ten measured 200-insert batches per
repeat. The median per-batch **end-to-end** time (write + maintenance + result
read) was 42.02 ms for unindexed recomputation, 10.07 ms for covering-index
recomputation and 3.70 ms for ivmlite. Initial ivmlite bootstrap took
103.09 ms versus 57.06 ms to build the covering index. These are local
synthetic results for the partial core query; the 6.1M-row command above is
provided for reproduction but was not run for this record.

## Red full-summary gates

The opt-in tests in
[`capability_red.rs`](../../crates/ivmlite-test/tests/capability_red.rs)
use SQLite itself as the oracle. Each currently fails when creating the
ivmlite view; after a future implementation accepts the query, it must also
survive the test's mutation and refresh comparison.

```sh
cargo test --locked -p ivmlite-test --test capability_red \
  cpa_exact_p95_matches_ranked_sqlite_after_append -- --ignored
cargo test --locked -p ivmlite-test --test capability_red \
  cpa_task_buckets_match_sqlite_after_new_task -- --ignored
```

The first follows the source's nearest-rank
[P95 window query](https://github.com/seakee/CPA-Manager-Plus/blob/64ce94e6aa1af691555429a3211e67c59c0480ad/apps/manager-server/internal/repository/usageevent/analytics.go#L946-L989).
The second preserves the expensive shape of its
[task-bucket query](https://github.com/seakee/CPA-Manager-Plus/blob/64ce94e6aa1af691555429a3211e67c59c0480ad/apps/manager-server/internal/repository/usageevent/analytics.go#L2457-L2518):
`MIN/MAX`, `AVG`, `GROUP_CONCAT(DISTINCT)`, ordering and top 500. The fixture
uses fewer columns and fixed boundaries but retains those operators. The
[roadmap](real-world-demo-roadmap.md#october-18-staged-full-statistics-gates)
tracks arbitrary window edges, requested timezones, retention and API parity
as separate later work. These red gates are ignored by default so ordinary
CI remains green; their failure must not be described as a passing demo.
