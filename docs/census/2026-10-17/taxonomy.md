# Census taxonomy: features missing from ivmlite v0 (alpha.5)

## What v0 supports today

The authoritative list is `docs/superpowers/specs/2026-09-18-ivmlite-design.md` §13. In summary:

- The root is one `SELECT g1, …, COUNT(*), SUM(int_col) FROM … GROUP BY g1, …`. GROUP BY is non-empty, the keys are bare columns, and the SELECT list is the GROUP BY columns followed by the aggregates.
- **Aggregates:** only `COUNT(*)` and `SUM(<INTEGER column>)`.
- **Tables:** STRICT only, with INTEGER and TEXT columns. `ANY` columns are rejected, collation must be BINARY, and table names cannot be schema-qualified.
- **WHERE:** at most one predicate, of the form `col op literal` with `op` in `> >= < <= = !=`, or `IS [NOT] NULL`. No AND, OR, NOT, IN, BETWEEN or LIKE, and no comparison between two columns.
- **Joins:** at most one two-table inner equi-join on one pair of same-typed columns. No outer joins, self-joins, three or more tables, or multi-column keys.
- **Not supported:** subqueries, CTEs, HAVING, ORDER BY or LIMIT inside the view, DISTINCT, MIN and MAX, expressions anywhere, and floating-point values.
- **Runtime:** a native loadable extension, refreshed explicitly. It cannot be loaded into a browser, the iOS system SQLite, Turso's engine, or pure-Go drivers such as modernc.

## Feature codes

Use these codes exactly.

| code | feature |
|---|---|
| F1 | non-aggregated view: a projection or filter of rows, a bag, with no GROUP BY and no aggregate |
| F2 | global aggregate: an aggregate with no GROUP BY |
| F3a | `COUNT(col)` |
| F3b | `AVG(col)` (derivable from SUM and COUNT) |
| F3c | `MIN` or `MAX` |
| F3d | `COUNT(DISTINCT …)` or `SUM(DISTINCT …)` |
| F3e | `SUM` or `AVG` over REAL, or `TOTAL()` |
| F3f | other aggregates: `group_concat`, `json_group_array`, … |
| F4a | an expression inside an aggregate argument: `SUM(CASE …)`, `SUM(a*b)`, `COUNT(CASE …)` |
| F4b | an expression in the SELECT list outside aggregates: arithmetic, CASE, a scalar function |
| F5 | an expression in GROUP BY, for example `strftime(...)` bucketing or `lower(col)` |
| F6 | a richer WHERE: AND, OR, IN, BETWEEN, LIKE, NOT, several predicates, column-to-column comparison, functions in predicates |
| F6t | a time-relative predicate, such as `created_at > now() - interval` (non-deterministic) |
| F7a | LEFT or other outer join |
| F7b | three or more tables |
| F7c | a multi-column join key, a non-equi join, a self-join, or join columns of different types |
| F8 | a subquery or CTE in FROM, or nested aggregation (an aggregate over an aggregate) |
| F8s | a scalar or correlated subquery, or `EXISTS` / `IN (SELECT …)` |
| F9 | ORDER BY, LIMIT or top-N inside the maintained result, or window functions (`ROW_NUMBER`, `RANK`, …) |
| F10a | a non-STRICT base table: an existing app schema without STRICT |
| F10b | a column type outside INTEGER and TEXT: REAL, BLOB, BOOLEAN, NUMERIC, DATETIME, ANY |
| F10c | a non-BINARY collation, such as NOCASE |
| F11 | HAVING |
| F12 | streaming or time-window semantics: TUMBLE, HOP, SESSION, watermarks, temporal (FOR SYSTEM_TIME) joins, event-time |
| F13 | retention or sealing: aggregates that must survive deletion of the raw rows |
| F14 | runtime cannot load a native SQLite extension: Turso engine, modernc, a browser, iOS system SQLite, … |
| F15 | other; name it explicitly, e.g. "ordered state machine", "Flink-only function SPLIT_INDEX" |

## For each item

- `id`, plus the source URL or file at a pinned revision.
- `query`: the exact SQL, or a faithful excerpt.
- `schema`: the relevant DDL, or a summary of it.
- `features`: every code the query needs beyond v0. List all of them, not only the first error.
- `minimal_unlock`: the smallest set of codes that would make the main maintained query runnable without changing its meaning. Where possible, give an `adapted_unlock` as well: a set that keeps the query's answer once a faithful, documented rewrite is applied (for example, non-STRICT replaced by a STRICT projection maintained by the application). Say what the rewrite is.
- `fit`, one of:
  - `fits_now`: runs on v0 as is;
  - `fits_with`: runs once its minimal unlock is implemented;
  - `out_of_scope`: needs F12, F14 or an F15 like an ordered state machine;
  - `not_ivm_shaped`: the real problem is a missing index, N+1 queries, a cache, and similar.
- `confidence`, either high or low, with one sentence of reasoning.
