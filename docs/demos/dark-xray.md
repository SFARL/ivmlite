# dark-xray traffic ledger totals

This demo adapts the open [dark-xray #159](https://github.com/darktunnelmika/dark-xray/issues/159)
to two queries within ivmlite's current SQL subset. It is an isolated,
synthetic comparison of maintained totals, covering-index recomputation,
and a transactional hand-written rollup. It is not a deployment proposal for
the live billing database.

## Public workload

The issue reports 9,524,296 immutable traffic events in a roughly 1.7 GiB
SQLite database. `owner_stats` independently executes
`SUM(up_bytes+down_bytes)` for one owner and for one owner plus period. An
upstream seven-owner, read-only local test observed 0.38–2.5+ seconds per
query; heavy queries were interrupted at 2.5 seconds. Those figures describe
the upstream database and must not be compared as a speedup against this
demo. [PR #160](https://github.com/darktunnelmika/dark-xray/pull/160)
improved dashboard summary behavior, but the exact owner reads remain in
the [upstream source at `ed66ded`](https://github.com/darktunnelmika/dark-xray/blob/ed66ded4ec09c9d023635f64d9b05eb592ce06a1/backend/dark_policy.py).

## Adaptation and oracle

The source table is not `STRICT`, and ivmlite cannot maintain
`SUM(up_bytes+down_bytes)` directly. The demo uses a narrow `STRICT`
`traffic_ledger` projection with an integer `total_bytes` stored alongside
`up_bytes` and `down_bytes`. A `CHECK` constraint enforces
`total_bytes = up_bytes + down_bytes` on every row. A test additionally
compares both grouped queries and source-shaped point reads against SQLite's
original expression after every append batch. This projection would require
an atomic migration/write-path change in the application.

The two maintained relations are:

```sql
SELECT owner, period, SUM(total_bytes)
FROM traffic_ledger GROUP BY owner, period;

SELECT owner, SUM(total_bytes)
FROM traffic_ledger GROUP BY owner;
```

The fixture uses the issue's event count by default, with synthetic owner
skew, twelve periods, and small integer byte counts. It keeps a source-shaped
noncovering `(owner, period)` index; the extra covering `(owner, period,
total_bytes)` index is built only in the indexed baseline. All measured
batch operations append new events. The hand-written summary uses an `AFTER
INSERT` trigger in the same transaction. This is faithful to the reported
immutable ledger, but does not test reconciliation, deletion, updates, event
id conflicts, or multiple live writers. The synthetic `event_id` is an
integer surrogate instead of the source's text key.

The shared runner checks the materialized result against a full SQLite
recomputation after bootstrap, warm-up, and final batch. It times a complete
grouped scan in each recomputation mode, whereas the application's read is
one owner at a time. Consequently its read and end-to-end columns are useful
for this grouped-query shape, **not a prediction of the application's
`WHERE owner=?` latency**. In particular, the `(owner, period)` index may
make an individual owner lookup much cheaper than the full grouped scan.

## Run

From the repository root, build the release extension and run each case:

```sh
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  dark_xray_period_usage 9524296 200 3
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  dark_xray_lifetime_usage 9524296 200 3
```

Use smaller row counts for a fast correctness/shape check, for example
`10000 40 1`. Every mode receives the same deterministic fixture and eleven
append batches per repeat, with an untimed warm-up and ten measured batches.
The [shared protocol](README.md#benchmark-protocol) records bootstrap, write,
maintenance, read, end-to-end, and SQLite page allocation separately.

## Local synthetic measurement

The raw [period](results/2026-10-18-dark-xray-period-100k-m2pro.txt) and
[lifetime](results/2026-10-18-dark-xray-lifetime-100k-m2pro.txt) runs used an
Apple M2 Pro with 16 GiB memory, bundled SQLite 3.53.2 and ivmlite
`0.1.0-alpha.5` on 2026-10-18. Each mode had 100,000 synthetic base rows,
200 append operations per batch, one warm-up, ten measured batches and three
fresh-database repeats. The one-minute machine load was about 3.70 at the
end. These development runs used uncommitted demo code based on `66f38ef`;
the raw files preserve that provenance. The 100,000-row fixture is only about
1% of the reported production event count. The displayed totals are medians of each
batch's write + maintenance + read time, then medians across repeats.

| Query | No maintenance | Full recompute | Covering-index recompute | Hand trigger | ivmlite |
|---|---:|---:|---:|---:|---:|
| Owner + period | 0.236 ms | 21.700 ms | 5.206 ms | **0.525 ms** | 2.769 ms |
| Owner lifetime | 0.230 ms | 20.618 ms | 4.332 ms | **0.510 ms** | 2.403 ms |

The hand-written append-only trigger is the fastest maintained option here.
For both queries ivmlite beats *full grouped* recomputation, including the
covering-index variant, but is several times slower than the dedicated
trigger. That result favors the simpler trigger approach for this narrow,
immutable rollup if the application's acceptance tests permit it. The
measurements do not establish a win over source-shaped indexed point reads,
and they do not measure app lock contention.

A separate [9,524,296-row period run](results/2026-10-18-dark-xray-period-9m-m2pro.txt)
used **one repeat** on the same machine and build. Its ten measured batches
gave 4,285.311 ms for full grouped recomputation, 459.149 ms with the
covering index, 0.591 ms with the hand trigger, and 2.879 ms with ivmlite
(median end-to-end per 200-event batch). The ivmlite bootstrap took 9.274 s,
versus 4.303 s for the hand summary and 5.727 s to build the covering index.
The base projection occupied about 498 MiB of SQLite pages; ivmlite added
about 140 KiB and the covering index about 229 MiB. This single-repeat
measurement has no between-repeat uncertainty estimate. Its synthetic rows,
narrow schema, in-memory database, single connection and grouped result read
are materially different from the upstream 1.7 GiB file and serialized
owner-specific queries. The upstream 0.38–2.5+ second observations are **not**
a baseline for calculating an ivmlite speedup.

## Deployment boundary

The issue requires equality on a consistent read-only snapshot, bounded lock
hold time, cold/warm route latency and concurrent five-node lease renewals
under a 10M-event fixture. This demo does not test any of those application
conditions. A production path must account for both
[`dark_policy.py`](https://github.com/darktunnelmika/dark-xray/blob/ed66ded4ec09c9d023635f64d9b05eb592ce06a1/backend/dark_policy.py)
and the separate
[`manager.py` ledger writer](https://github.com/darktunnelmika/dark-xray/blob/ed66ded4ec09c9d023635f64d9b05eb592ce06a1/backend/manager.py#L714-L723),
idempotent event IDs, crash/restart recovery, and exact billing invariants.
The issue explicitly forbids installing triggers or heavyweight indexes in
the live database before snapshot acceptance. No maintainer has expressed
intent to adopt ivmlite.
