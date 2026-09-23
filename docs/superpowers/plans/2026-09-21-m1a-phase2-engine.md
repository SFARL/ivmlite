# M1a Phase 2: Single-Table Incremental Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build v0's incremental engine in `ivmlite-core` — the plan IR, `Arrangement`, Filter / Project / Aggregate, delta consolidation — and plug it into Phase 1's differential harness, green across every enumerated query × biased update sequence.

**Architecture:** `ViewQuery` (the harness's flat query representation) is lowered by `lower()` into spec §5.2's `Plan` tree; the `Plan` is then built into a stateful `Node` operator tree. Operators split along §6.1's three kinds: Filter / Project are stateless and deltas pass straight through; Aggregate holds per-group state and emits retraction pairs per §6.2. `refresh` first merges the batch's raw Δ as a Z-set, then pushes it through the operator tree — so consolidation is an **observable** behaviour of the engine, not an optimisation detail. The engine lives in core; `ivmlite-test` adds an `impl Engine` adapter to plug it into the harness.

**Tech Stack:** Rust 1.95, pure Rust with no `unsafe`, no new dependencies (per §4.2 `ivmlite-core` must not depend on `rusqlite`)

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md`

**Preceding plan:** `docs/superpowers/plans/2026-09-20-m1a-phase1-multi-table-harness.md` (complete and merged into master, 41ea247)

---

## This plan's scope boundary and three design rulings

### Scope: this plan **does not include Join**

Spec §11 gives M1a an internal checkpoint: "first finish the multi-table harness refactor and get the single-table engine green, then add join. That way, when join has a bug it can be bisected (introduced by join, or already wrong in the harness refactor) instead of debugging two kinds of bug at once."

Phase 1 delivered the multi-table harness. This plan delivers **the single-table engine running green**, which is the checkpoint itself. Join is Phase 3, a plan of its own. This split is not caution; it is the direct application of that bisection argument: join is the only bilinear operator, and its `ΔR⋈ΔS` term and the arrangements on both sides are new failure modes no part of this plan has.

**Two other items this plan does not include** (left to later milestones, listed here so they are not taken for omissions):
- A benchmark comparison (one of §11's M1 completion criteria). Both of M0's benchmark control groups run inside SQLite, while the M1a engine is in-memory Rust; comparing them directly is apples to oranges. It becomes comparable once M1b's `ivmlite-sqlite` connects the engine to real shadow tables.
- `ivmlite-sql` (`sqlparser-rs` → IR). M1b.

### Ruling one: `ViewQuery` stays as the harness surface, `Plan` is the engine IR, and `lower()` connects them

`ViewQuery { group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate }` **is** a flat encoding of v0's legal shape — spec §5.2 requires the root operator to be an `Aggregate` with a non-empty `GROUP BY`, so every legal v0 query is exactly `Scan → Filter? → Project → Aggregate`.

There are three reasons not to change the harness to enumerate `Plan` trees directly:

1. **The "exhaustive beats random" premise depends on the flat shape.** `enumerate`'s size is controlled (27 queries for a single table) precisely because the space is the product `group_by × aggs × predicate`; enumerating a tree IR would first have to solve "which trees are legal", which is exactly what `ViewQuery` already encodes away in its types.
2. **The oracle renders SQL from `ViewQuery`.** Switching to `Plan` would mean rewriting `view_query_to_sql`, and the oracle's independence (§9.1) is the foundation of the whole verification; it should not move for the engine's internal representation.
3. **§5.3 says outright that the IR may evolve freely** — what is persisted is the SQL text, not a serialised IR, precisely so the IR carries no compatibility burden. The cost of one extra representation is therefore just one function, `lower()`.

**Cost and ownership**: when join lands, `ViewQuery` is single-table and must be extended or replaced. **That decision belongs to Phase 3**, and this plan does not pre-judge it — by then the operator tree is real code rather than speculation, and the constraints will show themselves.

### Ruling two: `lower()` performs §5.2's validation at the boundary, closing a registered gap early

`docs/mutation-gates.md` has a registered row (m6, found by the final review): §5.2's root-operator constraint holds **only on the generator side** — a `ViewQuery` with `group_by: vec![]` can be built freely outside `enumerate`, and `check_invariants` lets it through all the way. The "When join lands" checklist at the end of the file schedules the boundary check for the join work.

**Moved into this plan**: this plan is exactly the first entry point where `ViewQuery` crosses outside `enumerate` — the engine consumes it directly. `lower()` returns a `Result` and hard-errors on an empty `group_by` or empty `aggs`. That row's "n/a" becomes "verified" in this plan's Task 1 and is struck from the join checklist.

### Ruling three: no `Expr` with only one variant

Spec §5.2 writes `Plan` as `Filter { predicate: Expr }`, `Project { exprs: Vec<Expr> }`, `Aggregate { group_by: Vec<Expr> }`. But §5.2 also requires v0's group-by keys to be **bare columns only**, and §6.1's whitelist of comparison operators contains nothing that needs an expression tree.

So v0's `Expr` would be an enum with a single variant, `Column(usize)` — pure hollow generalisation. This plan uses `Vec<usize>` (projection and group-by columns) and the existing `Predicate` (filtering) instead. When the first feature that really needs expressions appears (computed columns beyond M4's `MIN`/`MAX`), introducing `Expr` then is a local change, whereas introducing it now is a contentless layer of indirection every one of five operators has to pass through.

---

## Global Constraints

Each of the following comes from the spec and is an implicit requirement of every task:

- **§4.2 `ivmlite-core` must not depend on `rusqlite`**, nor on `ivmlite-test` (the latter depends on the former; the reverse would create a cycle). This plan introduces no new dependency.
- **§4.2 the only place in the whole project allowed to use `unsafe` is `ivmlite-sqlite` (M1b).** Not one line of `unsafe` should appear in this plan.
- **§5.1 a row whose weight reaches zero must be deleted**, leaving no zombie entries. `ZSet::update` already does this; operator state must too.
- **§5.2 the root operator must be an `Aggregate` with a non-empty `group_by`**; global aggregates are forbidden.
- **§6.1 `SUM` over zero non-NULL inputs returns `NULL`, not `0`.** Aggregate state must maintain both the running sum and the count of non-NULL inputs.
- **§6.1 predicates use three-valued logic**: NULL evaluates to UNKNOWN and the row is not included; **do not conclude from this that `NOT p` is equivalent to `!p`**.
- **§6.1 integer overflow is undefined**; the value range is clamped by the generator (`|per-group sum| < 2^62`); the engine does no static check.
- **§6.2 aggregates must emit retraction pairs**: when a SUM goes from 100 to 150, emit `(key,100) w=-1` and `(key,150) w=+1`, not a lone `+1` row. This is IVM's biggest source of bugs.
- **§6.3 `Arrangement::get` returns an iterator, not an `Option`** (key → many values), and the trait must be object-safe (use `Box<dyn Iterator>`, not RPITIT).
- **§8.5 `apply` takes the unconsolidated raw Δ**; `refresh` is separate from `apply`; `apply` carries a table name. None of these three signatures may change.
- **§9.4 iteration order is deterministic**: any iteration that can affect the output uses `Vec` / `BTreeMap`, never `HashMap` / `HashSet`.
- **Mutation gates**: every spec-mandated behaviour gets a row in `docs/mutation-gates.md`, and "verified" must come from actually running the mutation. Break it → **confirm it still compiles** → run the tests → confirm the named test goes red → restore. A compile failure produces no test output, which under careless filtering looks exactly like "tests passed".
- **The test command must be `cargo test --workspace --locked --no-fail-fast`.** Without `--no-fail-fast`, a failure in an upstream crate masks the real target.
- Before every commit, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` must pass.
- After adding gate rows, run `python3 scripts/count-mutation-gates.py --fix`, then run it bare once more to confirm it prints "consistent". Note the script **checks only the summary sentence and the literal cell contents**; it does not judge whether a row's "verified" was really obtained by running the mutation — "consistent" does not mean the table is honest.
- Stage specific files, not `git add -A`; never use `--no-verify`. End the commit message with:
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ivmlite-core/src/plan.rs` (new) | spec §5.2's `Plan` enum; `lower(&ViewQuery, &str) -> Result<Plan, PlanError>`; §5.2's boundary validation |
| `crates/ivmlite-core/src/arrangement.rs` (new) | spec §6.3's `Arrangement` trait; `MemArrangement` (M1a's in-memory implementation, replaced by a shadow table in M1b) |
| `crates/ivmlite-core/src/node.rs` (new) | The stateful operator tree `Node`: built from a `Plan`; `delta()` advances by §6.1's three kinds of rule |
| `crates/ivmlite-core/src/agg.rs` (new) | `Aggregate`'s group state and §6.2's retraction logic. It gets its own file because it is where all of v0's difficulty lies (§6.1: "all of v0's difficulty is concentrated in aggregation") |
| `crates/ivmlite-core/src/engine.rs` (new) | `IncrementalEngine`: holds the operator tree and the per-table pending raw Δ; `refresh` consolidates first, then advances |
| `crates/ivmlite-core/src/lib.rs` (modified) | Exports the modules above |
| `crates/ivmlite-test/src/incremental.rs` (new) | The `impl Engine for IncrementalEngine` adapter (local trait + foreign type, allowed) |
| `crates/ivmlite-test/src/lib.rs` (modified) | Exports the adapter |
| `crates/ivmlite-test/tests/harness_catches_bugs.rs` (modified) | End-to-end tests of the real engine plugged into the differential harness |
| `docs/mutation-gates.md` (modified) | Each task registers its own gate rows |

The operators are split into two files, `node.rs` + `agg.rs`, rather than one: a linear operator's delta rule is three lines (`Δ(f(R)) = f(ΔR)`), while aggregation is dozens of lines with a state machine. Put together, the latter drowns the former, and their correctness arguments are entirely different.

---

## Task 1: Plan IR, lowering, §5.2 boundary validation

**Files:**
- Create: `crates/ivmlite-core/src/plan.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ivmlite_core::{ViewQuery, Agg, AggFn, Predicate}` (already in `query.rs`)
- Produces:
  - `pub enum Plan { Scan { table: String, columns: Vec<usize> }, Filter { input: Box<Plan>, predicate: Predicate }, Project { input: Box<Plan>, columns: Vec<usize> }, Aggregate { input: Box<Plan>, group_by: Vec<usize>, aggs: Vec<Agg> } }`
  - `pub struct PlanError(pub String)`
  - `pub fn lower(query: &ViewQuery, table: &str, arity: usize) -> Result<Plan, PlanError>`

> **This plan does not add the `Join` variant.** Spec §5.2 lists it in `Plan` marked "M1a", but adding a variant every match arm has to answer with `unreachable!()` leaves, on every `match`, code no test can reach — exactly the kind this project keeps pushing down. When Phase 3 adds the variant, the compiler will point out every place that must handle it, one by one, which is more reliable than a reserved placeholder.

### Why lowering emits a `Project`

The naive lowering is `Aggregate { input: Filter { Scan } }`, which produces no `Project` at all. But spec §11 lists `Project` in M1a, so there would be an operator that **no test can reach**.

This plan's lowering makes `Project` really carry weight: `Scan` takes every column → `Filter` evaluates against the original column indices → `Project` narrows to the columns the query actually needs (`group_by ∪ each SUM's column`) → `Aggregate` works on the narrowed new indices.

So a `Project` node exists in every enumerated case (`enumerate` × `lower` at arity 2 was measured across all 27 cases, with `Project` present in every one), but "present" does not mean "narrowing": in the same measurement, `Project` really narrowed the column count in 12 cases, and in the other 15 it narrowed to the same column set as its input — an identity projection. For example: with `group_by=[0]`, `aggs=[COUNT(*)]`, `predicate=IntGt{column:1}`, the filter uses column 1 while the aggregate needs only column 0, and `Project` narrows 2 columns to 1 — one of those 12 truly narrowing cases. Even so, adding the `Project` step still holds up: without it, `Project` as an operator has no reachable call site at all, and "12/27 narrow" could not even be said.

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-core/src/plan.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery { group_by, aggs, predicate }
    }

    fn count() -> Agg {
        Agg { func: AggFn::Count, column: None }
    }

    fn sum(column: usize) -> Agg {
        Agg { func: AggFn::Sum, column: Some(column) }
    }

    #[test]
    fn lowers_to_scan_filter_project_aggregate() {
        // The predicate uses column 1 and the aggregate needs only column 0 — Project must narrow 2 columns to 1,
        // and Aggregate's group_by indices must be remapped to the narrowed positions.
        let plan = lower(
            &q(vec![0], vec![count()], Predicate::IntGt { column: 1, value: 3 }),
            "orders",
            2,
        )
        .expect("a legal query must lower");

        let Plan::Aggregate { input, group_by, aggs } = &plan else {
            panic!("the root operator must be Aggregate (spec §5.2): {plan:?}");
        };
        assert_eq!(group_by, &vec![0], "after narrowing, the group key lands at position 0");
        assert_eq!(aggs.len(), 1);

        let Plan::Project { input, columns } = &**input else {
            panic!("Aggregate's input must be a Project: {input:?}");
        };
        assert_eq!(columns, &vec![0], "only column 0 is used by the aggregate");

        let Plan::Filter { input, predicate } = &**input else {
            panic!("Project's input must be a Filter: {input:?}");
        };
        assert_eq!(predicate, &Predicate::IntGt { column: 1, value: 3 });

        let Plan::Scan { table, columns } = &**input else {
            panic!("the bottom must be a Scan: {input:?}");
        };
        assert_eq!(table, "orders");
        assert_eq!(columns, &vec![0, 1], "Scan takes every column — Filter evaluates against the original indices");
    }

    #[test]
    fn no_filter_node_when_predicate_is_none() {
        // Predicate::None must not produce an always-true Filter node: one more node is one more
        // pointless pass every batch, and it would make "Filter is correctly skipped" unobservable.
        let plan = lower(&q(vec![0], vec![count()], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, .. } = &plan else { panic!("{plan:?}") };
        let Plan::Project { input, .. } = &**input else { panic!("{input:?}") };
        assert!(
            matches!(&**input, Plan::Scan { .. }),
            "under Predicate::None the input should be a Scan directly, with no always-true Filter inserted: {input:?}"
        );
    }

    #[test]
    fn projection_keeps_group_keys_and_summed_columns_in_a_stable_order() {
        // The group key is column 1 and the SUM is over column 0 — the narrowed order must be deterministic and predictable,
        // or Aggregate's index remapping cannot line up (spec §9.4).
        let plan = lower(&q(vec![1], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, group_by, aggs } = &plan else { panic!("{plan:?}") };
        let Plan::Project { columns, .. } = &**input else { panic!("{input:?}") };
        assert_eq!(columns, &vec![1, 0], "group keys first (in original order), then each agg's column (in original order)");
        assert_eq!(group_by, &vec![0], "the group key is remapped to narrowed position 0");
        assert_eq!(aggs[0].column, Some(1), "the SUM's column is remapped to narrowed position 1");
    }

    #[test]
    fn a_column_used_as_both_group_key_and_sum_target_is_projected_once() {
        // A column that is both a group key and a SUM target must not appear twice in the projection — appearing twice
        // would make Project's output row width disagree with what Aggregate expects.
        let plan = lower(&q(vec![0], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, group_by, aggs } = &plan else { panic!("{plan:?}") };
        let Plan::Project { columns, .. } = &**input else { panic!("{input:?}") };
        assert_eq!(columns, &vec![0], "projected only once after dedup");
        assert_eq!(group_by, &vec![0]);
        assert_eq!(aggs[0].column, Some(0));
    }

    #[test]
    fn empty_group_by_is_rejected_at_the_boundary() {
        // spec §5.2: global aggregates are forbidden — over an empty table one returns 1 row (with NULL values), while a grouped
        // aggregate returns 0 rows, and the rule "delete the row when the group count reaches zero" is wrong for the former.
        // Until now this held only on the generator side (enumerate never produces this shape); once the engine
        // consumes ViewQuery directly, the boundary check must be here.
        let err = lower(&q(vec![], vec![count()], Predicate::None), "orders", 2)
            .expect_err("an empty group_by must be rejected");
        assert!(
            err.0.contains("group_by") || err.0.contains("GROUP BY"),
            "the error should say the problem is group_by: {}",
            err.0
        );
    }

    #[test]
    fn empty_aggs_is_rejected_at_the_boundary() {
        // The root operator must be an Aggregate; an "Aggregate" with no aggregates is really
        // Scan→Project becoming the view directly, which is exactly the shape §5.2 rules illegal
        // (a Z-set weight of 2 shows as 2 rows, while an ordinary SQL view shows 3).
        let err = lower(&q(vec![0], vec![], Predicate::None), "orders", 2)
            .expect_err("an empty aggs must be rejected");
        assert!(err.0.contains("agg"), "the error should say the problem is aggs: {}", err.0);
    }

    #[test]
    fn out_of_range_column_is_rejected() {
        // An out-of-range index must be reported while lowering, not as a panic at refresh —
        // after create_view the engine should have no foreseeable panic path.
        let err = lower(&q(vec![7], vec![count()], Predicate::None), "orders", 2)
            .expect_err("an out-of-range group key must be rejected");
        assert!(err.0.contains('7'), "the error should name the out-of-range index: {}", err.0);
    }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked plan`
Expected: a compile failure, `cannot find function lower` / `cannot find type Plan`

- [ ] **Step 3: Write the implementation**

At the start of `crates/ivmlite-core/src/plan.rs`:

```rust
use crate::{Agg, Predicate, ViewQuery};

/// spec §5.2's plan IR.
///
/// v0's legal shape is always `Scan → Filter? → Project → Aggregate`, guaranteed by `lower`.
/// The `Join` variant is added in Phase 3 together with the join operator — adding now a variant every match arm
/// can only answer with `unreachable!()` would leave, at every match, code no test can reach.
///
/// spec §5.2 writes the expressions inside nodes as `Expr`; this implementation uses `Vec<usize>` (column indices) and
/// `Predicate` instead: v0's group-by keys can only be bare columns (§5.2), and the predicate whitelist (§6.1)
/// contains nothing that needs an expression tree, so in v0 `Expr` would be a hollow enum with a single variant,
/// `Column(usize)`. Introducing it when the first feature that really needs expressions appears is a local change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Scan {
        table: String,
        columns: Vec<usize>,
    },
    Filter {
        input: Box<Plan>,
        predicate: Predicate,
    },
    Project {
        input: Box<Plan>,
        /// The **input** column indices to keep, in output order.
        columns: Vec<usize>,
    },
    Aggregate {
        input: Box<Plan>,
        /// Indices are relative to `Project`'s **output**, not the base table.
        group_by: Vec<usize>,
        /// Each `Agg::column` is likewise already remapped to `Project`'s output positions.
        aggs: Vec<Agg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PlanError {}

/// Lower the harness's flat `ViewQuery` into an operator tree, enforcing §5.2's legality checks at the boundary.
///
/// This is the first entry point where `ViewQuery` crosses outside `enumerate` — the engine consumes it directly.
/// `enumerate` never produces an illegal shape (`enumerate_covers_the_v0_space_and_is_nonempty`
/// guards that), but a `ViewQuery` can be built freely, so the check has to be here.
///
/// `arity` is the base table's column count, used for index bounds checks.
pub fn lower(query: &ViewQuery, table: &str, arity: usize) -> Result<Plan, PlanError> {
    if query.group_by.is_empty() {
        return Err(PlanError(
            "spec §5.2: the view's root operator must be an Aggregate with a non-empty GROUP BY; \
             global aggregates are forbidden (over an empty table one returns 1 row while a grouped aggregate returns 0, \
             and the rule \"delete the row when the group count reaches zero\" is wrong for the former)"
                .into(),
        ));
    }
    if query.aggs.is_empty() {
        return Err(PlanError(
            "spec §5.2: a query with no agg is really Scan→Project becoming the view directly, \
             and in that shape Z-set weights and SQL row counts disagree semantically"
                .into(),
        ));
    }

    let mut check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} references column index {c}, but table {table} has only {arity} columns"
            )))
        } else {
            Ok(())
        }
    };
    for &c in &query.group_by {
        check(c, "group_by")?;
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            check(c, "agg")?;
        }
    }
    match &query.predicate {
        Predicate::None => {}
        Predicate::IntGt { column, .. } | Predicate::IsNotNull { column } => {
            check(*column, "predicate")?
        }
    }

    // Columns the projection keeps: group keys first (in original order), then each agg's column (in original order), deduplicated.
    // The order must be deterministic, or Aggregate's index remapping cannot line up (spec §9.4).
    let mut keep: Vec<usize> = Vec::new();
    for &c in &query.group_by {
        if !keep.contains(&c) {
            keep.push(c);
        }
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            if !keep.contains(&c) {
                keep.push(c);
            }
        }
    }
    let remap = |c: usize| {
        keep.iter()
            .position(|&k| k == c)
            .expect("keep is built from group_by and the agg columns, so it must contain them")
    };

    // Scan takes every column: Filter's predicate is evaluated against **base-table** indices, and narrowing happens after Filter.
    let mut node = Plan::Scan {
        table: table.to_string(),
        columns: (0..arity).collect(),
    };
    if query.predicate != Predicate::None {
        node = Plan::Filter {
            input: Box::new(node),
            predicate: query.predicate.clone(),
        };
    }
    node = Plan::Project {
        input: Box::new(node),
        columns: keep.clone(),
    };
    Ok(Plan::Aggregate {
        input: Box::new(node),
        group_by: query.group_by.iter().map(|&c| remap(c)).collect(),
        aggs: query
            .aggs
            .iter()
            .map(|a| Agg {
                func: a.func,
                column: a.column.map(remap),
            })
            .collect(),
    })
}
```

`crates/ivmlite-core/src/lib.rs` gains:

```rust
pub mod plan;
pub use plan::{lower, Plan, PlanError};
```

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes

- [ ] **Step 5: Mutation verification**

Run them one at a time, each time first confirming it **compiles** before reading the test output:

1. Delete the `query.group_by.is_empty()` branch → expect `empty_group_by_is_rejected_at_the_boundary` to go red
2. Delete the `query.aggs.is_empty()` branch → expect `empty_aggs_is_rejected_at_the_boundary` to go red
3. Delete every `check(...)` call → expect `out_of_range_column_is_rejected` to go red
4. Remove every `if !keep.contains(&c)` dedup condition on `keep` (push directly) → expect `a_column_used_as_both_group_key_and_sum_target_is_projected_once` to go red
5. Change `Scan`'s `columns` to `keep.clone()` (narrowing at the Scan) → expect `lowers_to_scan_filter_project_aggregate` to go red (Filter's predicate index would point at the wrong column)
6. Insert a `Filter` node even under `Predicate::None` → expect `no_filter_node_when_predicate_is_none` to go red

- [ ] **Step 6: Register the gates and commit**

Add the 6 rows above to the "ivmlite-core" table of `docs/mutation-gates.md`, with "verified" set to `**verified**`.

**Also** change the m6 row — `§5.2 the root operator must be an aggregate with a non-empty GROUP BY — ViewQuery moved into ivmlite-core precisely so the M1 engine could consume it directly, yet there is no check at the boundary` — from "n/a" to `**verified**`, with the mutation "delete the `group_by.is_empty()` check in `lower`" and the test that goes red `empty_group_by_is_rejected_at_the_boundary`.

**And** delete the §5.2 paragraph from the "When join lands" checklist at the end of the file, since it is no longer owed. The checklist should list only what is really still owed — leaving a debt that has been paid on it makes the next person working through the list discount the whole thing.

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): plan IR and lowering, with §5.2 validation at the boundary"
```

---

## Task 2: The `Arrangement` trait and an in-memory implementation

**Files:**
- Create: `crates/ivmlite-core/src/arrangement.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ivmlite_core::Row`
- Produces:
  - `pub trait Arrangement { fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>; fn update(&mut self, key: &Row, val: &Row, weight_delta: i64); fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>; }`
  - `pub struct MemArrangement` (`Default` + `new()`)

> `get` returning an iterator rather than an `Option` is a hard requirement of spec §6.3: v0's group-by stores only one value per key and has no use for multiple values, but **each side of a join is key → many rows**. M0 already paid for this per §6.3; this task collects on it. Likewise the trait must be object-safe — `Box<dyn Iterator>` rather than `-> impl Iterator`, or `dyn Arrangement` does not hold and type parameters propagate through the whole operator tree (§6.3 implementation note).

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-core/src/arrangement.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn r(vals: Vec<i64>) -> Row {
        Row::new(vals.into_iter().map(Value::Int).collect())
    }

    #[test]
    fn one_key_can_hold_multiple_values() {
        // spec §6.3: get returns an iterator rather than an Option, because each side of a join is
        // key → many rows. v0's group-by has no use for it, but this shape must hold now,
        // or Phase 3 would have to change the signature of the whole operator tree.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![1]), &r(vec![20]), 3);
        let mut got: Vec<(Row, i64)> = a.get(&r(vec![1])).collect();
        got.sort();
        assert_eq!(got, vec![(r(vec![10]), 1), (r(vec![20]), 3)]);
    }

    #[test]
    fn weights_accumulate_and_zero_removes_the_entry() {
        // spec §5.1: a row whose weight reaches zero must be deleted, leaving no zombie entry.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 2);
        a.update(&r(vec![1]), &r(vec![10]), -2);
        assert_eq!(a.get(&r(vec![1])).count(), 0, "no entry may remain after reaching zero");
        assert_eq!(a.scan().count(), 0, "scan must not see it either");
    }

    #[test]
    fn a_key_with_no_values_left_disappears_from_scan() {
        // Removing only the (key, val) is not enough — the key itself must not remain as an empty shell,
        // or scan's row count would grow with history rather than with the current state.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![2]), &r(vec![20]), 1);
        a.update(&r(vec![1]), &r(vec![10]), -1);
        let keys: Vec<Row> = a.scan().map(|(k, _, _)| k).collect();
        assert_eq!(keys, vec![r(vec![2])], "a key that became empty must disappear");
    }

    #[test]
    fn negative_weights_are_representable() {
        // Negative weights are legal in intermediate deltas (spec §5.1) — they only must not appear in the final materialised result.
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), -5);
        assert_eq!(a.get(&r(vec![1])).collect::<Vec<_>>(), vec![(r(vec![10]), -5)]);
    }

    #[test]
    fn scan_order_is_deterministic() {
        // spec §9.4: a failing case must replay exactly from its seed, so any iteration that can affect the output
        // must come from an ordered container.
        let build = || {
            let mut a = MemArrangement::new();
            for k in [3, 1, 2] {
                for v in [30, 10, 20] {
                    a.update(&r(vec![k]), &r(vec![v]), 1);
                }
            }
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
        let keys: Vec<Row> = build().into_iter().map(|(k, _, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "scan must be ordered by key");
    }

    #[test]
    fn get_on_a_missing_key_is_empty_not_a_panic() {
        let a = MemArrangement::new();
        assert_eq!(a.get(&r(vec![99])).count(), 0);
    }

    #[test]
    fn mem_arrangement_is_usable_as_a_trait_object() {
        // spec §6.3 implementation note: the trait must be object-safe, or type parameters would be forced
        // to propagate through the operator tree. This test is that constraint's compile-time gate.
        let mut a: Box<dyn Arrangement> = Box::new(MemArrangement::new());
        a.update(&r(vec![1]), &r(vec![10]), 1);
        assert_eq!(a.get(&r(vec![1])).count(), 1);
    }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked arrangement`
Expected: a compile failure, `cannot find type MemArrangement`

- [ ] **Step 3: Write the implementation**

At the start of `crates/ivmlite-core/src/arrangement.rs`:

```rust
use std::collections::BTreeMap;

use crate::Row;

/// spec §6.3. key → many (value, weight).
///
/// `get` returns an iterator rather than an `Option`: v0's group-by stores only one value per key and has no use
/// for multiple values, but each side of a join is key → many rows (§6.3 says outright this is the main place where M0 must
/// not make a decision that would force join to be reworked).
///
/// `Box<dyn Iterator>` rather than RPITIT is for object safety: operators need to hold a
/// `dyn Arrangement` (M1b's implementation comes from `ivmlite-sqlite`), and RPITIT would make the trait
/// not object-safe, forcing type parameters to propagate through the whole operator tree.
pub trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}

/// M1a's in-memory implementation. M1b adds another implementation backed by SQLite shadow tables.
///
/// Both levels use `BTreeMap`: spec §9.4 requires every iteration order that can affect the output to be deterministic,
/// and `scan()`'s order goes straight into the delta stream.
#[derive(Debug, Clone, Default)]
pub struct MemArrangement {
    inner: BTreeMap<Row, BTreeMap<Row, i64>>,
}

impl MemArrangement {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Arrangement for MemArrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_> {
        match self.inner.get(key) {
            Some(vals) => Box::new(vals.iter().map(|(v, &w)| (v.clone(), w))),
            None => Box::new(std::iter::empty()),
        }
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) {
        if weight_delta == 0 {
            return;
        }
        let vals = self.inner.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        // spec §5.1: delete on reaching zero, no zombie entries. When a key becomes empty the key goes too,
        // or scan's size would grow with history rather than with the current state.
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                self.inner.remove(key);
            }
        }
    }

    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_> {
        Box::new(
            self.inner
                .iter()
                .flat_map(|(k, vals)| vals.iter().map(move |(v, &w)| (k.clone(), v.clone(), w))),
        )
    }
}
```

`crates/ivmlite-core/src/lib.rs` gains:

```rust
pub mod arrangement;
pub use arrangement::{Arrangement, MemArrangement};
```

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes

- [ ] **Step 5: Mutation verification**

1. Make `get` return at most one value (`vals.iter().take(1)`) → expect `one_key_can_hold_multiple_values` to go red
2. Delete the whole `if *w == 0 { vals.remove(val); ... }` block → expect `weights_accumulate_and_zero_removes_the_entry` to go red
3. Delete only the line `if vals.is_empty() { self.inner.remove(key); }` → expect `a_key_with_no_values_left_disappears_from_scan` to go red
4. Replace the outer `BTreeMap` with a `HashMap` (making `scan`'s order nondeterministic) → expect `scan_order_is_deterministic` to go red. **Note this one is statistical**: `HashMap`'s `RandomState` reseeds per process, and 3 keys have a 1/3! ≈ 16.7% chance of happening to come out ordered. Run at least 10 processes to confirm, and state honestly in the gate row that this is a statistical guard, not an absolute one.
5. Delete `if weight_delta == 0 { return; }` in `update` → expect it to **stay green** (after `or_insert(0)`, adding 0 and then checking for zero deletes the entry — equivalent behaviour). Record this row as "known unguarded", because it is purely a short-circuit optimisation with no observable semantics.

- [ ] **Step 6: Register the gates and commit**

Set the first 4 rows to `**verified**` (noting the 4th is statistical), and the 5th to `n/a` with the reason written out.

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/arrangement.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): the Arrangement trait and an in-memory implementation"
```

---

## Task 3: Linear operators — Filter and Project

**Files:**
- Create: `crates/ivmlite-core/src/node.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `Plan` (Task 1), `ZSet`, `Row`, `Value`, `Predicate`
- Produces:
  - `pub enum Node { Scan { table: String }, Filter { input: Box<Node>, predicate: Predicate }, Project { input: Box<Node>, columns: Vec<usize> }, Aggregate { input: Box<Node>, agg: AggState } }` (the inner type of the `Aggregate` variant is defined by Task 4; **this task implements only the first three variants, and `Node::build` returns a `NodeError` on `Plan::Aggregate`**)
  - `pub struct NodeError(pub String)`
  - `impl Node { pub fn build(plan: &Plan) -> Result<Node, NodeError>; pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet }`

> **Why `Aggregate` is split into two tasks**: a linear operator's delta rule is `Δ(f(R)) = f(ΔR)` — three lines, stateless, correct at a glance. Aggregation is dozens of lines with a state machine, and §6.1 says outright that "all of v0's difficulty is concentrated in aggregation". In one combined task, the review would have to judge two entirely different kinds of correctness argument at once, and one would drown the other.

### `Scan`'s delta rule and the table name

`Scan { table }`'s `delta(t, input)` returns `input` when `t == table` and an empty `ZSet` otherwise. With a single table the check looks redundant, but it is exactly the mechanism by which each side of a join absorbs only its own table's deltas — Phase 3 need not change `Scan`.

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-core/src/node.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Predicate, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    #[test]
    fn scan_only_absorbs_its_own_table() {
        // With a single table this looks redundant, but it is exactly the mechanism by which each side of a join absorbs only
        // its own table's deltas (Phase 3 need not change Scan).
        let mut n = Node::build(&Plan::Scan {
            table: "orders".into(),
            columns: vec![0, 1],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]);
        assert_eq!(n.delta("orders", &d), d, "its own table: passes through unchanged");
        assert_eq!(n.delta("customers", &d), ZSet::new(), "another table: empty");
    }

    #[test]
    fn filter_passes_deltas_through_unchanged_for_matching_rows() {
        // spec §6.1: linear operators satisfy Δ(f(R)) = f(ΔR) — deltas pass straight through, stateless.
        // Weights must be kept unchanged, including negative ones (retracting a row that satisfies the predicate —
        // the retraction itself must pass through too).
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(5)]), 1), (row(vec![int(9)]), -2)]);
        assert_eq!(n.delta("t", &d), d);
    }

    #[test]
    fn filter_drops_rows_that_fail_the_predicate() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1)]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn filter_treats_null_as_unknown_not_as_false_negation() {
        // spec §6.1 three-valued logic: with v in {1, NULL, 5}, `v > 3` matches 1 row,
        // and `NOT (v > 3)` also matches only 1 row — together 2, not 3.
        // What this test pins is that the NULL row goes into neither side, not that "NULL is equivalent to false".
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(1)]), 1),
            (row(vec![Value::Null]), 1),
            (row(vec![int(5)]), 1),
        ]);
        let got = n.delta("t", &d);
        assert_eq!(got, ZSet::from_rows([(row(vec![int(5)]), 1)]));
        assert_eq!(got.weight_of(&row(vec![Value::Null])), 0, "the NULL row must not enter the result");
    }

    #[test]
    fn is_not_null_predicate_filters_null_rows() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IsNotNull { column: 0 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![Value::Null]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn project_narrows_columns_and_preserves_weights() {
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1, 2] }),
            columns: vec![2, 0],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2), int(3)]), 4)]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![int(3), int(1)]), 4)]),
            "columns are reordered as `columns` specifies, weights kept unchanged"
        );
    }

    #[test]
    fn project_merges_rows_that_become_identical_after_narrowing() {
        // When two rows become the same row after narrowing, their weights must add rather than the latter overwriting the former —
        // this is Z-set semantics, and the only non-trivial thing about Project.
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1] }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), 3),
        ]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(7)]), 5)]));
    }

    #[test]
    fn project_drops_rows_whose_weights_cancel_after_narrowing() {
        // A row whose weights cancel to 0 after narrowing must disappear (spec §5.1), not remain as a weight-0 entry.
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1] }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), -2),
        ]);
        assert!(n.delta("t", &d).is_empty(), "must be empty after cancellation");
    }

    #[test]
    fn building_an_aggregate_is_an_error_until_task_4() {
        // Placeholder: Task 4 deletes this test and replaces it with real aggregate tests. It is here so that
        // "not implemented" has an explicit shape that actually gets executed, rather than a panic.
        let err = Node::build(&Plan::Aggregate {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            group_by: vec![0],
            aggs: vec![],
        })
        .expect_err("Task 3 has not implemented Aggregate yet");
        assert!(err.0.contains("Aggregate"));
    }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked node`
Expected: a compile failure, `cannot find type Node`

- [ ] **Step 3: Write the implementation**

At the start of `crates/ivmlite-core/src/node.rs`:

```rust
use crate::{Plan, Predicate, Row, Value, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeError(pub String);

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NodeError {}

/// The stateful operator tree. Built from a `Plan`, after which `delta` is called repeatedly.
///
/// Separate from `Plan` because `Plan` is a pure description (comparable, printable, and one day rebuildable from SQL),
/// while operators must hold state. Spec §5.3 storing the SQL text rather than a serialised IR relies on this separation.
#[derive(Debug)]
pub enum Node {
    Scan {
        table: String,
    },
    Filter {
        input: Box<Node>,
        predicate: Predicate,
    },
    Project {
        input: Box<Node>,
        columns: Vec<usize>,
    },
}

impl Node {
    pub fn build(plan: &Plan) -> Result<Node, NodeError> {
        match plan {
            Plan::Scan { table, .. } => Ok(Node::Scan {
                table: table.clone(),
            }),
            Plan::Filter { input, predicate } => Ok(Node::Filter {
                input: Box::new(Node::build(input)?),
                predicate: predicate.clone(),
            }),
            Plan::Project { input, columns } => Ok(Node::Project {
                input: Box::new(Node::build(input)?),
                columns: columns.clone(),
            }),
            Plan::Aggregate { .. } => Err(NodeError(
                "the Aggregate operator is not implemented yet (this plan's Task 4)".into(),
            )),
        }
    }

    /// Push one batch of a table's deltas through this node, returning this node's output delta.
    ///
    /// spec §6.1: linear operators satisfy `Δ(f(R)) = f(ΔR)`, so Filter / Project are stateless and
    /// deltas pass straight through.
    pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        match self {
            Node::Scan { table: own } => {
                if own == table {
                    input.clone()
                } else {
                    ZSet::new()
                }
            }
            Node::Filter { input: child, predicate } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    if passes(predicate, row) {
                        out.update(row.clone(), w);
                    }
                }
                out
            }
            Node::Project { input: child, columns } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    // After narrowing it may coincide with another row — `ZSet::update` adds the weights and
                    // deletes the entry on reaching zero, exactly the Z-set semantics needed.
                    let narrowed = Row::new(columns.iter().map(|&c| row.get(c).clone()).collect());
                    out.update(narrowed, w);
                }
                out
            }
        }
    }
}

/// spec §6.1's three-valued logic: NULL evaluates to UNKNOWN, and the row is not included.
///
/// It returns `bool` rather than a three-valued enum because under **filtering** semantics "does not pass" and "unknown"
/// merge into the same treatment. But **do not conclude from this that `NOT p` is equivalent to `!p`** — v0's predicate
/// whitelist has no `NOT` precisely because every addition means re-arguing three-valued logic.
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(i) => i > value,
            // NULL > 3 is UNKNOWN; Text > Int does not occur in v0's enumeration
            // (as enumerate_only_sums_integer_columns implies, IntGt is generated only for Integer columns).
            _ => false,
        },
        Predicate::IsNotNull { column } => !matches!(row.get(*column), Value::Null),
    }
}
```

`crates/ivmlite-core/src/lib.rs` gains:

```rust
pub mod node;
pub use node::{Node, NodeError};
```

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes

- [ ] **Step 5: Mutation verification**

1. Remove the table-name check from `Node::Scan`'s `delta` (always return `input.clone()`) → expect `scan_only_absorbs_its_own_table` to go red
2. Make `passes` return `true` for `Value::Null` → expect `filter_treats_null_as_unknown_not_as_false_negation` and `is_not_null_predicate_filters_null_rows` to go red
3. Change `Project`'s `out.update(narrowed, w)` to a direct overwriting insert (via a temporary `BTreeMap` and `insert`) → expect `project_merges_rows_that_become_identical_after_narrowing` to go red
4. Replace `w` with `1` in `Filter`'s `out.update(row.clone(), w)` → expect `filter_passes_deltas_through_unchanged_for_matching_rows` to go red
5. Change `Project`'s `columns.iter().map(...)` to clone the whole row as is → expect `project_narrows_columns_and_preserves_weights` to go red

- [ ] **Step 6: Register the gates and commit**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): linear operators Filter and Project"
```

---

## Task 4: Aggregate and retraction semantics

**Files:**
- Create: `crates/ivmlite-core/src/agg.rs`
- Modify: `crates/ivmlite-core/src/node.rs` (add the `Aggregate` variant, delete Task 3's placeholder test)
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `Agg`, `AggFn`, `Row`, `Value`, `ZSet`, `Node` (Task 3)
- Produces:
  - `pub struct AggState`, with `pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState` and `pub fn absorb(&mut self, input: &ZSet) -> ZSet`
  - The `Node::Aggregate { input: Box<Node>, state: AggState }` variant; `Node::build` no longer errors on `Plan::Aggregate`

> **This is where all of this plan's difficulty lies.** Spec §6.2 opens with "this is IVM's biggest source of bugs and must be followed strictly".

### The state and the retraction contract

Each group maintains:

- `rows: i64` — the sum of the weights of the group's rows. It is exactly `COUNT(*)`'s output; when it reaches zero, the group disappears from the output.
- One accumulator per agg:
  - `Count` needs no extra state (it uses `rows`).
  - `Sum` needs **two** quantities: `sum: i64` (the running sum) and `non_null: i64` (the sum of the weights of non-NULL inputs). §6.1 says outright that an implementation maintaining only the running sum outputs `0` when "the group is non-empty but the column is all NULL", while SQLite outputs `NULL`, and that mismatch is **silent**.
- `emitted: Option<Row>` — **the row this group last emitted**. §6.2: an aggregate must remember what it emitted in order to retract it. This is the real reason aggregation needs state.

The emission rule (once for each touched group at the end of every batch):

```
new_out = if rows > 0 { Some(the group's output row) } else { None }
if new_out != emitted:
    if let Some(old) = emitted      -> out.update(old, -1)      // retract the old row
    if let Some(new) = &new_out     -> out.update(new.clone(), +1)  // emit the new row
    emitted = new_out
```

When `new_out == emitted`, **emit nothing** — this is not an optimisation, it is correctness: an extra `(-1, +1)` pair causes needless churn downstream, while a missing one is exactly the biggest bug source §6.2 describes.

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-core/src/agg.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggFn, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    fn sum_state() -> AggState {
        // The group key is column 0; the SUM is over column 1
        AggState::new(vec![0], vec![Agg { func: AggFn::Sum, column: Some(1) }])
    }

    fn count_state() -> AggState {
        AggState::new(vec![0], vec![Agg { func: AggFn::Count, column: None }])
    }

    #[test]
    fn a_changed_sum_emits_a_retraction_pair_not_a_bare_insert() {
        // spec §6.2: when a SUM goes from 100 to 150, what is emitted is (key,100) w=-1 and (key,150) w=+1,
        // not a lone +1 row. This is IVM's biggest source of bugs.
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));
        assert_eq!(first, ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ]),
            "the old output row must be retracted and the new one emitted"
        );
    }

    #[test]
    fn an_unchanged_group_emits_nothing() {
        // When a group is touched but its **output** does not change, not even a (-1,+1) pair should be emitted.
        //
        // The input must be two **different** rows (one in, one out), not +1/-1 of the same row:
        // the latter already cancels to an empty set inside `ZSet::from_rows`, `absorb` would never see
        // any input, so `touched` would be empty and the emission loop would not run once — the test would pass,
        // but for a reason unrelated to what it claims to guard, and the mutation "make `new_out != emitted`
        // always true" would not turn it red either.
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), int(2)]), 1),
        ]));
        // Replace one row in the group: the rows changed, but the group's row count did not, so COUNT's output is unchanged.
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(3)]), 1),
            (row(vec![txt("a"), int(1)]), -1),
        ]));
        assert!(d.is_empty(), "nothing may be emitted when a group is touched but its output is unchanged: {d:?}");
    }

    #[test]
    fn a_group_that_empties_is_retracted_and_not_replaced() {
        // spec §5.2: a grouped aggregate returns 0 rows over an empty table (unlike a global aggregate).
        // When a group's count reaches zero, only the retraction is emitted, no new row.
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]),
            "only a retraction, no new row"
        );
    }

    #[test]
    fn sum_over_only_null_inputs_is_null_not_zero() {
        // spec §6.1 (measured): when the group is non-empty but the column is all NULL, the group appears, COUNT(*) is positive,
        // and SUM is NULL. An implementation maintaining only the running sum would output 0, silently disagreeing with SQLite.
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), Value::Null]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)]),
            "SUM must be NULL, not Int(0)"
        );
    }

    #[test]
    fn sum_that_genuinely_totals_zero_is_int_zero_not_null() {
        // The counterpart of the previous test: with non-NULL inputs whose sum is exactly 0, it must be Int(0).
        // An implementation that only checks "is the sum 0" would output NULL here.
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), int(-5)]), 1),
        ]));
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(0)]), 1)]));
    }

    #[test]
    fn a_group_whose_last_non_null_input_leaves_falls_back_to_null() {
        // When the non-NULL inputs are all deleted but the group is still non-empty, SUM must go from Int back to NULL —
        // this path is only right if both sum and the non_null count are maintained.
        let mut s = sum_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), -1),
                (row(vec![txt("a"), Value::Null]), 1),
            ]),
            "the group is still there (the NULL row remains), but SUM falls back to NULL"
        );
    }

    #[test]
    fn groups_are_independent() {
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("b"), int(1)]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), -1),
                (row(vec![txt("a"), int(2)]), 1),
            ]),
            "only group a is affected; group b must not appear in the delta"
        );
    }

    #[test]
    fn a_null_group_key_is_a_group_like_any_other() {
        // NULL as a group key forms its own group in SQL GROUP BY (unlike WHERE's three-valued
        // logic). NULL is frequent in the differential-testing value domain, so this path is certain to be exercised.
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
        assert_eq!(d, ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
    }

    #[test]
    fn emitted_output_weight_is_always_one() {
        // spec §5.2: group key → exactly one output row; __w is always 1 in the final output.
        // Weights appear only in internal deltas and operator state.
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 5)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(5)]), 1)]),
            "an input weight of 5 becomes one row with COUNT=5, whose output weight is 1, not 5"
        );
    }
}
```

In the test module of `crates/ivmlite-core/src/node.rs`, **delete** `building_an_aggregate_is_an_error_until_task_4` and replace it with:

```rust
    #[test]
    fn aggregate_can_be_built_and_runs_through_the_tree() {
        // End to end: push one batch of deltas through the whole Scan → Filter → Project → Aggregate tree.
        let plan = crate::lower(
            &crate::ViewQuery {
                group_by: vec![0],
                aggs: vec![crate::Agg { func: crate::AggFn::Count, column: None }],
                predicate: Predicate::IntGt { column: 1, value: 3 },
            },
            "t",
            2,
        )
        .unwrap();
        let mut n = Node::build(&plan).unwrap();
        let d = ZSet::from_rows([
            (row(vec![Value::Text("a".into()), int(9)]), 1),
            (row(vec![Value::Text("a".into()), int(1)]), 1), // blocked by the Filter
        ]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "only the row that passes the predicate enters the count"
        );
    }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked agg`
Expected: a compile failure, `cannot find type AggState`

- [ ] **Step 3: Write the implementation**

At the start of `crates/ivmlite-core/src/agg.rs`:

```rust
use std::collections::BTreeMap;

use crate::{Agg, AggFn, Row, Value, ZSet};

/// One agg's accumulator.
///
/// `Sum` must maintain both `sum` and `non_null`: spec §6.1 says outright that an implementation maintaining only
/// the running sum outputs `0` when "the group is non-empty but the column is all NULL", while SQLite outputs `NULL`,
/// and that mismatch is silent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// One group's state.
#[derive(Debug, Clone, Default)]
struct Group {
    /// The sum of the weights of the group's rows. `COUNT(*)`'s output is exactly this; when it reaches zero the group disappears from the output.
    rows: i64,
    accs: Vec<Acc>,
    /// The row this group last emitted. spec §6.2: an aggregate must remember what it emitted
    /// in order to retract it — this is the real reason aggregation needs state.
    emitted: Option<Row>,
}

/// spec §6.2's aggregate operator state.
///
/// `BTreeMap` rather than `HashMap`: the order groups are walked in goes into the delta stream, and spec §9.4
/// requires a failing case to replay exactly from its seed.
#[derive(Debug, Clone)]
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    groups: BTreeMap<Row, Group>,
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState {
        AggState {
            group_by,
            aggs,
            groups: BTreeMap::new(),
        }
    }

    /// Absorb one batch of input deltas, returning the delta this operator emits **outward**.
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // First merge the batch's changes into the group state, noting which groups were touched; emission happens afterwards, all at once.
        // The two phases are necessary: the same group may be touched by several rows in one batch, and emitting row by row would emit
        // a chain of retraction pairs for intermediate states, whereas outside only the batch's net change should be seen.
        let mut touched: Vec<Row> = Vec::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            if !touched.contains(&key) {
                touched.push(key.clone());
            }
            let g = self
                .groups
                .entry(key)
                .or_insert_with(|| Group {
                    rows: 0,
                    accs: vec![Acc::default(); self.aggs.len()],
                    emitted: None,
                });
            g.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                let col = agg.column.expect("SUM must carry a column (lower has checked)");
                if let Value::Int(v) = row.get(col) {
                    g.accs[i].sum += v * w;
                    g.accs[i].non_null += w;
                }
                // A NULL input goes into neither sum nor non_null — this is where the contract "output NULL when
                // everything is NULL" lands at the state level.
            }
        }

        let mut out = ZSet::new();
        // The emission order comes from the BTreeMap's ordered walk rather than `touched`'s arrival order,
        // so the output delta's order does not depend on the arrangement of the input rows (spec §9.4).
        let mut keys: Vec<Row> = touched;
        keys.sort();
        for key in keys {
            let Some(g) = self.groups.get_mut(&key) else {
                continue;
            };
            let new_out = if g.rows > 0 {
                let mut vals: Vec<Value> = key.0.clone();
                for (i, agg) in self.aggs.iter().enumerate() {
                    vals.push(match agg.func {
                        AggFn::Count => Value::Int(g.rows),
                        AggFn::Sum => {
                            if g.accs[i].non_null == 0 {
                                Value::Null
                            } else {
                                Value::Int(g.accs[i].sum)
                            }
                        }
                    });
                }
                Some(Row::new(vals))
            } else {
                None
            };

            if new_out != g.emitted {
                if let Some(old) = &g.emitted {
                    out.update(old.clone(), -1);
                }
                if let Some(new) = &new_out {
                    out.update(new.clone(), 1);
                }
                g.emitted = new_out;
            }

            // spec §5.1: once a group is completely empty, no zombie state remains.
            if g.rows == 0 && g.emitted.is_none() {
                self.groups.remove(&key);
            }
        }
        out
    }
}
```

Add the variant to `crates/ivmlite-core/src/node.rs`'s `Node` enum, and one arm each to `build` and `delta`:

```rust
    Aggregate {
        input: Box<Node>,
        state: crate::AggState,
    },
```

```rust
            Plan::Aggregate { input, group_by, aggs } => Ok(Node::Aggregate {
                input: Box::new(Node::build(input)?),
                state: crate::AggState::new(group_by.clone(), aggs.clone()),
            }),
```

```rust
            Node::Aggregate { input: child, state } => {
                let upstream = child.delta(table, input);
                state.absorb(&upstream)
            }
```

`crates/ivmlite-core/src/lib.rs` gains:

```rust
pub mod agg;
pub use agg::AggState;
```

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes

- [ ] **Step 5: Mutation verification**

1. Delete `if let Some(old) = &g.emitted { out.update(old.clone(), -1); }` → expect `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert` and `a_group_that_empties_is_retracted_and_not_replaced` to go red
2. Make the emission condition `if new_out != g.emitted` always true → expect `an_unchanged_group_emits_nothing` to go red
3. Change the criterion of `if g.accs[i].non_null == 0 { Value::Null }` to `g.accs[i].sum == 0` → expect `sum_that_genuinely_totals_zero_is_int_zero_not_null` to go red
4. Delete the maintenance of the `non_null` field (the `g.accs[i].non_null += w` line) and make the criterion look only at `rows` → expect `sum_over_only_null_inputs_is_null_not_zero` to go red
5. Change `g.accs[i].sum += v * w` to `+= v` (ignoring the weight) → expect `emitted_output_weight_is_always_one` or `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert` to go red; record whichever actually goes red in the gate table
6. Change the weight in `out.update(new.clone(), 1)` to `g.rows` → expect `emitted_output_weight_is_always_one` to go red
7. Replace `groups`' `BTreeMap` with a `HashMap`, and delete `keys.sort()` → expect some multi-group test to go red. **This one is statistical**; run at least 10 processes to confirm, and say so honestly in the gate row
8. Delete `if g.rows == 0 && g.emitted.is_none() { self.groups.remove(&key); }` → expect it to **stay green** (zombie group state is unobservable, since its `emitted` is `None` and `rows` is 0, so it never emits anything again). Record it as "known unguarded", with the reason written out: it is memory reclamation rather than semantics, visible only in memory use over long sequences, and differential testing does not test memory

- [ ] **Step 6: Register the gates and commit**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/agg.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): the Aggregate operator and §6.2's retraction semantics"
```

---

## Task 5: The engine and the harness adapter — the differential harness runs a real engine for the first time

**Files:**
- Create: `crates/ivmlite-core/src/engine.rs`
- Create: `crates/ivmlite-test/src/incremental.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-test/src/lib.rs`
- Modify: `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `lower` (Task 1), `Node` (Tasks 3/4), `Database`, `ViewQuery`, `ZSet`
- Produces:
  - `pub struct IncrementalEngine`, with:
    - `pub fn new() -> IncrementalEngine`
    - `pub fn create_view(&mut self, db: &Database, query: &ViewQuery, initial: &BTreeMap<String, ZSet>) -> Result<(), EngineError>`
    - `pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>`
    - `pub fn refresh(&mut self) -> Result<(), EngineError>`
    - `pub fn materialize(&self) -> ZSet`
  - `pub struct EngineError(pub String)` (core's own, a different type from `ivmlite-test`'s type of the same name)
- On the `ivmlite-test` side: `impl crate::Engine for ivmlite_core::IncrementalEngine`

> The engine goes in `ivmlite-core` rather than `ivmlite-test`: it is the core product, and M1b's `ivmlite-sqlite` also consumes it. The `Engine` trait lives in `ivmlite-test`, and core must not depend on test (§4.2, and the reverse would create a cycle). The adapter is written on the test side — local trait + foreign type, which the orphan rule allows.

### This task's deliverable

**The differential testing harness runs green on a real incremental engine for the first time**: every enumerated query × biased update sequence × per-batch oracle comparison × batch independence. This is M1a's checkpoint itself (spec §11).

This task **does not do consolidation** — `refresh` pushes the raw Δ in pending through the operator tree one by one. Task 6 adds merging, and then consolidation's effect must be **observable**. Not doing it yet gives Task 6's gates a real "before" to compare against: if Task 5 had merged along the way, Task 6's mutations could not tell whether they really guard consolidation.

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-test/tests/harness_catches_bugs.rs`:

```rust
/// The M1a checkpoint (spec §11): the differential harness running green on a real incremental engine for the first time.
///
/// This test has the same structure as `naive_engine_is_green_across_many_seeds`,
/// with `IncrementalEngine` as the subject — it is incremental while the reference implementation recomputes
/// in full, and both must agree with the oracle at every refresh point.
#[test]
fn incremental_engine_is_green_across_the_enumerated_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate(&db.tables()[0]);
    let mut checked = 0usize;
    for seed in seed_range() {
        let query = &queries[seed as usize % queries.len()];
        let case = gen_case_with_query(seed, &db, &domain, query.clone(), 20, 60, Batching::Chunks(4));
        let mut engine = IncrementalEngine::new();
        if let Err(f) = run(&mut engine, &case) {
            panic!("the incremental engine disagrees with the oracle at seed={seed}: {f}");
        }
        checked += 1;
    }
    assert!(checked >= 50, "at least 50 seeds must run, ran {checked}");
}

/// The incremental engine must agree with full recomputation at **every refresh point**, not only at the final state.
/// TransientDriftEngine exists to prove the two are not the same thing (spec §9.1).
#[test]
fn incremental_engine_matches_naive_recompute_at_every_refresh_point() {
    let db = gen_database(2);
    let case = gen_case(11, &db, &Domain::default(), 25, 120, Batching::Chunks(5));

    let mut inc = IncrementalEngine::new();
    let mut naive = NaiveRecompute::new();
    let bases: BTreeMap<String, ZSet> = case
        .initial
        .iter()
        .map(|(t, rows)| (t.clone(), ZSet::from_rows(rows.iter().map(|r| (r.clone(), 1)))))
        .collect();
    inc.create_view(&case.database, &case.query, &bases).unwrap();
    naive.create_view(&case.database, &case.query, &bases).unwrap();
    assert_eq!(inc.materialize().unwrap(), naive.materialize().unwrap(), "they already disagree at bootstrap");

    for batch in case.batches() {
        for (table, raw) in &batch {
            inc.apply(table, raw).unwrap();
            naive.apply(table, raw).unwrap();
        }
        inc.refresh().unwrap();
        naive.refresh().unwrap();
        assert_eq!(
            inc.materialize().unwrap(),
            naive.materialize().unwrap(),
            "incremental maintenance and full recomputation diverge at a refresh point"
        );
    }
}

/// Spec §5.2's boundary check must take effect at create_view, not as a panic at refresh.
#[test]
fn create_view_rejects_a_global_aggregate() {
    let db = gen_database(1);
    let bad = ViewQuery {
        group_by: vec![],
        aggs: vec![Agg { func: AggFn::Count, column: None }],
        predicate: Predicate::None,
    };
    let mut engine = IncrementalEngine::new();
    let err = engine
        .create_view(&db, &bad, &BTreeMap::from([(db.tables()[0].table.clone(), ZSet::new())]))
        .expect_err("an empty group_by must be rejected at create_view");
    assert!(
        err.0.contains("GROUP BY") || err.0.contains("group_by"),
        "the error should name group_by: {}",
        err.0
    );
}
```

> **This task needs to add `case.batches()`.** Today `batches(ops, batching)` is
> a private free function in `differential.rs`, called inside `run`. The second test walks two engines
> side by side through the same sequence of batches, so the batching has to be available from outside. Add a method rather than making the free function
> `pub`: the batch split is a property of `TestCase`, the method form makes the call site read as "this case's
> batches", and it saves callers from pairing `ops` and `batching` themselves (pairing them wrong would not fail to compile,
> it would just silently run with the wrong batches).
>
> ```rust
> impl TestCase {
>     /// This case's batches, via the same code path `run` uses internally.
>     pub fn batches(&self) -> Vec<BTreeMap<String, Vec<(Row, i64)>>> {
>         batches(&self.ops, self.batching.clone())
>     }
> }
> ```
>
> Change `run` to call `case.batches()` internally; do not let the two paths batch separately — otherwise
> the "side-by-side comparison" might compare two different batch splits, and that mismatch would be caught by no test.

> **`gen_case_with_query` is a generator entry point this task adds** (`crates/ivmlite-test/src/differential.rs`), with signature `pub fn gen_case_with_query(seed: u64, db: &Database, domain: &Domain, query: ViewQuery, rows_per_table: usize, op_count: usize, batching: Batching) -> TestCase`. The existing `gen_case` picks a query from `enumerate` itself; covering the query space **one query at a time** by enumeration requires being able to specify the query. Implement `gen_case` as `gen_case_with_query(seed, db, domain, enumerate(&db.tables()[0])[seed as usize % n].clone(), ...)`, so the two share one code path.

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-test --locked incremental`
Expected: a compile failure, `cannot find type IncrementalEngine`

- [ ] **Step 3: Write the implementation**

`crates/ivmlite-core/src/engine.rs`:

```rust
use std::collections::BTreeMap;

use crate::{lower, Database, Node, Row, ViewQuery, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// v0's incremental engine.
///
/// `apply` only piles up pending, and only `refresh` advances the operator tree — spec §8.5 requires the two to be separate,
/// and §8.2 makes explicit refresh a permanent API rather than a temporary v0 compromise.
#[derive(Debug, Default)]
pub struct IncrementalEngine {
    tree: Option<Node>,
    /// The view's current materialised result. Deltas emitted by the operators are merged in here.
    view: ZSet,
    /// Raw Δ that has been ingested but not maintained yet, **unconsolidated** (§8.5).
    /// A `Vec` rather than a per-table map: the arrival order within a batch is kept until `refresh`,
    /// and whether to merge is `refresh`'s decision (Task 6).
    pending: Vec<(String, Row, i64)>,
}

impl IncrementalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        let anchor = db
            .tables()
            .first()
            .ok_or_else(|| EngineError("the Database must have at least one table".into()))?;
        let plan = lower(query, &anchor.table, anchor.arity()).map_err(|e| EngineError(e.0))?;
        let mut tree = Node::build(&plan).map_err(|e| EngineError(e.0))?;

        // bootstrap: push each table's initial state in as the first batch of deltas.
        // A declared table with no initial state is an error, not an empty table — the same convention as the oracle's
        // `missing_base_state_for_a_declared_table_is_an_error`.
        self.view = ZSet::new();
        for schema in db.tables() {
            let base = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!("table {} is declared but has no initial state", schema.table))
            })?;
            self.view.merge(&tree.delta(&schema.table, base));
        }
        self.tree = Some(tree);
        self.pending.clear();
        Ok(())
    }

    pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        if self.tree.is_none() {
            return Err(EngineError("apply was called before create_view".into()));
        }
        self.pending
            .extend(raw.iter().map(|(r, w)| (table.to_string(), r.clone(), *w)));
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<(), EngineError> {
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| EngineError("refresh was called before create_view".into()))?;
        // Task 6 inserts consolidation here. For now push one at a time: one row per batch.
        for (table, row, w) in std::mem::take(&mut self.pending) {
            let d = ZSet::from_rows([(row, w)]);
            self.view.merge(&tree.delta(&table, &d));
        }
        Ok(())
    }

    pub fn materialize(&self) -> ZSet {
        self.view.clone()
    }
}
```

`crates/ivmlite-test/src/incremental.rs`:

```rust
use std::collections::BTreeMap;

use ivmlite_core::{Database, IncrementalEngine, Row, ZSet};

use crate::{Engine, EngineError, ViewQuery};

/// Plug core's engine into the differential harness. Local trait + foreign type, which the orphan rule allows.
///
/// It only translates the error type: core must not depend on `ivmlite-test` (§4.2, and the reverse would create a cycle),
/// so each side has its own `EngineError`.
impl Engine for IncrementalEngine {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        IncrementalEngine::create_view(self, db, query, initial).map_err(|e| EngineError(e.0))
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        IncrementalEngine::apply(self, table, raw).map_err(|e| EngineError(e.0))
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        IncrementalEngine::refresh(self).map_err(|e| EngineError(e.0))
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        Ok(IncrementalEngine::materialize(self))
    }
}
```

Export them from `crates/ivmlite-core/src/lib.rs` and `crates/ivmlite-test/src/lib.rs` respectively.

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes. **If the differential tests go red, the engine really has a bug — investigate; do not loosen the assertions.** The failure message prints `IVMLITE_SEED=N` for exact replay.

- [ ] **Step 5: Mutation verification**

1. Change `create_view`'s bootstrap loop to handle only `db.tables()[0]` → expect it to **stay green** (registered in Phase 1: the query renders only the anchor table). This is exactly the engine-side counterpart of the three items on the join-landing checklist; **add a new row to the gate table pointing back at that checklist**, rather than registering it again as a separate gap
2. Change `std::mem::take` in `refresh` to `clone` (pending not cleared) → expect `incremental_engine_matches_naive_recompute_at_every_refresh_point` to go red (deltas applied repeatedly)
3. Make `apply` return `Ok(())` rather than an error when `tree.is_none()` → expect it to **stay green** (the harness never calls apply before create_view). Record it as "known unguarded", because what it guards is misuse rather than semantics
4. Make `materialize` return `ZSet::new()` → expect the differential tests to go red across the board
5. Stop propagating `lower`'s error (`.unwrap()` in `create_view`) → expect `create_view_rejects_a_global_aggregate` to go red (it becomes a panic rather than an Err)

- [ ] **Step 6: Register the gates and commit**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ docs/mutation-gates.md
git commit -m "feat: plug the incremental engine into the differential harness — the M1a checkpoint"
```

---

## Task 6: Delta consolidation

**Files:**
- Modify: `crates/ivmlite-core/src/engine.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces: `IncrementalEngine::rows_processed_last_refresh(&self) -> usize`

> Spec §8.2 lists consolidation as **part of M1, not a later optimisation**, and §8.5 calls it "this project's most likely performance story". §8.5 also points out that if the harness merged for the engine, consolidation would be **structurally invisible** at the seam — the test results would be the same whether the engine did it or not. `apply` taking raw Δ is exactly what makes it visible, and this task makes it **testable**.

### Why a counter is needed

Consolidation does not change the result, only the amount of work. So it is by nature unobservable through "is the output right" — exactly the structural invisibility §8.5 warns about.

The countermeasure is for the engine to expose a statistic: the number of rows the last `refresh` actually pushed through the operator tree. With merging in effect, a row appearing 5 times in one batch is pushed only once; a row whose `+1` and `-1` cancel is pushed 0 times. These two numbers are consolidation's only observable footprint.

This is not a test-only back door: write amplification is one of §11's M1 completion criteria ("write amplification has a definite number"), and this counter is where that number comes from.

- [ ] **Step 1: Write the failing tests**

At the end of `crates/ivmlite-core/src/engine.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, Value};

    fn db() -> Database {
        Database::new(vec![Schema {
            table: "t".into(),
            columns: vec![
                Column { name: "k".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "v".into(), ty: ColumnType::Integer, nullable: true },
            ],
        }])
    }

    fn query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        }
    }

    fn engine() -> IncrementalEngine {
        let mut e = IncrementalEngine::new();
        e.create_view(&db(), &query(), &BTreeMap::from([("t".to_string(), ZSet::new())]))
            .unwrap();
        e
    }

    fn row(k: &str, v: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(v)])
    }

    #[test]
    fn duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators() {
        // spec §8.2/§8.5: a row that appears 5 times in one batch is pushed only once after merging.
        // This is consolidation's only observable footprint — it does not change the result, only the amount of work.
        let mut e = engine();
        let raw: Vec<(Row, i64)> = (0..5).map(|_| (row("a", 1), 1)).collect();
        e.apply("t", &raw).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "5 identical raw Δ must be merged into 1 before reaching the operators"
        );
        assert_eq!(
            e.materialize().weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(5)])),
            1,
            "merging must not change the result: COUNT is still 5"
        );
    }

    #[test]
    fn rows_that_cancel_within_a_batch_never_reach_the_operators() {
        // Inserting and then deleting the same row in one batch gives a net weight of 0 after merging; it should never reach the operators.
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 0, "rows that cancel must not reach the operators");
        assert!(e.materialize().is_empty());
    }

    #[test]
    fn distinct_rows_are_not_over_merged() {
        // The reverse guard: merging must not merge different rows into one.
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("b", 1), 1), (row("a", 2), 1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 3, "the three rows are all distinct; not one should be merged away");
    }

    #[test]
    fn deltas_for_different_tables_are_consolidated_separately() {
        // When the same row value appears in two tables it must not be merged across tables — that would let one table's changes
        // cancel another table's. With a single table this shape does not exist; once join lands it is the norm.
        let two = Database::new(vec![
            Schema {
                table: "t".into(),
                columns: db().tables()[0].columns.clone(),
            },
            Schema {
                table: "u".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let mut e = IncrementalEngine::new();
        e.create_view(
            &two,
            &query(),
            &BTreeMap::from([("t".to_string(), ZSet::new()), ("u".to_string(), ZSet::new())]),
        )
        .unwrap();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.apply("u", &[(row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            2,
            "one row in each of the two tables; they must not cancel across tables"
        );
    }

    #[test]
    fn the_counter_resets_between_refreshes() {
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 1, "the count is for \"the last refresh\", not cumulative");
    }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked engine`
Expected: a compile failure, `no method named rows_processed_last_refresh`

- [ ] **Step 3: Write the implementation**

Add a field and a method to `IncrementalEngine`, and change `refresh` to merge first:

```rust
    /// The number of rows the last `refresh` actually pushed through the operator tree.
    ///
    /// Consolidation does not change the result, only the amount of work, so it cannot be observed through "is the output
    /// right" — spec §8.5 calls this situation structural invisibility. This counter is its only
    /// observable footprint, and the source of the "write amplification has a definite number" that §11 requires.
    rows_processed: usize,
```

```rust
    pub fn rows_processed_last_refresh(&self) -> usize {
        self.rows_processed
    }

    pub fn refresh(&mut self) -> Result<(), EngineError> {
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| EngineError("refresh was called before create_view".into()))?;

        // spec §8.2: raw Δ first has the weights of identical rows merged as a Z-set, then goes into the operators.
        // Merge per table — cancelling across tables when the same row value appears in two tables would be wrong.
        // `BTreeMap` rather than `HashMap`: the push order goes into the delta stream (§9.4).
        let mut by_table: BTreeMap<String, ZSet> = BTreeMap::new();
        for (table, row, w) in std::mem::take(&mut self.pending) {
            by_table.entry(table).or_default().update(row, w);
        }

        self.rows_processed = 0;
        for (table, delta) in &by_table {
            // Rows whose net weight is 0 after merging have already been deleted by `ZSet::update` (§5.1),
            // so they never appear here at all.
            self.rows_processed += delta.len();
            if delta.is_empty() {
                continue;
            }
            self.view.merge(&tree.delta(table, delta));
        }
        Ok(())
    }
```

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes, **including all of Task 5's differential tests** — consolidation must not change any result. That in itself is a necessary condition for its correctness.

- [ ] **Step 5: Mutation verification**

1. Remove the merging (back to Task 5's one-at-a-time push) → expect `duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators` and `rows_that_cancel_within_a_batch_never_reach_the_operators` to go red
2. Merge without grouping by table (everything into one `ZSet`) → expect `deltas_for_different_tables_are_consolidated_separately` to go red
3. Delete the `self.rows_processed = 0` line (making it cumulative) → expect `the_counter_resets_between_refreshes` to go red
4. Change `self.rows_processed += delta.len()` to `+= 1` (1 per table) → expect `distinct_rows_are_not_over_merged` to go red
5. Replace `by_table`'s `BTreeMap` with a `HashMap` → expect it to **stay green** (with a single table there is only one key; with several, the order in which operators are pushed per table is currently unobservable). **This row must be recorded as "known unguarded" and added to the join-landing checklist**: once join lands, the update order of the two sides' arrangements affects the `ΔR⋈ΔS` term, and it must then be re-run to confirm it turns red

- [ ] **Step 6: Register the gates and commit**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/engine.rs docs/mutation-gates.md
git commit -m "feat(core): delta consolidation — merge before reaching the operators"
```

---

## Self-Review

**1. Spec coverage**

| Spec requirement | Where it lands |
|---|---|
| §5.2 the plan IR's five operators | Task 1 (IR), Task 3 (Scan/Filter/Project), Task 4 (Aggregate). **Join is not in this plan** — Phase 3, for the reasons in the scope section at the start |
| §5.2 the root operator must be an Aggregate with a non-empty GROUP BY | Task 1's `lower` boundary check; it also changes the registered m6 gap from "n/a" to "verified" and strikes it from the join checklist |
| §5.2 no global aggregates | Task 1's `empty_group_by_is_rejected_at_the_boundary`; Task 5's `create_view_rejects_a_global_aggregate` guards the boundary's **entry point** |
| §6.1 linear operators Δ(f(R)) = f(ΔR) | Task 3 |
| §6.1 SUM over no non-NULL inputs returns NULL | Task 4, `sum_over_only_null_inputs_is_null_not_zero` paired with its converse `sum_that_genuinely_totals_zero_is_int_zero_not_null` |
| §6.1 three-valued logic | Task 3's `filter_treats_null_as_unknown_not_as_false_negation`; the comment says outright not to conclude that `NOT p == !p` |
| §6.1 integer overflow is undefined | Not implemented; guaranteed by the generator's narrow value domain (already guarded by `domain_cannot_overflow_integer_sum`). This plan adds no code, **on purpose**: the spec says outright that v0's countermeasure is clamping the value domain, not detection |
| §6.2 retraction pairs | Task 4, five tests each guarding one thing: a change emits a pair, no change emits nothing, emptying only retracts, NULL fallback, output weight always 1 |
| §6.3 Arrangement key → many values, object-safe | Task 2 |
| §8.2/§8.5 consolidation | Task 6, made observable by `rows_processed_last_refresh` |
| §8.5 apply/refresh separate, apply carries a table name, apply takes raw Δ | Task 5's adapter; none of the three signatures changes |
| §9.1 oracle comparison at every refresh point | Task 5 gets it via `run`; `incremental_engine_matches_naive_recompute_at_every_refresh_point` additionally compares point by point with the reference implementation |
| §9.4 deterministic order | Task 2 (`scan` ordered), Task 4 (groups emitted sorted by key), Task 6 (`by_table` uses `BTreeMap`) |
| §11 the M1a checkpoint "single-table engine runs green" | Task 5 |
| §11 benchmark, write-amplification number | **Not in this plan** — both of M0's control groups run in SQLite and are not comparable with a pure in-memory engine; Task 6's counter is the source of the write-amplification number, and real numbers wait for M1b |

**Known uncovered, on purpose**: in this plan `Arrangement` is built by Task 2 but **no operator uses it yet** — v0's Aggregate holds group state in a `BTreeMap`, which is enough, and `Arrangement`'s consumers are the two sides of join (Phase 3) and M1b's shadow-table implementation. This is an "implemented but no consumer" shape, exactly the kind this project keeps pushing down. **Task 2's gate rows must state this honestly**: `MemArrangement` is currently covered only by its own unit tests, and no operator consumes it; Phase 3's join is the first consumer, and all of Task 2's mutations must then be re-run to confirm they still go red. This also goes on the join-landing checklist.

> Why still do it now: spec §6.3 says outright that "M0 must not make any design decision that would force rework when join is added, and `Arrangement`'s key → many-values shape is the main place this constraint lands". Settling the trait's shape and validating it with an in-memory implementation costs one task; settling it in Phase 3 would tangle join's debugging with debugging the trait's shape.

**2. Placeholder scan**: no TBD / TODO. Every Step 3 gives complete compilable code. Task 3's `Node` enum gains the `Aggregate` variant in Task 4 — a deliberate increment: Task 3 has an explicit placeholder test, `building_an_aggregate_is_an_error_until_task_4`, stating the current state, and Task 4 explicitly requires deleting it and gives replacement tests.

**3. Type consistency**: `lower(&ViewQuery, &str, usize) -> Result<Plan, PlanError>` (Task 1) → `Node::build(&Plan) -> Result<Node, NodeError>` (Task 3) → `AggState::new(Vec<usize>, Vec<Agg>)` (Task 4) → `IncrementalEngine::create_view(&Database, &ViewQuery, &BTreeMap<String, ZSet>)` (Task 5). `ivmlite-core::EngineError` and `ivmlite-test::EngineError` share a name but are different types, and Task 5's adapter translates explicitly — as Task 5's Interfaces point out.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-21-m1a-phase2-engine.md`.
