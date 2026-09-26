# M1b Phase 3a: The SQLite Extension, One View End to End — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real loadable SQLite extension, `ivmlite-sqlite`, that maintains one view per base table — including a two-table join view — through create, capture, refresh, read, reopen, failed-and-retried refresh, and drop, verified by the differential harness against the loaded library.

**Architecture:** Operator state becomes fallible in core (`Arrangement`, the provider, `Node::build`/`delta` return `Result`), and `Plan` gains a canonical text used as each view's plan fingerprint. The extension is a `cdylib` built outside the Cargo workspace; an `ivm` virtual-table module compiles the view's SQL with `ivmlite-sql` against a catalog read from SQLite, keeps state in `__ivm_state_*` tables, captures base-table writes with triggers into `__ivm_delta_*` tables, and applies every refresh's changes in **one** SQL statement so they commit or roll back together. The test host loads the built library and runs the harness's sweeps and lifecycle scenarios against it.

**Tech Stack:** Rust 1.95; `rusqlite` 0.40 with `loadable_extension` + `vtab` in the extension and `bundled` + `load_extension` in the test host; SQLite 3.53 (bundled).

**Spec:** `docs/superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md` (the Phase 3a spec — binding for this plan) and its parent `docs/superpowers/specs/2026-09-18-ivmlite-design.md`.

**Preceding plan:** `docs/superpowers/plans/2026-09-24-m1b-phase2b-sql-front-end.md` (complete and merged into master).

## Global Constraints

- **English only** in everything committed: code, comments, error strings, docs, commit messages.
- `ivmlite-core` and `ivmlite-sql` must not depend on `rusqlite` or `libsqlite3-sys` (spec §4.2). All `unsafe` and FFI lives in `crates/ivmlite-sqlite` only; every `unsafe` block carries a `// SAFETY:` comment.
- `crates/ivmlite-sqlite` is **not** a workspace member; it builds with `scripts/build-extension.sh` (Phase 3a spec §2).
- Mutation-gate protocol: break the code, confirm it compiles, run the suite, sum `passed`/`failed` over every test binary, restore, confirm `git diff` shows no leftover. Record `N passed / M failed (baseline B/0)` and every red test, read off the terminal — never computed. **Before Task 3 lands**, the suite is `cargo test --workspace --locked --no-fail-fast`; **from Task 3 on**, it is `scripts/test-all.sh` (it rebuilds the extension, so a mutation anywhere reaches the extension tests; one run takes about 40 seconds). A row whose mutation cannot be run starts its Verified cell with `None — `. `python3 scripts/count-mutation-gates.py` must print `consistent`; delete `scripts/__pycache__` if it appears.
- Before every commit: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, the suite above all clean — and from Task 3 on also `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check` and `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`.
- Stage specific files, never `git add -A`. Never `--no-verify`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not loosen any existing test threshold. An extension test must fail, never skip, when the library is missing or stale.

---

## This plan's scope and the rulings behind it

**The code in this plan was prototyped and run before it was written down.** In a throwaway worktree the workspace plus the extension's own unit tests passed at 288 tests with fmt and clippy clean on both builds, every join query of both harness databases (2 × 900) ran green against the loaded extension, and the mutations named in Task 4's gate step were run and went red as stated. Treat the code as verified text to transcribe; where anything disagrees, the tests are the binding specification.

### Ruling 1: five tasks

1. The three front-end fixes the Phase 3a spec §7 carries (small, `ivmlite-sql` only).
2. Core: fallible state and `Plan::canonical` (Phase 3a spec §3). The harness keeps passing unchanged.
3. The extension crate, its build scripts and its own unit tests (§2, §4, §5).
4. The test host: `SqliteExtensionEngine`, the lifecycle scenarios and the differential sweeps against the loaded library (§6), CI, and the extension's gate rows.
5. Documentation: the parent spec, the mutation-gate protocol, the docs index.

### Ruling 2: one-statement apply, not a savepoint (Phase 3a spec §5, as amended)

Measured while prototyping: inside `xUpdate` a `SAVEPOINT` fails (`cannot open savepoint - SQL statements in progress`), and when `xUpdate` fails inside an explicit transaction SQLite does not undo the callback's own writes. So a refresh buffers every state write in memory (`BufferedArrangement` overlays them on later reads), writes all changes into the view's stage table, and applies them with one `UPDATE … SET armed = 1` whose trigger does the work. The prototype's failure tests — faults on the output table and on a join state table, in autocommit mode and inside a transaction — pass only because of this.

### Ruling 3: a broken view still opens, so it can be dropped (Phase 3a spec §5, as amended)

SQLite connects a virtual table before dropping it. If `xConnect` failed on a tampered plan or a missing state table, `DROP TABLE` would fail too and the view could never be removed. So `__ivm_view` stores the table declaration, `xConnect` always declares it, and a failed check is kept as the reason every read and refresh then reports. `xDestroy` works from `__ivm_dep` and the state tables' exact names alone.

### Ruling 4: the harness's view is named `ivm_view`

A view's name is its command column, so a result column may not share it; the harness's tables have a column `v`. The engine names its view `ivm_view`.

### Ruling 5: sweep strides come from measured cost

Debug build, per case: about 9 ms single-table and 14 ms join in memory, about 60 ms reopening a database file before every refresh. The in-memory single-table sweep runs all 153 queries; the join sweep every 5th query on each database; the reopening sweeps every 3rd single-table and every 25th join query. The whole extension test binary set runs in about 10 seconds.

### Out of scope

Everything the Phase 3a spec §1 assigns to Phase 3b: several views sharing a delta table, GC of consumed deltas, per-view progress on a shared table.

---

## File structure

| File | Task | Change |
|---|---|---|
| `crates/ivmlite-sql/src/source.rs`, `compile.rs`; `crates/ivmlite-test/tests/sql_front_end.rs` | 1 | the three front-end fixes and their tests |
| `crates/ivmlite-core/src/{arrangement,agg,join,node,engine,plan,test_support,lib}.rs` | 2 | fallible state; `Plan::canonical` |
| `Cargo.toml` (root) | 3 | `exclude = ["crates/ivmlite-sqlite"]` |
| `crates/ivmlite-sqlite/{Cargo.toml,Cargo.lock,src/*.rs}` | 3 | new: the extension |
| `scripts/build-extension.sh`, `scripts/test-all.sh` | 3 | new |
| `crates/ivmlite-test/{Cargo.toml,src/lib.rs,src/extension.rs}` | 4 | `load_extension`; `SqliteExtensionEngine` |
| `crates/ivmlite-test/tests/extension_{lifecycle,differential}.rs` | 4 | new |
| `.github/workflows/ci.yml` | 4 | build and test the extension |
| `docs/mutation-gates.md` | 1–5 | rows; the protocol |
| `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, `docs/README.md` | 5 | amendments; index |

---

### Task 1: Three front-end fixes

**Files:** Modify `crates/ivmlite-sql/src/source.rs`, `crates/ivmlite-sql/src/compile.rs`, `crates/ivmlite-test/tests/sql_front_end.rs`, `docs/mutation-gates.md`.

Measured against SQLite 3.53 (Phase 3a spec §7): an unaliased result name keeps trailing whitespace inside a line comment (`-- hi\r\n` leaves a `\r`, `-- hi  ` the spaces) where SQLite trims it; `1L` compiles as 1 where SQLite rejects the token; `FROM t0 GLOBAL JOIN t1` compiles where SQLite reads `GLOBAL` as an alias.

- [ ] **Step 1: The failing tests**

In `crates/ivmlite-test/tests/sql_front_end.rs`'s `result_column_names_match_sqlite`, add to the hand-written SQL list, after `"SELECT k, COUNT(*) -- )\nFROM t0 GROUP BY k",`:

```rust
            "SELECT k, COUNT(*) -- hi\r\nFROM t0 GROUP BY k",
            "SELECT k, COUNT(*) -- hi  \nFROM t0 GROUP BY k",
```

In `crates/ivmlite-sql/src/compile.rs`'s `everything_outside_the_subset_is_rejected_by_name`, add after the `LEFT OUTER JOIN` case:

```rust
            ("SELECT region, COUNT(*) FROM orders GLOBAL JOIN regions ON region = name GROUP BY region", "GLOBAL JOIN"),
```

and after the `amount > 1.5` case:

```rust
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1L GROUP BY region", "L suffix"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > -1L GROUP BY region", "L suffix"),
```

Run `cargo test -p ivmlite-sql --locked` and `cargo test -p ivmlite-test --locked --test sql_front_end`: both fail (RED).

- [ ] **Step 2: The fixes**

`crates/ivmlite-sql/src/source.rs`, at the end of `piece_text`, replace `sql.get(from..to).map(str::to_string).ok_or_else(malformed)` with:

```rust
    // SQLite trims trailing whitespace from the name, including whitespace a
    // trailing line comment's token carries (`-- hi  ` and the `\r` of a CRLF
    // line end; measured, 3.53). Its whitespace set is sqlite3Isspace's.
    sql.get(from..to)
        .map(|text| {
            text.trim_end_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
                .to_string()
        })
        .ok_or_else(malformed)
```

`crates/ivmlite-sql/src/compile.rs`, in `from_clause`: bind `global` instead of `global: _` in the `Join { … }` pattern, and as the first statement of that arm:

```rust
            // `FROM t0 GLOBAL JOIN t1`: sqlparser reads a GLOBAL join, but
            // SQLite reads `GLOBAL` as `t0`'s alias (measured, 3.53).
            reject_if(*global, "GLOBAL JOIN")?;
```

Add above `fn literal`:

```rust
/// `1L`: sqlparser accepts a long suffix that SQLite rejects as an
/// unrecognized token (measured, 3.53).
fn long_literal(text: &str) -> SqlError {
    unsupported(&format!(
        "the literal {text}L (SQLite does not accept an L suffix)"
    ))
}
```

and in `literal`, replace both `SqlValue::Number(text, _) => …` arms: the positive one with

```rust
            SqlValue::Number(text, false) => integer(text),
            SqlValue::Number(text, true) => Err(long_literal(text)),
```

and the negated one with

```rust
                SqlValue::Number(text, false) => integer(&format!("-{text}")),
                SqlValue::Number(text, true) => Err(long_literal(text)),
```

Run both test commands again: green. Full suite, fmt, clippy.

- [ ] **Step 3: Gate rows** (under `## ivmlite-sql`)

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| Phase 3a §7: an unaliased name has SQLite's trailing whitespace trimmed | In `piece_text`, drop the `trim_end_matches` call | `result_column_names_match_sqlite` |
| Phase 3a §7: `1L` is rejected | Map `SqlValue::Number(text, true)` to `integer(text)` in the positive arm | `everything_outside_the_subset_is_rejected_by_name` |
| Phase 3a §7: `GLOBAL JOIN` is rejected | Delete the `reject_if(*global, …)` line | `everything_outside_the_subset_is_rejected_by_name` |

- [ ] **Step 4: Commit**

```bash
git add crates/ivmlite-sql/src/source.rs crates/ivmlite-sql/src/compile.rs crates/ivmlite-test/tests/sql_front_end.rs docs/mutation-gates.md
git commit -m "fix(sql): trim result names as SQLite does; reject 1L and GLOBAL JOIN

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Fallible operator state, and the plan's canonical text

**Files:** Modify `crates/ivmlite-core/src/{arrangement,agg,join,node,engine,plan,test_support,lib}.rs`, `docs/mutation-gates.md`.

**Interfaces — produces (Tasks 3 and 4 rely on them):**
```rust
pub trait Arrangement {
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError>;
    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError>;
}
pub struct StateError(pub String);
pub type ArrangementProvider<'a> = dyn FnMut(ArrangementId) -> Result<Box<dyn Arrangement>, StateError> + 'a;
impl Node { pub fn build(plan: &Plan, arrangements: &mut ArrangementProvider) -> Result<Node, StateError>;
            pub fn delta(&mut self, table: &str, input: &ZSet) -> Result<ZSet, StateError>; }
impl Plan { pub fn canonical(&self) -> String; }
pub fn fresh_mem_arrangement(id: ArrangementId) -> Result<Box<dyn Arrangement>, StateError>;
```
`StateError` and `ArrangementProvider` are exported from `ivmlite_core`.

- [ ] **Step 1: `arrangement.rs`**

Replace everything above `#[cfg(test)]` with:

```rust
use std::collections::BTreeMap;

use crate::Row;

/// Spec §6.3: key → many (value, weight) pairs.
///
/// `get` returns every value of a key rather than an `Option`: v0's group-by
/// stores one value per key and has no use for many, but each side of a join
/// is key → many rows (§6.3 names this as the main place the rule "M0 may make
/// no decision that forces rework for join" lands).
///
/// Every method can fail (M1b Phase 3a): the SQLite implementation in
/// `ivmlite-sqlite` reads and writes shadow tables. Reads return a `Vec`
/// rather than a lazy iterator — an iterator borrowing a prepared statement
/// would cost lifetimes and `unsafe` for no gain at v0's sizes. The trait stays
/// object-safe: operators hold a `Box<dyn Arrangement>`.
pub trait Arrangement {
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError>;
    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError>;
}

/// Operator state could not be read or written, or what was read is corrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateError(pub String);

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StateError {}

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
    /// An aggregate's per-group state, keyed by the group key; see `AggState`.
    AggregateGroups,
}

/// The provider `IncrementalEngine` passes to `Node::build`: every
/// arrangement starts empty, in memory.
pub fn fresh_mem_arrangement(_id: ArrangementId) -> Result<Box<dyn Arrangement>, StateError> {
    Ok(Box::new(MemArrangement::new()))
}

/// M1a's in-memory implementation. M1b adds one backed by SQLite shadow tables.
///
/// Both levels are `BTreeMap`s: spec §9.4 requires every iteration order that
/// can affect output to be deterministic, and `scan()`'s order feeds straight
/// into the delta stream.
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
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        Ok(match self.inner.get(key) {
            Some(vals) => vals.iter().map(|(v, &w)| (v.clone(), w)).collect(),
            None => Vec::new(),
        })
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError> {
        if weight_delta == 0 {
            return Ok(());
        }
        let vals = self.inner.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        // Spec §5.1: remove on reaching zero, leaving no zombie entry. Once a
        // key's values are empty, remove the key too. Leaving it would not make
        // scan()'s output any larger — scan() expands each key's value set with
        // flat_map, and an empty set contributes zero records whether or not
        // its key is still in `inner`. What grows with history (every key used
        // and then emptied) rather than with the current state is `inner`'s own
        // entry count: a memory-footprint problem, not an output-size one, but
        // still worth fixing, or `inner` accumulates empty shells that are
        // never read again and scan()'s traversal cost only ever grows.
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                self.inner.remove(key);
            }
        }
        Ok(())
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        Ok(self
            .inner
            .iter()
            .flat_map(|(k, vals)| vals.iter().map(move |(v, &w)| (k.clone(), v.clone(), w)))
            .collect())
    }
}
```

- [ ] **Step 2: `agg.rs`**

Add `StateError` to the `use crate::{…}` line. Replace `GroupState::decode`, `AggState::load` and `AggState::store` with these, and change `absorb`'s signature to `pub fn absorb(&mut self, input: &ZSet) -> Result<ZSet, StateError>`, its `let old = self.load(&key);` to `let old = self.load(&key)?;`, its `self.store(&key, &old, &new);` to `self.store(&key, &old, &new)?;`, and its final `out` to `Ok(out)`:

```rust
    /// The inverse of `encode`.
    ///
    /// # Panics
    /// If `row` does not have `encode`'s layout for `aggs` accumulators. The
    /// in-memory engine only ever stores rows `encode` produced; a state loaded
    /// from a persisted table may not be, and M1b Phase 3 turns this panic into
    /// an error (the plan's Ruling 3).
    fn decode(row: &Row, aggs: usize) -> Result<GroupState, StateError> {
        if row.len() != 1 + 2 * aggs {
            return Err(StateError(format!(
                "corrupted aggregate state: expected {} values, found {row:?}",
                1 + 2 * aggs
            )));
        }
        let int = |i: usize| match row.get(i) {
            Value::Int(n) => Ok(*n),
            other => Err(StateError(format!(
                "corrupted aggregate state: value {i} is {other:?}, not an Int"
            ))),
        };
        let mut accs = Vec::with_capacity(aggs);
        for i in 0..aggs {
            accs.push(Acc {
                sum: int(1 + 2 * i)?,
                non_null: int(2 + 2 * i)?,
            });
        }
        Ok(GroupState {
            rows: int(0)?,
            accs,
        })
    }

    /// The group's stored state, or the empty state if it has none.
    fn load(&self, key: &Row) -> Result<GroupState, StateError> {
        match self.state.get(key)?.as_slice() {
            [] => Ok(GroupState::empty(self.aggs.len())),
            [(value, 1)] => GroupState::decode(value, self.aggs.len()),
            values => Err(StateError(format!(
                "corrupted aggregate state for group {key:?}: expected at most one value \
                 of weight 1, found {values:?}"
            ))),
        }
    }

    /// Replace the group's stored state `old` with `new`. A group whose state
    /// is all zero is not stored at all (spec §5.1: no zombie entries).
    ///
    /// The retraction and the insertion are two writes; if the second fails,
    /// the arrangement is left without this group's state. Callers that
    /// persist state must therefore make a failed batch roll back as a whole —
    /// `ivmlite-sqlite`'s refresh runs inside one savepoint (M1b Phase 3a).
    fn store(&mut self, key: &Row, old: &GroupState, new: &GroupState) -> Result<(), StateError> {
        if old == new {
            return Ok(());
        }
        if !old.is_empty() {
            self.state.update(key, &old.encode(), -1)?;
        }
        if !new.is_empty() {
            self.state.update(key, &new.encode(), 1)?;
        }
        Ok(())
    }
```

- [ ] **Step 3: `join.rs`**

Add `StateError` to the `use crate::{…}` line and replace `JoinState::absorb` with:

```rust
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
    pub fn absorb(&mut self, left_delta: &ZSet, right_delta: &ZSet) -> Result<ZSet, StateError> {
        let mut out = ZSet::new();

        // ΔR ⋈ S, against S from before this call.
        for (l, &wl) in left_delta.iter() {
            let Some(key) = join_key(l, self.left_key) else {
                continue;
            };
            for (r, wr) in self.right.get(&key)? {
                out.update(concat(l, &r), wl * wr);
            }
        }
        for (l, &wl) in left_delta.iter() {
            if let Some(key) = join_key(l, self.left_key) {
                self.left.update(&key, l, wl)?;
            }
        }

        // (R + ΔR) ⋈ ΔS: R⋈ΔS and ΔR⋈ΔS together.
        for (r, &wr) in right_delta.iter() {
            let Some(key) = join_key(r, self.right_key) else {
                continue;
            };
            for (l, wl) in self.left.get(&key)? {
                out.update(concat(&l, r), wl * wr);
            }
        }
        for (r, &wr) in right_delta.iter() {
            if let Some(key) = join_key(r, self.right_key) {
                self.right.update(&key, r, wr)?;
            }
        }

        Ok(out)
    }
```

- [ ] **Step 4: `node.rs`**

Replace the file's head (the `use` block before `/// The stateful operator tree.`) with:

```rust
use crate::{
    Arrangement, ArrangementId, ArrangementRole, JoinState, Plan, Predicate, Row, StateError,
    Value, ZSet,
};

/// Supplies an operator's arrangements by id: fresh in memory for
/// `IncrementalEngine`, backed by shadow tables in `ivmlite-sqlite`.
pub type ArrangementProvider<'a> =
    dyn FnMut(ArrangementId) -> Result<Box<dyn Arrangement>, StateError> + 'a;

```

Replace `Node::build` (with its doc comment — the old comment about `NodeError` is obsolete now that building can fail) and `Node::build_at` with:

```rust
    /// Build the operator tree from a `Plan`, recursively.
    ///
    /// `arrangements` supplies every arrangement an operator needs, asked for
    /// by `ArrangementId`: the operator's pre-order position in `plan` and the
    /// arrangement's role (M1b Phase 1, Ruling 2). The engine passes
    /// `fresh_mem_arrangement`; a provider that returns non-empty arrangements
    /// rebuilds a tree from persisted state. Building fails only when the
    /// provider does — for instance a shadow table that should exist and does
    /// not (M1b Phase 3a).
    pub fn build(plan: &Plan, arrangements: &mut ArrangementProvider) -> Result<Node, StateError> {
        let mut next = 0;
        Node::build_at(plan, &mut next, arrangements)
    }

    /// `build`'s recursion: `next` is the pre-order index the next plan node gets.
    fn build_at(
        plan: &Plan,
        next: &mut usize,
        arrangements: &mut ArrangementProvider,
    ) -> Result<Node, StateError> {
        let node = *next;
        *next += 1;
        Ok(match plan {
            Plan::Scan { table, .. } => Node::Scan {
                table: table.clone(),
            },
            Plan::Join {
                left,
                right,
                left_key,
                right_key,
            } => {
                let left = Node::build_at(left, next, arrangements)?;
                let right = Node::build_at(right, next, arrangements)?;
                Node::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    state: JoinState::new(
                        *left_key,
                        *right_key,
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinLeft,
                        })?,
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinRight,
                        })?,
                    ),
                }
            }
            Plan::Filter { input, predicate } => Node::Filter {
                input: Box::new(Node::build_at(input, next, arrangements)?),
                predicate: predicate.clone(),
            },
            Plan::Project { input, columns } => Node::Project {
                input: Box::new(Node::build_at(input, next, arrangements)?),
                columns: columns.clone(),
            },
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => {
                let input = Node::build_at(input, next, arrangements)?;
                Node::Aggregate {
                    input: Box::new(input),
                    state: crate::AggState::new(
                        group_by.clone(),
                        aggs.clone(),
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::AggregateGroups,
                        })?,
                    ),
                }
            }
        })
    }
```

and `Node::delta` with:

```rust
    /// Push one batch of deltas for some table through this node, returning the node's output delta.
    ///
    /// Spec §6.1: linear operators satisfy `Δ(f(R)) = f(ΔR)`, so Filter and
    /// Project are stateless and deltas pass straight through. It fails only
    /// when a stateful operator's arrangement does.
    pub fn delta(&mut self, table: &str, input: &ZSet) -> Result<ZSet, StateError> {
        Ok(match self {
            Node::Scan { table: own } => {
                if own == table {
                    input.clone()
                } else {
                    ZSet::new()
                }
            }
            Node::Join { left, right, state } => {
                // Each child is a `Scan` routing by table name, so for one
                // table's delta at most one of these is non-empty (v0 rejects
                // self-joins in `lower`); `JoinState::absorb` is correct either way.
                let left_delta = left.delta(table, input)?;
                let right_delta = right.delta(table, input)?;
                state.absorb(&left_delta, &right_delta)?
            }
            Node::Filter {
                input: child,
                predicate,
            } => {
                let upstream = child.delta(table, input)?;
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    if passes(predicate, row) {
                        out.update(row.clone(), w);
                    }
                }
                out
            }
            Node::Project {
                input: child,
                columns,
            } => {
                let upstream = child.delta(table, input)?;
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    // After narrowing a row may coincide with another —
                    // `ZSet::update` adds the weights and removes the entry on
                    // reaching zero, exactly the Z-set semantics needed.
                    let narrowed = Row::new(columns.iter().map(|&c| row.get(c).clone()).collect());
                    out.update(narrowed, w);
                }
                out
            }
            Node::Aggregate {
                input: child,
                state,
            } => {
                // Spec §6.2: aggregation is one of the engine's two stateful
                // operators (the join is the other). Compute the upstream
                // delta first, then let `AggState` decide what to retract and
                // what to emit.
                let upstream = child.delta(table, input)?;
                state.absorb(&upstream)?
            }
        })
    }
```

- [ ] **Step 5: `engine.rs`, `lib.rs`, `test_support.rs`**

`engine.rs`: add `StateError` to the `use crate::{…}` line; replace `CountingTree::delta` with the first block below and add the second above `/// v0's incremental engine.`; in `create_view` build the tree with `CountingTree::new(Node::build(&plan, &mut fresh_mem_arrangement).map_err(state_error)?)`; the two `tree.delta(…)` calls in `create_view` and `refresh` take a `?`.

```rust
    fn delta(&mut self, table: &str, input: &ZSet) -> Result<ZSet, EngineError> {
        self.pushes += 1;
        self.rows_fed += input.len();
        self.node.delta(table, input).map_err(state_error)
    }

/// `IncrementalEngine` keeps its arrangements in memory, where only a
/// corrupted aggregate state can fail; it is reported as an engine error.
fn state_error(e: StateError) -> EngineError {
    EngineError(format!("operator state: {e}"))
}
```

`lib.rs`: export `StateError` with the other `arrangement` items, and `pub use node::{ArrangementProvider, Node};`.

`test_support.rs` becomes (the `Mirrors` double now returns `Result`s, and `Failing` is new):

```rust
//! Test-only helpers for ivmlite-core's unit tests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::{
    Agg, AggFn, Arrangement, ArrangementId, Column, ColumnType, MemArrangement, Predicate, Row,
    Schema, StateError, Value, ViewQuery,
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
    pub(crate) fn arrangement(
        &self,
        id: ArrangementId,
    ) -> Result<Box<dyn Arrangement>, StateError> {
        Ok(Box::new(Mirrored {
            id,
            inner: MemArrangement::new(),
            mirrors: self.clone(),
        }))
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
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        self.inner.get(key)
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError> {
        self.inner.update(key, val, weight_delta)?;
        self.mirrors
            .0
            .borrow_mut()
            .entry(self.id)
            .or_default()
            .update(key, val, weight_delta)
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        self.inner.scan()
    }
}

/// An arrangement whose every read and write fails, as a shadow table that
/// SQLite cannot read would (M1b Phase 3a).
pub(crate) struct Failing;

impl Arrangement for Failing {
    fn get(&self, _key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        Err(StateError("injected read failure".into()))
    }

    fn update(&mut self, _key: &Row, _val: &Row, _weight_delta: i64) -> Result<(), StateError> {
        Err(StateError("injected write failure".into()))
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        Err(StateError("injected read failure".into()))
    }
}

/// `(k TEXT, v INTEGER)`, the harness's two-column shape.
pub(crate) fn kv(name: &str) -> Schema {
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

pub(crate) fn kv_row(k: &str, v: i64) -> Row {
    Row::new(vec![Value::Text(k.into()), Value::Int(v)])
}

/// `SELECT t0.k, COUNT(*), SUM(t1.v) FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t0.k`
pub(crate) fn join_on_k() -> ViewQuery {
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
```

- [ ] **Step 6: `Plan::canonical`**

In `plan.rs`, add `CmpOp` to the `use crate::{…}` line and insert, directly above `#[derive(Debug, Clone, PartialEq, Eq)]\npub struct PlanError`:

```rust
impl Plan {
    /// A text rendering of the plan that identifies it exactly and does not
    /// change with the Rust version: it is written out here by hand rather
    /// than taken from `Debug`.
    ///
    /// `ivmlite-sqlite` stores it next to a view's SQL and compares it with the
    /// rendering of a fresh `lower` on every reopen (M1b Phase 3a): the
    /// operators' `ArrangementId`s and the aggregate's state layout mean
    /// something only for one exact plan (spec §7), so a change to `lower` must
    /// fail loudly instead of reading the wrong shadow tables. Names are
    /// length-prefixed, so no table name can imitate the structure around it.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    fn write_canonical(&self, out: &mut String) {
        let list = |xs: &[usize]| {
            xs.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        match self {
            Plan::Scan { table, columns } => {
                out.push_str(&format!(
                    "scan({}:{table},[{}])",
                    table.len(),
                    list(columns)
                ));
            }
            Plan::Join {
                left,
                right,
                left_key,
                right_key,
            } => {
                out.push_str(&format!("join({left_key},{right_key},"));
                left.write_canonical(out);
                out.push(',');
                right.write_canonical(out);
                out.push(')');
            }
            Plan::Filter { input, predicate } => {
                out.push_str("filter(");
                out.push_str(&canonical_predicate(predicate));
                out.push(',');
                input.write_canonical(out);
                out.push(')');
            }
            Plan::Project { input, columns } => {
                out.push_str(&format!("project([{}],", list(columns)));
                input.write_canonical(out);
                out.push(')');
            }
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => {
                let aggs: Vec<String> = aggs
                    .iter()
                    .map(|a| match (a.func, a.column) {
                        (AggFn::Count, None) => "count".to_string(),
                        (AggFn::Count, Some(c)) => format!("count({c})"),
                        (AggFn::Sum, None) => "sum".to_string(),
                        (AggFn::Sum, Some(c)) => format!("sum({c})"),
                    })
                    .collect();
                out.push_str(&format!(
                    "aggregate([{}],[{}],",
                    list(group_by),
                    aggs.join(",")
                ));
                input.write_canonical(out);
                out.push(')');
            }
        }
    }
}

fn canonical_predicate(predicate: &Predicate) -> String {
    match predicate {
        Predicate::None => "none".to_string(),
        Predicate::Compare { column, op, value } => {
            let op = match op {
                CmpOp::Gt => "gt",
                CmpOp::Ge => "ge",
                CmpOp::Lt => "lt",
                CmpOp::Le => "le",
                CmpOp::Eq => "eq",
                CmpOp::Ne => "ne",
            };
            let value = match value {
                Value::Null => "null".to_string(),
                Value::Int(n) => format!("int:{n}"),
                Value::Text(t) => format!("text:{}:{t}", t.len()),
            };
            format!("{op}({column},{value})")
        }
        Predicate::IsNull { column } => format!("is_null({column})"),
        Predicate::IsNotNull { column } => format!("is_not_null({column})"),
    }
}

```

- [ ] **Step 7: The existing tests**

The compiler lists every test that still calls the old signatures. The changes are mechanical; make exactly these:

- a call that now returns `Result` gets `.unwrap()` **at the call**: `Node::build(…)`, `.delta(…)`, `.absorb(…)`, and an arrangement's `.get(…)`, `.scan()` and `.update(…)`. Never unwrap at a later use of the variable.
- a returned `Vec` replaces an iterator: `….unwrap().collect()` / `.collect::<Vec<_>>()` becomes `….unwrap()`, `.count()` becomes `.len()`, and `….map(…)` on it becomes `….unwrap().into_iter().map(…)`.
- an `assert_eq!` comparing an absorb/delta result with a plain `ZSet` compares the unwrapped value.
- a provider closure returns `Ok(…)`: `&mut |id| mirrors.arrangement(id)` (which now returns `Result`), and `|id| -> Result<Box<dyn crate::Arrangement>, crate::StateError> { Ok(Box::new(mirrors.snapshot(id))) }` in the tree-rebuild test.
- the two `#[should_panic(expected = "corrupted aggregate state")]` tests in `agg.rs` become error assertions — replace them with the three tests below (the third is new).

```rust
    #[test]
    fn a_group_with_two_stored_values_is_reported_as_corrupted() {
        // In memory this cannot happen; from a persisted table it can, so it
        // is a `StateError`, not a panic (M1b Phase 3a).
        let mut state = MemArrangement::new();
        let key = row(vec![txt("a")]);
        state
            .update(&key, &row(vec![int(1), int(5), int(1)]), 1)
            .unwrap();
        state
            .update(&key, &row(vec![int(1), int(6), int(1)]), 1)
            .unwrap();
        let mut s = sum_state_on(Box::new(state));
        let err = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .expect_err("two stored values for one group is corrupted state");
        assert!(err.0.contains("corrupted aggregate state"), "{err}");
    }
    #[test]
    fn a_group_whose_one_stored_value_has_weight_two_is_reported_as_corrupted() {
        // One value, but with weight 2: `store` only ever writes a group's
        // state with weight 1, so this too is corrupted state, not a group
        // whose state counts twice. The previous test cannot see this case:
        // it has two values, and `load` rejects it for that alone.
        let mut state = MemArrangement::new();
        state
            .update(&row(vec![txt("a")]), &row(vec![int(1), int(5), int(1)]), 2)
            .unwrap();
        let mut s = sum_state_on(Box::new(state));
        let err = s
            .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
            .expect_err("a stored value of weight 2 is corrupted state");
        assert!(err.0.contains("corrupted aggregate state"), "{err}");
    }
    #[test]
    fn a_stored_value_of_the_wrong_shape_is_reported_as_corrupted() {
        // What a shadow table written by another plan, or damaged, could hold:
        // too few values, or a TEXT where an accumulator should be.
        for bad in [
            row(vec![int(1), int(5)]),
            row(vec![int(1), txt("x"), int(1)]),
        ] {
            let mut state = MemArrangement::new();
            state.update(&row(vec![txt("a")]), &bad, 1).unwrap();
            let mut s = sum_state_on(Box::new(state));
            let err = s
                .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]))
                .expect_err("a malformed stored value is corrupted state");
            assert!(err.0.contains("corrupted aggregate state"), "{err}");
        }
    }
```

- [ ] **Step 8: New tests**

In `node.rs`'s test module add (the `use` line goes at the top of the module if those names are not imported there already):

```rust
    use crate::test_support::{join_on_k, kv, kv_row};

    #[test]
    fn a_provider_that_fails_fails_the_build() {
        // M1b Phase 3a: a shadow table that should exist and does not is an
        // error from `build`, not an empty arrangement.
        let err = Node::build(
            &crate::lower_query(
                &join_on_k(),
                &crate::Database::new(vec![kv("t0"), kv("t1")]),
            )
            .unwrap(),
            &mut |_| Err(crate::StateError("no such state table".into())),
        )
        .expect_err("the provider's error must reach the caller");
        assert_eq!(err.0, "no such state table");
    }

    #[test]
    fn a_state_error_inside_the_tree_reaches_the_caller() {
        // Both stateful operators pass their arrangement's error up through
        // `delta` rather than swallowing it or panicking.
        use crate::test_support::Failing;
        let db = crate::Database::new(vec![kv("t0"), kv("t1")]);
        let plan = crate::lower_query(&join_on_k(), &db).unwrap();
        for failing in [ArrangementRole::JoinRight, ArrangementRole::AggregateGroups] {
            let mut tree = Node::build(&plan, &mut |id| -> Result<
                Box<dyn crate::Arrangement>,
                crate::StateError,
            > {
                if id.role == failing {
                    Ok(Box::new(Failing))
                } else {
                    Ok(Box::new(crate::MemArrangement::new()))
                }
            })
            .unwrap();
            let err = tree
                .delta("t0", &ZSet::from_rows([(kv_row("a", 1), 1)]))
                .and_then(|_| tree.delta("t1", &ZSet::from_rows([(kv_row("a", 2), 1)])))
                .expect_err("a failing arrangement must fail the push");
            assert!(err.0.starts_with("injected"), "{failing:?}: {err}");
        }
    }
```

In `plan.rs`'s test module add:

```rust
    #[test]
    fn the_canonical_text_of_a_join_view_is_pinned() {
        // Stored with every persisted view (M1b Phase 3a): if this text ever
        // changes for an unchanged plan, every existing view reports its state
        // as unusable on reopen. Change it only together with a format bump.
        let plan = lower_query(
            &jq(
                vec![2],
                vec![sum(1), count()],
                cmp(0, CmpOp::Ne, Value::Text("a,b".into())),
                join("t1", 0, 0),
            ),
            &two_kv(),
        )
        .unwrap();
        assert_eq!(
            plan.canonical(),
            "aggregate([0],[sum(1),count],project([2,1],filter(ne(0,text:3:a,b),\
             join(0,0,scan(2:t0,[0,1]),scan(2:t1,[0,1])))))"
        );
    }

    #[test]
    fn plans_that_differ_have_different_canonical_texts() {
        let db = Database::single(ints(2));
        let texts: Vec<String> = [
            q(vec![0], vec![count()], Predicate::None),
            q(vec![1], vec![count()], Predicate::None),
            q(vec![0], vec![sum(1)], Predicate::None),
            q(vec![0], vec![count()], Predicate::IsNull { column: 1 }),
            q(vec![0], vec![count()], cmp(1, CmpOp::Gt, Value::Int(3))),
            q(vec![0], vec![count()], cmp(1, CmpOp::Ge, Value::Int(3))),
            q(vec![0], vec![count()], cmp(1, CmpOp::Gt, Value::Int(-3))),
        ]
        .iter()
        .map(|query| lower_query(query, &db).unwrap().canonical())
        .collect();
        let mut unique = texts.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");
    }
```

Run `cargo test --workspace --locked --no-fail-fast`: green, and the harness's own tests unchanged. fmt, clippy.

- [ ] **Step 9: Gate rows**

Rewrite, in terms of the current code, every existing row whose mutation or test names something this task changed, and **re-run each** — at least: the `MemArrangement` contract rows (`get` returning "an iterator", zero removal, empty-key removal, `scan()` order and history — `grep -n "Arrangement::get\|MemArrangement\|scan()" docs/mutation-gates.md`) and the M1b Phase 1 corrupted-state rows (`grep -n "corrupted" docs/mutation-gates.md`), whose "a panic whose message starts…" is now a `StateError`. Add under `## ivmlite-core`:

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| Phase 3a §3: a provider's error fails `Node::build` | In `build_at`'s `Plan::Aggregate` arm, replace `arrangements(…)?` with `arrangements(…).unwrap_or_else(\|_\| Box::new(crate::MemArrangement::new()))` | `a_provider_that_fails_fails_the_build` |
| Phase 3a §3: an arrangement's error reaches `delta`'s caller | In `JoinState::absorb`, replace `self.right.get(&key)?` with `self.right.get(&key).unwrap_or_default()` | `a_state_error_inside_the_tree_reaches_the_caller` |
| Phase 3a §3: a malformed stored aggregate state is a `StateError` | In `GroupState::decode`, delete the length check | `a_stored_value_of_the_wrong_shape_is_reported_as_corrupted` |
| Phase 3a §4: the plan fingerprint is stable text | In `canonical_predicate`, render `Value::Text` without its length prefix | `the_canonical_text_of_a_join_view_is_pinned` |
| Phase 3a §4: different plans have different fingerprints | In `canonical_predicate`, render `Compare` without its value | `plans_that_differ_have_different_canonical_texts` |

- [ ] **Step 10: Commit**

```bash
git add crates/ivmlite-core/src docs/mutation-gates.md
git commit -m "feat(core): fallible operator state; the plan's canonical text

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The extension crate

**Files:** Modify root `Cargo.toml`, `.gitignore`. Create `crates/ivmlite-sqlite/{Cargo.toml,Cargo.lock,src/lib.rs,src/encode.rs,src/names.rs,src/catalog.rs,src/state.rs,src/view.rs,src/vtab.rs}`, `scripts/build-extension.sh`, `scripts/test-all.sh`.

**Interfaces:**
- Consumes: Task 2's fallible `Arrangement`/`Node`/`StateError`/`Plan::canonical`, `MemArrangement`, `ArrangementId`/`ArrangementRole`; `ivmlite_sql::{compile, Catalog, CatalogError, CompiledView}`.
- Produces: the library `crates/ivmlite-sqlite/target/debug/libivmlite_sqlite.{dylib,so}` with entry point `sqlite3_ivmlite_init`, registering the `ivm` module; the scripts.

- [ ] **Step 1: Keep the crate out of the workspace**

In the root `Cargo.toml`, directly under `resolver = "2"`:

```toml
# The SQLite extension builds separately: see its Cargo.toml.
exclude = ["crates/ivmlite-sqlite"]
```

- [ ] **Step 2: The crate**

`crates/ivmlite-sqlite/Cargo.toml` (the empty `[workspace]` makes it its own workspace root):

```toml
# Not a member of the root workspace: this crate needs rusqlite's
# `loadable_extension` feature, and the test host (`ivmlite-test`) needs
# `bundled` + `load_extension`; Cargo unifies features across one build, and
# the two cannot be compiled together. Build it with `scripts/test-all.sh` or
# `cargo build --manifest-path crates/ivmlite-sqlite/Cargo.toml`.
[package]
name = "ivmlite-sqlite"
version = "0.0.0"
edition = "2021"
rust-version = "1.95"
license = "MIT"
repository = "https://github.com/SFARL/ivmlite"

[lib]
crate-type = ["cdylib"]

[dependencies]
ivmlite-core = { path = "../ivmlite-core" }
ivmlite-sql = { path = "../ivmlite-sql" }
rusqlite = { version = "0.40", features = ["loadable_extension", "vtab"] }

[workspace]
```

`src/lib.rs`:

```rust
//! ivmlite as a SQLite loadable extension (spec §4.3; Phase 3a spec): the
//! `ivm` virtual-table module.
//!
//! ```sql
//! CREATE VIRTUAL TABLE revenue USING ivm('SELECT region, SUM(amount) FROM orders GROUP BY region');
//! INSERT INTO revenue(revenue) VALUES ('refresh');
//! SELECT * FROM revenue;
//! DROP TABLE revenue;
//! ```

mod catalog;
mod encode;
mod names;
mod state;
mod view;
mod vtab;

use std::ffi::{c_char, c_int};
use std::panic::AssertUnwindSafe;

use rusqlite::vtab::Module;
use rusqlite::{ffi, Connection};

/// The extension's entry point; load it with
/// `load_extension('<path>', 'sqlite3_ivmlite_init')`.
///
/// # Safety
/// Called by SQLite with a live connection handle and API table.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_ivmlite_init(
    db: *mut ffi::sqlite3,
    err: *mut *mut c_char,
    api: *mut ffi::sqlite3_api_routines,
) -> c_int {
    Connection::extension_init2(db, err, api, |conn| {
        // The `ivm` module: a writable virtual table, `INSERT` being its
        // command channel. A `const` is promoted to the `'static` the
        // registration needs.
        const IVM: Module<'static, vtab::IvmTab> = Module::update_module();
        std::panic::catch_unwind(AssertUnwindSafe(|| conn.create_module(c"ivm", &IVM, None)))
            .unwrap_or_else(|_| {
                Err(rusqlite::Error::ModuleError(
                    "ivmlite internal error while registering the ivm module".to_string(),
                ))
            })?;
        Ok(false)
    })
}
```

`src/encode.rs` (with its tests):

```rust
//! The canonical byte encoding of a `Row` in a state table's `key` and `val`
//! columns (Phase 3a spec §4): per value a tag byte, then its payload —
//! `0x00` NULL; `0x01` INTEGER, 8 bytes big-endian; `0x02` TEXT, a 4-byte
//! big-endian length, then the UTF-8 bytes. The same row always encodes to the
//! same bytes, so `PRIMARY KEY(key, val)` identifies a row.

use ivmlite_core::{Row, StateError, Value};

const NULL: u8 = 0x00;
const INT: u8 = 0x01;
const TEXT: u8 = 0x02;

pub fn encode(row: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    for value in &row.0 {
        match value {
            Value::Null => out.push(NULL),
            Value::Int(n) => {
                out.push(INT);
                out.extend_from_slice(&n.to_be_bytes());
            }
            Value::Text(s) => {
                out.push(TEXT);
                let len = u32::try_from(s.len()).expect("a TEXT value SQLite stored fits in 4 GiB");
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> Result<Row, StateError> {
    let corrupt = |what: &str| StateError(format!("corrupted state row encoding: {what}"));
    let mut values = Vec::new();
    let mut rest = bytes;
    while let Some((&tag, tail)) = rest.split_first() {
        rest = tail;
        match tag {
            NULL => values.push(Value::Null),
            INT => {
                let (n, tail) = rest
                    .split_first_chunk::<8>()
                    .ok_or_else(|| corrupt("truncated INTEGER"))?;
                values.push(Value::Int(i64::from_be_bytes(*n)));
                rest = tail;
            }
            TEXT => {
                let (len, tail) = rest
                    .split_first_chunk::<4>()
                    .ok_or_else(|| corrupt("truncated TEXT length"))?;
                let len = u32::from_be_bytes(*len) as usize;
                if tail.len() < len {
                    return Err(corrupt("truncated TEXT"));
                }
                let (text, tail) = tail.split_at(len);
                let text = std::str::from_utf8(text).map_err(|_| corrupt("TEXT is not UTF-8"))?;
                values.push(Value::Text(text.to_string()));
                rest = tail;
            }
            other => return Err(corrupt(&format!("unknown tag {other:#04x}"))),
        }
    }
    Ok(Row::new(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Row {
        Row::new(vec![
            Value::Null,
            Value::Int(-1),
            Value::Int(i64::MAX),
            Value::Text(String::new()),
            Value::Text("Ā é".into()),
        ])
    }

    #[test]
    fn a_row_round_trips() {
        assert_eq!(decode(&encode(&row())).unwrap(), row());
    }

    #[test]
    fn the_encoding_is_the_documented_layout() {
        // Pinned byte for byte: state tables written by one build must be
        // readable by the next, and PRIMARY KEY(key, val) relies on the same
        // row always encoding to the same bytes (spec §7).
        let bytes = encode(&Row::new(vec![
            Value::Null,
            Value::Int(1),
            Value::Text("ab".into()),
        ]));
        assert_eq!(
            bytes,
            vec![0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 2, b'a', b'b']
        );
    }

    #[test]
    fn corrupt_bytes_are_an_error_not_a_panic() {
        for bad in [
            vec![0x01, 0, 0],             // truncated INTEGER
            vec![0x02, 0, 0, 0, 9, b'a'], // TEXT shorter than its length
            vec![0x02, 0, 0, 0, 1, 0xff], // TEXT that is not UTF-8
            vec![0x07],                   // unknown tag
        ] {
            let e = decode(&bad).expect_err("corrupt state must be reported");
            assert!(e.0.contains("corrupted state row encoding"), "{e}");
        }
    }
}
```

`src/names.rs`:

```rust
//! The names of every shadow object a view creates (Phase 3a spec §4), and
//! identifier quoting. Every statement the extension issues quotes every
//! identifier it did not write itself.

use ivmlite_core::{ArrangementId, ArrangementRole};

/// `name` as an SQL identifier: double-quoted, with each `"` doubled.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub const META: &str = "__ivm_meta";
pub const VIEWS: &str = "__ivm_view";
pub const DEPS: &str = "__ivm_dep";
pub const PROGRESS: &str = "__ivm_progress";

/// Every name the extension creates starts with this; the catalog refuses a
/// view over such a table.
pub const PREFIX: &str = "__ivm_";

pub fn delta_table(table: &str) -> String {
    format!("__ivm_delta_{table}")
}

pub fn trigger(table: &str, event: &str) -> String {
    format!("__ivm_{table}_{event}")
}

/// Where a refresh stages every change before applying them in one statement.
pub fn stage_table(view: &str) -> String {
    format!("__ivm_stage_{view}")
}

/// The trigger on the stage table that applies the staged changes.
pub fn apply_trigger(view: &str) -> String {
    format!("__ivm_apply_{view}")
}

/// `text` as an SQL string literal.
pub fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

pub fn out_table(view: &str) -> String {
    format!("__ivm_out_{view}")
}

/// One table per `(node, role)` (spec §7). The role is a fixed string, never
/// the enum's `Debug` output, so renaming a variant cannot rename a table.
pub fn state_table(view: &str, id: ArrangementId) -> String {
    let role = match id.role {
        ArrangementRole::JoinLeft => "join_left",
        ArrangementRole::JoinRight => "join_right",
        ArrangementRole::AggregateGroups => "agg_groups",
    };
    format!("__ivm_state_{view}_{}_{role}", id.node)
}
```

`src/catalog.rs`:

```rust
//! The SQLite implementation of `ivmlite_sql::Catalog` (spec §4.3, §7.1).

use ivmlite_core::{Column, ColumnType, Schema};
use ivmlite_sql::{Catalog, CatalogError};
use rusqlite::{Connection, OptionalExtension};

use crate::names::{literal, PREFIX};

/// Reads a table's shape from `pragma_table_list`, `pragma_table_info` and its
/// `CREATE` statement, and refuses what v0 cannot represent.
pub struct SqliteCatalog<'a> {
    pub conn: &'a Connection,
}

impl Catalog for SqliteCatalog<'_> {
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError> {
        let err = |e: rusqlite::Error| CatalogError(format!("reading the catalog: {e}"));
        // pragma_table_list matches names case-insensitively, as SQLite does.
        let found: Option<(String, String, bool)> = self
            .conn
            .query_row(
                "SELECT name, type, strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((declared, kind, strict)) = found else {
            return Ok(None);
        };
        let refuse = |why: &str| Err(CatalogError(format!("table {declared}: {why}")));
        if declared.len() >= PREFIX.len() && declared[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
            return refuse("is one of ivmlite's own shadow tables");
        }
        if kind != "table" {
            return refuse(&format!("is a {kind}, not an ordinary table"));
        }
        if !strict {
            return refuse(
                "is not STRICT; v0 needs STRICT tables so a column's values have one type (spec §7.1)",
            );
        }
        let sql: String = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [&declared],
                |r| r.get(0),
            )
            .map_err(err)?;
        if sql.to_ascii_uppercase().contains("COLLATE") {
            return refuse(
                "declares a COLLATE clause; v0 supports only the BINARY collation (spec §7.1)",
            );
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info({})",
                literal(&declared)
            ))
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(err)?;
        let mut columns = Vec::new();
        for row in rows {
            let (column, ty, not_null, pk) = row.map_err(err)?;
            let ty = match ty.to_ascii_uppercase().as_str() {
                "INTEGER" | "INT" => ColumnType::Integer,
                "TEXT" => ColumnType::Text,
                other => {
                    return refuse(&format!(
                    "column {column} has type {other}; v0 supports INTEGER and TEXT columns only"
                ))
                }
            };
            // An INTEGER PRIMARY KEY is the rowid and can never be NULL.
            let rowid = pk == 1 && ty == ColumnType::Integer;
            columns.push(Column {
                name: column,
                ty,
                nullable: !not_null && !rowid,
            });
        }
        Ok(Some(Schema {
            table: declared,
            columns,
        }))
    }
}

/// Fails unless the database is UTF-8: TEXT compares by UTF-8 byte order in
/// the engine, which is SQLite's BINARY collation only for UTF-8 (Phase 3a
/// spec §5; measured, `'Ā' > 'a'` differs between UTF-8 and UTF-16LE).
pub fn require_utf8(conn: &Connection) -> Result<(), String> {
    let encoding: String = conn
        .query_row("PRAGMA encoding", [], |r| r.get(0))
        .map_err(|e| format!("reading the database encoding: {e}"))?;
    if encoding == "UTF-8" {
        Ok(())
    } else {
        Err(format!(
            "the database encoding is {encoding}; ivmlite v0 supports UTF-8 databases only"
        ))
    }
}
```

`src/state.rs`:

```rust
//! `Arrangement` over a `__ivm_state_<view>_<node>_<role>` table (spec §6.3,
//! §7) that **reads** the table and **buffers** its writes.
//!
//! A refresh must change state, output and watermarks together or not at all,
//! and it runs inside the `INSERT INTO v(v)` statement, where SQLite forbids
//! a `SAVEPOINT` and does not roll back the callback's own writes when it
//! fails inside an explicit transaction (measured, Phase 3a). So nothing is
//! written while the operator tree runs: each arrangement overlays its pending
//! changes on the table's rows, and `view.rs` applies every pending change
//! afterwards in one statement.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ivmlite_core::{Arrangement, Row, StateError};
use rusqlite::Connection;

use crate::encode::{decode, encode};
use crate::names::quote;

/// An arrangement's pending weight changes, by key then value; shared with
/// the refresh that stages them once the operator tree is done.
pub type Pending = Rc<RefCell<BTreeMap<Row, BTreeMap<Row, i64>>>>;

pub struct BufferedArrangement {
    conn: Rc<Connection>,
    get_sql: String,
    scan_sql: String,
    pending: Pending,
}

impl BufferedArrangement {
    pub fn new(conn: Rc<Connection>, table: &str, pending: Pending) -> Self {
        let t = quote(table);
        BufferedArrangement {
            conn,
            get_sql: format!("SELECT val, w FROM {t} WHERE key = ?1"),
            scan_sql: format!("SELECT key, val, w FROM {t}"),
            pending,
        }
    }
}

fn state_error(e: rusqlite::Error) -> StateError {
    StateError(format!("operator state table: {e}"))
}

/// Add `changes` to `stored` and drop what reaches zero (spec §5.1).
fn overlay(stored: &mut BTreeMap<Row, i64>, changes: Option<&BTreeMap<Row, i64>>) {
    for (val, dw) in changes.into_iter().flatten() {
        let w = stored.entry(val.clone()).or_insert(0);
        *w += dw;
        if *w == 0 {
            stored.remove(val);
        }
    }
}

impl Arrangement for BufferedArrangement {
    fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError> {
        let mut stmt = self
            .conn
            .prepare_cached(&self.get_sql)
            .map_err(state_error)?;
        let rows = stmt
            .query_map([encode(key)], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(state_error)?;
        let mut values = BTreeMap::new();
        for row in rows {
            let (val, w) = row.map_err(state_error)?;
            values.insert(decode(&val)?, w);
        }
        overlay(&mut values, self.pending.borrow().get(key));
        Ok(values.into_iter().collect())
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError> {
        if weight_delta == 0 {
            return Ok(());
        }
        let mut pending = self.pending.borrow_mut();
        let vals = pending.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                pending.remove(key);
            }
        }
        Ok(())
    }

    fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError> {
        let mut stmt = self
            .conn
            .prepare_cached(&self.scan_sql)
            .map_err(state_error)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(state_error)?;
        let mut all: BTreeMap<Row, BTreeMap<Row, i64>> = BTreeMap::new();
        for row in rows {
            let (key, val, w) = row.map_err(state_error)?;
            all.entry(decode(&key)?)
                .or_default()
                .insert(decode(&val)?, w);
        }
        let pending = self.pending.borrow();
        for key in pending.keys() {
            all.entry(key.clone()).or_default();
        }
        let mut out = Vec::new();
        for (key, mut values) in all {
            overlay(&mut values, pending.get(&key));
            out.extend(values.into_iter().map(|(val, w)| (key.clone(), val, w)));
        }
        Ok(out)
    }
}
```

`src/view.rs` (with its test):

```rust
//! A view's life cycle on one connection (Phase 3a spec §5): create with
//! bootstrap, reconnect, refresh, destroy. Everything here is ordinary SQL on
//! the connection the virtual table was called on; `vtab.rs` only adapts it to
//! SQLite's callbacks.

use std::rc::Rc;

use ivmlite_core::{ArrangementId, MemArrangement, Node, Plan, Row, Schema, Value, ZSet};
use ivmlite_sql::{compile, Catalog, CompiledView};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension};

use crate::catalog::{require_utf8, SqliteCatalog};
use crate::names::{
    apply_trigger, delta_table, literal, out_table, quote, stage_table, state_table, trigger, DEPS,
    META, PROGRESS, VIEWS,
};
use crate::state::{BufferedArrangement, Pending};

/// The shadow-table layout's version, stored in `__ivm_meta` and with each view.
pub const FORMAT: i64 = 1;

/// The column of the view's output table that holds each row's weight.
const WEIGHT: &str = "__w";

type Result<T> = std::result::Result<T, String>;

fn sql_error(e: rusqlite::Error) -> String {
    e.to_string()
}

fn exec(conn: &Connection, sql: &str) -> Result<()> {
    conn.execute_batch(sql).map_err(sql_error)
}

/// Compile `sql` against the database's catalog, refusing a non-UTF-8 database.
fn compile_view(conn: &Connection, sql: &str) -> Result<CompiledView> {
    require_utf8(conn)?;
    compile(sql, &SqliteCatalog { conn }).map_err(|e| e.0)
}

/// Every arrangement the plan's operators ask for, in build order.
fn arrangement_ids(plan: &Plan) -> Vec<ArrangementId> {
    let mut ids = Vec::new();
    Node::build(plan, &mut |id| {
        ids.push(id);
        Ok(Box::new(MemArrangement::new()))
    })
    .expect("an in-memory provider cannot fail");
    ids
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(sql_error)
}

fn base_schema(conn: &Connection, table: &str) -> Result<Schema> {
    SqliteCatalog { conn }
        .table(table)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("no such table: {table}"))
}

fn column_list(schema: &Schema, prefix: &str) -> String {
    schema
        .columns
        .iter()
        .map(|c| format!("{prefix}{}", quote(&c.name)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn sql_type(column: &ivmlite_core::Column) -> &'static str {
    match column.ty {
        ivmlite_core::ColumnType::Integer => "INTEGER",
        ivmlite_core::ColumnType::Text => "TEXT",
    }
}

/// The `CREATE TABLE` statement SQLite is given for the virtual table: the
/// output columns, then a hidden column named after the view — the command
/// channel of `INSERT INTO v(v) VALUES('refresh')` (the FTS5 idiom, spec §8.3).
pub fn declaration(name: &str, view: &CompiledView) -> Result<String> {
    if view
        .columns
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "a result column is named {name}, like the view; the view's name is its command column"
        ));
    }
    let columns: Vec<String> = view
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    Ok(format!(
        "CREATE TABLE x({}, {} HIDDEN)",
        columns.join(", "),
        quote(name)
    ))
}

fn create_global_tables(conn: &Connection) -> Result<()> {
    exec(
        conn,
        &format!(
            "CREATE TABLE IF NOT EXISTS {META}(key TEXT PRIMARY KEY, value);
             CREATE TABLE IF NOT EXISTS {VIEWS}(name TEXT PRIMARY KEY, sql TEXT NOT NULL,
                 plan TEXT NOT NULL, declaration TEXT NOT NULL, format INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS {DEPS}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 PRIMARY KEY(view, tbl));
             CREATE TABLE IF NOT EXISTS {PROGRESS}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl));
             INSERT OR IGNORE INTO {META}(key, value) VALUES ('format', {FORMAT});"
        ),
    )?;
    let format: i64 = conn
        .query_row(
            &format!("SELECT value FROM {META} WHERE key = 'format'"),
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if format != FORMAT {
        return Err(format!(
            "the database's ivmlite state has format {format}; this extension reads format {FORMAT}"
        ));
    }
    Ok(())
}

fn create_delta_table(conn: &Connection, schema: &Schema) -> Result<()> {
    let t = &schema.table;
    let delta = quote(&delta_table(t));
    let defs: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    let cols = column_list(schema, "");
    let new = column_list(schema, "NEW.");
    let old = column_list(schema, "OLD.");
    let base = quote(t);
    // Spec §8.1: an UPDATE is a retraction of OLD plus an insertion of NEW, so
    // the delta table already holds a Z-set. AUTOINCREMENT: once Phase 3b
    // deletes consumed deltas, a reused `seq` would fall below a watermark.
    exec(
        conn,
        &format!(
            "CREATE TABLE {delta}(seq INTEGER PRIMARY KEY AUTOINCREMENT, w INTEGER NOT NULL, {defs});
             CREATE TRIGGER {ins} AFTER INSERT ON {base} BEGIN
                 INSERT INTO {delta}(w, {cols}) VALUES (1, {new});
             END;
             CREATE TRIGGER {del} AFTER DELETE ON {base} BEGIN
                 INSERT INTO {delta}(w, {cols}) VALUES (-1, {old});
             END;
             CREATE TRIGGER {upd} AFTER UPDATE ON {base} BEGIN
                 INSERT INTO {delta}(w, {cols}) VALUES (-1, {old});
                 INSERT INTO {delta}(w, {cols}) VALUES (1, {new});
             END;",
            defs = defs.join(", "),
            ins = quote(&trigger(t, "ins")),
            del = quote(&trigger(t, "del")),
            upd = quote(&trigger(t, "upd")),
        ),
    )
}

fn create_out_table(conn: &Connection, name: &str, view: &CompiledView) -> Result<()> {
    if view
        .columns
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(WEIGHT))
    {
        return Err(format!(
            "a result column is named {WEIGHT}, which ivmlite reserves"
        ));
    }
    let defs: Vec<String> = view
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    exec(
        conn,
        &format!(
            "CREATE TABLE {}({}, {WEIGHT} INTEGER NOT NULL)",
            quote(&out_table(name)),
            defs.join(", ")
        ),
    )
}

fn value_of(v: ValueRef<'_>) -> std::result::Result<Value, String> {
    match v {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(Value::Int(n)),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(|s| Value::Text(s.to_string()))
            .map_err(|_| "a TEXT value is not UTF-8".to_string()),
        other => Err(format!(
            "a {:?} value in a column v0 declares INTEGER or TEXT",
            other.data_type()
        )),
    }
}

fn read_row(
    r: &rusqlite::Row<'_>,
    from: usize,
    n: usize,
) -> rusqlite::Result<std::result::Result<Row, String>> {
    let mut values = Vec::with_capacity(n);
    for i in from..from + n {
        match value_of(r.get_ref(i)?) {
            Ok(v) => values.push(v),
            Err(e) => return Ok(Err(e)),
        }
    }
    Ok(Ok(Row::new(values)))
}

/// Every row of a base table, as the first batch of deltas.
fn read_base(conn: &Connection, schema: &Schema) -> Result<ZSet> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM {}",
            column_list(schema, ""),
            quote(&schema.table)
        ))
        .map_err(sql_error)?;
    let n = schema.columns.len();
    let rows = stmt
        .query_map([], |r| read_row(r, 0, n))
        .map_err(sql_error)?;
    let mut z = ZSet::new();
    for row in rows {
        z.update(row.map_err(sql_error)??, 1);
    }
    Ok(z)
}

/// The deltas of `table` after `after`, consolidated, and the highest `seq` read.
fn read_deltas(conn: &Connection, schema: &Schema, after: i64) -> Result<(ZSet, i64)> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT seq, w, {} FROM {} WHERE seq > ?1 ORDER BY seq",
            column_list(schema, ""),
            quote(&delta_table(&schema.table))
        ))
        .map_err(sql_error)?;
    let n = schema.columns.len();
    let rows = stmt
        .query_map([after], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, read_row(r, 2, n)?))
        })
        .map_err(sql_error)?;
    let mut z = ZSet::new();
    let mut last = after;
    for row in rows {
        let (seq, w, row) = row.map_err(sql_error)?;
        z.update(row?, w);
        last = seq;
    }
    Ok((z, last))
}

/// The stage table and the trigger that applies it (see `apply`). One stage
/// row is one change: `op` is `state` (a weight change of arrangement `arr`),
/// `out+` / `out-` (an output row, in `c0`, `c1`, …), or `progress` (table
/// `tbl` consumed through `seq`).
fn create_stage(
    conn: &Connection,
    name: &str,
    view: &CompiledView,
    ids: &[ArrangementId],
) -> Result<()> {
    let stage = quote(&stage_table(name));
    let out = quote(&out_table(name));
    let n = view.columns.len();
    let defs: Vec<String> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("c{i} {}", sql_type(c)))
        .collect();
    let cols: Vec<String> = view.columns.iter().map(|c| quote(&c.name)).collect();
    let new_cols: Vec<String> = (0..n).map(|i| format!("NEW.c{i}")).collect();
    let same: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} IS NEW.c{i}"))
        .collect();
    let same = same.join(" AND ");
    let mut body = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let t = quote(&state_table(name, *id));
        body.push(format!(
            "INSERT INTO {t}(key, val, w) SELECT NEW.key, NEW.val, NEW.w \
             WHERE NEW.op = 'state' AND NEW.arr = {i} \
             ON CONFLICT(key, val) DO UPDATE SET w = w + excluded.w;"
        ));
        body.push(format!(
            "DELETE FROM {t} WHERE NEW.op = 'state' AND NEW.arr = {i} \
             AND key = NEW.key AND val = NEW.val AND w = 0;"
        ));
    }
    body.push(format!(
        "SELECT RAISE(ABORT, 'ivmlite broken invariant: the view retracted a row its output table does not hold') \
         WHERE NEW.op = 'out-' AND NOT EXISTS (SELECT 1 FROM {out} WHERE {same});"
    ));
    body.push(format!(
        "DELETE FROM {out} WHERE NEW.op = 'out-' \
         AND rowid = (SELECT rowid FROM {out} WHERE {same} LIMIT 1);"
    ));
    body.push(format!(
        "INSERT INTO {out}({}, {WEIGHT}) SELECT {}, 1 WHERE NEW.op = 'out+';",
        cols.join(", "),
        new_cols.join(", ")
    ));
    body.push(format!(
        "UPDATE {PROGRESS} SET applied_seq = NEW.seq \
         WHERE NEW.op = 'progress' AND view = {} AND tbl = NEW.tbl;",
        literal(name)
    ));
    exec(
        conn,
        &format!(
            "CREATE TABLE {stage}(op TEXT NOT NULL, arr INTEGER, key BLOB, val BLOB, w INTEGER,
                 tbl TEXT, seq INTEGER, {}, armed INTEGER NOT NULL DEFAULT 0);
             CREATE TRIGGER {} AFTER UPDATE OF armed ON {stage}
                 WHEN OLD.armed = 0 AND NEW.armed = 1
             BEGIN
                 {}
             END;",
            defs.join(", "),
            quote(&apply_trigger(name)),
            body.join("\n                 ")
        ),
    )
}

/// What one refresh (or the bootstrap) changes.
struct Changes {
    /// Each arrangement's pending weight changes, indexed like `ids`.
    state: Vec<Pending>,
    output: ZSet,
    /// `(table, seq)`: the table's deltas through `seq` are consumed.
    progress: Vec<(String, i64)>,
}

/// Push one batch per table through the view's operator tree. Its
/// arrangements read the state tables and buffer their writes.
fn compute(
    conn: &Rc<Connection>,
    name: &str,
    plan: &Plan,
    ids: &[ArrangementId],
    batches: &[(String, ZSet)],
) -> Result<(Vec<Pending>, ZSet)> {
    let pending: Vec<Pending> = ids.iter().map(|_| Pending::default()).collect();
    let mut tree = Node::build(plan, &mut |id| {
        let i = ids
            .iter()
            .position(|x| *x == id)
            .expect("the ids were collected from this same plan");
        Ok(Box::new(BufferedArrangement::new(
            conn.clone(),
            &state_table(name, id),
            pending[i].clone(),
        )))
    })
    .map_err(|e| e.0)?;
    let mut out = ZSet::new();
    for (table, delta) in batches {
        if !delta.is_empty() {
            out.merge(&tree.delta(table, delta).map_err(|e| e.0)?);
        }
    }
    Ok((pending, out))
}

fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Int(n) => rusqlite::types::Value::Integer(*n),
        Value::Text(s) => rusqlite::types::Value::Text(s.clone()),
    }
}

/// Apply `changes` to the state tables, the output table and the watermarks
/// **in one statement**, so they change together or not at all.
///
/// A refresh runs inside `INSERT INTO v(v)`, where a `SAVEPOINT` is refused
/// and, inside an explicit transaction, a failed callback's own writes are not
/// rolled back (both measured, Phase 3a). One statement is atomic on its own:
/// the changes are first written to the stage table — harmless if that fails
/// part way, since the stage is emptied at the start of every apply — and then
/// a single `UPDATE … SET armed = 1` fires the apply trigger for every row.
/// If any row fails, SQLite rolls that whole statement back.
fn apply(conn: &Connection, name: &str, view: &CompiledView, changes: &Changes) -> Result<()> {
    let stage = quote(&stage_table(name));
    exec(conn, &format!("DELETE FROM {stage}"))?;
    let stage_state =
        format!("INSERT INTO {stage}(op, arr, key, val, w) VALUES ('state', ?1, ?2, ?3, ?4)");
    for (i, pending) in changes.state.iter().enumerate() {
        for (key, vals) in pending.borrow().iter() {
            for (val, w) in vals {
                conn.prepare_cached(&stage_state)
                    .and_then(|mut s| {
                        s.execute(params![
                            i as i64,
                            crate::encode::encode(key),
                            crate::encode::encode(val),
                            w
                        ])
                    })
                    .map_err(sql_error)?;
            }
        }
    }
    let n = view.columns.len();
    let placeholders: Vec<String> = (0..n).map(|i| format!("?{}", i + 2)).collect();
    let cs: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
    let stage_out = format!(
        "INSERT INTO {stage}(op, {}) VALUES (?1, {})",
        cs.join(", "),
        placeholders.join(", ")
    );
    for (row, &w) in changes.output.iter() {
        let op = match w {
            1 => "out+",
            -1 => "out-",
            w => {
                return Err(format!(
                    "broken invariant: output row {row:?} has weight {w}; a v0 view's rows have weight 1"
                ))
            }
        };
        let mut values = vec![rusqlite::types::Value::Text(op.to_string())];
        values.extend(row.0.iter().map(sql_value));
        conn.prepare_cached(&stage_out)
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(sql_error)?;
    }
    for (table, seq) in &changes.progress {
        conn.execute(
            &format!("INSERT INTO {stage}(op, tbl, seq) VALUES ('progress', ?1, ?2)"),
            params![table, seq],
        )
        .map_err(sql_error)?;
    }
    // The one statement that changes durable state.
    exec(conn, &format!("UPDATE {stage} SET armed = 1"))?;
    exec(conn, &format!("DELETE FROM {stage}"))
}

/// `CREATE VIRTUAL TABLE <name> USING ivm('<sql>')`, inside the statement's
/// own transaction: create every shadow object, then bootstrap (spec §7.3).
pub fn create(conn: &Rc<Connection>, name: &str, sql: &str) -> Result<CompiledView> {
    let view = compile_view(conn, sql)?;
    // Computed first: a name clash fails before anything is created.
    let declared = declaration(name, &view)?;
    create_global_tables(conn)?;
    let schemas: Vec<Schema> = view
        .tables
        .iter()
        .map(|t| base_schema(conn, t))
        .collect::<Result<_>>()?;
    for schema in &schemas {
        if table_exists(conn, &delta_table(&schema.table))? {
            return Err(format!(
                "table {} is already tracked by another ivmlite view; Phase 3a supports one view per base table",
                schema.table
            ));
        }
    }
    for schema in &schemas {
        create_delta_table(conn, schema)?;
    }
    let ids = arrangement_ids(&view.plan);
    for id in &ids {
        exec(
            conn,
            &format!(
                "CREATE TABLE {}(key BLOB NOT NULL, val BLOB NOT NULL, w INTEGER NOT NULL,
                     PRIMARY KEY(key, val)) WITHOUT ROWID",
                quote(&state_table(name, *id))
            ),
        )?;
    }
    create_out_table(conn, name, &view)?;
    create_stage(conn, name, &view, &ids)?;
    conn.execute(
        &format!(
            "INSERT INTO {VIEWS}(name, sql, plan, declaration, format) VALUES (?1, ?2, ?3, ?4, ?5)"
        ),
        params![name, sql, view.plan.canonical(), declared, FORMAT],
    )
    .map_err(sql_error)?;
    for t in &view.tables {
        conn.execute(
            &format!("INSERT INTO {DEPS}(view, tbl) VALUES (?1, ?2)"),
            params![name, t],
        )
        .map_err(sql_error)?;
    }

    // Bootstrap. The delta tables and triggers were created above, in this
    // same transaction: every write from now on is captured, and no write so
    // far is in a delta table, so the snapshot read here is exactly the state
    // at watermark 0 (spec §7.3).
    let batches: Vec<(String, ZSet)> = schemas
        .iter()
        .map(|s| Ok((s.table.clone(), read_base(conn, s)?)))
        .collect::<Result<_>>()?;
    for schema in &schemas {
        conn.execute(
            &format!("INSERT INTO {PROGRESS}(view, tbl, applied_seq) VALUES (?1, ?2, 0)"),
            params![name, schema.table],
        )
        .map_err(sql_error)?;
    }
    let (state, output) = compute(conn, name, &view.plan, &ids, &batches)?;
    apply(
        conn,
        name,
        &view,
        &Changes {
            state,
            output,
            progress: Vec::new(),
        },
    )?;
    Ok(view)
}

/// A reopened view: the table declaration it was created with, and either the
/// compiled view or why it can no longer be maintained.
pub struct Reopened {
    pub declaration: String,
    pub view: std::result::Result<CompiledView, String>,
}

/// Reopen an existing view. Its stored SQL must compile to the same plan and
/// every shadow table it relies on must exist; if not, the view still opens —
/// with the stored declaration — so it can be dropped, and every read and
/// refresh reports why it is broken.
pub fn connect(conn: &Connection, name: &str) -> Result<Reopened> {
    let stored: Option<(String, String, String, i64)> = conn
        .query_row(
            &format!("SELECT sql, plan, declaration, format FROM {VIEWS} WHERE name = ?1"),
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let Some((sql, plan, declaration, format)) = stored else {
        return Err(format!("ivmlite has no record of the view {name}"));
    };
    let view = verify(conn, name, &sql, &plan, format)
        .map_err(|why| format!("view {name} cannot be maintained: {why}; drop and recreate it"));
    Ok(Reopened { declaration, view })
}

fn verify(
    conn: &Connection,
    name: &str,
    sql: &str,
    plan: &str,
    format: i64,
) -> Result<CompiledView> {
    if format != FORMAT {
        return Err(format!(
            "it was stored in format {format}, and this extension reads format {FORMAT}"
        ));
    }
    let view = compile_view(conn, sql)?;
    let now = view.plan.canonical();
    if now != plan {
        return Err(format!(
            "its SQL now compiles to a different plan than the one its state was built with \
             (stored {plan}, now {now})"
        ));
    }
    let mut needed: Vec<String> = arrangement_ids(&view.plan)
        .into_iter()
        .map(|id| state_table(name, id))
        .collect();
    needed.push(out_table(name));
    needed.push(stage_table(name));
    needed.extend(view.tables.iter().map(|t| delta_table(t)));
    for table in needed {
        if !table_exists(conn, &table)? {
            return Err(format!("its shadow table {table} is missing"));
        }
    }
    Ok(view)
}

/// `INSERT INTO v(v) VALUES('refresh')`: bring the view up to date. State,
/// output and watermarks change together or not at all (see `apply`).
pub fn refresh(conn: &Rc<Connection>, name: &str, view: &CompiledView) -> Result<()> {
    let mut batches = Vec::new();
    let mut progress = Vec::new();
    for table in &view.tables {
        let schema = base_schema(conn, table)?;
        let applied: i64 = conn
            .query_row(
                &format!("SELECT applied_seq FROM {PROGRESS} WHERE view = ?1 AND tbl = ?2"),
                params![name, table],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        let (delta, last) = read_deltas(conn, &schema, applied)?;
        batches.push((table.clone(), delta));
        if last > applied {
            progress.push((table.clone(), last));
        }
    }
    let ids = arrangement_ids(&view.plan);
    let (state, output) = compute(conn, name, &view.plan, &ids, &batches)?;
    apply(
        conn,
        name,
        view,
        &Changes {
            state,
            output,
            progress,
        },
    )
}

/// Whether `table` is one of `view`'s state tables: `__ivm_state_<view>_`
/// followed by exactly `<node>_<role>`. The suffix is checked exactly, so a
/// view named `v` never claims the tables of a view named `v_1`.
fn is_state_table_of(table: &str, view: &str) -> bool {
    let Some(rest) = table.strip_prefix(&format!("__ivm_state_{view}_")) else {
        return false;
    };
    let Some((node, role)) = rest.split_once('_') else {
        return false;
    };
    !node.is_empty()
        && node.bytes().all(|b| b.is_ascii_digit())
        && ["join_left", "join_right", "agg_groups"].contains(&role)
}

/// `DROP TABLE v`: triggers first, so the base tables stay writable (M-1
/// scenario 9), then every shadow table and the view's metadata. It uses only
/// what is recorded, not the compiled view, so a view that can no longer be
/// maintained can still be dropped.
pub fn destroy(conn: &Connection, name: &str) -> Result<()> {
    let tables: Vec<String> = conn
        .prepare(&format!("SELECT tbl FROM {DEPS} WHERE view = ?1"))
        .and_then(|mut s| s.query_map([name], |r| r.get(0))?.collect())
        .map_err(sql_error)?;
    for t in &tables {
        for event in ["ins", "del", "upd"] {
            exec(
                conn,
                &format!("DROP TRIGGER IF EXISTS {}", quote(&trigger(t, event))),
            )?;
        }
        exec(
            conn,
            &format!("DROP TABLE IF EXISTS {}", quote(&delta_table(t))),
        )?;
    }
    let state: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sql_error)?
        .into_iter()
        .filter(|t| is_state_table_of(t, name))
        .collect();
    for t in state {
        exec(conn, &format!("DROP TABLE {}", quote(&t)))?;
    }
    exec(
        conn,
        &format!("DROP TABLE IF EXISTS {}", quote(&out_table(name))),
    )?;
    exec(
        conn,
        &format!("DROP TABLE IF EXISTS {}", quote(&stage_table(name))),
    )?;
    for table in [VIEWS, DEPS, PROGRESS] {
        let column = if table == VIEWS { "name" } else { "view" };
        conn.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), [name])
            .map_err(sql_error)?;
    }
    let left: i64 = conn
        .query_row(&format!("SELECT count(*) FROM {VIEWS}"), [], |r| r.get(0))
        .map_err(sql_error)?;
    if left == 0 {
        exec(
            conn,
            &format!(
                "DROP TABLE {META}; DROP TABLE {VIEWS}; DROP TABLE {DEPS}; DROP TABLE {PROGRESS};"
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_views_state_tables_are_recognized_exactly() {
        assert!(is_state_table_of("__ivm_state_v_0_agg_groups", "v"));
        assert!(is_state_table_of("__ivm_state_v_12_join_right", "v"));
        // View `v_1`'s tables are not view `v`'s, and the other way round.
        assert!(!is_state_table_of("__ivm_state_v_1_0_agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_state_v_0_agg_groups", "v_1"));
        assert!(is_state_table_of("__ivm_state_v_1_0_agg_groups", "v_1"));
        assert!(!is_state_table_of("__ivm_state_v_x_agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_state_v__agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_out_v", "v"));
    }
}
```

`src/vtab.rs`:

```rust
//! The `ivm` virtual-table module: SQLite's callbacks, adapted to `view.rs`.
//!
//! Every callback runs inside `guard`, so a panic becomes an SQLite error
//! instead of unwinding across the FFI boundary (Phase 3a spec §5).

use std::borrow::Cow;
use std::ffi::{c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use ivmlite_sql::CompiledView;
use rusqlite::types::Value as SqlValue;
use rusqlite::vtab::{
    dequote, Context, CreateVTab, Filters, IndexInfo, Inserts, UpdateVTab, Updates, VTab,
    VTabConnection, VTabCursor, VTabKind,
};
use rusqlite::{ffi, Connection, Error};

use crate::names::{out_table, quote};
use crate::view;

/// Run a callback body, turning its error and any panic into an SQLite error.
fn guard<T>(body: impl FnOnce() -> Result<T, String>) -> rusqlite::Result<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(Error::ModuleError(message)),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".to_string());
            Err(Error::ModuleError(format!(
                "ivmlite internal error: {message}"
            )))
        }
    }
}

/// A non-owning `Connection` over the handle SQLite called us on.
fn connection(db: *mut ffi::sqlite3) -> Result<Rc<Connection>, String> {
    // SAFETY: `db` is the live handle SQLite passed to this callback; a
    // `Connection` made by `from_handle` never closes it.
    unsafe { Connection::from_handle(db) }
        .map(Rc::new)
        .map_err(|e| e.to_string())
}

fn utf8<'a>(bytes: &'a [u8], what: &str) -> Result<&'a str, String> {
    std::str::from_utf8(bytes).map_err(|_| format!("the {what} is not UTF-8"))
}

#[repr(C)]
pub struct IvmTab {
    /// Must come first: SQLite sees this struct as a `sqlite3_vtab`.
    base: ffi::sqlite3_vtab,
    db: *mut ffi::sqlite3,
    name: String,
    /// The compiled view, or why a reopened view can no longer be maintained.
    view: Result<CompiledView, String>,
}

impl IvmTab {
    /// Shared by create and connect: `make` returns the declaration and the
    /// view (or why it is broken).
    fn open_view(
        db: &mut VTabConnection,
        database: &[u8],
        table: &[u8],
        make: impl FnOnce(
            &Rc<Connection>,
            &str,
        ) -> Result<(String, Result<CompiledView, String>), String>,
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        guard(|| {
            if database != b"main" {
                return Err("ivmlite v0 views live in the main database only".to_string());
            }
            let name = utf8(table, "view name")?.to_string();
            // SAFETY: the handle of the connection running this statement.
            let handle = unsafe { db.handle() };
            let conn = connection(handle)?;
            let (declaration, view) = make(&conn, &name)?;
            let declared = CString::new(declaration)
                .map(Cow::Owned)
                .map_err(|_| "a column name contains a NUL byte".to_string())?;
            Ok((
                declared,
                IvmTab {
                    base: ffi::sqlite3_vtab::default(),
                    db: handle,
                    name,
                    view,
                },
            ))
        })
    }

    fn view(&self) -> Result<&CompiledView, String> {
        self.view.as_ref().map_err(Clone::clone)
    }
}

unsafe impl<'vtab> VTab<'vtab> for IvmTab {
    type Aux = ();
    type Cursor = IvmCursor<'vtab>;

    fn connect(
        db: &mut VTabConnection,
        _aux: Option<&()>,
        _module: &[u8],
        database: &[u8],
        table: &[u8],
        _args: &[&[u8]],
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        IvmTab::open_view(db, database, table, |conn, name| {
            let reopened = view::connect(conn, name)?;
            Ok((reopened.declaration, reopened.view))
        })
    }

    fn best_index(&self, info: &mut IndexInfo) -> rusqlite::Result<bool> {
        guard(|| {
            // Only full scans: v0 reads the whole output table.
            info.set_estimated_cost(1_000_000.0);
            Ok(true)
        })
    }

    fn open(&'vtab mut self) -> rusqlite::Result<IvmCursor<'vtab>> {
        Ok(IvmCursor {
            base: ffi::sqlite3_vtab_cursor::default(),
            tab: self,
            rows: Vec::new(),
            at: 0,
        })
    }
}

impl CreateVTab<'_> for IvmTab {
    const KIND: VTabKind = VTabKind::Default;

    fn create(
        db: &mut VTabConnection,
        _aux: Option<&()>,
        _module: &[u8],
        database: &[u8],
        table: &[u8],
        args: &[&[u8]],
    ) -> rusqlite::Result<(Cow<'static, CStr>, Self)> {
        IvmTab::open_view(db, database, table, |conn, name| {
            let [arg] = args else {
                return Err(
                    "USING ivm takes one argument, the view's SELECT as a string literal"
                        .to_string(),
                );
            };
            let sql = dequote(utf8(arg, "view's SQL")?).into_owned();
            let compiled = view::create(conn, name, &sql)?;
            Ok((view::declaration(name, &compiled)?, Ok(compiled)))
        })
    }

    fn destroy(&self) -> rusqlite::Result<()> {
        guard(|| view::destroy(&*connection(self.db)?, &self.name))
    }
}

impl UpdateVTab<'_> for IvmTab {
    fn delete(&mut self, _rowid: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<()> {
        Err(read_only())
    }

    fn insert(&mut self, args: &Inserts<'_>) -> rusqlite::Result<i64> {
        guard(|| {
            // argv: old rowid (NULL), new rowid, the output columns, then the
            // hidden command column.
            let view = self.view()?;
            let command_at = 2 + view.columns.len();
            let command: Option<String> = args.get(command_at).map_err(|e| e.to_string())?;
            match command.as_deref() {
                Some("refresh") => {
                    view::refresh(&connection(self.db)?, &self.name, view)?;
                    Ok(0)
                }
                Some(other) => Err(format!(
                    "unknown ivmlite command {other:?}; the only command is 'refresh'"
                )),
                None => Err(read_only().to_string()),
            }
        })
    }

    fn update(&mut self, _args: &Updates<'_>) -> rusqlite::Result<()> {
        Err(read_only())
    }
}

fn read_only() -> Error {
    Error::ModuleError(
        "an ivmlite view is read-only; bring it up to date with INSERT INTO v(v) VALUES('refresh')"
            .to_string(),
    )
}

#[repr(C)]
pub struct IvmCursor<'vtab> {
    /// Must come first: SQLite sees this struct as a `sqlite3_vtab_cursor`.
    base: ffi::sqlite3_vtab_cursor,
    tab: &'vtab IvmTab,
    rows: Vec<(i64, Vec<SqlValue>)>,
    at: usize,
}

unsafe impl VTabCursor for IvmCursor<'_> {
    fn filter(
        &mut self,
        _idx: c_int,
        _idx_str: Option<&str>,
        _args: &Filters<'_>,
    ) -> rusqlite::Result<()> {
        guard(|| {
            let view = self.tab.view()?;
            let conn = connection(self.tab.db)?;
            let n = view.columns.len();
            let cols: Vec<String> = view.columns.iter().map(|c| quote(&c.name)).collect();
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT rowid, {} FROM {}",
                    cols.join(", "),
                    quote(&out_table(&self.tab.name))
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    let mut values = Vec::with_capacity(n);
                    for i in 1..=n {
                        values.push(r.get::<_, SqlValue>(i)?);
                    }
                    Ok((r.get::<_, i64>(0)?, values))
                })
                .map_err(|e| e.to_string())?;
            self.rows = rows
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?;
            self.at = 0;
            Ok(())
        })
    }

    fn next(&mut self) -> rusqlite::Result<()> {
        self.at += 1;
        Ok(())
    }

    fn eof(&self) -> bool {
        self.at >= self.rows.len()
    }

    fn column(&self, ctx: &mut Context, i: c_int) -> rusqlite::Result<()> {
        guard(|| {
            let values = &self.rows[self.at].1;
            let value = usize::try_from(i).ok().and_then(|i| values.get(i));
            // Past the output columns is the hidden command column: NULL.
            ctx.set_result(value.unwrap_or(&SqlValue::Null))
                .map_err(|e| e.to_string())
        })
    }

    fn rowid(&self) -> rusqlite::Result<i64> {
        Ok(self.rows[self.at].0)
    }
}
```

- [ ] **Step 3: The scripts**

`scripts/build-extension.sh`:

```bash
#!/usr/bin/env bash
# Build the ivmlite SQLite extension (a cdylib outside the Cargo workspace;
# see crates/ivmlite-sqlite/Cargo.toml for why). The extension tests in
# ivmlite-test load crates/ivmlite-sqlite/target/debug/libivmlite_sqlite.*.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
```

`scripts/test-all.sh`:

```bash
#!/usr/bin/env bash
# Build the extension, then run every workspace test. The mutation-gate
# protocol (docs/mutation-gates.md) uses this rather than a bare `cargo test`,
# so a mutation in core, the SQL front end or the extension reaches the
# extension tests.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-extension.sh
cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked
cargo test --workspace --locked --no-fail-fast "$@"
```

`chmod +x` both.

- [ ] **Step 4: Build and test**

The first build resolves the crate's own dependencies, so run it once without `--locked`: `cargo build --manifest-path crates/ivmlite-sqlite/Cargo.toml`. That writes `crates/ivmlite-sqlite/Cargo.lock`, which is committed. Then:

- `scripts/build-extension.sh` — builds.
- `cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked` — 4 passed (encode ×3, state-table names ×1).
- fmt and clippy for the extension (Global Constraints), and for the workspace.
- `cargo test --workspace --locked --no-fail-fast` — unchanged from Task 2 (the workspace does not build the extension).

The crate's behaviour is exercised end to end in Task 4; this task's reviewer checks it against the Phase 3a spec by reading.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml .gitignore crates/ivmlite-sqlite/Cargo.toml crates/ivmlite-sqlite/Cargo.lock crates/ivmlite-sqlite/src scripts/build-extension.sh scripts/test-all.sh
git commit -m "feat(sqlite): the ivm virtual-table module — create, refresh, read, reopen, drop

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

The root `.gitignore` ignores only `/target`, so add a line `/crates/ivmlite-sqlite/target` to it and include `.gitignore` in the commit; `git status` must not list the extension's build output.

---

### Task 4: The extension under test

**Files:** Modify `crates/ivmlite-test/Cargo.toml`, `crates/ivmlite-test/src/lib.rs`, `.github/workflows/ci.yml`, `docs/mutation-gates.md`. Create `crates/ivmlite-test/src/extension.rs`, `crates/ivmlite-test/tests/extension_lifecycle.rs`, `crates/ivmlite-test/tests/extension_differential.rs`.

**Interfaces:** Consumes the library Task 3 builds. Produces `ivmlite_test::{SqliteExtensionEngine, open_with_extension, extension_library}`.

- [ ] **Step 1: The host can load extensions**

`crates/ivmlite-test/Cargo.toml`: `rusqlite.workspace = true` becomes `rusqlite = { workspace = true, features = ["load_extension"] }`. In `src/lib.rs` add `mod extension;` (after `mod engine;`) and `pub use extension::{extension_library, open_with_extension, SqliteExtensionEngine};` (after the `engine` re-export).

- [ ] **Step 2: `SqliteExtensionEngine`**

`crates/ivmlite-test/src/extension.rs`:

```rust
//! The real SQLite extension as an engine under test (M1b Phase 3a).
//!
//! The extension is built separately (see `crates/ivmlite-sqlite/Cargo.toml`),
//! so this module loads its dynamic library from one fixed path and refuses
//! to run against a library older than its sources: a stale or missing
//! library fails the test with an instruction, and is never skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use ivmlite_core::{Database, Row, Value, ZSet};
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;

use crate::{create_table_sql, view_query_to_sql, Engine, EngineError, ViewQuery};

/// The view every engine instance creates.
/// Not `v`: the harness's tables have a column `v`, and a view may not share its
/// name with a result column (the name is its command column).
const VIEW: &str = "ivm_view";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The extension's dynamic library, checked to be at least as new as every
/// source file it is built from.
///
/// # Panics
/// If the library is missing or stale; run `scripts/build-extension.sh`.
pub fn extension_library() -> PathBuf {
    let name = if cfg!(target_os = "macos") {
        "libivmlite_sqlite.dylib"
    } else {
        "libivmlite_sqlite.so"
    };
    let lib = repo().join("crates/ivmlite-sqlite/target/debug").join(name);
    let built = std::fs::metadata(&lib)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| {
            panic!(
                "the ivmlite extension is not built at {}; run scripts/build-extension.sh",
                lib.display()
            )
        });
    let newest = [
        "crates/ivmlite-sqlite",
        "crates/ivmlite-core",
        "crates/ivmlite-sql",
    ]
    .iter()
    .flat_map(|dir| sources(&repo().join(dir)))
    .max()
    .expect("the extension has sources");
    assert!(
        built >= newest,
        "the ivmlite extension at {} is older than its sources; run scripts/build-extension.sh",
        lib.display()
    );
    lib
}

/// The modification times of a crate's manifest and every file under `src/`.
fn sources(dir: &Path) -> Vec<SystemTime> {
    let mut out = Vec::new();
    let mut stack = vec![dir.join("src")];
    if let Ok(m) = std::fs::metadata(dir.join("Cargo.toml")).and_then(|m| m.modified()) {
        out.push(m);
    }
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                out.push(m);
            }
        }
    }
    out
}

/// Open `path` (or an in-memory database) with the extension loaded.
pub fn open_with_extension(path: Option<&Path>) -> rusqlite::Result<Connection> {
    let conn = match path {
        Some(p) => Connection::open(p)?,
        None => Connection::open_in_memory()?,
    };
    // SAFETY: loading our own library, whose entry point only registers the
    // `ivm` module.
    unsafe {
        conn.load_extension_enable()?;
        conn.load_extension(extension_library(), Some("sqlite3_ivmlite_init"))?;
        conn.load_extension_disable()?;
    }
    Ok(conn)
}

fn err(e: rusqlite::Error) -> EngineError {
    EngineError(e.to_string())
}

fn sql_value(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Int(n) => SqlValue::Integer(*n),
        Value::Text(s) => SqlValue::Text(s.clone()),
    }
}

/// A database file under the system temp directory, removed when dropped.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        TempDb(std::env::temp_dir().join(format!("ivmlite-ext-{}-{n}.db", std::process::id())))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Drives the loaded extension through SQL: base-table writes are captured by
/// its triggers, `refresh` is the command channel, `materialize` reads the
/// virtual table.
pub struct SqliteExtensionEngine {
    /// Close and reopen the database before every refresh, so each refresh
    /// starts from persisted state alone (Phase 3a spec §6, scenario 2).
    reopen: bool,
    file: Option<TempDb>,
    conn: Option<Connection>,
    db: Option<Database>,
}

impl SqliteExtensionEngine {
    pub fn new() -> Self {
        SqliteExtensionEngine {
            reopen: false,
            file: None,
            conn: None,
            db: None,
        }
    }

    /// An engine that reopens its database file before every refresh.
    pub fn reopening() -> Self {
        SqliteExtensionEngine {
            reopen: true,
            ..SqliteExtensionEngine::new()
        }
    }

    fn conn(&self) -> Result<&Connection, EngineError> {
        self.conn
            .as_ref()
            .ok_or_else(|| EngineError("create_view has not been called".into()))
    }

    fn columns(&self, table: &str) -> Result<Vec<String>, EngineError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| EngineError("no database".into()))?;
        let schema = db
            .get(table)
            .ok_or_else(|| EngineError(format!("unknown table {table}")))?;
        Ok(schema
            .columns
            .iter()
            .map(|c| format!("\"{}\"", c.name))
            .collect())
    }

    fn insert(&self, table: &str, row: &Row) -> Result<(), EngineError> {
        let cols = self.columns(table)?;
        let params: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let values: Vec<SqlValue> = row.0.iter().map(sql_value).collect();
        self.conn()?
            .prepare_cached(&format!(
                "INSERT INTO \"{table}\"({}) VALUES ({})",
                cols.join(", "),
                params.join(", ")
            ))
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(err)?;
        Ok(())
    }

    fn delete_one(&self, table: &str, row: &Row) -> Result<(), EngineError> {
        let cols = self.columns(table)?;
        let same: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c} IS ?{}", i + 1))
            .collect();
        let values: Vec<SqlValue> = row.0.iter().map(sql_value).collect();
        let changed = self
            .conn()?
            .prepare_cached(&format!(
                "DELETE FROM \"{table}\" WHERE rowid = (SELECT rowid FROM \"{table}\" WHERE {} LIMIT 1)",
                same.join(" AND ")
            ))
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(err)?;
        if changed != 1 {
            return Err(EngineError(format!(
                "delete of {row:?} from {table} matched no row"
            )));
        }
        Ok(())
    }
}

impl Default for SqliteExtensionEngine {
    fn default() -> Self {
        SqliteExtensionEngine::new()
    }
}

impl Engine for SqliteExtensionEngine {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        if self.reopen {
            self.file = Some(TempDb::new());
        }
        self.conn =
            Some(open_with_extension(self.file.as_ref().map(|f| f.0.as_path())).map_err(err)?);
        self.db = Some(db.clone());
        for schema in db.tables() {
            self.conn()?
                .execute_batch(&create_table_sql(schema))
                .map_err(err)?;
            let rows = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!("table {} has no initial state", schema.table))
            })?;
            for (row, &w) in rows.iter() {
                for _ in 0..w {
                    self.insert(&schema.table, row)?;
                }
            }
        }
        let sql = view_query_to_sql(query, db).replace('\'', "''");
        self.conn()?
            .execute_batch(&format!("CREATE VIRTUAL TABLE {VIEW} USING ivm('{sql}')"))
            .map_err(err)
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        for (row, w) in raw {
            for _ in 0..w.unsigned_abs() {
                if *w > 0 {
                    self.insert(table, row)?;
                } else {
                    self.delete_one(table, row)?;
                }
            }
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        if self.reopen {
            drop(self.conn.take());
            self.conn =
                Some(open_with_extension(self.file.as_ref().map(|f| f.0.as_path())).map_err(err)?);
        }
        self.conn()?
            .execute_batch(&format!("INSERT INTO {VIEW}({VIEW}) VALUES ('refresh')"))
            .map_err(err)
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {VIEW}"))
            .map_err(err)?;
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                let mut values = Vec::with_capacity(n);
                for i in 0..n {
                    values.push(match r.get::<_, SqlValue>(i)? {
                        SqlValue::Null => Value::Null,
                        SqlValue::Integer(v) => Value::Int(v),
                        SqlValue::Text(s) => Value::Text(s),
                        other => Value::Text(format!("unexpected {other:?}")),
                    });
                }
                Ok(Row::new(values))
            })
            .map_err(err)?;
        let mut z = ZSet::new();
        for row in rows {
            z.update(row.map_err(err)?, 1);
        }
        Ok(z)
    }
}
```

- [ ] **Step 3: The lifecycle scenarios (Phase 3a spec §6, scenarios 2–6)**

`crates/ivmlite-test/tests/extension_lifecycle.rs`:

```rust
//! M1b Phase 3a spec §6, scenarios 2–6, against the real loaded extension:
//! capture from any connection, failure and retry, atomic create, rejections,
//! broken views, and drop.

use std::path::{Path, PathBuf};

use ivmlite_test::open_with_extension;
use rusqlite::types::Value;
use rusqlite::Connection;

const SUMS: &str = "SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region";
const JOIN: &str = "SELECT r.manager, SUM(o.amount) FROM orders o JOIN regions r \
                    ON o.region = r.name GROUP BY r.manager";

fn setup(c: &Connection) {
    c.execute_batch(
        "CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;
         CREATE TABLE regions(name TEXT, manager TEXT) STRICT;
         INSERT INTO orders VALUES ('a', 1), ('a', 2), ('b', 5), (NULL, 4);
         INSERT INTO regions VALUES ('a', 'ann'), ('b', 'bob'), ('c', 'bob');",
    )
    .unwrap();
}

fn create(c: &Connection, name: &str, sql: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {name} USING ivm('{}')",
        sql.replace('\'', "''")
    ))
}

fn refresh(c: &Connection, name: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!("INSERT INTO {name}({name}) VALUES ('refresh')"))
}

/// Every row `sql` returns, sorted, so results compare as multisets.
fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = c.prepare(sql).unwrap();
    let n = stmt.column_count();
    let mut out: Vec<Vec<Value>> = stmt
        .query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    out.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    out
}

/// The view agrees with SQLite computing its SELECT from scratch.
fn assert_matches_oracle(c: &Connection, name: &str, sql: &str) {
    assert_eq!(
        rows(c, &format!("SELECT * FROM {name}")),
        rows(c, sql),
        "view {name}"
    );
}

/// The names of the database's objects other than SQLite's own.
fn objects(c: &Connection) -> Vec<Vec<Value>> {
    rows(
        c,
        "SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
    )
}

/// Everything a refresh may change: every state table, the output table and
/// the watermarks.
fn durable_state(c: &Connection, name: &str) -> Vec<Vec<Vec<Value>>> {
    let tables: Vec<String> = rows(
        c,
        &format!(
            "SELECT name FROM sqlite_schema WHERE type = 'table' \
             AND (name LIKE '__ivm_state_{name}_%' OR name = '__ivm_out_{name}')"
        ),
    )
    .into_iter()
    .map(|r| match &r[0] {
        Value::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    })
    .collect();
    let mut all: Vec<Vec<Vec<Value>>> = tables
        .iter()
        .map(|t| rows(c, &format!("SELECT * FROM \"{t}\"")))
        .collect();
    all.push(rows(c, "SELECT * FROM __ivm_progress"));
    all
}

struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ivmlite-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        TempFile(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn writes_from_any_connection_are_captured_and_survive_a_reopen() {
    let file = TempFile::new("capture");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        assert_matches_oracle(&c, "sums", SUMS);
    }
    {
        // A connection that never loaded the extension: its INSERT, DELETE
        // and UPDATE are captured by the triggers (spec §8.1).
        let plain = Connection::open(file.path()).unwrap();
        plain
            .execute_batch(
                "INSERT INTO orders VALUES ('a', 10), ('c', 7);
                 DELETE FROM orders WHERE region = 'b';
                 UPDATE orders SET amount = 100 WHERE amount = 1;
                 UPDATE orders SET region = 'c' WHERE region IS NULL;",
            )
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_join_view_is_maintained_across_both_tables() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "managers", JOIN).unwrap();
    assert_matches_oracle(&c, "managers", JOIN);
    c.execute_batch(
        "INSERT INTO orders VALUES ('c', 3); UPDATE regions SET manager = 'cy' WHERE name = 'c';
         DELETE FROM regions WHERE name = 'a'; INSERT INTO regions VALUES ('a', 'ann');",
    )
    .unwrap();
    refresh(&c, "managers").unwrap();
    assert_matches_oracle(&c, "managers", JOIN);
}

/// Spec §5 and §6 scenario 3: a refresh that fails part way changes nothing
/// durable — in autocommit mode and inside an explicit transaction, whose
/// earlier statements must survive — and a retry then succeeds.
#[test]
fn a_failed_refresh_changes_nothing_and_a_retry_succeeds() {
    for explicit_transaction in [false, true] {
        for fault_on in ["__ivm_out_managers", "__ivm_state_managers_2_join_left"] {
            let c = open_with_extension(None).unwrap();
            setup(&c);
            create(&c, "managers", JOIN).unwrap();
            c.execute_batch(&format!(
                "CREATE TRIGGER fault BEFORE INSERT ON \"{fault_on}\" \
                 BEGIN SELECT RAISE(ABORT, 'injected fault'); END;"
            ))
            .unwrap();
            if explicit_transaction {
                c.execute_batch("BEGIN").unwrap();
            }
            c.execute_batch("INSERT INTO orders VALUES ('c', 3), ('a', 9)")
                .unwrap();
            let before = durable_state(&c, "managers");
            let err = refresh(&c, "managers").expect_err("the fault must fail the refresh");
            assert!(err.to_string().contains("injected fault"), "{err}");
            assert_eq!(
                durable_state(&c, "managers"),
                before,
                "a failed refresh changed durable state (transaction: {explicit_transaction}, fault on {fault_on})"
            );
            if explicit_transaction {
                assert!(!c.is_autocommit(), "the transaction must still be open");
            }
            c.execute_batch("DROP TRIGGER fault").unwrap();
            refresh(&c, "managers").unwrap();
            if explicit_transaction {
                c.execute_batch("COMMIT").unwrap();
            }
            assert_matches_oracle(&c, "managers", JOIN);
            assert_eq!(
                rows(&c, "SELECT count(*) FROM orders WHERE region = 'c'"),
                vec![vec![Value::Integer(1)]]
            );
        }
    }
}

/// Spec §6 scenario 4.
#[test]
fn creating_a_view_is_atomic() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let user_objects = objects(&c);

    c.execute_batch("BEGIN").unwrap();
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        objects(&c),
        user_objects,
        "a rolled-back create left objects behind"
    );

    // A create that fails after creating shadow tables (the output column
    // name `__w` is reserved and is checked after them), inside a
    // transaction: nothing is left, and the transaction's earlier work is.
    c.execute_batch("BEGIN; INSERT INTO orders VALUES ('z', 1);")
        .unwrap();
    let err = create(
        &c,
        "bad",
        "SELECT region, COUNT(*) AS __w FROM orders GROUP BY region",
    )
    .expect_err("__w is reserved");
    assert!(err.to_string().contains("__w"), "{err}");
    assert_eq!(objects(&c), user_objects);
    c.execute_batch("COMMIT").unwrap();
    assert_eq!(
        rows(&c, "SELECT count(*) FROM orders WHERE region = 'z'"),
        vec![vec![Value::Integer(1)]]
    );

    // Bootstrap over tables that already hold rows.
    create(&c, "sums", SUMS).unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Spec §6 scenario 5: each rejection names its problem.
#[test]
fn what_v0_cannot_maintain_is_rejected_by_name() {
    let q = "SELECT k, COUNT(*) FROM t GROUP BY k";
    let cases = [
        ("CREATE TABLE t(k TEXT, v INTEGER)", q, "not STRICT"),
        ("CREATE TABLE t(k ANY, v INTEGER) STRICT", q, "type ANY"),
        ("CREATE TABLE t(k TEXT COLLATE NOCASE, v INTEGER) STRICT", q, "COLLATE"),
        ("CREATE TABLE t(k TEXT, v REAL) STRICT", q, "type REAL"),
        ("PRAGMA encoding = 'UTF-16le'; CREATE TABLE t(k TEXT) STRICT", q, "UTF-8"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k, COUNT(*) FROM nope GROUP BY k", "no such table: nope"),
        ("CREATE TABLE t(k TEXT) STRICT; CREATE VIEW vv AS SELECT * FROM t", "SELECT k, COUNT(*) FROM vv GROUP BY k", "view"),
        ("CREATE TABLE __ivm_x(k TEXT) STRICT", "SELECT k, COUNT(*) FROM __ivm_x GROUP BY k", "ivmlite's own"),
        ("CREATE TABLE t(k TEXT) STRICT; CREATE VIRTUAL TABLE a USING ivm('SELECT k, COUNT(*) FROM t GROUP BY k')", q, "already tracked"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k, COUNT(*) AS w FROM t GROUP BY k", "like the view"),
        ("CREATE TABLE t(k TEXT) STRICT", "SELECT k FROM t", "GROUP BY"),
    ];
    for (setup_sql, sql, expected) in cases {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(setup_sql).unwrap();
        let err = create(&c, "w", sql).expect_err(setup_sql);
        assert!(
            err.to_string().contains(expected),
            "{setup_sql} / {sql}: {err}"
        );
    }
}

#[test]
fn a_view_accepts_only_the_refresh_command() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    for (sql, expected) in [
        (
            "INSERT INTO sums(sums) VALUES ('rebuild')",
            "unknown ivmlite command",
        ),
        ("INSERT INTO sums(region) VALUES ('x')", "read-only"),
        ("UPDATE sums SET region = 'x'", "read-only"),
        ("DELETE FROM sums", "read-only"),
    ] {
        let err = c.execute_batch(sql).expect_err(sql);
        assert!(err.to_string().contains(expected), "{sql}: {err}");
    }
}

/// Spec §6 scenarios 5 and 6: a view whose stored plan no longer matches, or
/// whose state table is missing, reports why on every use — and can still
/// be dropped, leaving the base tables writable.
#[test]
fn a_broken_view_reports_why_and_can_still_be_dropped() {
    for (breakage, expected) in [
        ("UPDATE __ivm_view SET plan = 'tampered'", "different plan"),
        ("DROP TABLE __ivm_state_sums_0_agg_groups", "is missing"),
    ] {
        let file = TempFile::new("broken");
        {
            let c = open_with_extension(Some(file.path())).unwrap();
            setup(&c);
            create(&c, "sums", SUMS).unwrap();
        }
        Connection::open(file.path())
            .unwrap()
            .execute_batch(breakage)
            .unwrap();
        let c = open_with_extension(Some(file.path())).unwrap();
        for sql in [
            "SELECT * FROM sums",
            "INSERT INTO sums(sums) VALUES ('refresh')",
        ] {
            let err = c.execute_batch(sql).expect_err(sql);
            assert!(
                err.to_string().contains(expected),
                "{breakage} / {sql}: {err}"
            );
        }
        c.execute_batch("DROP TABLE sums").unwrap();
        assert_eq!(
            objects(&c),
            rows(
                &c,
                "SELECT type, name FROM sqlite_schema WHERE name IN ('orders', 'regions')"
            )
        );
        c.execute_batch("INSERT INTO orders VALUES ('a', 1)")
            .unwrap();
    }
}

/// Spec §6 scenario 6 (M-1 scenario 9).
#[test]
fn drop_removes_every_shadow_object_and_the_base_tables_stay_writable() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    let user_objects = objects(&c);
    create(&c, "managers", JOIN).unwrap();
    c.execute_batch("DROP TABLE managers").unwrap();
    assert_eq!(objects(&c), user_objects);
    c.execute_batch("INSERT INTO orders VALUES ('a', 1); INSERT INTO regions VALUES ('d', 'dee');")
        .unwrap();
}
```

- [ ] **Step 4: The differential sweeps (scenarios 1 and 2)**

`crates/ivmlite-test/tests/extension_differential.rs`:

```rust
//! M1b Phase 3a spec §6, scenarios 1 and 2: the differential harness against
//! the real loaded extension, with and without reopening the database before
//! every refresh.
//!
//! Strides are set from measured cost (debug build, 2026-09-26): about 9 ms a
//! single-table case and 14 ms a join case in memory, about 60 ms a case when
//! reopening a database file before every refresh. Every join query was run
//! in full once while the plan was prototyped (1800 cases, all green).

use ivmlite_test::{
    enumerate, enumerate_join, gen_case_with_query, gen_database,
    gen_database_with_swapped_right_table, run, seed_range, Batching, Database, Domain,
    SqliteExtensionEngine, ViewQuery,
};

/// Run every `stride`-th query of `queries`, one case each, seeded by its
/// index. Under `IVMLITE_SEED=<n>` only query `n` runs, as in the harness's
/// other sweeps.
fn sweep(db: &Database, queries: &[ViewQuery], stride: usize, make: fn() -> SqliteExtensionEngine) {
    let domain = Domain::default();
    let selected: Vec<u64> = if std::env::var("IVMLITE_SEED").is_ok() {
        seed_range()
    } else {
        (0..queries.len() as u64).step_by(stride).collect()
    };
    assert!(!selected.is_empty());
    for seed in selected {
        let query = queries[seed as usize % queries.len()].clone();
        let case = gen_case_with_query(seed, db, &domain, query, 20, 60, Batching::Chunks(4));
        let mut engine = make();
        if let Err(f) = run(&mut engine, &case) {
            panic!("the SQLite extension disagrees with the oracle on query {seed}: {f}");
        }
    }
}

#[test]
fn the_extension_is_green_across_the_single_table_space() {
    let db = gen_database(2);
    sweep(
        &db,
        &enumerate(&db.tables()[0]),
        1,
        SqliteExtensionEngine::new,
    );
}

#[test]
fn the_extension_is_green_across_the_join_space() {
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 5, SqliteExtensionEngine::new);
    }
}

/// Every refresh starts from a freshly opened database file: what the view
/// knows it knows from its shadow tables alone.
#[test]
fn the_extension_resumes_from_persisted_state_on_every_refresh() {
    let db = gen_database(2);
    sweep(
        &db,
        &enumerate(&db.tables()[0]),
        3,
        SqliteExtensionEngine::reopening,
    );
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 25, SqliteExtensionEngine::reopening);
    }
}
```

- [ ] **Step 5: Run everything**

`scripts/test-all.sh`: green — the prototype counted 288 across both cargo invocations (the workspace plus the extension's 4). Also run `cargo test -p ivmlite-test --locked --test extension_lifecycle` once **after deleting the built library** and confirm it fails with the "not built … run scripts/build-extension.sh" message, then rebuild. fmt, clippy (both builds).

- [ ] **Step 6: CI**

In `.github/workflows/ci.yml`, replace the three `run` steps with:

```yaml
      - run: cargo fmt --all -- --check
      - run: cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - run: cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings
      - run: scripts/test-all.sh
```

- [ ] **Step 7: Gate rows** — a new section `## ivmlite-sqlite` in `docs/mutation-gates.md`, after `## ivmlite-sql`. Each mutation below was run while prototyping, with the result shown; run each again with `scripts/test-all.sh` and record what you observe.

| Spec requirement | Mutation (in `crates/ivmlite-sqlite/src/`) | Prototype: red tests |
|---|---|---|
| Phase 3a §5: state, output and watermarks change in one statement | In `view.rs`'s `apply`, before the arming `UPDATE`, add `exec(conn, &format!("UPDATE {stage} SET armed = 1 WHERE op = 'progress'"))?;` | `a_failed_refresh_changes_nothing_and_a_retry_succeeds` (285/1) |
| Phase 3a §5: later reads in one refresh see its earlier buffered writes | In `state.rs`'s `BufferedArrangement::get`, delete the `overlay(…)` call | `a_failed_refresh_changes_nothing_and_a_retry_succeeds`, `a_join_view_is_maintained_across_both_tables`, `the_extension_is_green_across_the_join_space`, `the_extension_resumes_from_persisted_state_on_every_refresh` (282/4) |
| §7.3 / Phase 3a §5: a refresh consumes exactly the deltas after the watermark | In `view.rs`'s `read_deltas`, `seq > ?1` → `seq >= ?1` | the three `extension_differential` sweeps (283/3) |
| §8.1: an UPDATE is captured as a retraction plus an insertion | In `create_delta_table`, delete the update trigger's `VALUES (-1, {old})` insert | `a_join_view_is_maintained_across_both_tables`, `writes_from_any_connection_are_captured_and_survive_a_reopen` (284/2) |
| §4.4 / Phase 3a §5: a plan mismatch on reopen is reported | In `verify`, `if now != plan` → `if false && now != plan` | `a_broken_view_reports_why_and_can_still_be_dropped` (285/1) |
| M-1 scenario 9 / Phase 3a §5: drop removes the triggers first | In `destroy`, replace the `DROP TRIGGER IF EXISTS …` statement with `SELECT {literal of the name}` | `a_broken_view_reports_why_and_can_still_be_dropped`, `drop_removes_every_shadow_object_and_the_base_tables_stay_writable` (284/2) |
| §7.1: a non-STRICT table is refused | In `catalog.rs`, `if !strict` → `if false && !strict` | `what_v0_cannot_maintain_is_rejected_by_name` (285/1) |

Add rows for these too, measured (not prototyped): the UTF-8 check (`require_utf8` returns `Ok(())` unconditionally); the `COLLATE` check; the `__ivm_` prefix check; the "already tracked" check in `create`; the command-column clash in `declaration`; the canonical encoding (`encode`'s INTEGER tag changed to `0x03` → `the_encoding_is_the_documented_layout`); `is_state_table_of` accepting any suffix (→ `a_views_state_tables_are_recognized_exactly`); and one n/a row: "every callback runs inside `guard`" — `None — no input reaches a panic in the extension today, so removing a guard changes nothing observable; the guard is checked by review`.

- [ ] **Step 8: Commit**

```bash
git add crates/ivmlite-test/Cargo.toml crates/ivmlite-test/src/lib.rs crates/ivmlite-test/src/extension.rs crates/ivmlite-test/tests/extension_lifecycle.rs crates/ivmlite-test/tests/extension_differential.rs .github/workflows/ci.yml docs/mutation-gates.md
git commit -m "test(harness): run the differential harness and lifecycle scenarios against the loaded extension

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Documentation

**Files:** Modify `docs/mutation-gates.md`, `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, `docs/README.md`.

- [ ] **Step 1: The mutation-gate protocol**

In `docs/mutation-gates.md`'s "How to use it", replace the instruction to use `cargo test --workspace --locked --no-fail-fast` with `scripts/test-all.sh`: it rebuilds the extension first, so a mutation in core, the SQL front end or the extension reaches the extension tests (which otherwise run against a stale library and fail with a rebuild instruction); sum `passed`/`failed` over every test binary of both cargo invocations it makes; a run takes about 40 seconds. Keep the existing explanation of why `--no-fail-fast` matters.

- [ ] **Step 2: The parent spec**

In `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, with dated "**Amended 2026-09-26, M1b Phase 3a:**" paragraphs that point to the Phase 3a spec for detail:
- §4.1: `ivmlite-sqlite` is built outside the Cargo workspace, and why (feature unification).
- §4.4: its open questions are answered — the `Arrangement` signature is fallible with eager reads; the plan fingerprint is the canonical plan text stored per view; pre-order numbering is kept with fixed role strings; a missing state table on reopen is an error, not an empty table; the provider is fallible. Point 2 (the in-memory output) is closed: the output is `__ivm_out_<view>`. The `unreachable!` arm of `passes` is guarded by strict decoding (a non-INTEGER/TEXT value read from a base or delta table is an error) and every callback's `catch_unwind`.
- §7: the table list gains `declaration` in `__ivm_view`, `__ivm_meta`, and the per-view `__ivm_stage_<view>` with its apply trigger.
- §8.2: a refresh inside a transaction is atomic by one-statement apply, not a savepoint, and why (measured).
- §13: add "a view's result columns may not be named like the view or `__w`" and "Phase 3a maintains one view per base table" (Phase 3b lifts it).

- [ ] **Step 3: The index**

`docs/README.md`: under specs add
`  - [`2026-09-26-m1b-phase3a-sqlite-extension-design.md`](superpowers/specs/2026-09-26-m1b-phase3a-sqlite-extension-design.md) — M1b Phase 3a: the SQLite extension, one view end to end`
and under plans, after the Phase 2b entry,
`  - [`2026-09-26-m1b-phase3a-sqlite-extension.md`](superpowers/plans/2026-09-26-m1b-phase3a-sqlite-extension.md) — M1b Phase 3a: the SQLite extension`

- [ ] **Step 4: Verify and commit**

`scripts/test-all.sh`, fmt, clippy (both builds), `python3 scripts/count-mutation-gates.py`.

```bash
git add docs/mutation-gates.md docs/superpowers/specs/2026-09-18-ivmlite-design.md docs/README.md
git commit -m "docs: record the SQLite extension in the spec, the gate protocol and the index

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage (Phase 3a spec)

| Section | Where |
|---|---|
| §2 build and test layout | Tasks 3, 4 (scripts, fixed path, fail-not-skip, CI) |
| §3 fallible state, `Plan::canonical` | Task 2 |
| §4 persistence conventions | Task 3 (`names.rs`, `view.rs`, `encode.rs`) |
| §5 lifecycle, one-statement apply, broken views, `catch_unwind` | Task 3 (`view.rs`, `vtab.rs`, `state.rs`) |
| §6 acceptance scenarios 1–6 | Task 4 |
| §7 front-end fixes | Task 1 |
