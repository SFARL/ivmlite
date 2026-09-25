# M1b Phase 2b: The SQL Front End Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new crate, `ivmlite-sql`, compiles a view's `SELECT` into the engine's `Plan`, its output columns and its base tables, and rejects everything outside v0's subset with an error that names what it rejected.

**Architecture:** `lower` in `ivmlite-core` becomes the one place legality lives: it takes a `ResolvedView` (tables as schemas, columns as indices), and the harness's `ViewQuery` path becomes a thin resolver, `lower_query`. `ivmlite-sql` parses with `sqlparser`'s SQLite dialect, resolves names against a `Catalog` trait, and calls the same `lower`. The proof that the front end is right: every query the differential harness verifies — 153 single-table and 900 join queries over each of two databases — rendered to SQL and compiled, yields exactly the `Plan` `lower_query` builds from the query itself.

**Tech Stack:** Rust 1.95; new dependency `sqlparser` 0.63 (default features) in `ivmlite-sql` only.

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md` (§4.1, §4.3 `ivmlite-sql`; §5.2; §5.3; §6.1; §12.5; §13)

**Preceding plan:** `docs/superpowers/plans/2026-09-24-m1b-phase2a-predicate-whitelist.md` (complete and merged into master, 57e59fa)

## Global Constraints

- **English only** in everything committed: code, comments, error strings, docs, commit messages.
- `ivmlite-core` must not depend on `rusqlite`, `libsqlite3-sys` or `sqlparser` (spec §4.2). Dependencies run one way: `ivmlite-sql` → `ivmlite-core`. No `unsafe` anywhere in this plan.
- Every spec-mandated behaviour this plan adds gets a row in `docs/mutation-gates.md`, with the mutation actually run: break the code, confirm it compiles, run `cargo test --workspace --locked --no-fail-fast`, sum `passed`/`failed` over every test binary, restore and confirm `git diff` shows no leftover. Record `N passed / M failed (baseline B/0)` and every red test, read off the terminal, never computed. A row whose mutation cannot be run starts its Verified cell with `None — ` and says why. After editing the table, `python3 scripts/count-mutation-gates.py` must print `consistent`; delete `scripts/__pycache__` if it appears.
- Before every commit: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked --no-fail-fast` all clean.
- Stage specific files, never `git add -A`. Never `--no-verify`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not loosen any existing test threshold.

---

## This plan's scope and the rulings behind it

M1b's phases: 1, state ownership (done); 2a, the predicate whitelist (done); **2b, `ivmlite-sql` (this plan)**; 3, `ivmlite-sqlite`; 4, the benchmark.

The design decisions below were approved before this plan was written (items 2–8 of the Phase 2 design, 2026-09-24). **The code in this plan was prototyped and run before it was written down**: against `sqlparser` 0.63.0 and the bundled SQLite 3.53, the whole workspace passed at 258 tests with fmt and clippy clean, and the mutations named in the gate steps were run and went red as stated. Treat the code as verified text to transcribe; the tests are the binding specification if anything disagrees.

### Ruling 1: `lower` holds every legality check; front ends only resolve names

`lower(&ResolvedView) -> Result<Plan, PlanError>`. A `ResolvedView` names its tables by `&Schema` and its columns by index into the joined row. Every check that exists today — the non-empty GROUP BY and aggregates, SUM's column and type, bounds, literal types, and the join checks (self-join, key bounds, key types, formerly in `resolve_join`) — lives in `lower` (the join ones in its helper `check_join`). The harness's path is `lower_query(&ViewQuery, &Database)`: it takes the anchor as `db.tables()[0]`, looks the join's right table up by name, and calls `lower`. `ivmlite-sql` does the same resolution from SQL.

The builder keeps the name `lower` and the `ViewQuery` adapter takes the new name, so the twenty gate rows that say "in `lower`" stay true; only the five that name `resolve_join` change (Task 1).

### Ruling 2: the accepted SQL

```text
SELECT <column>, …, <aggregate>, …
FROM <table> [[AS] <alias>]
     [[INNER] JOIN <table> [[AS] <alias>] ON <column> = <column>]
[WHERE <column> <op> <literal> | <literal> <op> <column>
     | <column> IS NULL | <column> IS NOT NULL]
GROUP BY <column>, …
```

- A column is `name` or `qualifier.name`; the qualifier is the table's alias if it has one, else its name. Identifiers match ASCII case-insensitively, quoted or not, as in SQLite. An unqualified name must belong to exactly one table.
- The SELECT list's bare columns are exactly the GROUP BY columns, each once, and come before the aggregates. A group key missing from the SELECT list would let two groups produce one row — §5.2's weight problem again. The output order of the group keys is the SELECT order; GROUP BY's order does not matter.
- An aggregate is `COUNT(*)` or `SUM(<column>)`, in any letter case.
- `<op>` is `>` `>=` `<` `<=` `=` `!=` `<>`. A literal on the left flips the operator (`3 < v` is `v > 3`). A literal is a 64-bit integer (with an optional leading `-`, down to `i64::MIN`), a single-quoted string, or `NULL` — which `lower` rejects, pointing at `IS NULL`. Parentheses around the WHERE expression or an operand are allowed.
- `ON` is one equality between a column of each table, in either order.

Everything else is an error naming what it rejected (spec §12.5).

### Ruling 3: result column names are the ones SQLite gives

An alias if there is one; else a bare column's **declared** name (`SELECT K FROM t` is named `k`); else an aggregate's text **exactly as written** (`count( * )`). These were measured with SQLite 3.53 through rusqlite; SQLite documents unaliased names as unspecified, so `result_column_names_match_sqlite` pins them against the bundled SQLite. `sqlparser` 0.63's span for a function call ends at its last argument, not at its `)`, so the text is recovered by matching parentheses from the span's start (`source::call_text`).

Two result columns with the same name, compared case-insensitively, are an error: Phase 3 declares the view's columns as a table, and SQLite rejects a table with two columns of one name. The harness renders a join grouped by `t0.k` and `t1.k`, which would collide, with the second aliased `"t1_k"` (Task 3).

### Ruling 4: `CompiledView` and the `Catalog`

```rust
pub struct CompiledView {
    pub plan: Plan,
    pub columns: Vec<Column>,   // output columns, SELECT order
    pub tables: Vec<String>,    // anchor first — the view's `__ivm_dep` rows
}
pub trait Catalog {
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError>;
}
```

An output column's type and nullability: a group key's are its table column's; `COUNT(*)` is a non-null INTEGER; `SUM` is a nullable INTEGER, since SUM over only NULLs is NULL (§6.1). Phase 3 needs all three fields to create `__ivm_out_<view>` and declare the virtual table. Table names in `plan` and `tables` are the catalog's declared spelling, so the engine routes deltas by the names the triggers will use.

The catalog lookup is fallible now, because Phase 3's SQLite implementation reads `PRAGMA table_info` and the table's DDL, and reports what v0 cannot represent (a non-STRICT table, an `ANY` column, a `COLLATE` clause, §7.1) as a `CatalogError`. `Database` implements `Catalog`, for tests, and never fails.

### Ruling 5: `sqlparser`'s AST is destructured without `..`

`Query`, `Select`, `TableFactor::Table`, `Join`, `TableAlias` and `Function` are destructured field by field and every field v0 does not use is required to be empty. A field a future `sqlparser` adds is then a compile error in `ivmlite-sql`, not a clause the front end silently ignores. The dependency keeps its default features, including `recursive-protection`: the extension will parse SQL a user wrote, and a stack overflow there would abort the host process.

### Ruling 6: equivalence with `lower_query` stands in for running the differential sweeps through SQL

The engine's behaviour is a function of its `Plan`. So if the SQL a query renders to compiles to the `Plan` `lower_query` builds, every differential result for that query holds for the SQL too. `every_enumerated_query_compiles_to_the_plan_lower_query_builds` checks this for all 2 × (153 + 900) queries in under a second, instead of adding a second copy of each sweep. The front end's own ground — literals on the left, aliases, reversed `ON`, case, parentheses, and every rejection — is covered by `ivmlite-sql`'s unit tests, since rendered SQL never exercises it.

### Ruling 7: the three-column schema question is closed — not widening

Spec §9.2 left open whether to widen the differential schema to three columns per table, for the one shape three columns would add: a view that groups by one column, sums a second and filters on a third. That shape is already enumerated. A join's row has four columns — for example `GROUP BY t0.k`, `SUM(t0.v)`, `WHERE t1.v …` — and a single-table view runs the same `Filter → Project → Aggregate` chain. Widening would cost 12285 join queries per database, about 56 seconds per sweep (extrapolated in Phase 2a). Task 4 records the decision in §9.2.

### Out of scope

- The SQLite `Catalog` (`PRAGMA table_info`, STRICT/ANY/COLLATE checks) — Phase 3.
- Self-joins, outer joins, more than two tables, multi-column keys, `AND`/`OR`, expressions, `HAVING`, `ORDER BY` — v0 limitations (§13).
- Storing a view's SQL and re-compiling it on load (§5.3) — Phase 3.

---

## File structure

| File | Change |
|---|---|
| `crates/ivmlite-core/src/plan.rs` | `ResolvedView`, `ResolvedJoin`, `lower_query`, `lower(&ResolvedView)`, `check_join` |
| `crates/ivmlite-core/src/lib.rs` | exports |
| `crates/ivmlite-core/src/engine.rs`, `node.rs` | call `lower_query` |
| `Cargo.toml` | workspace member and dependency `ivmlite-sql` |
| `Cargo.lock` | `sqlparser` and its dependencies |
| `crates/ivmlite-sql/Cargo.toml` | new |
| `crates/ivmlite-sql/src/lib.rs` | new: crate docs, exports |
| `crates/ivmlite-sql/src/catalog.rs` | new: `Catalog`, `CatalogError`, `impl Catalog for Database` |
| `crates/ivmlite-sql/src/source.rs` | new: span → byte offset, a call's text |
| `crates/ivmlite-sql/src/compile.rs` | new: `compile`, `CompiledView`, `SqlError` |
| `crates/ivmlite-test/Cargo.toml` | dev-dependency on `ivmlite-sql` |
| `crates/ivmlite-test/src/sql.rs` | alias a group key whose name is taken |
| `crates/ivmlite-test/tests/sql_front_end.rs` | new: equivalence and naming tests |
| `docs/mutation-gates.md` | rows |
| `docs/superpowers/specs/2026-09-18-ivmlite-design.md` | §4.3, §9.2, §13 |
| `docs/README.md` | index this plan |

---

### Task 1: `lower` takes a resolved view

**Files:**
- Modify: `crates/ivmlite-core/src/plan.rs`, `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-core/src/engine.rs`, `crates/ivmlite-core/src/node.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces (used by Task 2):
  ```rust
  pub struct ResolvedView<'a> { pub anchor: &'a Schema, pub join: Option<ResolvedJoin<'a>>, pub group_by: Vec<usize>, pub aggs: Vec<Agg>, pub predicate: Predicate }
  pub struct ResolvedJoin<'a> { pub right: &'a Schema, pub left_column: usize, pub right_column: usize }
  pub fn lower(view: &ResolvedView) -> Result<Plan, PlanError>;
  pub fn lower_query(query: &ViewQuery, db: &Database) -> Result<Plan, PlanError>;
  ```
  all exported from `ivmlite_core`.

This is a pure refactor: no behaviour changes, and every existing test keeps passing once its calls are renamed.

- [ ] **Step 1: Replace `lower` and `resolve_join` in `crates/ivmlite-core/src/plan.rs`**

Replace everything from the doc comment `/// Lower the harness's flat \`ViewQuery\` into an operator tree` down to (not including) `#[cfg(test)]` with:

```rust
/// A view definition with every name resolved: its tables are schemas, and
/// every column in `group_by`, `aggs` and `predicate` is an index into the row
/// the operators above the scans see — the anchor's columns, then (for a join)
/// the right table's.
///
/// Both front ends build one and hand it to `lower`, which holds every
/// legality check: `lower_query` from the harness's `ViewQuery`, and
/// `ivmlite-sql` from SQL (M1b Phase 2b). Resolving names is theirs; deciding
/// what v0 supports is `lower`'s alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedView<'a> {
    pub anchor: &'a Schema,
    pub join: Option<ResolvedJoin<'a>>,
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
}

/// A two-table inner equi-join: `anchor.columns[left_column] = right.columns[right_column]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJoin<'a> {
    pub right: &'a Schema,
    /// A column of the anchor table.
    pub left_column: usize,
    /// A column of the right table.
    pub right_column: usize,
}

/// Lower the harness's flat `ViewQuery` against `db`: resolve its tables, then
/// `lower`.
///
/// The view's left (or only) input is the anchor, `db.tables()[0]`. In M1b the
/// table order comes from `__ivm_dep`, not from the query's FROM clause —
/// choosing "the first" is only v0's convention (M1a Phase 2 final review
/// Finding I). A join's right input is looked up by name in `db`.
pub fn lower_query(query: &ViewQuery, db: &Database) -> Result<Plan, PlanError> {
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| PlanError("the database has no tables".into()))?;
    let join = match &query.join {
        None => None,
        Some(join) => Some(resolve_join(join, db)?),
    };
    lower(&ResolvedView {
        anchor,
        join,
        group_by: query.group_by.clone(),
        aggs: query.aggs.clone(),
        predicate: query.predicate.clone(),
    })
}

/// Look up a `ViewQuery` join's right table by name.
fn resolve_join<'a>(join: &Join, db: &'a Database) -> Result<ResolvedJoin<'a>, PlanError> {
    let right = db
        .get(&join.right)
        .ok_or_else(|| PlanError(format!("join: table {} is not in the database", join.right)))?;
    Ok(ResolvedJoin {
        right,
        left_column: join.left_column,
        right_column: join.right_column,
    })
}

/// Lower a resolved view into an operator tree, enforcing §5.2's legality
/// checks at the boundary.
///
/// Every front end ends here, so these checks are the only ones: `enumerate`
/// never produces an illegal shape, but a `ViewQuery` can be constructed
/// freely, and SQL can say anything.
///
/// Column indices are checked against the row the operators above the scans
/// see — the anchor's columns, then (for a join) the right table's — and
/// their types drive the type checks: v0 supports `SUM` over INTEGER columns
/// only, comparisons only against a literal of the column's own type, and
/// join keys of one type only.
pub fn lower(view: &ResolvedView) -> Result<Plan, PlanError> {
    let anchor = view.anchor;
    if let Some(join) = &view.join {
        check_join(join, anchor)?;
    }
    let right = view.join.as_ref().map(|j| j.right);
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
    if view.group_by.is_empty() {
        return Err(PlanError(
            "spec §5.2: a view's root operator must be an Aggregate with a \
             non-empty GROUP BY; global aggregates are forbidden (over an empty \
             table one returns 1 row where a grouped aggregate returns 0, so the \
             rule \"delete the row when its group's count reaches zero\" is wrong \
             for it)"
                .into(),
        ));
    }
    if view.aggs.is_empty() {
        return Err(PlanError(
            "spec §5.2: a query with no aggs is really Scan→Project made \
             directly into a view, a shape in which Z-set weights and SQL row \
             counts disagree"
                .into(),
        ));
    }

    // `SUM` must carry a column. This check is **not** redundant: `Agg::column`
    // is an `Option<usize>` (`None` for `COUNT(*)`), so the type system cannot
    // stop `Agg { func: Sum, column: None }`, and `AggState::absorb` can only
    // `expect` SUM's column — without this gate a legally constructed
    // `ViewQuery` would pass through `lower` and `Node::build` and panic only on
    // the first batch of deltas. The reasoning of `out_of_range_column_is_rejected`
    // applies unchanged: the engine should have no foreseeable panic path after
    // create_view.
    for (i, agg) in view.aggs.iter().enumerate() {
        if agg.func == AggFn::Sum && agg.column.is_none() {
            return Err(PlanError(format!(
                "spec §5.2: agg {i} is SUM but names no column; only COUNT(*) may omit one"
            )));
        }
    }

    let check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} references column index {c}, but {source} has only {arity} columns"
            )))
        } else {
            Ok(())
        }
    };
    for &c in &view.group_by {
        check(c, "group_by")?;
    }
    for agg in &view.aggs {
        if let Some(c) = agg.column {
            check(c, "agg")?;
        }
    }
    match &view.predicate {
        Predicate::None => {}
        Predicate::Compare { column, .. }
        | Predicate::IsNull { column }
        | Predicate::IsNotNull { column } => check(*column, "predicate")?,
    }

    // Type checks. These run after the bounds checks because they index
    // `schema.columns`, which is only safe once every index is known valid.
    //
    // v0 cannot agree with SQLite on either of these over a TEXT column, so
    // they are rejected here instead of producing a silent divergence
    // (external review P2-1). Measured against SQLite, `STRICT` table
    // `t(g INTEGER, v TEXT)`:
    // - `SUM(v)` coerces numeric-looking text and returns `7.0` — a REAL — for
    //   rows `('7')` and `('abc')`. v0's `Value` has no `Real` variant at all
    //   (floating-point addition is not associative), and its accumulator only
    //   sees `Value::Int`, so it would report NULL.
    // - Every comparison operator has the same problem, not just `>`: SQLite
    //   converts a mismatched literal by the column's affinity (so
    //   `v > 3` is true for every TEXT value, since SQLite orders storage
    //   classes as NULL < INTEGER/REAL < TEXT < BLOB), while v0 compares a
    //   column only with a literal of its own type.
    // `IS NULL` and `IS NOT NULL` are type-agnostic and stay allowed on any column.
    let require_integer = |c: usize, what: &str| -> Result<(), PlanError> {
        let col = columns[c];
        if col.ty == ColumnType::Integer {
            Ok(())
        } else {
            Err(PlanError(format!(
                "{what} over column {c} (`{}`) of type {:?} is not supported: \
                 v0 allows it over INTEGER columns only",
                col.name, col.ty
            )))
        }
    };
    for agg in &view.aggs {
        if let (AggFn::Sum, Some(c)) = (agg.func, agg.column) {
            require_integer(c, "SUM")?;
        }
    }
    if let Predicate::Compare { column, op, value } = &view.predicate {
        let col = columns[*column];
        let matches = match (col.ty, value) {
            (_, Value::Null) => {
                return Err(PlanError(format!(
                    "the comparison {op:?} on column {column} (`{}`) has a NULL literal, \
                     which makes it UNKNOWN for every row (spec §6.1); use IS NULL or \
                     IS NOT NULL instead",
                    col.name
                )))
            }
            (ColumnType::Integer, Value::Int(_)) | (ColumnType::Text, Value::Text(_)) => true,
            _ => false,
        };
        if !matches {
            return Err(PlanError(format!(
                "the comparison {op:?} between column {column} (`{}`) of type {:?} and the \
                 literal {value:?} is not supported: v0 compares a column only with a literal \
                 of its own type, while SQLite would convert the literal by the column's \
                 affinity (spec §6.1)",
                col.name, col.ty
            )));
        }
    }

    // The columns the projection keeps: group keys first (in their original
    // order), then each agg's column (in order), deduplicated. The order must be
    // deterministic, or Aggregate's index remapping has nothing to line up
    // against (spec §9.4).
    let mut keep: Vec<usize> = Vec::new();
    for &c in &view.group_by {
        if !keep.contains(&c) {
            keep.push(c);
        }
    }
    for agg in &view.aggs {
        if let Some(c) = agg.column {
            if !keep.contains(&c) {
                keep.push(c);
            }
        }
    }
    // `keep` always starts with the group_by columns, in their original order
    // and (at this point) deduplicated, so as long as group_by itself has no
    // duplicates, remap(group_by[i]) is provably always i — the group_by
    // remapping is always the trivial identity and cannot go wrong through a
    // remapping bug. The part that can go wrong, and the only part of this logic
    // worth testing, is the agg-column remapping (those columns land after the
    // group_by ones in `keep`, at positions that depend on dedup and order, not
    // trivially).
    let remap = |c: usize| {
        keep.iter()
            .position(|&k| k == c)
            .expect("keep is built from the group_by and agg columns, so it contains them")
    };

    // Each Scan takes every column of its table: Filter's predicate is
    // evaluated against the joined (or base-table) row, and narrowing happens
    // after Filter.
    let scan = |s: &Schema| Plan::Scan {
        table: s.table.clone(),
        columns: (0..s.arity()).collect(),
    };
    let mut node = match &view.join {
        Some(join) => Plan::Join {
            left: Box::new(scan(anchor)),
            right: Box::new(scan(join.right)),
            left_key: join.left_column,
            right_key: join.right_column,
        },
        None => scan(anchor),
    };
    if view.predicate != Predicate::None {
        node = Plan::Filter {
            input: Box::new(node),
            predicate: view.predicate.clone(),
        };
    }
    node = Plan::Project {
        input: Box::new(node),
        columns: keep.clone(),
    };
    Ok(Plan::Aggregate {
        input: Box::new(node),
        group_by: view.group_by.iter().map(|&c| remap(c)).collect(),
        aggs: view
            .aggs
            .iter()
            .map(|a| Agg {
                func: a.func,
                column: a.column.map(remap),
            })
            .collect(),
    })
}

/// Validate a join against its anchor.
fn check_join(join: &ResolvedJoin, anchor: &Schema) -> Result<(), PlanError> {
    let right = join.right;
    if right.table == anchor.table {
        return Err(PlanError(format!(
            "join: a self-join of table {} is not supported in v0 — the differential \
             oracle cannot check one yet (the harness's `ViewQuery` names a table, not an \
             alias, and a self-join needs table aliases)",
            anchor.table
        )));
    }
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
    Ok(())
}
```

What moved where: `lower`'s body is unchanged except that `query.` became `view.` and the `Plan::Join` construction reads `join.right`; `resolve_join` now only looks the right table up; its self-join, key-bound and key-type checks are `check_join`, called first thing in `lower`. The self-join message no longer says the check exists because of `ViewQuery` alone: SQL can express a self-join with aliases, and it is still rejected because the oracle cannot check one yet.

- [ ] **Step 2: Rename the `ViewQuery` call sites**

- `crates/ivmlite-core/src/lib.rs`: `pub use plan::{lower, lower_query, Plan, PlanError, ResolvedJoin, ResolvedView};`
- `crates/ivmlite-core/src/engine.rs`: import `lower_query` instead of `lower`, and `IncrementalEngine::create_view` calls `lower_query(query, db)`.
- `crates/ivmlite-core/src/node.rs` tests: import `lower_query` instead of `lower`; `crate::lower(` → `crate::lower_query(` and `lower(&query, …)` → `lower_query(&query, …)`.
- `crates/ivmlite-core/src/plan.rs` tests: every call `lower(` → `lower_query(` (27 calls; the arguments are unchanged).

Comments that say "`lower` rejects …" stay true and are left alone.

- [ ] **Step 3: Run everything**

`cargo test --workspace --locked --no-fail-fast`: 237 passed / 0 failed, as before. fmt, clippy.

- [ ] **Step 4: Re-verify the gate rows that named `resolve_join`**

`grep -n resolve_join docs/mutation-gates.md` lists five rows (the M1a Phase 3 Task 2 join-scope rows and the join-key-type row). For each, rewrite the mutation's location: the self-join, `left_column`, `right_column` and key-type checks are in `check_join` now; the right table's lookup by name is still in `resolve_join`. Re-run each mutation, record the counts and red tests, and add "(re-verified in M1b Phase 2b after the join checks moved into `check_join`)". Afterwards, `grep -n resolve_join docs/mutation-gates.md` shows only the lookup row.

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-core/src/engine.rs crates/ivmlite-core/src/node.rs docs/mutation-gates.md
git commit -m "refactor(core): lower a resolved view; the ViewQuery path becomes lower_query

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The `ivmlite-sql` crate

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Create: `crates/ivmlite-sql/Cargo.toml`, `crates/ivmlite-sql/src/lib.rs`, `crates/ivmlite-sql/src/catalog.rs`, `crates/ivmlite-sql/src/source.rs`, `crates/ivmlite-sql/src/compile.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `lower`, `ResolvedView`, `ResolvedJoin` (Task 1); `Agg`, `AggFn`, `CmpOp`, `Column`, `ColumnType`, `Database`, `Plan`, `PlanError`, `Predicate`, `Schema`, `Value` from `ivmlite_core`.
- Produces (used by Task 3 and Phase 3): `ivmlite_sql::{compile, CompiledView, SqlError, Catalog, CatalogError}` with `pub fn compile(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError>`.

- [ ] **Step 1: The crate skeleton and the dependency**

In the root `Cargo.toml`, add `"crates/ivmlite-sql"` to `members` (keep the list sorted) and `ivmlite-sql = { path = "crates/ivmlite-sql" }` under `[workspace.dependencies]`.

`crates/ivmlite-sql/Cargo.toml`:

```toml
[package]
name = "ivmlite-sql"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
ivmlite-core.workspace = true
sqlparser = "0.63"
```

`crates/ivmlite-sql/src/lib.rs`:

```rust
//! SQL → plan IR (spec §4.3): parse a view's `SELECT` with `sqlparser`'s SQLite
//! dialect, resolve its names against a [`Catalog`], and lower it through
//! `ivmlite_core::lower`, which holds every legality check. Any query outside
//! v0's subset is a hard error (spec §12.5).

mod catalog;
mod compile;
mod source;

pub use catalog::{Catalog, CatalogError};
pub use compile::{compile, CompiledView, SqlError};
```

The crate builds once Steps 2–4 have created its three modules. Its first build fetches `sqlparser` 0.63.0 and its dependencies, so run it **without** `--locked` once — `cargo build -p ivmlite-sql` — which updates `Cargo.lock`. From then on use `--locked` as usual. `Cargo.lock` is committed with this task.

- [ ] **Step 2: The catalog, test first**

`crates/ivmlite-sql/src/catalog.rs` (its two tests are at the bottom; write the file, then run `cargo test -p ivmlite-sql catalog` after Step 4 makes the crate build):

```rust
use ivmlite_core::{Database, Schema};

/// Where the SQL front end looks tables up (spec §4.3).
///
/// A trait so the front end is testable without SQLite: `Database` implements
/// it here. M1b Phase 3's SQLite implementation reads `PRAGMA table_info` and
/// the table's DDL, and reports what v0 cannot represent — a non-STRICT table,
/// an `ANY` column, a `COLLATE` clause (spec §7.1) — as a `CatalogError`.
pub trait Catalog {
    /// The table called `name`, matched the way SQLite matches identifiers:
    /// ASCII case-insensitively, quoted or not. `Ok(None)` if there is none.
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError>;
}

/// A table exists but cannot be used, or the catalog could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl Catalog for Database {
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError> {
        Ok(self
            .tables()
            .iter()
            .find(|s| s.table.eq_ignore_ascii_case(name))
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{Column, ColumnType};

    fn db() -> Database {
        Database::single(Schema {
            table: "Orders".into(),
            columns: vec![Column {
                name: "amount".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        })
    }

    #[test]
    fn a_database_finds_tables_case_insensitively_and_keeps_the_declared_name() {
        let found = db().table("ORDERS").unwrap().expect("ORDERS names Orders");
        assert_eq!(found.table, "Orders");
    }

    #[test]
    fn a_database_reports_an_unknown_table_as_none() {
        assert_eq!(db().table("customers"), Ok(None));
    }
}
```

- [ ] **Step 3: Recovering a call's text**

`crates/ivmlite-sql/src/source.rs`:

```rust
//! Recovering a result column's text from the view's SQL, so an unaliased
//! aggregate gets the name SQLite gives it: its text exactly as written.

use sqlparser::tokenizer::Location;

/// The byte offset of `location` in `sql`. `sqlparser` counts lines and
/// columns from 1, in characters.
pub fn offset(sql: &str, location: Location) -> usize {
    let mut line_start = 0;
    for _ in 1..location.line {
        line_start += sql[line_start..]
            .find('\n')
            .expect("a location the parser reported lies inside the SQL")
            + 1;
    }
    let column = usize::try_from(location.column).expect("a column fits in usize") - 1;
    sql[line_start..]
        .char_indices()
        .nth(column)
        .map_or(sql.len(), |(i, _)| line_start + i)
}

/// The text of the function call that starts at byte `start`: its name
/// through its matching `)`.
///
/// `sqlparser` 0.63's span for a call ends at its last argument, before the
/// `)` (`COUNT(*)`'s span covers only `COUNT`), so the end is found here by
/// matching parentheses, skipping quoted strings and identifiers.
pub fn call_text(sql: &str, start: usize) -> &str {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in sql[start..].char_indices() {
        match quote {
            Some(close) => {
                if c == close {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '[' => quote = Some(']'),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &sql[start..start + i + c.len_utf8()];
                    }
                }
                _ => {}
            },
        }
    }
    panic!(
        "the parser accepted this call, so its parentheses balance: {}",
        &sql[start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_counts_lines_and_characters() {
        let sql = "SELECT\n  é, COUNT(*)";
        let at = offset(sql, Location::new(2, 6));
        assert_eq!(&sql[at..], "COUNT(*)");
    }

    #[test]
    fn call_text_runs_to_the_matching_parenthesis() {
        let sql = r#"SELECT sum ( "a)b" ), 1"#;
        assert_eq!(call_text(sql, 7), r#"sum ( "a)b" )"#);
    }
}
```

- [ ] **Step 4: The compiler's tests**

Create `crates/ivmlite-sql/src/compile.rs` with the test module below, and above it a stub — `pub struct CompiledView; pub struct SqlError(pub String); pub fn compile(_: &str, _: &dyn crate::Catalog) -> Result<CompiledView, SqlError> { unimplemented!() }` — enough for `lib.rs` to build. Run `cargo test -p ivmlite-sql` and confirm the test module fails to compile (it uses `CompiledView`'s fields) — that is the RED.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{lower_query, Database, Join, ViewQuery};

    /// `orders(region TEXT, amount INTEGER NOT NULL)` and
    /// `regions(name TEXT, manager TEXT)`.
    fn db() -> Database {
        let col = |name: &str, ty, nullable| Column {
            name: name.into(),
            ty,
            nullable,
        };
        Database::new(vec![
            Schema {
                table: "orders".into(),
                columns: vec![
                    col("region", ColumnType::Text, true),
                    col("amount", ColumnType::Integer, false),
                ],
            },
            Schema {
                table: "regions".into(),
                columns: vec![
                    col("name", ColumnType::Text, true),
                    col("manager", ColumnType::Text, true),
                ],
            },
        ])
    }

    fn ok(sql: &str) -> CompiledView {
        compile(sql, &db()).unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    fn err(sql: &str) -> String {
        match compile(sql, &db()) {
            Ok(v) => panic!("{sql} must be rejected, compiled to {v:?}"),
            Err(e) => e.0,
        }
    }

    fn plan_of(q: ViewQuery) -> Plan {
        lower_query(&q, &db()).expect("the expected query is legal")
    }

    fn count() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    fn sum(column: usize) -> Agg {
        Agg {
            func: AggFn::Sum,
            column: Some(column),
        }
    }

    fn query(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery {
            group_by,
            aggs,
            predicate,
            join: None,
        }
    }

    #[test]
    fn compiles_the_spec_example() {
        // Spec §8.3's example view.
        let v = ok("SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(vec![0], vec![sum(1), count()], Predicate::None))
        );
        assert_eq!(v.tables, vec!["orders".to_string()]);
        let names: Vec<&str> = v.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["region", "SUM(amount)", "COUNT(*)"]);
        let types: Vec<(ColumnType, bool)> = v.columns.iter().map(|c| (c.ty, c.nullable)).collect();
        assert_eq!(
            types,
            vec![
                (ColumnType::Text, true),
                // SUM over only NULLs is NULL (spec §6.1); COUNT(*) never is.
                (ColumnType::Integer, true),
                (ColumnType::Integer, false),
            ]
        );
    }

    #[test]
    fn names_resolve_case_insensitively_and_keep_their_declared_spelling() {
        let v = ok(r#"SELECT "REGION", count(*) FROM Orders GROUP BY Region"#);
        assert_eq!(
            v.plan,
            plan_of(query(vec![0], vec![count()], Predicate::None))
        );
        assert_eq!(v.tables, vec!["orders".to_string()]);
        assert_eq!(v.columns[0].name, "region");
    }

    #[test]
    fn an_alias_names_the_result_column() {
        let v = ok("SELECT region AS r, SUM(amount) total FROM orders GROUP BY region");
        let names: Vec<&str> = v.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["r", "total"]);
    }

    #[test]
    fn the_select_order_of_group_keys_is_the_output_order() {
        // GROUP BY's order does not matter; the SELECT list's does.
        let v = ok("SELECT amount, region, COUNT(*) FROM orders GROUP BY region, amount");
        assert_eq!(
            v.plan,
            plan_of(query(vec![1, 0], vec![count()], Predicate::None))
        );
    }

    #[test]
    fn every_comparison_operator_and_both_literal_types_compile() {
        let cases = [
            (">", CmpOp::Gt),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            ("<=", CmpOp::Le),
            ("=", CmpOp::Eq),
            ("!=", CmpOp::Ne),
            ("<>", CmpOp::Ne),
        ];
        for (symbol, op) in cases {
            let sql = format!(
                "SELECT region, COUNT(*) FROM orders WHERE amount {symbol} 3 GROUP BY region"
            );
            let expected = Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(3),
            };
            assert_eq!(
                ok(&sql).plan,
                plan_of(query(vec![0], vec![count()], expected)),
                "{sql}"
            );
        }
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE region = 'it''s' GROUP BY region");
        let expected = Predicate::Compare {
            column: 0,
            op: CmpOp::Eq,
            value: Value::Text("it's".into()),
        };
        assert_eq!(v.plan, plan_of(query(vec![0], vec![count()], expected)));
    }

    #[test]
    fn a_literal_on_the_left_flips_the_operator() {
        // `3 < amount` is `amount > 3` (M1b Phase 2a, Ruling 1).
        for (sql_op, op) in [
            ("<", CmpOp::Gt),
            ("<=", CmpOp::Ge),
            (">", CmpOp::Lt),
            (">=", CmpOp::Le),
            ("=", CmpOp::Eq),
            ("!=", CmpOp::Ne),
        ] {
            let sql = format!(
                "SELECT region, COUNT(*) FROM orders WHERE 3 {sql_op} amount GROUP BY region"
            );
            let expected = Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(3),
            };
            assert_eq!(
                ok(&sql).plan,
                plan_of(query(vec![0], vec![count()], expected)),
                "{sql}"
            );
        }
    }

    #[test]
    fn negative_literals_reach_i64_min() {
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE amount > -9223372036854775808 GROUP BY region");
        let expected = Predicate::Compare {
            column: 1,
            op: CmpOp::Gt,
            value: Value::Int(i64::MIN),
        };
        assert_eq!(v.plan, plan_of(query(vec![0], vec![count()], expected)));
    }

    #[test]
    fn is_null_parentheses_and_is_not_null_compile() {
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE (region IS NULL) GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(
                vec![0],
                vec![count()],
                Predicate::IsNull { column: 0 }
            ))
        );
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE region IS NOT NULL GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(
                vec![0],
                vec![count()],
                Predicate::IsNotNull { column: 0 }
            ))
        );
    }

    #[test]
    fn a_join_with_aliases_and_a_reversed_on_compiles() {
        // The ON condition names the right table first, and its two key
        // columns sit at different positions (orders.region is column 0,
        // regions.manager column 1), so reading one side's index for the
        // other cannot go unnoticed.
        let v = ok(
            "SELECT r.name, SUM(o.amount) FROM orders AS o INNER JOIN regions r \
             ON r.manager = o.region GROUP BY r.name",
        );
        let expected = ViewQuery {
            join: Some(Join {
                right: "regions".into(),
                left_column: 0,
                right_column: 1,
            }),
            ..query(vec![2], vec![sum(1)], Predicate::None)
        };
        assert_eq!(v.plan, plan_of(expected));
        assert_eq!(v.tables, vec!["orders".to_string(), "regions".to_string()]);
    }

    #[test]
    fn an_unqualified_column_resolves_when_only_one_table_has_it() {
        let v = ok(
            "SELECT manager, COUNT(*) FROM orders JOIN regions ON region = name GROUP BY manager",
        );
        let expected = ViewQuery {
            join: Some(Join {
                right: "regions".into(),
                left_column: 0,
                right_column: 0,
            }),
            ..query(vec![3], vec![count()], Predicate::None)
        };
        assert_eq!(v.plan, plan_of(expected));
    }

    #[test]
    fn lower_s_checks_apply_to_sql() {
        // `lower` holds the legality rules; the front end only resolves names.
        assert!(err("SELECT SUM(amount) FROM orders").contains("GROUP BY"));
        assert!(err("SELECT region FROM orders GROUP BY region").contains("aggs"));
        assert!(err("SELECT region, SUM(region) FROM orders GROUP BY region").contains("SUM"));
        assert!(
            err("SELECT region, COUNT(*) FROM orders WHERE amount > 'x' GROUP BY region")
                .contains("comparison")
        );
        assert!(
            err("SELECT region, COUNT(*) FROM orders WHERE amount = NULL GROUP BY region")
                .contains("IS NULL")
        );
        assert!(err(
            "SELECT a.region, COUNT(*) FROM orders a JOIN orders b ON a.region = b.region \
             GROUP BY a.region"
        )
        .contains("self-join"));
        assert!(err(
            "SELECT region, COUNT(*) FROM orders JOIN regions ON amount = name GROUP BY region"
        )
        .contains("same type"));
    }

    /// Spec §12.5: everything outside the subset is a hard error that names
    /// what it rejected.
    #[test]
    fn everything_outside_the_subset_is_rejected_by_name() {
        let cases = [
            ("SELECT region, COUNT(*) FROM orders GROUP BY region; SELECT 1", "exactly one SELECT"),
            ("INSERT INTO orders VALUES ('a', 1)", "exactly one SELECT"),
            ("WITH x AS (SELECT 1) SELECT region, COUNT(*) FROM orders GROUP BY region", "WITH"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region ORDER BY region", "ORDER BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region LIMIT 1", "LIMIT"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region UNION SELECT region, COUNT(*) FROM orders GROUP BY region", "compound"),
            ("SELECT DISTINCT region, COUNT(*) FROM orders GROUP BY region", "DISTINCT"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region HAVING COUNT(*) > 1", "HAVING"),
            ("SELECT COUNT(*)", "without FROM"),
            ("SELECT region, COUNT(*) FROM orders, regions GROUP BY region", "comma-separated"),
            ("SELECT region, COUNT(*) FROM (SELECT * FROM orders) GROUP BY region", "subquery"),
            ("SELECT region, COUNT(*) FROM main.orders GROUP BY region", "schema prefix"),
            ("SELECT region, COUNT(*) FROM nope GROUP BY region", "no such table"),
            ("SELECT region, COUNT(*) FROM orders LEFT JOIN regions ON region = name GROUP BY region", "non-inner join"),
            ("SELECT region, COUNT(*) FROM orders CROSS JOIN regions GROUP BY region", "non-inner join"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions USING (region) GROUP BY region", "without ON"),
            ("SELECT region, COUNT(*) FROM orders NATURAL JOIN regions GROUP BY region", "join"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name AND manager = 'x' GROUP BY region", "join condition"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = 'x' GROUP BY region", "join condition"),
            ("SELECT region, COUNT(*) FROM orders o JOIN regions o ON o.region = o.name GROUP BY region", "both tables"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name JOIN regions r2 ON region = r2.name GROUP BY region", "more than two tables"),
            ("SELECT * FROM orders GROUP BY region", "SELECT *"),
            ("SELECT region, amount + 1, COUNT(*) FROM orders GROUP BY region", "SELECT expression"),
            ("SELECT region, COUNT(amount) FROM orders GROUP BY region", "COUNT(*) only"),
            ("SELECT region, COUNT(DISTINCT amount) FROM orders GROUP BY region", "DISTINCT"),
            ("SELECT region, SUM(amount + 1) FROM orders GROUP BY region", "SUM over"),
            ("SELECT region, MIN(amount) FROM orders GROUP BY region", "MIN"),
            ("SELECT region, AVG(amount) FROM orders GROUP BY region", "AVG"),
            ("SELECT region, SUM(amount) OVER () FROM orders GROUP BY region", "window"),
            ("SELECT COUNT(*), region FROM orders GROUP BY region", "after an aggregate"),
            ("SELECT region, region, COUNT(*) FROM orders GROUP BY region", "twice in the SELECT list"),
            ("SELECT region, COUNT(*) AS region FROM orders GROUP BY region", "named region"),
            ("SELECT region, amount, COUNT(*) FROM orders GROUP BY region", "neither in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region, amount", "not in the SELECT list"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region, region", "twice in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY 1", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY lower(region)", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1 AND amount < 5 GROUP BY region", "at most one predicate"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1 OR amount < 5 GROUP BY region", "OR in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE NOT amount > 1 GROUP BY region", "NOT in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE region LIKE 'a%' GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount IN (1, 2) GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount BETWEEN 1 AND 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > (SELECT 1) GROUP BY region", "operand"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > amount GROUP BY region", "two columns"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1.5 GROUP BY region", "64-bit integers"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 9223372036854775808 GROUP BY region", "64-bit integers"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount + 1 > 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE nope > 1 GROUP BY region", "no such column"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name WHERE x.region > 'a' GROUP BY region", "no such table"),
            ("SELECT region, COUNT(*) FROM orders o JOIN regions r ON o.region = r.name GROUP BY o.region, r.name", "not in the SELECT list"),
            ("SELECT k, COUNT(*) FROM orders JOIN regions ON region = name GROUP BY k", "no such column"),
        ];
        for (sql, expected) in cases {
            let e = err(sql);
            assert!(
                e.contains(expected),
                "{sql}\n  error: {e}\n  expected it to mention: {expected}"
            );
        }
    }

    #[test]
    fn an_ambiguous_unqualified_column_is_rejected() {
        let db = Database::new(vec![
            db().tables()[0].clone(),
            Schema {
                table: "others".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let e = compile(
            "SELECT region, COUNT(*) FROM orders JOIN others ON orders.region = others.region GROUP BY region",
            &db,
        )
        .expect_err("region is in both tables");
        assert!(e.0.contains("ambiguous"), "{}", e.0);
    }

    #[test]
    fn a_parse_error_is_reported_as_such() {
        assert!(err("SELEC region FROM orders").contains("cannot parse"));
    }
}
```

- [ ] **Step 5: The compiler**

Replace the stub with the implementation — this is the whole file above the test module:

```rust
use ivmlite_core::{
    lower, Agg, AggFn, CmpOp, Column, ColumnType, Plan, PlanError, Predicate, ResolvedJoin,
    ResolvedView, Schema, Value,
};
use sqlparser::ast::{
    BinaryOperator, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgumentList,
    FunctionArguments, GroupByExpr, Ident, Join, JoinConstraint, JoinOperator, ObjectName,
    ObjectNamePart, Query, Select, SelectFlavor, SelectItem, SetExpr, Spanned, Statement,
    TableAlias, TableFactor, TableWithJoins, UnaryOperator, Value as SqlValue,
};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::source::{call_text, offset};
use crate::{Catalog, CatalogError};

/// A view's SQL, compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledView {
    pub plan: Plan,
    /// The view's output columns in SELECT order — its group-by columns, then
    /// its aggregates — with the names SQLite would give them.
    pub columns: Vec<Column>,
    /// The base tables the view reads, the anchor (the FROM table) first:
    /// the view's rows in `__ivm_dep` (spec §7).
    pub tables: Vec<String>,
}

/// Why a view's SQL was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlError(pub String);

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SqlError {}

impl From<PlanError> for SqlError {
    fn from(e: PlanError) -> Self {
        SqlError(e.0)
    }
}

impl From<CatalogError> for SqlError {
    fn from(e: CatalogError) -> Self {
        SqlError(e.0)
    }
}

fn unsupported(what: &str) -> SqlError {
    SqlError(format!(
        "{what} is not supported in an ivmlite v0 view (spec §13)"
    ))
}

fn reject_if(present: bool, what: &str) -> Result<(), SqlError> {
    if present {
        Err(unsupported(what))
    } else {
        Ok(())
    }
}

/// Compile a view's `SELECT` against `catalog`.
///
/// v0's subset (spec §5.2, §6.1, §13):
///
/// ```text
/// SELECT <column>, …, <aggregate>, …
/// FROM <table> [[AS] <alias>]
///      [[INNER] JOIN <table> [[AS] <alias>] ON <column> = <column>]
/// [WHERE <column> <op> <literal> | <literal> <op> <column>
///      | <column> IS NULL | <column> IS NOT NULL]
/// GROUP BY <column>, …
/// ```
///
/// where the SELECT list's bare columns are exactly the GROUP BY columns and
/// come before the aggregates, an aggregate is `COUNT(*)` or `SUM(<column>)`,
/// `<op>` is one of `>` `>=` `<` `<=` `=` `!=` `<>`, and a literal is an
/// integer or a single-quoted string. Anything else is an error.
pub fn compile(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError> {
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| SqlError(format!("cannot parse the view's SQL: {e}")))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(unsupported("anything but exactly one SELECT statement"));
    };
    let select = plain_select(query)?;
    let (anchor, join) = from_clause(select, catalog)?;
    let scope = Scope::new(&anchor, join.as_ref().map(|j| &j.table));
    let join = match &join {
        None => None,
        Some(j) => Some(join_on(&scope, j)?),
    };
    let outputs = projection(sql, &scope, &select.projection)?;
    let group_by = group_by(&scope, &select.group_by, &outputs)?;
    let predicate = match &select.selection {
        None => Predicate::None,
        Some(e) => predicate(&scope, e)?,
    };
    let aggs = outputs
        .iter()
        .filter_map(|o| match &o.item {
            Output::Agg(agg) => Some(agg.clone()),
            Output::Column(_) => None,
        })
        .collect();
    let plan = lower(&ResolvedView {
        anchor: &anchor.schema,
        join,
        group_by,
        aggs,
        predicate,
    })?;
    let columns = outputs.iter().map(|o| o.column(&scope)).collect();
    let mut tables = vec![anchor.schema.table.clone()];
    tables.extend(scope.right.map(|r| r.schema.table.clone()));
    Ok(CompiledView {
        plan,
        columns,
        tables,
    })
}

/// A table in the FROM clause, and the name the query uses for it.
struct Source {
    schema: Schema,
    /// The alias if there is one, else the table's name as written.
    qualifier: String,
}

/// The JOIN clause, before its ON condition is resolved.
struct JoinClause<'a> {
    table: Source,
    on: &'a Expr,
}

/// The tables a query's column names resolve against. Column `i` of the
/// joined row is the anchor's column `i`, then the right table's.
struct Scope<'a> {
    anchor: &'a Source,
    right: Option<&'a Source>,
}

impl<'a> Scope<'a> {
    fn new(anchor: &'a Source, right: Option<&'a Source>) -> Self {
        Scope { anchor, right }
    }

    fn sources(&self) -> impl Iterator<Item = (&'a Source, usize)> {
        let offset = self.anchor.schema.arity();
        std::iter::once((self.anchor, 0)).chain(self.right.map(|r| (r, offset)))
    }

    fn column_at(&self, index: usize) -> &'a Column {
        let n = self.anchor.schema.arity();
        if index < n {
            &self.anchor.schema.columns[index]
        } else {
            &self
                .right
                .expect("an index past the anchor's columns belongs to the right table")
                .schema
                .columns[index - n]
        }
    }

    /// Resolve a column reference to its index in the joined row.
    fn column(&self, expr: &Expr) -> Result<Option<usize>, SqlError> {
        match strip_parens(expr) {
            Expr::Identifier(name) => {
                let mut found = self
                    .sources()
                    .filter_map(|(s, offset)| position(&s.schema, &name.value).map(|i| offset + i));
                match (found.next(), found.next()) {
                    (Some(i), None) => Ok(Some(i)),
                    (None, _) => Err(SqlError(format!("no such column: {}", name.value))),
                    (Some(_), Some(_)) => Err(SqlError(format!(
                        "ambiguous column name: {} — qualify it with its table",
                        name.value
                    ))),
                }
            }
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [table, name] => {
                    let (source, offset) = self
                        .sources()
                        .find(|(s, _)| s.qualifier.eq_ignore_ascii_case(&table.value))
                        .ok_or_else(|| SqlError(format!("no such table: {}", table.value)))?;
                    let i = position(&source.schema, &name.value).ok_or_else(|| {
                        SqlError(format!("no such column: {}.{}", table.value, name.value))
                    })?;
                    Ok(Some(offset + i))
                }
                _ => Err(unsupported("a column name with a schema prefix")),
            },
            _ => Ok(None),
        }
    }

    /// Like `column`, but anything other than a column reference is an error.
    fn require_column(&self, expr: &Expr, context: &str) -> Result<usize, SqlError> {
        self.column(expr)?.ok_or_else(|| {
            unsupported(&format!(
                "{context} `{expr}` (only a bare column is allowed there)"
            ))
        })
    }
}

fn position(schema: &Schema, name: &str) -> Option<usize> {
    schema
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(name))
}

fn strip_parens(mut expr: &Expr) -> &Expr {
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    expr
}

/// The query's single SELECT, with every clause v0 does not support rejected.
///
/// `Query` and `Select` are destructured without `..`, so a field a future
/// `sqlparser` adds is a compile error here rather than a clause that is
/// silently ignored.
fn plain_select(query: &Query) -> Result<&Select, SqlError> {
    let Query {
        with,
        body,
        order_by,
        limit_clause,
        fetch,
        locks,
        for_clause,
        settings,
        format_clause,
        pipe_operators,
    } = query;
    reject_if(with.is_some(), "WITH")?;
    reject_if(order_by.is_some(), "ORDER BY")?;
    reject_if(limit_clause.is_some() || fetch.is_some(), "LIMIT")?;
    reject_if(
        !locks.is_empty()
            || for_clause.is_some()
            || settings.is_some()
            || format_clause.is_some()
            || !pipe_operators.is_empty(),
        "a non-SQLite query clause",
    )?;
    let SetExpr::Select(select) = body.as_ref() else {
        return Err(unsupported(
            "a compound or parenthesized query (UNION, INTERSECT, EXCEPT, VALUES)",
        ));
    };
    let Select {
        select_token: _,
        optimizer_hints,
        distinct,
        select_modifiers,
        top,
        top_before_distinct: _,
        projection: _,
        exclude,
        into,
        from: _,
        lateral_views,
        prewhere,
        selection: _,
        connect_by,
        group_by: _,
        cluster_by,
        distribute_by,
        sort_by,
        having,
        named_window,
        qualify,
        window_before_qualify: _,
        value_table_mode,
        flavor,
    } = select.as_ref();
    reject_if(distinct.is_some(), "DISTINCT")?;
    reject_if(having.is_some(), "HAVING")?;
    reject_if(!named_window.is_empty(), "WINDOW")?;
    reject_if(
        !optimizer_hints.is_empty()
            || select_modifiers.is_some()
            || top.is_some()
            || exclude.is_some()
            || into.is_some()
            || !lateral_views.is_empty()
            || prewhere.is_some()
            || !connect_by.is_empty()
            || !cluster_by.is_empty()
            || !distribute_by.is_empty()
            || !sort_by.is_empty()
            || qualify.is_some()
            || value_table_mode.is_some()
            || !matches!(flavor, SelectFlavor::Standard),
        "a non-SQLite SELECT clause",
    )?;
    Ok(select)
}

/// The FROM clause: the anchor table and, optionally, one inner join.
fn from_clause<'a>(
    select: &'a Select,
    catalog: &dyn Catalog,
) -> Result<(Source, Option<JoinClause<'a>>), SqlError> {
    let [TableWithJoins { relation, joins }] = select.from.as_slice() else {
        return Err(if select.from.is_empty() {
            unsupported("a SELECT without FROM")
        } else {
            unsupported("a comma-separated FROM list (write JOIN … ON instead)")
        });
    };
    let anchor = source(relation, catalog)?;
    let join = match joins.as_slice() {
        [] => None,
        [Join {
            relation,
            global: _,
            join_operator,
        }] => {
            let constraint =
                match join_operator {
                    JoinOperator::Join(c) | JoinOperator::Inner(c) => c,
                    _ => return Err(unsupported(
                        "an outer, cross or other non-inner join (v0 joins with INNER JOIN … ON)",
                    )),
                };
            let JoinConstraint::On(on) = constraint else {
                return Err(unsupported("a join without ON (USING, NATURAL, or none)"));
            };
            let table = source(relation, catalog)?;
            if table.qualifier.eq_ignore_ascii_case(&anchor.qualifier) {
                return Err(SqlError(format!(
                    "the name {} is used for both tables of the join; give them different aliases",
                    table.qualifier
                )));
            }
            Some(JoinClause { table, on })
        }
        _ => return Err(unsupported("a join of more than two tables")),
    };
    Ok((anchor, join))
}

fn source(relation: &TableFactor, catalog: &dyn Catalog) -> Result<Source, SqlError> {
    let TableFactor::Table {
        name,
        alias,
        args,
        with_hints,
        version,
        with_ordinality,
        partitions,
        json_path,
        sample,
        index_hints,
    } = relation
    else {
        return Err(unsupported("a subquery or table function in FROM"));
    };
    reject_if(
        args.is_some()
            || !with_hints.is_empty()
            || version.is_some()
            || *with_ordinality
            || !partitions.is_empty()
            || json_path.is_some()
            || sample.is_some()
            || !index_hints.is_empty(),
        "a table modifier in FROM",
    )?;
    let table_name = single_ident(name)?;
    let schema = catalog
        .table(&table_name.value)?
        .ok_or_else(|| SqlError(format!("no such table: {}", table_name.value)))?;
    let qualifier = match alias {
        None => table_name.value.clone(),
        Some(TableAlias {
            explicit: _,
            name,
            columns,
            at,
        }) => {
            reject_if(
                !columns.is_empty() || at.is_some(),
                "a table alias with a column list",
            )?;
            name.value.clone()
        }
    };
    Ok(Source { schema, qualifier })
}

fn single_ident(name: &ObjectName) -> Result<&Ident, SqlError> {
    match name.0.as_slice() {
        [ObjectNamePart::Identifier(ident)] => Ok(ident),
        _ => Err(unsupported("a table name with a schema prefix")),
    }
}

/// `ON <column> = <column>`, one column from each table, in either order.
fn join_on<'a>(scope: &Scope<'a>, join: &JoinClause) -> Result<ResolvedJoin<'a>, SqlError> {
    let right = scope.right.expect("a join has a right table");
    let not_supported = || {
        unsupported(&format!(
            "the join condition `{}` (v0 joins on one equality between a column of each table)",
            join.on
        ))
    };
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right: other,
    } = strip_parens(join.on)
    else {
        return Err(not_supported());
    };
    let (Some(a), Some(b)) = (scope.column(left)?, scope.column(other)?) else {
        return Err(not_supported());
    };
    let n = scope.anchor.schema.arity();
    let (left_column, right_column) = match (a < n, b < n) {
        (true, false) => (a, b - n),
        (false, true) => (b, a - n),
        _ => return Err(not_supported()),
    };
    Ok(ResolvedJoin {
        right: &right.schema,
        left_column,
        right_column,
    })
}

enum Output {
    Column(usize),
    Agg(Agg),
}

/// One SELECT-list entry and the name SQLite gives it.
struct Named {
    item: Output,
    name: String,
}

impl Named {
    fn column(&self, scope: &Scope) -> Column {
        match &self.item {
            &Output::Column(i) => Column {
                name: self.name.clone(),
                ..scope.column_at(i).clone()
            },
            Output::Agg(agg) => Column {
                name: self.name.clone(),
                ty: ColumnType::Integer,
                // SUM over only NULLs is NULL (spec §6.1); COUNT(*) is never NULL.
                nullable: agg.func == AggFn::Sum,
            },
        }
    }
}

/// The SELECT list: bare columns first, then aggregates, each named the way
/// SQLite names a result column — its alias, else a bare column's declared
/// name, else an aggregate's text as written.
fn projection(sql: &str, scope: &Scope, items: &[SelectItem]) -> Result<Vec<Named>, SqlError> {
    let mut out: Vec<Named> = Vec::new();
    for item in items {
        let (expr, alias) = match item {
            SelectItem::UnnamedExpr(expr) => (expr, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias)),
            _ => return Err(unsupported("SELECT * and multi-name aliases")),
        };
        let (item, default_name) = match scope.column(expr)? {
            Some(i) => {
                if out.iter().any(|o| matches!(o.item, Output::Agg(_))) {
                    return Err(unsupported(
                        "a column after an aggregate in the SELECT list (list the GROUP BY \
                         columns first, then the aggregates)",
                    ));
                }
                if out
                    .iter()
                    .any(|o| matches!(o.item, Output::Column(c) if c == i))
                {
                    return Err(SqlError(format!(
                        "column {} appears twice in the SELECT list",
                        scope.column_at(i).name
                    )));
                }
                (Output::Column(i), scope.column_at(i).name.clone())
            }
            None => {
                let Expr::Function(call) = strip_parens(expr) else {
                    return Err(unsupported(&format!(
                        "the SELECT expression `{expr}` (v0 selects bare columns, COUNT(*) and SUM(column))"
                    )));
                };
                let start = offset(sql, expr.span().start);
                (
                    Output::Agg(aggregate(scope, call)?),
                    call_text(sql, start).to_string(),
                )
            }
        };
        let name = alias.map_or(default_name, |a| a.value.clone());
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
            return Err(SqlError(format!(
                "two result columns are named {name}; give one an alias"
            )));
        }
        out.push(Named { item, name });
    }
    Ok(out)
}

/// `COUNT(*)` or `SUM(<column>)`.
fn aggregate(scope: &Scope, call: &Function) -> Result<Agg, SqlError> {
    let Function {
        name,
        uses_odbc_syntax,
        parameters,
        args,
        within_group,
        filter,
        null_treatment,
        over,
    } = call;
    reject_if(over.is_some(), "a window function (OVER)")?;
    reject_if(filter.is_some(), "an aggregate FILTER clause")?;
    reject_if(
        *uses_odbc_syntax
            || !matches!(parameters, FunctionArguments::None)
            || !within_group.is_empty()
            || null_treatment.is_some(),
        "this function call syntax",
    )?;
    let func_name = &single_ident(name)?.value;
    let FunctionArguments::List(FunctionArgumentList {
        duplicate_treatment,
        args,
        clauses,
    }) = args
    else {
        return Err(unsupported(&format!("the call `{call}`")));
    };
    reject_if(
        duplicate_treatment.is_some(),
        "DISTINCT or ALL inside an aggregate",
    )?;
    reject_if(
        !clauses.is_empty(),
        "ORDER BY or other clauses inside an aggregate",
    )?;
    if func_name.eq_ignore_ascii_case("count") {
        match args.as_slice() {
            [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] => Ok(Agg {
                func: AggFn::Count,
                column: None,
            }),
            _ => Err(unsupported(&format!(
                "`{call}` (v0 supports COUNT(*) only; COUNT(column) skips NULLs)"
            ))),
        }
    } else if func_name.eq_ignore_ascii_case("sum") {
        match args.as_slice() {
            [FunctionArg::Unnamed(FunctionArgExpr::Expr(e))] => Ok(Agg {
                func: AggFn::Sum,
                column: Some(scope.require_column(e, "SUM over")?),
            }),
            _ => Err(unsupported(&format!("`{call}`"))),
        }
    } else {
        Err(unsupported(&format!(
            "the function `{func_name}` (v0's aggregates are COUNT(*) and SUM; MIN, MAX, AVG \
             and the rest are not)"
        )))
    }
}

/// GROUP BY: bare columns only, each once, and exactly the SELECT list's bare
/// columns. Returned in SELECT order, which is the output order.
fn group_by(
    scope: &Scope,
    clause: &GroupByExpr,
    outputs: &[Named],
) -> Result<Vec<usize>, SqlError> {
    let GroupByExpr::Expressions(exprs, modifiers) = clause else {
        return Err(unsupported("GROUP BY ALL"));
    };
    reject_if(
        !modifiers.is_empty(),
        "GROUP BY modifiers (ROLLUP, CUBE, …)",
    )?;
    let mut keys = Vec::new();
    for e in exprs {
        let i = scope.require_column(e, "the GROUP BY term")?;
        if keys.contains(&i) {
            return Err(SqlError(format!(
                "column {} appears twice in GROUP BY",
                scope.column_at(i).name
            )));
        }
        keys.push(i);
    }
    let selected: Vec<usize> = outputs
        .iter()
        .filter_map(|o| match o.item {
            Output::Column(i) => Some(i),
            Output::Agg(_) => None,
        })
        .collect();
    if let Some(&i) = keys.iter().find(|i| !selected.contains(i)) {
        return Err(SqlError(format!(
            "column {} is in GROUP BY but not in the SELECT list; v0 needs every group key in \
             the view's output, or two groups could produce the same row (spec §5.2)",
            scope.column_at(i).name
        )));
    }
    if let Some(&i) = selected.iter().find(|i| !keys.contains(i)) {
        return Err(SqlError(format!(
            "column {} is in the SELECT list but neither in GROUP BY nor aggregated",
            scope.column_at(i).name
        )));
    }
    Ok(selected)
}

/// WHERE: one comparison between a column and a literal, or IS [NOT] NULL
/// (spec §6.1).
fn predicate(scope: &Scope, expr: &Expr) -> Result<Predicate, SqlError> {
    let not_supported = || {
        unsupported(&format!(
            "the WHERE clause `{expr}` (a v0 view filters on one comparison between a column \
             and a literal, or on IS [NOT] NULL)"
        ))
    };
    match strip_parens(expr) {
        Expr::IsNull(e) => Ok(Predicate::IsNull {
            column: scope.require_column(e, "IS NULL over")?,
        }),
        Expr::IsNotNull(e) => Ok(Predicate::IsNotNull {
            column: scope.require_column(e, "IS NOT NULL over")?,
        }),
        Expr::BinaryOp {
            op: BinaryOperator::And,
            ..
        } => Err(unsupported(
            "AND in WHERE (a v0 view has at most one predicate)",
        )),
        Expr::BinaryOp {
            op: BinaryOperator::Or,
            ..
        } => Err(unsupported("OR in WHERE")),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            ..
        } => Err(unsupported("NOT in WHERE")),
        Expr::BinaryOp { left, op, right } => {
            let op = cmp_op(op).ok_or_else(not_supported)?;
            match (scope.column(left)?, scope.column(right)?) {
                (Some(column), None) => Ok(Predicate::Compare {
                    column,
                    op,
                    value: literal(right)?,
                }),
                (None, Some(column)) => Ok(Predicate::Compare {
                    column,
                    op: flip(op),
                    value: literal(left)?,
                }),
                (Some(_), Some(_)) => Err(unsupported(
                    "a comparison between two columns (v0 compares a column with a literal)",
                )),
                (None, None) => Err(not_supported()),
            }
        }
        _ => Err(not_supported()),
    }
}

fn cmp_op(op: &BinaryOperator) -> Option<CmpOp> {
    match op {
        BinaryOperator::Gt => Some(CmpOp::Gt),
        BinaryOperator::GtEq => Some(CmpOp::Ge),
        BinaryOperator::Lt => Some(CmpOp::Lt),
        BinaryOperator::LtEq => Some(CmpOp::Le),
        BinaryOperator::Eq => Some(CmpOp::Eq),
        BinaryOperator::NotEq => Some(CmpOp::Ne),
        _ => None,
    }
}

/// The operator that keeps a comparison's meaning when its operands swap
/// sides: `3 < v` is `v > 3`.
fn flip(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Ge => CmpOp::Le,
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Eq => CmpOp::Eq,
        CmpOp::Ne => CmpOp::Ne,
    }
}

/// An integer, a single-quoted string, or NULL (which `lower` rejects with a
/// pointer to IS NULL).
fn literal(expr: &Expr) -> Result<Value, SqlError> {
    let integer = |text: &str| {
        text.parse::<i64>().map(Value::Int).map_err(|_| {
            unsupported(&format!(
                "the literal {text} (v0's numeric literals are 64-bit integers)"
            ))
        })
    };
    match strip_parens(expr) {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(text, _) => integer(text),
            SqlValue::SingleQuotedString(s) => Ok(Value::Text(s.clone())),
            SqlValue::Null => Ok(Value::Null),
            _ => Err(unsupported(&format!("the literal `{expr}`"))),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => match strip_parens(inner) {
            Expr::Value(v) => match &v.value {
                SqlValue::Number(text, _) => integer(&format!("-{text}")),
                _ => Err(unsupported(&format!("the literal `{expr}`"))),
            },
            _ => Err(unsupported(&format!("the operand `{expr}`"))),
        },
        _ => Err(unsupported(&format!(
            "the operand `{expr}` (a comparison's other side must be a literal)"
        ))),
    }
}
```

Run `cargo test -p ivmlite-sql --locked`: 18 passed. Then the full suite (255 passed / 0 failed: 237 + 18), fmt, clippy.

- [ ] **Step 6: Register the gate rows**

Add a new section `## ivmlite-sql` to `docs/mutation-gates.md`, after `## ivmlite-core`, with these rows. Each mutation was run while this plan was prototyped; the red tests below are what it showed **before** Task 3's tests existed — record what you observe.

| Spec requirement | Mutation (in `crates/ivmlite-sql/src/`) | Test that should go red |
|---|---|---|
| Ruling 2: a literal on the left flips the operator | In `compile.rs`'s `flip`, map `CmpOp::Gt` to `CmpOp::Le` | `a_literal_on_the_left_flips_the_operator` |
| Ruling 2: `ON` resolves in either order, each key to its own table | In `join_on`, change the `(false, true)` arm to `(a - n, b)` | `a_join_with_aliases_and_a_reversed_on_compiles` |
| Ruling 2: the output order of group keys is the SELECT order | At the end of `group_by`, return `Ok(keys)` instead of `Ok(selected)` | `the_select_order_of_group_keys_is_the_output_order` |
| Ruling 2: a column reference into the right table is offset by the anchor's arity | In `Scope::sources`, give the right table offset `0` | several `compile::tests` (the prototype saw five) |
| Ruling 2 / §5.2: every group key must be in the SELECT list | In `group_by`, make the "in GROUP BY but not in the SELECT list" check never fire (`.find(\|_\| false)`) | `everything_outside_the_subset_is_rejected_by_name` |
| Ruling 2: bare columns come before aggregates | In `projection`, make the "column after an aggregate" check `if false` | `everything_outside_the_subset_is_rejected_by_name` |
| Ruling 3: result column names are unique | In `projection`, delete the duplicate-name check | `everything_outside_the_subset_is_rejected_by_name` |
| Ruling 1 / §6.1: at most one predicate — `AND` is rejected by name | In `predicate`, delete the `BinaryOperator::And` arm (it then falls to the generic WHERE error) | `everything_outside_the_subset_is_rejected_by_name` |
| §13: only inner joins | In `from_clause`, accept `JoinOperator::LeftOuter(c)` alongside `Join` / `Inner` | `everything_outside_the_subset_is_rejected_by_name` |
| §13: ORDER BY is rejected | In `plain_select`, delete the `reject_if(order_by.is_some(), …)` | `everything_outside_the_subset_is_rejected_by_name` |
| Ruling 3: an unaliased aggregate is named by its text through the `)` | In `source.rs`'s `call_text`, return `&sql[start..start + i]` (dropping the `)`) | `call_text_runs_to_the_matching_parenthesis`, `compiles_the_spec_example` |
| Ruling 4: `SUM`'s output column is nullable | In `Named::column`, make every aggregate `nullable: false` | `compiles_the_spec_example` |
| Ruling 4 / SQLite: table names match case-insensitively and keep their declared spelling | In `catalog.rs`, compare with `s.table == name` | `a_database_finds_tables_case_insensitively_and_keeps_the_declared_name`, `names_resolve_case_insensitively_and_keep_their_declared_spelling` |

Also one n/a row: requirement "Ruling 5: a field a future `sqlparser` adds to `Query`, `Select`, `TableFactor::Table`, `Join`, `TableAlias` or `Function` cannot be silently ignored"; mutation "add `..` to one of the destructuring patterns"; Verified `None — the guard is the compiler, not a test: with every field named, an added field fails to compile; adding `..` changes no behaviour today, so no test can go red`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/ivmlite-sql docs/mutation-gates.md
git commit -m "feat(sql): compile a view's SELECT into a Plan through lower

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The front end against the harness's query space

**Files:**
- Modify: `crates/ivmlite-test/Cargo.toml`, `crates/ivmlite-test/src/sql.rs`
- Create: `crates/ivmlite-test/tests/sql_front_end.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ivmlite_sql::compile` (Task 2), `ivmlite_core::lower_query` (Task 1), and the harness's `enumerate`, `enumerate_join`, `gen_database`, `gen_database_with_swapped_right_table`, `view_query_to_sql`, `create_table_sql`.

- [ ] **Step 1: The dev-dependency and the failing tests**

Append to `crates/ivmlite-test/Cargo.toml`:

```toml

[dev-dependencies]
ivmlite-sql.workspace = true
```

Create `crates/ivmlite-test/tests/sql_front_end.rs`:

```rust
//! M1b Phase 2b: the SQL front end against the harness's query space.

use ivmlite_core::lower_query;
use ivmlite_sql::compile;
use ivmlite_test::{
    create_table_sql, enumerate, enumerate_join, gen_database,
    gen_database_with_swapped_right_table, view_query_to_sql, Database, ViewQuery,
};

fn every_query(db: &Database) -> Vec<ViewQuery> {
    let mut qs = enumerate(&db.tables()[0]);
    qs.extend(enumerate_join(&db.tables()[0], &db.tables()[1]));
    qs
}

fn databases() -> [Database; 2] {
    [gen_database(2), gen_database_with_swapped_right_table()]
}

/// The SQL a `ViewQuery` renders to compiles to the very `Plan` `lower_query`
/// builds from the `ViewQuery` itself — so every query the differential
/// harness verifies through `lower_query` is verified through SQL too.
#[test]
fn every_enumerated_query_compiles_to_the_plan_lower_query_builds() {
    for db in databases() {
        let queries = every_query(&db);
        assert_eq!(queries.len(), 153 + 900);
        for q in queries {
            let sql = view_query_to_sql(&q, &db);
            let compiled = compile(&sql, &db).unwrap_or_else(|e| panic!("{sql}: {e}"));
            assert_eq!(
                compiled.plan,
                lower_query(&q, &db).expect("an enumerated query lowers"),
                "{sql}"
            );
            let mut tables = vec![db.tables()[0].table.clone()];
            if q.join.is_some() {
                tables.push(db.tables()[1].table.clone());
            }
            assert_eq!(compiled.tables, tables, "{sql}");
        }
    }
}

/// Unaliased result columns get the names SQLite gives them (measured with
/// SQLite 3.53 through rusqlite): a bare column its declared name, an
/// aggregate its text exactly as written.
#[test]
fn result_column_names_match_sqlite() {
    let db = gen_database(2);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    for t in db.tables() {
        conn.execute(&create_table_sql(t), []).unwrap();
    }
    let mut sqls: Vec<String> = every_query(&db)
        .iter()
        .map(|q| view_query_to_sql(q, &db))
        .collect();
    sqls.extend(
        [
            "SELECT K, count( * ), Sum(V) FROM T0 GROUP BY k",
            "SELECT t0.k AS region, sum(t0.v) total FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t0.k",
            "SELECT a.k, SUM ( b.\"v\" ) FROM t0 a JOIN t1 AS b ON b.k = a.k GROUP BY a.k",
            "SELECT\n  k,\n  COUNT(*)\nFROM t0\nGROUP BY k",
        ]
        .map(String::from),
    );
    for sql in sqls {
        let expected: Vec<String> = conn
            .prepare(&sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .column_names()
            .into_iter()
            .map(String::from)
            .collect();
        let compiled = compile(&sql, &db).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let names: Vec<String> = compiled.columns.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, expected, "{sql}");
    }
}
```

Run `cargo test -p ivmlite-test --locked --test sql_front_end`. Both tests fail on the first join query grouped by `t0.k` and `t1.k`: "two result columns are named k; give one an alias". That is the RED this task's Step 2 fixes.

- [ ] **Step 2: Alias a group key whose name is taken**

In `crates/ivmlite-test/src/sql.rs`'s `view_query_to_sql`, replace the line `let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();` with:

```rust
    // A result column's name must be unique — `ivmlite-sql` rejects a view
    // whose output repeats one, as SQLite rejects a table with two columns of
    // one name — so a group key whose column name an earlier key already
    // took (a join grouped by `t0.k` and `t1.k`) is aliased `<table>_<column>`.
    let mut taken: Vec<&str> = Vec::new();
    let mut select: Vec<String> = Vec::new();
    for &i in &query.group_by {
        let (table, column) = columns[i];
        if taken.iter().any(|t| t.eq_ignore_ascii_case(column)) {
            select.push(format!("{} AS \"{table}_{column}\"", name(i)));
        } else {
            select.push(name(i));
            taken.push(column);
        }
    }
```

and add to its test module:

```rust
    /// A join grouped by `t0.k` and `t1.k` would name two result columns `k`,
    /// which `ivmlite-sql` rejects, so the second is aliased.
    #[test]
    fn to_sql_aliases_a_group_key_whose_name_is_taken() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let q = ViewQuery {
            group_by: vec![0, 2],
            ..join_on_k()
        };
        let sql = view_query_to_sql(&q, &db);
        assert!(
            sql.starts_with("SELECT \"t0\".\"k\", \"t1\".\"k\" AS \"t1_k\", "),
            "{sql}"
        );
        assert!(
            sql.ends_with("GROUP BY \"t0\".\"k\", \"t1\".\"k\""),
            "the GROUP BY clause names the column, not the alias: {sql}"
        );
    }
```

The oracle compares result rows by position, not by name, so the alias changes nothing it checks.

- [ ] **Step 3: Run everything**

`cargo test -p ivmlite-test --locked --test sql_front_end`: 2 passed. Full suite: 258 passed / 0 failed. fmt, clippy.

- [ ] **Step 4: Register the gate rows**

Under `## ivmlite-sql`:

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| Ruling 6: every enumerated query compiles to the `Plan` `lower_query` builds | In `compile.rs`'s `Scope::sources`, give the right table offset `0` | `every_enumerated_query_compiles_to_the_plan_lower_query_builds` and `result_column_names_match_sqlite`, plus the unit tests of Task 2's row for the same mutation |
| Ruling 3: result column names match SQLite's | In `source.rs`'s `call_text`, drop the `)` as in Task 2's row | `result_column_names_match_sqlite`, plus Task 2's two |

Under `## ivmlite-test: semantic contracts`:

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| Ruling 3: the rendered SQL never names two result columns alike | In `view_query_to_sql`, make the collision test `false && …` | `to_sql_aliases_a_group_key_whose_name_is_taken`, `every_enumerated_query_compiles_to_the_plan_lower_query_builds`, `result_column_names_match_sqlite` |

Then update Task 2's rows for the `Scope::sources` offset, `call_text` and catalog mutations: re-run them and record the red set now that these integration tests exist (the prototype saw `result_column_names_match_sqlite` join each). Merge the first row above into Task 2's offset row if the two would record the same mutation twice — one mutation, one row, one current measurement.

- [ ] **Step 5: Commit**

```bash
git add crates/ivmlite-test/Cargo.toml crates/ivmlite-test/src/sql.rs crates/ivmlite-test/tests/sql_front_end.rs docs/mutation-gates.md
git commit -m "test(harness): every enumerated query compiles through SQL to the same Plan

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The spec

**Files:**
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, `docs/README.md`

- [ ] **Step 1: §4.3 `ivmlite-sql`**

Under the `ivmlite-sql` bullets, add a paragraph "**Implemented (M1b Phase 2b, 2026-09-24).**" stating:
- `compile(sql, &dyn Catalog) -> Result<CompiledView, SqlError>`, where `CompiledView` carries the `Plan`, the output columns (name, type, nullability, SELECT order), and the base tables with the anchor first.
- The accepted grammar, as in this plan's Ruling 2, including what is accepted beyond the harness's rendered SQL (aliases, unqualified columns, literals on the left, `<>`, parentheses, `INNER`).
- The naming rule of Ruling 3, and that duplicate output names are rejected.
- The Catalog of Ruling 4, which is fallible so that the SQLite catalog can reject non-STRICT tables, `ANY` columns and `COLLATE` (§7.1).
- That legality lives in `ivmlite_core::lower` alone (Ruling 1), and that the harness's `ViewQuery` is resolved by `lower_query`.
- The equivalence evidence of Ruling 6, naming `every_enumerated_query_compiles_to_the_plan_lower_query_builds`.

- [ ] **Step 2: §9.2 — close the three-column question**

At the end of the "open question" block about widening to three columns, add "**Decided (M1b Phase 2b, 2026-09-24): not widening.**" followed by Ruling 7's argument, with its numbers: 12285 join queries per database and about 56 seconds per sweep (extrapolated, as recorded in the Phase 2a paragraph above). Do not delete the open question's text; it is the record of what was weighed.

- [ ] **Step 3: §13**

- Item 9 (joins): add "written `FROM a [AS x] [INNER] JOIN b [AS y] ON <column> = <column>`; table aliases are supported, but a self-join is rejected even with aliases".
- Add an item after item 10: "The SELECT list is the GROUP BY columns, each once, followed by the aggregates; GROUP BY names bare columns only (no positions like `GROUP BY 1`, no expressions); result column names must be unique; table names cannot be schema-qualified (`main.t`) (M1b Phase 2b)". Renumber the items after it.

- [ ] **Step 4: The docs index**

`docs/README.md`: after the Phase 2a entry, add
`  - [`2026-09-24-m1b-phase2b-sql-front-end.md`](superpowers/plans/2026-09-24-m1b-phase2b-sql-front-end.md) — M1b Phase 2b: the SQL front end`

- [ ] **Step 5: Verify and commit**

Full suite, fmt, clippy (docs-only, but the suite confirms nothing else moved).

```bash
git add docs/superpowers/specs/2026-09-18-ivmlite-design.md docs/README.md
git commit -m "docs(spec): record the SQL front end and close the three-column question

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage

| Spec requirement | Where |
|---|---|
| §4.1 / §4.3: `ivmlite-sql`, `sqlparser-rs` SQLite dialect → plan IR | Task 2 |
| §4.3: name resolution and type inference against a `Catalog` trait | Task 2 (`Catalog`, `Scope`, output column types) |
| §4.3 / §12.5: any query outside the subset is a hard error | Task 2 (`everything_outside_the_subset_is_rejected_by_name`, 52 cases) |
| §4.2: dependencies one way, core free of SQL crates | Global Constraints; Task 2's manifest |
| §5.2: root is an Aggregate with a non-empty GROUP BY; every group key in the output | Task 1 (`lower`), Task 2 (`group_by`) |
| §6.1: the predicate whitelist, literal types, NULL | Task 2 (`predicate`, `literal`) through `lower` |
| §8.3: the example view compiles | Task 2 (`compiles_the_spec_example`) |
| §9.2: the three-column question | Ruling 7, Task 4 |
| §13: v0's limitations as SQL | Task 4 |
