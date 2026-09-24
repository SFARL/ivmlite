# M1b Phase 1: State Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put every piece of operator state behind `Arrangement`, addressed by a stable per-operator identity, so an operator tree can be rebuilt from its arrangements' contents alone — the precondition for M1b's shadow tables.

**Architecture:** `JoinSide` becomes `ArrangementId { node, role }`, where `node` is the operator's pre-order position in its `Plan` and `role` says which of the operator's arrangements it is. `AggState` stops keeping a private `BTreeMap<Row, Group>`: each group's accumulators are stored in an `Arrangement` passed in through the same provider (key = group key, value = the encoded accumulators, weight 1), and the row a group last emitted is derived from that state instead of being stored. A test double that mirrors every arrangement update lets the tests rebuild a tree from its arrangements and check that it continues exactly where the old tree left off.

**Tech Stack:** Rust 1.95, pure Rust with no `unsafe`, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md` (§4.4 State ownership and its M1a amendment; §6.2; §6.3; §7)

**Preceding plan:** `docs/superpowers/plans/2026-09-23-m1a-phase3-join.md` (complete and merged into master, fe834f0)

---

## This plan's scope and the rulings behind it

M1b turns the pure-Rust engine into a SQLite extension. It is split into phases:

1. **State ownership (this plan)** — pure Rust.
2. `ivmlite-sql` — `sqlparser-rs` → `Plan`, the Catalog, hard errors outside the v0 subset.
3. `ivmlite-sqlite` — the cdylib: control surface (§8.3), triggers, shadow tables, bootstrap watermark atomicity (§7.3), delta GC (§7.2).
4. The benchmark against the M0 baselines (§11's M1 completion criterion).

Spec §4.4's M1a amendment names the gap this plan closes: `Aggregate` keeps its state in a private map, so "there is no path to rebuild an operator from persisted state". Doing this now, in pure Rust, means the existing differential suite (216 tests, including 700-query join sweeps over two databases) guards the refactor completely before any SQLite code exists.

### Ruling 1: aggregate state is the accumulators; the emitted row is derived

A group's state is `rows` (the total weight of its rows) plus, per aggregate, `(sum, non_null)`. Today `AggState` also stores `emitted`, "the row this group last emitted". That field is a pure function of the accumulators: after every `absorb`, `emitted == output(accumulators)`. So the new code stores only the accumulators and recomputes the old output row from the **old** state when it needs to retract it. This is what makes a rebuilt tree correct: it retracts what the previous tree emitted without ever having seen that emission.

The stored value is a `Row` of `Value::Int`s in a fixed layout, `[rows, sum_0, non_null_0, sum_1, non_null_1, …]` — one `(sum, non_null)` pair per entry of `aggs`, in order, with `COUNT(*)`'s pair left at zero. A fixed layout is canonical: the same state always encodes to the same row (§7's requirement for the future BLOB encoding). Each group has exactly one value, with weight 1. A group whose state becomes all-zero is removed from the arrangement (§5.1, no zombie entries).

**One behaviour change, reachable only with illegal input:** today a group is dropped when `rows == 0` and it emits nothing, even if its sums are non-zero. The new code drops a group only when its whole state is zero. For legal input the two are identical — `rows == 0` means the group has no rows, so every sum and count over those rows is zero too.

### Ruling 2: arrangement identity is `(pre-order node index, role)`

`ArrangementId { node: usize, role: ArrangementRole }`. `node` numbers the `Plan`'s nodes in pre-order: the root is 0, and a node's children are numbered after it, left before right. `role` is `JoinLeft`, `JoinRight` or `AggregateGroups`. The same `Plan` always produces the same ids, which is what a provider loading `__ivm_state_<view>_<op>` tables (§7) needs. This also removes the M1a Phase 3 limitation recorded in spec §4.4: `JoinSide` could not tell apart two joins in one view, and `ArrangementId` can.

`lower` is deterministic, so the same view SQL always produces the same `Plan` and therefore the same ids. Changing `lower`'s output shape for an existing view would change its ids. That concern belongs to M1b Phase 3 (persisted views), not here.

### Ruling 3: `Arrangement` stays infallible in this phase

Spec §4.4 records that the trait's methods cannot fail and that the change to `Result` "is to be decided when M1b writes the shadow-table implementation and its failure modes are known". That condition still holds after this plan. The shape of a fallible trait depends on things only the SQLite implementation will show:
- whether errors are per call or per row of a cursor;
- whether `get` can stay a lazy `Box<dyn Iterator>` given a `rusqlite::Statement`'s borrow;
- what the engine must do with its in-memory state after a failed `refresh`.

Choosing a signature now would be a guess, and every operator would have to be rewritten against it. So this plan keeps the trait unchanged and moves the decision to Phase 3, where the spec amendment in Task 3 names these three questions explicitly. The one new place that could meet corrupt state — decoding an aggregate's stored value — panics with a clear message and a test. Phase 3 turns that into an error.

### Out of scope

- Persisting `IncrementalEngine`'s materialized view (`view: ZSet`) — that is `__ivm_out_<view>` in Phase 3.
- An engine constructor that accepts a provider or resumes from persisted state — Phase 3, together with the watermarks it needs. This plan proves resumability at the operator-tree level.
- Changing the `Arrangement` trait (Ruling 3).
- The three-column schema decision (spec §9.2) — still open.

### Also in this plan

The three documentation issues parked by M1a Phase 3's final review (a stale sentence in spec §9.2, a misleading doc comment in `differential.rs`, a pre-join number in `data.rs`), and the docs index.

---

## Global Constraints

- **§4.2 `ivmlite-core` must not depend on `rusqlite`**, nor on `ivmlite-test`. No new dependency. Not one line of `unsafe`.
- **§5.1** a row or state entry whose weight reaches zero is removed; no zombie entries.
- **§6.1** `SUM` over zero non-NULL inputs is `NULL`, not `0`; state keeps both the running sum and the non-NULL count.
- **§6.2** aggregates emit retraction pairs: when a group's output changes from `old` to `new`, emit `old` with weight −1 and `new` with weight +1; emit nothing when the output is unchanged.
- **§6.3** `Arrangement`'s signature does not change in this plan (Ruling 3).
- **§9.4** iteration order is deterministic: `Vec` / `BTreeMap` for anything that can affect output, never `HashMap` / `HashSet`.
- **The refactor must not weaken any existing test.** Existing tests may change only at constructor call sites and in comments that describe code this plan replaces; no assertion is removed or loosened. The whole suite (currently 216) must stay green after every task.
- **Mutation gates**: every spec-mandated behaviour has a row in `docs/mutation-gates.md`, and "verified" comes only from actually running the mutation: break → confirm `cargo build --workspace --all-targets --locked` compiles → `cargo test --workspace --locked --no-fail-fast` → sum the failures over **all** test binaries → restore → confirm green and a clean `git status`. Record `N passed / M failed (baseline B/0)` exactly as measured; an `n/a` row's test column starts with `None — `. Run `python3 scripts/count-mutation-gates.py --fix`, then bare, and confirm it prints `consistent`.
- **Record measurements, never computed numbers**; label anything deduced or extrapolated as such.
- Before every commit: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- **English only** in code, comments, error strings, docs and commit messages.
- Stage specific files, never `git add -A`; never `--no-verify`. End every commit message with:

```
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## File structure

| File | Responsibility |
|---|---|
| `crates/ivmlite-core/src/arrangement.rs` (modified) | Adds `ArrangementId`, `ArrangementRole`; `fresh_mem_arrangement` moves here from `join.rs` |
| `crates/ivmlite-core/src/join.rs` (modified) | `JoinSide` and `fresh_mem_arrangement` removed; tests build arrangements directly |
| `crates/ivmlite-core/src/node.rs` (modified) | `Node::build` numbers nodes in pre-order and asks the provider by `ArrangementId`; `Aggregate` gets its arrangement from the provider |
| `crates/ivmlite-core/src/agg.rs` (rewritten, non-test part) | `AggState` over a `Box<dyn Arrangement>`; `GroupState` encoding |
| `crates/ivmlite-core/src/test_support.rs` (new, `#[cfg(test)]`) | `Mirrors`: an arrangement provider that records every update, so a tree can be rebuilt from its arrangements; shared two-table test fixtures |
| `crates/ivmlite-core/src/engine.rs` (modified) | Import changes only; its join test helpers move to `test_support.rs` |
| `crates/ivmlite-core/src/lib.rs` (modified) | Exports |
| `docs/mutation-gates.md`, the spec, `docs/README.md`, two `ivmlite-test` doc comments (modified) | Gate rows, spec amendments, parked documentation fixes |

---

## Task 1: `ArrangementId` — a stable identity for every arrangement

**Files:**
- Modify: `crates/ivmlite-core/src/arrangement.rs`, `crates/ivmlite-core/src/join.rs`, `crates/ivmlite-core/src/node.rs`, `crates/ivmlite-core/src/engine.rs` (the `use` line only), `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `Node::build(&Plan, &mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>)`, `JoinSide`, `fresh_mem_arrangement(JoinSide)` (current code)
- Produces:
  - `pub struct ArrangementId { pub node: usize, pub role: ArrangementRole }` and `pub enum ArrangementRole { JoinLeft, JoinRight }`, both deriving `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
  - `pub fn fresh_mem_arrangement(id: ArrangementId) -> Box<dyn Arrangement>` in `arrangement.rs`
  - `Node::build(plan: &Plan, arrangements: &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>) -> Node`
  - `JoinSide` no longer exists

- [ ] **Step 1: Write the failing tests**

In `crates/ivmlite-core/src/node.rs`'s test module, replace `build_asks_for_one_arrangement_per_side` with the tests below, and add the plan helpers. `join_plan()` already exists (a `Join` of `Scan t0` and `Scan t1`, keys 0 and 0) and is reused.

```rust
    fn scan(table: &str) -> Plan {
        Plan::Scan {
            table: table.into(),
            columns: vec![0, 1],
        }
    }

    fn count_agg() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    /// Records every id `Node::build` asks its provider for, in order.
    fn ids_asked_for(plan: &Plan) -> Vec<crate::ArrangementId> {
        let mut asked = Vec::new();
        let _ = Node::build(plan, &mut |id| {
            asked.push(id);
            crate::fresh_mem_arrangement(id)
        });
        asked
    }

    fn id(node: usize, role: crate::ArrangementRole) -> crate::ArrangementId {
        crate::ArrangementId { node, role }
    }

    #[test]
    fn build_asks_for_each_join_input_by_node_and_role() {
        use crate::ArrangementRole::{JoinLeft, JoinRight};
        assert_eq!(ids_asked_for(&join_plan()), vec![id(0, JoinLeft), id(0, JoinRight)]);
    }

    #[test]
    fn node_ids_number_the_plan_in_pre_order() {
        // Aggregate(0) → Project(1) → Join(2) → Scan(3), Scan(4): a provider
        // loading persisted state finds an operator's tables by this number,
        // so it must depend only on the plan's shape.
        use crate::ArrangementRole::{JoinLeft, JoinRight};
        let plan = Plan::Aggregate {
            input: Box::new(Plan::Project {
                input: Box::new(join_plan()),
                columns: vec![0],
            }),
            group_by: vec![0],
            aggs: vec![count_agg()],
        };
        assert_eq!(ids_asked_for(&plan), vec![id(2, JoinLeft), id(2, JoinRight)]);

        // A Filter is a node too: it shifts the join to position 3.
        let filtered = Plan::Aggregate {
            input: Box::new(Plan::Project {
                input: Box::new(Plan::Filter {
                    input: Box::new(join_plan()),
                    predicate: Predicate::IsNotNull { column: 1 },
                }),
                columns: vec![0],
            }),
            group_by: vec![0],
            aggs: vec![count_agg()],
        };
        assert_eq!(ids_asked_for(&filtered), vec![id(3, JoinLeft), id(3, JoinRight)]);
    }

    #[test]
    fn two_joins_in_one_plan_get_distinct_ids() {
        // `JoinSide` could not tell two joins in one view apart (spec §4.4,
        // M1a Phase 3 amendment); the node index can. Join(0) over Join(1)
        // and Scan t2.
        use crate::ArrangementRole::{JoinLeft, JoinRight};
        let plan = Plan::Join {
            left: Box::new(join_plan()),
            right: Box::new(scan("t2")),
            left_key: 0,
            right_key: 0,
        };
        assert_eq!(
            ids_asked_for(&plan),
            vec![id(1, JoinLeft), id(1, JoinRight), id(0, JoinLeft), id(0, JoinRight)]
        );
    }
```

`Agg` and `AggFn` are already imported in node.rs's test module; add them if the compiler says otherwise.

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked`
Expected: a compile failure — `cannot find type ArrangementId`.

- [ ] **Step 3: Write the implementation**

In `crates/ivmlite-core/src/arrangement.rs`, after the `Arrangement` trait:

```rust
/// Which operator an arrangement belongs to, and which of its states it holds.
///
/// `node` is the operator's position in its `Plan` in pre-order: the root is
/// 0, and a node's children are numbered after it, left before right. The same
/// plan therefore always yields the same ids — what a provider that loads
/// persisted state (§7's `__ivm_state_<view>_<op>`) needs to find the right
/// table. `role` tells apart the arrangements of one operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArrangementId {
    pub node: usize,
    pub role: ArrangementRole,
}

/// The state an arrangement holds for its operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArrangementRole {
    /// A join's left input, keyed by the join key.
    JoinLeft,
    /// A join's right input, keyed by the join key.
    JoinRight,
}

/// The provider the engine passes to `Node::build` today: every arrangement
/// starts empty, in memory.
pub fn fresh_mem_arrangement(_id: ArrangementId) -> Box<dyn Arrangement> {
    Box::new(MemArrangement::new())
}
```

In `crates/ivmlite-core/src/join.rs`: delete `JoinSide` and `fresh_mem_arrangement`, change the import line to `use crate::{Arrangement, Row, Value, ZSet};`, and replace the reference to "(M1a Phase 3, Ruling 2)" in `JoinState`'s doc if it mentions `JoinSide`. In its test module, replace every `fresh_mem_arrangement(JoinSide::Left)` / `fresh_mem_arrangement(JoinSide::Right)` with `Box::new(MemArrangement::new())` (add `MemArrangement` to the test module's imports if it is not already there).

In `crates/ivmlite-core/src/node.rs`, change the import line to `use crate::{Arrangement, ArrangementId, ArrangementRole, JoinState, Plan, Predicate, Row, Value, ZSet};` and replace `build` with a public entry point plus a numbering recursion. The `Filter`, `Project` and `Aggregate` arms are unchanged except that they recurse through `build_at`:

```rust
    /// Build the operator tree from a `Plan`, recursively.
    ///
    /// (Keep the existing paragraph about `NodeError` / Finding H here.)
    ///
    /// `arrangements` supplies every arrangement an operator needs, asked for
    /// by `ArrangementId`: the operator's pre-order position in `plan` and the
    /// arrangement's role (M1b Phase 1, Ruling 2). The engine passes
    /// `fresh_mem_arrangement`; a provider that returns non-empty arrangements
    /// rebuilds a tree from persisted state.
    pub fn build(
        plan: &Plan,
        arrangements: &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>,
    ) -> Node {
        let mut next = 0;
        Node::build_at(plan, &mut next, arrangements)
    }

    /// `build`'s recursion: `next` is the pre-order index the next plan node gets.
    fn build_at(
        plan: &Plan,
        next: &mut usize,
        arrangements: &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>,
    ) -> Node {
        let node = *next;
        *next += 1;
        match plan {
            Plan::Scan { table, .. } => Node::Scan {
                table: table.clone(),
            },
            Plan::Join {
                left,
                right,
                left_key,
                right_key,
            } => {
                let left = Node::build_at(left, next, arrangements);
                let right = Node::build_at(right, next, arrangements);
                Node::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    state: JoinState::new(
                        *left_key,
                        *right_key,
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinLeft,
                        }),
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinRight,
                        }),
                    ),
                }
            }
            Plan::Filter { input, predicate } => Node::Filter {
                input: Box::new(Node::build_at(input, next, arrangements)),
                predicate: predicate.clone(),
            },
            Plan::Project { input, columns } => Node::Project {
                input: Box::new(Node::build_at(input, next, arrangements)),
                columns: columns.clone(),
            },
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => Node::Aggregate {
                input: Box::new(Node::build_at(input, next, arrangements)),
                state: crate::AggState::new(group_by.clone(), aggs.clone()),
            },
        }
    }
```

In `crates/ivmlite-core/src/lib.rs`: change `pub use arrangement::{Arrangement, MemArrangement};` to `pub use arrangement::{fresh_mem_arrangement, Arrangement, ArrangementId, ArrangementRole, MemArrangement};`, and `pub use join::{fresh_mem_arrangement, JoinSide, JoinState};` to `pub use join::JoinState;`. In `engine.rs`, `fresh_mem_arrangement` keeps its name, so only confirm the `use crate::{...}` line still resolves.

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes.

- [ ] **Step 5: Mutation verification**

1. Delete `*next += 1;` in `build_at` (every node gets index 0) → expect `node_ids_number_the_plan_in_pre_order` and `two_joins_in_one_plan_get_distinct_ids` to go red
2. In the `Plan::Join` arm, ask for `ArrangementRole::JoinLeft` for both arrangements → expect `build_asks_for_each_join_input_by_node_and_role` (and the other two id tests) to go red
3. In the `Plan::Join` arm, build `right` before `left` (swap the two `build_at` lines) → expect `two_joins_in_one_plan_get_distinct_ids` to go red, because the inner join would then be numbered after the right scan. Record what the run shows; if it stays green, explain why in the row.

- [ ] **Step 6: Register the gates and commit**

The gate table has a row for M1a Phase 3 Ruling 2's node-level form ("`Node::build`'s `Plan::Join` arm must call the a…", around line 98) whose mutation names `JoinSide`. Rewrite that row in terms of `ArrangementRole` and `build_asks_for_each_join_input_by_node_and_role`, and re-run its mutation. Add the three rows above (spec requirement: §7's per-operator `__ivm_state_<view>_<op>` needs a stable operator identity; Ruling 2 of this plan). `grep -n JoinSide docs/mutation-gates.md crates` must print nothing afterwards. Do not edit the spec here; Task 3 does.

```bash
git add crates/ivmlite-core/src/arrangement.rs crates/ivmlite-core/src/join.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): identify arrangements by operator position and role

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 2: Aggregate state on `Arrangement`, and rebuilding a tree from its arrangements

**Files:**
- Modify: `crates/ivmlite-core/src/agg.rs` (the non-test part is rewritten; tests adapted and extended)
- Modify: `crates/ivmlite-core/src/arrangement.rs` (adds `ArrangementRole::AggregateGroups`)
- Modify: `crates/ivmlite-core/src/node.rs` (the `Aggregate` arm and tests)
- Create: `crates/ivmlite-core/src/test_support.rs`
- Modify: `crates/ivmlite-core/src/engine.rs` (its join test helpers move to `test_support.rs`)
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ArrangementId`, `ArrangementRole`, `Node::build(&Plan, &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>)` (Task 1)
- Produces:
  - `ArrangementRole::AggregateGroups`
  - `AggState::new(group_by: Vec<usize>, aggs: Vec<Agg>, state: Box<dyn Arrangement>) -> AggState` (was two arguments)
  - `crate::test_support::Mirrors` (test-only): `Mirrors::default()`, `fn arrangement(&self, id: ArrangementId) -> Box<dyn Arrangement>`, `fn snapshot(&self, id: ArrangementId) -> MemArrangement`
  - `crate::test_support::{kv, kv_row, join_on_k}` (test-only; moved from `engine.rs`'s tests)

> **Why a mirror, not an accessor on `AggState` / `JoinState`:** the claim under test is "an operator's arrangements are its whole state". Reading them back through the operator would test the operator's own view of itself. Mirroring every `update` at the provider boundary records exactly what a SQLite-backed provider would have written to its tables, and nothing else.

- [ ] **Step 1: The test double and shared fixtures**

Create `crates/ivmlite-core/src/test_support.rs`:

```rust
//! Test-only helpers for ivmlite-core's unit tests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::{
    Agg, AggFn, Arrangement, ArrangementId, Column, ColumnType, Join, MemArrangement,
    Predicate, Row, Schema, Value, ViewQuery,
};

/// An arrangement provider that records every update, by `ArrangementId`.
///
/// Each arrangement it hands out behaves like a fresh `MemArrangement` and
/// mirrors every `update` into a shared record — exactly what a provider backed
/// by `__ivm_state_<view>_<op>` tables would have written. `snapshot` then
/// returns a copy of one arrangement's contents, so a test can build a second
/// tree from nothing but those contents.
#[derive(Clone, Default)]
pub(crate) struct Mirrors(Rc<RefCell<BTreeMap<ArrangementId, MemArrangement>>>);

impl Mirrors {
    pub(crate) fn arrangement(&self, id: ArrangementId) -> Box<dyn Arrangement> {
        Box::new(Mirrored {
            id,
            inner: MemArrangement::new(),
            mirrors: self.clone(),
        })
    }

    /// A copy of what the arrangement `id` holds now; empty if it was never updated.
    pub(crate) fn snapshot(&self, id: ArrangementId) -> MemArrangement {
        self.0.borrow().get(&id).cloned().unwrap_or_default()
    }
}

struct Mirrored {
    id: ArrangementId,
    inner: MemArrangement,
    mirrors: Mirrors,
}

impl Arrangement for Mirrored {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_> {
        self.inner.get(key)
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) {
        self.inner.update(key, val, weight_delta);
        self.mirrors
            .0
            .borrow_mut()
            .entry(self.id)
            .or_default()
            .update(key, val, weight_delta);
    }

    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_> {
        self.inner.scan()
    }
}
```

Then **move** `kv`, `kv_row` and `join_on_k` from `engine.rs`'s test module into this file, unchanged apart from `pub(crate)`, and make `engine.rs`'s tests `use crate::test_support::{join_on_k, kv, kv_row};`. Adjust the `use` line above to exactly what the moved helpers need (clippy's `unused_imports` is an error). In `lib.rs`, add:

```rust
#[cfg(test)]
mod test_support;
```

- [ ] **Step 2: Write the failing tests**

In `agg.rs`'s test module, change `sum_state()`, `count_state()` and the inline `AggState::new(...)` in `each_agg_reads_its_own_accumulator` to pass a third argument, `Box::new(MemArrangement::new())`. Add `MemArrangement` to the test module's imports. No assertion changes. Then add:

```rust
    use crate::test_support::Mirrors;
    use crate::{ArrangementId, ArrangementRole};

    fn groups_id() -> ArrangementId {
        ArrangementId {
            node: 0,
            role: ArrangementRole::AggregateGroups,
        }
    }

    fn sum_state_on(state: Box<dyn Arrangement>) -> AggState {
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
            state,
        )
    }

    #[test]
    fn a_state_rebuilt_from_its_arrangement_retracts_what_the_old_one_emitted() {
        // The old state emits (a, 100). A new AggState built from nothing but
        // the arrangement's contents must, on the next batch, retract (a, 100)
        // — a row it never emitted itself. This works only if the emitted row
        // is derived from the stored accumulators rather than kept on the side
        // (the plan's Ruling 1).
        let mirrors = Mirrors::default();
        let mut old = sum_state_on(mirrors.arrangement(groups_id()));
        old.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let mut rebuilt = sum_state_on(Box::new(mirrors.snapshot(groups_id())));
        let d = rebuilt.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ])
        );
    }

    #[test]
    fn the_stored_state_is_one_value_per_group() {
        // Each group's state is a single value of weight 1: updating a group
        // must retract its old value, not add a second one.
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(2)]), 1)]));
        let stored: Vec<(Row, Row, i64)> = mirrors.snapshot(groups_id()).scan().collect();
        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(stored[0].0, row(vec![txt("a")]));
        assert_eq!(stored[0].2, 1);
    }

    #[test]
    fn a_group_that_empties_leaves_no_state_behind() {
        // Spec §5.1: no zombie entries. A group whose rows are all deleted must
        // disappear from the arrangement (and from the future shadow table).
        let mirrors = Mirrors::default();
        let mut s = sum_state_on(mirrors.arrangement(groups_id()));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(mirrors.snapshot(groups_id()).scan().count(), 0);
    }

    #[test]
    #[should_panic(expected = "corrupted aggregate state")]
    fn a_group_with_two_stored_values_is_reported_as_corrupted() {
        // In memory this cannot happen; from a persisted table it can (M1b
        // Phase 3 turns this panic into an error, per the plan's Ruling 3).
        let mut state = MemArrangement::new();
        let key = row(vec![txt("a")]);
        state.update(&key, &row(vec![int(1), int(5), int(1)]), 1);
        state.update(&key, &row(vec![int(1), int(6), int(1)]), 1);
        let mut s = sum_state_on(Box::new(state));
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
    }
```

In `node.rs`'s test module, add (and update `node_ids_number_the_plan_in_pre_order`: its first expected list gains `id(0, AggregateGroups)` after the join's two ids, and its second list gains the same; import `ArrangementRole::AggregateGroups`):

```rust
    #[test]
    fn a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off() {
        // The whole operator tree of a join view — the join's two sides and
        // the aggregate's groups — rebuilt from nothing but its arrangements'
        // contents, must produce exactly the old tree's next delta, including
        // the retraction of a row only the old tree emitted.
        use crate::test_support::{join_on_k, kv, kv_row, Mirrors};
        let db = crate::Database::new(vec![kv("t0"), kv("t1")]);
        let plan = crate::lower(&join_on_k(), &db).expect("a legal join query must lower");

        let mirrors = Mirrors::default();
        let mut old = Node::build(&plan, &mut |id| mirrors.arrangement(id));
        old.delta("t0", &ZSet::from_rows([(kv_row("a", 1), 1)]));
        old.delta("t1", &ZSet::from_rows([(kv_row("a", 10), 1), (kv_row("a", 20), 1)]));

        let mut rebuilt = Node::build(&plan, &mut |id| -> Box<dyn crate::Arrangement> {
            Box::new(mirrors.snapshot(id))
        });
        let next = ZSet::from_rows([(kv_row("a", 5), 1)]);
        let from_rebuilt = rebuilt.delta("t1", &next);
        let from_old = old.delta("t1", &next);

        let out = |count: i64, sum: i64| {
            Row::new(vec![Value::Text("a".into()), Value::Int(count), Value::Int(sum)])
        };
        assert_eq!(from_old, ZSet::from_rows([(out(2, 30), -1), (out(3, 35), 1)]));
        assert_eq!(from_rebuilt, from_old);
    }
```

(`join_on_k()` groups by `t0.k` with `COUNT(*)` and `SUM(t1.v)`, joined on `k = k` — the query `engine.rs`'s join tests already use.)

- [ ] **Step 3: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked`
Expected: a compile failure — `AggState::new` takes 2 arguments / `no variant AggregateGroups`.

- [ ] **Step 4: Write the implementation**

In `arrangement.rs`, add the variant to `ArrangementRole`:

```rust
    /// An aggregate's per-group state, keyed by the group key; see `AggState`.
    AggregateGroups,
```

In `node.rs`, the `Aggregate` arm of `build_at` becomes:

```rust
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => {
                let input = Node::build_at(input, next, arrangements);
                Node::Aggregate {
                    input: Box::new(input),
                    state: crate::AggState::new(
                        group_by.clone(),
                        aggs.clone(),
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::AggregateGroups,
                        }),
                    ),
                }
            }
```

Replace everything in `agg.rs` above `#[cfg(test)]` with:

```rust
use std::collections::BTreeMap;

use crate::{Agg, AggFn, Arrangement, Row, Value, ZSet};

/// One agg's accumulator.
///
/// `Sum` must keep both `sum` and `non_null`: spec §6.1 states that an
/// implementation keeping only the running sum outputs `0` when "the group is
/// non-empty but the column is entirely NULL", where SQLite outputs `NULL` — and
/// that the disagreement is silent. `COUNT(*)` needs no accumulator (it is the
/// group's `rows`); its pair stays zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// Everything the operator knows about one group.
///
/// The row the group last emitted is **not** part of it: that row is a pure
/// function of this state (`output`), so it is recomputed from the old state
/// whenever it has to be retracted. That is what lets a tree rebuilt from its
/// arrangements retract what the previous tree emitted (M1b Phase 1, Ruling 1).
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupState {
    /// The total weight of the group's rows; `COUNT(*)` outputs exactly this.
    rows: i64,
    /// One accumulator per entry of `aggs`, in order.
    accs: Vec<Acc>,
}

impl GroupState {
    fn empty(aggs: usize) -> Self {
        GroupState {
            rows: 0,
            accs: vec![Acc::default(); aggs],
        }
    }

    fn is_empty(&self) -> bool {
        self.rows == 0 && self.accs.iter().all(|a| *a == Acc::default())
    }

    fn plus(&self, delta: &GroupState) -> GroupState {
        GroupState {
            rows: self.rows + delta.rows,
            accs: self
                .accs
                .iter()
                .zip(&delta.accs)
                .map(|(a, d)| Acc {
                    sum: a.sum + d.sum,
                    non_null: a.non_null + d.non_null,
                })
                .collect(),
        }
    }

    /// The stored form: `[rows, sum_0, non_null_0, sum_1, non_null_1, …]`, all
    /// `Value::Int`. The layout is fixed, so the encoding is canonical — the
    /// same state always encodes to the same row (spec §7).
    fn encode(&self) -> Row {
        let mut values = Vec::with_capacity(1 + 2 * self.accs.len());
        values.push(Value::Int(self.rows));
        for acc in &self.accs {
            values.push(Value::Int(acc.sum));
            values.push(Value::Int(acc.non_null));
        }
        Row::new(values)
    }

    /// The inverse of `encode`.
    ///
    /// # Panics
    /// If `row` does not have `encode`'s layout for `aggs` accumulators. The
    /// in-memory engine only ever stores rows `encode` produced; a state loaded
    /// from a persisted table may not be, and M1b Phase 3 turns this panic into
    /// an error (the plan's Ruling 3).
    fn decode(row: &Row, aggs: usize) -> GroupState {
        assert_eq!(
            row.len(),
            1 + 2 * aggs,
            "corrupted aggregate state: expected {} values, found {row:?}",
            1 + 2 * aggs
        );
        let int = |i: usize| match row.get(i) {
            Value::Int(n) => *n,
            other => panic!("corrupted aggregate state: value {i} is {other:?}, not an Int"),
        };
        GroupState {
            rows: int(0),
            accs: (0..aggs)
                .map(|i| Acc {
                    sum: int(1 + 2 * i),
                    non_null: int(2 + 2 * i),
                })
                .collect(),
        }
    }

    /// The row the group emits in this state, or `None` when it has no rows.
    fn output(&self, key: &Row, aggs: &[Agg]) -> Option<Row> {
        if self.rows <= 0 {
            return None;
        }
        let mut values: Vec<Value> = key.0.clone();
        for (acc, agg) in self.accs.iter().zip(aggs) {
            values.push(match agg.func {
                AggFn::Count => Value::Int(self.rows),
                AggFn::Sum if acc.non_null == 0 => Value::Null,
                AggFn::Sum => Value::Int(acc.sum),
            });
        }
        Some(Row::new(values))
    }
}

/// Spec §6.2's aggregate operator.
///
/// Its whole state lives in `state`, an `Arrangement` keyed by group key, whose
/// value for a group is that group's `GroupState::encode` with weight 1 — the
/// arrangement a SQLite provider will back with `__ivm_state_<view>_<op>`
/// (spec §7). The arrangement is passed in rather than created here, so a tree
/// can be rebuilt from persisted state (M1b Phase 1).
///
/// **The state must survive between batches**: the second batch can retract
/// the row the first one emitted only because the first batch's accumulators
/// are still in `state`. Pinned by
/// `aggregate_retracts_across_two_batches_through_the_same_node` in `node.rs`
/// and by `a_state_rebuilt_from_its_arrangement_retracts_what_the_old_one_emitted`.
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    state: Box<dyn Arrangement>,
}

impl std::fmt::Debug for AggState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Arrangement` is not `Debug` (see `JoinState`'s impl).
        f.debug_struct("AggState")
            .field("group_by", &self.group_by)
            .field("aggs", &self.aggs)
            .finish_non_exhaustive()
    }
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>, state: Box<dyn Arrangement>) -> AggState {
        AggState {
            group_by,
            aggs,
            state,
        }
    }

    /// Absorb one batch of input deltas, returning the delta this operator **emits**.
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // Phase one: fold the whole batch into one delta per touched group.
        // Emitting per input row would send out retraction pairs for
        // intermediate states, when only the batch's net change should be
        // visible. A `BTreeMap` rather than a `Vec` with a linear search keeps
        // a batch touching N groups at O(N log N) (external review P2-2), and
        // yields the groups in key order for the emit loop.
        let mut deltas: BTreeMap<Row, GroupState> = BTreeMap::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            let d = deltas
                .entry(key)
                .or_insert_with(|| GroupState::empty(self.aggs.len()));
            d.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                // `lower` rejects a SUM without a column
                // (`sum_without_a_column_is_rejected_at_the_boundary`), so a
                // tree built through `Node::build` never reaches this `expect`.
                let col = agg
                    .column
                    .expect("SUM must have a column; a tree built through lower was checked at the boundary");
                // A NULL input goes into neither sum nor non_null — where the
                // "all NULL outputs NULL" contract lands in the state.
                if let Value::Int(v) = row.get(col) {
                    d.accs[i].sum += v * w;
                    d.accs[i].non_null += w;
                }
            }
        }

        // Phase two: for each touched group, compare the output of its old
        // state with that of its new state, emit the difference, and store the
        // new state. Emitting nothing when the output is unchanged is not
        // observable in the returned `ZSet` (an equal -1/+1 pair cancels in
        // `ZSet::update`); see the matching n/a row in docs/mutation-gates.md.
        let mut out = ZSet::new();
        for (key, delta) in deltas {
            let old = self.load(&key);
            let new = old.plus(&delta);
            let old_out = old.output(&key, &self.aggs);
            let new_out = new.output(&key, &self.aggs);
            if new_out != old_out {
                if let Some(row) = old_out {
                    out.update(row, -1);
                }
                if let Some(row) = new_out {
                    out.update(row, 1);
                }
            }
            self.store(&key, &old, &new);
        }
        out
    }

    /// The group's stored state, or the empty state if it has none.
    fn load(&self, key: &Row) -> GroupState {
        let mut values = self.state.get(key);
        match (values.next(), values.next()) {
            (None, _) => GroupState::empty(self.aggs.len()),
            (Some((value, 1)), None) => GroupState::decode(&value, self.aggs.len()),
            (first, second) => panic!(
                "corrupted aggregate state for group {key:?}: expected at most one value \
                 of weight 1, found {first:?} and {second:?}"
            ),
        }
    }

    /// Replace the group's stored state `old` with `new`. A group whose state
    /// is all zero is not stored at all (spec §5.1: no zombie entries).
    fn store(&mut self, key: &Row, old: &GroupState, new: &GroupState) {
        if old == new {
            return;
        }
        if !old.is_empty() {
            self.state.update(key, &old.encode(), -1);
        }
        if !new.is_empty() {
            self.state.update(key, &new.encode(), 1);
        }
    }
}
```

Update the comments inside `agg.rs`'s existing tests that name code this replaces: `an_unchanged_group_emits_nothing` refers to `g.rows += w` (now `d.rows += w`) and `new_out != emitted` (now `new_out != old_out`); `each_agg_reads_its_own_accumulator` refers to `g.accs[i]` on the read side (now `output`'s `zip` over `self.accs`). The comments must describe the new code. The assertions do not change.

- [ ] **Step 5: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes — the 216 existing tests unchanged, plus the new ones. **If an existing differential test goes red, the refactor changed behaviour: find out why; do not adjust the test.**

- [ ] **Step 6: Re-measure the performance row**

The external-review P2-2 row records release timings for a batch touching 10k / 20k / 40k distinct groups. Add this temporary test to `agg.rs`'s test module:

```rust
    #[test]
    #[ignore]
    fn timing_distinct_groups() {
        for n in [10_000i64, 20_000, 40_000] {
            let mut s = count_state();
            let batch = ZSet::from_rows((0..n).map(|i| (row(vec![int(i), int(1)]), 1)));
            let start = std::time::Instant::now();
            s.absorb(&batch);
            eprintln!("{n} groups: {:?}", start.elapsed());
        }
    }
```

Run `cargo test --release -p ivmlite-core --lib agg::tests::timing_distinct_groups -- --ignored --nocapture` three times, and record the median per size. Then run it again under this row's re-mapped mutation (step 7, item 10), record that too, and **delete the test before committing**.

- [ ] **Step 7: Mutation verification**

Every existing gate row that names code this task replaces must be re-mapped to the equivalent edit of the new code, re-run, and updated in place with the new measured counts and the note "re-mapped in M1b Phase 1". These are the rows in `docs/mutation-gates.md`'s `ivmlite-core` table that name `g.emitted`, `groups`, `touched`, `g.accs[i]`, `g.rows`, `new_out != g.emitted`, `groups.remove` or `out.update(new.clone(), 1)`:

1. Retraction of the last emitted row: delete `if let Some(row) = old_out { out.update(row, -1); }`
2. `accs[i]` vs `accs[0]`: in `output`, replace `self.accs.iter().zip(aggs)` with `std::iter::repeat(&self.accs[0]).zip(aggs)`
3. State survives across batches: in `absorb`, replace `let old = self.load(&key);` with `let old = GroupState::empty(self.aggs.len());`
4. `COUNT(*)` is the total weight: `d.rows += w` → `d.rows += 1`
5. The unchanged-output check (`n/a` today): `if new_out != old_out` → `if true`
6. `non_null` is maintained: delete `d.accs[i].non_null += w;` and change `output`'s `acc.non_null == 0` to `self.rows == 0`
7. "any non-NULL input" rather than "sum is 0": `acc.non_null == 0` → `acc.sum == 0`
8. `SUM` scales by weight: `d.accs[i].sum += v * w` → `+= v`
9. Output weight is always 1: in the emit block, `out.update(row, 1)` → `out.update(row, new.rows)`
10. P2-2 quadratic behaviour: replace `deltas` with a `Vec<(Row, GroupState)>` searched linearly (`iter_mut().find`) — record the release timings from Step 6
11. §9.4 emit order (`n/a` today): replace `deltas`' `BTreeMap` with a `HashMap` — run at least 8 times
12. Zombie groups (`n/a` today, "memory reclamation, not semantics"): delete the `if !new.is_empty()` guard in `store`, so an all-zero state is stored → expect `a_group_that_empties_leaves_no_state_behind` to go red. **This row changes from n/a to verified**: the state is now visible through the arrangement, as it will be in a shadow table.

Then the new rows:

13. `load` reads the arrangement — the same edit as item 3 → expect `a_state_rebuilt_from_its_arrangement_retracts_what_the_old_one_emitted` and `a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off` among the reds. Record it as its own row: the property is "an operator's arrangement is its whole state", not "state survives across batches".
14. `store` retracts the old value: delete `self.state.update(key, &old.encode(), -1);` → expect `the_stored_state_is_one_value_per_group` among the reds
15. Corrupted state is reported: replace the `panic!` arm in `load` with `GroupState::empty(self.aggs.len())` → expect `a_group_with_two_stored_values_is_reported_as_corrupted`
16. `Node::build` gives the aggregate the provider's arrangement: in the `Aggregate` arm, pass `Box::new(crate::MemArrangement::new())` instead of `arrangements(...)` → expect `a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off` and `node_ids_number_the_plan_in_pre_order`

The predicted red sets are predictions: record what each run shows.

- [ ] **Step 8: Register the gates and commit**

Update the 12 re-mapped rows and add rows 13–16 (spec requirements: §4.4, "operator state lives in the arrangement a provider supplies"; §5.1 for rows 12 and 14). Run the counter with `--fix`, then bare.

```bash
git add crates/ivmlite-core/src/agg.rs crates/ivmlite-core/src/arrangement.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/test_support.rs crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "refactor(core): keep aggregate state in an Arrangement, derive the emitted row

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 3: Spec amendments and the parked documentation fixes

**Files:**
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md`
- Modify: `crates/ivmlite-test/src/differential.rs` (one doc comment), `crates/ivmlite-test/src/data.rs` (one doc comment)
- Modify: `docs/README.md`

**Interfaces:** none.

- [ ] **Step 1: Amend the spec**

- **§4.4, the M1a amendment.** Add an "Amended 2026-09-24, M1b Phase 1" paragraph after the Join paragraph:
  - Point 1 ("no path to rebuild an operator from persisted state") is closed at the operator level. Every stateful operator keeps its whole state in arrangements supplied by `Node::build`'s provider, addressed by `ArrangementId { node, role }` (pre-order node index, role). `a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off` pins this.
  - The aggregate's value encoding and the fact that the emitted row is derived, not stored.
  - Point 2 (the in-memory `view: ZSet` versus `__ivm_out_<view>`) is still open and belongs to M1b Phase 3.
  - The signature issue is still deferred, now explicitly to M1b Phase 3, with the three questions from this plan's Ruling 3 written out: per-call or per-row errors; whether `get` stays a lazy iterator given a `rusqlite::Statement`'s borrow; what the engine does with its state after a failed `refresh`.
  - In the Join paragraph, replace the `JoinSide` description with `ArrangementId`, and delete the sentence saying `JoinSide` cannot tell two joins apart: that limitation is gone.
- **§6.2, last paragraph** ("So `Aggregate`'s state holds both the accumulators `(sum, count)` and **the output row currently emitted**"): add that since M1b Phase 1 the emitted row is derived from the accumulators instead of stored. It is still what the operator retracts; it is simply recomputed from the old state, which is what makes the state rebuildable.
- **§7**, under the schema block: one sentence saying `<op>` in `__ivm_state_<view>_<op>` corresponds to an `ArrangementId` (pre-order operator index plus role), with one table per `(node, role)`.

- [ ] **Step 2: Fix the three parked documentation issues from M1a Phase 3**

1. **Spec §9.2**, the sentence starting "It was also confirmed by measurement that the enumeration sweep is **a single isolated test**". Measure the two `NaiveRecompute` join sweeps first, each with its median of 3 runs in a debug build:

   ```bash
   cargo test -p ivmlite-test --lib --locked -- --exact differential::tests::naive_engine_passes_every_enumerated_join_query
   cargo test -p ivmlite-test --lib --locked -- --exact differential::tests::naive_engine_passes_every_join_query_with_keys_at_different_positions
   ```

   Then rewrite the sentence to say what is now true. There are five enumeration sweeps, each an isolated test:
   - the single-table sweep;
   - two databases × two engines of join sweeps.

   Give all their measured times. The detection test and the shrinker still take one query per case, so the cost does not compound across tests.
2. **`crates/ivmlite-test/src/differential.rs`**, the doc comment of `naive_engine_passes_every_join_query_with_keys_at_different_positions`. It currently reads as if it guards a key-index mistake shared by `NaiveRecompute` and the oracle's `ON` clause. Rewrite it to say what was measured:
   - it catches a key-index mistake in either one alone;
   - a mistake shared by both cancels out here, and only `incremental_engine_is_green_across_the_join_space_with_keys_at_different_positions` catches it (see the matching gate row).

   Use the test names exactly as they exist in the code; check them with `grep`.
3. **`crates/ivmlite-test/src/data.rs`**, `gen_database`'s doc says widening to 3 columns "was measured to grow the enumeration from about 554 to about 4209". 554 is a pre-join estimate. Replace it with what spec §9.2 now records: the two-column join enumeration is 700 queries (measured), and the three-column one is about 4209 (extrapolated).

- [ ] **Step 3: The docs index**

In `docs/README.md`, add the plan after the Phase 3 entry:

```markdown
  - [`2026-09-24-m1b-phase1-state-ownership.md`](superpowers/plans/2026-09-24-m1b-phase1-state-ownership.md) — M1b Phase 1: operator state behind `Arrangement`
```

- [ ] **Step 4: Check and commit**

Run `grep -rn "JoinSide\|single isolated test\|about 554" crates docs` — nothing may remain except inside historical plan documents (`docs/superpowers/plans/2026-09-2[0-3]-*.md`), which record what was true when they were written and are not edited. Then run fmt, clippy and the full test suite.

```bash
git add docs/superpowers/specs/2026-09-18-ivmlite-design.md crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/data.rs docs/README.md
git commit -m "docs: record operator state ownership in the spec, and fix three stale doc lines

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review

**1. Spec coverage**

| Spec requirement | Where it lands |
|---|---|
| §4.4 point 1: a path to rebuild operators from persisted state | Task 2: `AggState` on `Arrangement`; `a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off` |
| §4.4 join paragraph: `JoinSide` cannot tell two joins apart | Task 1: `ArrangementId`; `two_joins_in_one_plan_get_distinct_ids` |
| §4.4 point 2: materialized output in memory | Out of scope, M1b Phase 3 (Task 3 records it) |
| §4.4 signature issue: infallible `Arrangement` | Ruling 3: deferred to Phase 3 with the concrete questions (Task 3 records it) |
| §5.1 no zombie entries | Task 2 rows 12, 14 |
| §6.1 SUM NULL contract | Unchanged semantics; rows 6–7 re-mapped |
| §6.2 retraction pairs | Task 2 rows 1, 3, 13 and the rebuild tests |
| §6.3 trait unchanged | No task edits the trait |
| §7 canonical encoding; per-operator state tables | Ruling 1's fixed layout; Ruling 2's ids; Task 3's §7 sentence |
| §9.4 deterministic order | `deltas` is a `BTreeMap`; `Mirrors` uses a `BTreeMap`; row 11 re-measured |

**2. Placeholder scan**: no TBD / TODO. Instructions without full code are mechanical and named exactly: constructor call sites, moving three test helpers, comment updates in existing tests, and the spec prose of Task 3, whose content is specified point by point.

**3. Type consistency**: `ArrangementId { node: usize, role: ArrangementRole }` and `fresh_mem_arrangement(ArrangementId)` (Task 1) → `ArrangementRole::AggregateGroups` and `AggState::new(Vec<usize>, Vec<Agg>, Box<dyn Arrangement>)` (Task 2) → `Mirrors::arrangement(ArrangementId) -> Box<dyn Arrangement>`, `Mirrors::snapshot(ArrangementId) -> MemArrangement` (Task 2's tests). `Node::build(&Plan, &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>)` is the one signature both Task 1 and Task 2 use.

## Execution order

Tasks 1 → 3 depend strictly on each other. Task 2 updates two expectations of Task 1's `node_ids_number_the_plan_in_pre_order`, because the aggregate starts asking for an arrangement.
