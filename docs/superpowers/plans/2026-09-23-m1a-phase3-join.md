# M1a Phase 3: Two-Table Equi-Join Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a two-table inner equi-join to v0's incremental engine, green in the differential harness across the whole enumerated join space, and close the "When join lands" checklist in `docs/mutation-gates.md`.

**Architecture:** `ViewQuery` gains an `Option<Join>` field. `lower` turns it into a `Plan::Join` over two `Scan`s, with `Filter` / `Project` / `Aggregate` above it indexing the joined row (left table's columns first, then the right table's). The new `JoinState` operator keeps one `Arrangement` per side and applies spec §6.1's bilinear rule `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS`. Both arrangements are passed in from outside as `Box<dyn Arrangement>`: `Node::build` asks a caller-supplied provider for one per side, rather than the join creating its own. On the test side, the oracle renders `JOIN ... ON` SQL, `NaiveRecompute` computes the join by nested loops, and the enumerator adds the join query space.

**Tech Stack:** Rust 1.95, pure Rust with no `unsafe`, no new dependencies (per §4.2, `ivmlite-core` must not depend on `rusqlite`).

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md`

**Preceding plan:** `docs/superpowers/plans/2026-09-21-m1a-phase2-engine.md` (complete and merged into master, df579f3)

---

## This plan's scope and the design rulings behind it

### In scope: two-table inner equi-join only

`SELECT ... FROM t0 JOIN t1 ON t0.a = t1.b [WHERE ...] GROUP BY ...`: one pair of key columns of the same type, inner join, no self-join. The left input is always the anchor table (`db.tables()[0]`, the existing v0 convention); the right input is named by `Join::right`. `WHERE` is evaluated after the join, over the joined row.

**Out of scope** (listed so they are not mistaken for omissions):
- LEFT / RIGHT / FULL OUTER joins: the retraction semantics of the NULL-padded side are much harder, and are a separate plan.
- Chains of more than two tables, and multi-column join keys.
- Self-joins: `Node::Scan` routes deltas by table name, so both inputs of a self-join would receive every change to that table. v0 rejects self-joins in `lower`.
- Filter push-down below the join: correct but not needed; `Filter` stays above the `Join`.
- Moving `Aggregate`'s state onto `Arrangement` (spec §4.4's state-ownership gap). This refactor changes no semantics, and the existing differential tests guard it fully, but doing it here would break the premise that a join bug can be bisected on its own. **It is a separate small step after this plan and before M1b, or the first task of the M1b plan.**
- Widening the differential schema to 3 columns. Spec §9.2 says to decide this with a measured join multiplier. This plan **measures and records** that multiplier (Task 4). The decision itself is not made here.

### Ruling 1: `ViewQuery` gains `Option<Join>`; it is not replaced by a tree

`ViewQuery` never needs to become the product's IR. M1b's SQL front end lowers straight from `sqlparser` to `Plan`, without going through `ViewQuery`. So `ViewQuery` is only the harness's input format, and it only has to cover the query shapes v0 supports. With v0 doing only two-table equi-joins, one `Option<Join>` field is exactly enough. Once the shapes really multiply (multi-table joins, filters before the join), replacing it with a tree is still cheap, because what changes then is test-side code.

### Ruling 2: the join's arrangements come from outside

`JoinState::new` takes both arrangements as arguments, and `Node::build` obtains them from a provider `&mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>`. This costs almost nothing today (the engine passes `fresh_mem_arrangement`). But later, "rebuild from persisted state" is just a provider that returns a non-empty arrangement for each side, with no change to the join's interface. The `JoinSide` argument is what lets such a provider tell the two inputs apart.

### Ruling 3: `Box<dyn Arrangement>`, not a type parameter

Spec §6.3 already requires `Arrangement` to be object-safe, precisely so that a type parameter does not spread through the whole operator tree. `Node` stays non-generic.

### Ruling 4: key types must match; NULL keys never match

Both rules follow SQLite, measured on `STRICT` tables `t0(k TEXT, v INTEGER)` / `t1(k TEXT, v INTEGER)`:

```sql
INSERT INTO t0 VALUES ('7',1),('v7',2);  INSERT INTO t1 VALUES ('x',7);
SELECT t0.k, t1.v FROM t0 JOIN t1 ON t0.k = t1.v;   -- 7|7   (TEXT '7' = INTEGER 7 is true)
-- with NULL keys on both sides:
SELECT COUNT(*) FROM t0 JOIN t1 ON t0.k = t1.k WHERE t0.k IS NULL;   -- 0
```

SQLite compares an INTEGER column with a TEXT column under numeric affinity, so `'7' = 7` is true there, while v0 compares `Value`s exactly. This is the same class of problem as §6.1's operand-type rule, so `lower` rejects join keys of different types. `NULL = NULL` is UNKNOWN, so a row whose key is NULL never matches anything. `JoinState` neither stores nor probes such rows, and `NaiveRecompute` skips them too.

---

## Global Constraints

Each of the following comes from the spec or the repository's rules and is an implicit requirement of every task:

- **§4.2 `ivmlite-core` must not depend on `rusqlite`**, nor on `ivmlite-test`. This plan introduces no new dependency.
- **§4.2 `unsafe` is allowed only in `ivmlite-sqlite` (M1b).** Not one line of `unsafe` may appear in this plan.
- **§5.1 a row whose weight reaches zero must be deleted**, leaving no zombie entries.
- **§5.2 the root operator must be an `Aggregate` with a non-empty `group_by`**; global aggregates are forbidden.
- **§6.1 `SUM` over zero non-NULL inputs returns `NULL`, not `0`.**
- **§6.1 predicates use three-valued logic**; do not conclude that `NOT p` is `!p`.
- **§6.1 join (bilinear): `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS`**, with state kept per side.
- **§6.3 `Arrangement::get` returns an iterator**, and the trait stays object-safe. This plan does **not** change the `Arrangement` trait.
- **§8.5 `apply` takes the unconsolidated raw Δ, `refresh` is separate from `apply`, `apply` carries a table name.** None of these signatures may change.
- **§9.2 item 4: the differential schema is fixed at 2 columns per table.** Do not widen it.
- **§9.4 iteration order is deterministic**: use `Vec` / `BTreeMap` for anything that can affect output, never `HashMap` / `HashSet`.
- **Mutation gates**: every spec-mandated behaviour gets a row in `docs/mutation-gates.md`, and "verified" must come from actually running the mutation: break it → **confirm it still compiles** (`cargo build --workspace --all-targets --locked`) → run the tests → confirm the named test goes red → restore. A compile failure produces no test output, which careless filtering reads as "tests passed".
- **The test command is `cargo test --workspace --locked --no-fail-fast`.**
- Before every commit, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` must pass.
- After adding gate rows, run `python3 scripts/count-mutation-gates.py --fix`, then run it bare once more to confirm it prints `consistent`. The script checks only the summary sentence and the literal cell prefixes. It does not judge whether a row is honest.
- **Record measurements, never computed numbers**, in gate rows: `N passed / M failed (baseline B/0)` as printed by the run.
- **English only** in code, comments, error strings, docs and commit messages (ivmlite ships as a public SQLite extension).
- Stage specific files, never `git add -A`; never use `--no-verify`. End every commit message with:

```
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## File structure

| File | Responsibility |
|---|---|
| `crates/ivmlite-core/src/join.rs` (new) | `JoinSide`, `fresh_mem_arrangement`, `JoinState` with the bilinear delta rule |
| `crates/ivmlite-core/src/plan.rs` (modified) | `Plan::Join`; `lower(&ViewQuery, &Database)` with join validation and lowering |
| `crates/ivmlite-core/src/node.rs` (modified) | `Node::Join`; `Node::build` takes the arrangement provider |
| `crates/ivmlite-core/src/query.rs` (modified) | `Join`; `ViewQuery::join` |
| `crates/ivmlite-core/src/engine.rs` (modified) | `create_view` calls `lower(query, db)` and passes `fresh_mem_arrangement` |
| `crates/ivmlite-core/src/lib.rs` (modified) | Exports |
| `crates/ivmlite-test/src/sql.rs` (modified) | `view_query_to_sql(&ViewQuery, &Database)`, qualified column names, `JOIN ... ON` |
| `crates/ivmlite-test/src/oracle.rs` (modified) | Renders the view against the whole `Database` |
| `crates/ivmlite-test/src/naive.rs` (modified) | Nested-loop join in `materialize` |
| `crates/ivmlite-test/src/query.rs` (modified) | `enumerate_join`, `enumerate_database` |
| `crates/ivmlite-test/src/differential.rs` (modified) | `gen_case` uses `enumerate_database`; join tests; overlap probes |
| `crates/ivmlite-test/tests/harness_catches_bugs.rs` (modified) | The incremental engine across the join space |
| `docs/mutation-gates.md`, the spec (modified) | Gate rows, checklist closure, spec amendments |

`join.rs` is its own file for the same reason `agg.rs` is: it is a stateful operator with its own correctness argument, and putting it in `node.rs` would bury the linear operators' three-line rules again.

---

## Task 1: `Plan::Join`, `JoinState`, and `Node::Join` with injected arrangements

**Files:**
- Create: `crates/ivmlite-core/src/join.rs`
- Modify: `crates/ivmlite-core/src/plan.rs` (the `Plan` enum and its doc comment only)
- Modify: `crates/ivmlite-core/src/node.rs`
- Modify: `crates/ivmlite-core/src/engine.rs` (the one `Node::build` call)
- Modify: `crates/ivmlite-core/src/lib.rs`

**Interfaces:**
- Consumes: `Arrangement`, `MemArrangement`, `Row`, `Value`, `ZSet` (existing)
- Produces:
  - `pub enum JoinSide { Left, Right }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn fresh_mem_arrangement(side: JoinSide) -> Box<dyn Arrangement>`
  - `pub struct JoinState` with `pub fn new(left_key: usize, right_key: usize, left: Box<dyn Arrangement>, right: Box<dyn Arrangement>) -> JoinState` and `pub fn absorb(&mut self, left_delta: &ZSet, right_delta: &ZSet) -> ZSet`
  - `Plan::Join { left: Box<Plan>, right: Box<Plan>, left_key: usize, right_key: usize }`
  - `Node::Join { left: Box<Node>, right: Box<Node>, state: JoinState }`
  - `Node::build(plan: &Plan, arrangements: &mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>) -> Node`

> **Why the join key lives in a one-column `Row`:** `Arrangement` is keyed by `Row`. A join arrangement's key is `Row::new(vec![key_value])`, and its value is the whole input row. This is the contract a future persisted-state provider must follow, so it is written into `JoinState`'s doc comment and pinned by `a_join_uses_the_arrangements_it_is_given`.

- [ ] **Step 1: Write the failing tests**

Create `crates/ivmlite-core/src/join.rs` with only the test module below for now (Step 3 adds the implementation above it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemArrangement, Row, Value, ZSet};

    /// A `(k TEXT, v INTEGER)` row; `k = None` is a NULL key.
    fn kv(k: Option<&str>, v: i64) -> Row {
        Row::new(vec![
            k.map_or(Value::Null, |s| Value::Text(s.into())),
            Value::Int(v),
        ])
    }

    fn joined(left: &Row, right: &Row) -> Row {
        let mut values = left.0.clone();
        values.extend(right.0.iter().cloned());
        Row::new(values)
    }

    fn z(rows: &[(Row, i64)]) -> ZSet {
        ZSet::from_rows(rows.iter().cloned())
    }

    fn empty_join() -> JoinState {
        JoinState::new(
            0,
            0,
            fresh_mem_arrangement(JoinSide::Left),
            fresh_mem_arrangement(JoinSide::Right),
        )
    }

    #[test]
    fn delta_on_the_left_joins_against_earlier_right_state() {
        // The ΔR⋈S term: S arrived in an earlier call, ΔR arrives now.
        let mut j = empty_join();
        assert!(j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)])).is_empty());
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn delta_on_the_right_joins_against_earlier_left_state() {
        // The R⋈ΔS term.
        let mut j = empty_join();
        assert!(j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new()).is_empty());
        let out = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn both_deltas_in_one_call_include_the_delta_cross_term() {
        // Spec §6.1: Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS. Starting empty, the first
        // two terms are empty, so the only output can come from ΔR⋈ΔS. An
        // implementation that joins both deltas against the state from before
        // the call drops this term.
        let mut j = empty_join();
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &z(&[(kv(Some("a"), 10), 1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }

    #[test]
    fn null_keys_never_match() {
        // SQL: NULL = NULL is UNKNOWN, so a NULL key matches nothing — not even
        // another NULL. Measured in SQLite: an inner join on two NULL keys
        // yields no row.
        let mut j = empty_join();
        assert!(j.absorb(&ZSet::new(), &z(&[(kv(None, 10), 1)])).is_empty());
        assert!(j.absorb(&z(&[(kv(None, 1), 1)]), &ZSet::new()).is_empty());
        assert!(j
            .absorb(&z(&[(kv(None, 2), 1)]), &z(&[(kv(None, 20), 1)]))
            .is_empty());
    }

    #[test]
    fn output_weight_is_the_product_of_input_weights() {
        // Z-set join: a row of weight 2 matched with a row of weight 3 is 6
        // joined rows. The retraction of one left copy is then -3.
        let mut j = empty_join();
        j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 3)]));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 2)]), &ZSet::new());
        let row = joined(&kv(Some("a"), 1), &kv(Some("a"), 10));
        assert_eq!(out, z(&[(row.clone(), 6)]));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), -1)]), &ZSet::new());
        assert_eq!(out, z(&[(row, -3)]));
    }

    #[test]
    fn a_key_with_several_rows_on_the_other_side_matches_each() {
        // Spec §6.3: each side of a join is key → many rows, which is why
        // `Arrangement::get` returns an iterator rather than an `Option`.
        let mut j = empty_join();
        j.absorb(
            &ZSet::new(),
            &z(&[(kv(Some("a"), 10), 1), (kv(Some("a"), 20), 1)]),
        );
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[
                (joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1),
                (joined(&kv(Some("a"), 1), &kv(Some("a"), 20)), 1),
            ])
        );
    }

    #[test]
    fn output_row_is_left_columns_then_right_columns_on_both_paths() {
        // Every operator above the join indexes the joined row as "left
        // columns, then right columns", on the ΔR⋈S path and on the R⋈ΔS path.
        let mut j = empty_join();
        let from_right = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        assert!(from_right.is_empty());
        let from_left = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        let later_right = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 20), 1)]));
        assert_eq!(
            from_left,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
        assert_eq!(
            later_right,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 20)), 1)])
        );
    }

    #[test]
    fn retracting_a_row_retracts_its_join_results() {
        let mut j = empty_join();
        j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), 1)]));
        j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        let out = j.absorb(&ZSet::new(), &z(&[(kv(Some("a"), 10), -1)]));
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), -1)])
        );
        // After the retraction the right side is empty again: a new left row
        // for key "a" matches nothing.
        assert!(j.absorb(&z(&[(kv(Some("a"), 2), 1)]), &ZSet::new()).is_empty());
    }

    #[test]
    fn a_join_uses_the_arrangements_it_is_given() {
        // Ruling 2: the arrangements are passed in, so "rebuild from persisted
        // state" is only a matter of passing a non-empty one. The key layout is
        // part of that contract: a one-column Row holding the key value, with
        // the whole input row as the value.
        let mut right = MemArrangement::new();
        right.update(
            &Row::new(vec![Value::Text("a".into())]),
            &kv(Some("a"), 10),
            1,
        );
        let mut j = JoinState::new(0, 0, fresh_mem_arrangement(JoinSide::Left), Box::new(right));
        let out = j.absorb(&z(&[(kv(Some("a"), 1), 1)]), &ZSet::new());
        assert_eq!(
            out,
            z(&[(joined(&kv(Some("a"), 1), &kv(Some("a"), 10)), 1)])
        );
    }
}
```

Add to the end of the test module in `crates/ivmlite-core/src/node.rs`:

```rust
    fn join_plan() -> Plan {
        Plan::Join {
            left: Box::new(Plan::Scan {
                table: "t0".into(),
                columns: vec![0, 1],
            }),
            right: Box::new(Plan::Scan {
                table: "t1".into(),
                columns: vec![0, 1],
            }),
            left_key: 0,
            right_key: 0,
        }
    }

    #[test]
    fn a_join_tree_routes_each_tables_delta_to_its_own_side() {
        let mut n = Node::build(&join_plan(), &mut crate::fresh_mem_arrangement);
        let right_row = Row::new(vec![Value::Text("a".into()), Value::Int(10)]);
        let left_row = Row::new(vec![Value::Text("a".into()), Value::Int(1)]);
        assert!(n
            .delta("t1", &ZSet::from_rows([(right_row, 1)]))
            .is_empty());
        let out = n.delta("t0", &ZSet::from_rows([(left_row, 1)]));
        assert_eq!(
            out,
            ZSet::from_rows([(
                Row::new(vec![
                    Value::Text("a".into()),
                    Value::Int(1),
                    Value::Text("a".into()),
                    Value::Int(10),
                ]),
                1
            )])
        );
    }

    #[test]
    fn build_asks_for_one_arrangement_per_side() {
        // Ruling 2: a future persisted-state provider tells the two inputs
        // apart only through the `JoinSide` it is asked for.
        let mut asked = Vec::new();
        let _ = Node::build(&join_plan(), &mut |side| {
            asked.push(side);
            crate::fresh_mem_arrangement(side)
        });
        assert_eq!(asked, vec![crate::JoinSide::Left, crate::JoinSide::Right]);
    }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked`
Expected: a compile failure — `mod join` is not declared yet, so first add `mod join;` to `lib.rs`, then the failure is `cannot find type JoinState` / `no variant named Join`.

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-core/src/join.rs`, above the test module:

```rust
use crate::{Arrangement, MemArrangement, Row, Value, ZSet};

/// Which input of a join an arrangement belongs to.
///
/// `Node::build` asks its arrangement provider for one arrangement per side
/// and passes this so the provider can tell the two apart — the hook a future
/// "rebuild the join from persisted state" provider needs (M1a Phase 3,
/// Ruling 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinSide {
    Left,
    Right,
}

/// The provider the engine passes to `Node::build` today: every side starts
/// from an empty in-memory arrangement.
pub fn fresh_mem_arrangement(_side: JoinSide) -> Box<dyn Arrangement> {
    Box::new(MemArrangement::new())
}

/// Spec §6.1's bilinear operator: a two-table inner equi-join.
///
/// Each side's current contents live in an `Arrangement` keyed by the join
/// key. **The key is a one-column `Row` holding the key value; the value is the
/// whole input row.** Both arrangements are passed in rather than created
/// here, so rebuilding a join from persisted state is a matter of passing
/// non-empty ones (M1a Phase 3, Ruling 2).
///
/// A row whose key is NULL is neither stored nor probed: SQL's `NULL = NULL`
/// is UNKNOWN, so such a row can never match (measured in SQLite; see the
/// plan's Ruling 4).
pub struct JoinState {
    left_key: usize,
    right_key: usize,
    left: Box<dyn Arrangement>,
    right: Box<dyn Arrangement>,
}

impl JoinState {
    pub fn new(
        left_key: usize,
        right_key: usize,
        left: Box<dyn Arrangement>,
        right: Box<dyn Arrangement>,
    ) -> Self {
        Self {
            left_key,
            right_key,
            left,
            right,
        }
    }

    /// Absorb one call's deltas from both inputs and return the join's output delta.
    ///
    /// Spec §6.1: `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS`. This computes it in two
    /// steps: `ΔR` is joined against `S` as it stood before this call and then
    /// folded into the left arrangement; `ΔS` is then joined against the
    /// **updated** left side, `R + ΔR`, which contributes both `R⋈ΔS` and
    /// `ΔR⋈ΔS`. Folding `ΔR` in before probing with `ΔS` is what carries the
    /// cross term — doing it afterwards silently drops it.
    ///
    /// The same argument makes the result independent of which table the
    /// engine refreshes first: across two calls, one per table, the two steps
    /// are exactly the two calls, in either order.
    pub fn absorb(&mut self, left_delta: &ZSet, right_delta: &ZSet) -> ZSet {
        let mut out = ZSet::new();

        // ΔR ⋈ S, against S from before this call.
        for (l, &wl) in left_delta.iter() {
            let Some(key) = join_key(l, self.left_key) else {
                continue;
            };
            for (r, wr) in self.right.get(&key) {
                out.update(concat(l, &r), wl * wr);
            }
        }
        for (l, &wl) in left_delta.iter() {
            if let Some(key) = join_key(l, self.left_key) {
                self.left.update(&key, l, wl);
            }
        }

        // (R + ΔR) ⋈ ΔS: R⋈ΔS and ΔR⋈ΔS together.
        for (r, &wr) in right_delta.iter() {
            let Some(key) = join_key(r, self.right_key) else {
                continue;
            };
            for (l, wl) in self.left.get(&key) {
                out.update(concat(&l, r), wl * wr);
            }
        }
        for (r, &wr) in right_delta.iter() {
            if let Some(key) = join_key(r, self.right_key) {
                self.right.update(&key, r, wr);
            }
        }

        out
    }
}

impl std::fmt::Debug for JoinState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Arrangement` is not `Debug`, and adding it as a supertrait
        // would impose it on M1b's shadow-table implementation for no benefit.
        f.debug_struct("JoinState")
            .field("left_key", &self.left_key)
            .field("right_key", &self.right_key)
            .finish_non_exhaustive()
    }
}

/// The arrangement key for `row`'s join column, or `None` when that column is
/// NULL — a NULL key never matches (SQL three-valued equality).
fn join_key(row: &Row, column: usize) -> Option<Row> {
    match row.get(column) {
        Value::Null => None,
        value => Some(Row::new(vec![value.clone()])),
    }
}

/// The joined row: the left row's columns, then the right row's.
fn concat(left: &Row, right: &Row) -> Row {
    let mut values = left.0.clone();
    values.extend(right.0.iter().cloned());
    Row::new(values)
}
```

In `crates/ivmlite-core/src/plan.rs`, add the variant after `Scan` and before `Filter`:

```rust
    /// Spec §5.2 / §6.1: a two-table inner equi-join (M1a Phase 3). Its output
    /// row is the left child's row followed by the right child's row, so every
    /// operator above it indexes that concatenation: the left child's columns
    /// first, then the right child's.
    ///
    /// The keys are column indices into each child's output rather than spec
    /// §5.2's `Vec<(Expr, Expr)>`: v0 joins on exactly one pair of bare columns
    /// (the same reasoning as the doc comment on `Plan` about `Expr`).
    Join {
        left: Box<Plan>,
        right: Box<Plan>,
        left_key: usize,
        right_key: usize,
    },
```

and replace the first paragraph of `Plan`'s doc comment (the one ending "…code no test can reach.") with:

```rust
/// Spec §5.2's plan IR.
///
/// v0's legal shapes are `Scan → Filter? → Project → Aggregate` and, with a
/// join, `Join(Scan, Scan) → Filter? → Project → Aggregate`; `lower` guarantees
/// that nothing else is produced.
```

In `crates/ivmlite-core/src/node.rs`:

1. Change the import line to `use crate::{Arrangement, JoinSide, JoinState, Plan, Predicate, Row, Value, ZSet};`
2. Add the variant to `Node`, after `Scan`:

```rust
    Join {
        left: Box<Node>,
        right: Box<Node>,
        state: JoinState,
    },
```

3. Change `build`'s signature and body. Add to its doc comment: "`arrangements` supplies each join input's arrangement (M1a Phase 3, Ruling 2); the engine passes `fresh_mem_arrangement`." The new signature and the recursive calls:

```rust
    pub fn build(
        plan: &Plan,
        arrangements: &mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>,
    ) -> Node {
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
                let left = Node::build(left, arrangements);
                let right = Node::build(right, arrangements);
                Node::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    state: JoinState::new(
                        *left_key,
                        *right_key,
                        arrangements(JoinSide::Left),
                        arrangements(JoinSide::Right),
                    ),
                }
            }
            Plan::Filter { input, predicate } => Node::Filter {
                input: Box::new(Node::build(input, arrangements)),
                predicate: predicate.clone(),
            },
            Plan::Project { input, columns } => Node::Project {
                input: Box::new(Node::build(input, arrangements)),
                columns: columns.clone(),
            },
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => Node::Aggregate {
                input: Box::new(Node::build(input, arrangements)),
                state: crate::AggState::new(group_by.clone(), aggs.clone()),
            },
        }
    }
```

4. Add the `delta` arm after the `Scan` arm:

```rust
            Node::Join { left, right, state } => {
                // Each child is a `Scan` routing by table name, so for one
                // table's delta at most one of these is non-empty (v0 rejects
                // self-joins in `lower`); `JoinState::absorb` is correct either way.
                let left_delta = left.delta(table, input);
                let right_delta = right.delta(table, input);
                state.absorb(&left_delta, &right_delta)
            }
```

5. Update every existing `Node::build(&x)` call in `node.rs`'s tests to `Node::build(&x, &mut crate::fresh_mem_arrangement)`. Also update the stale comment in `scan_only_absorbs_its_own_table` ("(Phase 3 needs …)"): Phase 3 is where it happens, so it should read "each side of a join absorbs only its own table's deltas — this is what `Node::Join` relies on".

In `crates/ivmlite-core/src/engine.rs`, change `CountingTree::new(Node::build(&plan))` to `CountingTree::new(Node::build(&plan, &mut fresh_mem_arrangement))`, and add `fresh_mem_arrangement` to the `use crate::{...}` line.

In `crates/ivmlite-core/src/lib.rs`, add `mod join;` (alphabetically, after `mod engine;`) and `pub use join::{fresh_mem_arrangement, JoinSide, JoinState};`.

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes.

- [ ] **Step 5: Mutation verification**

Run these one at a time, confirming each still **compiles** before reading the test output:

1. In `JoinState::new`, ignore the `left`/`right` arguments and store `Box::new(MemArrangement::new())` for both → expect `a_join_uses_the_arrangements_it_is_given` to go red
2. Make `join_key` return `Some(Row::new(vec![Value::Null]))` for a NULL key instead of `None` → expect `null_keys_never_match` to go red
3. Replace `wl * wr` with `wl` in the ΔR loop → expect `output_weight_is_the_product_of_input_weights` to go red
4. Move the loop that folds `left_delta` into `self.left` to after the `right_delta` probe loop → expect `both_deltas_in_one_call_include_the_delta_cross_term` to go red, and **only** that test (the other paths never carry two deltas in one call)
5. In the right-delta probe loop, change `concat(&l, r)` to `concat(r, &l)` → expect `output_row_is_left_columns_then_right_columns_on_both_paths` to go red
6. Delete the loop that folds `right_delta` into `self.right` → expect `delta_on_the_left_joins_against_earlier_right_state` to go red
7. In `Node::build`'s `Plan::Join` arm, pass `arrangements(JoinSide::Left)` for both sides → expect `build_asks_for_one_arrangement_per_side` to go red
8. In `Node::delta`'s `Join` arm, replace `right.delta(table, input)` with `ZSet::new()` → expect `a_join_tree_routes_each_tables_delta_to_its_own_side` to go red

- [ ] **Step 6: Register the gates and commit**

Add the 8 rows above to the `ivmlite-core` table of `docs/mutation-gates.md`, marking each `**verified**` with the measured `N passed / M failed (baseline B/0)`. Spec requirements for the rows: §6.1 bilinear rule (rows 4 and 6), §6.1 NULL three-valued equality (row 2), Z-set weight semantics §5.1 (row 3), the joined-row layout (row 5), Ruling 2 (rows 1 and 7), §6.1 per-side routing (row 8). Then run the counting script with `--fix`, and again bare.

```bash
git add crates/ivmlite-core/src/join.rs crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): the join operator, with arrangements passed in from outside

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 2: `ViewQuery::join`, lowering and validation, and the engine running join views

**Files:**
- Modify: `crates/ivmlite-core/src/query.rs`
- Modify: `crates/ivmlite-core/src/plan.rs` (`lower` and its tests)
- Modify: `crates/ivmlite-core/src/engine.rs` (`create_view` and its tests)
- Modify: `crates/ivmlite-core/src/node.rs` (its tests' `lower` calls only)
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: every file with a `ViewQuery { ... }` literal (listed in Step 3)

**Interfaces:**
- Consumes: `Plan::Join`, `Node::build(&Plan, &mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>)`, `fresh_mem_arrangement` (Task 1)
- Produces:
  - `pub struct Join { pub right: String, pub left_column: usize, pub right_column: usize }` (derives `Debug, Clone, PartialEq, Eq`, plus the conditional serde derive)
  - `ViewQuery { group_by, aggs, predicate, join: Option<Join> }`, with `join` defaulting to `None` when deserialised (the frozen regression fixtures in `crates/ivmlite-test/tests/regressions/*.json` have no `join` field)
  - `lower(query: &ViewQuery, db: &Database) -> Result<Plan, PlanError>` (was `lower(&ViewQuery, &Schema)`)

- [ ] **Step 1: Write the failing tests**

In `crates/ivmlite-core/src/plan.rs`'s test module, add these helpers and tests:

```rust
    /// `t0(k TEXT, v INTEGER)` and `t1(k TEXT, v INTEGER)` — the shape the
    /// differential harness's `gen_database(2)` produces.
    fn two_kv() -> Database {
        let kv = |name: &str| Schema {
            table: name.into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        Database::new(vec![kv("t0"), kv("t1")])
    }

    fn join(right: &str, left_column: usize, right_column: usize) -> Option<Join> {
        Some(Join {
            right: right.into(),
            left_column,
            right_column,
        })
    }

    fn jq(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate, j: Option<Join>) -> ViewQuery {
        ViewQuery {
            join: j,
            ..q(group_by, aggs, predicate)
        }
    }

    #[test]
    fn lowers_a_join_to_two_scans_under_a_join() {
        // group by t1.k (joined column 2), SUM(t0.v) (joined column 1), join on k = k.
        let plan = lower(
            &jq(vec![2], vec![sum(1)], Predicate::None, join("t1", 0, 0)),
            &two_kv(),
        )
        .expect("a legal join query must lower");
        let scan = |t: &str| Plan::Scan {
            table: t.into(),
            columns: vec![0, 1],
        };
        assert_eq!(
            plan,
            Plan::Aggregate {
                input: Box::new(Plan::Project {
                    input: Box::new(Plan::Join {
                        left: Box::new(scan("t0")),
                        right: Box::new(scan("t1")),
                        left_key: 0,
                        right_key: 0,
                    }),
                    columns: vec![2, 1],
                }),
                group_by: vec![0],
                aggs: vec![Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                }],
            }
        );
    }

    #[test]
    fn a_column_past_the_joined_row_is_rejected() {
        let err = lower(
            &jq(vec![4], vec![count()], Predicate::None, join("t1", 0, 0)),
            &two_kv(),
        )
        .expect_err("the joined row has columns 0..4");
        assert!(err.0.contains('4') && err.0.contains("joined row"), "{}", err.0);
    }

    #[test]
    fn a_join_on_an_unknown_table_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("nope", 0, 0)),
            &two_kv(),
        )
        .expect_err("the right table must be declared");
        assert!(err.0.contains("nope"), "{}", err.0);
    }

    #[test]
    fn a_self_join_is_rejected() {
        // `Node::Scan` routes deltas by table name, so both inputs of a
        // self-join would receive every change to the table.
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t0", 0, 0)),
            &two_kv(),
        )
        .expect_err("v0 does not support self-joins");
        assert!(err.0.contains("self-join"), "{}", err.0);
    }

    #[test]
    fn an_out_of_range_left_join_key_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 5, 0)),
            &two_kv(),
        )
        .expect_err("t0 has only 2 columns");
        assert!(err.0.contains('5') && err.0.contains("left_column"), "{}", err.0);
    }

    #[test]
    fn an_out_of_range_right_join_key_is_rejected() {
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 0, 5)),
            &two_kv(),
        )
        .expect_err("t1 has only 2 columns");
        assert!(err.0.contains('5') && err.0.contains("right_column"), "{}", err.0);
    }

    #[test]
    fn join_keys_of_different_types_are_rejected() {
        // Measured in SQLite: an INTEGER column compared with a TEXT column
        // uses numeric affinity, so '7' = 7 is true there, while v0 compares
        // values exactly (the plan's Ruling 4).
        let err = lower(
            &jq(vec![0], vec![count()], Predicate::None, join("t1", 0, 1)),
            &two_kv(),
        )
        .expect_err("t0.k is TEXT and t1.v is INTEGER");
        assert!(err.0.contains("same type"), "{}", err.0);
    }

    #[test]
    fn int_gt_over_the_right_tables_text_column_is_rejected() {
        // The type checks must use the joined row's columns: joined column 2 is t1.k, a TEXT column.
        let err = lower(
            &jq(
                vec![0],
                vec![count()],
                Predicate::IntGt { column: 2, value: 3 },
                join("t1", 0, 0),
            ),
            &two_kv(),
        )
        .expect_err("IntGt over TEXT is rejected");
        assert!(err.0.contains("IntGt") && err.0.contains("Text"), "{}", err.0);
    }
```

The test module's `use` line becomes `use crate::{Agg, AggFn, Column, ColumnType, Database, Join, Predicate, Schema, ViewQuery};`.

In `crates/ivmlite-core/src/engine.rs`'s test module, add:

```rust
    /// `(k TEXT, v INTEGER)`, the harness's two-column shape.
    fn kv(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        }
    }

    fn kv_row(k: &str, v: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(v)])
    }

    /// `SELECT t0.k, COUNT(*), SUM(t1.v) FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t0.k`
    fn join_on_k() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
                Agg {
                    func: AggFn::Sum,
                    column: Some(3),
                },
            ],
            predicate: Predicate::None,
            join: Some(crate::Join {
                right: "t1".into(),
                left_column: 0,
                right_column: 0,
            }),
        }
    }

    fn out(k: &str, count: i64, sum: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(count), Value::Int(sum)])
    }

    fn joined_engine() -> IncrementalEngine {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let initial = BTreeMap::from([
            (
                "t0".to_string(),
                ZSet::from_rows([(kv_row("a", 1), 1), (kv_row("b", 2), 1)]),
            ),
            (
                "t1".to_string(),
                ZSet::from_rows([(kv_row("a", 10), 1), (kv_row("a", 20), 1)]),
            ),
        ]);
        let mut engine = IncrementalEngine::new();
        engine.create_view(&db, &join_on_k(), &initial).unwrap();
        engine
    }

    #[test]
    fn a_join_view_bootstraps_from_both_tables() {
        // Both tables' initial rows must reach the join: a bootstrap that
        // absorbed only the anchor table would leave the right side empty and
        // the view empty.
        assert_eq!(
            joined_engine().snapshot(),
            ZSet::from_rows([(out("a", 2, 30), 1)])
        );
    }

    #[test]
    fn a_join_view_follows_changes_to_both_tables() {
        let mut engine = joined_engine();
        // Group "a" loses its only left row; "b" gains a right row; and key
        // "c" arrives on both sides in the same refresh — the ΔR⋈ΔS term at
        // the engine level.
        engine.apply("t0", &[(kv_row("a", 1), -1), (kv_row("c", 1), 1)]).unwrap();
        engine.apply("t1", &[(kv_row("b", 5), 1), (kv_row("c", 7), 1)]).unwrap();
        engine.refresh().unwrap();
        assert_eq!(
            engine.snapshot(),
            ZSet::from_rows([(out("b", 1, 5), 1), (out("c", 1, 7), 1)])
        );
    }

    #[test]
    fn create_view_rejects_a_join_on_an_unknown_table() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let mut query = join_on_k();
        query.join.as_mut().unwrap().right = "nope".into();
        let initial = BTreeMap::from([
            ("t0".to_string(), ZSet::new()),
            ("t1".to_string(), ZSet::new()),
        ]);
        let err = IncrementalEngine::new()
            .create_view(&db, &query, &initial)
            .expect_err("the right table must be declared");
        assert!(err.0.contains("nope"), "{}", err.0);
    }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-core --locked`
Expected: a compile failure, `cannot find type Join` / `struct ViewQuery has no field named join`.

- [ ] **Step 3: Write the implementation**

In `crates/ivmlite-core/src/query.rs`, add before `ViewQuery`:

```rust
/// A two-table inner equi-join (M1a Phase 3): `FROM <anchor> JOIN <right> ON
/// <anchor>.<left_column> = <right>.<right_column>`.
///
/// The left input is always the anchor table (`db.tables()[0]`). Every other
/// column index in the query — `group_by`, each `Agg::column`, the
/// `predicate` — refers to the **joined row**: the anchor's columns, then the
/// right table's.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub right: String,
    /// A column of the anchor table.
    pub left_column: usize,
    /// A column of the right table.
    pub right_column: usize,
}
```

Replace `ViewQuery`'s definition with:

```rust
/// The differential harness's query format.
///
/// This is **not** the product's IR: M1b's SQL front end lowers straight from
/// `sqlparser` to `Plan` without going through it. It only has to cover the
/// query shapes v0 supports, so a join is one optional field rather than a
/// tree; replacing it with a tree once the shapes multiply (multi-table joins,
/// filters below a join) changes only test-side code.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// v0 allows only bare columns as group-by keys, not expressions (spec §7.1).
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
    /// `None` for a single-table view. `serde(default)` keeps the frozen
    /// regression fixtures, written before joins existed, loadable.
    #[cfg_attr(feature = "serde", serde(default))]
    pub join: Option<Join>,
}
```

In `lib.rs`, change the query export to `pub use query::{Agg, AggFn, Join, Predicate, ViewQuery};`.

Add `join: None` to every existing `ViewQuery { ... }` literal. The compiler lists them all; today they are in `ivmlite-core` (`query.rs`, `plan.rs`, `node.rs`, `engine.rs`) and `ivmlite-test` (`buggy.rs`, `differential.rs`, `invariants.rs`, `naive.rs`, `oracle.rs`, `query.rs`, `sql.rs`, `tests/harness_catches_bugs.rs`). In `plan.rs`'s test helper `q(...)`, add `join: None` once.

In `crates/ivmlite-core/src/plan.rs`, replace `lower` from its signature to the end of the bounds/type checks with the following. The dedup/`keep`/`remap` part is **unchanged**. The final tree construction changes only in how the bottom node is built, shown after:

```rust
/// Lower the harness's flat `ViewQuery` into an operator tree, enforcing §5.2's legality checks at the boundary.
///
/// This is the first place a `ViewQuery` crosses outside `enumerate`: the
/// engine consumes it directly. `enumerate` never produces an illegal shape,
/// but a `ViewQuery` can be constructed freely, so the checks must live here.
///
/// The view's left (or only) input is the anchor, `db.tables()[0]`. In M1b the
/// table order comes from `__ivm_dep`, not from the query's FROM clause —
/// choosing "the first" is only v0's convention (M1a Phase 2 final review
/// Finding I). A join's right input is looked up by name in `db`.
///
/// Column indices are checked against the row the operators above the scans
/// see — the anchor's columns, then (for a join) the right table's — and
/// their types drive the type checks: v0 supports `SUM` and `IntGt` over
/// INTEGER columns only, and join keys of one type only.
pub fn lower(query: &ViewQuery, db: &Database) -> Result<Plan, PlanError> {
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| PlanError("the database has no tables".into()))?;
    let right = match &query.join {
        None => None,
        Some(join) => Some(resolve_join(join, anchor, db)?),
    };
    let columns: Vec<&Column> = anchor
        .columns
        .iter()
        .chain(right.into_iter().flat_map(|s| s.columns.iter()))
        .collect();
    let arity = columns.len();
    let source = match right {
        None => format!("table {}", anchor.table),
        Some(r) => format!("the joined row of {} and {}", anchor.table, r.table),
    };
```

Keep the `group_by.is_empty()`, `aggs.is_empty()` and `SUM`-without-column checks exactly as they are. Change the bounds-check closure's message to use `source`:

```rust
    let check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} references column index {c}, but {source} has only {arity} columns"
            )))
        } else {
            Ok(())
        }
    };
```

In `require_integer`, change `let col = &schema.columns[c];` to `let col = columns[c];`. Replace the bottom-node construction (`let mut node = Plan::Scan { ... };`) with:

```rust
    // Each Scan takes every column of its table: Filter's predicate is
    // evaluated against the joined (or base-table) row, and narrowing happens
    // after Filter.
    let scan = |s: &Schema| Plan::Scan {
        table: s.table.clone(),
        columns: (0..s.arity()).collect(),
    };
    let mut node = match (&query.join, right) {
        (Some(join), Some(r)) => Plan::Join {
            left: Box::new(scan(anchor)),
            right: Box::new(scan(r)),
            left_key: join.left_column,
            right_key: join.right_column,
        },
        _ => scan(anchor),
    };
```

Add after `lower`:

```rust
/// Validate a join against the database and return its right table.
fn resolve_join<'a>(join: &Join, anchor: &Schema, db: &'a Database) -> Result<&'a Schema, PlanError> {
    if join.right == anchor.table {
        return Err(PlanError(format!(
            "join: a self-join of table {} is not supported in v0 — Scan routes each \
             delta by table name, so both inputs would receive every change to the table",
            anchor.table
        )));
    }
    let right = db.get(&join.right).ok_or_else(|| {
        PlanError(format!("join: table {} is not in the database", join.right))
    })?;
    if join.left_column >= anchor.arity() {
        return Err(PlanError(format!(
            "join: left_column {} is out of range, table {} has only {} columns",
            join.left_column,
            anchor.table,
            anchor.arity()
        )));
    }
    if join.right_column >= right.arity() {
        return Err(PlanError(format!(
            "join: right_column {} is out of range, table {} has only {} columns",
            join.right_column,
            right.table,
            right.arity()
        )));
    }
    let (l, r) = (
        &anchor.columns[join.left_column],
        &right.columns[join.right_column],
    );
    if l.ty != r.ty {
        return Err(PlanError(format!(
            "join: the key columns must have the same type, but {}.{} is {:?} and {}.{} is {:?} \
             — SQLite compares an INTEGER column with a TEXT column under numeric affinity, \
             so '7' = 7 is true there, while v0 compares values exactly",
            anchor.table, l.name, l.ty, right.table, r.name, r.ty
        )));
    }
    Ok(right)
}
```

Change the `use` line at the top of `plan.rs` to `use crate::{Agg, AggFn, Column, ColumnType, Database, Join, Predicate, Schema, ViewQuery};`.

Update existing call sites of `lower`:
- `plan.rs` tests: `lower(&x, &ints(2))` → `lower(&x, &Database::single(ints(2)))`, and likewise for `int_then_text()`.
- `node.rs` tests: `lower(&query, &ints3())` → `lower(&query, &crate::Database::single(ints3()))`, and likewise for `text_then_int()`.
- `engine.rs` `create_view`: delete the `let anchor = ...` block (its comment has moved into `lower`'s doc) and write `let plan = lower(query, db).map_err(|e| EngineError(e.0))?;`. The `db.tables()` bootstrap loop is unchanged.

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes, including `saved_regressions_still_pass` (the fixtures load without a `join` field because of `serde(default)`).

- [ ] **Step 5: Mutation verification**

1. Delete the self-join check in `resolve_join` → expect `a_self_join_is_rejected` to go red
2. Replace `db.get(&join.right).ok_or_else(...)?` with `&db.tables()[1]` → expect `a_join_on_an_unknown_table_is_rejected` and `create_view_rejects_a_join_on_an_unknown_table` to go red
3. Delete the `left_column` range check → expect `an_out_of_range_left_join_key_is_rejected` to go red
4. Delete the `right_column` range check → expect `an_out_of_range_right_join_key_is_rejected` to go red
5. Delete the key-type check → expect `join_keys_of_different_types_are_rejected` to go red
6. Make the bottom node always `scan(anchor)` (ignoring the join) → expect `lowers_a_join_to_two_scans_under_a_join`, `a_join_view_bootstraps_from_both_tables` and `a_join_view_follows_changes_to_both_tables` to go red
7. Remove `#[cfg_attr(feature = "serde", serde(default))]` from `ViewQuery::join` → expect `saved_regressions_still_pass` (and the other regression-replay tests) to go red, because the fixtures have no `join` field

- [ ] **Step 6: Register the gates and commit**

Add the 7 rows above to `docs/mutation-gates.md` (rows 1–6 in `ivmlite-core`; row 7 in `ivmlite-test`, requirement §9.4 "frozen regression cases keep loading"), each `**verified**` with measured numbers. Spec requirements: Ruling 4 and §6.1's operand-type rule (row 5), the v0 join scope (rows 1–4), §5.2 lowering (row 6). Do **not** yet touch the "When join lands" checklist; Task 5 does that.

```bash
git add crates/ivmlite-core/src/query.rs crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-test/src/buggy.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/invariants.rs crates/ivmlite-test/src/naive.rs crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/sql.rs crates/ivmlite-test/tests/harness_catches_bugs.rs docs/mutation-gates.md
git commit -m "feat(core): lower two-table equi-joins, and run join views in the engine

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If the compiler lists a `ViewQuery` literal in a file not named here, add that file too — and nothing else: check `git status` before committing.

---

## Task 3: The test side — join SQL, the oracle, `NaiveRecompute`, the join query space, key overlap

**Files:**
- Modify: `crates/ivmlite-test/src/sql.rs`, `oracle.rs`, `naive.rs`, `query.rs`, `differential.rs`, `lib.rs`
- Modify: `crates/ivmlite-test/tests/harness_catches_bugs.rs` (one assertion in `shrink_reduces_initial_rows_in_every_table_of_a_multi_table_case`, one doc comment)

**Interfaces:**
- Consumes: `Join`, `ViewQuery::join` (Task 2)
- Produces:
  - `view_query_to_sql(query: &ViewQuery, db: &Database) -> String` (was `(&ViewQuery, &Schema)`); every column is rendered qualified, `"table"."column"`
  - `enumerate_join(left: &Schema, right: &Schema) -> Vec<ViewQuery>`
  - `enumerate_database(db: &Database) -> Vec<ViewQuery>`: the anchor's single-table queries first, then (when `db` has at least two tables) `enumerate_join(tables[0], tables[1])`
  - `gen_case` picks its query from `enumerate_database(db)` by `seed % len`

> **Seed-to-query mapping.** For a single-table `Database`, `enumerate_database` equals `enumerate(anchor)`, so every single-table test keeps exactly the queries it had. For `gen_database(2)`, seeds `0..36` still map to the same 36 single-table queries (both of `gen_database`'s columns are nullable, so `enumerate` yields 3 group-bys × 3 aggregate sets × 4 predicates), and seeds from 36 up now map to join queries. `a_two_table_case_runs_green_against_the_reference_engine` (seeds `0..50`) therefore starts running 14 join queries through `NaiveRecompute`. That is what makes the non-anchor gaps of the checklist observable in Task 5.

- [ ] **Step 1: Write the failing tests**

In `sql.rs`'s tests, update the three existing expected strings to qualified names:
- `to_sql_renders_group_by_and_aggs`: `"SELECT \"orders\".\"region\", SUM(\"orders\".\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"orders\".\"region\""`
- `to_sql_renders_predicate`: contains `"WHERE \"orders\".\"amount\" > 3"`
- `to_sql_renders_is_not_null_predicate`: contains `"WHERE \"orders\".\"region\" IS NOT NULL"`

and change each call to `view_query_to_sql(&q, &Database::single(orders()))`. Add:

```rust
    #[test]
    fn to_sql_renders_a_join() {
        let kv = |name: &str| Schema {
            table: name.into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
                Agg {
                    func: AggFn::Sum,
                    column: Some(3),
                },
            ],
            predicate: Predicate::None,
            join: Some(Join {
                right: "t1".into(),
                left_column: 0,
                right_column: 0,
            }),
        };
        assert_eq!(
            view_query_to_sql(&q, &db),
            "SELECT \"t0\".\"k\", COUNT(*), SUM(\"t1\".\"v\") FROM \"t0\" JOIN \"t1\" \
             ON \"t0\".\"k\" = \"t1\".\"k\" GROUP BY \"t0\".\"k\""
        );
    }
```

(`use ivmlite_core::{Agg, Column, Database, Join};` in the test module.)

In `oracle.rs`'s tests, add (using the existing `Row` / `Value` imports and a local `kv` schema helper written the same way as above):

```rust
    #[test]
    fn a_join_query_is_computed_by_sqlite() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: &str, v: i64| Row::new(vec![Value::Text(k.into()), Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(r("a", 1), 1), (r("b", 2), 1)])),
            ("t1".to_string(), ZSet::from_rows([(r("a", 10), 1), (r("a", 20), 1)])),
        ]);
        let got = recompute_via_sqlite(&db, &join_on_k(), &bases).unwrap();
        assert_eq!(
            got,
            ZSet::from_rows([(
                Row::new(vec![Value::Text("a".into()), Value::Int(2), Value::Int(30)]),
                1
            )])
        );
    }

    #[test]
    fn sqlite_never_matches_null_join_keys() {
        // The semantics `JoinState` and `NaiveRecompute` copy (the plan's Ruling 4).
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let null_key = |v: i64| Row::new(vec![Value::Null, Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(null_key(1), 1)])),
            ("t1".to_string(), ZSet::from_rows([(null_key(10), 1)])),
        ]);
        assert!(recompute_via_sqlite(&db, &join_on_k(), &bases)
            .unwrap()
            .is_empty());
    }
```

where `join_on_k()` is the query from `to_sql_renders_a_join` (define it once in the oracle test module).

In `naive.rs`'s tests, add the same two cases against `NaiveRecompute` — `joins_on_the_key_and_aggregates_the_joined_rows` (materialize equals the `(a, 2, 30)` row) and `null_join_keys_never_match` (materialize is empty) — creating the view with `create_view(&db, &join_on_k(), &bases)`.

In `query.rs`'s tests, add:

```rust
    fn kv(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        }
    }

    #[test]
    fn enumerate_join_pairs_only_same_typed_keys() {
        let (l, r) = (kv("t0"), kv("t1"));
        let qs = enumerate_join(&l, &r);
        assert!(!qs.is_empty());
        for q in &qs {
            let j = q.join.as_ref().expect("every join query has a join");
            assert_eq!(j.right, "t1");
            assert_eq!(l.columns[j.left_column].ty, r.columns[j.right_column].ty);
        }
    }

    #[test]
    fn enumerate_join_groups_across_the_table_boundary() {
        // Spec §9.2: multi-column group keys that cross the boundary between
        // the two tables are exactly where joins are most likely to have bugs.
        let qs = enumerate_join(&kv("t0"), &kv("t1"));
        assert!(qs.iter().any(|q| q.group_by.iter().any(|&c| c < 2)
            && q.group_by.iter().any(|&c| c >= 2)));
    }

    #[test]
    fn enumerate_database_adds_join_queries_only_with_two_tables() {
        let one = Database::single(kv("t0"));
        assert_eq!(enumerate_database(&one), enumerate(&kv("t0")));
        let two = Database::new(vec![kv("t0"), kv("t1")]);
        let all = enumerate_database(&two);
        let single = enumerate(&kv("t0"));
        assert_eq!(&all[..single.len()], &single[..], "single-table queries come first");
        assert_eq!(all.len(), single.len() + enumerate_join(&kv("t0"), &kv("t1")).len());
    }
```

In `differential.rs`'s tests, add (with `use crate::{enumerate_join, Join};` as needed):

```rust
    /// The share of seeds whose initial state gives both tables at least one
    /// common non-NULL key value on column 0.
    const MIN_SEEDS_WITH_INITIAL_KEY_OVERLAP_PERCENT: usize = 90;
    /// The share of batches in which both tables change rows that share a
    /// non-NULL key value on column 0 — the batches that exercise ΔR⋈ΔS.
    const MIN_BATCHES_WITH_SHARED_DELTA_KEY_PERCENT: usize = 5;

    fn keys(rows: &[Row]) -> std::collections::BTreeSet<Value> {
        rows.iter()
            .map(|r| r.get(0).clone())
            .filter(|v| *v != Value::Null)
            .collect()
    }

    fn join_on_column_0(db: &Database) -> ViewQuery {
        enumerate_join(&db.tables()[0], &db.tables()[1])
            .into_iter()
            .find(|q| q.join.as_ref().is_some_and(|j| j.left_column == 0 && j.right_column == 0))
            .expect("the join space includes the k = k join")
    }

    #[test]
    fn join_keys_overlap_in_the_initial_state() {
        // A join test is only as good as its matches: if the two tables drew
        // their keys from disjoint value sets, every join would be empty and
        // every join test trivially green.
        let db = gen_database(2);
        let seeds = 50;
        let overlapping = (0..seeds)
            .filter(|&seed| {
                let case = gen_case_with_query(seed, &db, &Domain::default(), join_on_column_0(&db), 25, 0, Batching::All);
                !keys(&case.initial["t0"]).is_disjoint(&keys(&case.initial["t1"]))
            })
            .count();
        assert!(
            overlapping * 100 >= seeds as usize * MIN_SEEDS_WITH_INITIAL_KEY_OVERLAP_PERCENT,
            "only {overlapping} of {seeds} seeds have overlapping join keys"
        );
    }

    #[test]
    fn join_cases_exercise_the_delta_cross_term() {
        // ΔR⋈ΔS is non-empty only when both tables change rows with the same
        // key in the same batch. Without such batches, dropping that term from
        // the join would go unnoticed by the differential harness.
        let db = gen_database(2);
        let (mut shared, mut total) = (0usize, 0usize);
        for seed in 0..50 {
            let case = gen_case_with_query(seed, &db, &Domain::default(), join_on_column_0(&db), 25, 150, Batching::Chunks(5));
            for batch in case.batches() {
                total += 1;
                let side = |t: &str| {
                    let rows: Vec<Row> = batch.get(t).map_or(Vec::new(), |d| d.iter().map(|(r, _)| r.clone()).collect());
                    keys(&rows)
                };
                if !side("t0").is_disjoint(&side("t1")) {
                    shared += 1;
                }
            }
        }
        assert!(
            shared * 100 >= total * MIN_BATCHES_WITH_SHARED_DELTA_KEY_PERCENT,
            "only {shared} of {total} batches change both tables on a shared key"
        );
    }

    #[test]
    fn gen_case_picks_join_queries_for_two_table_databases() {
        // Checklist items 1 and 2 (non-anchor state reaching the engine and the
        // oracle) are observable only through join queries, and the tests that
        // observe them take their query from `gen_case`.
        let db = gen_database(2);
        let joins = (0..50)
            .filter(|&seed| {
                gen_case(seed, &db, &Domain::default(), 5, 0, Batching::All)
                    .query
                    .join
                    .is_some()
            })
            .count();
        assert!(joins > 0, "no seed in 0..50 picked a join query");
    }

    #[test]
    fn naive_engine_passes_every_enumerated_join_query() {
        let db = gen_database(2);
        let domain = Domain::default();
        for (i, query) in enumerate_join(&db.tables()[0], &db.tables()[1]).into_iter().enumerate() {
            let case = gen_case_with_query(i as u64, &db, &domain, query, 20, 60, Batching::Chunks(4));
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} failed at {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }
```

Rewrite `batch_invariance_holds_for_naive_engine_on_a_two_table_case` to use `gen_case_with_query(4343, &db, &domain, join_on_column_0(&db), 30, 200, Batching::All)`, and replace its doc comment's second paragraph ("To be honest about what this test can and cannot guard…") with: "With a join query, the non-anchor table's deltas reach both `materialize()` and the oracle, so this test now proves that they take part in the batch-independence comparison — the gap registered under I4 and item 3 of the join-landing checklist."

In `harness_catches_bugs.rs`, in `shrink_reduces_initial_rows_in_every_table_of_a_multi_table_case`, right after the `assert_eq!(case.database.len(), 2, ...)`, add:

```rust
    // The "t1 should shrink to 0 rows" argument below holds only for a
    // single-table query: with a join, t1 is observable. `gen_case` maps seeds
    // below the single-table query count to single-table queries, so this
    // guards against that mapping ever changing silently.
    assert!(
        case.query.join.is_none(),
        "this test's reasoning needs a single-table query, got {:?}",
        case.query
    );
```

Update the doc comment of `a_two_table_case_runs_green_against_the_reference_engine`: its second bullet ("`NaiveRecompute`'s per-table base / pending: **not caught**…") becomes "caught once join queries are in the seed range (seeds 36 and up are join queries since M1a Phase 3); see item 1 of the join-landing checklist in `docs/mutation-gates.md`". Its first paragraph's "(join is in the engine plan's Phase 3)" becomes "(seeds 0–35 are single-table queries; seeds 36 and up are join queries)".

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-test --locked --no-fail-fast`
Expected: compile failures, `cannot find function enumerate_join` / mismatched `view_query_to_sql` arguments.

- [ ] **Step 3: Write the implementation**

`crates/ivmlite-test/src/sql.rs` — replace `view_query_to_sql` with:

```rust
/// Render a ViewQuery as SQL against `db`.
///
/// Column indices refer to the query's row: the anchor table's (`db.tables()[0]`)
/// columns, then — for a join — the right table's. Every column is qualified
/// as `"table"."column"`, because the harness's tables share column names.
///
/// # Panics
/// If the query joins a table `db` does not declare. Only the oracle and the
/// harness's diff message call this, on queries `create_view` has already
/// accepted or that the enumerator produced.
pub fn view_query_to_sql(query: &ViewQuery, db: &Database) -> String {
    let anchor = &db.tables()[0];
    let right = query.join.as_ref().map(|j| {
        db.get(&j.right)
            .unwrap_or_else(|| panic!("the query joins {}, which the database does not declare", j.right))
    });
    let mut columns: Vec<(&str, &str)> = anchor
        .columns
        .iter()
        .map(|c| (anchor.table.as_str(), c.name.as_str()))
        .collect();
    if let Some(r) = right {
        columns.extend(r.columns.iter().map(|c| (r.table.as_str(), c.name.as_str())));
    }
    let name = |i: usize| format!("\"{}\".\"{}\"", columns[i].0, columns[i].1);

    let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();
    for agg in &query.aggs {
        select.push(match (agg.func, agg.column) {
            (AggFn::Count, _) => "COUNT(*)".to_string(),
            (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
            (AggFn::Sum, None) => panic!("SUM must name a column"),
        });
    }

    let from = match (&query.join, right) {
        (Some(j), Some(r)) => format!(
            "\"{}\" JOIN \"{}\" ON {} = {}",
            anchor.table,
            r.table,
            name(j.left_column),
            name(anchor.arity() + j.right_column)
        ),
        _ => format!("\"{}\"", anchor.table),
    };

    let where_clause = match &query.predicate {
        Predicate::None => String::new(),
        Predicate::IntGt { column, value } => format!(" WHERE {} > {}", name(*column), value),
        Predicate::IsNotNull { column } => format!(" WHERE {} IS NOT NULL", name(*column)),
    };

    let group = query
        .group_by
        .iter()
        .map(|i| name(*i))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "SELECT {} FROM {}{} GROUP BY {}",
        select.join(", "),
        from,
        where_clause,
        group
    )
}
```

with `use ivmlite_core::{AggFn, ColumnType, Database, Predicate, Schema, ViewQuery};`.

`oracle.rs`: replace the anchor lookup and rendering at the end of `recompute_via_sqlite` with:

```rust
    if db.is_empty() {
        return Err(EngineError("database has no tables".into()));
    }
    let sql = view_query_to_sql(query, db);
```

and update the stale comments: delete "View SQL is still rendered for a single table; …Phase 3." In `builds_every_table_in_the_database`, "otherwise join queries (Phase 3) would silently be one table short" becomes "otherwise join queries would silently be one table short".

`differential.rs`: in `run`'s `compare` closure, replace the `anchor` lookup and `view_query_to_sql(&case.query, anchor)` with `view_query_to_sql(&case.query, &case.database)`. In `gen_case`, replace the `enumerate(anchor)` lookup with `let queries = enumerate_database(db);` (keep the non-empty `expect`), and replace its doc comment's middle sentence ("the query is still a single-table aggregate — …which this follows.") with "the query is picked from `enumerate_database(db)` by `seed % len`: single-table queries over the anchor first, then — with two or more tables — the join queries over the first two." Update the `use crate::{...}` line: `enumerate` → `enumerate_database`.

`query.rs`: add after `enumerate`:

```rust
/// Enumerate v0's two-table join space: every pair of same-typed key columns,
/// crossed with the single-table dimensions (group-by, aggregates, predicates)
/// over the joined row — `left`'s columns, then `right`'s.
///
/// Keys of different types are never generated: `lower` rejects them (SQLite
/// would compare them under numeric affinity).
pub fn enumerate_join(left: &Schema, right: &Schema) -> Vec<ViewQuery> {
    // The joined row, as a schema, so the single-table enumerator can supply
    // the other dimensions. Its name and column names are never rendered.
    let joined = Schema {
        table: "joined".into(),
        columns: left.columns.iter().chain(right.columns.iter()).cloned().collect(),
    };
    let shapes = enumerate(&joined);
    let mut out = Vec::new();
    for (left_column, l) in left.columns.iter().enumerate() {
        for (right_column, r) in right.columns.iter().enumerate() {
            if l.ty != r.ty {
                continue;
            }
            for shape in &shapes {
                out.push(ViewQuery {
                    join: Some(Join {
                        right: right.table.clone(),
                        left_column,
                        right_column,
                    }),
                    ..shape.clone()
                });
            }
        }
    }
    out
}

/// Every query `gen_case` can pick for `db`: the anchor's single-table queries
/// first, then, when `db` has at least two tables, the join queries over its
/// first two.
pub fn enumerate_database(db: &Database) -> Vec<ViewQuery> {
    let tables = db.tables();
    let mut out = enumerate(&tables[0]);
    if tables.len() >= 2 {
        out.extend(enumerate_join(&tables[0], &tables[1]));
    }
    out
}
```

with `use crate::{Agg, AggFn, ColumnType, Predicate, Schema, ViewQuery};` extended by `use ivmlite_core::{Database, Join};`.

`lib.rs`: `pub use query::{enumerate, enumerate_database, enumerate_join};` and add `Database, Join` to the `pub use ivmlite_core::{...}` list.

`naive.rs`: in `materialize`, replace the line `let anchor_base = self.base.get(&self.anchor).cloned().unwrap_or_default();` with:

```rust
        let rows = self.input_rows(query);
```

change the loop to `for (row, weight) in rows.iter() {`, and add to `impl NaiveRecompute`:

```rust
    /// The rows the view aggregates: the anchor's base state, or — for a join —
    /// every matching pair of anchor and right rows, concatenated, with the
    /// product of their weights.
    ///
    /// A NULL key never matches: SQL's `NULL = NULL` is UNKNOWN (measured in
    /// SQLite; `sqlite_never_matches_null_join_keys` pins it on the oracle side).
    fn input_rows(&self, query: &ViewQuery) -> ZSet {
        let anchor = self.base.get(&self.anchor).cloned().unwrap_or_default();
        let Some(join) = &query.join else {
            return anchor;
        };
        let right = self.base.get(&join.right).cloned().unwrap_or_default();
        let mut joined = ZSet::new();
        for (l, wl) in anchor.iter() {
            for (r, wr) in right.iter() {
                let key = l.get(join.left_column);
                if *key != Value::Null && key == r.get(join.right_column) {
                    let mut values = l.0.clone();
                    values.extend(r.0.iter().cloned());
                    joined.update(Row::new(values), wl * wr);
                }
            }
        }
        joined
    }
```

Replace `NaiveRecompute`'s doc paragraph ending "…which this follows." with: "`materialize` aggregates the anchor's state (the first table in `db`), or for a join query the nested-loop join of the anchor and the right table — deliberately the simplest correct algorithm, sharing no code with `JoinState`."

- [ ] **Step 4: Run them to confirm they pass**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes. The two probe tests print nothing, so to record their measured shares for the gate row, temporarily add an `eprintln!` of `overlapping` / `shared` / `total`, run them with `-- --nocapture`, write the numbers down, and remove the `eprintln!` before committing.

- [ ] **Step 5: Mutation verification**

1. In `view_query_to_sql`, render unqualified names (`format!("\"{}\"", columns[i].1)`) → expect `to_sql_renders_a_join`, `a_join_query_is_computed_by_sqlite` and the join differential tests to go red (SQLite reports an ambiguous column name)
2. In `view_query_to_sql`, render the ON clause's right side as `name(j.right_column)` (forgetting the offset) → expect `to_sql_renders_a_join` to go red
3. In `NaiveRecompute::input_rows`, drop the `*key != Value::Null` condition → expect `null_join_keys_never_match` (naive) and `naive_engine_passes_every_enumerated_join_query` to go red
4. In `NaiveRecompute::input_rows`, use `wl` instead of `wl * wr` → expect `naive_engine_passes_every_enumerated_join_query` to go red
5. In `gen_row`, format TEXT values with the table name (`format!("{}{n}", schema.table)`) so the tables' key sets are disjoint → expect `join_keys_overlap_in_the_initial_state` and `join_cases_exercise_the_delta_cross_term` to go red
6. In `enumerate_join`, drop the `l.ty != r.ty` skip → expect `enumerate_join_pairs_only_same_typed_keys` to go red
7. In `gen_case`, pick from `enumerate(&db.tables()[0])` again (so two-table cases never get a join query) → expect `gen_case_picks_join_queries_for_two_table_databases` to go red

- [ ] **Step 6: Register the gates and commit**

All 7 rows go into the `ivmlite-test` table as `**verified**`. Row 7 matters beyond itself: without join queries in `gen_case`'s range, Task 5's checklist items 1 and 2 would have no observer. Row 5's requirement is the plan's "generator ensures join-key overlap", with the measured overlap and cross-term shares written into its cell.

```bash
git add crates/ivmlite-test/src/sql.rs crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/naive.rs crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/lib.rs crates/ivmlite-test/tests/harness_catches_bugs.rs docs/mutation-gates.md
git commit -m "feat(test): render and check join queries, and enumerate the join space

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 4: The incremental engine across the join space, and the measured cost

**Files:**
- Modify: `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md` (§9.2's enumeration table)

**Interfaces:**
- Consumes: `enumerate_join`, `gen_case_with_query`, `IncrementalEngine` with joins (Tasks 2–3)
- Produces: three integration tests; a measured row in spec §9.2

- [ ] **Step 1: Write the tests**

Add to `harness_catches_bugs.rs`, adding `enumerate_join` and `Join` to its `use ivmlite_test::{...}` list (Task 3 re-exports both):

```rust
/// The join counterpart of `incremental_engine_is_green_across_the_enumerated_space`:
/// every query of the join space, one case each, seeded by its index.
///
/// Under `IVMLITE_SEED=<n>` only query `n` runs, so the replay command a
/// `Failure` prints reproduces exactly the failing case.
#[test]
fn incremental_engine_is_green_across_the_join_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate_join(&db.tables()[0], &db.tables()[1]);
    let replay = std::env::var("IVMLITE_SEED").is_ok();
    let selected: Vec<u64> = if replay {
        seed_range()
    } else {
        (0..queries.len() as u64).collect()
    };
    for seed in selected {
        let query = queries[seed as usize % queries.len()].clone();
        let case = gen_case_with_query(seed, &db, &domain, query, 20, 60, Batching::Chunks(4));
        let mut engine = IncrementalEngine::new();
        if let Err(f) = run(&mut engine, &case) {
            panic!("the incremental engine disagrees with the oracle on join query {seed}: {f}");
        }
    }
}

/// A join query whose group key crosses the table boundary: group by
/// `t0.k, t1.v`, `SUM(t0.v)`, `COUNT(*)`, joined on `k = k`.
fn cross_boundary_join(db: &Database) -> ViewQuery {
    ViewQuery {
        group_by: vec![0, 3],
        aggs: vec![
            Agg {
                func: AggFn::Sum,
                column: Some(1),
            },
            Agg {
                func: AggFn::Count,
                column: None,
            },
        ],
        predicate: Predicate::None,
        join: Some(Join {
            right: db.tables()[1].table.clone(),
            left_column: 0,
            right_column: 0,
        }),
    }
}

/// The join counterpart of `incremental_engine_matches_naive_recompute_at_every_refresh_point`.
#[test]
fn incremental_engine_matches_naive_recompute_on_a_join_at_every_refresh_point() {
    let db = gen_database(2);
    let case = gen_case_with_query(
        11,
        &db,
        &Domain::default(),
        cross_boundary_join(&db),
        25,
        120,
        Batching::Chunks(5),
    );
    let bases: BTreeMap<String, ZSet> = case
        .initial
        .iter()
        .map(|(t, rows)| (t.clone(), ZSet::from_rows(rows.iter().map(|r| (r.clone(), 1)))))
        .collect();
    let mut inc = IncrementalEngine::new();
    let mut naive = NaiveRecompute::new();
    inc.create_view(&case.database, &case.query, &bases).unwrap();
    naive.create_view(&case.database, &case.query, &bases).unwrap();
    assert_eq!(
        inc.materialize().unwrap(),
        naive.materialize().unwrap(),
        "they already disagree at bootstrap"
    );
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

/// The join counterpart of `incremental_engine_satisfies_batch_invariance`:
/// the same delta sequence, batched four ways, must end in the same state.
#[test]
fn incremental_engine_satisfies_batch_invariance_on_joins() {
    let db = gen_database(2);
    let domain = Domain::default();
    // Spreads the 10 seeds across the join space instead of taking its first
    // ten queries, which all share one group-by.
    const BATCH_INVARIANCE_QUERY_STRIDE: usize = 67;
    let queries = enumerate_join(&db.tables()[0], &db.tables()[1]);
    for seed in seed_range().into_iter().take(10) {
        let query = queries[(seed as usize * BATCH_INVARIANCE_QUERY_STRIDE) % queries.len()].clone();
        let case = gen_case_with_query(seed, &db, &domain, query, 25, 120, Batching::All);
        check_batch_invariance(&case, IncrementalEngine::new).unwrap_or_else(|f| {
            panic!("the incremental engine should not violate batch independence on a join: {f}")
        });
    }
}
```

- [ ] **Step 2: Run them**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: everything passes. **If a join test goes red, the engine has a real bug — investigate it with the printed `IVMLITE_SEED`, do not loosen an assertion.**

- [ ] **Step 3: Measure the join sweep's cost**

```bash
cargo test -p ivmlite-test --test harness_catches_bugs --locked -- --exact incremental_engine_is_green_across_the_join_space
cargo test -p ivmlite-test --test harness_catches_bugs --locked -- --exact incremental_engine_is_green_across_the_enumerated_space
```

Each prints `finished in X.XXs`. Run each three times and take the median. Per-case cost = time / cases (the single-table sweep runs 50 cases; the join sweep runs `enumerate_join(..).len()` cases — print that length once). The measured multiplier is join per-case ÷ single-table per-case.

- [ ] **Step 4: Record the measurement in the spec**

In spec §9.2's enumeration table, replace the row `| Two tables, 2 columns each, + join | ~554 | ~10s (extrapolated) |` with the measured join-space size and the measured sweep time (debug build, `IncrementalEngine`). Replace the sentence "join cases are estimated at 2.5x — **that multiplier is an estimate, not a measurement**, and must be replaced by a measurement once join lands" with the measured multiplier and the date. Leave the three-column row marked as extrapolated, recomputed from the measured multiplier. **Do not decide the three-column question**: append to the open-question note one sentence saying the measurement now exists (M1a Phase 3) and the decision is still open.

If the join sweep takes more than 60 seconds, report DONE_WITH_CONCERNS with the numbers instead of reducing coverage — spec §9.2 says the group-by knob is the last one to turn.

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/tests/harness_catches_bugs.rs docs/superpowers/specs/2026-09-18-ivmlite-design.md
git commit -m "test: the incremental engine across the whole join space, with its measured cost

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 5: Close the join-landing checklist, and amend the spec

**Files:**
- Modify: `docs/mutation-gates.md`
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md`
- Modify: stale comments in `crates/ivmlite-core/src/engine.rs`, `crates/ivmlite-core/src/arrangement.rs`, `crates/ivmlite-test/src/test_support.rs`, `crates/ivmlite-test/src/differential.rs`, `crates/ivmlite-test/tests/harness_catches_bugs.rs`

**Interfaces:** none new.

> The checklist is `docs/mutation-gates.md`'s "Six items that must be revisited when join lands". Its own closing paragraph requires items 1, 2, 3, 4, 5a and 6 to be re-run, and a row may become "verified" only after its mutation really turns a test red. The predictions below are **predictions**; record what the run shows, including a prediction being wrong.

- [ ] **Step 1: Re-run the checklist's mutations**

For each, break → confirm it compiles → run `cargo test --workspace --locked --no-fail-fast` → record every red test → restore → confirm green.

1. **Item 1** — `NaiveRecompute::apply` returns `Ok(())` for `table != self.anchor`. Predicted red: `a_two_table_case_runs_green_against_the_reference_engine` (seeds 36 and up are join queries), `naive_engine_passes_every_enumerated_join_query` and `batch_invariance_holds_for_naive_engine_on_a_two_table_case`. Predicted green: `joins_on_the_key_and_aggregates_the_joined_rows`, which never calls `apply`.
2. **Item 2** — `run`'s `bases` bookkeeping updates only the anchor table. Predicted red: every join differential test (the engine's view is right, the oracle's `want` is computed from a stale right table). This is the "the oracle itself lies" item — it must not be skipped.
3. **Item 3** — the row's own mutation re-run: with items 1 and 2's mutations, confirm `batch_invariance_holds_for_naive_engine_on_a_two_table_case` now goes red (through `run`'s per-batch oracle comparison inside `check_batch_invariance`).
4. **Item 4** — `create_view`'s bootstrap loop over `db.tables().iter().take(1)`. Predicted red: `a_join_view_bootstraps_from_both_tables`, `a_join_view_follows_changes_to_both_tables`, the join sweep, and the existing `create_view_errors_when_a_declared_table_has_no_initial_state`.
5. **Item 5a** — the four public-contract `MemArrangement` mutations, now with a real consumer:
   - `get` returns at most one value (`.take(1)`): predicted red at the join level — `a_key_with_several_rows_on_the_other_side_matches_each` and the join sweep — in addition to its unit test.
   - Deleting the `if *w == 0 { ... }` removal: predicted **green** at the join level. The join only calls `get`, and a zombie entry of weight 0 contributes `w × 0 = 0`, which `ZSet::update` drops. Only the unit tests go red.
   - The two `scan()` ordering mutations: predicted **green** at the join level — the join never calls `scan`. Only the unit tests go red.
   Record per mutation which join-level tests, if any, went red. Do not add tests to force a red that the join cannot observe; the unit tests remain the guard for those three.
6. **Item 6** — `refresh`'s `by_table` becomes a `HashMap`. Predicted **green**, contradicting the checklist's own prediction: `JoinState::absorb` folds each side's delta in before the other side probes, so refreshing t0 then t1 computes `ΔR⋈S + (R+ΔR)⋈ΔS`, and t1 then t0 computes `R⋈ΔS + ΔR⋈(S+ΔS)` — both equal to the full bilinear formula. Because `HashMap` order varies per process, run the suite at least 10 times under the mutation. If it is green every time, the row stays n/a with this argument replacing the old one; if any run goes red, that is an engine bug to investigate, not a gate to record.

- [ ] **Step 2: Update the gate table**

For each checklist item, update its row's "Test that goes red" and status cells to the measured result: `**verified**` where a test went red, `n/a` with the measured reason where it did not (item 5a's three contract mutations stay `**verified**` through their unit tests, with a note of which ones join now observes; item 6 stays `n/a` with the corrected argument).

Then rewrite the "Six items that must be revisited when join lands" section into a short record: which items were closed by M1a Phase 3 and how (one line each, pointing at the rows), and what stays open — item 5b, until M1b decides whether its shadow-table arrangement needs an equivalent test. Keep the paragraph explaining why item 2 is the most dangerous; it is still true of any future multi-table shape.

Run `python3 scripts/count-mutation-gates.py --fix`, then bare, and confirm `consistent`.

- [ ] **Step 3: Amend the spec**

In `docs/superpowers/specs/2026-09-18-ivmlite-design.md`:

- **§13 item 9** becomes: "Joins are limited to a two-table inner equi-join on one pair of columns of the same type (M1a Phase 3). No outer joins, no self-joins, no more than two tables, no multi-column keys. A NULL key matches nothing. Filters are evaluated after the join."
- **§6.1**, after the operand-type subsection, add a subsection **"Join keys (amended 2026-09-23, M1a Phase 3)"** with the two measured SQLite results from this plan's Ruling 4, the rule (key types must match; rejected at `ivm_create_view`), and the NULL-key rule.
- **§5.2**, under the `Plan` listing, add one sentence: v0's `Join` carries one pair of column indices rather than `Vec<(Expr, Expr)>`, for the same reason `Expr` is not introduced elsewhere.
- **§4.4**'s M1a amendment, add a paragraph: the join's arrangements are passed in through `Node::build`'s provider (`JoinSide` tells the sides apart), so rebuilding a join from persisted state needs no interface change; moving `Aggregate`'s state onto `Arrangement` is deliberately **not** part of Phase 3 — it is a separate step after it, before M1b or as the M1b plan's first task — so that a join bug stays bisectable.

- [ ] **Step 4: Fix stale comments**

- `engine.rs`, the `by_table` comment ("It becomes observable once join lands… item 6 of the join-landing checklist"): replace with the measured Step 1 result for item 6 and its argument.
- `arrangement.rs`, test `one_key_can_hold_multiple_values`: "or Phase 3 would have to change the whole…" → "which `JoinState` relies on".
- `test_support.rs`'s module doc: "so that when Phase 3 first builds genuine two-table cases, it edits one place rather than five" → "so that multi-table cases are built in one place rather than five".
- `differential.rs`: the doc comment near line 417 mentioning "for join (M1a Phase 3), which needs several base tables" — make it present tense.
- `harness_catches_bugs.rs`: in the doc comment of `a_two_table_case_runs_green_against_the_reference_engine`, the "to be re-verified once join lands" clause → the Step 1 result for item 1.

Then run `grep -rn "Phase 3\|once join lands\|when join lands" crates docs/mutation-gates.md` and make sure every remaining hit describes the present, not a future that has now happened.

- [ ] **Step 5: Verify and commit**

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked --no-fail-fast`.

```bash
git add docs/mutation-gates.md docs/superpowers/specs/2026-09-18-ivmlite-design.md crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/arrangement.rs crates/ivmlite-test/src/test_support.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/tests/harness_catches_bugs.rs
git commit -m "docs: close the join-landing checklist with measured results, and record v0's join scope in the spec

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review

**1. Spec coverage**

| Spec requirement | Where it lands |
|---|---|
| §6.1 bilinear rule `ΔR⋈S + R⋈ΔS + ΔR⋈ΔS`, state per side | Task 1 (`JoinState::absorb`; the cross-term test and mutation 4); Task 2's engine test for the engine-level cross term; Task 3's cross-term probe |
| §6.3 key → many values, `Box<dyn Arrangement>` | Task 1 (`a_key_with_several_rows_on_the_other_side_matches_each`; the trait is unchanged); Task 5 item 5a |
| §5.2 `Plan::Join` | Task 1 (variant), Task 2 (lowering) |
| §5.2 root is a non-empty-`GROUP BY` `Aggregate` | Unchanged; `lower`'s existing checks still run for joins |
| §6.1 operand types / NULL semantics | Task 2 (key-type rule, IntGt over the right table's TEXT); Task 1 and 3 (NULL keys) |
| §8.5 signatures unchanged | No task changes `apply` / `refresh` / `create_view` signatures |
| §9.1 oracle independence | Task 3: the oracle is SQLite rendering `JOIN ... ON`; `NaiveRecompute` shares no code with `JoinState` |
| §9.2 enumeration beats randomness; measure the join multiplier | Task 3 (`enumerate_join`), Task 4 (full sweep + measurement) |
| §9.2 item 4, 2 columns per table | Unchanged; the three-column decision stays open |
| §9.4 deterministic order | `JoinState` uses `MemArrangement` (`BTreeMap`); `enumerate_join` is deterministic; item 6 re-checked in Task 5 |
| §11 M1a: Join | Tasks 1–4 |
| The "When join lands" checklist | Task 5 |

**Known uncovered, on purpose**: outer joins, multi-table joins, self-joins, filter push-down, `Aggregate` on `Arrangement`, the three-column decision, and checklist item 5b (belongs to M1b).

**2. Placeholder scan**: no TBD / TODO. The places where the plan gives instructions instead of full code are mechanical and named exactly: adding `join: None` to existing literals, updating existing `lower` / `Node::build` / `view_query_to_sql` call sites, and the stale-comment edits of Task 5 Step 4. Gate-row numbers are left to the measurement by design.

**3. Type consistency**: `JoinSide`, `fresh_mem_arrangement(JoinSide) -> Box<dyn Arrangement>`, `JoinState::new(usize, usize, Box<dyn Arrangement>, Box<dyn Arrangement>)` and `absorb(&ZSet, &ZSet) -> ZSet` (Task 1) → `Node::build(&Plan, &mut dyn FnMut(JoinSide) -> Box<dyn Arrangement>)` (Task 1, used by Task 2's engine) → `Join { right: String, left_column: usize, right_column: usize }` and `lower(&ViewQuery, &Database)` (Task 2) → `view_query_to_sql(&ViewQuery, &Database)`, `enumerate_join(&Schema, &Schema)`, `enumerate_database(&Database)` (Task 3, used by Task 4). `Plan::Join`'s fields are `left_key` / `right_key` (indices into the children's output); `Join`'s are `left_column` / `right_column` (indices into the tables) — `lower` maps one to the other, and since both children are full-width `Scan`s the values are the same.

## Execution order

Tasks 1 → 5 depend strictly on each other. Task 5 must not start before Task 4 is green: its checklist re-runs rely on the join tests of Tasks 3–4 being the observers.
