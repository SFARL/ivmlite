# Current ivmlite evaluation of the collected cases

Evaluated on 2026-09-28 against the current SQLite extension. This is a
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
correctness checks are outside the timed region. Full recomputation is not
written to a table, which deliberately favours that baseline. Results are the
median of independent runs. The runner refuses to use the debug extension.

Build and run the release binaries with:

```sh
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example real_case_bench -- \
  25000 1000 5
```

Arguments are initial rows, inserted rows per batch and repetitions. The raw
measurements are in [2026-09-28-apple-m2-pro.csv](2026-09-28-apple-m2-pro.csv).

### Observed crossover

| Slice | 25k base + 1k delta | 250k base + 100 delta |
|---|---:|---:|
| Datasette county facet | ivmlite 10.144 ms; recompute 1.578 ms (**6.43× slower**) | ivmlite 1.562 ms; recompute 12.110 ms (**7.75× faster**) |
| Org-roam tag counts | ivmlite 177.638 ms; recompute 7.115 ms (**24.97× slower**) | ivmlite 193.594 ms; recompute 95.347 ms (**2.03× slower**) |
| Taproot event counts | ivmlite 42.806 ms; recompute 9.685 ms (**4.42× slower**) | ivmlite 26.155 ms; recompute 121.853 ms (**4.66× faster**) |

These measurements show a real crossover for the Datasette and Taproot slices:
small deltas over a larger base table amortize capture and refresh costs. They
also show that the current join path is not uniformly good. The Org-roam slice
remains slower at both measured points, so it is currently evidence of a
performance problem, not a product win.

The two configurations vary both base size and delta size; they locate useful
and non-useful regions but do not isolate a formal scaling curve. The seed data
is synthetic because the original datasets were unavailable, and the update
trace is insert-only. Correctness tests cover insert, update and delete, but
their performance still needs separate measurement. Before using any number
in public project claims, run a matrix with more sizes, mixed update traces,
on-disk databases and the recovered application data where available.

Machine for this snapshot: Apple M2 Pro, macOS 26.2, arm64; Rust 1.95.0.
