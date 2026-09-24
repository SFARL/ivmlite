# M1b Phase 2a: The Predicate Whitelist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the engine, the reference implementation, the oracle and the enumerated query space cover spec §6.1's whole comparison whitelist — `>` `>=` `<` `<=` `=` `!=` `IS NULL` `IS NOT NULL`, over INTEGER and TEXT columns — so that Phase 2b's SQL front end has a verified target for every predicate it can parse.

**Architecture:** `Predicate::IntGt { column, value: i64 }` becomes `Predicate::Compare { column, op: CmpOp, value: Value }`, and `Predicate::IsNull { column }` joins `IsNotNull`. `lower` checks that the literal's type matches the column's declared type (§6.1, "Operand types must match") and rejects a NULL literal. The engine's `passes`, `NaiveRecompute`'s `passes` and the oracle's SQL renderer each implement the new variants independently. The enumerator crosses every operator with every column on single tables, and a reduced set (one operator per column, rotating) on joins, so the join sweep grows by 29% rather than 4.7×.

**Tech Stack:** Rust 1.95, pure Rust with no `unsafe`, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md` (§6.1 "Three-valued predicate logic", "Operand types must match"; §9.2; §13 items 7 and 8)

**Preceding plan:** `docs/superpowers/plans/2026-09-24-m1b-phase1-state-ownership.md` (complete and merged into master, e098d2e)

## Global Constraints

- **English only** in everything committed: code, comments, error strings, docs, commit messages.
- `ivmlite-core` must not depend on `rusqlite` or `libsqlite3-sys` (spec §4.2). No `unsafe` anywhere in this plan.
- Every spec-mandated behaviour this plan adds gets a row in `docs/mutation-gates.md`, with the mutation actually run: break the code, confirm it compiles, run `cargo test --workspace --locked --no-fail-fast`, sum `passed`/`failed` over every test binary, restore. Record `N passed / M failed (baseline B/0)` read off the terminal, never computed. A row whose mutation cannot be run starts its Verified cell with `None — ` and says why. After editing the table, `python3 scripts/count-mutation-gates.py` must print `consistent`; if it does not, run it with `--fix` and check the diff.
- Before every commit: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked --no-fail-fast` all clean.
- Stage specific files, never `git add -A`. Never `--no-verify`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not loosen any existing test threshold (detection rates, shrink sizes, coverage counts) to make it pass. A seed-dependent test that breaks for a reason this plan does not explain is a stop-and-report, not a fix.

---

## This plan's scope and the rulings behind it

M1b is split into phases: 1, state ownership (done); **2a, the predicate whitelist (this plan)**; 2b, `ivmlite-sql`; 3, `ivmlite-sqlite`; 4, the benchmark.

Spec §13 item 7 promises eight comparison forms, but `Predicate` has only `IntGt` and `IsNotNull`, and the enumerator, the reference implementation and the oracle cover only those two. Phase 2b's parser must lower every whitelisted form to something the engine runs correctly, so the whitelist is widened and differentially verified first, in core and the harness, before any parser exists. A predicate bug then bisects to this plan, not to the parser.

### Ruling 1: one comparison per view, the literal on the right

A view has at most one predicate: `None`, one `Compare`, one `IsNull` or one `IsNotNull`. `AND` is not supported — the spec's whitelist is silent on it, and each conjunction would multiply the enumerated space. The spec records this in §13 (Task 3). In `ViewQuery` and `Plan` the literal is always the right operand; Phase 2b normalizes `3 < v` to `v > 3` when it parses.

### Ruling 2: a literal must have its column's type, and NULL is not a literal

`Compare { value }` must be `Value::Int` for an INTEGER column and `Value::Text` for a TEXT column; `lower` rejects anything else. This extends §6.1's "Operand types must match" (written for `IntGt` over TEXT) to all six operators and both types: SQLite applies column affinity to a mismatched literal (a TEXT column compared with `3` compares with `'3'`), which v0 does not implement. A `Value::Null` literal is rejected with a message pointing at `IS NULL`: `v = NULL` is UNKNOWN for every row, so the view would always be empty — a query that is almost certainly a mistake.

TEXT values compare by byte order (Rust's `String` ordering), which is SQLite's BINARY collation — the only collation v0 accepts (§7.1).

### Ruling 3: `NULL <op> literal` is UNKNOWN for every operator, `!=` included

§6.1's three-valued logic already says a NULL operand makes a comparison UNKNOWN and filtering excludes it. `!=` is where an implementation is most likely to get this wrong (`NULL != 4` "looks" true), so it gets its own test and gate row.

### Ruling 4: the enumerated space — full on single tables, reduced on joins

| Space | Predicates | Queries |
|---|---|---|
| Single table (`gen_database`'s `t(k TEXT, v INTEGER)`) | `None`; each of 6 operators on each of 2 columns (12); `IsNull` and `IsNotNull` on each nullable column (4) — 17 | 3 group-bys × 3 aggregate sets × 17 = **153** (was 36) |
| Join (the 4-column joined row) | `None`; one comparison per column, operator `CmpOp::ALL[column % 6]` (4); one null test per column, `IsNull` on even columns and `IsNotNull` on odd ones (4) — 9 | 2 key pairs × 10 group-bys × 5 aggregate sets × 9 = **900** (was 700) |

Every operator's semantics are covered on single tables, where they are cheap. What the join adds is the predicate evaluated against the right position of the joined row, which one predicate per column covers; crossing all six operators with every join column would make each of the four join sweeps 4.7× slower (3300 queries) for no new kind of coverage. The join space never uses `=` or `!=` (4 columns reach only `CmpOp::ALL[0..4]`); the doc comment on `enumerate_join` says so.

Literals: `Value::Int(4)` for INTEGER, `Value::Text("v4")` for TEXT — the middle of `Domain::default()`'s values (`0..8`, rendered `"v0"`..`"v7"`), so each operator keeps some rows and drops others, and `=` / `!=` meet rows equal to the literal.

### Ruling 5: `gen_case` alternates single-table and join queries

`gen_case` picks `enumerate_database(db)[seed % len]`, and `enumerate_database` lists single-table queries first. With 153 of them, seeds 0–49 — the default `seed_range()` — would never draw a join query, silently switching off `a_two_table_case_runs_green_against_the_reference_engine`'s join coverage and the gate rows that depend on it (the ones recorded as going red "at seed 36"). So `enumerate_database` interleaves the two lists: single, join, single, join, …, then the rest of the longer list. Even seeds below 306 draw single-table queries and odd seeds draw joins.

### Ruling 6: the single-table incremental sweep runs every query once

`incremental_engine_is_green_across_the_enumerated_space` draws `seed % 36` for 50 seeds and asserts every query was exercised. With 153 queries that assertion fails. It becomes one case per query, seeded by the query's index — the shape the join sweeps already have, including their `IVMLITE_SEED` replay behaviour.

### Out of scope

- `AND`, `OR`, `NOT`, `LIKE`, `IN`, `BETWEEN` (Ruling 1; spec §6.1).
- Comparisons between two columns, and literals on the left (Phase 2b normalizes the latter).
- The three-column schema decision (spec §9.2) — still open; Task 3 recomputes its numbers under the new predicate sets.

---

## File structure

| File | Change |
|---|---|
| `crates/ivmlite-core/src/query.rs` | `CmpOp`; `Predicate::{Compare, IsNull}` replace `IntGt` |
| `crates/ivmlite-core/src/lib.rs` | export `CmpOp` |
| `crates/ivmlite-core/src/plan.rs` | literal-type check in `lower`; tests |
| `crates/ivmlite-core/src/node.rs` | `passes` for the new variants; tests |
| `crates/ivmlite-test/src/lib.rs` | re-export `CmpOp` |
| `crates/ivmlite-test/src/naive.rs` | its own `passes` for the new variants; tests |
| `crates/ivmlite-test/src/sql.rs` | render the new variants; tests |
| `crates/ivmlite-test/src/query.rs` | the widened enumeration; interleaving; tests |
| `crates/ivmlite-test/src/differential.rs` | `gen_case` doc; the `gen_case_picks_join…` test |
| `crates/ivmlite-test/tests/harness_catches_bugs.rs` | the single-table sweep; the seed-mapping comments; the shrink test's filter |
| `docs/mutation-gates.md` | new rows; rewrite of the rows that name `IntGt` |
| `docs/superpowers/specs/2026-09-18-ivmlite-design.md` | §6.1, §9.2, §13 |
| `crates/ivmlite-test/src/data.rs` | the query counts in `gen_database`'s doc |
| `docs/README.md` | index this plan |

---

### Task 1: The new predicate representation, evaluated by the engine, the reference and the oracle

**Files:**
- Modify: `crates/ivmlite-core/src/query.rs`, `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-core/src/plan.rs`, `crates/ivmlite-core/src/node.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`, `crates/ivmlite-test/src/naive.rs`, `crates/ivmlite-test/src/sql.rs`, `crates/ivmlite-test/src/query.rs` (only the one-line `IntGt` → `Compare` change in Step 6)
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces (used by Task 2 and by Phase 2b):
  ```rust
  pub enum CmpOp { Gt, Ge, Lt, Le, Eq, Ne }
  impl CmpOp { pub const ALL: [CmpOp; 6]; }
  pub enum Predicate {
      None,
      Compare { column: usize, op: CmpOp, value: Value },
      IsNull { column: usize },
      IsNotNull { column: usize },
  }
  ```
  `ivmlite_core::CmpOp` and `ivmlite_test::CmpOp` are both exported.

This task keeps the enumerated space exactly as it is — `IntGt { column, value: 4 }` becomes `Compare { column, op: Gt, value: Int(4) }` — so the differential suite checks that the representation change alone breaks nothing. Task 2 widens the space.

- [ ] **Step 1: Replace `Predicate` and add `CmpOp` in `crates/ivmlite-core/src/query.rs`**

Replace the `Predicate` enum with:

```rust
/// A comparison operator from spec §6.1's whitelist. The literal is always the
/// right operand: `Compare { op: Gt, .. }` is `column > literal`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    pub const ALL: [CmpOp; 6] = [
        CmpOp::Gt,
        CmpOp::Ge,
        CmpOp::Lt,
        CmpOp::Le,
        CmpOp::Eq,
        CmpOp::Ne,
    ];

    /// Whether `column <op> literal` holds, given how the column's value
    /// orders against the literal.
    pub(crate) fn holds(self, ord: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            CmpOp::Gt => ord == Greater,
            CmpOp::Ge => ord != Less,
            CmpOp::Lt => ord == Less,
            CmpOp::Le => ord != Greater,
            CmpOp::Eq => ord == Equal,
            CmpOp::Ne => ord != Equal,
        }
    }
}

/// A view's filter: spec §6.1's whitelist, at most one per view (M1b Phase 2a,
/// Ruling 1 — no `AND`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    None,
    /// `column <op> value`. `lower` requires `value` to have the column's
    /// declared type — `Int` for INTEGER, `Text` for TEXT — and rejects NULL.
    Compare { column: usize, op: CmpOp, value: Value },
    IsNull { column: usize },
    IsNotNull { column: usize },
}
```

Add `use crate::Value;` at the top of the file. In `crates/ivmlite-core/src/lib.rs` change the export to `pub use query::{Agg, AggFn, CmpOp, Join, Predicate, ViewQuery};`.

The frozen regression fixture `crates/ivmlite-test/tests/regressions/seed0-ops1.json` has `"predicate": "None"`, which still deserializes.

- [ ] **Step 2: Write the failing engine tests in `crates/ivmlite-core/src/node.rs`'s test module**

`passes` is a private function of this module, so its tests call it directly.

```rust
    /// Spec §6.1: each whitelisted operator keeps exactly the rows SQL keeps.
    /// The values sit below, at and above the literal, so every operator's
    /// boundary is pinned (`>` against `>=`, `<` against `<=`).
    #[test]
    fn each_comparison_operator_keeps_the_rows_sql_keeps() {
        let ints = [3, 4, 5];
        let texts = ["v3", "v4", "v5"];
        let expected: [(CmpOp, [bool; 3]); 6] = [
            (CmpOp::Gt, [false, false, true]),
            (CmpOp::Ge, [false, true, true]),
            (CmpOp::Lt, [true, false, false]),
            (CmpOp::Le, [true, true, false]),
            (CmpOp::Eq, [false, true, false]),
            (CmpOp::Ne, [true, false, true]),
        ];
        for (op, keep) in expected {
            let on_int = Predicate::Compare {
                column: 0,
                op,
                value: Value::Int(4),
            };
            let on_text = Predicate::Compare {
                column: 0,
                op,
                value: Value::Text("v4".into()),
            };
            for ((&n, &t), &k) in ints.iter().zip(&texts).zip(&keep) {
                assert_eq!(passes(&on_int, &row(vec![int(n)])), k, "{n} {op:?} 4");
                assert_eq!(
                    passes(&on_text, &row(vec![Value::Text(t.into())])),
                    k,
                    "'{t}' {op:?} 'v4'"
                );
            }
        }
    }

    /// TEXT compares by byte order — SQLite's BINARY collation, the only one v0
    /// accepts (§7.1) — not by any numeric reading of the text: 'v10' < 'v9'.
    #[test]
    fn text_comparison_is_byte_order() {
        let lt_v9 = Predicate::Compare {
            column: 0,
            op: CmpOp::Lt,
            value: Value::Text("v9".into()),
        };
        assert!(passes(&lt_v9, &row(vec![Value::Text("v10".into())])));
    }

    /// Spec §6.1 and Ruling 3: `NULL <op> literal` is UNKNOWN for every
    /// operator, and filtering excludes UNKNOWN — `NULL != 4` included.
    #[test]
    fn a_null_value_passes_no_comparison_not_even_not_equal() {
        for op in CmpOp::ALL {
            for value in [Value::Int(4), Value::Text("v4".into())] {
                let p = Predicate::Compare {
                    column: 0,
                    op,
                    value: value.clone(),
                };
                assert!(
                    !passes(&p, &row(vec![Value::Null])),
                    "NULL {op:?} {value:?} is UNKNOWN, so the row must be excluded"
                );
            }
        }
    }

    #[test]
    fn is_null_keeps_only_null_rows() {
        let p = Predicate::IsNull { column: 0 };
        assert!(passes(&p, &row(vec![Value::Null])));
        assert!(!passes(&p, &row(vec![int(0)])));
        assert!(!passes(&p, &row(vec![Value::Text(String::new())])));
    }
```

Add `CmpOp` to the test module's `use crate::{…}` line. Run `cargo test -p ivmlite-core --locked` and confirm the build fails: `Predicate::IntGt` no longer exists and `passes` does not match the new variants.

- [ ] **Step 3: Implement `passes` in `crates/ivmlite-core/src/node.rs`**

Replace the whole function and its doc comment with:

```rust
/// Spec §6.1's three-valued logic: a comparison with NULL is UNKNOWN, and the
/// row is excluded from the result.
///
/// It returns a `bool` rather than a three-valued enum because, for
/// **filtering**, "false" and "unknown" get the same treatment. But **do not
/// conclude from this that `NOT p` is equivalent to `!p`** — v0's predicate
/// whitelist has no `NOT` precisely because each addition means re-arguing
/// three-valued logic.
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::Compare { column, op, value } => match (row.get(*column), value) {
            // UNKNOWN for every operator, `!=` included: `NULL != 4` is not true.
            (Value::Null, _) => false,
            (Value::Int(a), Value::Int(b)) => op.holds(a.cmp(b)),
            // Byte order, which is SQLite's BINARY collation (§7.1).
            (Value::Text(a), Value::Text(b)) => op.holds(a.as_bytes().cmp(b.as_bytes())),
            (a, b) => unreachable!(
                "`lower` accepts a comparison only with a literal of the column's type, \
                 and a STRICT table stores only that type: compared {a:?} with {b:?}"
            ),
        },
        Predicate::IsNull { column } => matches!(row.get(*column), Value::Null),
        Predicate::IsNotNull { column } => !matches!(row.get(*column), Value::Null),
    }
}
```

Update the existing tests in this file: every `Predicate::IntGt { column: C, value: N }` becomes `Predicate::Compare { column: C, op: CmpOp::Gt, value: Value::Int(N) }` (`grep -n IntGt crates/ivmlite-core/src/node.rs` lists them). Update the comment in `filter_treats_null_as_unknown_not_as_false_negation` if it names `IntGt`.

- [ ] **Step 4: Write the failing `lower` tests in `crates/ivmlite-core/src/plan.rs`'s test module**

Replace `int_gt_over_a_text_column_is_rejected_at_the_boundary` with the first test below and add the other three. `int_then_text()` is the test module's existing `t(a INTEGER, b TEXT)` helper.

```rust
    fn cmp(column: usize, op: CmpOp, value: Value) -> Predicate {
        Predicate::Compare { column, op, value }
    }

    #[test]
    fn an_integer_literal_compared_with_a_text_column_is_rejected() {
        // SQLite applies the TEXT column's affinity to the literal and
        // compares it as text; v0 compares values exactly (spec §6.1,
        // "Operand types must match").
        let err = lower(
            &q(vec![0], vec![count()], cmp(1, CmpOp::Gt, Value::Int(3))),
            &Database::single(int_then_text()),
        )
        .expect_err("an INTEGER literal against a TEXT column must be rejected");
        assert!(
            err.0.contains("comparison") && err.0.contains("Text") && err.0.contains("Int"),
            "the error must name the comparison and both types: {}",
            err.0
        );
    }

    #[test]
    fn a_text_literal_compared_with_an_integer_column_is_rejected() {
        let err = lower(
            &q(vec![0], vec![count()], cmp(0, CmpOp::Eq, Value::Text("3".into()))),
            &Database::single(int_then_text()),
        )
        .expect_err("a TEXT literal against an INTEGER column must be rejected");
        assert!(
            err.0.contains("comparison") && err.0.contains("Integer") && err.0.contains("Text"),
            "the error must name the comparison and both types: {}",
            err.0
        );
    }

    #[test]
    fn a_null_literal_is_rejected_in_favour_of_is_null() {
        let err = lower(
            &q(vec![0], vec![count()], cmp(0, CmpOp::Eq, Value::Null)),
            &Database::single(int_then_text()),
        )
        .expect_err("a comparison with NULL is always UNKNOWN and must be rejected");
        assert!(
            err.0.contains("NULL") && err.0.contains("IS NULL"),
            "the error must point at IS NULL: {}",
            err.0
        );
    }

    #[test]
    fn a_literal_of_the_columns_own_type_is_accepted() {
        // Guards against over-rejecting: both types, several operators.
        let db = Database::single(int_then_text());
        for p in [
            cmp(0, CmpOp::Le, Value::Int(3)),
            cmp(1, CmpOp::Ne, Value::Text("x".into())),
            Predicate::IsNull { column: 1 },
        ] {
            lower(&q(vec![0], vec![count()], p.clone()), &db)
                .unwrap_or_else(|e| panic!("{p:?} is legal: {e}"));
        }
    }
```

Rename `int_gt_over_the_right_tables_text_column_is_rejected` to `a_comparison_on_the_right_tables_text_column_checks_its_type`, give it `cmp(2, CmpOp::Gt, Value::Int(3))`, and make its assertion `err.0.contains("comparison") && err.0.contains("Text")`. Every other `Predicate::IntGt { column: C, value: N }` in this file becomes `cmp(C, CmpOp::Gt, Value::Int(N))`, including the `assert_eq!(predicate, …)` in `lowers_to_scan_filter_project_aggregate`. Add `CmpOp, Value` to the test module's imports.

- [ ] **Step 5: Implement the checks in `lower`**

In `crates/ivmlite-core/src/plan.rs`, the bounds check becomes:

```rust
    match &query.predicate {
        Predicate::None => {}
        Predicate::Compare { column, .. }
        | Predicate::IsNull { column }
        | Predicate::IsNotNull { column } => check(*column, "predicate")?,
    }
```

Replace the `if let Predicate::IntGt { column, .. } = &query.predicate { require_integer(*column, "IntGt")?; }` block with:

```rust
    if let Predicate::Compare { column, op, value } = &query.predicate {
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
```

`Value` must be imported (`use crate::{…, Value, …}`). Update the comment above the type checks (the "Measured against SQLite…" block): the second bullet now covers every comparison operator — a mismatched literal is converted by the column's affinity in SQLite — and `IS NULL` joins `IS NOT NULL` as type-agnostic. Update `lower`'s doc comment ("v0 supports `SUM` and `IntGt` over INTEGER columns only") to say: `SUM` over INTEGER columns only, comparisons only against a literal of the column's type.

Run `cargo test -p ivmlite-core --locked`: all green.

- [ ] **Step 6: The harness's evaluators — `naive.rs`, `sql.rs`, and the enumerator's one line**

`NaiveRecompute` is the independent reference, so it spells the comparison differently from the engine: by `Value`'s derived ordering, not by `CmpOp::holds` (which is `pub(crate)` and not reachable from here anyway). In `crates/ivmlite-test/src/naive.rs` replace `passes` with:

```rust
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::Compare { column, op, value } => {
            let cell = row.get(*column);
            // NULL <op> anything is UNKNOWN (spec §6.1), for `!=` too.
            if *cell == Value::Null {
                return false;
            }
            // `lower` guarantees `cell` and `value` are the same variant, and
            // `Value`'s derived order within one variant is i64's order or
            // `String`'s byte order — SQLite's BINARY collation.
            match op {
                CmpOp::Gt => cell > value,
                CmpOp::Ge => cell >= value,
                CmpOp::Lt => cell < value,
                CmpOp::Le => cell <= value,
                CmpOp::Eq => cell == value,
                CmpOp::Ne => cell != value,
            }
        }
        Predicate::IsNull { column } => row.get(*column) == &Value::Null,
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}
```

Add `CmpOp` to its imports, and re-export `CmpOp` from `crates/ivmlite-test/src/lib.rs` next to `Predicate`. In naive.rs's tests, turn the `IntGt` at the existing test into `Compare { op: CmpOp::Gt, value: Value::Int(N) }` and add:

```rust
    #[test]
    fn naive_passes_follows_three_valued_logic_for_not_equal_and_is_null() {
        let ne = Predicate::Compare {
            column: 0,
            op: CmpOp::Ne,
            value: Value::Int(4),
        };
        assert!(passes(&ne, &Row::new(vec![Value::Int(3)])));
        assert!(!passes(&ne, &Row::new(vec![Value::Int(4)])));
        assert!(!passes(&ne, &Row::new(vec![Value::Null])), "NULL != 4 is UNKNOWN");
        let is_null = Predicate::IsNull { column: 0 };
        assert!(passes(&is_null, &Row::new(vec![Value::Null])));
        assert!(!passes(&is_null, &Row::new(vec![Value::Int(0)])));
    }
```

In `crates/ivmlite-test/src/sql.rs`, replace the `where_clause` match with:

```rust
    let where_clause = match &query.predicate {
        Predicate::None => String::new(),
        Predicate::Compare { column, op, value } => {
            format!(" WHERE {} {} {}", name(*column), sql_op(*op), sql_literal(value))
        }
        Predicate::IsNull { column } => format!(" WHERE {} IS NULL", name(*column)),
        Predicate::IsNotNull { column } => format!(" WHERE {} IS NOT NULL", name(*column)),
    };
```

and add, next to `sql_type`:

```rust
fn sql_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Eq => "=",
        CmpOp::Ne => "!=",
    }
}

/// A literal as SQL. A TEXT literal is single-quoted, with each `'` doubled.
///
/// # Panics
/// On `Value::Null`, which `lower` rejects as a comparison literal.
fn sql_literal(value: &Value) -> String {
    match value {
        Value::Int(n) => n.to_string(),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Null => panic!("a comparison literal is never NULL; lower rejects it"),
    }
}
```

(import `CmpOp` and `Value` from `ivmlite_core`). In its tests, change `to_sql_renders_predicate`'s predicate to `Compare { column: 1, op: CmpOp::Gt, value: Value::Int(3) }` (the asserted string is unchanged) and add:

```rust
    #[test]
    fn to_sql_renders_every_operator_text_literals_and_is_null() {
        let render = |predicate: Predicate| {
            view_query_to_sql(
                &ViewQuery {
                    group_by: vec![0],
                    aggs: vec![Agg {
                        func: AggFn::Count,
                        column: None,
                    }],
                    predicate,
                    join: None,
                },
                &Database::single(orders()),
            )
        };
        let expected = [
            (CmpOp::Gt, ">"),
            (CmpOp::Ge, ">="),
            (CmpOp::Lt, "<"),
            (CmpOp::Le, "<="),
            (CmpOp::Eq, "="),
            (CmpOp::Ne, "!="),
        ];
        for (op, symbol) in expected {
            let sql = render(Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(-3),
            });
            assert!(
                sql.contains(&format!("WHERE \"orders\".\"amount\" {symbol} -3 ")),
                "{sql}"
            );
        }
        let sql = render(Predicate::Compare {
            column: 0,
            op: CmpOp::Eq,
            value: Value::Text("it's".into()),
        });
        assert!(sql.contains("WHERE \"orders\".\"region\" = 'it''s' "), "{sql}");
        let sql = render(Predicate::IsNull { column: 0 });
        assert!(sql.contains("WHERE \"orders\".\"region\" IS NULL "), "{sql}");
    }
```

(The trailing space in each expected fragment is the one before `GROUP BY`; it stops `>` from matching inside `>=`.)

Finally, in `crates/ivmlite-test/src/query.rs`, change only the `IntGt` push inside `enumerate` to:

```rust
        predicates.push(Predicate::Compare {
            column: *i,
            op: CmpOp::Gt,
            value: Value::Int(4),
        });
```

and the coverage test's `Predicate::IntGt { .. } => saw_int_gt = true` arm to `Predicate::Compare { .. } => saw_int_gt = true` plus `Predicate::IsNull { .. } => {}` (Task 2 rewrites this test). The enumerated space is unchanged: still 36 single-table and 700 join queries.

- [ ] **Step 7: Run everything**

`cargo test --workspace --locked --no-fail-fast`: all green. The test count is the baseline (224) plus the tests this task added. Then fmt and clippy.

- [ ] **Step 8: Register the gate rows**

Add under `## ivmlite-core` in `docs/mutation-gates.md`, each mutation actually run:

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| §6.1 `>` excludes the literal's own value | In `CmpOp::holds`, make `Gt` return `ord != Less` | `each_comparison_operator_keeps_the_rows_sql_keeps` |
| §6.1 `>=` includes the literal's own value | Make `Ge` return `ord == Greater` | same |
| §6.1 `<` excludes the literal's own value | Make `Lt` return `ord != Greater` | same |
| §6.1 `<=` includes the literal's own value | Make `Le` return `ord == Less` | same |
| §6.1 `=` | Make `Eq` return `ord != Equal` | same |
| §6.1 `!=` | Make `Ne` return `ord == Equal` | same |
| §6.1 / Ruling 3: `NULL <op> literal` is UNKNOWN for every operator, `!=` included | In `passes`, make the `(Value::Null, _)` arm return `*op == CmpOp::Ne` | `a_null_value_passes_no_comparison_not_even_not_equal` |
| §6.1 `IS NULL` keeps exactly the NULL rows | Make `IsNull`'s arm the same as `IsNotNull`'s | `is_null_keeps_only_null_rows` |
| §7.1 TEXT compares by byte order (BINARY collation) | Compare TEXT by length first, then bytes (`a.len().cmp(&b.len()).then(a.cmp(b))`) | `text_comparison_is_byte_order` |
| §6.1 / Ruling 2: a literal must have its column's type | Delete the `if !matches { … }` block in `lower` | `an_integer_literal_compared_with_a_text_column_is_rejected`, `a_text_literal_compared_with_an_integer_column_is_rejected`, `a_comparison_on_the_right_tables_text_column_checks_its_type` |
| Ruling 2: a NULL literal is rejected | Make the `(_, Value::Null)` arm evaluate to `true` instead of returning the error | `a_null_literal_is_rejected_in_favour_of_is_null` |
| Ruling 2 must not over-reject | Make `matches` require `ColumnType::Integer` | `a_literal_of_the_columns_own_type_is_accepted` |

Also add one n/a row: requirement "`passes`'s mixed-type arm is unreachable: `lower` admits only a literal of the column's type, and a STRICT table stores only that type"; mutation "Replace the `unreachable!` with `false`"; Verified `None — no legal input reaches the arm; a mutation there changes no observable behaviour, so no test can go red`.

For every row, record which tests went red, not only the one named: the sweeps do not generate the new operators yet, so the named test is expected to be the only red one for the operator rows.

The existing rows that name `IntGt` (`grep -n IntGt docs/mutation-gates.md`) are rewritten in Task 3, not here. Run `python3 scripts/count-mutation-gates.py`.

- [ ] **Step 9: Commit**

```bash
git add crates/ivmlite-core/src/query.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/node.rs crates/ivmlite-test/src/lib.rs crates/ivmlite-test/src/naive.rs crates/ivmlite-test/src/sql.rs crates/ivmlite-test/src/query.rs docs/mutation-gates.md
git commit -m "feat(core): the full comparison whitelist, typed literals and IS NULL

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The enumerated space covers the whitelist

**Files:**
- Modify: `crates/ivmlite-test/src/query.rs`, `crates/ivmlite-test/src/differential.rs`, `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `CmpOp`, `CmpOp::ALL`, `Predicate::{Compare, IsNull, IsNotNull}` from Task 1.
- Produces: `enumerate(schema)` returns 153 queries over `gen_database`'s tables; `enumerate_join` returns 900 over `gen_database(2)` and over `gen_database_with_swapped_right_table()`; `enumerate_database` interleaves the two (Ruling 5).

- [ ] **Step 1: Write the failing enumerator tests in `crates/ivmlite-test/src/query.rs`**

Replace `enumerate_covers_the_v0_space_and_is_nonempty`'s predicate bookkeeping (from the `// I4:` comment to the end of the test) with:

```rust
        // I4: an exhaustive match — adding a new Predicate variant later fails
        // to compile here, forcing the matching coverage assertion to be added,
        // instead of quietly missing a whole branch and staying green the way
        // IsNotNull once did.
        let mut saw_none = false;
        let mut saw_compare: Vec<(CmpOp, ColumnType)> = Vec::new();
        let mut saw_is_null = false;
        let mut saw_is_not_null = false;
        for q in &qs {
            match &q.predicate {
                Predicate::None => saw_none = true,
                Predicate::Compare { column, op, .. } => {
                    saw_compare.push((*op, orders().columns[*column].ty))
                }
                Predicate::IsNull { .. } => saw_is_null = true,
                Predicate::IsNotNull { .. } => saw_is_not_null = true,
            }
        }
        assert!(saw_none, "enumerate must produce Predicate::None");
        for op in CmpOp::ALL {
            for ty in [ColumnType::Integer, ColumnType::Text] {
                assert!(
                    saw_compare.contains(&(op, ty)),
                    "enumerate must compare a {ty:?} column with {op:?}"
                );
            }
        }
        assert!(saw_is_null, "enumerate must produce Predicate::IsNull");
        assert!(saw_is_not_null, "enumerate must produce Predicate::IsNotNull");
```

(`orders()` has a nullable TEXT column and a NOT NULL INTEGER column; `IsNull`/`IsNotNull` come from the nullable one.) Add:

```rust
    #[test]
    fn enumerate_sizes_match_the_plan() {
        // M1b Phase 2a, Ruling 4.
        let db = crate::gen_database(2);
        assert_eq!(enumerate(&db.tables()[0]).len(), 153);
        assert_eq!(enumerate_join(&db.tables()[0], &db.tables()[1]).len(), 900);
        let swapped = crate::gen_database_with_swapped_right_table();
        assert_eq!(
            enumerate_join(&swapped.tables()[0], &swapped.tables()[1]).len(),
            900
        );
    }

    #[test]
    fn every_comparison_literal_has_its_columns_type() {
        let db = crate::gen_database(2);
        let (l, r) = (&db.tables()[0], &db.tables()[1]);
        let joined: Vec<ColumnType> = l.columns.iter().chain(&r.columns).map(|c| c.ty).collect();
        let single = enumerate(l).into_iter().map(|q| (q, &joined[..2]));
        let joins = enumerate_join(l, r).into_iter().map(|q| (q, &joined[..]));
        for (q, types) in single.chain(joins) {
            if let Predicate::Compare { column, value, .. } = &q.predicate {
                let ok = matches!(
                    (types[*column], value),
                    (ColumnType::Integer, Value::Int(_)) | (ColumnType::Text, Value::Text(_))
                );
                assert!(ok, "{q:?}");
            }
        }
    }

    #[test]
    fn join_predicates_reach_every_column_of_the_joined_row() {
        // Ruling 4: the join space's job is to evaluate a predicate at every
        // position of the joined row, both sides of the boundary.
        let db = crate::gen_database(2);
        let qs = enumerate_join(&db.tables()[0], &db.tables()[1]);
        for column in 0..4 {
            assert!(
                qs.iter().any(|q| matches!(q.predicate, Predicate::Compare { column: c, .. } if c == column)),
                "no comparison on joined column {column}"
            );
            assert!(
                qs.iter().any(|q| matches!(
                    q.predicate,
                    Predicate::IsNull { column: c } | Predicate::IsNotNull { column: c } if c == column
                )),
                "no null test on joined column {column}"
            );
        }
    }

    #[test]
    fn enumerate_database_alternates_single_table_and_join_queries() {
        // Ruling 5: any run of consecutive seeds draws both kinds.
        let two = Database::new(vec![kv("t0"), kv("t1")]);
        let all = enumerate_database(&two);
        let single = enumerate(&kv("t0"));
        let joins = enumerate_join(&kv("t0"), &kv("t1"));
        assert_eq!(all.len(), single.len() + joins.len());
        let paired = single.len().min(joins.len());
        for i in 0..paired {
            assert_eq!(all[2 * i], single[i], "even positions are single-table queries");
            assert_eq!(all[2 * i + 1], joins[i], "odd positions are join queries");
        }
        let rest = if single.len() > paired { &single[paired..] } else { &joins[paired..] };
        assert_eq!(&all[2 * paired..], rest);
    }
```

Replace `enumerate_database_adds_join_queries_only_with_two_tables` with a version that keeps only its first two lines (`one`, and the `assert_eq!(enumerate_database(&one), enumerate(&kv("t0")))`) — the interleaving test above now covers the two-table case. Add `CmpOp` and `Value` to the test module's imports. Run `cargo test -p ivmlite-test --lib --locked query::` and confirm these tests fail.

- [ ] **Step 2: Implement the enumeration**

Rewrite `crates/ivmlite-test/src/query.rs`'s non-test code as follows (`enumerate`, `enumerate_join` and `enumerate_database` keep their signatures):

```rust
use crate::{Agg, AggFn, CmpOp, ColumnType, Predicate, Schema, ViewQuery};
use ivmlite_core::{Database, Join, Value};

/// Enumerate v0's single-table query space.
///
/// Spec §9.2: v0 has finitely many combinations, so enumeration beats
/// randomness — reproducible and complete. Randomness is left to the update
/// sequences.
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    cross(
        &group_by_choices(schema),
        &agg_choices(schema),
        &predicates(schema),
    )
}

/// Enumerate v0's two-table join space: every pair of same-typed key columns,
/// crossed with the group-bys, aggregates and join predicates over the joined
/// row — `left`'s columns, then `right`'s.
///
/// Keys of different types are never generated: `lower` rejects them (SQLite
/// would compare them under numeric affinity).
///
/// The predicates are `join_predicates`, not the single-table set (M1b Phase
/// 2a, Ruling 4): one comparison and one null test per column, which reaches
/// every position of the joined row. The operators rotate with the column, so
/// over a 4-column joined row only `>`, `>=`, `<` and `<=` appear; `=` and `!=`
/// are covered by the single-table space.
pub fn enumerate_join(left: &Schema, right: &Schema) -> Vec<ViewQuery> {
    // The joined row, as a schema, so the dimension functions can run on it.
    // Its name and column names are never rendered.
    let joined = Schema {
        table: "joined".into(),
        columns: left
            .columns
            .iter()
            .chain(right.columns.iter())
            .cloned()
            .collect(),
    };
    let shapes = cross(
        &group_by_choices(&joined),
        &agg_choices(&joined),
        &join_predicates(&joined),
    );
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
/// and, when `db` has at least two tables, the join queries over its first
/// two, **alternating** — single, join, single, join, …, then the rest of the
/// longer list (M1b Phase 2a, Ruling 5). `gen_case` picks by `seed % len`, so
/// any run of consecutive seeds draws both kinds.
///
/// # Panics
/// If `db` declares no tables: the single-table queries are enumerated over
/// `db.tables()[0]`.
pub fn enumerate_database(db: &Database) -> Vec<ViewQuery> {
    let tables = db.tables();
    let single = enumerate(&tables[0]);
    if tables.len() < 2 {
        return single;
    }
    let joins = enumerate_join(&tables[0], &tables[1]);
    let mut out = Vec::with_capacity(single.len() + joins.len());
    let mut single = single.into_iter();
    let mut joins = joins.into_iter();
    loop {
        match (single.next(), joins.next()) {
            (None, None) => return out,
            (s, j) => out.extend(s.into_iter().chain(j)),
        }
    }
}

fn cross(group_bys: &[Vec<usize>], aggs: &[Vec<Agg>], predicates: &[Predicate]) -> Vec<ViewQuery> {
    let mut out = Vec::new();
    for group_by in group_bys {
        for aggs in aggs {
            for predicate in predicates {
                out.push(ViewQuery {
                    group_by: group_by.clone(),
                    aggs: aggs.clone(),
                    predicate: predicate.clone(),
                    join: None,
                });
            }
        }
    }
    out
}

fn group_by_choices(schema: &Schema) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    for i in 0..schema.arity() {
        out.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            out.push(vec![i, j]);
        }
    }
    out
}

fn agg_choices(schema: &Schema) -> Vec<Vec<Agg>> {
    let count = Agg {
        func: AggFn::Count,
        column: None,
    };
    let mut out = vec![vec![count.clone()]];
    for (i, col) in schema.columns.iter().enumerate() {
        if col.ty != ColumnType::Integer {
            continue;
        }
        let sum = Agg {
            func: AggFn::Sum,
            column: Some(i),
        };
        out.push(vec![sum.clone()]);
        out.push(vec![sum, count.clone()]);
    }
    out
}

/// The literal every enumerated comparison on a column of type `ty` uses: the
/// middle of `Domain::default()`'s values (`0..8`, rendered `"v0"`..`"v7"` for
/// TEXT), so each operator keeps some rows and drops others, and `=` / `!=`
/// meet rows equal to the literal.
fn literal(ty: ColumnType) -> Value {
    match ty {
        ColumnType::Integer => Value::Int(4),
        ColumnType::Text => Value::Text("v4".into()),
    }
}

/// The single-table predicates: `None`, every operator on every column, and
/// `IS NULL` / `IS NOT NULL` on every nullable column.
fn predicates(schema: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in schema.columns.iter().enumerate() {
        for op in CmpOp::ALL {
            out.push(Predicate::Compare {
                column,
                op,
                value: literal(col.ty),
            });
        }
    }
    for (column, col) in schema.columns.iter().enumerate() {
        if col.nullable {
            out.push(Predicate::IsNull { column });
            out.push(Predicate::IsNotNull { column });
        }
    }
    out
}

/// The join predicates (Ruling 4): `None`, one comparison per column with the
/// operator `CmpOp::ALL[column % 6]`, and one null test per nullable column —
/// `IS NULL` on even columns, `IS NOT NULL` on odd ones.
fn join_predicates(joined: &Schema) -> Vec<Predicate> {
    let mut out = vec![Predicate::None];
    for (column, col) in joined.columns.iter().enumerate() {
        out.push(Predicate::Compare {
            column,
            op: CmpOp::ALL[column % CmpOp::ALL.len()],
            value: literal(col.ty),
        });
    }
    for (column, col) in joined.columns.iter().enumerate() {
        if col.nullable {
            out.push(if column % 2 == 0 {
                Predicate::IsNull { column }
            } else {
                Predicate::IsNotNull { column }
            });
        }
    }
    out
}
```

Run `cargo test -p ivmlite-test --lib --locked query::`: green.

- [ ] **Step 3: The seed-mapping consequences**

In `crates/ivmlite-test/src/differential.rs`:
- `gen_case`'s doc comment: replace "single-table queries over the anchor first, then — with two or more tables — the join queries over the first two" with "which alternates the anchor's single-table queries with — for two or more tables — the join queries over the first two (M1b Phase 2a, Ruling 5)".
- `gen_case_picks_join_queries_for_two_table_databases`: strengthen the assertion to `assert!(joins >= 20, "only {joins} of seeds 0..50 picked a join query")` — with interleaving it is 25.

In `crates/ivmlite-test/tests/harness_catches_bugs.rs`:
- The doc comment of `a_two_table_case_runs_green_against_the_reference_engine`: replace "Seeds 0–35 draw single-table queries and seeds 36 and up draw join queries" with "Even seeds draw single-table queries and odd seeds draw join queries (`enumerate_database` alternates them)", and each "goes red at seed 36" with "goes red at seed N", where N is the first failing seed you observe when you re-run that mutation in Step 5.
- `shrink_reduces_initial_rows_in_every_table_of_a_multi_table_case`: add `.filter(|c| c.query.join.is_none())` before `.find(…)`, and replace the comment above the `case.query.join.is_none()` assertion with: "The "t1 should shrink to 0 rows" argument below holds only for a single-table query: with a join, t1 is observable. The filter above picks one; this assertion keeps the argument honest if the filter is ever removed." Keep the assertion.
- `incremental_engine_is_green_across_the_enumerated_space` (Ruling 6): replace its body with one case per query, the same shape as `incremental_engine_is_green_across_the_join_space_of`:

```rust
fn incremental_engine_is_green_across_the_enumerated_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate(&db.tables()[0]);
    // One case per query, seeded by its index, so the whole space is covered
    // whatever its size (M1b Phase 2a, Ruling 6; before it, 50 seeds drew
    // `seed % 36` and a coverage assertion checked every query was hit).
    // Under `IVMLITE_SEED=<n>` only query `n` runs, so the replay command a
    // `Failure` prints reproduces exactly the failing case.
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
            panic!("the incremental engine disagrees with the oracle at seed={seed}: {f}");
        }
    }
}
```

Keep the test's existing doc comment, and add a sentence to it: since M1b Phase 2a it runs every enumerated query once rather than 50 seeds (Ruling 6).

- [ ] **Step 4: Run everything and look at every seed-dependent test**

`cargo test --workspace --locked --no-fail-fast`. The single-table seed-to-query mapping has changed (the predicate list is longer), so every test that takes its query from `gen_case` over `Database::single(…)` now draws different queries: `naive_engine_is_green_across_many_seeds`, the detection-rate test, `failing_case_shrinks_to_under_ten_ops`, the batch-invariance tests. They must still pass unchanged. If any does not, stop and report it with the failing output (Global Constraints: no threshold is loosened).

Record the new total test count.

- [ ] **Step 5: Register and re-verify gate rows**

New rows (under `## ivmlite-test: generators` unless stated):

| Spec requirement | Mutation | Test that should go red |
|---|---|---|
| §6.1 every operator is enumerated on both column types | In `predicates`, iterate `CmpOp::ALL[..5]` (drop `Ne`) | `enumerate_covers_the_v0_space_and_is_nonempty` |
| §6.1 comparisons on TEXT columns are enumerated | In `predicates`, skip columns whose type is TEXT | `enumerate_covers_the_v0_space_and_is_nonempty` |
| §6.1 `IS NULL` is enumerated | Delete the `IsNull` push in `predicates` | `enumerate_covers_the_v0_space_and_is_nonempty` |
| Ruling 4: the join space tests a predicate at every position of the joined row | In `join_predicates`, add `.take(2)` after both `.enumerate()` calls, so only the left table's columns get predicates | `join_predicates_reach_every_column_of_the_joined_row` |
| Ruling 5: consecutive seeds draw both single-table and join queries | Make `enumerate_database` return `single` followed by `joins` | `enumerate_database_alternates_single_table_and_join_queries`, `gen_case_picks_join_queries_for_two_table_databases` |
| §6.1 the engine agrees with SQLite on every operator (differential) | In core's `CmpOp::holds`, make `Ge` return `ord == Greater` | `incremental_engine_is_green_across_the_enumerated_space` (and the unit test from Task 1) |
| §6.1 the engine agrees with SQLite on `!=` over NULL (differential) | In core's `passes`, make the `(Value::Null, _)` arm return `*op == CmpOp::Ne` | `incremental_engine_is_green_across_the_enumerated_space` |
| The reference implementation evaluates each operator correctly (under `## ivmlite-test: semantic contracts`) | In `naive.rs`'s `passes`, make `CmpOp::Le => cell < value` | `naive_engine_passes_every_enumerated_query` |
| The reference implementation treats `NULL != literal` as UNKNOWN | In `naive.rs`'s `passes`, delete the `if *cell == Value::Null { return false; }` | `naive_passes_follows_three_valued_logic_for_not_equal_and_is_null`, `naive_engine_passes_every_enumerated_query` |
| The oracle renders each operator as its SQL symbol (under `## ivmlite-test: semantic contracts`) | In `sql_op`, map `Ne` to `"="` | `to_sql_renders_every_operator_text_literals_and_is_null`, and the sweeps |
| The oracle quotes TEXT literals | In `sql_literal`, render `Value::Text(s)` as `s.clone()` | `to_sql_renders_every_operator_text_literals_and_is_null`, and the sweeps (SQLite reads `v4` as a column name) |

Also add one n/a row: requirement "Ruling 6: the single-table incremental sweep runs every enumerated query"; mutation "Make the sweep's `selected` `(0..50).collect()`"; Verified `None — no test observes how many queries a sweep ran; the sweep's structure, one case per index of `enumerate`, is the guard` (run the mutation once anyway and say in your report what, if anything, went red).

For the "and the sweeps" and differential rows, record every test that went red.

Re-verify the existing rows whose recorded evidence depends on the seed mapping: `grep -n "seed 36" docs/mutation-gates.md`, plus the two rows at "§8.5 `apply` must really keep non-anchor tables' deltas" and "§8.2 `run`'s own reference bookkeeping". Re-run each of those mutations, and update the row's recorded counts and seed with what you observe, noting "(re-verified in M1b Phase 2a after `enumerate_database` began alternating)". If one of them no longer goes red, stop and report it.

Run `python3 scripts/count-mutation-gates.py`.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/tests/harness_catches_bugs.rs docs/mutation-gates.md
git commit -m "test(harness): enumerate the whole predicate whitelist; alternate single-table and join seeds

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Spec, measured costs, and the rows that still name `IntGt`

**Files:**
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md`, `docs/mutation-gates.md`, `crates/ivmlite-test/src/data.rs`, `docs/README.md`

- [ ] **Step 1: Measure the sweeps**

Run each of the five enumeration sweeps three times, on its own, in a debug build, and read the time off the terminal:

```bash
cargo test -p ivmlite-test --locked --test harness_catches_bugs incremental_engine_is_green_across_the_enumerated_space -- --exact
cargo test -p ivmlite-test --locked --test harness_catches_bugs incremental_engine_is_green_across_the_join_space -- --exact
cargo test -p ivmlite-test --locked --test harness_catches_bugs incremental_engine_is_green_across_the_join_space_with_keys_at_different_positions -- --exact
cargo test -p ivmlite-test --locked --lib naive_engine_passes_every_enumerated_join_query -- --exact
cargo test -p ivmlite-test --locked --lib naive_engine_passes_every_join_query_with_keys_at_different_positions -- --exact
```

(If `--exact` needs the module path for the `--lib` tests, use `differential::tests::<name>`.) Record the three times and the median of each.

Also count — do not time — the three-column join space under the new predicate sets: in a scratch test that is **not committed**, build two 3-column tables (`k TEXT, v INTEGER, w INTEGER`, all nullable) and print `enumerate_join(…).len()`. Delete the scratch test afterwards.

- [ ] **Step 2: Amend the spec**

In `docs/superpowers/specs/2026-09-18-ivmlite-design.md`:

1. §6.1 "Three-valued predicate logic": after the whitelist paragraph, add a paragraph headed "**Implemented (amended 2026-09-24, M1b Phase 2a).**" saying: the whole whitelist is implemented as `Predicate::{Compare { column, op, value }, IsNull, IsNotNull}` with `CmpOp::{Gt, Ge, Lt, Le, Eq, Ne}`; a view has **at most one** predicate — `AND` is not supported either, since the whitelist is about single comparisons and each conjunction multiplies the enumerated space; the literal is always the right operand (the SQL front end normalizes `3 < v`); `NULL <op> literal` is UNKNOWN for every operator, `!=` included; TEXT compares by byte order, which is the BINARY collation of §7.1.
2. §6.1 "Operand types must match": add a paragraph "**Amended 2026-09-24, M1b Phase 2a:** the rule now covers all six comparison operators and both column types: a comparison's literal must have its column's declared type, since SQLite would convert a mismatched literal by the column's affinity (a TEXT column compared with `3` compares with `'3'`). A NULL literal is rejected too — `v = NULL` is UNKNOWN for every row — with an error pointing at `IS NULL`."
3. §9.2: in the table, add the measured rows "One table, 2 columns, full predicate whitelist (measured in M1b Phase 2a, <date>) | 153 | <median>" and "Two tables, 2 columns each, + join, reduced join predicates (measured in M1b Phase 2a) | 900 | <median>", keep the older rows as history, and replace the three-column row's count with the one you counted (time: count × the new measured per-case join cost, marked extrapolated). In the paragraph that lists the five sweeps' medians, add the new medians as the current figures, keeping the dated older ones. Explain the reduced join predicate set in one sentence (Ruling 4 of this plan). Recompute the "one knob" paragraph's figure for the new space: 2 key pairs × 4 single-column group-bys × 5 aggregate sets × 9 predicates = 360 queries.
4. §13 item 7: append "; a view has at most one predicate — no `AND` (M1b Phase 2a)". Item 8: replace "a column may be compared only with a literal of its own type" with "a column may be compared only with a non-NULL literal of its own type".

- [ ] **Step 3: Rewrite the gate rows that still name `IntGt`**

`grep -n IntGt docs/mutation-gates.md` lists them (the three-valued-logic row, the TEXT-arm n/a row, the P2-1 rejection row, the `IsNotNull`-over-TEXT row, the "all three predicate forms enumerated" row, and any other). For each: rewrite its mutation and test names in terms of the current code (`Compare`, `CmpOp::Gt`, the renamed tests), **re-run the mutation**, and replace the recorded counts with what you observe, adding "(re-verified in M1b Phase 2a after `IntGt` became `Compare`)". The TEXT-arm n/a row describes an arm that no longer exists: rewrite it to point at Task 1's `unreachable!` n/a row, or delete it if it now duplicates that row, and say which you did in your report. `grep -n IntGt docs/mutation-gates.md crates` must print nothing afterwards.

Run `python3 scripts/count-mutation-gates.py`.

- [ ] **Step 4: The remaining stale numbers**

- `crates/ivmlite-test/src/data.rs`, `gen_database`'s doc: "the two-column join enumeration is 700 queries (measured), and widening to 3 columns is projected to grow it to about 4209" becomes 900 and the count from Step 1.
- `grep -rn "\b36\b\|\b700\b" crates/ivmlite-test docs/mutation-gates.md` and fix any other place that states the old sizes as current (leave dated historical measurements alone).
- `docs/README.md`: add after the M1b Phase 1 entry:
  `  - [`2026-09-24-m1b-phase2a-predicate-whitelist.md`](superpowers/plans/2026-09-24-m1b-phase2a-predicate-whitelist.md) — M1b Phase 2a: the full predicate whitelist`

- [ ] **Step 5: Verify and commit**

`cargo test --workspace --locked --no-fail-fast`, fmt, clippy, `python3 scripts/count-mutation-gates.py` all clean.

```bash
git add docs/superpowers/specs/2026-09-18-ivmlite-design.md docs/mutation-gates.md crates/ivmlite-test/src/data.rs docs/README.md
git commit -m "docs: record the predicate whitelist, its measured cost, and re-verified gate rows

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage

| Spec requirement | Where |
|---|---|
| §6.1 whitelist `>` `>=` `<` `<=` `=` `!=` | Task 1 (engine, reference, oracle), Task 2 (enumerated) |
| §6.1 `IS NULL` / `IS NOT NULL` | Task 1, Task 2 |
| §6.1 three-valued logic, `!=` over NULL | Task 1 unit tests and gate rows; Task 2 differential rows |
| §6.1 operand types must match, extended | Task 1 (`lower`), Task 3 (spec) |
| §7.1 BINARY collation for TEXT comparison | Task 1 (`text_comparison_is_byte_order`) |
| §9.2 enumeration beats randomness; measured cost | Task 2, Task 3 |
| §13 items 7 and 8 | Task 3 |
| `AND` | Ruling 1: out of scope, recorded in §13 by Task 3 |
