# M2d query shapes implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development, then independent spec, quality and integrated review. Continue through validation, commit, PR and latest-head CI without routine user check-ins.

**Goal:** Implement the four user-selected M2d query shapes under the explicit
contract in `docs/superpowers/specs/2026-10-24-m2d-query-shapes-design.md`.

**Architecture:** A resolved scalar AST plus stateless Map and explicit SQLite
evaluator; recursive static AND; physical typed bag copies applied atomically;
format 7; legacy paths/canonical plans and typeless outputs preserved.

**Review amendments:** host REAL-literal parsing can intentionally correct rare
legacy canonical literal bits. New grouped Map plans add a coherent live-key
representative arrangement; legacy aggregate layouts are unchanged. See the
design's two review amendments for the acceptance contract.

**Tech Stack:** Rust 1.95, sqlparser, existing rusqlite/loadable extension and
bundled SQLite; no new server or runtime dependency.

## Task 1: Core IR and safe evaluator seam

Files: `crates/ivmlite-core/src/{query,plan,node,lib}.rs`, with a focused new
scalar expression module if useful.

- [x] Add typed ScalarExpr and approved unary/binary arithmetic operators,
  deterministic canonical rendering, Map plan/node and explicit projection
  evaluator entry point. Preserve legacy Project/Agg and canonical strings.
- [x] Make legacy Node::delta detect evaluator-required plans before any child
  mutation. Keep current core workloads and serde regression fixtures readable.
- [x] Add Predicate::And and recursively validate every leaf/index/affinity;
  canonicalize conjunction order as written. Existing leaf semantics remain.
- [x] Add meaningful core tests for canonical collisions, recursive validation,
  weighted Map collisions with an injected evaluator, and refusal-before-mutation.

## Task 2: SQL lowering and SQLite evaluator

Files: `crates/ivmlite-sql/src/{compile,source}.rs`,
`crates/ivmlite-sqlite/src/view.rs`, optional new evaluator module.

- [x] Implement the spec's scalar parser including minimum INTEGER literal,
  argument maps, row views/wildcard expansion and actual final SELECT order.
  Keep grouped bare-key validation, reject nested/global aggregates and unsupported
  scalar/predicate forms; require unique result names and reserve internal aliases.
- [x] Keep old SQL's legacy lowering when no new feature is required. New richer
  lowering must validate all indices and preserve aliases, joined-column offsets,
  aggregate argument sharing and internal group-key visibility independently of
  final projection.
- [x] Evaluate only approved AST projections in one bound SQLite SELECT; cache
  templates/statements. Call it during bootstrap and refresh at every Map position.
  Keep current aggregate semantics, typeless outputs and failure-before-apply rule.
- [x] Test direct scalar/aggregate/AND examples, SELECT after aggregate, reordering,
  omitted keys, repeated source values under distinct aliases, unknown functions,
  bad recursion, ambiguous columns and unsupported clauses.

## Task 3: Durable physical bags and compatibility

Files: `crates/ivmlite-sqlite/src/{view,names,vtab}.rs` and relevant catalog /
snapshot helpers only as required by actual ownership checks.

- [x] For final bag roots, create encoded typed identity/index and count-aware
  negative staging. Insert physical positive copies; never make duplicate negative
  LIMIT-1 selections compete. Preserve stable physical rowids and cursor snapshots.
- [x] Implement atomic count-check/delete-exact-N inside the final apply statement;
  verify cross-trigger invocation with recursive_triggers OFF/ON. Reject weight
  negation overflow and alias collisions. Check new owned objects on refresh/reopen.
- [x] Use format 7 with explicit non-destructive format 6 refusal/drop/rebuild.
  Preserve old aggregate-only physical path and existing read planner, making new
  bag roots use full-snapshot fallback.
- [x] Exercise duplicate/fanout/retraction/typed-collision and failed-apply cases,
  including deleting many identical rows in a single refresh and post-aggregate
  projection collisions. Check full data/state/watermarks/GC, not only visible rows.

## Task 4: End-to-end acceptance, corpus and delivery

Files: new `crates/ivmlite-test/tests/m2d_query_shapes.rs` and relevant existing
tests only where the supported contract intentionally changes; README,
CHANGELOG, SQL contract docs and new M2d/Nexmark evidence under `docs/`.

- [x] Compare typed multisets and independently intended base changes through
  bootstrap, mixed batches, rollback/savepoints, reopen, shared captures and failures.
  Use actual SQLite expression semantics; exact dyadic fixtures and documented
  aggregate numeric boundaries, not broad tolerance hiding type mismatches.
- [x] Reprobe pinned Nexmark originals/ports and directly execute Q0/Q1/Q20-shaped
  bags where supported. Record all remaining blockers and REAL/DECIMAL or naming
  adaptations. Update coverage projection honestly; do not change historical evidence.
- [x] Run scripts/test-all.sh serially, then required fmt/clippy/minimum SQLite
  checks and targeted compiled red/green controls for expression wiring, AND leaf
  validation, multiplicity and typed identity. Resolve independent spec then quality
  review findings and rerun changed acceptance as needed.
- [x] Archive readable results/provenance with exact code identities and complete
  independent integrated source/evidence review. Local acceptance: 907/0/3,
  six compiled controls, 84 minimum-SQLite assertions, 24 Nexmark checkpoints,
  and six existing native integration sections. See `docs/validation/m2d/`.

Delivery protocol after this local checkpoint: stage specific files, commit
with hooks, push `codex/m2d-query-shapes`, create a stacked PR based on
`codex/selective-reads` so existing PR #7 is unchanged, and attach it to this
chat. Verify latest-head Linux/macOS core/native CI and inspect uploaded raw
results. Live delivery status belongs to the PR checks, separately from this
pre-delivery checkpoint. Release/tag/merge is separate; this milestone does not
authorize those operations.

Reproduction baseline:

```sh
scripts/test-all.sh
cargo fmt --all -- --check
cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings
```

All native execution remains serial under one owner. Existing frozen M2a/M2c
workloads, historical benchmark archives and the parked Redis experiment remain
untouched. Completion requires executable four-shape support and passing current
validation, not only this plan or a theoretical Nexmark feature count.
