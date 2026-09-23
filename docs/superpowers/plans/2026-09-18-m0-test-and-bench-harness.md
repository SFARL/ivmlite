# M0 Test and Benchmark Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Before writing any engine code, build an IVM differential testing harness that can prove it reports errors, and a benchmark harness that can produce baseline curves.

**Architecture:** The testing harness is decoupled from the implementation under test through an `Engine` trait. M0 provides two implementations — `NaiveRecompute` (trivially correct, doubling as a benchmark baseline) and `NoRetractionEngine` (with a deliberately planted bug: its aggregate does not retract old rows). The correctness judge comes from an **independent** oracle: load the final state into an in-memory SQLite and run the original SQL. The harness must turn the former all green, and catch the latter and shrink it to a minimal case.

**Tech Stack:** Rust 1.95+, `rusqlite` (bundled SQLite), `rand` (`StdRng` + seed), GitHub Actions. M0 does not bring in `sqlparser-rs` or `criterion`.

**Spec:** [`docs/superpowers/specs/2026-09-18-ivmlite-design.md`](../specs/2026-09-18-ivmlite-design.md)

## Global Constraints

- **`ivmlite-core` must not depend on `rusqlite` or `libsqlite3-sys`** (spec §4.2). `ivmlite-test` may.
- **All `unsafe` and FFI may appear only in `ivmlite-sqlite`** (spec §4.2). M0 does not create that crate, so M0 has zero `unsafe` throughout.
- **v0's `Value` has only three variants, `Null` / `Int` / `Text`**. No floating point (spec §9: floating-point SUM is not associative), no BLOB. `Real` and `Blob` wait until M4.
- **The view's root operator must be an `Aggregate` with a non-empty GROUP BY** (spec §5.2). Views with no aggregate are not generated, and neither are global aggregates — the latter return 1 row of NULL over an empty set while grouped aggregates return 0 rows, so the two cannot share the rule "delete the row when the count reaches zero".
- **The integer value domain must guarantee no overflow** (spec §6.1): SQLite's `SUM` errors on integer overflow, and whether it errors depends on scan order, so incremental maintenance and full recomputation diverge in the overflow zone — the same class of problem as floating-point associativity. Under the default `Domain`, per-group sums stay far below 2^62, and the generator must not widen to a domain that could overflow.
- **The whitelist of comparison operators** (spec §6.1): `>` `>=` `<` `<=` `=` `!=` `IS NULL` `IS NOT NULL`. `NOT` / `OR` / `LIKE` / `IN` / `BETWEEN` and any subquery are not generated.
- **The weight invariant** (spec §5.1): the final materialised state must have no negative weights; a row whose weight reaches zero must be deleted, with no `w = 0` zombie rows left.
- **The generator's value domain must be narrow** (spec §9.2): 8 distinct values per column by default, a NULL probability of 0.2 by default.
- **All randomness goes through an explicit seed**, and a failure prints a replayable seed (spec §9.4).
- **`ZSet` uses a `BTreeMap` internally, not a `HashMap`**, so iteration order is deterministic and tests are reproducible.
- **Workloads must be portable artifacts** (spec §10.3 item 7): the schema DDL, view SQL, data and update trace are defined in `workloads/*.toml`, parsed by `ivmlite-workload`, and exportable as CSV/SQL. There is one runner per engine and only one workload.
- **Never put another project's publicly published numbers into a comparison table** (spec §10.3 item 6). Comparing with Turso or other systems means running them yourself, on the same machine, with the same workload.
- **Group cardinality is an explicit benchmark dimension, not a constant** (spec §10.1). It decides whether IVM wins more than base-table size does; numbers reported at a single cardinality do not make a conclusion.
- M0 **does not create** `ivmlite-sql` or `ivmlite-sqlite` — they have nothing to hold until M1. This is a narrowing of spec §11's "crate skeleton", on YAGNI grounds.

---

## File Structure

```
Cargo.toml                              workspace
.github/workflows/ci.yml                fmt + clippy + test
crates/
  ivmlite-core/
    Cargo.toml
    src/lib.rs                          pub use
    src/value.rs                        Value
    src/row.rs                          Row
    src/zset.rs                         ZSet
  ivmlite-test/
    Cargo.toml
    src/lib.rs                          pub use
    src/schema.rs                       Column / ColumnType / Schema
    src/query.rs                        AggFn / Agg / Predicate / ViewQuery / enumerate
    src/data.rs                         Domain / initial data generation
    src/ops.rs                          Op / OpGenerator (biased sampling)
    src/engine.rs                       Engine trait / EngineError
    src/naive.rs                        NaiveRecompute
    src/buggy.rs                        NoRetractionEngine
    src/oracle.rs                       recompute_via_sqlite
    src/invariants.rs                   the invariant layer of the four layers of assertions
    src/differential.rs                 TestCase / Batching / run / Failure / seed_range
    src/shrink.rs                       a legality-preserving shrinker (ops → query → data)
    src/regression.rs                   freezing failing cases and reading them back
    tests/harness_catches_bugs.rs       M0's completion criteria
    tests/regressions/*.json            frozen historical failing cases (committed to the repository)
  ivmlite-workload/
    Cargo.toml
    src/lib.rs                          Workload definition, data and trace generation, export
  ivmlite-bench/
    Cargo.toml
    src/main.rs                         matrix driver + CSV output
    src/baseline.rs                     no maintenance / hand-written triggers / naive re-run
    src/plot.rs                         baseline-curve SVGs
workloads/
  m0-baseline.toml                      the portable workload definition (shared across runners)
docs/bench/
  m0-baseline.csv                       full matrix results
  m0-baseline-card{10,1000,100000}.svg  one headline chart per group cardinality
  README.md                             where the crossover falls at each group cardinality, written as measured
```

---

## Task 1: Workspace skeleton, `Value`, `Row`, CI

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `.github/workflows/ci.yml`
- Create: `crates/ivmlite-core/Cargo.toml`, `crates/ivmlite-core/src/lib.rs`
- Create: `crates/ivmlite-core/src/value.rs`, `crates/ivmlite-core/src/row.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `ivmlite_core::Value` (an enum with variants `Null` / `Int(i64)` / `Text(String)`), `ivmlite_core::Row` (a newtype `Row(pub Vec<Value>)`, methods `new(Vec<Value>) -> Row`, `get(usize) -> &Value`, `len() -> usize`). Both derive `Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash`.

- [ ] **Step 1: Write the workspace and crate manifests**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/ivmlite-core"]

[workspace.package]
edition = "2021"
rust-version = "1.95"
license = "MIT"
repository = "https://github.com/SFARL/ivmlite"

[workspace.dependencies]
ivmlite-core = { path = "crates/ivmlite-core" }
rusqlite = { version = "0.40", features = ["bundled"] }
rand = "0.10"
```

`.gitignore`:

```
/target
Cargo.lock
```

> Cargo.lock is ignored because this repository currently produces only libraries and an internal bench bin. When M1 produces a cdylib, switch to committing the lock.

`crates/ivmlite-core/Cargo.toml`:

```toml
[package]
name = "ivmlite-core"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
```

The empty dependency table is **deliberate**, matching the first Global Constraint.

- [ ] **Step 2: Write the failing tests**

`crates/ivmlite-core/src/value.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_and_text_with_same_digits_are_distinct() {
        assert_ne!(Value::Int(1), Value::Text("1".to_string()));
    }

    #[test]
    fn null_orders_before_everything() {
        let mut vs = vec![Value::Text("a".into()), Value::Int(3), Value::Null];
        vs.sort();
        assert_eq!(vs[0], Value::Null);
    }
}
```

`crates/ivmlite-core/src/row.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    #[test]
    fn row_exposes_values_by_index() {
        let r = Row::new(vec![Value::Int(7), Value::Null]);
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(0), &Value::Int(7));
        assert_eq!(r.get(1), &Value::Null);
    }

    #[test]
    fn rows_sort_deterministically() {
        let a = Row::new(vec![Value::Int(1)]);
        let b = Row::new(vec![Value::Int(2)]);
        let mut v = vec![b.clone(), a.clone()];
        v.sort();
        assert_eq!(v, vec![a, b]);
    }
}
```

- [ ] **Step 3: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-core`
Expected: a compile failure, `cannot find type Value` / `cannot find type Row`

- [ ] **Step 4: Write the implementation**

At the top of `crates/ivmlite-core/src/value.rs`:

```rust
/// v0's value domain. Real and Blob are deliberately excluded:
/// Real would stop incremental SUM from being bit-for-bit equal to full recomputation (floating-point addition is not associative),
/// and Blob is not needed under v0's STRICT table restrictions. Both are scheduled for M4.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
}
```

At the top of `crates/ivmlite-core/src/row.rs`:

```rust
use crate::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Row(pub Vec<Value>);

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Row(values)
    }

    pub fn get(&self, index: usize) -> &Value {
        &self.0[index]
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

`crates/ivmlite-core/src/lib.rs`:

```rust
mod row;
mod value;

pub use row::Row;
pub use value::Value;
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-core`
Expected: everything passes (4 new in this task)

- [ ] **Step 6: Add CI**

`.github/workflows/ci.yml`:

```yaml
name: ci
on:
  push:
    branches: [master]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 7: Confirm the same three commands pass locally and in CI**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: everything passes.

> `members` containing only `ivmlite-core` at this point is correct — `ivmlite-test` (Task 3),
> `ivmlite-workload` (Task 12) and `ivmlite-bench` (Task 13) each add themselves when they are created.
> Do not list crates that do not exist yet here; that makes `cargo` refuse to load the workspace outright.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml .gitignore .github/workflows/ci.yml crates/ivmlite-core
git commit -m "feat(core): workspace skeleton, the Value/Row data types, and CI"
```

---

## Task 2: `ZSet`

**Files:**
- Create: `crates/ivmlite-core/src/zset.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`

**Interfaces:**
- Consumes: `Row` (Task 1)
- Produces: `ivmlite_core::ZSet`, methods `new() -> ZSet`, `from_rows(impl IntoIterator<Item = (Row, i64)>) -> ZSet`, `update(&mut self, Row, i64)`, `merge(&mut self, &ZSet)`, `weight_of(&self, &Row) -> i64`, `iter(&self) -> impl Iterator<Item = (&Row, &i64)>`, `len(&self) -> usize`, `is_empty(&self) -> bool`. Derives `Debug, Clone, Default, PartialEq, Eq`.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-core/src/zset.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Int(n)])
    }

    #[test]
    fn weights_accumulate() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), 2);
        assert_eq!(z.weight_of(&row(1)), 3);
    }

    #[test]
    fn zero_weight_rows_are_removed_not_kept() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), -1);
        assert_eq!(z.weight_of(&row(1)), 0);
        assert_eq!(z.len(), 0, "a row whose weight reaches zero must be deleted, not left as a w=0 zombie row");
        assert!(z.is_empty());
    }

    #[test]
    fn update_with_zero_weight_is_a_noop() {
        let mut z = ZSet::new();
        z.update(row(1), 0);
        assert_eq!(z.len(), 0);
    }

    #[test]
    fn negative_weights_are_representable() {
        let mut z = ZSet::new();
        z.update(row(1), -2);
        assert_eq!(z.weight_of(&row(1)), -2);
    }

    #[test]
    fn merge_is_pointwise_addition() {
        let mut a = ZSet::from_rows([(row(1), 1), (row(2), 5)]);
        let b = ZSet::from_rows([(row(1), -1), (row(3), 2)]);
        a.merge(&b);
        assert_eq!(a.weight_of(&row(1)), 0);
        assert_eq!(a.weight_of(&row(2)), 5);
        assert_eq!(a.weight_of(&row(3)), 2);
        assert_eq!(a.len(), 2, "row(1) should be removed after reaching zero");
    }

    #[test]
    fn iteration_order_is_deterministic() {
        let a = ZSet::from_rows([(row(3), 1), (row(1), 1), (row(2), 1)]);
        let seen: Vec<i64> = a
            .iter()
            .map(|(r, _)| match r.get(0) {
                Value::Int(n) => *n,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(seen, vec![1, 2, 3], "BTreeMap guarantees order; HashMap does not");
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-core zset`
Expected: a compile failure, `cannot find type ZSet`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-core/src/zset.rs`:

```rust
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use crate::Row;

/// A weighted multiset. Weights are i64: INSERT = +1, DELETE = -1.
///
/// BTreeMap rather than HashMap, so that iteration order is deterministic — a failing differential-testing case
/// must replay exactly from its seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZSet {
    inner: BTreeMap<Row, i64>,
}

impl ZSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: impl IntoIterator<Item = (Row, i64)>) -> Self {
        let mut z = Self::new();
        for (row, weight) in rows {
            z.update(row, weight);
        }
        z
    }

    /// Add `weight` to `row`'s existing weight. A row that reaches zero is removed.
    pub fn update(&mut self, row: Row, weight: i64) {
        if weight == 0 {
            return;
        }
        match self.inner.entry(row) {
            Entry::Occupied(mut slot) => {
                let combined = *slot.get() + weight;
                if combined == 0 {
                    slot.remove();
                } else {
                    *slot.get_mut() = combined;
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(weight);
            }
        }
    }

    pub fn merge(&mut self, other: &ZSet) {
        for (row, weight) in other.iter() {
            self.update(row.clone(), *weight);
        }
    }

    pub fn weight_of(&self, row: &Row) -> i64 {
        self.inner.get(row).copied().unwrap_or(0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Row, &i64)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
```

Append to `crates/ivmlite-core/src/lib.rs`:

```rust
mod zset;
pub use zset::ZSet;
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-core`
Expected: everything passes (6 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-core/src/zset.rs crates/ivmlite-core/src/lib.rs
git commit -m "feat(core): ZSet, with rows deleted when their weight reaches zero"
```

---

## Task 3: `Schema` and the `ivmlite-test` crate

**Files:**
- Create: `crates/ivmlite-test/Cargo.toml`, `crates/ivmlite-test/src/lib.rs`, `crates/ivmlite-test/src/schema.rs`
- Modify: `Cargo.toml` (add `ivmlite-test` back to members)

**Interfaces:**
- Consumes: `ivmlite_core::{Row, Value, ZSet}`
- Produces: `ColumnType` (`Integer` / `Text`), `Column { name: String, ty: ColumnType, nullable: bool }`, `Schema { table: String, columns: Vec<Column> }`, methods `Schema::create_table_sql(&self) -> String`, `Schema::arity(&self) -> usize`, `Schema::column_names(&self) -> Vec<&str>`.

- [ ] **Step 1: Create the crate manifest**

`crates/ivmlite-test/Cargo.toml`:

```toml
[package]
name = "ivmlite-test"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
ivmlite-core.workspace = true
rusqlite.workspace = true
rand.workspace = true
```

and add `ivmlite-test` back to the `members` of the root `Cargo.toml`.

- [ ] **Step 2: Write the failing tests**

`crates/ivmlite-test/src/schema.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn create_table_sql_is_strict() {
        let sql = orders().create_table_sql();
        assert!(sql.contains("STRICT"), "spec §7.1 requires a STRICT table: {sql}");
        assert!(sql.contains("\"region\" TEXT"));
        assert!(sql.contains("\"amount\" INTEGER NOT NULL"));
    }

    #[test]
    fn arity_counts_columns() {
        assert_eq!(orders().arity(), 2);
    }
}
```

- [ ] **Step 3: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test`
Expected: a compile failure, `cannot find type Schema`

- [ ] **Step 4: Write the implementation**

At the top of `crates/ivmlite-test/src/schema.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    Text,
}

impl ColumnType {
    fn sql(self) -> &'static str {
        match self {
            ColumnType::Integer => "INTEGER",
            ColumnType::Text => "TEXT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
    pub nullable: bool,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub table: String,
    pub columns: Vec<Column>,
}

impl Schema {
    /// Generate a STRICT CREATE TABLE statement. STRICT is a hard requirement of v0: it pins column types,
    /// eliminating the whole class of problems where SQLite's type affinity splits a group key (spec §7.1).
    pub fn create_table_sql(&self) -> String {
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                let null = if c.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", c.name, c.ty.sql(), null)
            })
            .collect();
        format!(
            "CREATE TABLE \"{}\" ({}) STRICT",
            self.table,
            cols.join(", ")
        )
    }

    pub fn arity(&self) -> usize {
        self.columns.len()
    }

    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}
```

`crates/ivmlite-test/src/lib.rs`:

```rust
mod schema;

pub use schema::{Column, ColumnType, Schema};
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (2 new in this task)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/ivmlite-test
git commit -m "feat(test): the ivmlite-test crate and STRICT schemas"
```

---

## Task 4: `ViewQuery` and exhaustive enumeration of the query space

**Files:**
- Create: `crates/ivmlite-test/src/query.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ColumnType` (Task 3)
- Produces: `AggFn` (`Count` / `Sum`), `Agg { func: AggFn, column: Option<usize> }`, `Predicate` (`None` / `IntGt { column, value }` / `IsNotNull { column }`), `ViewQuery { group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate }`, methods `ViewQuery::to_sql(&self, &Schema) -> String`, `ViewQuery::output_arity(&self) -> usize`; the free function `enumerate(&Schema) -> Vec<ViewQuery>`.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/query.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn to_sql_renders_group_by_and_aggs() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        assert_eq!(
            q.to_sql(&orders()),
            "SELECT \"region\", SUM(\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"region\""
        );
    }

    #[test]
    fn to_sql_renders_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::IntGt { column: 1, value: 3 },
        };
        assert!(q.to_sql(&orders()).contains("WHERE \"amount\" > 3"));
    }

    #[test]
    fn output_arity_is_group_by_plus_aggs() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        assert_eq!(q.output_arity(), 3);
    }

    #[test]
    fn enumerate_covers_the_v0_space_and_is_nonempty() {
        let qs = enumerate(&orders());
        assert!(!qs.is_empty());
        assert!(
            qs.iter().all(|q| !q.group_by.is_empty()),
            "v0 forbids global aggregates: over an empty table `SELECT SUM(v) FROM t` returns 1 row of NULL, \
             while `... GROUP BY g` returns 0 rows, so the two cannot share one row-deletion rule (spec §5.2)"
        );
        assert!(
            qs.iter().all(|q| !q.aggs.is_empty()),
            "v0's root operator must be an Aggregate — otherwise the __w weights make the materialised table's row count disagree with an ordinary SQL view (spec §5.2)"
        );
        assert!(qs.iter().any(|q| matches!(q.predicate, Predicate::None)));
        assert!(qs.iter().any(|q| matches!(q.predicate, Predicate::IntGt { .. })));
    }

    #[test]
    fn enumerate_only_sums_integer_columns() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Sum {
                    let idx = agg.column.expect("SUM must have a column");
                    assert_eq!(
                        schema.columns[idx].ty,
                        ColumnType::Integer,
                        "v0 has no floating point; SUM may only apply to INTEGER columns"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test query`
Expected: a compile failure, `cannot find type ViewQuery`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/query.rs`:

```rust
use crate::{ColumnType, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Count,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agg {
    pub func: AggFn,
    /// None for COUNT(*); SUM must be Some, pointing at an INTEGER column.
    pub column: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    None,
    IntGt { column: usize, value: i64 },
    IsNotNull { column: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// v0 allows only bare columns as group-by keys, not expressions (spec §7.1).
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
}

impl ViewQuery {
    pub fn output_arity(&self) -> usize {
        self.group_by.len() + self.aggs.len()
    }

    pub fn to_sql(&self, schema: &Schema) -> String {
        let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

        let mut select: Vec<String> = self.group_by.iter().map(|i| name(*i)).collect();
        for agg in &self.aggs {
            select.push(match (agg.func, agg.column) {
                (AggFn::Count, _) => "COUNT(*)".to_string(),
                (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
                (AggFn::Sum, None) => unreachable!("SUM must name a column"),
            });
        }

        let where_clause = match &self.predicate {
            Predicate::None => String::new(),
            Predicate::IntGt { column, value } => {
                format!(" WHERE {} > {}", name(*column), value)
            }
            Predicate::IsNotNull { column } => {
                format!(" WHERE {} IS NOT NULL", name(*column))
            }
        };

        let group = self
            .group_by
            .iter()
            .map(|i| name(*i))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "SELECT {} FROM \"{}\"{} GROUP BY {}",
            select.join(", "),
            schema.table,
            where_clause,
            group
        )
    }
}

/// Exhaustively enumerate v0's query space.
///
/// spec §9.2: v0 has a finite number of combinations, and exhaustive beats random — reproducible and fully covering.
/// Randomness is left to the update sequences.
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    let int_cols: Vec<usize> = (0..schema.arity())
        .filter(|i| schema.columns[*i].ty == ColumnType::Integer)
        .collect();

    let mut group_by_choices: Vec<Vec<usize>> = Vec::new();
    for i in 0..schema.arity() {
        group_by_choices.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            group_by_choices.push(vec![i, j]);
        }
    }

    let mut agg_choices: Vec<Vec<Agg>> = vec![vec![Agg { func: AggFn::Count, column: None }]];
    for i in &int_cols {
        agg_choices.push(vec![Agg { func: AggFn::Sum, column: Some(*i) }]);
        agg_choices.push(vec![
            Agg { func: AggFn::Sum, column: Some(*i) },
            Agg { func: AggFn::Count, column: None },
        ]);
    }

    let mut predicates = vec![Predicate::None];
    for i in &int_cols {
        predicates.push(Predicate::IntGt { column: *i, value: 4 });
    }
    for i in 0..schema.arity() {
        if schema.columns[i].nullable {
            predicates.push(Predicate::IsNotNull { column: i });
        }
    }

    let mut out = Vec::new();
    for group_by in &group_by_choices {
        for aggs in &agg_choices {
            for predicate in &predicates {
                out.push(ViewQuery {
                    group_by: group_by.clone(),
                    aggs: aggs.clone(),
                    predicate: predicate.clone(),
                });
            }
        }
    }
    out
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod query;
pub use query::{enumerate, Agg, AggFn, Predicate, ViewQuery};
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (5 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): ViewQuery and exhaustive enumeration of v0's query space"
```

---

## Task 5: Data generator (narrow value domain, high NULL rate)

**Files:**
- Create: `crates/ivmlite-test/src/data.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ColumnType` (Task 3), `ivmlite_core::{Row, Value}`
- Produces: `Domain { distinct: usize, null_rate: f64 }` (`Default` is `distinct: 8, null_rate: 0.2`), `gen_row(&mut StdRng, &Schema, &Domain) -> Row`, `gen_rows(&mut StdRng, &Schema, &Domain, usize) -> Vec<Row>`.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/data.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};
    use rand::SeedableRng;
    use std::collections::HashSet;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn domain_is_narrow_by_default() {
        let d = Domain::default();
        assert_eq!(d.distinct, 8, "a narrow value domain is the precondition for catching retraction bugs (spec §9.2)");
    }

    /// spec §6.1: SQLite's integer SUM errors on overflow, and whether it errors depends on scan order,
    /// so incremental maintenance and full recomputation diverge in the overflow zone. The generator must make overflow unreachable.
    #[test]
    fn domain_cannot_overflow_integer_sum() {
        let d = Domain::default();
        let worst_case_sum = (d.distinct as i128) * 1_000_000;
        assert!(
            worst_case_sum < (1i128 << 62),
            "even with a million rows all in the same group, the sum must stay far below 2^62"
        );
    }

    #[test]
    fn generated_values_stay_within_the_domain() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let schema = orders();
        let domain = Domain::default();
        let rows = gen_rows(&mut rng, &schema, &domain, 500);

        let distinct_regions: HashSet<&Value> = rows.iter().map(|r| r.get(0)).collect();
        assert!(
            distinct_regions.len() <= domain.distinct + 1,
            "the number of distinct values must be bounded by the domain (+1 for NULL), got {}",
            distinct_regions.len()
        );
    }

    #[test]
    fn nullable_columns_actually_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        let nulls = rows.iter().filter(|r| r.get(0) == &Value::Null).count();
        assert!(nulls > 0, "NULL forms its own group in GROUP BY, a classic bug spot, so it must appear frequently");
    }

    #[test]
    fn non_nullable_columns_never_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        assert!(rows.iter().all(|r| r.get(1) != &Value::Null));
    }

    #[test]
    fn same_seed_yields_same_rows() {
        let schema = orders();
        let a = gen_rows(&mut rand::rngs::StdRng::seed_from_u64(7), &schema, &Domain::default(), 20);
        let b = gen_rows(&mut rand::rngs::StdRng::seed_from_u64(7), &schema, &Domain::default(), 20);
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test data`
Expected: a compile failure, `cannot find type Domain`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/data.rs`:

```rust
use ivmlite_core::{Row, Value};
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{ColumnType, Schema};

/// The generator's value-domain configuration.
///
/// `distinct` is deliberately small: if a column had a million distinct values, every group would have one row,
/// and "the same group being inserted into and deleted from repeatedly" would never be tested — which is exactly where retraction and zombie-row
/// bugs come from (spec §9.2).
#[derive(Debug, Clone)]
pub struct Domain {
    pub distinct: usize,
    pub null_rate: f64,
}

impl Default for Domain {
    fn default() -> Self {
        Domain { distinct: 8, null_rate: 0.2 }
    }
}

pub fn gen_row(rng: &mut StdRng, schema: &Schema, domain: &Domain) -> Row {
    let values = schema
        .columns
        .iter()
        .map(|col| {
            if col.nullable && rng.random_bool(domain.null_rate) {
                return Value::Null;
            }
            let n = rng.random_range(0..domain.distinct) as i64;
            match col.ty {
                ColumnType::Integer => Value::Int(n),
                ColumnType::Text => Value::Text(format!("v{n}")),
            }
        })
        .collect();
    Row::new(values)
}

pub fn gen_rows(rng: &mut StdRng, schema: &Schema, domain: &Domain, count: usize) -> Vec<Row> {
    (0..count).map(|_| gen_row(rng, schema, domain)).collect()
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod data;
pub use data::{gen_row, gen_rows, Domain};
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (5 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/data.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): a data generator with a narrow value domain and a high NULL rate"
```

---

## Task 6: Biased update-sequence generator

**Files:**
- Create: `crates/ivmlite-test/src/ops.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `Domain`, `gen_row` (Tasks 3, 5), `ivmlite_core::Row`
- Produces: `Op` (`Insert(Row)` / `Delete(Row)` / `Update { old: Row, new: Row }`), `Op::to_delta(&self) -> Vec<(Row, i64)>`, `gen_ops(&mut StdRng, &Schema, &Domain, &[Row], usize) -> Vec<Op>`.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Domain, Schema};
    use ivmlite_core::Value;
    use rand::SeedableRng;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Text(format!("v{n}")), Value::Int(n)])
    }

    #[test]
    fn update_becomes_retract_plus_insert() {
        let op = Op::Update { old: row(1), new: row(2) };
        assert_eq!(op.to_delta(), vec![(row(1), -1), (row(2), 1)]);
    }

    #[test]
    fn insert_and_delete_map_to_plus_and_minus_one() {
        assert_eq!(Op::Insert(row(1)).to_delta(), vec![(row(1), 1)]);
        assert_eq!(Op::Delete(row(1)).to_delta(), vec![(row(1), -1)]);
    }

    #[test]
    fn deletes_target_rows_that_actually_exist() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let schema = orders();
        let domain = Domain::default();
        let initial = crate::gen_rows(&mut rng, &schema, &domain, 40);
        let ops = gen_ops(&mut rng, &schema, &domain, &initial, 300);

        // Replay the sequence, checking that the row every DELETE / UPDATE hits really exists at that moment.
        let mut live: Vec<Row> = initial.clone();
        let mut hits = 0usize;
        for op in &ops {
            match op {
                Op::Insert(r) => live.push(r.clone()),
                Op::Delete(r) => {
                    let pos = live.iter().position(|x| x == r);
                    assert!(pos.is_some(), "a DELETE must hit an existing row");
                    live.remove(pos.unwrap());
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = live.iter().position(|x| x == old);
                    assert!(pos.is_some(), "an UPDATE must hit an existing row");
                    live.remove(pos.unwrap());
                    live.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "biased sampling must produce enough deletes/updates, or retraction goes untested; got {hits}/{}",
            ops.len()
        );
    }

    #[test]
    fn sequence_is_reproducible_from_seed() {
        let schema = orders();
        let domain = Domain::default();
        let make = || {
            let mut rng = rand::rngs::StdRng::seed_from_u64(99);
            let initial = crate::gen_rows(&mut rng, &schema, &domain, 10);
            gen_ops(&mut rng, &schema, &domain, &initial, 50)
        };
        assert_eq!(make(), make());
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test ops`
Expected: a compile failure, `cannot find type Op`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/ops.rs`:

```rust
use ivmlite_core::Row;
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{gen_row, Domain, Schema};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Insert(Row),
    Delete(Row),
    Update { old: Row, new: Row },
}

impl Op {
    /// UPDATE splits into retract + insert — what is written into the delta is itself already a Z-set (spec §8.1).
    pub fn to_delta(&self) -> Vec<(Row, i64)> {
        match self {
            Op::Insert(r) => vec![(r.clone(), 1)],
            Op::Delete(r) => vec![(r.clone(), -1)],
            Op::Update { old, new } => vec![(old.clone(), -1), (new.clone(), 1)],
        }
    }
}

/// Generate a biased update sequence.
///
/// spec §9.2: a purely random generator catches almost no bugs in IVM testing — a random DELETE rarely
/// hits a row that really exists. This keeps a set of live rows and samples every DELETE / UPDATE from it,
/// so "deleting a row that was just inserted" and "emptying a group and filling it back" happen naturally and often.
pub fn gen_ops(
    rng: &mut StdRng,
    schema: &Schema,
    domain: &Domain,
    initial: &[Row],
    count: usize,
) -> Vec<Op> {
    let mut live: Vec<Row> = initial.to_vec();
    let mut ops = Vec::with_capacity(count);

    for _ in 0..count {
        // With live empty, only an insert is possible.
        let choice = if live.is_empty() { 0 } else { rng.random_range(0..10) };
        match choice {
            0..=3 => {
                let r = gen_row(rng, schema, domain);
                live.push(r.clone());
                ops.push(Op::Insert(r));
            }
            4..=6 => {
                let idx = rng.random_range(0..live.len());
                let r = live.swap_remove(idx);
                ops.push(Op::Delete(r));
            }
            _ => {
                let idx = rng.random_range(0..live.len());
                let old = live.swap_remove(idx);
                let new = gen_row(rng, schema, domain);
                live.push(new.clone());
                ops.push(Op::Update { old, new });
            }
        }
    }
    ops
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod ops;
pub use ops::{gen_ops, Op};
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (4 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/ops.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): an update-sequence generator with biased sampling"
```

---

## Task 7: The `Engine` trait and `NaiveRecompute`

**Files:**
- Create: `crates/ivmlite-test/src/engine.rs`, `crates/ivmlite-test/src/naive.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ViewQuery`, `AggFn`, `Predicate` (Tasks 3, 4), `ivmlite_core::{Row, Value, ZSet}`
- Produces: `EngineError` (`struct EngineError(pub String)`), `trait Engine { fn create_view(&mut self, &Schema, &ViewQuery, &ZSet) -> Result<(), EngineError>; fn apply(&mut self, &ZSet) -> Result<(), EngineError>; fn materialize(&mut self) -> Result<ZSet, EngineError>; }`, `NaiveRecompute::new() -> NaiveRecompute`.

> `materialize` takes `&mut self`: a real engine may need to drain pending deltas before reading (spec §8.2). This signature has to be right from M0, or plugging in M1 would mean changing every call site.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/naive.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
    use ivmlite_core::{Row, Value};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn sum_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        }
    }

    fn row(region: &str, amount: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(amount)])
    }

    fn out(region: Value, sum: i64, count: i64) -> Row {
        Row::new(vec![region, Value::Int(sum), Value::Int(count)])
    }

    #[test]
    fn aggregates_initial_state() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1), (row("b", 3), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 15, 2)), 1);
        assert_eq!(got.weight_of(&out(Value::Text("b".into()), 3, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn applying_a_delete_updates_the_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply(&ZSet::from_rows([(row("a", 5), -1)])).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 10, 1)), 1);
        assert_eq!(got.len(), 1, "the old (a,15,2) must disappear");
    }

    #[test]
    fn emptying_a_group_removes_it_entirely() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply(&ZSet::from_rows([(row("a", 10), -1)])).unwrap();

        assert!(e.materialize().unwrap().is_empty(), "an empty group must not leave a zombie row");
    }

    #[test]
    fn null_forms_its_own_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Null, Value::Int(4)]), 1),
            (row("a", 1), 1),
        ]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Null, 4, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn predicate_filters_before_aggregating() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::IntGt { column: 1, value: 4 },
        };
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 1), 1)]);
        e.create_view(&schema(), &q, &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
    }

    /// spec §6.1's NULL-semantics contract. Note this differs from "the group is empty":
    /// the group is non-empty (COUNT(*) is positive), but the summed column is all NULL, so SUM is NULL.
    #[test]
    fn sum_over_all_null_column_is_null_not_zero() {
        let nullable_amount = Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: false },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: true },
            ],
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        // Two identical rows are merged by the ZSet into weight 2
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
        ]);

        let mut e = NaiveRecompute::new();
        e.create_view(&nullable_amount, &q, &base).unwrap();
        let got = e.materialize().unwrap();

        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "with no non-NULL input SUM should be NULL, while COUNT(*) is still 2"
        );
    }
}
```

> This semantic contract also needs SQLite's own confirmation, but `recompute_via_sqlite` does not exist until Task 8.
> The cross-check against the oracle goes in Task 8 Step 1; **do not forward-reference it in this task**.

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test naive`
Expected: a compile failure, `cannot find type NaiveRecompute`

- [ ] **Step 3: Write the `Engine` trait**

`crates/ivmlite-test/src/engine.rs`:

```rust
use ivmlite_core::ZSet;

use crate::{Schema, ViewQuery};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// The only seam between the implementation under test and the testing harness.
///
/// M0 provides NaiveRecompute (trivially correct) and NoRetractionEngine (deliberately buggy);
/// once M1's real engine implements the same trait it plugs straight into every test and benchmark.
pub trait Engine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError>;

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError>;

    /// Takes &mut self: a real engine may need to drain pending deltas before reading (spec §8.2).
    fn materialize(&mut self) -> Result<ZSet, EngineError>;
}
```

- [ ] **Step 4: Write `NaiveRecompute`**

At the top of `crates/ivmlite-test/src/naive.rs`:

```rust
use std::collections::BTreeMap;

use ivmlite_core::{Row, Value, ZSet};

use crate::{AggFn, Engine, EngineError, Predicate, Schema, ViewQuery};

/// A trivially correct reference implementation: keeps the full base table and recomputes on every materialize.
///
/// Two uses: verifying the testing harness does not report false positives, and serving as the benchmark's "naive re-run" baseline (spec §10.2).
#[derive(Debug, Default)]
pub struct NaiveRecompute {
    query: Option<ViewQuery>,
    base: ZSet,
}

impl NaiveRecompute {
    pub fn new() -> Self {
        Self::default()
    }
}

fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(n) => n > value,
            _ => false, // NULL and non-integers never pass, consistent with SQL's three-valued logic
        },
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}

impl Engine for NaiveRecompute {
    fn create_view(
        &mut self,
        _schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.query = Some(query.clone());
        self.base = initial.clone();
        Ok(())
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.base.merge(delta);
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let query = self
            .query
            .as_ref()
            .ok_or_else(|| EngineError("materialize called before create_view".into()))?;

        // Each aggregate slot is (running sum, count of non-NULL inputs).
        // The second item is required: SUM returns NULL rather than 0 when there are zero non-NULL input rows
        // (spec §6.1, "the NULL-semantics contract of aggregates"). Maintaining only the running sum would silently output 0.
        let mut groups: BTreeMap<Vec<Value>, Vec<(i64, i64)>> = BTreeMap::new();

        for (row, weight) in self.base.iter() {
            if *weight <= 0 {
                continue;
            }
            if !passes(&query.predicate, row) {
                continue;
            }
            let key: Vec<Value> = query.group_by.iter().map(|i| row.get(*i).clone()).collect();
            let acc = groups
                .entry(key)
                .or_insert_with(|| vec![(0, 0); query.aggs.len()]);
            for (slot, agg) in acc.iter_mut().zip(&query.aggs) {
                match (agg.func, agg.column) {
                    (AggFn::Count, _) => slot.0 += weight,
                    (AggFn::Sum, Some(col)) => {
                        if let Value::Int(n) = row.get(col) {
                            slot.0 += n * weight;
                            slot.1 += weight;
                        }
                    }
                    (AggFn::Sum, None) => {
                        return Err(EngineError("SUM is missing its column".into()));
                    }
                }
            }
        }

        let mut out = ZSet::new();
        for (key, acc) in groups {
            let mut values = key;
            for (agg, (total, non_null)) in query.aggs.iter().zip(acc) {
                values.push(match agg.func {
                    // COUNT(*) counts rows, regardless of whether a column value is NULL
                    AggFn::Count => Value::Int(total),
                    AggFn::Sum if non_null == 0 => Value::Null,
                    AggFn::Sum => Value::Int(total),
                });
            }
            out.update(Row::new(values), 1);
        }
        Ok(out)
    }
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod engine;
mod naive;
pub use engine::{Engine, EngineError};
pub use naive::NaiveRecompute;
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (6 new in this task)

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-test/src/engine.rs crates/ivmlite-test/src/naive.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): the Engine trait and the NaiveRecompute reference implementation"
```

---

## Task 8: SQLite oracle

**Files:**
- Create: `crates/ivmlite-test/src/oracle.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ViewQuery` (Tasks 3, 4), `ivmlite_core::{Row, Value, ZSet}`
- Produces: `recompute_via_sqlite(&Schema, &ViewQuery, &ZSet) -> Result<ZSet, EngineError>`

> **Why the oracle must be independent of `NaiveRecompute`**: using `NaiveRecompute` as the oracle to test `NaiveRecompute` is circular. The real judge comes from SQLite executing the original SQL itself — a completely independent implementation.

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/oracle.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn row(region: Value, amount: i64) -> Row {
        Row::new(vec![region, Value::Int(amount)])
    }

    #[test]
    fn matches_hand_computed_aggregate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([
            (row(Value::Text("a".into()), 10), 1),
            (row(Value::Text("a".into()), 5), 1),
            (row(Value::Text("b".into()), 3), 1),
        ]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Int(15),
                Value::Int(2)
            ])),
            1
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn expands_rows_by_weight() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), 3)]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(3)])),
            1,
            "a weight of 3 should expand to 3 rows, giving COUNT(*) = 3"
        );
    }

    #[test]
    fn rejects_negative_weights_in_base_state() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), -1)]);
        assert!(
            recompute_via_sqlite(&schema(), &q, &base).is_err(),
            "a negative weight in the base state means something upstream is already wrong; the oracle must reject it, not stay silent"
        );
    }

    /// Pins spec §6.1's NULL-semantics contract and lines NaiveRecompute up with SQLite.
    /// Task 7 already asserted NaiveRecompute's behaviour on its own; this adds SQLite's confirmation.
    #[test]
    fn sum_over_all_null_matches_naive_recompute() {
        use crate::{Engine, NaiveRecompute};

        let nullable_amount = Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: false },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: true },
            ],
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([(
            Row::new(vec![Value::Text("a".into()), Value::Null]),
            2,
        )]);

        let want = recompute_via_sqlite(&nullable_amount, &q, &base).unwrap();
        assert_eq!(
            want.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SQLite's SUM returns NULL when there is no non-NULL input"
        );

        let mut e = NaiveRecompute::new();
        e.create_view(&nullable_amount, &q, &base).unwrap();
        assert_eq!(e.materialize().unwrap(), want);
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test oracle`
Expected: a compile failure, `cannot find function recompute_via_sqlite`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/oracle.rs`:

```rust
use ivmlite_core::{Row, Value, ZSet};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};

use crate::{EngineError, Schema, ViewQuery};

struct Bound<'a>(&'a Value);

impl ToSql for Bound<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self.0 {
            Value::Null => ToSqlOutput::Owned(rusqlite::types::Value::Null),
            Value::Int(n) => ToSqlOutput::Owned(rusqlite::types::Value::Integer(*n)),
            Value::Text(s) => ToSqlOutput::Owned(rusqlite::types::Value::Text(s.clone())),
        })
    }
}

fn from_sqlite(v: ValueRef<'_>) -> Result<Value, EngineError> {
    match v {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(Value::Int(n)),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(|s| Value::Text(s.to_string()))
            .map_err(|e| EngineError(format!("text is not valid UTF-8: {e}"))),
        ValueRef::Real(_) => Err(EngineError("v0 does not support REAL".into())),
        ValueRef::Blob(_) => Err(EngineError("v0 does not support BLOB".into())),
    }
}

/// The authoritative judge: load the base-table state into an in-memory SQLite and let SQLite execute the original SQL itself.
///
/// This is an implementation independent of all of this project's code, so it can be used to judge whether
/// NaiveRecompute and the future real engine are correct.
pub fn recompute_via_sqlite(
    schema: &Schema,
    query: &ViewQuery,
    base: &ZSet,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;
    conn.execute_batch(&schema.create_table_sql())
        .map_err(|e| EngineError(e.to_string()))?;

    let placeholders = vec!["?"; schema.arity()].join(", ");
    let insert_sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({})",
        schema.table,
        schema
            .column_names()
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders
    );

    {
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "the base state has a negative weight {weight}, row {row:?}"
                )));
            }
            let bound: Vec<Bound> = row.0.iter().map(Bound).collect();
            let params: Vec<&dyn ToSql> = bound.iter().map(|b| b as &dyn ToSql).collect();
            for _ in 0..*weight {
                stmt.execute(params.as_slice())
                    .map_err(|e| EngineError(e.to_string()))?;
            }
        }
    }

    let sql = query.to_sql(schema);
    let mut stmt = conn.prepare(&sql).map_err(|e| EngineError(e.to_string()))?;
    let arity = query.output_arity();

    let mut out = ZSet::new();
    let mut rows = stmt.query([]).map_err(|e| EngineError(e.to_string()))?;
    while let Some(r) = rows.next().map_err(|e| EngineError(e.to_string()))? {
        let mut values = Vec::with_capacity(arity);
        for i in 0..arity {
            let raw = r.get_ref(i).map_err(|e| EngineError(e.to_string()))?;
            values.push(from_sqlite(raw)?);
        }
        out.update(Row::new(values), 1);
    }
    Ok(out)
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod oracle;
pub use oracle::recompute_via_sqlite;
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (4 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): an independent SQLite oracle"
```

---

## Task 9: Invariant assertions

**Files:**
- Create: `crates/ivmlite-test/src/invariants.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `ViewQuery` (Task 4), `ivmlite_core::{Row, Value, ZSet}`
- Produces: `check_invariants(&ZSet, &ViewQuery) -> Result<(), String>`

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/invariants.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        }
    }

    fn out(region: &str, count: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(count)])
    }

    #[test]
    fn accepts_a_well_formed_state() {
        let z = ZSet::from_rows([(out("a", 1), 1), (out("b", 2), 1)]);
        assert!(check_invariants(&z, &q()).is_ok());
    }

    #[test]
    fn rejects_negative_weights() {
        let z = ZSet::from_rows([(out("a", 1), -1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("negative weight"), "got: {err}");
    }

    #[test]
    fn rejects_duplicate_group_keys() {
        // The same group key "a" appears in two rows with different aggregate results
        let z = ZSet::from_rows([(out("a", 1), 1), (out("a", 2), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("group key"), "got: {err}");
    }

    #[test]
    fn rejects_weight_greater_than_one_for_aggregate_views() {
        let z = ZSet::from_rows([(out("a", 1), 2)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("weight"), "got: {err}");
    }
}
```

> `ZSet` already guarantees no `w = 0` rows remain (Task 2), so the invariant layer does not check for zombie rows again — that property is covered by `ZSet::update`'s unit tests.

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test invariants`
Expected: a compile failure, `cannot find function check_invariants`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/invariants.rs`:

```rust
use std::collections::BTreeSet;

use ivmlite_core::{Row, Value, ZSet};

use crate::ViewQuery;

/// Properties that can be checked without an oracle (the first layer of spec §9.1).
/// They run very fast, so they are checked after every batch of deltas rather than only at the end.
pub fn check_invariants(state: &ZSet, query: &ViewQuery) -> Result<(), String> {
    let key_arity = query.group_by.len();
    let mut seen: BTreeSet<Vec<Value>> = BTreeSet::new();

    for (row, weight) in state.iter() {
        if *weight < 0 {
            return Err(format!("negative weight {weight} in the final state, row {row:?}"));
        }
        if *weight != 1 {
            return Err(format!(
                "each group of an aggregate view should be exactly one row with weight 1; got weight {weight}, row {row:?}"
            ));
        }
        if row.len() != query.output_arity() {
            return Err(format!(
                "output row width {} does not match the view's {}, row {row:?}",
                row.len(),
                query.output_arity()
            ));
        }
        let key: Vec<Value> = (0..key_arity).map(|i| row.get(i).clone()).collect();
        if !seen.insert(key.clone()) {
            return Err(format!("group key {key:?} appears more than once in the output"));
        }
    }
    Ok(())
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod invariants;
pub use invariants::check_invariants;
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (4 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/invariants.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): the invariant assertion layer"
```

---

## Task 10: The differential-testing driver and batch independence

**Files:**
- Create: `crates/ivmlite-test/src/differential.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: everything so far
- Produces: `Batching` (`All` / `One` / `Chunks(usize)`), `TestCase { seed: u64, schema: Schema, query: ViewQuery, initial: Vec<Row>, ops: Vec<Op>, batching: Batching }`, `Failure { case_seed: u64, stage: String, detail: String }`, `run<E: Engine>(&mut E, &TestCase) -> Result<(), Failure>`, `gen_case(u64, &Schema, &Domain, usize, usize, Batching) -> TestCase`, `check_batch_invariance<E, F>(&TestCase, F) -> Result<(), Failure> where F: Fn() -> E`, `seed_range() -> Vec<u64>` (reads the `IVMLITE_SEED` environment variable; returns `0..50` when it is unset).

- [ ] **Step 1: Write the failing tests**

`crates/ivmlite-test/src/differential.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Domain, NaiveRecompute};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn naive_engine_passes_every_enumerated_query() {
        let schema = schema();
        let domain = Domain::default();
        for (i, query) in crate::enumerate(&schema).into_iter().enumerate() {
            let mut case = gen_case(i as u64, &schema, &domain, 30, 200, Batching::Chunks(7));
            case.query = query;
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} failed at {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn batch_invariance_holds_for_naive_engine() {
        let schema = schema();
        let domain = Domain::default();
        let case = gen_case(4242, &schema, &domain, 30, 200, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    #[test]
    fn same_seed_produces_the_same_case() {
        let schema = schema();
        let domain = Domain::default();
        let a = gen_case(5, &schema, &domain, 10, 40, Batching::One);
        let b = gen_case(5, &schema, &domain, 10, 40, Batching::One);
        assert_eq!(a.initial, b.initial);
        assert_eq!(a.ops, b.ops);
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-test differential`
Expected: a compile failure, `cannot find function gen_case`

- [ ] **Step 3: Write the implementation**

At the top of `crates/ivmlite-test/src/differential.rs`:

```rust
use ivmlite_core::{Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate, gen_ops, gen_rows, recompute_via_sqlite, Domain, Engine, Op,
    Schema, ViewQuery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batching {
    /// Apply all deltas at once
    All,
    /// Apply each delta on its own
    One,
    /// One batch per n deltas
    Chunks(usize),
}

#[derive(Debug, Clone)]
pub struct TestCase {
    pub seed: u64,
    pub schema: Schema,
    pub query: ViewQuery,
    pub initial: Vec<Row>,
    pub ops: Vec<Op>,
    pub batching: Batching,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub case_seed: u64,
    pub stage: String,
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[seed={seed}] {stage}: {detail}\n\
             replay this case: IVMLITE_SEED={seed} cargo test -p ivmlite-test --test harness_catches_bugs -- --nocapture",
            seed = self.case_seed,
            stage = self.stage,
            detail = self.detail
        )
    }
}

/// The range of seeds the integration tests walk. With `IVMLITE_SEED` set, only that one seed runs —
/// this is the mechanism that makes the replay command in a Failure work (spec §9.4).
pub fn seed_range() -> Vec<u64> {
    match std::env::var("IVMLITE_SEED") {
        Ok(s) => match s.parse::<u64>() {
            Ok(seed) => vec![seed],
            Err(_) => panic!("IVMLITE_SEED must be a u64, got {s:?}"),
        },
        Err(_) => (0..50).collect(),
    }
}

pub fn gen_case(
    seed: u64,
    schema: &Schema,
    domain: &Domain,
    initial_rows: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    let mut rng = StdRng::seed_from_u64(seed);
    let initial = gen_rows(&mut rng, schema, domain, initial_rows);
    let ops = gen_ops(&mut rng, schema, domain, &initial, op_count);
    let queries = enumerate(schema);
    let query = queries[seed as usize % queries.len()].clone();
    TestCase {
        seed,
        schema: schema.clone(),
        query,
        initial,
        ops,
        batching,
    }
}

fn batches(ops: &[Op], batching: Batching) -> Vec<ZSet> {
    let size = match batching {
        Batching::All => ops.len().max(1),
        Batching::One => 1,
        Batching::Chunks(n) => n.max(1),
    };
    ops.chunks(size)
        .map(|chunk| {
            let mut z = ZSet::new();
            for op in chunk {
                for (row, weight) in op.to_delta() {
                    z.update(row, weight);
                }
            }
            z
        })
        .collect()
}

fn initial_zset(initial: &[Row]) -> ZSet {
    ZSet::from_rows(initial.iter().cloned().map(|r| (r, 1)))
}

/// Run one case: apply deltas batch by batch, checking invariants at **every observable refresh point**
/// and comparing strictly against the oracle.
///
/// Why comparing only the final state is not enough (spec §9.1): an implementation that "goes wrong midway, stays formally legal, and later
/// heals itself" can pass an end-of-run comparison completely — and that is the typical shape of state-drift bugs.
/// The invariant layer cannot stop it, because the wrong values also satisfy "weight 1, unique group key".
///
/// The cost is that complexity goes from O(n) to O(n × base-table size), so differential cases must
/// stay small (by default 25 rows of initial data and 150 operations). Large-scale scenarios are left to the benchmark.
pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure> {
    let fail = |stage: &str, detail: String| Failure {
        case_seed: case.seed,
        stage: stage.to_string(),
        detail,
    };

    let compare = |engine: &mut E, base: &ZSet, stage: &str| -> Result<(), Failure> {
        let got = engine
            .materialize()
            .map_err(|e| fail(&format!("materialize[{stage}]"), e.to_string()))?;
        check_invariants(&got, &case.query)
            .map_err(|e| fail(&format!("invariants[{stage}]"), e))?;
        let want = recompute_via_sqlite(&case.schema, &case.query, base)
            .map_err(|e| fail(&format!("oracle[{stage}]"), e.to_string()))?;
        if got != want {
            return Err(fail(
                &format!("diff[{stage}]"),
                format!(
                    "the engine disagrees with the oracle\n  query: {}\n  engine: {:?}\n  oracle: {:?}",
                    case.query.to_sql(&case.schema),
                    got,
                    want
                ),
            ));
        }
        Ok(())
    };

    let mut base = initial_zset(&case.initial);
    engine
        .create_view(&case.schema, &case.query, &base)
        .map_err(|e| fail("create_view", e.to_string()))?;

    // Compare once right after bootstrap — so a case with no ops is genuinely checked too.
    compare(engine, &base, "bootstrap")?;

    for (i, delta) in batches(&case.ops, case.batching).into_iter().enumerate() {
        engine
            .apply(&delta)
            .map_err(|e| fail(&format!("apply[{i}]"), e.to_string()))?;
        base.merge(&delta);
        compare(engine, &base, &i.to_string())?;
    }
    Ok(())
}

/// The second layer of spec §9.1: however the same sequence of deltas is batched, the final state must be the same.
/// This property cannot be tested in automatic-maintenance mode, which is one of the benefits of v0 choosing explicit refresh.
pub fn check_batch_invariance<E, F>(case: &TestCase, make: F) -> Result<(), Failure>
where
    E: Engine,
    F: Fn() -> E,
{
    let modes = [Batching::All, Batching::One, Batching::Chunks(3), Batching::Chunks(17)];
    let mut reference: Option<(Batching, ZSet)> = None;

    for mode in modes {
        let mut engine = make();
        let scoped = TestCase { batching: mode, ..case.clone() };
        run(&mut engine, &scoped)?;
        let state = engine.materialize().map_err(|e| Failure {
            case_seed: case.seed,
            stage: format!("batch_invariance[{mode:?}]"),
            detail: e.to_string(),
        })?;

        match &reference {
            None => reference = Some((mode, state)),
            Some((ref_mode, ref_state)) => {
                if *ref_state != state {
                    return Err(Failure {
                        case_seed: case.seed,
                        stage: "batch_invariance".into(),
                        detail: format!(
                            "the final states of {ref_mode:?} and {mode:?} differ\n  {ref_state:?}\n  {state:?}"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod differential;
pub use differential::{
    check_batch_invariance, gen_case, run, seed_range, Batching, Failure, TestCase,
};
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-test`
Expected: everything passes (3 new in this task)

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): the differential-testing driver and a batch-independence check"
```

---

## Task 11: The bug-planted engine + shrinker + M0 completion criteria

**Files:**
- Create: `crates/ivmlite-test/src/buggy.rs`, `crates/ivmlite-test/src/shrink.rs`, `crates/ivmlite-test/src/regression.rs`
- Create: `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`
- Modify: `crates/ivmlite-test/Cargo.toml` (add serde / serde_json, and turn on the serde feature for `ivmlite-core`)
- Modify: `crates/ivmlite-core/Cargo.toml` (add an **optional** serde feature)
- Modify: `crates/ivmlite-core/src/value.rs`, `crates/ivmlite-core/src/row.rs` (add `cfg_attr` derives)

**Interfaces:**
- Consumes: everything so far
- Produces: `NoRetractionEngine::new() -> NoRetractionEngine`, `TransientDriftEngine::new(drift_at: usize) -> TransientDriftEngine` (both implement `Engine`), `shrink<E, F>(&TestCase, F) -> TestCase where E: Engine, F: Fn() -> E`, `save_regression(&Path, &TestCase) -> std::io::Result<PathBuf>`, `load_regressions(&Path) -> std::io::Result<Vec<TestCase>>`; and gives `ivmlite-core` an optional `serde` feature

> **These are M0's completion criteria.** Without verifying that "the testing harness really goes red", every green obtained afterwards is a false green.

- [ ] **Step 1: Write the bug-planted engine**

`crates/ivmlite-test/src/buggy.rs`:

```rust
use ivmlite_core::ZSet;

use crate::{Engine, EngineError, NaiveRecompute, Schema, ViewQuery};

/// An engine with a deliberately planted bug: when an aggregate result changes it **only emits the new row and never retracts the old one**.
///
/// This is exactly what spec §6.2 calls IVM's number-one source of bugs. It exists not to be fixed,
/// but to prove the testing harness catches it.
#[derive(Debug, Default)]
pub struct NoRetractionEngine {
    inner: NaiveRecompute,
    accumulated: ZSet,
}

impl NoRetractionEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Engine for NoRetractionEngine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.inner.create_view(schema, query, initial)?;
        self.accumulated = self.inner.materialize()?;
        Ok(())
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.inner.apply(delta)?;
        // BUG (deliberate): merge in the new aggregate result, but never retract the previously emitted row.
        let fresh = self.inner.materialize()?;
        for (row, weight) in fresh.iter() {
            self.accumulated.update(row.clone(), *weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        Ok(self.accumulated.clone())
    }
}

/// On the `drift_at`-th `materialize` call, return a corrupted but **formally legal** state,
/// and be correct again afterwards.
///
/// It exists to prove one thing only: per-batch oracle comparison catches an implementation that "goes wrong midway, stays formally legal, and later heals itself",
/// while comparing only the final state does not. This is the **only evidence** for the O(n × base-table size) cost spec §9.1
/// pays for per-batch comparison — without this counterexample that cost has no justification.
///
/// The corruption deliberately keeps every invariant true: the weight is still 1, the row width is unchanged, and the group key (the prefix columns)
/// is unchanged, so `check_invariants` lets it through. Only the oracle comparison can catch it.
#[derive(Debug)]
pub struct TransientDriftEngine {
    inner: NaiveRecompute,
    calls: usize,
    drift_at: usize,
}

impl TransientDriftEngine {
    /// `drift_at` counts `materialize` calls, starting at 1.
    /// In `run`, call 1 is the bootstrap and call 2 comes after the first batch.
    pub fn new(drift_at: usize) -> Self {
        Self { inner: NaiveRecompute::new(), calls: 0, drift_at }
    }
}

impl Engine for TransientDriftEngine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.inner.create_view(schema, query, initial)
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.inner.apply(delta)
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        self.calls += 1;
        let truth = self.inner.materialize()?;
        if self.calls != self.drift_at {
            return Ok(truth);
        }
        // Add 1 to only the last aggregate column of the first row: row width, group key and weight all unchanged.
        let mut drifted = ZSet::new();
        for (i, (row, weight)) in truth.iter().enumerate() {
            let mut values = row.0.clone();
            if i == 0 {
                if let Some(Value::Int(n)) = values.last() {
                    let bumped = *n + 1;
                    *values.last_mut().expect("just checked it is non-empty") = Value::Int(bumped);
                }
            }
            drifted.update(Row::new(values), *weight);
        }
        Ok(drifted)
    }
}
```

The `use` at the top of `buggy.rs` needs to grow accordingly:

```rust
use ivmlite_core::{Row, Value, ZSet};

use crate::{Engine, EngineError, NaiveRecompute, Schema, ViewQuery};
```

- [ ] **Step 2: Write the shrinker**

`crates/ivmlite-test/src/shrink.rs`:

```rust
use ivmlite_core::Row;

use crate::{run, Engine, Op, Predicate, TestCase};

fn still_fails<E, F>(case: &TestCase, make: &F) -> bool
where
    E: Engine,
    F: Fn() -> E,
{
    let mut engine = make();
    run(&mut engine, case).is_err()
}

/// Legality of a sequence: every DELETE / UPDATE must hit a row that exists at that moment.
///
/// This is why the shrinker is home-grown rather than proptest used directly — naive shrinking deletes some
/// INSERT, leaving a later DELETE aimed at that row dangling, and produces an illegal sequence the engine was never obliged
/// to handle, so the "failure" becomes meaningless (spec §9.3).
fn is_legal(initial: &[Row], ops: &[Op]) -> bool {
    let mut live: Vec<Row> = initial.to_vec();
    for op in ops {
        match op {
            Op::Insert(r) => live.push(r.clone()),
            Op::Delete(r) => match live.iter().position(|x| x == r) {
                Some(i) => {
                    live.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match live.iter().position(|x| x == old) {
                Some(i) => {
                    live.swap_remove(i);
                    live.push(new.clone());
                }
                None => return false,
            },
        }
    }
    true
}

/// Shrink a failing case to a minimum. The order follows spec §9.3: **shrink the update sequence first, then the query,
/// and the data last**. Every step requires the shrunk case to be **still legal and still failing**.
pub fn shrink<E, F>(case: &TestCase, make: F) -> TestCase
where
    E: Engine,
    F: Fn() -> E,
{
    let mut best = case.clone();

    // Phase one: delete op ranges at decreasing delta-debugging granularity.
    let mut granularity = best.ops.len().max(1);
    while granularity >= 1 {
        let mut improved = true;
        while improved {
            improved = false;
            let chunk = (best.ops.len() / granularity).max(1);
            let mut start = 0;
            while start < best.ops.len() {
                let end = (start + chunk).min(best.ops.len());
                let mut ops = best.ops.clone();
                ops.drain(start..end);

                if is_legal(&best.initial, &ops) {
                    let candidate = TestCase { ops, ..best.clone() };
                    if still_fails(&candidate, &make) {
                        best = candidate;
                        improved = true;
                        continue; // do not advance start; keep trying at the same position
                    }
                }
                start = end;
            }
        }
        if granularity == 1 {
            break;
        }
        granularity /= 2;
    }

    // Phase two: shrink the query. Shrinking the query does not affect the sequence's legality (legality concerns rows, not the query),
    // so no is_legal gate is needed here.
    loop {
        let mut improved = false;

        // Remove one aggregate, keeping at least one
        if best.query.aggs.len() > 1 {
            for i in 0..best.query.aggs.len() {
                let mut query = best.query.clone();
                query.aggs.remove(i);
                let candidate = TestCase { query, ..best.clone() };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    improved = true;
                    break;
                }
            }
        }

        // Remove one group-by column, keeping at least one
        if !improved && best.query.group_by.len() > 1 {
            for i in 0..best.query.group_by.len() {
                let mut query = best.query.clone();
                query.group_by.remove(i);
                let candidate = TestCase { query, ..best.clone() };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    improved = true;
                    break;
                }
            }
        }

        // Degrade the predicate to None
        if !improved && best.query.predicate != Predicate::None {
            let mut query = best.query.clone();
            query.predicate = Predicate::None;
            let candidate = TestCase { query, ..best.clone() };
            if still_fails(&candidate, &make) {
                best = candidate;
                improved = true;
            }
        }

        if !improved {
            break;
        }
    }

    // Phase three: delete initial rows one by one.
    let mut i = 0;
    while i < best.initial.len() {
        let mut initial = best.initial.clone();
        initial.remove(i);
        if is_legal(&initial, &best.ops) {
            let candidate = TestCase { initial, ..best.clone() };
            if still_fails(&candidate, &make) {
                best = candidate;
                continue; // do not advance i
            }
        }
        i += 1;
    }

    best
}
```

- [ ] **Step 3: Make cases serialisable, and write the regression-freezing mechanism**

Give `ivmlite-core` an **optional** serde feature — the default build still has zero dependencies, and only
`ivmlite-test` enables it. That way neither the Global Constraint "core does not depend on rusqlite"
nor the preference "core has no dependencies by default" is broken.

`crates/ivmlite-core/Cargo.toml`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"], optional = true }

[features]
serde = ["dep:serde"]
```

Add one line before the derive line of each of `Value` (value.rs) and `Row` (row.rs):

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
```

Change the dependencies of `crates/ivmlite-test/Cargo.toml` (`ivmlite-core` is already a normal dependency,
available to the integration tests in `tests/` too, with no extra dev-dependency needed):

```toml
ivmlite-core = { workspace = true, features = ["serde"] }
rusqlite.workspace = true
rand.workspace = true
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Add `serde::Serialize, serde::Deserialize` to the derive lists of `ColumnType` / `Column` / `Schema` (schema.rs), `AggFn` / `Agg` /
`Predicate` / `ViewQuery` (query.rs), `Op` (ops.rs), and `Batching` /
`TestCase` (differential.rs).

`crates/ivmlite-test/src/regression.rs`:

```rust
use std::fs;
use std::path::{Path, PathBuf};

use crate::TestCase;

/// Write a minimal failing case into the regressions directory. The file name uses seed + op count, so repeated runs overwrite
/// the same file rather than piling up — the minimal case shrunk from the same seed should be deterministic.
pub fn save_regression(dir: &Path, case: &TestCase) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("seed{}-ops{}.json", case.seed, case.ops.len()));
    let json = serde_json::to_string_pretty(case)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, json)?;
    Ok(path)
}

/// Read every case in the regressions directory. A missing directory returns an empty vec — not having frozen
/// any case yet on a first run is a normal state, not an error.
pub fn load_regressions(dir: &Path) -> std::io::Result<Vec<TestCase>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut cases = Vec::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort(); // deterministic order, for reproducibility

    for path in paths {
        let text = fs::read_to_string(&path)?;
        let case: TestCase = serde_json::from_str(&text).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })?;
        cases.push(case);
    }
    Ok(cases)
}
```

Append to `crates/ivmlite-test/src/lib.rs`:

```rust
mod buggy;
mod regression;
mod shrink;
pub use buggy::{NoRetractionEngine, TransientDriftEngine};
pub use regression::{load_regressions, save_regression};
pub use shrink::shrink;
```

- [ ] **Step 4: Write the integration tests for M0's completion criteria**

`crates/ivmlite-test/tests/harness_catches_bugs.rs`:

```rust
use ivmlite_core::ZSet;
use ivmlite_test::{
    check_batch_invariance, gen_case, load_regressions, recompute_via_sqlite, run, save_regression,
    seed_range, shrink, Batching, Column, ColumnType, Domain, Engine, NaiveRecompute,
    NoRetractionEngine, Schema, TransientDriftEngine,
};

/// `amount` is deliberately nullable: otherwise the "SUM over zero non-NULL inputs" path is never reached by random testing,
/// and spec §6.1's NULL-semantics contract has unit-test coverage only, no differential coverage.
fn schema() -> Schema {
    Schema {
        table: "orders".into(),
        columns: vec![
            Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
            Column { name: "amount".into(), ty: ColumnType::Integer, nullable: true },
        ],
    }
}

#[test]
fn naive_engine_is_green_across_many_seeds() {
    let schema = schema();
    let domain = Domain::default();
    for seed in seed_range() {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("the reference implementation should not fail: {f}"));
    }
}

#[test]
fn naive_engine_satisfies_batch_invariance() {
    let schema = schema();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(10) {
        let case = gen_case(seed, &schema, &domain, 25, 120, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new)
            .unwrap_or_else(|f| panic!("the reference implementation should not violate batch independence: {f}"));
    }
}

/// Frozen historical failing cases must always pass. In M0 the reference implementation is trivially correct, so this test's
/// role is to build the mechanism; it really starts catching bugs once M1 plugs in the real engine.
#[test]
fn saved_regressions_still_pass() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    for case in load_regressions(&dir).expect("failed to read the regressions directory") {
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("a regression case failed: {f}"));
    }
}

/// Shows that per-batch oracle comparison has independent value: it catches an implementation that "goes wrong midway, stays formally legal, and later heals itself".
///
/// This is the only evidence for the O(n × base-table size) cost spec §9.1 pays for per-batch comparison.
/// The assertion has two halves: run must fail at **some intermediate point other than bootstrap**, while the same engine
/// replayed to the end by hand reaches a final state that **agrees** with the oracle — "correct at the end + run fails"
/// is exactly why comparing only the final state would miss it.
#[test]
fn per_batch_oracle_comparison_catches_transient_drift() {
    let schema = schema();
    let domain = Domain::default();
    let case = gen_case(3, &schema, &domain, 25, 150, Batching::Chunks(5));

    // drift_at = 2: the first materialize is the bootstrap, the second comes after the first batch
    let mut engine = TransientDriftEngine::new(2);
    let failure = run(&mut engine, &case).expect_err("per-batch comparison must catch the midway drift");
    assert!(
        failure.stage.starts_with("diff["),
        "it should fail at the oracle comparison, got stage={}",
        failure.stage
    );
    assert_ne!(
        failure.stage, "diff[bootstrap]",
        "the drift is set after the first batch and should not be reported at bootstrap"
    );

    // Replay to the end by hand, showing this engine's final state is correct
    let mut settled = TransientDriftEngine::new(2);
    let mut base = ZSet::from_rows(case.initial.iter().cloned().map(|r| (r, 1)));
    settled.create_view(&case.schema, &case.query, &base).unwrap();
    let _ = settled.materialize().unwrap(); // call 1: bootstrap

    let mut all = ZSet::new();
    for op in &case.ops {
        for (row, w) in op.to_delta() {
            all.update(row.clone(), w);
            base.update(row, w);
        }
    }
    settled.apply(&all).unwrap();
    let _ = settled.materialize().unwrap(); // call 2: the corrupted one
    let settled_state = settled.materialize().unwrap(); // call 3: recovered

    let want = recompute_via_sqlite(&case.schema, &case.query, &base).unwrap();
    assert_eq!(
        settled_state, want,
        "the final state must be correct — which is exactly why comparing only the final state misses this bug"
    );
}

/// M0's first acceptance criterion: the framework must catch the planted bug.
#[test]
fn harness_catches_the_missing_retraction_bug() {
    let schema = schema();
    let domain = Domain::default();
    let seeds = seed_range();
    let total = seeds.len();
    let mut caught = 0;
    for seed in seeds {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NoRetractionEngine::new();
        if run(&mut engine, &case).is_err() {
            caught += 1;
        }
    }
    assert!(
        caught * 10 >= total * 9,
        "only {caught} of {total} seeds caught the bug — the generator's detection rate is too low, \
         so the value domain or the biased-sampling parameters need adjusting; do not relax this assertion"
    );
}

/// M0's second acceptance criterion: a failing case must shrink to 10 steps or fewer and be frozen as a regression case.
#[test]
fn failing_case_shrinks_to_under_ten_ops() {
    let schema = schema();
    let domain = Domain::default();

    let case = seed_range()
        .into_iter()
        .map(|seed| gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5)))
        .find(|c| {
            let mut engine = NoRetractionEngine::new();
            run(&mut engine, c).is_err()
        })
        .expect("there should be at least one failing case");

    let minimal = shrink(&case, NoRetractionEngine::new);

    let mut engine = NoRetractionEngine::new();
    assert!(run(&mut engine, &minimal).is_err(), "the shrunk case must still fail");
    assert!(
        minimal.ops.len() <= 10,
        "spec §11's M0 requires shrinking to 10 steps or fewer, got {} steps",
        minimal.ops.len()
    );

    // Freeze: write into tests/regressions/, guarded from then on by saved_regressions_still_pass.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    let path = save_regression(&dir, &minimal).expect("failed to freeze the regression case");
    eprintln!("froze the minimal case: {}", path.display());
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p ivmlite-test --test harness_catches_bugs -- --nocapture`
Expected: 6 passed (this task's integration test file has 6 in total)

If `harness_catches_the_missing_retraction_bug`'s detection rate falls short, **do not relax the assertion** — tune `Domain::distinct` (smaller) or `gen_ops`'s delete/update ratio (higher). A low detection rate means the generator is not producing enough group reuse, exactly the failure mode spec §9.2 warns about.

- [ ] **Step 6: Run everything and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/ivmlite-core/Cargo.toml crates/ivmlite-core/src \
        crates/ivmlite-test/Cargo.toml crates/ivmlite-test/src \
        crates/ivmlite-test/tests
git commit -m "feat(test): the bug-planted engine, shrinker, regression freezing, and M0's completion criteria"
```

> `tests/regressions/*.json` must be committed too — they are permanent regression assets, not temporary artifacts.

---

## Task 12: A portable workload definition

**Files:**
- Create: `crates/ivmlite-workload/Cargo.toml`, `crates/ivmlite-workload/src/lib.rs`
- Create: `workloads/m0-baseline.toml`
- Modify: `Cargo.toml` (add `crates/ivmlite-workload` to members)

**Interfaces:**
- Consumes: nothing (a standalone crate that depends on no other crate in this project)
- Produces: `Workload { name: String, seed: u64, schema: WorkloadSchema, data: DataSpec, updates: UpdateSpec, views: Vec<ViewSpec> }`, `Workload::load(&Path) -> Result<Workload, WorkloadError>`, `Workload::rows(&self) -> impl Iterator<Item = (i64, String, i64)>`, `Workload::update_trace(&self) -> Vec<TraceOp>`, `Workload::export(&self, &Path) -> std::io::Result<()>`, `ViewSpec::sql(&self, table: &str) -> String`, `ViewSpec::table(&self) -> String`, `TraceOp` (`Insert { id: i64, region: String, amount: i64 }` / `Delete { id: i64 }`). `Workload` and all its field types derive `Clone`.

> **Why a separate crate**: spec §10.3 item 7 requires workloads to be portable artifacts.
> There is one runner per engine (M0 is SQLite; M2 adds Turso) and **only one workload**.
> If it were hard-coded into the bench, every new comparison system would mean redesigning the benchmark, and redesigned
> benchmarks are not comparable with each other. The crate deliberately does not depend on `ivmlite-core` — a future
> external runner should not drag in the whole engine just to read a workload.

- [ ] **Step 1: Create the crate and the workload file**

`crates/ivmlite-workload/Cargo.toml`:

```toml
[package]
name = "ivmlite-workload"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "1"
rand.workspace = true
```

Add `"crates/ivmlite-workload"` to the `members` of the root `Cargo.toml`.

`workloads/m0-baseline.toml`:

```toml
name = "m0-baseline"
seed = 47034

[schema]
table = "orders"
ddl = """
CREATE TABLE orders(
    id     INTEGER PRIMARY KEY,
    region TEXT    NOT NULL,
    amount INTEGER NOT NULL
) STRICT"""

[data]
base_rows = 100000
# group cardinality: the primary parameter deciding whether IVM wins (spec §10.1)
group_cardinality = 1000
amount_max = 200
# M0 is fixed at a uniform distribution; Zipf is scheduled for M2 (spec §10.6)
distribution = "uniform"

[updates]
batch_size = 100
delete_ratio = 0.33
# M0 is fixed at no locality; hot-spot updates are scheduled for M2 (spec §10.6)
locality = "uniform"

# The view shape is limited by the least expressive control group — hand-written triggers (spec §10.3 item 3).
# The threshold is what keeps the views distinct; the trigger side expresses the same predicate with a WHEN clause.
[[views]]
id = 0
threshold = 0

[[views]]
id = 1
threshold = 7
```

- [ ] **Step 2: Write the failing tests**

`crates/ivmlite-workload/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn spec() -> Workload {
        Workload {
            name: "t".into(),
            seed: 1,
            schema: WorkloadSchema {
                table: "orders".into(),
                ddl: "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT".into(),
            },
            data: DataSpec {
                base_rows: 500,
                group_cardinality: 7,
                amount_max: 50,
                distribution: Distribution::Uniform,
            },
            updates: UpdateSpec {
                batch_size: 30,
                delete_ratio: 0.5,
                locality: Locality::Uniform,
            },
            views: vec![
                ViewSpec { id: 0, threshold: 0 },
                ViewSpec { id: 1, threshold: 10 },
            ],
        }
    }

    #[test]
    fn rows_respect_group_cardinality() {
        let regions: BTreeSet<String> = spec().rows().map(|(_, r, _)| r).collect();
        assert_eq!(
            regions.len(),
            7,
            "the number of distinct group keys must be exactly group_cardinality — it is the benchmark's core dimension"
        );
    }

    #[test]
    fn row_ids_are_dense_and_unique() {
        let ids: Vec<i64> = spec().rows().map(|(id, _, _)| id).collect();
        assert_eq!(ids.len(), 500);
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 500);
        assert_eq!(*ids.iter().min().unwrap(), 0);
        assert_eq!(*ids.iter().max().unwrap(), 499);
    }

    #[test]
    fn trace_is_always_legal() {
        let w = spec();
        let mut live: BTreeSet<i64> = w.rows().map(|(id, _, _)| id).collect();
        for op in w.update_trace() {
            match op {
                TraceOp::Insert { id, .. } => {
                    assert!(live.insert(id), "the trace must not insert the same id twice");
                }
                TraceOp::Delete { id } => {
                    assert!(live.remove(&id), "every DELETE in the trace must hit an existing id");
                }
            }
        }
    }

    #[test]
    fn view_sql_matches_threshold() {
        let sql = spec().views[1].sql("orders");
        assert!(sql.contains("amount > 10"), "{sql}");
        assert!(sql.contains("GROUP BY region"), "{sql}");
    }

    #[test]
    fn same_seed_yields_same_trace() {
        assert_eq!(spec().update_trace(), spec().update_trace());
    }

    #[test]
    fn shipped_workload_file_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../workloads/m0-baseline.toml");
        let w = Workload::load(&path).expect("the shipped workload file must parse");
        assert_eq!(w.name, "m0-baseline");
        assert!(!w.views.is_empty());
        assert!(w.schema.ddl.contains("INTEGER PRIMARY KEY"), "spec §10.3 item 5 requires a stable primary key");
    }
}
```

- [ ] **Step 3: Run the tests to confirm they fail**

Run: `cargo test -p ivmlite-workload`
Expected: a compile failure, `cannot find type Workload`

- [ ] **Step 4: Write the implementation**

At the top of `crates/ivmlite-workload/src/lib.rs`:

```rust
use std::fs;
use std::path::Path;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

/// M0 has only Uniform. The enum exists already so that adding Zipf in M2
/// does not change the workload file format (spec §10.6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Distribution {
    Uniform,
}

/// Likewise: M2 adds Hot (updates concentrated on hot groups).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Locality {
    Uniform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSchema {
    pub table: String,
    pub ddl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSpec {
    pub base_rows: usize,
    pub group_cardinality: usize,
    pub amount_max: i64,
    pub distribution: Distribution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSpec {
    pub batch_size: usize,
    pub delete_ratio: f64,
    pub locality: Locality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    pub id: usize,
    pub threshold: i64,
}

impl ViewSpec {
    pub fn sql(&self, table: &str) -> String {
        format!(
            "SELECT region, SUM(amount), COUNT(*) FROM \"{}\" \
             WHERE amount > {} GROUP BY region",
            table, self.threshold
        )
    }

    pub fn table(&self) -> String {
        format!("mv_{}", self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workload {
    pub name: String,
    pub seed: u64,
    pub schema: WorkloadSchema,
    pub data: DataSpec,
    pub updates: UpdateSpec,
    pub views: Vec<ViewSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceOp {
    Insert { id: i64, region: String, amount: i64 },
    Delete { id: i64 },
}

#[derive(Debug)]
pub enum WorkloadError {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for WorkloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkloadError::Io(e) => write!(f, "{e}"),
            WorkloadError::Parse(e) => write!(f, "failed to parse the workload: {e}"),
        }
    }
}

impl std::error::Error for WorkloadError {}

impl Workload {
    pub fn load(path: &Path) -> Result<Workload, WorkloadError> {
        let text = fs::read_to_string(path).map_err(WorkloadError::Io)?;
        toml::from_str(&text).map_err(|e| WorkloadError::Parse(e.to_string()))
    }

    /// The base-table rows. Ids are dense and unique; the number of distinct group keys is **exactly** group_cardinality
    /// — the first card rows cover each key in turn, and the remaining rows fall into existing keys at random. Random placement alone cannot guarantee
    /// every key is covered, and the benchmark depends on this number being exact.
    pub fn rows(&self) -> impl Iterator<Item = (i64, String, i64)> + '_ {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let card = self.data.group_cardinality.max(1);
        let amount_max = self.data.amount_max.max(1);
        (0..self.data.base_rows).map(move |i| {
            let g = if i < card { i } else { rng.random_range(0..card) };
            (i as i64, format!("r{g}"), rng.random_range(0..amount_max))
        })
    }

    /// One batch of updates. Every DELETE hits an id that exists and has not been deleted yet, and every INSERT uses a fresh id,
    /// so the trace is always legal: any runner can replay it directly, without keeping its own
    /// model of "which rows are still alive".
    pub fn update_trace(&self) -> Vec<TraceOp> {
        let mut rng = StdRng::seed_from_u64(self.seed ^ 0x5EED);
        let card = self.data.group_cardinality.max(1);
        let amount_max = self.data.amount_max.max(1);
        let base = self.data.base_rows as i64;
        let mut next_id = base;
        let mut deleted: std::collections::BTreeSet<i64> = Default::default();

        (0..self.updates.batch_size)
            .map(|_| {
                let want_delete = rng.random_bool(self.updates.delete_ratio.clamp(0.0, 1.0));
                if want_delete && (deleted.len() as i64) < base {
                    let mut id = rng.random_range(0..base);
                    while deleted.contains(&id) {
                        id = rng.random_range(0..base);
                    }
                    deleted.insert(id);
                    TraceOp::Delete { id }
                } else {
                    let id = next_id;
                    next_id += 1;
                    TraceOp::Insert {
                        id,
                        region: format!("r{}", rng.random_range(0..card)),
                        amount: rng.random_range(0..amount_max),
                    }
                }
            })
            .collect()
    }

    /// Export in a form any engine can load: schema.sql / views.sql / data.csv /
    /// updates.csv. This is how the "workloads are portable" constraint is actually met (spec §10.3 item 7).
    pub fn export(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("schema.sql"), format!("{};\n", self.schema.ddl))?;

        let views: String = self
            .views
            .iter()
            .map(|v| format!("-- {}\n{};\n", v.table(), v.sql(&self.schema.table)))
            .collect();
        fs::write(dir.join("views.sql"), views)?;

        let mut data = String::from("id,region,amount\n");
        for (id, region, amount) in self.rows() {
            data.push_str(&format!("{id},{region},{amount}\n"));
        }
        fs::write(dir.join("data.csv"), data)?;

        let mut ups = String::from("op,id,region,amount\n");
        for op in self.update_trace() {
            match op {
                TraceOp::Insert { id, region, amount } => {
                    ups.push_str(&format!("insert,{id},{region},{amount}\n"))
                }
                TraceOp::Delete { id } => ups.push_str(&format!("delete,{id},,\n")),
            }
        }
        fs::write(dir.join("updates.csv"), ups)
    }
}
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p ivmlite-workload`
Expected: everything passes (6 new in this task)

- [ ] **Step 6: Confirm the export can really be consumed externally**

Run: `cargo run -q -p ivmlite-workload --example export 2>/dev/null || true`

Instead of writing an example, verify with a temporary test that `sqlite3` can take the exported artifacts:

```rust
#[test]
fn exported_artifacts_load_into_sqlite() {
    let dir = std::env::temp_dir().join("ivmlite-workload-export");
    let mut w = spec();
    w.data.base_rows = 50;
    w.updates.batch_size = 10;
    w.export(&dir).unwrap();

    for f in ["schema.sql", "views.sql", "data.csv", "updates.csv"] {
        assert!(dir.join(f).exists(), "missing export artifact {f}");
    }
    let data = std::fs::read_to_string(dir.join("data.csv")).unwrap();
    assert_eq!(data.lines().count(), 51, "header + 50 rows");
}
```

Run: `cargo test -p ivmlite-workload`
Expected: everything passes (including this new one)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/ivmlite-workload workloads
git commit -m "feat(workload): a portable workload definition, trace generation, and export"
```

---

## Task 13: Benchmark harness, three same-host baselines, and baseline curves

**Files:**
- Create: `crates/ivmlite-bench/Cargo.toml`, `crates/ivmlite-bench/src/main.rs`, `crates/ivmlite-bench/src/baseline.rs`, `crates/ivmlite-bench/src/plot.rs`
- Modify: `Cargo.toml` (add `ivmlite-bench` back to members)

**Interfaces:**
- Consumes: `ivmlite-workload`'s `Workload` / `ViewSpec` / `TraceOp` (Task 12)
- Produces: the executable `ivmlite-bench`, writing CSV to stdout: `baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms`; and writing one SVG per group cardinality to `docs/bench/`

- [ ] **Step 1: Create the crate manifest**

`crates/ivmlite-bench/Cargo.toml`:

```toml
[package]
name = "ivmlite-bench"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
ivmlite-workload = { path = "../ivmlite-workload" }
rusqlite.workspace = true
```

and add `ivmlite-bench` back to the `members` of the root `Cargo.toml`.

> **The bench does not depend on `ivmlite-test` or `ivmlite-core`.** The table structure, view definitions, data and
> update trace all come from `ivmlite-workload` (with `id INTEGER PRIMARY KEY`). The correctness
> tests' types deliberately have no primary key — they want "locate a row by value" semantics; the benchmark wants
> "locate a row by primary key" performance. Forcing the two into one would only make one side compromise. When M1 plugs in the real engine,
> the bench gains a baseline that depends on `ivmlite-sqlite`, and only then does `ivmlite-core` come in.
>
> The bench itself no longer holds `rand` either — all randomness is decided by the workload's seed, so
> **the same workload produces exactly the same data and trace on any runner**, which is what makes cross-engine comparison
> hold (spec §10.3 item 7).

- [ ] **Step 2: Write the three baselines**

`crates/ivmlite-bench/src/baseline.rs`:

```rust
use std::time::Instant;

use ivmlite_workload::{TraceOp, ViewSpec, Workload};
use rusqlite::Connection;

/// spec §10.2's three same-host control groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// The lower bound: write only the base table, maintain no view at all. Pure write cost.
    NoMaintenance,
    /// The skeptic: hand-written triggers maintain summary tables. The v0 engine must beat it, or there is no story.
    HandWrittenTrigger,
    /// The baseline: after every batch of deltas, re-run every view's SQL. The crossover is measured here.
    NaiveRecompute,
}

impl Baseline {
    pub fn label(self) -> &'static str {
        match self {
            Baseline::NoMaintenance => "no_maintenance",
            Baseline::HandWrittenTrigger => "hand_written_trigger",
            Baseline::NaiveRecompute => "naive_recompute",
        }
    }
}

/// Create the base table and load the initial data. Not timed.
///
/// The table structure comes from the workload, in which `id INTEGER PRIMARY KEY` is a hard requirement:
/// spec §10.3 item 5 — locating a row by the values of all its columns has no usable index, `EXPLAIN QUERY PLAN`
/// shows `SCAN orders`, and delete/update time grows linearly with base-table size, whereas "the incremental cost does not grow with
/// base-table size" is the only thing this benchmark sets out to prove.
pub fn seed_base(conn: &Connection, w: &Workload) -> rusqlite::Result<()> {
    conn.execute_batch(&w.schema.ddl)?;
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{}\"(id, region, amount) VALUES (?1, ?2, ?3)",
            w.schema.table
        ))?;
        for (id, region, amount) in w.rows() {
            ins.execute((id, &region, amount))?;
        }
    }
    tx.commit()
}

/// Create the summary table → **bootstrap in full first** → then create the triggers. The order cannot be reversed.
///
/// spec §10.3 item 4: creating an empty summary table after the base table already has data gives a view that is forever
/// incomplete, whose maintenance cost is not representative either. And the triggers must be created **after** the bootstrap,
/// or the bootstrap's INSERT ... SELECT would be counted again by the triggers.
///
/// Note the summary table's `k` column is declared `TEXT` rather than `ANY`: a STRICT table lets an `ANY` column
/// mix types row by row, and `1` and `'1'` would split into two groups (spec §7.1).
pub fn install_trigger_view(conn: &Connection, table: &str, v: &ViewSpec) -> rusqlite::Result<()> {
    let t = v.table();
    let k = v.threshold;
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE "{t}" (
            k TEXT    NOT NULL PRIMARY KEY,
            s INTEGER NOT NULL,
            c INTEGER NOT NULL
        ) STRICT;

        INSERT INTO "{t}"(k, s, c)
            SELECT region, SUM(amount), COUNT(*)
            FROM "{table}" WHERE amount > {k} GROUP BY region;

        CREATE TRIGGER "{t}_ins" AFTER INSERT ON "{table}"
        WHEN NEW.amount > {k} BEGIN
            INSERT INTO "{t}"(k, s, c) VALUES (NEW.region, NEW.amount, 1)
            ON CONFLICT(k) DO UPDATE SET s = s + NEW.amount, c = c + 1;
        END;

        CREATE TRIGGER "{t}_del" AFTER DELETE ON "{table}"
        WHEN OLD.amount > {k} BEGIN
            UPDATE "{t}" SET s = s - OLD.amount, c = c - 1 WHERE k = OLD.region;
            DELETE FROM "{t}" WHERE k = OLD.region AND c = 0;
        END;
        "#
    ))
}

/// Apply one batch of changes and return the milliseconds. The timing includes the commit — commit cost is real cost.
///
/// For the HandWrittenTrigger baseline the triggers' overhead is naturally counted here, so
/// `apply_ms(trigger) − apply_ms(no_maintenance)` is the write amplification spec §10.5 asks for.
pub fn apply(conn: &Connection, table: &str, ops: &[TraceOp]) -> rusqlite::Result<f64> {
    let start = Instant::now();
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{table}\"(id, region, amount) VALUES (?1, ?2, ?3)"
        ))?;
        let mut del = tx.prepare_cached(&format!("DELETE FROM \"{table}\" WHERE id = ?1"))?;
        for op in ops {
            match op {
                TraceOp::Insert { id, region, amount } => {
                    ins.execute((id, region, amount))?;
                }
                TraceOp::Delete { id } => {
                    del.execute((id,))?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// Naive re-run: run each view's SQL once and drain the result set.
///
/// The results are **not written back to a table** here, a deliberately conservative choice in naive re-run's favour — if the incremental approach
/// cannot even beat a "read-only, no write" naive re-run, the conclusion is beyond dispute.
pub fn recompute_all(conn: &Connection, w: &Workload) -> rusqlite::Result<f64> {
    let start = Instant::now();
    for v in &w.views {
        let mut stmt = conn.prepare_cached(&v.sql(&w.schema.table))?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
```

- [ ] **Step 3: Write the charts**

`crates/ivmlite-bench/src/plot.rs`:

```rust
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::Record;

const W: f64 = 720.0;
const H: f64 = 420.0;
const PAD: f64 = 64.0;
const COLORS: [&str; 3] = ["#888888", "#1f77b4", "#d62728"];

/// Draw a headline chart: fix views / batch / group cardinality, with base-table size on the x axis (log) and
/// total time on the y axis, one line per baseline. The crossover is where two lines meet (spec §10.4).
///
/// The group cardinality must be fixed and labelled on the chart — the crossover moves sharply with it, and mixing points of different cardinalities into
/// one chart would draw a meaningless line (spec §10.1).
pub fn write_svg(
    path: &Path,
    records: &[Record],
    fixed_views: usize,
    fixed_batch: usize,
    fixed_card: usize,
) -> std::io::Result<()> {
    let mut series: BTreeMap<&str, Vec<(f64, f64)>> = BTreeMap::new();
    for r in records {
        if r.views == fixed_views && r.batch == fixed_batch && r.cardinality == fixed_card {
            series
                .entry(r.baseline)
                .or_default()
                .push(((r.base_rows as f64).log10(), r.apply_ms + r.maintain_ms));
        }
    }
    for pts in series.values_mut() {
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    let xs: Vec<f64> = series.values().flatten().map(|p| p.0).collect();
    let ys: Vec<f64> = series.values().flatten().map(|p| p.1).collect();
    if xs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no data points for views={fixed_views} batch={fixed_batch} card={fixed_card}"),
        ));
    }
    let (x0, x1) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
    let y1 = ys.iter().cloned().fold(f64::MIN, f64::max).max(1e-6);

    let sx = |x: f64| PAD + (x - x0) / (x1 - x0).max(1e-9) * (W - 2.0 * PAD);
    let sy = |y: f64| H - PAD - (y / y1) * (H - 2.0 * PAD);

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="sans-serif" font-size="12">
<rect width="{W}" height="{H}" fill="white"/>
<text x="{tx}" y="24" text-anchor="middle" font-size="15">apply + maintain &#183; views={fixed_views} &#183; batch={fixed_batch} &#183; groups={fixed_card}</text>
<line x1="{PAD}" y1="{by}" x2="{rx}" y2="{by}" stroke="#333"/>
<line x1="{PAD}" y1="{PAD}" x2="{PAD}" y2="{by}" stroke="#333"/>
<text x="{tx}" y="{lx}" text-anchor="middle">base_rows (log10)</text>
<text x="16" y="{PAD}" fill="#333">{y1:.1} ms</text>
"##,
        tx = W / 2.0,
        by = H - PAD,
        rx = W - PAD,
        lx = H - 20.0,
    );

    for (i, (name, pts)) in series.iter().enumerate() {
        let color = COLORS[i % COLORS.len()];
        let d: Vec<String> = pts
            .iter()
            .map(|(x, y)| format!("{:.1},{:.1}", sx(*x), sy(*y)))
            .collect();
        svg.push_str(&format!(
            "<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" points=\"{}\"/>\n",
            d.join(" ")
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{color}\">{name}</text>\n",
            W - PAD - 150.0,
            PAD + 18.0 * (i as f64 + 1.0)
        ));
    }
    svg.push_str("</svg>\n");

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, svg)
}
```

- [ ] **Step 4: Write the matrix driver**

`crates/ivmlite-bench/src/main.rs`:

```rust
mod baseline;
mod plot;

use std::path::Path;

use baseline::{apply, install_trigger_view, recompute_all, seed_base, Baseline};
use ivmlite_workload::{ViewSpec, Workload};
use rusqlite::Connection;

const BASE_ROWS: [usize; 3] = [10_000, 100_000, 1_000_000];
const BATCH_SIZES: [usize; 4] = [1, 10, 100, 1000];
const VIEW_COUNTS: [usize; 4] = [1, 10, 50, 200];
const GROUP_CARDINALITIES: [usize; 3] = [10, 1_000, 100_000];

/// The view count fixed while sweeping group cardinality, and the group cardinality fixed while sweeping view count.
///
/// A full four-way cross is 144 configurations, too many. Spec §10.1 settles these two fixed values, so the two sweeps
/// have 36 configurations each, and both pass through the same shared point (views=10, cardinality=1k), so the two sets of charts can
/// be read side by side.
const FIXED_VIEWS: usize = 10;
const FIXED_CARDINALITY: usize = 1_000;

#[derive(Debug, Clone)]
pub struct Record {
    pub baseline: &'static str,
    pub views: usize,
    pub base_rows: usize,
    pub batch: usize,
    pub cardinality: usize,
    pub apply_ms: f64,
    pub maintain_ms: f64,
}

/// Derive one concrete configuration from the benchmark workload.
///
/// The view shape is limited by the least expressive control group — hand-written triggers (spec §10.3 item 3),
/// so only the view **count** and thresholds change here, not the shape; all three baselines get the same set of views.
fn variant(base: &Workload, base_rows: usize, cardinality: usize, views: usize) -> Workload {
    let mut w = base.clone();
    w.data.base_rows = base_rows;
    w.data.group_cardinality = cardinality;
    w.views = (0..views)
        .map(|i| ViewSpec { id: i, threshold: (i as i64 * 7) % 150 })
        .collect();
    w
}

fn run_one(
    base: &Workload,
    b: Baseline,
    rows: usize,
    card: usize,
    views: usize,
    batch: usize,
) -> rusqlite::Result<Record> {
    let mut w = variant(base, rows, card, views);
    w.updates.batch_size = batch;

    let conn = Connection::open_in_memory()?;

    // ---- None of the following is timed: building the initial state ----
    seed_base(&conn, &w)?;
    if b == Baseline::HandWrittenTrigger {
        for v in &w.views {
            install_trigger_view(&conn, &w.schema.table, v)?;
        }
    }
    let ops = w.update_trace();

    // ---- The timed section ----
    let apply_ms = apply(&conn, &w.schema.table, &ops)?;
    let maintain_ms = match b {
        // The triggers' cost is already counted in apply_ms — that is exactly the write amplification
        Baseline::NoMaintenance | Baseline::HandWrittenTrigger => 0.0,
        Baseline::NaiveRecompute => recompute_all(&conn, &w)?,
    };

    Ok(Record {
        baseline: b.label(),
        views,
        base_rows: rows,
        batch,
        cardinality: card,
        apply_ms,
        maintain_ms,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Workload::load(Path::new("workloads/m0-baseline.toml"))?;
    let mut records: Vec<Record> = Vec::new();

    let baselines = [
        Baseline::NoMaintenance,
        Baseline::HandWrittenTrigger,
        Baseline::NaiveRecompute,
    ];

    // Sweep one: group cardinality × base-table size × batch size, fixed view count
    for b in baselines {
        for card in GROUP_CARDINALITIES {
            for rows in BASE_ROWS {
                // A group cardinality larger than the row count is semantically meaningless — a table of N rows cannot have more than N
                // distinct group keys. ivmlite-workload rejects such a configuration at load time,
                // so skip it here rather than let it error. Each skipped cell gets a line on stderr,
                // so a reader of the CSV does not think it was missed.
                if card > rows {
                    eprintln!("skipping a meaningless cell: card={card} > base_rows={rows}");
                    continue;
                }
                for batch in BATCH_SIZES {
                    records.push(run_one(&base, b, rows, card, FIXED_VIEWS, batch)?);
                }
            }
        }
    }

    // Sweep two: view count × base-table size × batch size, fixed group cardinality
    for b in baselines {
        for views in VIEW_COUNTS {
            if views == FIXED_VIEWS {
                continue; // duplicates sweep one's shared point
            }
            for rows in BASE_ROWS {
                for batch in BATCH_SIZES {
                    records.push(run_one(&base, b, rows, FIXED_CARDINALITY, views, batch)?);
                }
            }
        }
    }

    println!("baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms");
    for r in &records {
        println!(
            "{},{},{},{},{},{:.3},{:.3}",
            r.baseline, r.views, r.base_rows, r.batch, r.cardinality, r.apply_ms, r.maintain_ms
        );
    }

    // One chart per group cardinality — the crossover moves sharply with this parameter, and producing only one
    // would amount to picking a flattering point yourself (spec §10.1).
    for card in GROUP_CARDINALITIES {
        let path = format!("docs/bench/m0-baseline-card{card}.svg");
        match plot::write_svg(Path::new(&path), &records, FIXED_VIEWS, 100, card) {
            Ok(()) => eprintln!("chart written to {path}"),
            // A group cardinality skipped at every base-table size has no data points;
            // that is not an error — say so and carry on.
            Err(e) => eprintln!("skipping the chart for card={card}: {e}"),
        }
    }

    Ok(())
}
```

- [ ] **Step 5: First do a reduced-scale smoke run**

Temporarily change `BASE_ROWS` to `[1_000, 10_000]`, `VIEW_COUNTS` to `[1, 10]`, and
`GROUP_CARDINALITIES` to `[10, 1_000]`, and run it through:

Run: `cargo run -p ivmlite-bench --release 2>/dev/null | head -20`

Expected: the CSV header contains a `group_cardinality` column; `no_maintenance`'s `maintain_ms`
is always 0; `naive_recompute`'s `maintain_ms` grows clearly with `base_rows`;
`hand_written_trigger`'s `apply_ms` is clearly higher than `no_maintenance`'s (the difference is the write amplification).

**The most important sanity check**: at the same `base_rows`, `naive_recompute`'s
`maintain_ms` should **barely change with `group_cardinality`** (it always scans the whole table), while
`hand_written_trigger`'s `apply_ms` should **rise with `group_cardinality`**
(the more spread out the group keys, the larger the summary table and the deeper the B-tree `ON CONFLICT` walks). If this
difference is not observed, the cardinality dimension is not really in effect — check `Workload::rows` first; do not keep running.

Also verify the primary key really takes effect; no `SCAN` should appear:

Run: `sqlite3 :memory: "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT; EXPLAIN QUERY PLAN DELETE FROM orders WHERE id = 1;"`
Expected: the output contains `SEARCH orders USING INTEGER PRIMARY KEY` and no `SCAN`.

- [ ] **Step 6: Run the full matrix and archive it**

After changing back to the full constants:

Run: `cargo run -p ivmlite-bench --release > docs/bench/m0-baseline.csv`
Expected: the CSV is written; the three charts `docs/bench/m0-baseline-card10.svg`, `-card1000.svg`,
and `-card100000.svg` are generated.

Write the conclusions in `docs/bench/README.md`, **one crossover row per group cardinality**:

```markdown
| group cardinality | Δ size | full recompute / hand-written trigger ratio | hand-written trigger's write amplification |
|---|---|---|---|
| 10     | 1 / 1000 | ... | ... |
| 1k     | 1 / 1000 | ... | ... |
| 100k   | 1 / 1000 | ... | ... |
```

The cells with `card > base_rows` in the table are empty — not missed runs, but configurations that semantically do not exist
(a table of N rows cannot have more than N group keys), which `ivmlite-workload` rejects at load time.
Say so in the README, so readers do not think data is missing.

**Do not report "one crossover".** Spec §10.4 has deleted the off-the-cuff threshold "a crossover above 1 million rows is meaningless"
— the same implementation under "10 groups + large Δ batches" and "100,000 groups +
single-row Δ" gives two completely different conclusions; there is no single crossover. What should be reported is a surface.

The falsification criterion becomes a shape: **if no region of this surface gives incremental maintenance a substantial advantage over full recomputation
(ratio > 2), the project's premise does not hold.** If an advantage region exists, write down honestly where it falls, including
"it holds only in a very narrow corner".

**What M0 fills in is the relationship between the three baselines** (the two incremental columns wait until M1 for numbers), but the table structure
is settled now, and M1 fills it in directly. Note the cell most worth watching is **large Δ batches + low group cardinality**
— the only place the "extra surprise" of spec §10.2's three-tier criterion could possibly appear.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/ivmlite-bench docs/bench
git commit -m "feat(bench): the benchmark matrix, three same-host baselines, and baseline curves"
```

---

## Self-Review

**1. Spec coverage**

| Spec requirement | Where it lands |
|---|---|
| §4.2 core does not depend on rusqlite | Task 1 Step 1 (empty dependency table) + Global Constraints |
| §5.1 the weight invariant | Task 2 (`ZSet` deletes on reaching zero), Task 9 (negative weights, duplicate group keys) |
| §5.1 Value has no floating point | Task 1 (the enum has only three variants), Task 4 (SUM applies only to INTEGER) |
| §6.2 retraction semantics | Task 11 (`NoRetractionEngine` is its cautionary counterexample) |
| §7.1 STRICT table | Task 3 (`create_table_sql` asserts STRICT) |
| §7.1 bare-column group-by keys | Task 4 (`group_by: Vec<usize>` can only be column indices) |
| §8.1 UPDATE = retract + insert | Task 6 (`Op::to_delta`) |
| §6.1 the NULL-semantics contract of aggregates | Task 7 (the `(running sum, non-NULL count)` slot + unit tests), Task 8 (cross-checked with SQLite) |
| §9.1 four layers of verification | invariants → Task 9; batch independence → Task 10; **per-batch** oracle comparison → Task 10's `compare` closure; cross-validation → M2 (the spec already rules it out of M0) |
| §9.2 narrow value domain / high NULL rate / biased sampling / exhaustive queries | Task 5, Task 6, Task 4 |
| §9.3 legality-preserving shrinking, in the order ops → query → data | Task 11 (the `is_legal` gate + three phases) |
| §9.4 seed reproducibility | Tasks 5, 6 and 10 each have a reproducibility test; `IVMLITE_SEED` + `seed_range()` give a one-command replay |
| §9.4 freezing failing cases | Task 11 Step 3 (`save_regression` / `load_regressions`) + Step 4's `saved_regressions_still_pass` |
| §10.2 three same-host control groups | Task 13 |
| §10.3 item 3, the same set of views | Task 13 (`views(n)` is the same for all three baselines; the shape is limited by what triggers can express) |
| §10.3 item 4, bootstrap finished before timing | Task 13 (`install_trigger_view`: create table → `INSERT ... SELECT` → create triggers) |
| §10.3 item 5, a stable primary key | Task 13 (`id INTEGER PRIMARY KEY`; Step 5 verifies with `EXPLAIN QUERY PLAN` that there is no `SCAN`) |
| §10.1 group cardinality as an explicit dimension | Task 13 (`GROUP_CARDINALITIES`; the view count fixed at 10 while sweeping; one chart per cardinality) |
| §10.3 item 6, no citing of others' published numbers | Global Constraints; M0 has no cross-system comparison |
| §10.3 item 7, portable workloads | Task 12 (`ivmlite-workload` + `workloads/m0-baseline.toml` + `Workload::export`) |
| §10.5 write amplification | Task 13 (trigger cost counted in `apply_ms`; subtract `no_maintenance` to get it) |
| §11 M0 completion criteria (catch the planted bug + shrink to 10 steps) | the two tests of Task 11 Step 4 |
| §11 M0's three baselines "charted" | Task 13 Step 3 (`plot::write_svg`) + Step 6 |

**Known gaps (intentional, not omissions):**
- **§7.1 rejecting `ANY` columns and checking collations** is the job of `ivm_create_view`, which does not exist until M1. M0's `ColumnType` has only `Integer` / `Text`, and the generator never produces `ANY`, so M0 cannot trigger the problem. **M1 must implement both rejections**.
- **§10.6's three known simplifications** (uniform distribution, no update locality, no standard benchmark queries) all remain in M0. The `Distribution` and `Locality` enums each have only a `Uniform` variant, as deliberate placeholders — adding `Zipf` / `Hot` in M2 does not require changing the workload file format.
- **§10.7 Nexmark** belongs to M2: v0 has no join and cannot run any Nexmark query.
- **Space amplification and bootstrap time** (spec §10.5) are not measured in M0 — both are only meaningful with a real engine; M1 adds them.
- **`criterion` micro-benchmarks** (spec §10) are not brought in — M0's main benchmark is the end-to-end matrix, and micro-benchmarks wait until core has operators to measure.
- **The `ivmlite-sql` / `ivmlite-sqlite` crates** are not created; see the last Global Constraint.

**Known costs (accepted knowingly):**
- Per-batch oracle comparison makes differential testing's complexity O(batch count × base-table size). At the default case size (25 rows / 150 steps / 5 per batch ≈ 30 batches) each case rebuilds a small table 30 times, which is acceptable. **If the case size is ever enlarged, first make this configurable, rather than directly lowering the comparison frequency.**

**2. Placeholder scan**: no TBD / TODO; every code step gives a complete implementation ready to paste; no undefined type or function appears between tasks.

**3. Type consistency check**: `Engine::materialize` is `&mut self` throughout (consistent across engine.rs / naive.rs / buggy.rs / differential.rs); the `Domain` fields `distinct` / `null_rate` are spelled consistently in data.rs, differential.rs and main.rs; `Op::to_delta` returns `Vec<(Row, i64)>`, consumed that way in differential.rs and baseline.rs; `Failure`'s three fields `case_seed` / `stage` / `detail` are consistent in construction and in `Display`.

---

## Notes on execution order

Tasks 1 → 13 depend strictly on each other and cannot be run in parallel or out of order. Task 11 is M0's acceptance gate — while it is red, M0 is not complete, and **relaxing that test's assertions to make it green is not allowed** (spec §11 states the reasons).
