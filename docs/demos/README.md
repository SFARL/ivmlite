# Demand-backed demos

Research snapshot: 2026-10-08. These are **documented problem patterns**, not
customer interviews, endorsements, or evidence that their authors want ivmlite.
Historical performance reports are not benchmarks of current software.

The [real-world demo roadmap](real-world-demo-roadmap.md) records the next
source-backed candidates, their evidence levels, current ivmlite boundaries,
and the order in which they were implemented.

Four executable workloads from that roadmap now use the real extension,
deterministic large fixtures, mixed mutations and SQLite recomputation oracles:

- [FluxFlow grouped-flow rollup](fluxflow.md), including a hand-written trigger
  baseline;
- [noop gravity-witness count](noop.md);
- [Zcash transparent balance](zcash.md);
- [Kener quarter-hour status rollup](kener.md).

The latter three share one selectable benchmark executable:

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  noop_gravity_witness
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  zcash_transparent_balance
cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
  kener_quarter_hour_rollup
```

Each case records its public evidence, exact adaptation, unsupported behavior,
source-scale command and raw output. These are synthetic development
benchmarks rather than reproductions of production deployments.

### Benchmark protocol

Both executables run the same protocol, implemented once in
[`demo_bench.rs`](../../crates/ivmlite-test/src/demo_bench.rs). It follows the
Phase 4 benchmark protocol
([spec §3](../superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md)):

- **Extension.** The release library under
  `crates/ivmlite-sqlite/target/release/` is refused when it is missing or
  older than any of its sources. `--extension <path>` loads another build and
  skips that check, so one binary can time two builds.
- **One run.** Each mode of each repeat gets a fresh in-memory database:
  1. seed the base table (untimed);
  2. bootstrap (timed as `bootstrap_ms`);
  3. verify the materialized state against SQLite (untimed);
  4. prepare every statement a timer covers, including `BEGIN`, `COMMIT`,
     refresh, recomputation and the result read;
  5. apply an untimed warm-up batch, then time its maintenance as
     `first_refresh_ms`, read the result and verify again;
  6. apply ten measured batches, one after another. For each, time its
     writes, its maintenance and its result read separately. The run's
     `apply_ms`, `maintain_ms` and `read_ms` are each that step's median
     over the ten batches, taken step by step;
  7. verify the final state, and record `database_kib`.

  Every batch has the same mix, and no two batches touch the same rows, so
  one run applies eleven. The state is verified after the warm-up and after
  the last measured batch, outside the timers, but not between measured
  batches. Verification compares multisets of rows: duplicates count, and the
  integer 5 differs from the real 5.0. A mismatch aborts the run.

  **Why ten batches.** A single refresh is too noisy to resolve a change of
  about 10%. In the first published M1b Phase 5 run, one build's five single
  Kener refreshes ranged from 2.743 to 8.828 ms. Until 2026-10-09 each run timed one batch, so a median
  over five repeats rested on five single refreshes. That is how the first
  M1b Phase 5 publication came to report a Kener refresh regression that a
  longer measurement did not confirm (see the "M1b Phase 5" section of
  [`docs/bench/README.md`](../bench/README.md)).
- **Order and statistics.** The mode order rotates left by one each repeat.
  Every column is a median over the repeats, followed by its minimum and
  maximum. `apply_plus_maintain_ms` and `end_to_end_ms` are summed per run,
  from that run's per-step medians, before taking the median over the
  repeats. An even repeat count averages the two middle values.
- **Provenance.** The output starts with `#` lines that give the command,
  commit and dirty flag, the bundled SQLite version, the extension's path
  and version, and the number of measured batches per run.

The modes are:

- `no_maintenance`: base writes only, the floor;
- `unindexed_recompute`: base writes, then SQLite draining the view's query
  over the table with only the indexes its schema declares;
- `indexed_recompute`: the same, over a covering index on the grouping and
  aggregated columns. The index is built at bootstrap (its `bootstrap_ms`);
  its maintenance is inside `apply_ms`; the runner checks with
  `EXPLAIN QUERY PLAN` that the query reads it. This is the headline
  baseline;
- `handwritten_trigger` (FluxFlow only): a rollup table kept inside each write
  by hand-written triggers; reading it is `read_ms`;
- `ivmlite`: capture triggers inside the writes, then explicit refresh
  (`maintain_ms`) and reading the view (`read_ms`).

`first_refresh_ms` is the maintenance after the warm-up batch. For `ivmlite`
it is the first refresh after `CREATE`. With a build before format 4, that
refresh also drains the stage the bootstrap left behind; since M1b Phase 5
(format 4), bootstrap empties its own stage. Either way, the first refresh is
reported there, and is not part of the steady-state `maintain_ms`. For the
recomputation modes it is simply their first recomputation. Recomputation
drains its result without storing it, which favours those baselines. The
recomputation modes are not compared with the oracle after the last batch:
they run the oracle's own query, so the check would be tautological.
`database_kib` is SQLite's in-memory page allocation after the last batch,
not an on-disk size.

The checked-in results in [`results/`](results/) were taken on 2026-10-10 with
the `0.1.0-alpha.3` extension and the ten-batch protocol, one demo after
another, while desktop applications stayed open; each file records its wall
time and its load averages, and the one-minute load average was between 2.71
and 16.06. They show the shape of each comparison. The before/after comparison
of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md), with its data in
`docs/bench/m1b-phase5-demos.csv`.

## Run the first two demos

From the repository root, with the project's Rust toolchain installed:

```sh
cargo run --locked -p ivmlite-test --example demand_demos -- all
cargo run --locked -p ivmlite-test --example demand_demos -- members
cargo run --locked -p ivmlite-test --example demand_demos -- facets
```

[Implementation](../../crates/ivmlite-test/examples/demand_demos.rs).
No new dependencies. All records and change sequences are synthetic fixtures.
Each demo prints its schema, equivalent SQL, and results after each batch.
It runs the actual `IncrementalEngine`, verifies each checkpoint against SQLite
full recomputation, checks that changes wait for explicit refresh, checks a
second refresh is a no-op, and checks the final result against a hand-written
expectation. Any mismatch exits unsuccessfully.

**These are core behavior demos.** They use `ViewQuery` and manually supplied
deltas. They do not use the SQL front end or load an extension into SQLite.
SQLite is used as the independent oracle, rebuilding a small database at each
checkpoint. That rebuild is suitable for correctness checking, **not timing**.
There is no persistence, transaction, installation, or performance claim here.

## Evidence and selection

| Candidate | Evidence | What is established | Decision |
|---|---|---|---|
| Landing-page member counts | Developer's account of a shipped website, S1 | Repeated conditional counts became a concern; the author implemented a trigger-maintained stats table | Demo 1; strongest match |
| Multiple sidebar facet counts | Datasette author's query report, S2; official feature docs, S3 | Multiple group-by counts are a real product feature with a query budget | Demo 2; fixed-query adaptation |
| Orders per customer over a time interval | SQLite user's schema/query and follow-up, S4 | A slow query became much faster after `ANALYZE` | Keep as a counterexample; no demo yet |
| Arbitrary facets on any SQL result | S2/S3 | Dynamic filtering is part of the larger product requirement | Outside these demos; do not promise a drop-in Datasette accelerator |

### S1 — Visible members and non-members

[Samuel Plumppu: Using SQLite Triggers to Boost the Performance of SELECT COUNT(*)](https://samuelplumppu.se/blog/using-sqlite-triggers-to-boost-performance-of-select-count)
(published 2025-08-27; page states updated 2026-01-22).

The developer reports a homepage showing two counts from `users`, restricted to
people who chose public visibility. Repeated reads became slower as usage grew.
The published solution stores the counters in a separate table. Its triggers
**recompute the counts** after writes; do not describe it as a hand-written
incremental implementation.

Our adaptation groups visible users by `is_member`:

```sql
SELECT is_member, COUNT(*)
FROM users
WHERE is_visible = 1
GROUP BY is_member;
```

This replaces the two scalar subqueries and avoids requiring `AND` or a global
aggregate in the maintained view. The application maps the two groups back to
cards and supplies zero for absent groups. The schema is made STRICT, includes
an ID, and uses integer boolean fixtures. Those are demo choices, not properties
claimed for the author's database.

The trace covers signup, visibility withdrawal, membership change, and deletion.
The last non-member disappears, so the final SQL result has only the member
bucket; the homepage's non-member card must still show zero.

**Unanswered:** Would this user accept extension installation and explicit
refresh when their current solution already works? How many writes and reads
occur between refreshes? The demo establishes semantic fit, not adoption.

### S2/S3 — Sidebar facets over a changing catalog

[Simon Willison: Surprising (to me) query performance](https://sqlite.org/forum/forumpost/c0e0fcbe36?hist=&t=c)
(2021-11-17) describes Datasette executing several filtered group-by counts per
page and investigating whether sharing work through a CTE would help.

[Datasette's faceting documentation](https://docs.datasette.io/en/stable/facets.html)
describes value/count sidebars and recommends indexes. Its 50 ms limit applies
to **facet suggestion queries**, not a universal budget for the whole page.

Our synthetic catalog replaces the original dataset. Three predeclared views
count categories, regions, and categories within a fixed EU filter:

```sql
SELECT category, COUNT(*) FROM catalog GROUP BY category;
SELECT region, COUNT(*) FROM catalog GROUP BY region;
SELECT category, COUNT(*) FROM catalog
WHERE region = 'EU' GROUP BY category;
```

The trace adds an item, changes its category, moves another item out of the EU
filter, and deletes the last item in a category. All affected counts update at
explicit refresh. Each view has its own engine; this does not demonstrate shared
operator state between views.

The demo assumes repeated fixed queries against mutable data. The sources do
not establish this exact update pattern. No arbitrary filter combinations,
JSON/date facets, nullable facets, search results, ranking, or pagination are
implemented. Sorting a small materialized result can later happen in the
consumer; it is not maintained `ORDER BY` support.

**Unanswered:** Do likely users repeatedly read a small set of fixed filters,
or generate mostly unique filters? Is their database mutable? Static databases
may only need indexes or one-time precomputation.

### S4 — A useful reason to reject a demo

[SQLite Forum: An index is not used to optimize GROUP BY query](https://sqlite.org/forum/info/0f41f6119002e0e9a5c3a679e952d8a8e91cb375b9d984660331487de92f20a9)
(2022-01-09).

The user describes millions of orders and a time-filtered per-client count.
They subsequently report a large improvement after running `ANALYZE`.
This is evidence of a real pain point **and an existing simpler remedy**.
Do not count it as an unmet IVM request. A fixed time predicate could fit v0;
sliding windows that change merely because time passes need additional work.

## Turn a behavior demo into a release demo

1. Run the same scenario through the real loadable extension once available:
   ordinary SQL writes, persisted state, explicit refresh, and close/reopen.
2. Build a small visible application: two homepage cards for Demo 1; a catalog
   with count sidebars for Demo 2. Show pending changes and refreshed results.
3. Measure on the same machine and workload: indexed SQLite recomputation,
   a correct hand-written incremental trigger implementation, and ivmlite.
   For Demo 1, optionally also reproduce the source's recomputing triggers,
   clearly labeled as a separate baseline. Do not handicap the trigger baseline.
4. Include bootstrap, write plus refresh plus result-read latency, space/state
   growth, and repeated samples. Vary base size, batch size, read frequency,
   and number of views. Include a small-data or infrequent-read case where the
   installation and maintenance overhead may not pay off.
5. Ask prospective users to substitute their own schema/query and report the
   first blocker. Expand SQL support only when the missing behavior recurs.

## Next demand-collection pass

For each public report, record the original SQL/schema, read and write cadence
if stated, deployment environment, attempted fixes, report date, current issue
status, and exact v0 incompatibilities. Leave missing fields unknown. Prefer
first-person reports over vendor use-case articles.

Questions for consenting trial users: What is the query and its measured cost?
How often does the data change and the result get read? How stale may the result
be? Can the deployed SQLite host load an extension and use the supported schema?

Prioritize one successful external replay over adding a third polished demo.
No outreach has been sent as part of this research.
