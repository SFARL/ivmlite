# M1b Phase 7: `ivm_refresh_all()`

**Status:** approved 2026-10-12.

**Parent specs:**
- [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md): §8.2 (explicit refresh is permanent) and §11 M3 (`ivm_refresh_all()` as an ergonomic improvement);
- [2026-10-11-m1b-phase6-fixed-cost-design.md](2026-10-11-m1b-phase6-fixed-cost-design.md): its statement-cache rules, §3, still bind.

The Phase 3a, 3b, 5 and 6 conventions all still hold.

## 1. Scope

**What this phase adds.** A SQL function that refreshes every ivmlite view of the database in one call, in either of two modes:
- **atomic:** every view refreshes, or none does;
- **continue:** each view on its own; a failure rolls back only that view and is reported by name.

**One optimisation that also benefits a single refresh.** A refresh with nothing new to apply returns before it computes or applies, after its checks.

**The motivation** is the research in `.superpowers/research/reports/` (2026-10-11, "Compile once, validate once, refresh many"). Oracle's `DBMS_MVIEW.REFRESH_ALL_MVIEWS` and its `atomic_refresh` flag are the model.
- A view's own refresh, `INSERT INTO v(v)`, runs inside a writing statement, where SQLite refuses `SAVEPOINT`.
- A function called from a top-level `SELECT` can open one (measured).
- So only a function can make many views' refreshes atomic together.

**Not in scope** (the next phase may take these up, once this one is measured):
- one delta read shared by several views;
- calling each view's refresh directly instead of through nested SQL;
- a table-valued form with one row per view;
- refreshing a chosen subset of views or the dependents of a table.

## 2. The function

**Signatures:**
- `ivm_refresh_all()` means `ivm_refresh_all('atomic')`.
- `ivm_refresh_all('atomic')`:
  - Opens one savepoint, then refreshes every view (§3).
  - If any refresh fails, it rolls back to the savepoint, releases it, and fails with `ivm_refresh_all: view <name>: <the refresh's own message>; no view was refreshed`.
  - Otherwise it releases the savepoint and returns an INTEGER: the number of views it refreshed, not counting the ones it skipped (§4).
- `ivm_refresh_all('continue')`:
  - Refreshes each view inside its own savepoint. A view whose refresh fails is rolled back to that savepoint and released; the function then goes on to the next view.
  - Returns TEXT: `refreshed <r>, skipped <s>, failed <f>`. When `f > 0`, the text continues with `: <name>: <message>; <name>: <message>`, in view order.
- Any other argument, or more than one argument, fails with `ivm_refresh_all takes 'atomic' or 'continue'`.

**Registration:**
- The function is registered in `sqlite3_ivmlite_init`, next to the module.
- Flags: `SQLITE_UTF8 | SQLITE_DIRECTONLY`. It is not deterministic. `SQLITE_DIRECTONLY` (SQLite 3.30) keeps it out of triggers, views and CHECK constraints, as SQLite recommends for a function with side effects.
- It may be registered through rusqlite's `functions` feature on the extension crate. That is a feature of an existing dependency, not a new one. Alternatively, `sqlite3_create_function_v2` through `ffi`, like the module.

**Where it may be called.** Only from a statement that does not write, in practice a top-level `SELECT`.
- If the function's first `SAVEPOINT` is refused, it fails with `ivm_refresh_all must be called from a SELECT, not from a statement that writes`. Example: `INSERT INTO t SELECT ivm_refresh_all()`.
- Inside an explicit transaction it works: the savepoint nests in the transaction. A later `ROLLBACK` undoes everything it did.

**The savepoint name** is reserved: `__ivm_refresh_all`. It is quoted, and released or rolled back on every path, including every error path. The function never returns with its savepoint open. A function that fails without rolling back its own nested writes would leave them in place (measured), so no code path may return early around the rollback.

## 3. Which views, in what order

- **The views:** every row of `"main"."__ivm_view"`, read once at the start of the call, ordered by `name` so that a run is reproducible.
- **No views:** if the database holds no ivmlite views, the call returns 0, or `refreshed 0, skipped 0, failed 0`. It does not create the global tables.
- **Order:** v0 has no view that reads another view, so the order has no other meaning.
- **One nested statement per view:** each view is refreshed by `INSERT INTO "main".<quoted name>(<quoted name>) VALUES ('refresh')`. That goes through SQLite's own virtual-table machinery, so the view's checks, its statement cache, its snapshot and its single-statement apply all apply unchanged.
- **Broken views:** a view that is broken fails its refresh with its usual message. The atomic mode fails the call; the continue mode reports the view as failed.

**These nested statements are not cached.** A statement that names an ivmlite virtual table holds a reference on it, and that reference defers its xDisconnect and makes `sqlite3_close` fail with `SQLITE_BUSY` (Phase 6 §3 rule 2). So each call prepares its nested statements and finalizes them before it returns. §6 measures that cost.

## 4. Skipping a view with nothing new

**Inside `refresh`.**
- After the catalog checks and the latch reads (Phase 5 §3, Phase 6 §4), and after reading each base table's deltas: if no base table has a delta row past the view's watermark, `refresh` returns without computing, staging or applying.
- The stage keeps whatever the previous apply left; the next apply empties it as before.
- This benefits every refresh, not only `ivm_refresh_all`.

**The checks always run first.**
- A base table dropped and recreated has lost its capture triggers. Its writes are no longer captured, so its delta table stays empty and the view would look up to date forever.
- The checks are what turn that into a broken view. A skip that bypassed them would make the view silently stale, which is the failure the checks exist to prevent.

**Counting skips.**
- Before each view's nested statement, the function compares that view's watermarks with each base table's highest delta seq (`MAX(__ivm_seq)`, `NULL` when the table is empty).
- If none is behind, the view counts as skipped, and its refresh still runs, which is the check-then-return above. In continue mode a skipped view that fails its checks counts as failed, not as skipped.
- The comparison reads `__ivm_progress` and the delta tables only. It changes nothing.

## 5. Tests

**Modes:**
- Atomic success returns the number of views refreshed, and every view matches the oracle.
- Atomic failure: inject a `RAISE(ABORT)` on one view's output table, between two healthy views.
  - The call fails with the view's name and `no view was refreshed`.
  - Every view, healthy ones included, still holds its earlier contents, with unchanged watermarks.
  - This must hold in autocommit and inside an explicit transaction, which stays usable afterwards.
- Continue: with the same fault, the two healthy views are refreshed and match the oracle. The failed view is unchanged. The returned text names it and its message, with exact counts.
- **Broken views:** a broken view (a dropped capture trigger) fails the atomic call and is reported failed by the continue call, with the broken-view message.

**Where it may be called:**
- `INSERT INTO t SELECT ivm_refresh_all()` fails with the §2 message and changes nothing.
- A trigger body that calls `ivm_refresh_all()` is refused by SQLite (`SQLITE_DIRECTONLY`).
- Inside `BEGIN … ROLLBACK`, a call's effects are undone. Inside `BEGIN … COMMIT` they are kept.
- A bad argument fails with the §2 message.
- A database with no views returns 0 and creates nothing.

**Skipping:**
- With three views on two base tables, write only to one table. The call refreshes the views of that table and skips the rest, with exact counts in both modes.
- **A recreated base table:** after `DROP TABLE` and a recreate of a base table, the call reports its views broken (atomic fails; continue reports failed), never skipped.
- A single `INSERT INTO v(v) VALUES('refresh')` with no new deltas changes no table. Check `total_changes()` and the stage's contents.

**Lifetime:** after any of the calls above, `sqlite3_close` (v1) returns `SQLITE_OK`. The function leaves no statement and no savepoint behind.

Every new check or behaviour gets a verified gate row, including:
- dropping the rollback on the atomic failure path;
- dropping the per-view rollback in continue mode;
- skipping the checks on the early return;
- dropping `SQLITE_DIRECTONLY`;
- releasing the savepoint before the error is formatted on an early-return path.

## 6. Measurement

Use the m0 workload (100,000 base rows, 1,000 groups) with release builds, `before` (master at Phase 7 start) against `after`. Each cell has a warm-up and five measured batches, as in Phase 6 §6.

**A committed driver, `scripts/bench-refresh-all.py`.** It is Python, using the standard library and `sqlite3` with `load_extension`. It is not a new `ivmlite-bench` mode, because the matrix measures engines, not call shapes. It writes `docs/bench/m1b-phase7-refresh-all.csv`, and runs these cases:
- **Shapes compared:** 10 and 200 views, batch 1 and batch 1,000, three ways:
  - one `INSERT INTO v(v)` per view, prepared once by the application and reused;
  - the same, prepared afresh every time;
  - `SELECT ivm_refresh_all()`.
- **Skip cases:** 200 views over 2 base tables, where only one table is written, and where nothing is written.
- **The nested prepares.** The cost of re-preparing the nested statements, per view per call.

**Results** go in a new "M1b Phase 7" section in `docs/bench/README.md`. Its numbers come from that CSV through a committed `docs/bench/phase7_tables.py`. The section says what the driver measures and how it differs from the matrix protocol.

**Release.** The phase ends with `v0.1.0-alpha.5`: a CHANGELOG entry, release notes, and the README's usage section documenting `ivm_refresh_all`. The format is unchanged (5).
