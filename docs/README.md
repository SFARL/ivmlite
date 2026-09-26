# ivmlite documentation

## Contents

- **`superpowers/specs/`** — design documents (specs). The authority implementation works from.
  - [`2026-09-18-ivmlite-design.md`](superpowers/specs/2026-09-18-ivmlite-design.md) — the overall design
  - [`2026-09-26-m1b-phase3a-sqlite-extension-design.md`](superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md) — M1b Phase 3a: the SQLite extension, one view end to end
- **`superpowers/plans/`** — implementation plans.
  - [`2026-09-18-m0-test-and-bench-harness.md`](superpowers/plans/2026-09-18-m0-test-and-bench-harness.md) — M0: the test and benchmark harness
  - [`2026-09-20-m1a-phase1-multi-table-harness.md`](superpowers/plans/2026-09-20-m1a-phase1-multi-table-harness.md) — M1a Phase 1: the multi-table harness refactor
  - [`2026-09-21-m1a-phase2-engine.md`](superpowers/plans/2026-09-21-m1a-phase2-engine.md) — M1a Phase 2: the single-table incremental engine
  - [`2026-09-23-m1a-phase3-join.md`](superpowers/plans/2026-09-23-m1a-phase3-join.md) — M1a Phase 3: the two-table equi-join
  - [`2026-09-24-m1b-phase1-state-ownership.md`](superpowers/plans/2026-09-24-m1b-phase1-state-ownership.md) — M1b Phase 1: operator state behind `Arrangement`
  - [`2026-09-24-m1b-phase2a-predicate-whitelist.md`](superpowers/plans/2026-09-24-m1b-phase2a-predicate-whitelist.md) — M1b Phase 2a: the full predicate whitelist
  - [`2026-09-24-m1b-phase2b-sql-front-end.md`](superpowers/plans/2026-09-24-m1b-phase2b-sql-front-end.md) — M1b Phase 2b: the SQL front end
  - [`2026-09-26-m1b-phase3a-sqlite-extension.md`](superpowers/plans/2026-09-26-m1b-phase3a-sqlite-extension.md) — M1b Phase 3a: the SQLite extension
- [`mutation-gates.md`](mutation-gates.md) — **the mutation-gate table**: spec requirement → mutation → the test that goes red. Every invariant M1 adds must be registered here.
- **`bench/`** — benchmark results, charts and conclusions ([README](bench/README.md)).
- **`cases/`** — original real-world reports, queries and implementation snapshots, collected before phase evaluation ([README](cases/README.md)).
- **`adr/`** — Architecture Decision Records (reserved; see below).
- **`spikes/`** — feasibility probes and their conclusions. The output is a conclusion, not code to keep.
  - [`2026-09-18-m-1-sqlite-extension-mechanics.md`](spikes/2026-09-18-m-1-sqlite-extension-mechanics.md) — M-1: can the control surface work as designed (blocks M1)
  - [`2026-09-18-m-1-results.md`](spikes/2026-09-18-m-1-results.md) — M-1: results

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
