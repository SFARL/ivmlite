# ivmlite design

- **Date**: 2026-09-18
- **Status**: approved, being implemented
- **Name**: `ivmlite` (following SQLite's `Lite` spelling; not taken on crates.io)

---

## 1. What this is

ivmlite is a SQLite extension that gives SQLite **incremental materialized views** (Incremental View Maintenance). The cost of maintaining a view is proportional to the size of the change `Δ`, not to the size of the base tables.

It is written in Rust, and its data model is DBSP's Z-set (a multiset with weights).

### 1.1 Goals

1. **Learn Rust and database internals** — implement the operators and state management ourselves, rather than generating SQL for the host to execute.
2. **Build a trustworthy correctness-verification system for IVM** — so far **no reusable, cross-implementation property-based differential testing harness for IVM has been found**. (This does not claim "nobody has ever done it": that is not a provable proposition.)
3. **Reach an honest performance conclusion**, including the conclusion "not worth doing".

### 1.2 Non-goals

- **Algorithmic novelty.** DBSP already provides a complete theoretical framework; this project is an engineering implementation and does not try to invent new incremental algorithms.
- **Production readiness** (explicitly not a goal before M2).
- **Competing with Turso for adoption.**

### 1.3 Why build it when implementations already exist

Known implementations of the same kind:

| Project | Host | Language | Architecture | Status |
|---|---|---|---|---|
| Turso | SQLite-compatible (a Rust rewrite) | Rust | DBSP circuit | experimental; on-disk format not stable |
| duckDBSP | DuckDB | C++ | DBSP | experimental |
| OpenIVM | DuckDB | C++ | SQL-to-SQL compilation | research prototype (SIGMOD 2024) |
| Feldera | standalone engine | Rust | DBSP, SQL→Rust code generation | relatively mature, MIT (except the enterprise parts) |

This project does not assume it will win. Its value rests on three points:

1. **The learning value** is not reduced by someone else having done it.
2. **All four implementations are labelled experimental, and their semantics are not stable.** Nobody currently runs a cross-implementation differential test suite; one would produce value for all four projects at once, which makes it a clean entry point for upstream contributions.
3. **Turso is an implementation on the same host and from the same theory, so it is a natural cross-validation oracle.**

---

## 2. Why SQLite and not DuckDB

For IVM to pay for itself, four premises must hold at once:

| Premise | DuckDB | SQLite |
|---|---|---|
| The process lives long (so the state has somewhere to live and its cost can be amortized) | Often false — start a process, scan Parquet, query once, exit | True: app process / browser tab / agent session |
| The data changes while the process lives | Often false — Parquet is mostly a static snapshot | True, and continuous (the definition of OLTP) |
| The same query is asked again and again | Weaker still in the AI era: agents generate ad-hoc queries, and views cannot be declared in advance | Strongly true: every UI render / every sync re-runs the same set of queries |
| Recomputation is expensive relative to the budget | The data is large, but DuckDB itself is extremely fast | The data is small, but the budget is a 16ms frame |

Conclusion: **DuckDB is hotter, but SQLite is where IVM actually has work to do.** The corroborating evidence is the local-first ecosystem (LiveStore, TanStack DB) implementing incremental computation on its own — TanStack DB even hand-wrote differential dataflow in JS.

SQLite is also a better engineering fit for this project: its C API is small and stable, binding to it from Rust is a mature path (`rusqlite` / `libsqlite3-sys`), and there is none of DuckDB's C++ ABI and FFI impedance — which would undercut the "learn Rust" goal outright.

---

## 3. Deliverable

**v0 form: a SQLite loadable extension (a Rust cdylib).**

It covers server / desktop / CLI. It explicitly does not cover browsers or the iOS system SQLite (neither can load extensions).

The rejected alternative forms are in §12.2.

---

## 4. Architecture

### 4.1 Crates

```
ivmlite/
├── crates/
│   ├── ivmlite-core/     pure Rust, no rusqlite / libsqlite3
│   ├── ivmlite-sql/      SQL → plan IR (SQLite dialect + SQLite semantics)
│   ├── ivmlite-sqlite/   cdylib: extension entry point, triggers, shadow tables, vtab
│   ├── ivmlite-test/     differential testing harness (lib + bin, runs in CI)
│   ├── ivmlite-workload/ portable benchmark workloads (no dependency on the other crates)
│   └── ivmlite-bench/    benchmark runner and baselines
├── workloads/
└── docs/
```

Dependencies run one way: `ivmlite-sqlite` → `ivmlite-sql` → `ivmlite-core`.

> **Amended 2026-09-26, M1b Phase 3a:** `ivmlite-sqlite` is built **outside the Cargo workspace**, with its own `Cargo.lock` (`crates/ivmlite-sqlite/Cargo.toml` declares an empty `[workspace]` of its own, and the root `Cargo.toml` excludes the crate). The reason is feature unification: the extension needs `rusqlite`'s `loadable_extension` and `vtab` features, while the test host `ivmlite-test` needs `bundled` and `load_extension` to load the built library; Cargo unifies features across every member of one workspace build, and the two combinations do not compile together (measured 2026-09-26). See the Phase 3a spec §2 for the resulting two-build layout and its scripts.

### 4.2 Hard constraints

> **`ivmlite-core` must not depend on `rusqlite` or `libsqlite3-sys`.**

This keeps the core unit-testable with no SQLite at all (feed deltas in, take deltas out), so operator-correctness tests never need to start a database. A side benefit is that a future "library next to SQLite" form would need no rework — but **no abstraction is added ahead of time for it**.

> **All `unsafe` and FFI is allowed only in `ivmlite-sqlite`.**

### 4.3 Crate responsibilities

**`ivmlite-core`**
- `Value` / `Row` / `ZSet` (with i64 weights)
- the plan IR
- the operator trait and its implementations
- the `Arrangement` trait — a key-indexed state abstraction whose shape is defined by what join needs
- incrementalization: plan → dataflow

**`ivmlite-sql`**
- `sqlparser-rs` (SQLite dialect) → plan IR
- name resolution and type inference against a `Catalog` trait (a trait so tests can fake it without a real database)
- **any query outside the subset is a hard error**, with no silent fallback to full recomputation (see §12.5)

> **Implemented (M1b Phase 2b, 2026-09-24).** `compile(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError>` parses a view's `SELECT` with `sqlparser`'s SQLite dialect and resolves it into a `ResolvedView` that it lowers through `ivmlite_core::lower`. `CompiledView` carries the `Plan`, the output columns (name, type, nullability, SELECT order), and the base tables the view reads, anchor first. The accepted grammar is
>
> ```text
> SELECT <column>, …, <aggregate>, …
> FROM <table> [[AS] <alias>]
>      [[INNER] JOIN <table> [[AS] <alias>] ON <column> = <column>]
> [WHERE <column> <op> <literal> | <literal> <op> <column>
>      | <column> IS NULL | <column> IS NOT NULL]
> GROUP BY <column>, …
> ```
>
> — strictly wider than the harness's rendered SQL: it also accepts table and column aliases, an unqualified column when only one table in scope has it, a literal on the left of a comparison (`3 < v` flips to `v > 3`), `<>` alongside `!=`, parentheses around the WHERE expression or an operand, and the explicit `INNER` keyword on JOIN. A result column is named the way SQLite names it: an alias if there is one, else a bare column's declared name, else an aggregate's text exactly as written; two result columns named alike, compared case-insensitively, are rejected, since Phase 3 declares a view's columns as a table and SQLite rejects two columns of one name. Name resolution and typing go through a `Catalog` trait (`fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError>`) — fallible so the SQLite `Catalog` (Phase 3) can reject what v0 cannot represent, reading `PRAGMA table_info` and the table's DDL: a non-STRICT table, an `ANY` column, a `COLLATE` clause (§7.1); `Database` implements it for tests and never fails. Legality lives in `ivmlite_core::lower` alone: `ivmlite-sql` only resolves names into a `ResolvedView` and calls `lower`, and the harness's `ViewQuery` is resolved the same way, through the new `lower_query`. `every_enumerated_query_compiles_to_the_plan_lower_query_builds` (`crates/ivmlite-test/tests/sql_front_end.rs`) compiles the SQL every one of the 2 × (153 + 900) enumerated queries renders to and checks it produces the same `Plan` that `lower_query` builds from the `ViewQuery` itself — standing in for running the differential sweeps through SQL, since the engine's behaviour is a function of its `Plan` alone.

**`ivmlite-sqlite`**
- the `sqlite3_ivmlite_init` extension entry point
- the control surface: the vtab module for `CREATE VIRTUAL TABLE ... USING ivm(...)` and the `INSERT INTO v(v)` command channel (§8.3, settled by M-1; **not** scalar functions — design A was ruled out by measurement)
- DDL generation for triggers and delta tables
- reading and writing shadow tables; the SQLite implementation of `Arrangement`
- the `PRAGMA table_info` implementation of `Catalog`
- bootstrap (the initial full computation when a view is created on a table that already has data)

**`ivmlite-test`**
- generators for schemas / data / update sequences / queries
- oracle executors (full recomputation, this engine, Turso)
- shrinking that preserves sequence legality
- seed replay

### 4.4 State ownership

**v0: operator state lives directly in SQLite tables, with no in-memory cache.**

What that buys: persistence, crash recovery, transactional consistency, and multi-connection safety — all provided for free by SQLite's own WAL and locking, without writing a line. State is a plain table you can `SELECT` from, so debugging is fully transparent.

The cost is speed. But v0's goals are correctness and architecture, and this has an extra benefit: **an in-memory arrangement cache becomes a measurable optimization in M4**, with M1's baseline numbers to prove whether it is worth it, rather than assuming from the start that it is.

> **Amendment after M1a landed (2026-09-22): there is a gap between this section and M1a's implementation that has not been closed. It is recorded here so M1b does not discover it on its own.**
>
> M1a built §6.3's `Arrangement` (a `(key, val, w)` shape that maps one-to-one onto §7's `__ivm_state_<view>_<op>` table), but M1a's only stateful operator, `Aggregate`, is **not** built on it. It uses a private `BTreeMap<Row, Group>`, where `Group`'s `emitted` records "what this group last emitted". As a result:
>
> 1. **There is no path to rebuild an operator from persisted state.** `AggState` has a single constructor, `new(group_by, aggs)`, so the only way to obtain a live aggregate state is for `create_view` to replay every base table — O(base-table size) on every connection open, which is exactly the cost this section and §7.3's `__ivm_progress` watermark were designed to avoid.
> 2. `IncrementalEngine` keeps the whole materialized output in an in-memory `ZSet`, whereas in M1b it is the on-disk plain table `__ivm_out_<view>`.
>
> This is not a redesign: `Group` can be represented as `(key, val, w=1)` (`emitted` is a pure function of `rows` and `accs`). But it is **real rework**, and §6.3's rule that "no design decision may force rework when join is added" was written about join — the state ownership described in this section is the first hard constraint M1b will hit. M1b's plan must answer first: is aggregate state wired onto shadow tables, or is replaying all base tables on every connection open accepted?
>
> **A signature issue is recorded here as well:** all three methods of §6.3's `Arrangement` are infallible (`update` returns `()`). An implementation backed by SQLite tables can fail on IO or a constraint violation, and would then have only `panic!` available. **It is not changed to `Result` now** — no caller can handle an error today, so the change would only sprout `.unwrap()` everywhere, which is worse than the panic it replaces. This is to be decided when M1b writes the shadow-table implementation and its failure modes are known, the same treatment §8.5 received when it was amended in M1a Phase 1.
>
> **Join (amended 2026-09-23, M1a Phase 3):** the join is built on `Arrangement` from the start. Its two arrangements are passed in through `Node::build`'s provider (`&mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>`, with `ArrangementId` telling the two inputs apart), rather than created inside the operator, so rebuilding a join from persisted state is a provider that returns non-empty arrangements, with no interface change. Moving `Aggregate`'s state onto `Arrangement` is deliberately **not** part of Phase 3: it is a separate step after it, before M1b or as the M1b plan's first task, so that a join bug stays bisectable from that refactor.
>
> **Amended 2026-09-24, M1b Phase 1:** point 1 above — "there is no path to rebuild an operator from persisted state" — is closed at the operator level. Every stateful operator now keeps its whole state in arrangements supplied through `Node::build`'s provider, addressed by `ArrangementId { node, role }`: `node` is the operator's pre-order position in the `Plan` (root 0, children numbered after their parent, left before right), and `role` is `ArrangementRole::{JoinLeft, JoinRight, AggregateGroups}`. This also settles the join paragraph's open question: `ArrangementId` tells apart not just the two sides of one join but two joins in the same view, since each join has its own `node`. `Aggregate` now stores only its accumulators in its arrangement — `rows` plus a `(sum, non_null)` pair per aggregate, encoded as `Value::Int`s in the fixed layout `[rows, sum_0, non_null_0, sum_1, non_null_1, …]` (canonical: the same state always encodes to the same row) — and derives the row it last emitted from that state instead of storing it; a group whose state is all-zero is removed (§5.1, no zombie entries). `a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off` (`crates/ivmlite-core/src/node.rs`) pins this: a join view's whole tree — both sides of the join and the aggregate's groups — is rebuilt from mirrored copies of its arrangements, and the original and rebuilt trees are then sent the same seven batches and emit the same, exactly expected delta for each: an insert on each table (the first retracts a row only the original tree emitted; the second reads the rebuilt join's right side), a delete of two right rows, a delete of every left row that empties the group, a right-side insert with a NULL value that matches nothing, a left insert that brings the group back, and a right delete that makes SUM fall back to NULL.
>
> Point 2 above — the in-memory `view: ZSet` versus the on-disk `__ivm_out_<view>` — is still open, and still belongs to M1b Phase 3.
>
> The signature issue is also still deferred, now explicitly to M1b Phase 3, with the three questions this plan's Ruling 3 leaves open: whether errors are reported per call or per row; whether `get` can stay a lazy `Box<dyn Iterator>` given a `rusqlite::Statement`'s borrow; and what the engine does with its in-memory state after a failed `refresh`.
>
> **Open questions from the M1b Phase 1 final review**, recorded for M1b Phase 3 as questions, not decisions:
>
> - **A plan fingerprint per view.** `ArrangementId`s and the aggregate's value layout mean something only for one exact `Plan` (§7), yet a view is lowered again from its stored SQL on every load (§5.3). Should `__ivm_view` store a fingerprint of the lowered `Plan`, checked before any state table is read, so that a change to `lower` fails loudly instead of reading the wrong tables?
> - **Numbering only stateful operators.** Every plan node takes a pre-order index today, so adding a `Filter` or `Project` shifts the ids of the operators below it. Should only stateful operators (joins and aggregates) be numbered? Either way, once state is persisted, any change to the numbering needs a migration of existing state tables.
> - **A missing state table versus an empty one.** The test double `Mirrors::snapshot` returns an empty arrangement for an id that was never written. That is fine for a test, but as a real provider's policy it would turn a missing or misnamed table into a view that silently starts from nothing. How does the SQLite provider tell "this operator has no state yet" apart from "this table should exist and does not"?
> - **Explicit names for `ArrangementRole` in table names.** Should `<op>` spell each role with a fixed string chosen for the table name rather than with the enum's `Debug` output, so that renaming a variant cannot rename a table?
> - **Whether the provider itself can fail.** `Node::build`'s provider returns a `Box<dyn Arrangement>` and cannot fail, but opening a state table can. Should it return a `Result`, making `Node::build` fallible again (its doc comment already anticipates this)?
>
> **Amended 2026-09-26, M1b Phase 3a: every open question above is answered.** `Arrangement`'s three methods became fallible (`get`/`scan` return `Result<Vec<...>, StateError>`, `update` returns `Result<(), StateError>`), with reads collected **eagerly** into a `Vec` rather than through a lazy `Box<dyn Iterator>` — a lazy iterator borrowing a prepared statement would cost lifetimes and `unsafe` for no gain at this scale. The provider question is answered the same way: it is `&mut dyn FnMut(ArrangementId) -> Result<Box<dyn Arrangement>, StateError>`, so `Node::build` is fallible too. The plan-fingerprint question is answered: `Plan::canonical` renders a plan to stable text by hand, and `__ivm_view` stores it next to a view's SQL, comparing it against a fresh `lower` on every reopen. The numbering question is answered by **keeping** pre-order numbering over every plan node, not narrowing it to stateful operators only, with `ArrangementRole` spelled as a fixed string (`join_left`, `join_right`, `agg_groups`) in a state table's name rather than the enum's `Debug` output, so renaming a variant cannot rename a table. The missing-state-table question is answered: a state table that should exist and does not is an error from the provider, never an empty arrangement. **Point 2 above — the in-memory `view: ZSet` versus an on-disk table — is closed**: the materialized output lives in the plain table `__ivm_out_<view>`.
>
> The M1b Phase 2a review's open question about `node.rs`'s `passes` (below) is answered by two measures together: every value read out of a base or delta table is decoded strictly — one that is not INTEGER, TEXT or NULL is an error (the extension's own `String` error, which the failing create or refresh reports as an SQLite error), never silently coerced — and every extension callback runs inside `catch_unwind`, so a panic that still reaches `passes` becomes a reported SQLite error rather than a host-process abort. See the Phase 3a spec §3–§5 for the exact interfaces.

> **Open question from the M1b Phase 2a final review**, recorded for M1b Phase 3:
>
> - **`node.rs`'s `passes` has an `unreachable!` on a mixed-type comparison.** It holds today because `lower` admits only a literal of the column's declared type and a STRICT table stores only that type. M1b Phase 3's SQLite extension must decode rows strictly by declared type and catch panics at the FFI boundary — otherwise a corrupt row (one that somehow carries a mismatched cell) turns that `unreachable!` into a host-process abort instead of a reported error.

> **Open question from the M1b Phase 2b final review**, recorded for M1b Phase 3:
>
> - **`SqlError` is a string.** `ivmlite-sql`'s `SqlError(pub String)` flattens every failure into one message, and `From<CatalogError>` flattens the catalog's error into it too. Phase 3 may need to tell a transient SQLite error (e.g. `SQLITE_BUSY`) from an unsupported table, which would make `SqlError` an enum. Decided once the SQLite catalog exists.

---

## 5. Data model and plan IR

### 5.1 Z-sets

A row's weight is an `i64`. INSERT = `+1`, DELETE = `-1`, UPDATE = two rows, `-1` (OLD) and `+1` (NEW).

A state update is Z-set addition (the weights of the same row are added).

**Weight invariants** (asserted in the differential tests):

- Negative weights in intermediate deltas are normal.
- **Negative weights are not allowed in the final materialized state** — one appearing is a bug.
- **A row whose weight reaches zero must be removed from the state**; no zombie `w = 0` rows may remain, or `COUNT(*)` and memory use both drift.

### 5.2 Plan IR

```rust
enum Plan {
    Scan      { table: TableId, columns: Vec<ColumnId> },
    Filter    { input: Box<Plan>, predicate: Expr },
    Project   { input: Box<Plan>, exprs: Vec<Expr> },
    Aggregate { input: Box<Plan>, group_by: Vec<Expr>, aggs: Vec<AggSpec> },
    Join      { left: Box<Plan>, right: Box<Plan>, on: Vec<(Expr, Expr)> },  // M1a
}
```

v0's `Join` carries one pair of column indices (`left_key`, `right_key`) rather than `Vec<(Expr, Expr)>`, for the same reason `Expr` is not introduced elsewhere: v0 joins on exactly one pair of bare columns, so `Expr` would be an empty shell with a single `Column` variant, and introducing it once a feature genuinely needs expressions is a local change.

M0 implements no operators (M0 has no engine). M1a implements all five: first `Scan` / `Filter` / `Project` / `Aggregate` (the checkpoint: single-table differential tests green), then `Join`.

#### v0's root operator must be an Aggregate with a non-empty GROUP BY

This constraint closes a subtle semantic hole. The materialized output table carries a `__w` weight column, but **weights are an internal representation — SQL tables have no notion of weight**:

```sql
-- if Scan → Filter → Project were allowed to be a view
source data:   apple, apple, banana
Z-set form:    (apple, w=2), (banana, w=1)     -- 2 rows
user SELECT:   should see 3 rows
```

The materialized table would show 2 rows where a plain SQL view shows 3 — inconsistent semantics, when "a materialized view is just an ordinary SQL table" is this project's selling point.

**Rule: a view's root operator must be `Aggregate`, with a non-empty `group_by`.** Then each group key maps to exactly one output row, `__w` is always 1 in the final output, and Z-set weights appear only in internal deltas and operator state.

Legal: `Scan → Filter → Project → Aggregate`
Illegal: `Scan → Filter → Project` directly as a view

**Global aggregates without GROUP BY are forbidden too**, because their empty-set behaviour differs from grouped aggregation (measured):

```sql
SELECT SUM(v) FROM t;                -- empty table → 1 row (value NULL)
SELECT g, SUM(v) FROM t GROUP BY g;  -- empty table → 0 rows
```

The simple rule "delete the row when its group's count reaches zero" is wrong for the former. Rather than introduce a second set of rules for one special case, v0 simply rejects global aggregates.

Together these make v0's positioning precise: **automatically maintained aggregates**, not general materialized views.

### 5.3 Persisting view definitions

**Store the SQL text, not a serialized IR.** Re-parse it on reconnect.

That lets the IR evolve freely with no data migration. Turso is currently stuck on exactly this — "the on-disk format is unstable, and views from an older version cannot be read" — which is a ready-made lesson.

---

## 6. Operators and incrementalization

### 6.1 Three classes of operator

| Class | Operators | Delta rule | Needs state |
|---|---|---|---|
| **Linear** | Filter, Project | `Δ(f(R)) = f(ΔR)` | No |
| **Bilinear** | Join | `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS` | Yes, one per side |
| **Aggregate** | SUM, COUNT | Incrementally maintainable per group | Yes |

Linear operators come for free — deltas pass straight through, with no state. **All of v0's difficulty is concentrated in aggregation.**

MIN / MAX belong to none of these classes: deleting the current minimum requires knowing the next-smallest value, which needs a separate data structure. They are therefore scheduled for M4.

#### The NULL-semantics contract for aggregates

**`SUM` over zero non-NULL inputs returns `NULL`, not `0`.** Confirmed by measurement:

```sql
CREATE TABLE u(g TEXT, v INTEGER) STRICT;
INSERT INTO u VALUES ('a', NULL), ('a', NULL);
SELECT g, typeof(SUM(v)), COUNT(*) FROM u GROUP BY g;   -- a|null|2
```

Note that this differs from an empty group. An empty group does not appear in the output at all; a non-empty group whose **column is entirely NULL** does appear, with a positive `COUNT(*)` and a `SUM` of `NULL`.

So `SUM`'s operator state must keep both **the running sum** and **the count of non-NULL inputs**, and choose between emitting `Int` and `Null` by whether the latter is zero. An implementation that keeps only the running sum emits `0` in this case and silently disagrees with SQLite.

`COUNT(*)` is unaffected — it counts rows, regardless of whether a column is NULL.

#### Integer overflow: the same class of problem as floating-point associativity

**SQLite's `SUM` raises an error on integer overflow, and whether it does depends on scan order.** Measured:

```sql
INSERT INTO o VALUES (9223372036854775807), (9223372036854775807), (-9223372036854775807);
SELECT SUM(v) FROM o;   -- Error: integer overflow
```

The true sum is `i64::MAX`, which fits; but SQLite accumulates in order and overflows on the second step.

Incremental maintenance accumulates in an order that **necessarily** differs from a full recomputation's scan order, so "incremental succeeds, recompute errors" (or the reverse) is a reachable state. This is **the same class of problem** as floating-point addition not being associative, just on integers.

> **v0's answer: clamp the value domain so overflow is impossible, and declare overflow explicitly unsupported.**
>
> Concretely: `|the sum of all values in a group| < 2^62`. The differential-test generators must guarantee this (a narrow value domain satisfies it naturally); `ivm_create_view` does not check it statically (it cannot), behaviour on overflow is **undefined**, and the documentation says so.

This is written next to the floating-point treatment deliberately, to avoid the mistake — already made once — of "guarding only against floating point".

#### Three-valued predicate logic

`WHERE` evaluates to UNKNOWN on `NULL`, and the row is **excluded from the result**; `WHERE NOT (...)` excludes it too. Measured: with `v` in `{1, NULL, 5}`, `WHERE v > 3` matches 1 row and `WHERE NOT (v > 3)` also matches only 1 row — the two add up to 2, not 3.

So predicate evaluation must return a **three-valued** result rather than a boolean, with "false" and "unknown" merged into a single treatment for filtering (both are excluded). v0's implementation is right to merge them, but **must not conclude from this that `NOT p` is equivalent to `!p`**.

The comparison operators v0 allows are a whitelist: `>`, `>=`, `<`, `<=`, `=`, `!=`, `IS NULL`, `IS NOT NULL`. `NOT`, `OR`, `LIKE`, `IN`, `BETWEEN` and all subqueries are not allowed — each one added means re-arguing three-valued logic, and covering SQL is not v0's purpose.

**Implemented (amended 2026-09-24, M1b Phase 2a).** The whole whitelist is implemented as `Predicate::{Compare { column, op, value }, IsNull, IsNotNull}` with `CmpOp::{Gt, Ge, Lt, Le, Eq, Ne}`. A view has **at most one** predicate — `AND` is not supported either, since the whitelist is about single comparisons and each conjunction multiplies the enumerated space. The literal is always the right operand (the SQL front end normalizes `3 < v`). `NULL <op> literal` is UNKNOWN for every operator, `!=` included. TEXT compares by byte order, which is the BINARY collation of §7.1.

#### Operand types must match (amended 2026-09-22, after external review P2-1)

**`SUM` is allowed only over INTEGER columns, and a comparison between a column and a literal only when the literal's type matches the column's declared type.** Both are rejected at `ivm_create_view` otherwise. Measured against SQLite on `CREATE TABLE t(g INTEGER, v TEXT) STRICT`:

```sql
INSERT INTO t VALUES (1, '7'), (1, 'abc'), (2, '5');
SELECT g, SUM(v) FROM t GROUP BY g;   -- 1|7.0   2|5
SELECT v, v > 3 FROM t;               -- '7'|1  'abc'|1  '5'|1
SELECT '0' > 3;                       -- 1
```

`SUM` over text coerces numeric-looking values and returns a **REAL** (`7.0`), a type v0 cannot represent (§5.1 excludes REAL for floating-point associativity). An ordering comparison between TEXT and INTEGER follows SQLite's storage-class order, NULL < INTEGER/REAL < TEXT < BLOB, under which **every** text value is greater than every integer regardless of content — so `'0' > 3` is true. v0's predicate evaluation says false for both. Neither is caught by the differential tests, because the v0 query enumerator never generates these shapes; the rule therefore lives at the boundary, where any hand-built view definition passes. `IS NULL` / `IS NOT NULL` are type-agnostic and stay legal on any column.

Supporting cross-type comparisons later would mean implementing storage-class ordering, not a string comparison: converting the literal to text and comparing strings gives `'0' > '3'` = false, the opposite of SQLite.

**Amended 2026-09-24, M1b Phase 2a:** the rule now covers all six comparison operators and both column types: a comparison's literal must have its column's declared type, since SQLite would convert a mismatched literal by the column's affinity (a TEXT column compared with `3` compares with `'3'`). A NULL literal is rejected too — `v = NULL` is UNKNOWN for every row — with an error pointing at `IS NULL`.

#### Join keys (amended 2026-09-23, M1a Phase 3)

**A join's two key columns must have the same declared type**; a join on keys of different types is rejected at `ivm_create_view`. **A row whose key is NULL matches nothing**, not even another NULL. Both rules follow SQLite, measured on `STRICT` tables `t0(k TEXT, v INTEGER)` / `t1(k TEXT, v INTEGER)`:

```sql
INSERT INTO t0 VALUES ('7',1),('v7',2);  INSERT INTO t1 VALUES ('x',7);
SELECT t0.k, t1.v FROM t0 JOIN t1 ON t0.k = t1.v;   -- 7|7   (TEXT '7' = INTEGER 7 is true)
-- with NULL keys on both sides:
INSERT INTO t0 VALUES (NULL,3);  INSERT INTO t1 VALUES (NULL,4);
SELECT COUNT(*) FROM t0 JOIN t1 ON t0.k = t1.k WHERE t0.k IS NULL;   -- 0
```

SQLite compares an INTEGER column with a TEXT column under numeric affinity, so `'7' = 7` is true there, while v0 compares values exactly — the same class of problem as the operand-type rule above, and settled the same way, at the boundary. `NULL = NULL` is UNKNOWN (the three-valued logic of this section), so the join neither stores nor probes a row whose key is NULL.

### 6.2 Retraction semantics for aggregates

**This is IVM's biggest source of bugs and must be followed strictly.**

When a group's `SUM` changes from 100 to 150, the output delta is **not** `+1 row (region, 150)`, but:

```
(region, 100)  weight −1     ← retract the old output row
(region, 150)  weight +1     ← emit the new output row
```

An aggregate operator must remember **what it last emitted** in order to retract it. That is the real reason aggregation needs state. So `Aggregate`'s state holds both the accumulators `(sum, count)` and **the output row currently emitted**.

> **Amended 2026-09-24, M1b Phase 1:** the output row is no longer stored. It is still what the operator retracts, but it is now derived — recomputed from the accumulators — rather than kept as a separate field. This works because the row is a pure function of the old state: `AggState` recomputes it from the accumulators *before* absorbing the new delta, whenever it needs to retract. That is also what makes the state rebuildable from an arrangement alone (§4.4's M1b Phase 1 amendment): a rebuilt tree can retract what the previous tree emitted without ever having seen that emission.

### 6.3 The Arrangement trait

```rust
trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}
```

**`get` returns many values, not an `Option`.** v0's group-by stores one value per key and does not need many; but each side of a join is key → many rows.

> **Implementation note**: `Box<dyn Iterator>` is used here deliberately instead of RPITIT (`-> impl Iterator`). `ivmlite-core`'s operators must hold an `Arrangement` implementation supplied by `ivmlite-sqlite`; with RPITIT the trait is not object-safe, `dyn Arrangement` is impossible, and type parameters are forced to propagate through the whole operator tree. The boxed iterator's overhead is negligible in v0, where state lives in SQLite tables and every access does IO anyway. If M4's in-memory arrangement shows the boxing to be a bottleneck, switch to generic parameters then — by that point the operator tree is stable and the change is contained.

> **Constraint: M0 must not make any design decision that would force rework when join is added.** `Arrangement`'s key → many-values shape is where this constraint mainly lands. (Join was originally scheduled for M2 and has since moved up to M1a — the constraint was written for it in the first place, and moving join earlier only means it pays off sooner.)

The trait is defined in `ivmlite-core`; its implementation is supplied by `ivmlite-sqlite` (v0 = shadow tables).

---

## 7. State representation (shadow table schema)

```sql
__ivm_view(name TEXT PRIMARY KEY, sql TEXT)              -- view definitions, stored as SQL text
__ivm_dep(view TEXT, tbl TEXT, PRIMARY KEY(view, tbl))   -- which base tables each view depends on
__ivm_delta_<table>(seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    w INTEGER, <all of the table's columns...>)   -- CDC; w is the Z-set weight
__ivm_state_<view>_<op>(key BLOB, val BLOB, w INTEGER,
                        PRIMARY KEY(key, val))            -- arrangement
__ivm_out_<view>(<output columns...>, __w INTEGER)        -- materialized output, a plain table
__ivm_progress(view TEXT, tbl TEXT, applied_seq INTEGER,
               PRIMARY KEY(view, tbl))                    -- watermarks
```

`__ivm_out_<view>` is a **plain table**, readable with `SELECT` even without the extension loaded.

`<op>` in `__ivm_state_<view>_<op>` corresponds to an `ArrangementId` (§4.4's M1b Phase 1 amendment): the operator's pre-order index in the `Plan` plus its role, one table per `(node, role)`. Both the ids and the aggregate's positional value layout (`[rows, sum_0, non_null_0, …]`, one pair per entry of `aggs`, in order) are meaningful only for one exact `Plan`. A view is stored as SQL and lowered again on load (§5.3), so a change to `lower` that moves a node or reorders `aggs` would point an existing view's state at the wrong tables or read it with the wrong layout — often silently, since state of the right shape read by the wrong operator (one join side's rows as another's), or aggregate values of the right length in the wrong order, still decode. M1b Phase 3 must guard this before any persisted state is read, for example with a plan fingerprint stored per view (§4.4's open questions).

> **Amended 2026-09-26, M1b Phase 3a:** the table list above gains `declaration` in `__ivm_view` — the `CREATE TABLE` statement the virtual table was declared with, kept so a view whose stored SQL no longer compiles can still be reconnected to and dropped — and a new global table, `__ivm_meta`, recording the shadow-table format version. Each view also gets its own `__ivm_stage_<view>` table and an apply trigger on it: a refresh writes every state, output and watermark change there first, then applies them all with one `UPDATE ... SET armed = 1` statement, so they commit or roll back together. The exact column list of every table, including the delta table's own `__ivm_seq`/`__ivm_w` columns, is in the Phase 3a spec §4.

### 7.1 Two semantic traps in row encoding

**Trap one: in SQLite `1 = 1.0` is true, but INTEGER and REAL are different storage classes.** If they encode to different BLOBs, a group key that is one value in SQL terms splits into two groups.

> **Answer: require STRICT tables, and additionally reject `ANY` columns explicitly; and v0 allows only bare columns as group-by keys, not expressions** (an expression can still produce mixed types).
>
> **STRICT alone does not pin a column's type** — STRICT tables allow `ANY` columns, which store values as given and may differ in type from row to row. Confirmed by measurement:
>
> ```sql
> CREATE TABLE t(a ANY) STRICT;
> INSERT INTO t VALUES (1), ('1');
> SELECT count(*) FROM (SELECT a FROM t GROUP BY a);  -- 2
> ```
>
> So `ivm_create_view` must walk `PRAGMA table_info`'s `type` field and reject `ANY` outright. The column-type whitelist v0 accepts is `INTEGER` and `TEXT` (`REAL` is excluded because of floating-point associativity; `BLOB` is scheduled for M4).

**Trap two: collation.** If `GROUP BY name` runs over a column with `COLLATE NOCASE`, an encoding that ignores it will disagree with SQLite's grouping.

> **Answer: v0 supports only the BINARY collation and rejects everything else in `ivm_create_view`.**
>
> **`PRAGMA table_info` cannot see collation** — it returns only `cid, name, type, notnull, dflt_value, pk`, with no collation field (confirmed by measurement). A column's declared collation can be obtained only from the DDL itself. So the check is:
>
> ```sql
> SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?;
> ```
>
> Fetch the table's CREATE statement and reject the table if `COLLATE` appears anywhere in it (case-insensitive match). This over-rejects conservatively — `COLLATE` may appear somewhere unrelated to the group-by columns — but v0 would rather wrongly reject than wrongly accept: missing one NOCASE column makes the materialized result silently disagree with SQLite, and the differential tests will not necessarily cover a user's real collation configuration. Column-precise checking is scheduled for M3.

The encoding must be **canonical**: the same logical row must always encode to exactly the same byte sequence.

### 7.2 Garbage-collecting delta tables

The pure-SQL trigger design means even connections that never loaded the extension are captured (§8.1). The cost is that **delta tables grow without bound**. A design without GC looks fine in a benchmark and blows up the moment it meets a real workload — delta tables grow to tens or hundreds of millions of rows.

The GC watermark is set by **the most lagging of all views that depend on the base table**:

```sql
-- for each base table
gc_watermark(tbl) = (SELECT MIN(p.applied_seq)
                     FROM __ivm_progress p
                     JOIN __ivm_dep d ON d.view = p.view AND d.tbl = p.tbl
                     WHERE p.tbl = tbl);

DELETE FROM __ivm_delta_<tbl> WHERE seq <= gc_watermark(tbl);
```

This query is the sole reason `__ivm_dep` exists: without it there is no way to know "who has not finished consuming yet".

**When no view depends on a tracked table any more** (the last view was dropped), that table's triggers and delta table are dropped with it, rather than letting deltas accumulate forever.

> **Amended 2026-09-27, M1b Phase 3b §5:** GC is not a separate step. It is one `DELETE` added directly inside the view's apply trigger (§8.1's `__ivm_apply_<view>`, amended by the Phase 3a §5 note below), run against `MIN(applied_seq)` over `__ivm_progress` for that base table, so it commits atomically with that refresh's state, output and watermark or not at all. See the Phase 3b spec §5 for the exact statement.

### 7.3 Bootstrap must be atomic with the delta watermark

When a view is created on a table that already has data, getting the order wrong loses updates or applies them twice:

```
1. Read the delta table's high watermark H
2. Scan the base table in full and compute the initial state
3. Set progress = H
```

**Steps 1 and 2 must happen in the same read transaction.** Otherwise a concurrent write between them either gets recorded under progress H but is absent from the base-table snapshot (a lost update), or is present in the snapshot and then applied again by a delta with seq > H (a duplicate).

SQLite read transactions provide a consistent snapshot, so wrapping the two steps in a `BEGIN` is enough. This must be written into the implementation, not just the documentation — it belongs to the class of "invisible in a benchmark, blows up immediately under a real workload".

---

## 8. CDC and when maintenance happens

### 8.1 Triggers (pure SQL, no UDFs)

```sql
CREATE TRIGGER __ivm_orders_ins AFTER INSERT ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (+1, NEW.region, NEW.amount);
END;

CREATE TRIGGER __ivm_orders_del AFTER DELETE ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (-1, OLD.region, OLD.amount);
END;

CREATE TRIGGER __ivm_orders_upd AFTER UPDATE ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (-1, OLD.region, OLD.amount);
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (+1, NEW.region, NEW.amount);
END;
```

UPDATE is split into a retract plus an insert, so **what is written to the delta table is already a Z-set** and needs no further conversion.

**The key property of pure-SQL triggers: even a process that never loaded this extension has its writes captured.** The delta table keeps accumulating, and the next connection that has the extension loaded can catch up. Hooks cannot do this.

v0 captures **every column** of a table and does no pruning. That wastes space on wide tables, but avoids "adding a view requires changing the delta table's structure".

> **Amended 2026-09-27, M1b Phase 3b §6:** these three triggers become five, shared by every view of the table rather than owned by one, and gain a REPLACE-conflict capture mechanism that needs no `PRAGMA recursive_triggers`: a BEFORE trigger records the rows a write could replace in a shadow "pend" table, and the matching AFTER trigger confirms which of them are actually gone. See the Phase 3b spec §6 for the exact mechanism, its schema rules and its support boundary.

### 8.2 When maintenance happens: v0 uses explicit refresh

```sql
SELECT ivm_refresh('revenue');
```

Neither automatic path suits v0:

- **Draining automatically when the vtab is read**: SQLite read transactions cannot write, so this is impossible.
- **Calling a UDF from a trigger to maintain immediately**: technically possible (it is already inside the write transaction, so it is not re-entrancy), but **SQLite has only row-level triggers, not statement-level ones** — inserting 10,000 rows would fire maintenance 10,000 times, destroying the batched-delta optimization entirely and making bulk loads unusable.

**Explicit refresh is not a compromise. It is this project's permanent API, and quite possibly the right abstraction.**

Three reasons:

1. **Both automatic paths are blocked by SQLite's own mechanisms** (above); there is no "find a way to automate it later".
2. **It is the precondition for the test matrix** — batch independence (§9.1) is testable only when the moment of maintenance can be controlled precisely.
3. **Batching is itself where the performance advantage over hand-written triggers comes from**; see below.

The recommended usage puts the refresh inside the application's own write transaction:

```sql
BEGIN;
INSERT INTO orders ...;   -- ×10000
SELECT ivm_refresh('revenue');
COMMIT;
```

#### Why batching is an advantage, not a defect

A hand-written row-level trigger necessarily runs 10,000 aggregate UPDATEs for 10,000 inserted rows. An incremental engine handed the whole batch of deltas can **consolidate** first:

```
10000 raw Δ
      ↓  Z-set consolidation (weights of identical rows added)
if only 20 regions are touched
      ↓
20 group-state updates
```

**This is what explicit refresh buys, and what row-level triggers structurally cannot have** — and it is the performance story most likely to hold up for this project. Consolidation is therefore explicitly M1 content, not an optimization.

> **Amended 2026-09-26, M1b Phase 3a:** a refresh inside a transaction, as recommended above, is atomic by **one-statement apply, not a `SAVEPOINT`** — measured while prototyping that the obvious design does not work. Inside `xUpdate` a `SAVEPOINT` fails with `cannot open savepoint - SQL statements in progress`; and if `xUpdate` returns an error inside an explicit transaction, SQLite does not undo the writes the callback already made through its own statements. So a refresh instead buffers every state write in memory, overlaying it on what later reads see; writes every buffered state change, output change and new watermark into the view's stage table (emptied first); and applies them all with **one** statement, `UPDATE __ivm_stage_<view> SET armed = 1`, whose trigger performs each row's change. A single statement is atomic on its own — if any row of that step fails, SQLite rolls the whole statement back — so state, output and watermarks commit or roll back together whether or not the surrounding `BEGIN…COMMIT` itself succeeds. See the Phase 3a spec §5 for the full lifecycle.

### 8.3 Control surface: settled by M-1 — a virtual table plus a command channel (the FTS5 idiom)

An early draft made the control surface scalar UDFs:

```sql
SELECT ivm_create_view('revenue', 'SELECT ...');   -- creates tables and triggers internally
SELECT ivm_refresh('revenue');                     -- writes shadow tables internally
```

That is, doing DDL and writes on the same connection from inside a `SELECT` statement that is mid-`sqlite3_step()`. This **cannot be bet on "it ought to work in theory"** — SQLite restricts re-entrancy from hooks tightly (commit and update hooks explicitly forbid touching the connection that fired them from inside the callback), and although application-defined functions are held to looser rules, they still re-enter the same connection from inside a running statement.

**The M-1 spike ran a 10-scenario matrix for both scalar UDFs (design A) and the virtual-table command channel (design B) on a real cdylib loadable extension** (rollback, nested transactions, WAL, two connections, concurrency, the teardown path, a connection without the extension loaded, and design A's own "called during a scan"). The results are in [2026-09-18-m-1-results.md](../../spikes/2026-09-18-m-1-results.md). Conclusion: **design B is green on every scenario that applies to it; design B is adopted, and the control-surface syntax is settled.**

```sql
-- xCreate creates the shadow tables and triggers; the DDL context is correct by construction
CREATE VIRTUAL TABLE revenue USING ivm(
    'SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region'
);

-- the command channel: write to the column named after the table
INSERT INTO revenue(revenue) VALUES ('refresh');

-- xFilter reads the materialized state
SELECT * FROM revenue;

-- xDestroy: drop the triggers first, then the shadow tables, in an order the extension controls
DROP TABLE revenue;
```

For reference: `CREATE VIRTUAL TABLE docs USING fts5(body)` creates five shadow tables through xCreate — `docs_data`, `docs_idx`, `docs_content`, `docs_docsize`, `docs_config` — and `INSERT INTO docs(docs) VALUES('rebuild')` is its command entry point.

Design B beats scalar UDFs in three places, and M-1 measured support for all three:

1. **DDL happens in the context SQLite designed for it** (`xCreate`) — scenarios 1–5 confirm that creating tables and triggers and writing all succeed in a bare call, an explicit transaction, a nested savepoint, and WAL mode; and on rollback (scenarios 3/4) the shadow tables and triggers disappear with the transaction, leaving no orphans.
2. **The view becomes a real object that `sqlite_master` knows** — the `CREATE VIRTUAL TABLE` statement itself is in `sqlite_master`, and `SELECT * FROM revenue` reads normally through `xFilter`.
3. **`DROP TABLE revenue` cleans up the shadow tables and triggers naturally through `xDestroy`, with no separate teardown API** — scenario 9 shows that the scalar-UDF design lacks this property: a `DROP TABLE` on a shadow table does not cascade to the trigger pointing at it, and the dangling trigger makes every later write to **the user's own base table** fail with `no such table`. In the virtual-table design, `xDestroy` controls the order itself (triggers first, then shadow tables), and the base table stays writable after teardown.

M-1 also found a problem in design A more serious than scenario 9, whose concrete form the probe document had not anticipated (scenario 8): calling `ivm_refresh` to write a shadow table from inside a statement scanning that same table (a self-reference) makes the cursor keep seeing the rows it just inserted, and it falls into an unbounded loop that neither errors nor slows down; it was interrupted by hand at over 460,000 rows. Design B does not expose this path structurally — reads go through `xFilter` (no writes), and writes go through a separate `xUpdate` statement (not from inside a scan callback).

Under concurrency (scenarios 6/7: another connection reading or writing at the same time) the two designs behave exactly symmetrically, following standard SQLite locking (`SQLITE_BUSY` under rollback-journal; under WAL, readers do not block writers and writers exclude writers), which gives no additional signal for the choice. Scenario 10 confirms §8.1's claim: pure-SQL triggers work for connections that never loaded the extension.

**The control surface is now settled. Wherever `ivm_create_view` / `ivm_refresh` appear in the rest of this document, they mean the equivalent operations in the virtual-table command-channel form** (`CREATE VIRTUAL TABLE ... USING ivm(...)` / `INSERT INTO v(v) VALUES('refresh')`), not undecided syntax.

### 8.4 Guard rail: never trigger maintenance from inside a statement scanning its target table

M-1's scenario 8 measured a **silent** catastrophic behaviour under design A (scalar UDFs): calling maintenance logic that writes to `orders` from inside a `SELECT` that is scanning `orders` produces an unbounded self-referential loop — no error, no rollback, running on until it was killed by hand at about 464,000 rows.

With design B (virtual table + command channel) chosen, this path is unreachable in normal use: maintenance is triggered by `INSERT INTO v(v) VALUES('refresh')`, not by the evaluation of some `SELECT`. It is still recorded as a guard rail, for two reasons:

1. **The failure is silent.** It does not show up as an error code or a deadlock, but as "why hasn't this statement come back". Without this paragraph, whoever hits it in future would not think to look in the direction of self-referential scans.
2. **Design A might be reconsidered.** If scalar UDFs are revisited one day for distribution or compatibility reasons, this is one of the two measured reasons they were ruled out (the other is the teardown path leaving orphan triggers that poison the base table; see §8.3).

> **Rule: a maintenance operation must never be triggered during the evaluation of a statement that is reading the table the maintenance writes to.** At the implementation level, the control surface must make maintenance happen at a statement boundary (the command channel's `INSERT` is a statement of its own), never during the evaluation of an expression.

### 8.5 The engine seam contract

M1's engine plugs into the differential testing harness through `ivmlite-test`'s `Engine` trait. The trait's shape is not an implementation detail — it decides which behaviours **can be tested at all**:

```rust
fn create_view(&mut self, &Database, &ViewQuery, initial: &BTreeMap<String, ZSet>) -> Result<(), EngineError>;
fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>;
fn refresh(&mut self) -> Result<(), EngineError>;
fn materialize(&mut self) -> Result<ZSet, EngineError>;
```

Four constraints, with their reasons:

**`apply` receives raw, unconsolidated deltas, not a `ZSet`.** The same row can appear several times in one batch, and the engine must decide for itself whether to consolidate first. If the harness handed over an already-merged `ZSet`, the consolidation of §8.2 — M1 content rather than an optimization, and the performance story most likely to hold up for this project — would be **structurally invisible** at this seam: the tests would come out the same whether or not the engine did the merging. A recording engine in the harness guards this, feeding a batch that contains duplicate rows and asserting that it receives several entries rather than one merged entry.

**`refresh` is separate from `apply`.** §8.2 makes explicit refresh a permanent API rather than a temporary compromise, and §9.1's batch independence is testable only when the moment of maintenance is controllable. The reference implementation `NaiveRecompute` therefore **really** has two phases: `apply` only accumulates pending deltas, and only `refresh` merges them into the base. If `apply` merged eagerly, `refresh` would be a no-op, and no engine that ignored the contract would be caught.

**`apply` carries a table name.** §6.3 forbids decisions that would force rework for join, and join needs several base tables (join is now scheduled in M1a). `materialize` deliberately does **not** carry a view identifier: multiple views arrive only with M4's cascading views, in an undecided form, and adding it now would be speculation; multiple tables are a known, scheduled requirement, and now is the cheapest moment to add a parameter.

**`create_view` receives the whole `Database` and each table's initial state** (amended in M1a Phase 1, 2026-09-21). The original signature was `(&Schema, &ZSet)`, with the single-table assumption written into the type. Join must bootstrap **both sides**, and for an engine that can see only one table's initial state, the question "did it bootstrap the second table too?" **cannot be asked** at the seam — exactly the same failure mode as the first constraint: the tests come out the same whether or not the engine did it.

> This constraint has a different origin from the three above: those were anticipated during M0's design, while this one was forced when M1a Phase 1 actually made the harness multi-table. It is recorded here because the sentence that opens §8.5 holds for it too — it decides which behaviours can be tested, so it belongs in the authoritative document, not in an item of some plan.

> `apply`'s signature is **unchanged**: it has carried a table name since M0 (the third constraint above). M1a Phase 1 is the first time that parameter actually routes to more than one table — until then, with a single table, it was vestigial. That is exactly the outcome the third constraint's "now is the cheapest moment to add a parameter" anticipated.

---

## 9. Verification

The core assertion:

```
recompute(Q, D_final) == materialize(IVM(Q, Δ₁..Δₙ))
where D_final = D₀ + Δ₁ + ... + Δₙ
```

v0 has no floating point, so this is **strict equality** (floating-point SUM is not associative, so incremental and full recomputation would not be bit-for-bit equal — a mathematical property, not a bug, whose handling is scheduled for M4).

### 9.1 Four layers of verification

| Layer | Content | Needs an oracle |
|---|---|---|
| **Invariants** | No negative weights in the final state; no `w=0` zombie rows; `applied_seq` monotonic and never past the delta table's watermark; exactly one row per group key in the output table | No |
| **Batch independence** | The same delta sequence applied all at once / one at a time / in random batches → the final states must be identical | No |
| **Primary oracle** | Full recomputation on the same connection, compared strictly **at every observable refresh point**, not only at the final state | Authoritative |
| **Cross-validation** | Turso MVs run the same query and update sequence | M2+; a disagreement means at least one side has a bug |

Batch independence is testable only when the moment of maintenance can be controlled precisely; it is impossible in an automatic-maintenance mode.

> **But its actual standing must be stated honestly: given per-batch oracle comparison, the cross-mode comparison is logically unreachable as a failure-detection path.**
>
> The argument: `run` compares against the oracle at every refresh point, so `run` succeeding ⇒ that mode's final state equals the oracle's expectation; the oracle's expectation depends only on the final base-table state, not on how the deltas were batched; so any two modes whose `run` succeeded must equal each other. The cross-mode comparison can only fail when some mode's `run` has already failed — it catches nothing `run` does not.
>
> It is kept because it costs almost nothing, and because it would become meaningful again if someone ever reduced how often `run` compares (which would be a change needing its own argument). **Do not count it as independent detection capability**, and do not invent a failing case for it that cannot exist.

**Why the oracle must compare per batch rather than only at the end**: comparing only at the end lets an implementation pass that "goes wrong midway, stays well-formed, and later recovers on its own" — the typical shape of a state-drift bug (some operator's accumulator drifts until that group is next rewritten wholesale and the error is smoothed away). The invariant layer cannot stop it, because the wrong values also satisfy "weight 1, unique group key". The cost is that test complexity goes from O(n) to O(n × base-table size), so **differential test cases must stay very small** (25 rows of initial data and 150 operations by default), with large-scale scenarios left to the benchmark rather than the correctness tests.

### 9.2 Key generator design decisions

**1. The value domain must be narrowed on purpose.** If `region` had a million distinct values, each group would have one row, and "the same group inserted into and deleted from repeatedly" would never be tested — which is exactly where retraction and zombie-row bugs come from. Squeeze the domain to 5–20 distinct values to force collisions to happen often. NULL must also appear often (NULL forms its own group under GROUP BY, a classic bug site).

**2. Update sequences must be sampled with bias.** A purely random generator catches almost no bugs in IVM testing — a random DELETE rarely hits a row that actually exists. A sizeable share of operations must **sample existing rows from the current table** to delete or modify, and the generator must be able to produce two kinds of sequence deliberately:
- deleting a row just inserted (the weight-to-zero path)
- deleting a group empty and then filling it back up (breeding ground for retraction and zombie-row bugs)

**3. v0's query space is small enough to enumerate.** Column subsets × predicates × group-by columns × aggregate functions has a finite number of combinations within v0's scope — **enumeration beats randomness**, being reproducible and complete. Randomness is left to the update sequences.

**4. The differential tests' schema is fixed at two columns per table — a precondition for the previous point, not an arbitrary number.**

The size of the enumeration is **extremely sensitive** to the number of columns, because two-column group-by combinations grow as `C(n,2)` while the number of aggregates and predicates each grow linearly with the column count, and the three multiply. Measured and extrapolated at the end of M0:

| Schema | Queries | Time for the enumeration sweep |
|---|---|---|
| One table, 2 columns (measured in M0) | 27 | 0.21s |
| Two tables, 2 columns each, + join (measured in M1a Phase 3, 2026-09-23) | 700 | 3.36s |
| One table, 2 columns, full predicate whitelist (measured in M1b Phase 2a, 2026-09-24) | 153 | 0.57s |
| Two tables, 2 columns each, + join, reduced join predicates (measured in M1b Phase 2a) | 900 | 4.13s |
| Two tables, 3 columns each, + join | 12285 | ~56s (extrapolated) |

The baseline is measured (27 queries in 0.21 seconds → about 7.8ms each). The join row is now measured too: `enumerate_join(t0, t1)` on `gen_database(2)`'s two-column tables produces 700 queries, and `incremental_engine_is_green_across_the_join_space` (one case per query, debug build) ran in a median of 3.36s across 3 runs (3.34s, 3.36s, 3.37s) — about 4.8ms per case. `incremental_engine_is_green_across_the_enumerated_space` (the single-table counterpart, 50 cases) ran in a median of 0.20s across 3 runs (0.20s, 0.20s, 0.20s) — about 4.0ms per case. Since the final review of M1a Phase 3, the join sweep also runs over a second two-table database whose right table has its two columns swapped, so the join keys sit at different positions (key pairs `(0, 1)` and `(1, 0)`); that sweep ran in 3.48s, 3.46s and 3.47s (debug build), so the join space's measured cost is now paid twice. The measured multiplier, join per-case ÷ single-table per-case, is **1.2x**, measured 2026-09-23 in M1a Phase 3 — well below the 2.5x that had been estimated. The three-column row's time above is extrapolated from the measured per-case cost of a join case (4209 × 4.8ms ≈ 20s), and remains **extrapolated**, not measured: neither its query count nor its per-case cost was verified against a real three-column enumeration, and a wider joined row may well cost more per case.

**Re-measured in M1b Phase 2a (2026-09-24), after the full predicate whitelist landed.** The join predicate set is smaller than the single-table one — one comparison and one null test per column of the joined row (Ruling 4), rather than every operator on every column — because the single-table sweep already exercises every operator once and a join query multiplies group-by × aggregates × predicates, so testing the full per-column operator set again at the join level would blow up the space for coverage the single-table sweep already gives. The comparison's operator rotates with the column, `CmpOp::ALL[column % 6]` (`Gt, Ge, Lt, Le, Eq, Ne`), so over the 4-column joined row only `>`, `>=`, `<` and `<=` appear — `=` and `!=` are covered by the single-table space; the null test is `IS NULL` on even columns and `IS NOT NULL` on odd ones. `enumerate(t0)` now produces 153 queries (up from 36 — the count before Phase 2a widened the predicate set, not M0's 27 in the table above — for the full six-operator comparison whitelist plus `IS NULL`/`IS NOT NULL` on every nullable column), and `incremental_engine_is_green_across_the_enumerated_space` ran in a median of 0.57s across 3 runs (0.60s, 0.57s, 0.57s) — about 3.7ms per case. `enumerate_join(t0, t1)` now produces 900 queries, and `incremental_engine_is_green_across_the_join_space` ran in a median of 4.13s across 3 runs (4.12s, 4.13s, 4.13s) — about 4.6ms per case; the swapped-column database's sweep, `incremental_engine_is_green_across_the_join_space_with_keys_at_different_positions`, ran in a median of 4.14s across 3 runs (4.15s, 4.12s, 4.14s). The two naive-engine join sweeps, which recompute from scratch every batch instead of incrementally, ran slower: `naive_engine_passes_every_enumerated_join_query` in a median of 5.07s across 3 runs (5.07s, 5.07s, 5.07s), and `naive_engine_passes_every_join_query_with_keys_at_different_positions` in a median of 5.13s across 3 runs (5.29s, 5.13s, 5.07s). The single-table naive sweep, `naive_engine_passes_every_enumerated_query`, ran in a median of 0.93s across 3 runs (0.93s, 0.93s, 0.93s). The three-column row in the table above is now **counted**, not merely projected: two `(k TEXT, v INTEGER, w INTEGER)` tables, all columns nullable, give `enumerate_join(...).len()` = 12285 (measured directly with a scratch test, not kept in the tree). Its time is still **extrapolated** — from the newly measured per-case join cost, 4.13s / 900 ≈ 4.59ms per case: 12285 × 4.59ms ≈ 56s (extrapolated).

At two columns the enumeration holds and its cost is a few seconds; at three a single test would take about 56 seconds (extrapolated, see above), roughly fourteen times the measured join sweep. It was also confirmed by measurement that there are **five** enumeration sweeps, each **an isolated test on its own**: the single-table sweep (`incremental_engine_is_green_across_the_enumerated_space`, median 0.57s as of M1b Phase 2a) and two databases × two engines of join sweeps. Over the same-position database (`gen_database(2)`): `incremental_engine_is_green_across_the_join_space` (median 4.13s as of M1b Phase 2a) and `naive_engine_passes_every_enumerated_join_query` (median 5.07s as of M1b Phase 2a; previously 4.09s across 3 debug runs — 4.08s, 4.09s, 4.15s, measured 2026-09-24 before Phase 2a). Over the swapped-column database: `incremental_engine_is_green_across_the_join_space_with_keys_at_different_positions` (median 4.14s as of M1b Phase 2a) and `naive_engine_passes_every_join_query_with_keys_at_different_positions` (median 5.13s as of M1b Phase 2a; previously 4.14s across 3 debug runs — 4.11s, 4.14s, 4.17s, measured 2026-09-24 before Phase 2a). The 50-seed detection test (0.07s) and the shrinker (0.10s) still take only one query per case and do not grow with the enumeration, so the cost does not compound across tests.

> **An open question, to be decided with measured numbers**: the only shape three columns add to coverage is "three columns with three distinct roles" — sum over column A, group by column B, filter on column C. With two columns, at least two roles share one column. "The filtered column is not the aggregated column" is a plausible bug site (is the predicate evaluated against the right column index?). Whether that is worth a roughly 13.6x larger join enumeration (12285 / 900, computed) is still open.
>
> The measured join multiplier now exists (M1a Phase 3, 2026-09-23: 1.2x, see above) but the three-column decision itself is still open.
>
> If the decision is then not to widen, there is one knob already worked out: restrict join queries' group-by to a single column, which brings the join space down from 900 queries to 360 (2 key pairs × 4 single-column group-bys × 5 aggregate sets × 9 predicates) — about 1.65s instead of the measured 4.13s, a figure **computed** from the measured 4.13s / 900 queries, not measured. The cost is not testing joins "grouped by one column from each side" — and multi-column group keys that cross the boundary between two tables are exactly where joins are most likely to have bugs, so this knob should be the last one turned.

**Decided (M1b Phase 2b, 2026-09-24): not widening.** The one shape three columns would add — a view that groups by one column, sums a second and filters on a third — is already enumerated: a join's row has four columns (for example `GROUP BY t0.k`, `SUM(t0.v)`, `WHERE t1.v …`), and a single-table view runs the same `Filter → Project → Aggregate` chain regardless of how many columns its table has. Widening would cost 12285 join queries per database, about 56 seconds per sweep (extrapolated, as recorded in the Phase 2a paragraph above) — for coverage the existing space already gives.

### 9.3 Shrinking

**It must be written in-house; `proptest` cannot be used directly.** Naive sequence shrinking produces **illegal sequences** (remove an INSERT, and a later DELETE aimed at that row is left dangling). What is needed is a delta-debugging shrinker that preserves sequence legality: shrink the update sequence first, then the query, then the data.

Without shrinking, a failure means facing a sequence of thousands of steps that cannot be debugged.

### 9.4 Reproducibility

All randomness goes through a seed; a failure prints its seed and can be replayed with one command; failing cases are frozen into `tests/regressions/` as permanent regression tests.

### 9.5 Test layers

- **L0**: `ivmlite-core` unit tests, no SQLite, hand-built Z-sets fed to operators
- **L1**: differential tests, one view, enumerated queries × random update sequences
- **L2**: multiple views, cascading views (M2+)
- **L3**: Turso cross-validation (M2+)

---

## 10. Benchmark

> **Amended 2026-09-28, M1b Phase 4:** the extension has now been measured
> against the M0 baselines. Results, confirmed boundary cells, ablations,
> write amplification, space amplification, and limits are in
> [`docs/bench/README.md`](../../bench/README.md), section “M1b Phase 4: the
> SQLite extension.”

### 10.1 The main benchmark

```
N views (N = 1, 10, 50, 200)
  × base-table size (10k, 100k, 1M rows)
  × delta batch size (1, 10, 100, 1000 rows)
  × group cardinality (10, 1k, 100k distinct group keys)
→ measure: the time to apply one batch of deltas and bring every view up to date
```

**Group cardinality is the first-order parameter for whether IVM wins, more decisive than the base-table size itself**, so it must be an explicit dimension rather than a hard-coded constant:

| Configuration | Consequence |
|---|---|
| 10 groups / 1M rows | The view has only 10 rows, the state is tiny, every delta hits a hot group, and IVM's advantage is enormous |
| 1M groups / 1M rows | The view is as large as the base table, IVM's state is as large as the data, every delta creates a new group plus retraction churn, and the advantage essentially disappears |

The crossover moves sharply with this parameter. **Reporting numbers at a single group cardinality amounts to picking a flattering point**, and is not a conclusion.

A full four-way cross is 144 configurations, too many. The convention: **fix the view count at 10 while sweeping group cardinality**, without a full cross; the view-count sweep runs separately at group cardinality = 1k.

### 10.2 Baselines, layered by role

| Role | Baseline | Notes |
|---|---|---|
| **Lower bound** | Write the base table, maintain nothing | Pure write cost |
| **Skeptic** | Summary tables maintained by hand-written triggers | See the three-tier bar below — **not "must beat"** |
| **Baseline** | Naive recompute | The crossover is measured here |
| **Peer** | Turso MVs | Same host, same DBSP — the fairest comparison |
| **Ceiling** | The `dbsp` crate run bare (a hand-built circuit, no SQL, nothing on disk) | The gap to this project is the SQLite / storage tax, of great diagnostic value |
| **Reference** | duckDBSP / OpenIVM / pg_ivm | Cross-host, background only, never a verdict |

The "skeptic" row answers the most direct challenge to v0: single-table GROUP BY + SUM/COUNT is exactly the trigger-maintained summary table people have written by hand for thirty years. It must be answered head-on — but **the bar is not "must beat it"**.

#### The three-tier bar against hand-written triggers

An early draft said "v0 must beat hand-written triggers or there is no story". **That bar is wrong.** A special-purpose trigger written by hand for `GROUP BY region → SUM(amount)` is itself one of the best implementations of that query, compiled by hand, while a general engine must pay for its generality: a generic delta representation, serialization, arrangement lookups, operator dispatch, progress tracking, the CDC log. **Not beating it does not mean having no value.**

The right bar has three tiers:

| Tier | Bar | Meaning |
|---|---|---|
| **Must** | `ivmlite ≪ full recompute` | If not met, the project's premise does not hold |
| **Expected** | `ivmlite` close to hand-written triggers | The price of generality is acceptable |
| **Bonus** | `ivmlite` **faster than** hand-written row-level triggers on large Δ | The structural advantage consolidation brings |

> **Amended 2026-10-08, M1b Phase 5:** measured against these three tiers after Phase 5's refresh work, in [`docs/bench/README.md`](../../bench/README.md), "M1b Phase 5":
> - **Must:** met at 100,000 base rows and above. All 48 such cells were above 2x, and the small-table corner still loses.
> - **Expected:** still not met. In the confirmed cells, ivmlite's apply + refresh is 1.81x the hand-written trigger's at best (10 groups, batch 1,000), 6.64–17.55x with batches of 100 or more, and far more with tiny batches at 200 views.
> - **Bonus:** not observed in any cell or repeat.
>
> The remaining cost is the maintenance itself: the trigger body's B-tree work in the state and output tables is about 40% of refresh, and the operator tree about 27% (Phase 5 spec §6).

The third tier is reachable, and the opportunity comes precisely from the batching semantics argued in §8.2: if 10,000 inserts touch only 20 regions, a hand-written row-level trigger runs 10,000 aggregate UPDATEs, while an engine handed the whole batch needs only 20 after consolidating.

**So the benchmark must include the "large Δ + low group cardinality" cell** — the only place the third tier can show up, and, once hand-written triggers are correctly positioned as a "special-purpose upper bound" rather than a "threshold that must be crossed", the thing genuinely worth measuring.

### 10.3 Methodological constraints

1. **Absolute times across hosts are not comparable** (DuckDB vs SQLite measures the host, not IVM). Across hosts, compare only **the within-host speedup** `naive recompute / incremental`.
2. **CI keeps only same-host baselines**; cross-host comparisons are a one-off writeup, not in CI (otherwise they are bound to rot).
3. **Every baseline must maintain exactly the same set of views.** If "hand-written triggers" can express only one shape while "naive recompute" runs a different set of queries, the measured ratios cannot support §10.4's conclusions. The view set is capped by **the least expressive baseline** — for v0, `GROUP BY <one column> → SUM, COUNT` — and every baseline uses N copies of that shape.
4. **Every baseline must finish building its initial state before timing starts.** Creating an empty summary table after the base table already has data yields a view that is never complete, with unrepresentative maintenance cost. Each baseline performs one full bootstrap first, then measures incremental cost.
5. **Test data must have a stable primary key, and deletes and updates must locate rows by it.** Finding rows by the values of all columns degrades to a full table scan (`EXPLAIN QUERY PLAN` shows `SCAN`), making time grow linearly with the base table — and the base-table size is the one thing this benchmark exists to show, so once it is drowned by the scan the conclusion is worth nothing.
6. **Never put numbers published by other projects into a comparison table.** Different hardware, data, queries, and measurement methods — milliseconds from someone else's blog or paper are not comparable with this project's numbers. Comparing with Turso or any other system means running it yourself on the same machine with the same workload. Other people's published numbers have exactly one legitimate use: judging whether your own order of magnitude is so far off that something must be wrong — never as a conclusion.
7. **Workloads must be portable artifacts, not hard-coded in a runner.** The schema DDL, view SQL, data-generation parameters and update trace are defined in a standalone file that the generator can export as CSV/SQL for any engine to load. There is one runner per engine and only one workload. Otherwise every system brought in for comparison means redesigning the benchmark, and redesigned benchmarks are not comparable with each other. The same constraint applies to **the cell-derivation rules themselves**: if the measured matrix (how many base-table sizes, batch sizes, view counts and group cardinalities to sweep, and how to build a concrete configuration from those dimensions) lived in runner code, an external runner would have to re-implement that derivation and guess every field right — the same failure as a hard-coded workload, one layer up. So the cell-derivation rules belong to the workload definition itself, not to any one runner.

### 10.4 The conclusion to reach

IVM's time should grow with **Δ size** and hardly at all with **base-table size**; naive recompute grows linearly with base-table size.

**The output is not a number but a surface:**

```
base-table size × Δ size × group cardinality × view count × refresh frequency
                      ↓
       full-recompute time / incremental time
```

An early draft stipulated in advance that "a crossover above one million rows makes it pointless for SQLite". **That threshold has been removed** — it was made up, and it squashed a five-dimensional problem into one number. The same implementation reaches two completely different conclusions under "10 groups, large Δ" and "900,000 groups, one-row Δ"; there is no single crossover.

> **The benchmark must still be able to falsify this project; the bar is just a shape rather than a number:**
>
> **If there is no region anywhere on this surface where incremental maintenance has a substantial advantage over full recomputation (a ratio > 2), the project's premise does not hold.** Conversely, if an advantage region exists, report honestly where it falls — including the conclusion "it holds only in one narrow corner".
>
> A benchmark that can only reach good conclusions is worthless; so is one that stipulates in advance what a good conclusion looks like.

### 10.5 Secondary metrics

- **Write amplification** — once the extension is installed, triggers make **every** write slower, even if the views are never read. This is IVM's hidden tax and must be quantified, or the benefit numbers are fake.
- Space amplification: state + delta tables vs the base tables
- Bootstrap time

### 10.6 Known simplifications (accepted in M0/M1, removed in M2)

All three make M0/M1's numbers diverge from real workloads, and are recorded here so they are not mistaken for conclusions later:

- **The data distribution is uniform, not Zipf.** In real data a few hot groups take most of the updates. That changes cache behaviour significantly, and directly affects the evaluation of M4's in-memory arrangement — a uniform distribution understates the value of an in-memory cache.
- **Updates are spread uniformly, with no locality.** Real workloads concentrate updates on hot groups.
- **No standard benchmark's queries can be run.** TPC-H's queries need joins, Nexmark's mostly need joins and windows, and M0's engine side is empty. M0 can use only synthetic data; join lands in M1a, but a cross-system comparison also needs the real SQLite extension (M1b), so it cannot happen before M2.

### 10.7 Standard benchmark: Nexmark from M2 on

Once join lands, bring in **Nexmark** — the de facto standard for streaming / incremental systems. The Feldera repository ships a Nexmark benchmark and RisingWave publishes Nexmark results, so other people's numbers have a frame of reference when comparing on it (they still have to be run yourself; see §10.3 item 6).

The goal is fixed now so the plan IR and operator interfaces do not drift in a direction incompatible with it.

### 10.8 When to build it

**The benchmark harness first fills the "engine under test" slot with a naive-recompute implementation**, so there is a complete baseline curve from day one and every step of implementing the core has a live comparison, rather than finding out how fast it is only at the end.

---

## 11. Roadmap

> **Ordering constraint: the test framework and the benchmark skeleton must exist before the core; and the probe of SQLite's extension mechanics (M-1) must come before M1.**

### M-1 — SQLite extension mechanics spike (first) — ✅ done

**This is a spike; its output is a conclusion, not code.** The purpose is to find out, before writing any engine, whether the control surface can actually work as imagined — if not, changing it now is orders of magnitude cheaper than changing it once the core is written.

Both control surfaces were run on a **real cdylib extension** (not a host-language sqlite3 binding):

- **Design A**: DDL and writes inside scalar UDFs
- **Design B**: `CREATE VIRTUAL TABLE ... USING ivm(...)`, with xCreate creating the shadow tables and triggers and `INSERT INTO v(v) VALUES('refresh')` as the command channel (the FTS5 idiom, leading candidate)

Each covered: `CREATE TABLE` / `CREATE TRIGGER` / writing a shadow table / rollback / nested transactions / WAL mode / two concurrent connections / `DROP` cleanup, and design A additionally covered "called during a scan".

> **Done when: the control surface is settled and §8.3 changes from "to be verified" to a conclusion.** Result: design B all green, design B adopted; design A exposed two problems (the teardown path leaves orphan triggers that poison the base table; a self-referential write during a scan causes an unbounded loop), recorded as known pitfalls should B not be used — not a blocker in the "both designs have problems" sense. The full matrix and evidence are in [2026-09-18-m-1-results.md](../../spikes/2026-09-18-m-1-results.md).

M-1 and M0 are independent (M0 is the pure-Rust test and benchmark skeleton and never touches the extension API). The reason for doing M-1 first — its conclusion might rewrite §7 and §8 — has played out: §8.3 is updated to a conclusion, and §7.3's bootstrap-atomicity argument was re-checked against scenarios 3/4 and stands unchanged, with no rewrite needed.

### M0 — test and benchmark skeleton (before the core)

- crate skeleton + CI
- generators: schema / data / biased update sequences / query enumeration
- the four layers of assertions
- legality-preserving shrinking, seed replay
- the "engine under test" slot filled first with **naive recompute** (trivially correct) → should be all green
- then filled with a **deliberately buggy fake implementation** (for example, aggregation without retraction) → the framework must catch it and shrink it to a minimal case
- the benchmark harness + three same-host baseline curves

> **Done when: the framework catches the planted bug and shrinks it to 10 steps or fewer, and the three baseline curves are plotted.**

This step cannot be skipped — without verifying that the test framework really goes red, every later green is a false green.

### M1 — the v0 engine

- `ivmlite-core`: Value / Row / ZSet, plan IR, Arrangement trait, Filter / Project / Aggregate (SUM, COUNT)
- **delta consolidation** (§8.2): raw Δ is first merged by Z-set, adding up the weights of identical rows, before it reaches the operators. This is the performance story most likely to hold up for this project, and belongs to M1 rather than being a later optimization
- `ivmlite-sql`: `sqlparser-rs` → IR, the Catalog trait, hard errors outside the subset
- `ivmlite-sqlite`: the cdylib, the control surface settled by M-1, trigger DDL, shadow tables
- **watermark atomicity for bootstrap** (§7.3): the high watermark and the base-table snapshot must be taken in the same read transaction
- **delta-table GC** (§7.2): `__ivm_dep` + the most lagging view's watermark + dropping the triggers and delta tables once every view is dropped

**v0 limitations**: the root operator must be an Aggregate with a non-empty GROUP BY (§5.2); no global aggregates; STRICT tables only, with `ANY` columns rejected; BINARY collation only; group-by keys are bare columns only; no floating-point aggregation; integer overflow undefined; comparison operators limited to a whitelist; explicit refresh; INSERT / DELETE / UPDATE all supported.

> **Done when: all of M0's tests are green; the §10.4 surface has been measured; write amplification has a concrete number. Ugly numbers still count as done.**

#### M1 is split in two: M1a the pure-Rust engine, M1b the SQLite extension

M1 used to be one block, with join pushed to M2. Both have changed:

**Why split it**: the `Engine` trait (§8.5) **does not require SQLite**. A pure-Rust engine implementing it plugs straight into M0's differential testing harness and runs the whole suite — enumerated queries × biased update sequences × per-batch oracle comparison × batch independence. So at the end of M1a there is an incremental engine that is **verified correct and has not touched a single line of `unsafe`**. Doing it all at once instead would tangle FFI problems with operator problems, and M1b is the only place in the whole project with `unsafe`.

- **M1a**: plan IR, the `Arrangement` trait with an in-memory implementation, Filter / Project / Aggregate, **delta consolidation**, **Join**. Pure Rust, plugged into M0's harness.
- **M1b**: `ivmlite-sql` (`sqlparser-rs` → IR, Catalog, hard errors outside the subset), `ivmlite-sqlite` (the cdylib, the control surface settled in §8.3, trigger DDL, shadow tables), bootstrap watermark atomicity (§7.3), delta GC (§7.2).

**Why join moved up to M1a**: M0 already paid for it per §6.3 — `Arrangement::get` returns an iterator rather than an `Option`, the `Plan::Join` placeholder exists, and `apply` carries a table name. What remains of the cost falls mostly on **making the test framework multi-table** rather than on the engine, and the later that refactor is done, the more code it has to preserve. The differential mechanism itself does not care how many tables there are: a single table is just a multi-table case with one table.

**M1a has an internal checkpoint**: first finish the multi-table framework refactor and get the single-table engine green, then add join. That way, when join has a bug, it can be bisected (was it introduced by join, or was the framework refactor already wrong?) rather than debugging two kinds of bug at once.

### M2 — cross-system validation and standard benchmarks (after M1b)

With join moved into M1a, this milestone keeps only what needs the real SQLite extension:

- Bring in Turso, one setup serving two purposes: **correctness cross-validation** (the fourth layer of §9.1) and **performance peer comparison** (§10.2)
- Bring in **Nexmark** (§10.7)
- Extend the data distribution to Zipf and give updates locality (removing the first two simplifications of §10.6)
- Observe and record join state explosion (both sides must be kept in full) — M1a will see it first on the pure-Rust side
- Answer the open question of §9.2 item 4 with measured numbers: is widening the differential schema to 3 columns per table worth an 8x larger enumeration

### M3 — multi-connection semantics and maintenance strategy

**The "vtab auto-drain" that used to be listed here has been removed — it directly contradicts §8.2's argument.** §8.2 shows that read transactions cannot write, so `SELECT * FROM view` cannot apply pending deltas along the way; and maintaining immediately from inside a trigger would, because SQLite has only row-level triggers, turn one 10,000-row insert into 10,000 maintenance runs. Both paths are blocked, and M3 should not promise something §8 has already ruled out.

This milestone instead covers:

- staleness semantics and `applied_seq` watermark coordination across multiple connections
- concurrency safety of delta-table GC (see §7.2)
- **ergonomic improvements** to how maintenance is triggered, rather than an illusion of automation: for example an `ivm_refresh_all()`, and a recommended way to hook refresh into the application's own commit flow

**Explicit refresh is a permanent API, not a temporary v0 compromise** (see §8.2).

### M4+ — in order of value

- in-memory arrangement cache (proven worth it with M1's baselines)
- MIN / MAX (needs an extra data structure)
- cascading views
- DISTINCT
- floating-point aggregation + a tolerance strategy
- OUTER JOIN

### Explicitly not doing (for at least a year)

Recursive CTEs, window functions, correlated subqueries, any distributed capability.

### Success for the project as a whole

Being able to say: **"At X rows, Y views and Z update rate, it is N times faster / slower than hand-written triggers."**

**"Slower" also counts as success** — it is a true conclusion.

---

## 12. Decision record: rejected alternatives

### 12.1 Why not DuckDB

See §2. In addition: duckDBSP already implements everything this project originally planned for v0.1–v0.7 (including DISTINCT, MIN/MAX, window functions, recursive CTEs, cascading views, persistence), so building a subset on that host makes no sense. And DuckDB extensions are C++, which conflicts with the "learn Rust" goal.

### 12.2 Why not the "Rust library next to SQLite" form

That form (application writes go through its own API, similar to LiveStore's event-sourcing model) supports wasm / browsers naturally and could reach real local-first users. But: it gets no triggers, so CDC has to intercept writes itself; it requires the application to change how it writes, a high barrier to adoption; and the benchmark would mix in wasm↔JS boundary overhead, polluting the measurement.

The extension form can point straight at an existing `.db` file, and differential tests and benchmarks both run on the same connection, without noise. That matters more for M0/M1's goals.

### 12.3 Why not `preupdate_hook` / the session extension for CDC

Both need compile-time switches (`SQLITE_ENABLE_PREUPDATE_HOOK` / `SQLITE_ENABLE_SESSION`) that many distributions' stock builds leave off — a real portability tax. Triggers work everywhere, need no switches, roll back with the transaction, and still capture changes when the extension is not loaded.

### 12.4 Why not SQL-to-SQL compilation (the OpenIVM route)

That route compiles a view definition into SQL statements for maintenance, implementing no operators, managing no state and doing no persistence, reusing SQLite's executor directly for joins and aggregation. But: **what you learn is a compiler, not a dataflow engine**, which conflicts with goal 1; performance is capped by SQLite's executor and SQL round trips, so it cannot tell the low-latency story of "many views × tiny deltas"; and OpenIVM has already done it.

### 12.5 Why no silent fallback to full recomputation

An unsupported query is a hard error at `ivm_create_view`. A silent downgrade during v0 would hide bugs — the differential tests would pass, because full recomputation of course equals full recomputation. A fallback can be considered once the engine is stable.

### 12.6 Why v0 does not build a full DBSP circuit

It takes **design 2's data model with design 1's execution model**:

- **The data model is DBSP's from day one** — Z-sets with weights, operators as delta transformers with explicit state, state as key-indexed arrangements. This decides that join and recursion will have a place to go later, and that the project is comparable with Turso.
- **The execution model starts naive** — no circuit scheduler, no fixpoint machinery, no generic `I`/`D` operator pair. v0 is simply "deltas come in, get pushed through in plan-IR order, state is updated, results come out".
- **Whether a real circuit is needed is re-evaluated when join lands; it is required once recursion is on the schedule.** Re-evaluated for the two-table join (M1a Phase 3, 2026-09-23): no circuit is needed yet — pushing each table's delta through the tree in turn, with each join side's state updated before the other side probes, computes the full bilinear formula in either table order (the argument recorded at `IncrementalEngine::refresh`'s `by_table` step and in `docs/mutation-gates.md`'s §9.4 per-table merging row); a circuit is still required once recursion is on the schedule.

That keeps v0's code close to hand-written delta rules, without taking on any debt against DBSP.

### 12.7 Why DELETE is in v0 rather than v0.2

The original plan put DELETE / UPDATE in v0.2, making v0 insert-only — which is really a "materialized aggregate cache", not IVM, and would put **all the genuinely hard parts** — Z-set weights, retraction, state cleanup — **after the point where it looks done**. DELETE is where Z-sets earn their keep, and it must be in v0.

In exchange, join was moved out of v0 — join adds scope rather than architectural risk, and `Arrangement`'s shape already reserves room for it.

### 12.8 Why not compare with TanStack DB

TanStack DB is a browser-side JS library, with a different runtime and audience from v0's extension form. The comparison only makes sense in the "library next to SQLite" form (see §12.2), which v0 does not adopt, so it is out.

---

## 13. Known limitations (v0)

1. **The root operator must be an Aggregate with a non-empty GROUP BY**; views without aggregation and global aggregates are not supported (§5.2)
2. STRICT tables only, with **`ANY` columns rejected** (STRICT alone does not exclude `ANY`; see §7.1); the column-type whitelist is `INTEGER` / `TEXT`
3. BINARY collation only; detected by fetching the table's CREATE statement from `sqlite_master` and matching `COLLATE`, a conservative over-rejection (see §7.1)
4. Group-by keys are bare columns only, not expressions
5. No floating-point aggregation
6. **Integer overflow is undefined behaviour**; the absolute value of a group's sum must be < 2^62 (§6.1)
7. Comparison operators are limited to `>` `>=` `<` `<=` `=` `!=` `IS NULL` `IS NOT NULL`; no `NOT` / `OR` / `LIKE` / `IN` / `BETWEEN` / subqueries (§6.1); a view has at most one predicate — no `AND` (M1b Phase 2a)
8. `SUM` only over INTEGER columns, and a column may be compared only with a non-NULL literal of its own type; both are rejected at `ivm_create_view` (§6.1)
9. Joins are limited to a two-table inner equi-join on one pair of columns of the same type (M1a Phase 3). No outer joins, no self-joins, no more than two tables, no multi-column keys. A NULL key matches nothing. Filters are evaluated after the join. Written `FROM a [AS x] [INNER] JOIN b [AS y] ON <column> = <column>`; table aliases are supported, but a self-join is rejected even with aliases (M1b Phase 2b)
10. No MIN / MAX / DISTINCT
11. The SELECT list is the GROUP BY columns, each once, followed by the aggregates; GROUP BY names bare columns only (no positions like `GROUP BY 1`, no expressions); result column names must be unique; table names cannot be schema-qualified (`main.t`) (M1b Phase 2b)
12. Explicit refresh is required — a permanent API, not a temporary compromise (§8.2)
13. Delta tables capture every column, wasting space on wide tables
14. Every write pays the triggers' write amplification, even if the views are never read
15. Cannot be loaded in browsers or the iOS system SQLite
16. **A view's result columns may not be named like the view itself, or `__w`** (M1b Phase 3a): the view's name is its own command column, and `__w` is the output table's own weight column
17. **Phase 3a maintains one view per base table**; several views sharing one base table's delta table is Phase 3b (§1)
18. **`ALTER TABLE v RENAME TO w` on an ivmlite view leaves it undroppable** (M1b Phase 3a). rusqlite 0.40 exposes no `xRename`, so SQLite renames the table in `sqlite_schema` without telling the extension; `__ivm_view` still holds the row under the old name, so a later `DROP TABLE w` calls `xConnect` for `w`, finds no matching row, and fails with "ivmlite has no record of the view w". **Superseded by M1b Phase 3b §7**: `ALTER TABLE v RENAME` on an ivmlite view now fails outright with a message, instead of leaving the view undroppable.
19. **Statement-level REPLACE conflict resolution is captured only when the writing connection has `PRAGMA recursive_triggers = ON`** (M1b Phase 3a). `INSERT OR REPLACE`, `REPLACE INTO`, `UPDATE OR REPLACE` and `ON CONFLICT REPLACE` constraints remove the conflicting row without a DELETE statement, and SQLite fires the DELETE trigger for it only under that pragma; otherwise the view **silently diverges**. A base table whose DDL declares `ON CONFLICT REPLACE` is refused at create. **Superseded by M1b Phase 3b §6.4**: REPLACE conflict resolution is now captured without `PRAGMA recursive_triggers`, within the support boundary of Phase 3b §6.4 (see item 21 below); a table declaring `ON CONFLICT REPLACE` is now accepted and maintained.
20. **`ALTER TABLE t ADD COLUMN` on a base table breaks its views** (M1b Phase 3a): the scan's column list changes, so the view reports a different plan (and a different column shape) on every read and refresh and must be dropped and recreated.
21. **A writer with `PRAGMA recursive_triggers = OFF` whose REPLACE-style deletion happens during same-table re-entry (a trigger that writes the same base table again, directly or through triggers on other tables) is not supported** (M1b Phase 3b §6.4): the view may silently diverge. Recommended mitigation: `PRAGMA recursive_triggers = ON` for any writer of a database whose tracked tables have triggers that write back to the same table. A foreign-key cascade is not a re-entry.
