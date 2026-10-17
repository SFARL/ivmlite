# M2a — bounded non-uniform workload validation

Status: proposed implementation contract; no M2a measurements yet.
Amendment 2026-10-17: stage rows after refresh (see §5).
Prerequisite: alpha.5 release preparation and M1b correctness/performance/write-cost
acceptance. M2a validates the current engine; it does not add SQL operators,
change shadow-table format 5, or promise production readiness.

## 1. Question and scope

Does the measured advantage over full recomputation survive non-uniform group
sizes, concentrated updates and increased join fanout? Where does it disappear,
and how much state does each shape retain? A slowdown or high space cost is a
valid result, not a reason to tune or omit the cell before publishing it.

Deliver one versioned portable workload, an exported initial fixture and ordered
mutation trace, a small runner, raw results and a derived report. Use the real
release extension. Exclude Nexmark execution, Turso execution, durability,
concurrency, multi-view scaling and new performance optimizations from M2a.

Existing `ivmlite-workload` has Uniform-only distribution/locality, Insert/Delete
trace operations, and single-table `ViewSpec` queries. Its matrix runner is not
a join or SQL UPDATE runner. Add a separate versioned M2a workload/trace contract,
reusing helpers where suitable; keep existing TOML files, deterministic M1b
traces and legacy CSV readers valid. Do not widen the old matrix implicitly.

## 2. Fixed workload and 32-cell matrix

One view per cell, non-null bounded integers, STRICT tables, stable integer
primary keys, COUNT(*) and SUM(amount). `amount` is in [-100, 100]; zero sums
with positive counts must remain visible. Each engine receives identical rows,
SQL semantics, mutation order, transaction boundaries and refresh checkpoints.
No data generation, sampling, schema setup or trace validation is timed.

| Axis | Values |
|---|---|
| Profiles | uniform data/uniform updates; Zipf data/uniform updates; uniform data/hot updates; Zipf data/hot updates |
| Zipf | exponent 1.2, group rank probability proportional to rank^-1.2 |
| Hot updates | 80% of existing-row target selections from the first 10% of group ranks; remainder from the other 90% |
| Batch size | 10, 1,000 SQL row operations |
| Seeds | 1, 42, 2026; one independent run per seed |
| Timed rounds | one warm-up, five measured batches per engine/run |

**Single table: eight cells.** 100,000 rows, 1,000 group keys; query
`SELECT g, COUNT(*), SUM(amount) FROM events GROUP BY g`. Four profiles × two
batch sizes. Seed one row per group, then sample the remaining rows from the
chosen distribution; shuffle the association between group and row id so PK
locality does not determine group locality. Record the empirical distribution,
including the top 10% group's row share; the seeded coverage means the final
fixture is not an exact independent Zipf sample.

**Join: 24 cells.** Two tables, each with 32,768 rows:

```sql
CREATE TABLE lhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL,
                 g INTEGER NOT NULL) STRICT;
CREATE TABLE rhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL,
                 amount INTEGER NOT NULL) STRICT;
-- Logical view (COUNT/SUM column order is fixed in the workload):
SELECT lhs.g, COUNT(*), SUM(rhs.amount)
FROM lhs JOIN rhs ON lhs.k = rhs.k GROUP BY lhs.g;
```

Use 1,024 output group keys. Fanout f ∈ {1, 8, 64}; each side has
K = 32,768/f join keys and exactly f rows per key. Initial join output is
32,768*f rows (32,768; 262,144; 2,097,152). Both base-table row counts stay
fixed. Zipf controls **lhs.g only**, independently shuffled across join keys;
join-key multiplicity stays balanced, so group skew and fanout are not the same
axis. Four profiles × three fanouts × two batch sizes. This is a controlled
many-to-many sweep, not a claim to model skewed join-key multiplicities.

Include explicit ordinary join-key indexes on both tables for every mode.
Record their DDL and query plans. No baseline may be left doing an avoidable
join-key full scan just because the extension has internal arrangements.

## 3. Trace semantics and validation

An exported trace names schema version, cell, seed, table, SQL operation,
primary key, old/new values, batch boundary and checkpoint. Store a content
hash for the workload, initial data, view SQL and trace. Future systems replay
these artifacts; they must not independently regenerate a similar workload.
Cell derivation belongs to the workload file, not runner constants.

Within every ten operations, use two INSERTs, two DELETEs, three amount UPDATEs
and three group-key UPDATEs. Join cells use two lhs INSERTs, two lhs DELETEs,
three rhs.amount UPDATEs and three lhs.g UPDATEs. Insertions draw groups from
the chosen data distribution; existing-row selection draws a group uniformly
or by the hot policy, then a live eligible row uniformly within that group.
This explicitly measures group locality; uniform group selection is not
uniform row selection on Zipf data. Label it accordingly in every result.
The rhs updates in join cells select an eligible lhs target group, then a
live rhs row reachable by a matching join key; report the realized target
shares separately: lhs target groups, and the hot share of all lhs rows and
distinct output groups actually matched by each rhs UPDATE. The latter can
touch cold groups too; this is anchor-target locality, not a promise that
80% of changed output groups are hot.

Choose operation order and insertion keys so that deleted rows are live and
join multiplicity is restored by the end of each batch. INSERT/DELETE pairs
replace lhs rows at the same join key; lhs.g updates use existing groups.
Amount UPDATEs draw a different value from [-100, 100]; group UPDATEs draw
a different existing key from the initial data distribution. Reject no-op
updates. Profiles describe the initial fixture and the target-selection policy,
not a stationary distribution guarantee: record group histograms and realized
hot shares at every correctness/state checkpoint to expose profile drift.
At checkpoints both tables retain their initial row counts and each join key
retains f rows per side. Mid-batch counts may differ. Hot-group membership is
fixed by rank, but eligibility follows the live data. Do not substitute an
INSERT when a DELETE target is unavailable or silently fall back to a uniform
target. Reject an impossible trace before timing and report the cause.

Report target group counts, distinct PKs touched, repeated-target count,
rows actually changed, and matched partners per operation. Hot updates may
revisit a row across batches; M1b's disjoint-batch rule must not erase the
locality this experiment is intended to measure. PK lookups must stay indexed.

Separate small deterministic correctness fixtures cover join-key updates on
both sides, duplicate join keys, simultaneous left/right deltas (including the
delta×delta term), orphans becoming matched/unmatched, NULL join keys, empty
and vanished groups, positive-count zero sums, rollback and a repeated idle
refresh. These fixtures are not extra performance matrix cells. For each,
assert explicit expected rows as well as the independent SQLite oracle.

## 4. Correctness, timing and baselines

Run a correctness/state replay separately from the performance replay, from
fresh databases using exactly the same exported fixture/trace. Compare every
view as a multiset with SQLite full recomputation after bootstrap and every
batch; inspect state outside timing. Before refresh, assert the old snapshot;
a second refresh without writes must preserve results and retained state.
A mismatch fails the run and saves the cell, seed, prefix and expected/actual
rows. Neither two implementations agreeing nor an aggregate checksum alone
replaces the SQLite oracle.

Performance modes: no maintenance (write-cost lower bound), ordinary SQLite
recomputation, covering-index SQLite recomputation, and ivmlite. Additional
covering indexes exist only in the explicitly labeled covering mode; time
its write cost too. Compare against the faster **complete recompute-mode
total**, never mix one mode's writes with another mode's read time. The
no-maintenance mode has no current result and is excluded from speedups.
Hand-written trigger comparisons remain the M1b evidence; adding a generic
join-trigger compiler is outside M2a, and no new trigger-gap claim is allowed.

Reuse M1b's fresh in-memory connection, complete bootstrap and prepared-statement
protocol (`engine.rs`, `baseline.rs`), plus the demo runner's result-draining
helpers where appropriate. Pin and report page/cache settings, SQLite version,
foreign-key/recursive-trigger/journal/synchronous pragmas, extension SHA-256,
Rust toolchain, commit, OS, hardware, load and exact command. Use one transaction
per batch; apply includes BEGIN/COMMIT, maintenance follows commit, then drain
the same full result. Time apply, maintain and read separately; also record
each batch's own apply+maintain and apply+maintain+read sums. No-maintenance's
maintain/read and full-result total are N/A rather than a fake zero-cost query.

Prepare every timed SQL statement before the first timer and gate RePrepare
at the actual call sites. For recomputation, put full query execution and
materialization in maintain, then drain that materialized result in read;
account for bootstrap/materialization consistently. SQL triggers, if reported
later, would charge their work to apply. Performance replay verifies bootstrap,
warm-up and final results outside timing; per-batch oracle/state scans belong
only to the separate correctness replay so they do not warm subsequent timers.

Rotate mode order deterministically across cells/seeds. Report per-run medians
over the five measured batches, and paired seed-level ratios with min/median/max
across the three seeds. Preserve all raw rounds. Never sum component medians.
If a ratio's range straddles 1 or 2, or a stated regression relies on <10%,
confirm that cell with five additional paired runs at seed 42 on an otherwise
idle host; report both sets separately and their full ranges. Seeds measure
fixture sensitivity; they are not repeated timing trials or confidence bounds.

## 5. State and space measurements

At bootstrap, after writes before refresh, after each refresh, and after the
second idle refresh, record:

- base rows, joined row count and output groups;
- per-operator arrangement rows/weight totals, aggregate-state rows, output
  rows, pending delta rows and stage rows; distinguish physical entries from
  logical multiplicities (counting only final groups misses join state);
- page_size, page_count and freelist_count: allocated bytes = page_size ×
  page_count, live-page proxy = page_size × (page_count - freelist_count);
- same-checkpoint base-only and covering-recompute allocations, and ivmlite /
  base-only allocated-byte ratio. Page allocation is not RSS or peak memory.

One view has consumed every delta after refresh: pending deltas must be empty.
The stage table may hold exactly the last applied batch, marked by one `apply`
sentinel: format 5 empties the stage at the start of the next apply (Phase 5
§4), and M2a does not change the format. Its row count is reported as retained
space beside the other state, never as a pending delta. Verify persisted state
agrees with independently counted live base keys/multiplicities. No repeated
idle refresh may add retained rows.

Run a separate bounded churn probe: uniform and Zipf+hot at join f=64, seed 42,
batch 1,000, 100 balanced batches with fixed live cardinalities. Check the oracle
and capture all logical state counts each round. Derive each arrangement
multiset from the live bases using that operator's actual projection, and
compare decoded entries and weights, not only row counts or weight totals.
Then replay inverse mutations in reverse order to restore the initial bases
exactly; the decoded operator-state multiset must equal bootstrap state
(excluding sequence/progress bookkeeping). Residual entries or missing live
entries fail acceptance. Allocation may keep reusable
freelist pages: report its trajectory without treating every high-water page
as a leak or silently VACUUMing it away. This probes logical retention, not an
unbounded memory-leak proof. Extra process-memory tooling is deferred.

## 6. Acceptance and deliverables

1. All 32 cells × three seeds replay legally and deterministically, with
   correct per-batch and idle-refresh results; all targeted fixtures pass.
2. A versioned workload and exporter round-trip exactly, including UPDATEs,
   both join sides and transaction order. Preflight rejects invalid Zipf/hot
   parameters, duplicate cells, unavailable targets and integer overflow.
   Existing M1b workloads and legacy result readers remain valid.
3. Actual timing-path gates detect statement preparation/recompilation and
   incorrect total aggregation; mutation checks also detect lost locality,
   altered fanout and skipped oracle/state checks. Register each new invariant
   in `docs/mutation-gates.md` with observed red tests and restored green run.
4. Publish raw rounds, profile/target histograms, plans, hashes and state
   checkpoints; derive tables from these files. Report every loss, timeout or
   resource limit by cell, without computing a speedup for incomplete runs.
5. Publish a profile × batch × fanout comparison against the fastest complete
   recomputation mode, plus state growth. State whether >2x survives in any
   tested region, where it fails and how hotspots change it. No speedup is
   required for acceptance: a falsifying result completes validation too.
6. Logical retention and delta/stage cleanup checks pass, including bounded
   churn. Publish bootstrap and space costs beside timing advantages.
7. `scripts/test-all.sh`, both Cargo fmt checks and both all-target Clippy
   checks pass. Execute the full matrix only after small smoke cells pass;
   preserve resumable artifacts if quota/resources interrupt the run.

Implement in this order: portable trace + generator/preflight; targeted
correctness/state replay; timed adapters and metric gates; smoke; full matrix;
confirmation cells; derived report. Do not bundle performance fixes into the
first measurement; a discovered bug receives its own regression and rerun.

## 7. Handoff to M2b: standard queries and Turso

Before standard-load execution, pin an upstream Nexmark revision/query set and
create a per-query compatibility checklist with exact source SQL, required
operators/types, status (unchanged / rewritten / unsupported), named parser or
binder diagnostic, proposed SQL and semantic differences. Execute every
"unchanged" classification; do not infer compatibility from a query's name.
A rewrite that removes event-time windows, watermarks, auctions or ranking must
be labeled an adapted workload, with no canonical Nexmark score. Unsupported
queries do not authorize widening the SQL subset as part of M2a.

Only then pin a Turso revision/build and verify locally: MV feature availability,
SQL/type/NULL/overflow/multiset semantics, refresh visibility and transaction
boundaries, indexes/query plans, initial-state completeness, storage/durability
settings and identical exported trace ingestion. Use SQLite recomputation as
the independent oracle for both. Report within-engine naive/incremental ratios;
raw latency across different database hosts includes host/storage differences.
No claim of present Turso compatibility is made before that probe. Published
third-party timings cannot populate a same-host comparison table.
