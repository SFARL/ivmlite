# StoneHub/jot: session counts, bounds and a red complete-session gate

[StoneHub/jot #209](https://github.com/StoneHub/jot/issues/209) reports that
each committed recognition block rereads the active session summary. The
source query computes `MIN(started_at)`, `MAX(started_at + end_seconds)` and
`COUNT(*)`, joins a separate title table, then orders and limits the session
list. See the pinned
[`sessionSelect` source](https://github.com/StoneHub/jot/blob/b16ee67d7bb37e9432d2a29e28a3500fb4c301db/Sources/JotCore/TranscriptStore.swift#L267-L299).
The issue's 100,000-row ~10 ms result was a warm, in-memory synthetic SQLite
measurement, not observed app latency.

## Green slices

`jot_session_counts` maintains `COUNT(*)` of ambient transcript blocks by
session. The typed `STRICT` projection stores integer start/end times and a
separate title table but the green view deliberately reads neither time nor
title. Each batch combines arrival, deletion, session reassignment and mode
change. An extension test compares the refreshed count to SQLite after every
batch; another checks that duplicate IDs fail and rolled-back writes leave
the result unchanged. The fixture uses synthetic integer IDs rather than the
source's text IDs. This is **not** a full session summary.

M2c also supports `MIN(started_ms)`, `MAX(ended_ms)` and `COUNT(*)`
over that same ambient-session projection. The ordinary, non-ignored
`jot_min_max_count_matches_sqlite_after_mixed_changes` acceptance test in
[`capability_red.rs`](../../crates/ivmlite-test/tests/capability_red.rs)
compares the initial view and each of three mixed batches with SQLite,
including deletion and session reassignment. This proves the stored integer
bounds projection; the benchmark below still measures counts only.

```sh
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  jot_session_counts 100000 200 3
```

The shared benchmark compares grouped recomputation, covering-index
recomputation and ivmlite on an in-memory synthetic database. It does not
measure the native app, async snapshots or UI responsiveness.

On this M2 Pro, the recorded [100k-row run](results/2026-10-18-jot-session-counts-100k-m2pro.txt)
used three repeats, one warm-up batch and ten measured 200-operation mixed
batches per repeat. The median per-batch **end-to-end** time (writes +
maintenance + result read) was 17.48 ms for unindexed recomputation, 3.79 ms
for covering-index recomputation and 2.18 ms for ivmlite. Initial ivmlite
bootstrap took 69.41 ms versus 45.95 ms to build the covering index. These
numbers concern session counts only; they cannot be compared directly with
the issue's ~10 ms full-query SQLite measurement.

## Red complete-session gate

The remaining Jot opt-in test in
[`capability_red.rs`](../../crates/ivmlite-test/tests/capability_red.rs)
currently fails at view creation. Once that works it also requires exact
SQLite parity after a title change:

```sh
cargo test --locked -p ivmlite-test --test capability_red \
  jot_full_session_list_matches_sqlite_after_title_change -- --ignored
```

The query adds the title `LEFT JOIN`, an ungrouped title column, ordering
and `LIMIT`. The actual source also stores `REAL` timestamps and computes
the end expression. M2c supports REAL storage and MIN/MAX, but this test
still uses stored integer bounds; the computed source end and complete
session query remain unvalidated. Preserving
`addCommitted`'s asynchronous/idempotent behavior, split/regroup, native
recovery and concurrency are later
[roadmap](real-world-demo-roadmap.md#october-18-staged-full-statistics-gates)
gates. The ignored red test keeps default CI green without disguising the missing
capabilities.
