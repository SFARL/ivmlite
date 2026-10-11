# Current ivmlite evaluation of the collected cases

Evaluated on 2026-09-28 against the current SQLite extension; the performance
probe below was re-run on 2026-10-10 with the `0.1.0-alpha.3` extension. This is a
capability and crossover probe, not a claim that the original applications
have been reproduced. The original source corpus remains unchanged.

## What runs today

| Source case | Original query | Executable slice through ivmlite | Current result |
|---|---|---|---|
| Datasette Kentucky facets | Rows, total count and three facets combined with CTEs, subqueries, ordering, limits and `UNION ALL` | Exact county `GROUP BY`/`COUNT(*)` under the fixed `state = 'Kentucky'` filter | Maintained correctly after insert, update and delete |
| Org-roam complete node records | Four-plus tables, outer joins and `group_concat` into a denormalized record | Two-table inner join counting tags per node | Maintained correctly after changes to both tables |
| Taproot Assets universe stats | One join with two `COUNT(CASE ...)` aggregates | Two filtered join/count views, one for `SYNC` and one for `NEW_PROOF` | Both maintained correctly after event and root changes |
| Zero concurrent maintenance | Transaction/concurrency requirement, no public SQL workload | None | Not executable from the collected evidence |
| Generic SQLite IVM request | Capability request, no schema/query/data | None | Not executable from the collected evidence |

The exact Datasette combined query is exercised as a rejection test and stops
at `WITH`. The exact Taproot view is exercised as a rejection test and stops at
`COUNT(CASE ...)`. Org-roam's stored source is an Emacs Lisp SQL string and a
trigger installation script rather than a single view SELECT; a source-shaped
query verifies the first relevant boundary (outer/multi-table joins).

The executable slices are not presented as the full cases:

- Datasette's slice makes `county` non-NULL, so `county IS NOT NULL` becomes
  redundant. It omits rows, global count, state/FIPS facets, sort and limits.
- Org-roam's slice counts tags. It does not produce aliases, refs, links,
  citations or concatenated note records.
- Taproot's two rows of results must be merged by their three keys by a caller.
  This split has the same two positive counts when all relevant roots have at
  least one `SYNC` or `NEW_PROOF` event. A root with only some other event type
  appears as two zeros in the original query but is absent from both slices.
- Every slice uses a deliberately typed `STRICT` schema because that is part
  of the current extension contract. It does not assert that an application's
  historical database can be used without migration.

The executable correctness tests are in
[`real_world_cases.rs`](../../../crates/ivmlite-test/tests/real_world_cases.rs).
They create and load the real extension, create virtual ivmlite views, mutate
the base tables, refresh, and compare every view row with SQLite evaluating the
same slice from scratch.

Run them with:

```sh
scripts/build-extension.sh
cargo test --locked -p ivmlite-test --test real_world_cases
```

## Performance probe

The runner is
[`real_case_bench.rs`](../../../crates/ivmlite-test/examples/real_case_bench.rs).
For every slice it compares three fresh in-memory databases:

- `no_maintenance`: base writes only;
- `ivmlite`: base writes plus capture triggers, followed by explicit refresh;
- `full_recompute`: the same base writes, followed by draining every source
  SELECT directly from SQLite.

Seeding, extension loading, view creation/bootstrap, statement preparation and
correctness checks are outside the timed region. Every statement a timer runs
is prepared before it: the inserts, the transaction's `BEGIN IMMEDIATE` and
`COMMIT`, each view's refresh and each recompute query. Until 2026-10-10 this
was not true: each refresh statement was compiled inside the maintenance timer,
and `BEGIN IMMEDIATE` and `COMMIT` inside the apply timer, so the 2026-09-28
snapshot included that compilation. Each run applies one batch to a fresh
database, so the timed `ivmlite` maintenance is the first refresh after
`CREATE`. Since shadow-table format 4, bootstrap empties its own stage, so that
refresh no longer drains a leftover stage, but no warm-up refresh precedes it.
Full recomputation is not written to a table, which deliberately favours that
baseline. `apply_ms` and `maintain_ms` are each the median of independent
runs, and `total_ms` is the median of each run's own `apply + maintain`. Until
2026-10-10 it was the sum of the two medians, which may come from different
runs. The runner refuses to use the debug extension.

Build and run the release binaries with:

```sh
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example real_case_bench -- \
  25000 1000 5
cargo run --release --locked -p ivmlite-test --example real_case_bench -- \
  250000 100 3
```

Arguments are initial rows, inserted rows per batch and repetitions. The raw
measurements are in [2026-10-10-apple-m2-pro.csv](2026-10-10-apple-m2-pro.csv).

### Observed crossover

| Slice | 25k base + 1k delta | 250k base + 100 delta |
|---|---:|---:|
| Datasette county facet | ivmlite 9.043 ms; recompute 1.684 ms (**5.37× slower**) | ivmlite 1.320 ms; recompute 11.675 ms (**8.84× faster**) |
| Org-roam tag counts | ivmlite 21.274 ms; recompute 7.208 ms (**2.95× slower**) | ivmlite 2.804 ms; recompute 89.713 ms (**31.99× faster**) |
| Taproot event counts | ivmlite 26.502 ms; recompute 9.586 ms (**2.76× slower**) | ivmlite 4.161 ms; recompute 119.405 ms (**28.70× faster**) |

These measurements show a real crossover for all three slices: small deltas
over a larger base table amortize capture and refresh costs, and a large delta
over a small table still loses to recomputation. **This changed since the
2026-09-28 snapshot**, which found the Org-roam slice slower at both points
(24.97× and 2.03×) and called it evidence of a performance problem in the join
path. Its refresh fell from 167.872 to 13.442 ms at 25k + 1k and from 192.585
to 1.987 ms at 250k + 100. That change comes from the extension, not from
moving statement preparation out of the timers: the harness before that fix
(commit `e1346ce`), re-run against the same `0.1.0-alpha.3` extension on
2026-10-10, gave `ivmlite` totals within run-to-run noise of this harness's
(Org-roam 21.255 and 2.799 ms). The 25k + 1k point still loses for every
slice, including both join slices, so a large delta over a small table
remains a region where recomputation is the better choice.

The two configurations vary both base size and delta size; they locate useful
and non-useful regions but do not isolate a formal scaling curve. The seed data
is synthetic because the original datasets were unavailable, and the update
trace is insert-only. Correctness tests cover insert, update and delete, but
their performance still needs separate measurement. Before using any number
in public project claims, run a matrix with more sizes, mixed update traces,
on-disk databases and the recovered application data where available.

Machine for this snapshot: Apple M2 Pro, macOS 26.2, arm64; Rust 1.95.0;
SQLite 3.53.2. The 2026-10-10 runs took 2.65 s and 12.09 s of wall time, with a
one-minute load average of 18.75 to 21.06 while desktop applications and
system services stayed busy, so the machine was far from idle.
