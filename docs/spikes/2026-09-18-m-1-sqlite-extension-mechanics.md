# M-1 Spike: SQLite extension mechanics

- **Date**: 2026-09-18
- **Kind**: spike — **the output is a conclusion, not code to keep**
- **Blocks**: M1 (does not block M0)
- **Related**: [overall design](../superpowers/specs/2026-09-18-ivmlite-design.md) §7, §8.3

---

## The question

ivmlite's control surface has to do two kinds of work from inside the extension: **DDL** (creating shadow tables and triggers) and **writes** (updating arrangements and output tables).

The question is: **from which context inside a SQLite extension is it safe to start each of them?**

An early draft assumed it could be done from inside a scalar UDF:

```sql
SELECT ivm_create_view('revenue', 'SELECT ...');
SELECT ivm_refresh('revenue');
```

That is, inside a `SELECT` that is mid-`sqlite3_step()`, doing DDL and writes on the same connection. SQLite restricts re-entrancy from hooks tightly — commit and update hooks explicitly forbid touching the connection that fired them from inside the callback. Application-defined functions are held to looser rules, but they still re-enter the same connection from inside a running statement.

**This is not a place to pass on "it ought to work in theory".** If the control surface is unsound, changing it now is orders of magnitude cheaper than changing it after the core is written.

### Early signals (not enough to decide)

One rough attempt through a host-language sqlite3 binding: `CREATE TABLE` and `INSERT` inside a UDF raised no errors. That **does not settle it**:

- it tested the host binding's statement handling, not the real cdylib extension path
- it covered only the happiest path — no rollback, nested transactions, WAL, or second connection
- "writing to the table currently being scanned" sits at the edge of undefined behaviour by itself

It did confirm a more valuable fact: **FTS5 has already solved a problem of the same shape.**

```sql
CREATE VIRTUAL TABLE docs USING fts5(body);
-- xCreate creates docs_data, docs_idx, docs_content, docs_docsize, docs_config
INSERT INTO docs(docs) VALUES ('rebuild');   -- the command channel
```

So this spike is not a yes/no on "do UDFs work"; it is **a side-by-side test of two designs, with design B as the leading candidate**.

---

## The two designs

### Design A: scalar UDFs

```sql
SELECT ivm_create_view('revenue', 'SELECT region, SUM(amount) FROM orders GROUP BY region');
SELECT ivm_refresh('revenue');
```

### Design B: virtual table plus command channel (the FTS5 idiom, leading candidate)

```sql
CREATE VIRTUAL TABLE revenue USING ivm(
    'SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region'
);
INSERT INTO revenue(revenue) VALUES ('refresh');
SELECT * FROM revenue;
DROP TABLE revenue;
```

If B works, it beats A in three places: DDL happens in the context SQLite designed for it (xCreate); the view becomes a real object that `sqlite_master` knows about; and `DROP TABLE` cleans up shadow tables and triggers naturally through xDestroy, with no separate teardown API.

---

## How the probe works

Write a **throwaway** Rust cdylib — not in the workspace, not in CI, not polished. It only has to load into the stock `sqlite3` CLI with `.load` and expose the minimal implementation of each control surface: create one shadow table, create one trigger, write one row to the shadow table.

Then run the matrix below **once for each design**.

| # | Scenario | What to observe |
|---|---|---|
| 1 | Bare call: create shadow table + trigger + write one row | Does it succeed; is `sqlite_master` as expected |
| 2 | Called inside an explicit `BEGIN ... COMMIT` | Does it succeed; is it visible after commit |
| 3 | Called inside an explicit `BEGIN ... ROLLBACK` | **Do the shadow table and trigger roll back with it**, or are orphans left behind |
| 4 | Nested: called inside a `SAVEPOINT`, then `ROLLBACK TO` | Same as above |
| 5 | Re-run 1–4 in WAL mode | Does behaviour match rollback-journal mode |
| 6 | Connection A calls the control surface while connection B is reading | Does it block; does it report `SQLITE_LOCKED` / `SQLITE_BUSY` |
| 7 | Connection A calls the control surface while connection B is writing | Same as above |
| 8 | Called while a `SELECT` is scanning the same table (design A only) | Does it hit undefined behaviour — **the cell where design A is most likely to fail** |
| 9 | `DROP TABLE` / the teardown path | Are the shadow table and trigger cleaned up completely |
| 10 | Open the same database without the extension loaded and write the base table | Do the triggers still record deltas (§8.1 relies on this) |

Scenario 10 is not a control-surface question, but it directly tests §8.1's design claim that "pure-SQL triggers capture writes even from connections that never loaded the extension", so it is tested alongside.

---

## Decision rules

| Outcome | Decision |
|---|---|
| B all green | **Adopt B**; §8.3 is settled, and the control-surface syntax is fixed as `CREATE VIRTUAL TABLE ... USING ivm(...)` |
| B has problems but A is all green | Adopt A, and record why B failed in an ADR — a counter-intuitive result worth writing down |
| Both all green | Still choose B (its three advantages hold); record A as the fallback |
| **Both have problems** | **Stop here and do not enter M1.** Redesign the control surface; candidates include "a read-side virtual table only, with maintenance invoked by the host application inside its own write transactions", or abandoning the loadable-extension form |

The results of scenarios 3 and 4 must be recorded even if they are not fatal: **if the shadow tables and triggers do not roll back with the transaction**, then "create a view" is not transactional, and §7.3's bootstrap-atomicity argument has to be rewritten.

---

## Done when

1. The results table for the 10 scenarios × 2 designs lands in `docs/spikes/2026-09-18-m-1-results.md`
2. §8.3 changes from "pending M-1" to a conclusion, and the control-surface syntax is settled
3. If the conclusion overturns any argument in §7 or §8, the spec is amended before M1 starts
4. The probe code is **explicitly marked as a throwaway artifact** and not merged into the workspace

---

## Relation to M0

M-1 and M0 are independent: M0 is the pure-Rust test and benchmark skeleton, and `ivmlite-core` / `ivmlite-test` / `ivmlite-workload` never touch the SQLite extension API (`ivmlite-test` uses rusqlite only as an oracle, and `ivmlite-bench` only to run baselines).

But **M-1 goes first**, because its conclusion may rewrite §7 and §8, and those two sections are M1's foundation.
