# M1a Phase 1: Multi-Table Harness Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn M0's differential testing harness from "one table" into "many tables", and move the plan IR type family into `ivmlite-core`, so that M1a's engine has somewhere to be written.

**Architecture:** Two things. First, the `Schema` / `ViewQuery` family currently lives in `ivmlite-test`, but the engine must live in `ivmlite-core`, and core must not depend on test — these types are in essence v0's plan IR and have to move down; SQL rendering (`create_table_sql` / `to_sql`) stays in test, because it exists only to drive the oracle. Second, `TestCase` / the oracle / the generators / the driver are all indexed by table, and a single-table case becomes "a multi-table case with only one table".

**Tech Stack:** Rust 1.95, `rusqlite` (bundled), `rand` 0.10 (`RngExt`), `serde` (an optional feature of core).

**Spec:** [`docs/superpowers/specs/2026-09-18-ivmlite-design.md`](../specs/2026-09-18-ivmlite-design.md)

## Scope of this plan, and what is out of scope

**In scope**: the mutation-gate audit, the plan IR type migration, the `Database` type, the multi-table oracle, the multi-table generators, the multi-table driver.

**Out of scope**: any operator, `Arrangement`, consolidation, join. **The engine's plan is written separately after this plan lands** — the code of those tasks must be written against the real types this plan produces; writing it now would mean writing against guessed types, which is exactly where M0's two compile-level defects came from.

State at completion: all of M0's tests are still green under "a database with only one table", and the harness can already express multi-table cases. **The engine still does not exist**, as expected.

## Global Constraints

- **`ivmlite-core` must not depend on `rusqlite` / `libsqlite3-sys`** (spec §4.2). The types moved in must be pure data + pure methods.
- **SQL rendering does not go into core.** `create_table_sql` and `to_sql` stay in `ivmlite-test`, because they exist only to drive the oracle; core owns the IR, test owns "how to express this IR as SQL SQLite can execute".
- **Zero `unsafe`** (spec §4.2: `unsafe` is allowed only in `ivmlite-sqlite`, which does not exist yet).
- **The differential-testing schema is fixed at 2 columns per table** (spec §9.2 item 4). This is the precondition for "exhaustive beats random", not a magic number — widening to 3 columns grows the exhaustive space about 8×. Do not widen it on your own initiative "for better coverage".
- **`Value` has only `Null` / `Int` / `Text`** (spec §5.1).
- **The weight invariant** (spec §5.1): no negative weights in the final state; a row whose weight reaches zero is deleted, no zombie rows.
- **`rand` 0.10**: `random_range` / `random_bool` live on `RngExt`, not `Rng`. `use rand::RngExt;`.
- **Every new spec-mandated invariant must be registered as a row in [`docs/mutation-gates.md`](../../mutation-gates.md), and its "verified" cell must come from actually running the mutation.** The full suite takes about 1.5 seconds; do not skip it on the grounds that "running tests is expensive" — this project has already been wrong about that assumption once.
- During the migration and refactor, **M0's acceptance thresholds must not regress**: `harness_catches_the_missing_retraction_bug` must still detect ≥90%, and `failing_case_shrinks_to_under_ten_ops` must still reach ≤10 steps. A regression means a change touched something it should not have; **relaxing the assertions is not allowed**.

---

## File Structure

```
crates/ivmlite-core/src/
  lib.rs            + pub use schema::*, query::*, database::*
  schema.rs         new: ColumnType / Column / Schema (arity, column_names)
  query.rs          new: AggFn / Agg / Predicate / ViewQuery (output_arity)
  database.rs       new: Database
crates/ivmlite-test/src/
  sql.rs            new: create_table_sql / view_query_to_sql (render SQL from core types)
  schema.rs         deleted (the types moved out)
  query.rs          keeps only enumerate (generation is a testing responsibility)
  oracle.rs         changed: multi-table signature
  data.rs           changed: generate per table
  ops.rs            changed: table-tagged, live sets kept per table
  differential.rs   changed: TestCase / run / check_batch_invariance go multi-table
  engine.rs         changed: create_view takes &Database and per-table initial state
  naive.rs          changed: holds base per table
  buggy.rs          changed: follows naive
```

---

## Task 1: Mutation-gate audit

**Files:**
- Modify: `docs/mutation-gates.md`
- Modify: any test file where the audit finds a gap

**Interfaces:**
- Consumes: every existing test
- Produces: no new interface; the output is a gate table whose "verified" cells are all true, plus the gap tests that were added

> **Why this is the first task**: every later step of this plan refactors M0's code, and the safety net for a refactor is those tests. **First confirm the safety net actually catches, then start taking things apart.** 25 rows of the gate table are marked "unverified" — each has a named guarding test, but nobody has actually broken the implementation and run it. M0's final review used exactly this method, at about a second per run, to find three places where "the test exists but does not catch".

- [ ] **Step 1: Run all 25 unverified mutations, one by one**

For every row of `docs/mutation-gates.md` whose "verified" cell is "unverified":

1. Break the implementation as the "mutation" cell describes
2. **First confirm the broken code still compiles** — this step cannot be skipped. If the mutation introduces a syntax error, the `cargo test` failure is a compile failure rather than an assertion failure, `grep FAILED` cannot tell the difference, and an empty verification gets recorded as a real one. How to confirm: `cargo build --workspace --locked` succeeds.
3. Run `cargo test --workspace --locked` and record which test went red
4. Restore (`git checkout -- <file>`), and confirm `git status` is clean and the tests are all green again

- [ ] **Step 2: Fill the results back into the gate table**

The expected test went red → change the "verified" cell to `**verified**`.

**Nothing went red, or a different test went red** → this is a real gap. Do not edit the gate table to fit the status quo, and do not relax any assertion. Add a test that goes red, then mark the row verified, and list this row separately in the report.

- [ ] **Step 3: Update the counts and commit**

The counts at the end of the file **must be counted from this file by a script**, never written by hand — the previous hand-written version was wrong the first time (12/26 was really 13/25).

```bash
python3 - <<'PY'
s=open('docs/mutation-gates.md').read()
rows=[l for l in s.splitlines() if l.startswith('| §')]
print(f"{len(rows)} rows in total: {sum('**verified**' in l for l in rows)} verified, "
      f"{sum('unverified' in l for l in rows)} unverified, "
      f"{sum(l.rstrip().endswith('| — |') for l in rows)} n/a")
PY
```

```bash
git add docs/mutation-gates.md crates/
git commit -m "test: mutation-gate audit — actually run every unverified row"
```

---

## Task 2: Move the plan IR type family into `ivmlite-core`

**Files:**
- Create: `crates/ivmlite-core/src/schema.rs`, `crates/ivmlite-core/src/query.rs`
- Create: `crates/ivmlite-test/src/sql.rs`
- Delete: `crates/ivmlite-test/src/schema.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-test/src/lib.rs`, `crates/ivmlite-test/src/query.rs` (keeps only `enumerate`)
- Modify: every file that references these types (`oracle.rs` / `data.rs` / `ops.rs` / `differential.rs` / `engine.rs` / `naive.rs` / `buggy.rs` / `tests/harness_catches_bugs.rs`)

**Interfaces:**
- Consumes: no new dependency
- Produces: `ivmlite_core::{ColumnType, Column, Schema, AggFn, Agg, Predicate, ViewQuery}`, methods `Schema::arity()`, `Schema::column_names()`, `ViewQuery::output_arity()`; `ivmlite_test::sql::{create_table_sql, view_query_to_sql}`

> **Why the move is necessary**: the engine must live in `ivmlite-core`, and the signature of `Engine::create_view` contains `&Schema` and `&ViewQuery`. These two types now live in `ivmlite-test`, so the engine would either depend on test (the dependency direction reversed, which spec §4.2 forbids) or live in test (then it is not the product). These types are in essence v0's plan IR — `ViewQuery { group_by, aggs, predicate }` is exactly the flattened form of the v0 subset of spec §5.2's `Plan`.

- [ ] **Step 1: Move the types, keeping behaviour unchanged**

`crates/ivmlite-core/src/schema.rs`: move `ColumnType` / `Column` / `Schema` over from `ivmlite-test/src/schema.rs` unchanged, **but do not move `create_table_sql`** — it goes, together with its unit tests, to `ivmlite-test/src/sql.rs`. `Schema` keeps `arity()` and `column_names()`.

`crates/ivmlite-core/src/query.rs`: move `AggFn` / `Agg` / `Predicate` / `ViewQuery` and `output_arity()`; **do not move `to_sql`, and do not move `enumerate`**.

The types in both files keep their existing derives and gain serde's conditional derive, consistent with `Value` / `Row`:

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
```

`crates/ivmlite-test/src/sql.rs`:

```rust
//! Render core's plan IR as SQL that SQLite can execute.
//!
//! This layer deliberately stays in `ivmlite-test` rather than going into
//! `ivmlite-core`: its only reason to exist is to drive the oracle (letting SQLite
//! compute the answer itself as the authoritative judge). Core owns the IR; test owns "how to express this IR as SQL".

use ivmlite_core::{AggFn, ColumnType, Predicate, Schema, ViewQuery};

fn sql_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
    }
}

/// Generate a STRICT CREATE TABLE statement (spec §7.1: STRICT pins column types; otherwise type affinity
/// lets one column store different types row by row, splitting one logical group key in two).
pub fn create_table_sql(schema: &Schema) -> String {
    let cols: Vec<String> = schema
        .columns
        .iter()
        .map(|c| {
            let null = if c.nullable { "" } else { " NOT NULL" };
            format!("\"{}\" {}{}", c.name, sql_type(c.ty), null)
        })
        .collect();
    format!(
        "CREATE TABLE \"{}\" ({}) STRICT",
        schema.table,
        cols.join(", ")
    )
}

/// Render a ViewQuery as SQL. Column indices are resolved against `schema`.
pub fn view_query_to_sql(query: &ViewQuery, schema: &Schema) -> String {
    let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

    let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();
    for agg in &query.aggs {
        select.push(match (agg.func, agg.column) {
            (AggFn::Count, _) => "COUNT(*)".to_string(),
            (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
            (AggFn::Sum, None) => panic!("SUM must name a column"),
        });
    }

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
        "SELECT {} FROM \"{}\"{} GROUP BY {}",
        select.join(", "),
        schema.table,
        where_clause,
        group
    )
}
```

Move the existing unit tests of `create_table_sql` and `to_sql` into `sql.rs`'s test module as well, changed to call the free functions.

- [ ] **Step 2: Fix dependencies and exports**

`crates/ivmlite-core/Cargo.toml` is unchanged (still zero required dependencies).

`crates/ivmlite-core/src/lib.rs`:

```rust
mod query;
mod row;
mod schema;
mod value;
mod zset;

pub use query::{Agg, AggFn, Predicate, ViewQuery};
pub use row::Row;
pub use schema::{Column, ColumnType, Schema};
pub use value::Value;
pub use zset::ZSet;
```

`crates/ivmlite-test/src/lib.rs`: delete `mod schema;` and its `pub use`, add `mod sql;` and `pub use sql::{create_table_sql, view_query_to_sql};`. To keep the existing call sites mostly unchanged, **re-export these core types from test**:

```rust
pub use ivmlite_core::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
```

- [ ] **Step 3: Fix every call site**

`schema.create_table_sql()` → `create_table_sql(&schema)`; `query.to_sql(&schema)` → `view_query_to_sql(&query, &schema)`. After the change, run:

Run: `cargo build --workspace --locked`
Expected: it compiles.

- [ ] **Step 4: Confirm zero behaviour change**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: **the test count is exactly the same as at the end of Task 1, and everything passes.** This is a pure move; any change in the test count means something was left behind or changed behaviour.

- [ ] **Step 5: Verify by mutation that the move did not weaken any guard**

What a move most easily loses silently is an assertion like "STRICT". Run two mutations to confirm they moved along:

1. Remove the `STRICT` suffix in `create_table_sql` → expect `create_table_sql_is_strict` to go red
2. Make the `Count` arm of `view_query_to_sql` output `COUNT(1)` → expect the exact-string test of `to_sql` to go red

For both, first confirm the broken code compiles. After restoring, confirm everything is green.

- [ ] **Step 6: Commit**

```bash
git add crates/
git commit -m "refactor(core): move the plan IR type family into ivmlite-core, keep SQL rendering in ivmlite-test"
```

---

## Task 3: The `Database` type and a multi-table oracle

**Files:**
- Create: `crates/ivmlite-core/src/database.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-test/src/oracle.rs`

**Interfaces:**
- Consumes: `Schema` (Task 2)
- Produces: `ivmlite_core::Database`, methods `single(Schema) -> Database`, `tables(&self) -> &[Schema]`, `get(&self, &str) -> Option<&Schema>`, `len()`, `is_empty()`; `recompute_via_sqlite(&Database, &ViewQuery, &BTreeMap<String, ZSet>) -> Result<ZSet, EngineError>`

- [ ] **Step 1: Write the failing test**

`crates/ivmlite-core/src/database.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType};

    fn s(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![Column {
                name: "a".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        }
    }

    #[test]
    fn single_wraps_one_schema() {
        let db = Database::single(s("orders"));
        assert_eq!(db.len(), 1);
        assert_eq!(db.get("orders").map(|x| x.table.as_str()), Some("orders"));
    }

    #[test]
    fn get_returns_none_for_unknown_table() {
        assert!(Database::single(s("orders")).get("nope").is_none());
    }

    #[test]
    fn table_order_is_preserved() {
        let db = Database::new(vec![s("b"), s("a")]);
        let names: Vec<&str> = db.tables().iter().map(|t| t.table.as_str()).collect();
        assert_eq!(names, vec!["b", "a"], "order must be preserved — a failing case must replay exactly from its seed");
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p ivmlite-core database`
Expected: a compile failure, `cannot find type Database`

- [ ] **Step 3: Write the implementation**

```rust
use crate::Schema;

/// Every base table a differential case involves.
///
/// `Vec` rather than `HashMap`: the number of tables is in single digits, so a linear scan for lookup by name does not matter,
/// while deterministic order is a hard requirement — a failing case must replay exactly from its seed (spec §9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Database {
    tables: Vec<Schema>,
}

impl Database {
    pub fn new(tables: Vec<Schema>) -> Self {
        Database { tables }
    }

    /// A single-table case is just a multi-table case with one table.
    pub fn single(schema: Schema) -> Self {
        Database { tables: vec![schema] }
    }

    pub fn tables(&self) -> &[Schema] {
        &self.tables
    }

    pub fn get(&self, table: &str) -> Option<&Schema> {
        self.tables.iter().find(|s| s.table == table)
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}
```

`lib.rs` gains `mod database;` and `pub use database::Database;`.

- [ ] **Step 4: The multi-table oracle**

Change the signature and table-building logic of `crates/ivmlite-test/src/oracle.rs`:

```rust
/// The authoritative judge: load every base-table state into an in-memory SQLite and let SQLite execute the original SQL itself.
///
/// An implementation independent of all of this project's code — judging NaiveRecompute with NaiveRecompute
/// would be circular, which is exactly why this exists separately.
pub fn recompute_via_sqlite(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;

    for schema in db.tables() {
        conn.execute_batch(&create_table_sql(schema))
            .map_err(|e| EngineError(e.to_string()))?;

        let base = bases.get(&schema.table).ok_or_else(|| {
            EngineError(format!("missing the base state for table {}", schema.table))
        })?;

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

        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "the base state of table {} has a negative weight {weight}, row {row:?}",
                    schema.table
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

    // View SQL is still rendered for a single table; rendering join queries arrives in the engine plan's Phase 3.
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database has no tables".into()))?;
    let sql = view_query_to_sql(query, anchor);

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

Change the oracle's existing unit tests to pass `Database::single(...)` and a one-element `BTreeMap`, and add two new ones:

```rust
#[test]
fn builds_every_table_in_the_database() {
    // Two tables, and the query reads only one; the other must also be created and loaded,
    // otherwise join queries (Phase 3) would silently be one table short on the oracle side.
    let db = Database::new(vec![orders(), customers()]);
    let bases = BTreeMap::from([
        ("orders".to_string(), ZSet::from_rows([(row(Value::Text("a".into()), 10), 1)])),
        ("customers".to_string(), ZSet::from_rows([(cust_row("a"), 1)])),
    ]);
    let q = count_by_region();
    let got = recompute_via_sqlite(&db, &q, &bases).unwrap();
    assert_eq!(got.len(), 1);
}

#[test]
fn missing_base_state_for_a_declared_table_is_an_error() {
    let db = Database::new(vec![orders(), customers()]);
    let bases = BTreeMap::from([("orders".to_string(), ZSet::new())]);
    let err = recompute_via_sqlite(&db, &count_by_region(), &bases).unwrap_err();
    assert!(err.0.contains("customers"), "the error should name the missing table: {}", err.0);
}
```

- [ ] **Step 5: All green + mutation verification**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`

Two mutations:

1. Make the oracle create only `db.tables()[0]` → expect `builds_every_table_in_the_database` to go red
2. Replace the missing-table `ok_or_else` with a silent fallback such as `unwrap_or(&ZSet::new())` → expect `missing_base_state_for_a_declared_table_is_an_error` to go red

First confirm the mutated code compiles. After restoring, confirm everything is green.

- [ ] **Step 6: Register the gates and commit**

Add two rows to the "ivmlite-test: semantic contracts" table in `docs/mutation-gates.md`, with the "verified" cell set to `**verified**`.

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(core): the Database type; the oracle creates every table in the database"
```

---

## Task 4: Multi-table generators

**Files:**
- Modify: `crates/ivmlite-test/src/data.rs`, `crates/ivmlite-test/src/ops.rs`

**Interfaces:**
- Consumes: `Database` (Task 3), `Domain`, `Op`
- Produces: `gen_database(&mut StdRng, table_count: usize) -> Database`, `gen_initial(&mut StdRng, &Database, &Domain, rows_per_table: usize) -> BTreeMap<String, Vec<Row>>`, `gen_ops(&mut StdRng, &Database, &Domain, &BTreeMap<String, Vec<Row>>, count: usize) -> Vec<(String, Op)>`

- [ ] **Step 1: Write the failing tests**

Add to the test module of `crates/ivmlite-test/src/ops.rs`:

```rust
#[test]
fn generated_database_tables_have_exactly_two_columns() {
    let mut rng = StdRng::seed_from_u64(1);
    let db = gen_database(&mut rng, 2);
    assert_eq!(db.len(), 2);
    for t in db.tables() {
        assert_eq!(
            t.arity(),
            2,
            "spec §9.2 item 4: the differential schema is fixed at 2 columns per table — widening to 3 grows the exhaustive space about 8×, \
             which is the precondition for \"exhaustive beats random\", not a magic number"
        );
    }
}

#[test]
fn ops_are_tagged_with_a_table_that_exists() {
    let mut rng = StdRng::seed_from_u64(2);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 20);
    for (table, _) in gen_ops(&mut rng, &db, &domain, &initial, 200) {
        assert!(db.get(&table).is_some(), "unknown table {table}");
    }
}

#[test]
fn every_table_receives_some_ops() {
    // If the generator writes only to one table, only one of join's two paths, ΔR⋈S and R⋈ΔS, is tested.
    let mut rng = StdRng::seed_from_u64(3);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 20);
    let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
    for t in db.tables() {
        let n = ops.iter().filter(|(tbl, _)| *tbl == t.table).count();
        assert!(n > 20, "table {} received only {n} ops; the two delta paths are unevenly covered", t.table);
    }
}

#[test]
fn deletes_target_rows_that_exist_in_their_own_table() {
    // Biased sampling must keep a live set per table — deleting one table's row from another table is an illegal sequence.
    let mut rng = StdRng::seed_from_u64(4);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 30);
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    let mut hits = 0usize;
    let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
    for (table, op) in &ops {
        let l = live.get_mut(table).expect("the table must exist");
        match op {
            Op::Insert(r) => l.push(r.clone()),
            Op::Delete(r) => {
                let pos = l.iter().position(|x| x == r).expect("a DELETE must hit a row that exists in its own table");
                l.swap_remove(pos);
                hits += 1;
            }
            Op::Update { old, new } => {
                let pos = l.iter().position(|x| x == old).expect("an UPDATE must hit a row that exists in its own table");
                l.swap_remove(pos);
                l.push(new.clone());
                hits += 1;
            }
        }
    }
    assert!(hits > ops.len() / 10, "biased sampling produced too few deletes/updates: {hits}/{}", ops.len());
}

#[test]
fn same_seed_yields_the_same_multi_table_sequence() {
    let make = || {
        let mut rng = StdRng::seed_from_u64(99);
        let db = gen_database(&mut rng, 2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 10);
        gen_ops(&mut rng, &db, &domain, &initial, 50)
    };
    assert_eq!(make(), make());
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ivmlite-test ops`
Expected: a compile failure, `cannot find function gen_database`

- [ ] **Step 3: Write the implementation**

Add to `data.rs`:

```rust
/// Generate the table structure of a differential case.
///
/// Every table has exactly 2 columns (a nullable TEXT + a non-null INTEGER); the column count is **not** a tunable parameter:
/// spec §9.2 item 4 makes it the precondition for "exhaustive beats random"; measured, widening to 3 columns grows
/// the exhaustive space from about 554 to about 4209.
pub fn gen_database(_rng: &mut StdRng, table_count: usize) -> Database {
    let tables = (0..table_count)
        .map(|i| Schema {
            table: format!("t{i}"),
            columns: vec![
                Column { name: "k".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "v".into(), ty: ColumnType::Integer, nullable: true },
            ],
        })
        .collect();
    Database::new(tables)
}

pub fn gen_initial(
    rng: &mut StdRng,
    db: &Database,
    domain: &Domain,
    rows_per_table: usize,
) -> BTreeMap<String, Vec<Row>> {
    db.tables()
        .iter()
        .map(|s| (s.table.clone(), gen_rows(rng, s, domain, rows_per_table)))
        .collect()
}
```

> Both columns are nullable: `v` is nullable so that spec §6.1's path "`SUM` returns NULL when there is no non-NULL input" is actually reachable in random testing — M0's integration test made `amount` nullable for exactly this reason.

Change `gen_ops` in `ops.rs`: keep a live set per table, pick a table at random before picking an operation at each step, and return `(table name, Op)`. Tables are picked uniformly — `every_table_receives_some_ops` guards this.

- [ ] **Step 4: All green + mutation verification**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`

Three mutations:

1. Change `gen_database`'s column count to 3 → expect `generated_database_tables_have_exactly_two_columns` to go red
2. Make `gen_ops` write only to `db.tables()[0]` → expect `every_table_receives_some_ops` to go red
3. Make `gen_ops` sample delete/update targets across tables (from any table's live set) → expect `deletes_target_rows_that_exist_in_their_own_table` to go red

- [ ] **Step 5: Register the gates and commit**

One row each, with "verified" set to `**verified**`.

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(test): multi-table generators — per-table live sets, table-tagged op sequences"
```

---

## Task 5: A multi-table driver

**Files:**
- Modify: `crates/ivmlite-test/src/differential.rs`, `crates/ivmlite-test/src/engine.rs`, `crates/ivmlite-test/src/naive.rs`, `crates/ivmlite-test/src/buggy.rs`, `crates/ivmlite-test/src/shrink.rs`, `crates/ivmlite-test/tests/harness_catches_bugs.rs`

**Interfaces:**
- Consumes: everything the first four tasks produce
- Produces: `TestCase { seed, database: Database, query: ViewQuery, initial: BTreeMap<String, Vec<Row>>, ops: Vec<(String, Op)>, batching }`; `gen_case(seed: u64, &Database, &Domain, rows_per_table: usize, op_count: usize, Batching) -> TestCase` (**the original signature's `&Schema` becomes `&Database`; no new variant is added**); `Engine::create_view(&mut self, &Database, &ViewQuery, &BTreeMap<String, ZSet>) -> Result<(), EngineError>` (the `apply` / `refresh` / `materialize` signatures are unchanged); `is_legal(&BTreeMap<String, Vec<Row>>, &[(String, Op)]) -> bool`

- [ ] **Step 1: Change the `Engine` trait and TestCase**

`create_view` takes `&Database` and the per-table initial state. `apply(table, raw)` already carries the table name (spec §8.5); **do not touch it**.

Each batch of `run`'s loop becomes: group the batch's `(table name, Op)` by table → call `apply` once for each table with changes → call `refresh` once → `materialize` → invariants → oracle comparison. The reference `bases: BTreeMap<String, ZSet>` advances in step with the engine.

> **`refresh` is called once per batch**, not once per table. This is exactly the "N applies, one refresh" shape spec §8.2 describes, and the only place consolidation can do anything.

- [ ] **Step 2: Update the reference implementation and the fake engines to match**

`NaiveRecompute` holds `base: BTreeMap<String, ZSet>` and `pending: Vec<(String, Row, i64)>` per table; `refresh` merges pending into each table. `NoRetractionEngine` / `TransientDriftEngine` follow.

**Both fake engines must keep their existing wrong behaviour unchanged** — they are instruments that prove the harness works, not defects to fix.

- [ ] **Step 3: Make the shrinker's legality gate multi-table**

`is_legal` is now `fn is_legal(initial: &[Row], ops: &[Op]) -> bool`, with the single-table assumption baked into its signature. Change it to decide per table:

```rust
/// Legality of a sequence: every DELETE / UPDATE must hit a row that exists at that moment in **its own table**.
///
/// After going multi-table this is easier to get wrong: deleting from table B with a row of table A is an illegal sequence, and the engine never
/// has an obligation to handle illegal input — "failing" on an illegal sequence means nothing, and that is the whole reason for a home-grown shrinker
/// rather than proptest (spec §9.3).
pub fn is_legal(initial: &BTreeMap<String, Vec<Row>>, ops: &[(String, Op)]) -> bool {
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    for (table, op) in ops {
        let Some(l) = live.get_mut(table) else {
            return false; // unknown table
        };
        match op {
            Op::Insert(r) => l.push(r.clone()),
            Op::Delete(r) => match l.iter().position(|x| x == r) {
                Some(i) => {
                    l.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match l.iter().position(|x| x == old) {
                Some(i) => {
                    l.swap_remove(i);
                    l.push(new.clone());
                }
                None => return false,
            },
        }
    }
    true
}
```

`shrink`'s three phases follow: phase one deletes op ranges and phase three deletes initial rows (now per table), both passing the `is_legal` gate; phase two, shrinking the query, still needs no gate (shrinking the query does not affect the sequence's legality).

Change the existing `is_legal` unit tests to the multi-table form, and **add one specific to multiple tables**:

```rust
#[test]
fn deleting_a_row_that_exists_in_another_table_is_illegal() {
    // With a single table this shape cannot exist; after going multi-table it is the cell most easily written wrong.
    let initial = BTreeMap::from([
        ("t0".to_string(), vec![row(1)]),
        ("t1".to_string(), vec![]),
    ]);
    let ops = vec![("t1".to_string(), Op::Delete(row(1)))];
    assert!(!is_legal(&initial, &ops), "t1 does not contain this row, even though t0 does");
}
```

- [ ] **Step 4: All green, with no regression in the acceptance thresholds**

Run: `cargo test --workspace --locked`
Expected: everything passes. **`harness_catches_the_missing_retraction_bug` must still detect ≥90%, and `failing_case_shrinks_to_under_ten_ops` must still reach ≤10 steps.** Either regression means the refactor touched something it should not have — do not relax the assertions; find the cause.

- [ ] **Step 5: Add a multi-table end-to-end test**

```rust
#[test]
fn a_two_table_case_runs_green_against_the_reference_engine() {
    // This Phase's deliverable: the harness can express multi-table cases.
    // The query is still a single-table aggregate (join is in the engine plan's Phase 3), but both tables receive changes,
    // so apply's table-name routing, the per-table live sets, and the oracle's multi-table setup are all really exercised.
    let mut rng = StdRng::seed_from_u64(7);
    let db = gen_database(&mut rng, 2);
    let case = gen_case(7, &db, &Domain::default(), 25, 150, Batching::Chunks(5));
    let mut engine = NaiveRecompute::new();
    run(&mut engine, &case).unwrap_or_else(|f| panic!("the reference implementation should not fail: {f}"));
}
```

- [ ] **Step 6: Verify the routing by mutation**

1. Make `run` send every delta to `db.tables()[0]` → expect `a_two_table_case_runs_green_against_the_reference_engine` to go red (the second table's changes are lost, and the oracle comparison fails)
2. Make `run` call `refresh` once per table rather than once per batch → **expect it to stay green** (semantically equivalent for `NaiveRecompute`). Record this row as "known unguarded", left to the engine plan — what can really tell them apart is an engine that does consolidation inside `refresh`.

- [ ] **Step 7: Register the gates and commit**

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(test): a multi-table differential driver — route apply by table, one refresh per batch"
```

---

## Self-Review

**1. Spec coverage**

| Spec requirement | Where it lands |
|---|---|
| §4.2 core does not depend on rusqlite | Task 2 (what moves in is pure data types; SQL rendering stays in test) |
| §5.1 the weight invariant | Unchanged, guarded by M0's existing tests; Task 1 verifies they work |
| §8.5 `apply` carries a table name | Task 5 makes it really route to several tables for the first time — with only one table in M0 the parameter was a formality |
| §8.5 refresh separate from apply | Task 5, one refresh per batch |
| §9.2 item 4, 2 columns per table | Task 4 (`gen_database` + assertion + mutation) |
| §9.4 deterministic order, seed replay | Task 3 (`Database` uses a `Vec` to keep order), Task 4 (same seed, same sequence) |
| §9.1 oracle independence | Task 3 keeps it unchanged, only extending it to multiple tables |

**Known gaps (intentional)**: SQL rendering for join queries, the Join operator, and consolidation are not in this plan — they belong to the engine plan. For a multi-table database this plan's oracle renders only the first table's single-table query, as the code comment in Task 3 states.

**2. Placeholder scan**: no TBD / TODO. Task 4 Step 3 gives `gen_ops` only a description of the change rather than complete code — the **only** exception in this plan, because it is a per-table decomposition of an existing function, and a full rewrite would hide "what changed". If the implementer finds the description insufficient to follow, they should report NEEDS_CONTEXT rather than improvise.

**3. Type consistency**: `BTreeMap<String, ZSet>` as the per-table state runs through Tasks 3 / 5; `BTreeMap<String, Vec<Row>>` as the per-table initial rows runs through Tasks 4 / 5; `Vec<(String, Op)>` as the table-tagged op sequence runs through Tasks 4 / 5. `Database` uses `Vec<Schema>` internally rather than a map, because order must be preserved.

---

## Execution order

Tasks 1 → 5 depend strictly on each other. Task 1 is the safety-net check and **must come first**: the next four tasks all refactor M0's code, and what protects a refactor is those tests; first confirm they really catch.

After this plan is complete, write the engine plan (the operator representation of the plan IR, `Arrangement`, Filter/Project/Aggregate, consolidation, Join), whose code will be written against the real types this plan produces.
