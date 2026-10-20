# ivmlite documentation

## Contents

- **`superpowers/specs/`** — design documents (specs). The authority implementation works from.
  - [`2026-09-18-ivmlite-design.md`](superpowers/specs/2026-09-18-ivmlite-design.md) — the overall design
  - [`2026-09-26-m1b-phase3a-sqlite-extension-design.md`](superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md) — M1b Phase 3a: the SQLite extension, one view end to end
  - [`2026-09-27-m1b-phase3b-shared-capture-design.md`](superpowers/specs/2026-09-27-m1b-phase3b-shared-capture-design.md) — M1b Phase 3b: shared capture, delta GC, REPLACE capture, rename refusal
  - [`2026-09-28-m1b-phase4-benchmark-design.md`](superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md) — M1b Phase 4: benchmark protocol, hot-spot fixes, and write amplification
  - [`2026-10-08-m1b-phase5-refresh-performance-design.md`](superpowers/specs/2026-10-08-m1b-phase5-refresh-performance-design.md) — M1b Phase 5: refresh performance (schema-version-gated checks, set-based apply, format 4, an empty stage after bootstrap, multi-row staging)
  - [`2026-10-11-m1b-phase6-fixed-cost-design.md`](superpowers/specs/2026-10-11-m1b-phase6-fixed-cost-design.md) — M1b Phase 6: the fixed cost of a refresh (a per-view statement cache, a shared catalog snapshot, the progress index and format 5, a steady-state benchmark)
  - [`2026-10-12-m1b-phase7-refresh-all-design.md`](superpowers/specs/2026-10-12-m1b-phase7-refresh-all-design.md) — M1b Phase 7: `ivm_refresh_all()`, atomic or continuing, and an early return from a refresh with nothing new
  - [`2026-10-15-m2a-nonuniform-validation-design.md`](superpowers/specs/2026-10-15-m2a-nonuniform-validation-design.md) — M2a: the 32-cell Zipf, hot-update and join-fanout validation, with correctness, timing and retained-state acceptance (amended 2026-10-17: retained stage rows)
  - [`2026-10-17-m2c-types-and-aggregates-design.md`](superpowers/specs/2026-10-17-m2c-types-and-aggregates-design.md) — M2c: non-STRICT tables, five storage classes and exact/retractable aggregates
- **`superpowers/plans/`** — implementation plans.
  - [`2026-09-18-m0-test-and-bench-harness.md`](superpowers/plans/2026-09-18-m0-test-and-bench-harness.md) — M0: the test and benchmark harness
  - [`2026-09-20-m1a-phase1-multi-table-harness.md`](superpowers/plans/2026-09-20-m1a-phase1-multi-table-harness.md) — M1a Phase 1: the multi-table harness refactor
  - [`2026-09-21-m1a-phase2-engine.md`](superpowers/plans/2026-09-21-m1a-phase2-engine.md) — M1a Phase 2: the single-table incremental engine
  - [`2026-09-23-m1a-phase3-join.md`](superpowers/plans/2026-09-23-m1a-phase3-join.md) — M1a Phase 3: the two-table equi-join
  - [`2026-09-24-m1b-phase1-state-ownership.md`](superpowers/plans/2026-09-24-m1b-phase1-state-ownership.md) — M1b Phase 1: operator state behind `Arrangement`
  - [`2026-09-24-m1b-phase2a-predicate-whitelist.md`](superpowers/plans/2026-09-24-m1b-phase2a-predicate-whitelist.md) — M1b Phase 2a: the full predicate whitelist
  - [`2026-09-24-m1b-phase2b-sql-front-end.md`](superpowers/plans/2026-09-24-m1b-phase2b-sql-front-end.md) — M1b Phase 2b: the SQL front end
  - [`2026-09-26-m1b-phase3a-sqlite-extension.md`](superpowers/plans/2026-09-26-m1b-phase3a-sqlite-extension.md) — M1b Phase 3a: the SQLite extension
  - [`2026-09-27-m1b-phase3b-shared-capture.md`](superpowers/plans/2026-09-27-m1b-phase3b-shared-capture.md) — M1b Phase 3b: shared capture, delta GC, REPLACE capture, rename refusal
  - [`2026-09-28-m1b-phase4-benchmark.md`](superpowers/plans/2026-09-28-m1b-phase4-benchmark.md) — M1b Phase 4: run the extension against the M0 baselines and publish the result
  - [`2026-10-08-m1b-phase5-refresh-performance.md`](superpowers/plans/2026-10-08-m1b-phase5-refresh-performance.md) — M1b Phase 5: make refresh cheaper, then measure it with Phase 4's protocol
  - [`2026-10-11-m1b-phase6-fixed-cost.md`](superpowers/plans/2026-10-11-m1b-phase6-fixed-cost.md) — M1b Phase 6: remove a refresh's fixed costs, make the benchmark measure the steady state, then measure the phase
  - [`2026-10-12-m1b-phase7-refresh-all.md`](superpowers/plans/2026-10-12-m1b-phase7-refresh-all.md) — M1b Phase 7: the early return, `ivm_refresh_all()`, then measure its call shapes
  - [`2026-10-17-m2a-completion.md`](superpowers/plans/2026-10-17-m2a-completion.md) — M2a: finish the validation runner from the work in progress, then run the matrix and publish it
  - [`2026-10-17-m2c-types-and-aggregates.md`](superpowers/plans/2026-10-17-m2c-types-and-aggregates.md) — M2c implementation, validation and alpha.6 preparation
- **`releases/`** — release notes and the [alpha.5 preparation evidence](releases/v0.1.0-alpha.5-validation.md), including local artifact validation and the remaining tag-time checks; [alpha.6 release preparation](releases/v0.1.0-alpha.6.md) documents format 6 and numerical differences.
- [`mutation-gates.md`](mutation-gates.md) — **the mutation-gate table**: spec requirement → mutation → the test that goes red. Every invariant M1 adds must be registered here.
- **`bench/`** — benchmark results, charts and conclusions ([README](bench/README.md)), including the Phase 4 matrix, confirmation, ablation, write-amplification data, six charts, and [`phase4_tables.py`](bench/phase4_tables.py), which recomputes every Phase 4 table from the CSVs.
  Phase 5 adds its own matrix, confirmation, five-build ablation, demo before/after and six charts, and [`phase5_tables.py`](bench/phase5_tables.py), which recomputes every Phase 5 table from the CSVs.
  Phase 6 adds a steady-state matrix, confirmation, four-build ablation, demo before/after and six charts, and [`phase6_tables.py`](bench/phase6_tables.py), which recomputes every Phase 6 table from the CSVs.
  Phase 7 adds the call-shape measurement of `ivm_refresh_all()` against per-view refresh loops, with its own driver (below), and [`phase7_tables.py`](bench/phase7_tables.py), which recomputes every Phase 7 table from the CSVs.
  M2a adds the non-uniform validation ([`m2a.md`](bench/m2a.md)): its derived tables, every full-size case's raw rounds and a SHA-256 list of the full raw set, all generated by the `m2a` runner (`crates/ivmlite-bench/src/bin/m2a.rs`).
  M2c adds [column types and aggregates results](bench/README.md#m2c-column-types-and-aggregates),
  the [complete generated report](bench/m2c-report.md), and
  [`m2c_tables.py`](bench/m2c_tables.py), which validates inventories, derives
  before/after comparisons and preserves original integer flags. Inputs:
  [ablation](bench/m2c-ablation.csv), [confirmation before](bench/m2c-confirm-before.csv),
  [confirmation after](bench/m2c-confirm-after.csv), [demos](bench/m2c-demos.csv),
  [write amplification](bench/m2c-ablation-write-amp.csv),
  [benchmark builds](bench/m2c-ablation-builds.csv), and
  [demo builds](bench/m2c-demos-builds.csv). The
  [capture bundle](bench/m2c-capture/README.md) records dates, machine, load,
  commands and original/normalized hashes; the
  [one flag follow-up](bench/m2c-confirm-rerun/README.md) preserves both the
  original and new flags. Separate [runtime evidence](validation/m2c-test-runtime/README.md)
  and [mutation evidence](validation/m2c-mutations/README.md) document correctness
  work without treating test-suite runtime as application performance.
- **`scripts/`** — build, test and benchmark scripts. For Phase 5: [`bench-demos.sh`](../scripts/bench-demos.sh) runs the demo benchmarks against several builds of the extension, and [`profile-refresh.sh`](../scripts/profile-refresh.sh) with [`profile-refresh.py`](../scripts/profile-refresh.py) profiles refresh (Phase 5 spec §6). For Phase 7: [`bench-refresh-all.py`](../scripts/bench-refresh-all.py) times a refresh of every view in each call shape, against two builds (Phase 7 spec §6).
- **`cases/`** — original real-world reports, queries and implementation snapshots, collected before phase evaluation ([README](cases/README.md)).
- **`census/`** — the 2026-10-17 SQL-subset census: 28 candidate applications and the 24 Nexmark queries classified against the v0 subset, with the features that would unlock them and the recomputed [After M2c compatibility projection](census/2026-10-17/README.md#after-m2c).
- **`demos/`** — fourteen demand-backed extension workloads with SQLite recomputation oracles and a shared protocol runner (`crates/ivmlite-test/src/demo_bench.rs`), including Jot's supported stored-bounds projection and three ignored complete-query gates; each demo records its adaptations and results ([README](demos/README.md)).
- **`adr/`** — Architecture Decision Records (reserved; see below).
- **`spikes/`** — feasibility probes and their conclusions. The output is a conclusion, not code to keep.
  - [`2026-09-18-m-1-sqlite-extension-mechanics.md`](spikes/2026-09-18-m-1-sqlite-extension-mechanics.md) — M-1: can the control surface work as designed (blocks M1)
  - [`2026-09-18-m-1-results.md`](spikes/2026-09-18-m-1-results.md) — M-1: results
  - [`2026-09-28-m1b-phase4-cost-spike.md`](spikes/2026-09-28-m1b-phase4-cost-spike.md) — M1b Phase 4: the cost spike that motivated the output index and the one-scan latch
  - [`2026-09-28-m1b-phase4-sqlite342-check.md`](spikes/2026-09-28-m1b-phase4-sqlite342-check.md) — M1b Phase 4: a database with views stays readable and writable by SQLite 3.42

## How ADRs are used

**Every "why not the other path" decision so far is recorded in §12 of the overall design, "Decision record: rejected alternatives"**, including:

| Decision | Location |
|---|---|
| Why the target is SQLite rather than DuckDB | §12.1 |
| Why an extension rather than a "library next to SQLite" | §12.2 |
| Why triggers rather than `preupdate_hook` / the session extension | §12.3 |
| Why not the SQL-to-SQL compilation route | §12.4 |
| Why no silent fallback to full recomputation | §12.5 |
| Why v0 does not build a full DBSP circuit | §12.6 |
| Why DELETE is in v0 rather than v0.2 | §12.7 |
| Why not compare with TanStack DB | §12.8 |

`adr/` is kept for decisions that appear **after the spec is final** — judgments overturned or added during implementation. Each then gets one file, named `NNNN-<kebab-case-title>.md`, plus a row in the table in this file.

The reason: splitting decisions across a dozen files while the spec is still unimplemented only breaks the reading order, whereas decisions made during implementation must be recorded on their own, or they quietly change the spec's premises without anyone noticing.
