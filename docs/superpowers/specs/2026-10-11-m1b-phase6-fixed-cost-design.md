# M1b Phase 6: the fixed cost of a refresh

**Status:** approved 2026-10-11.

**Parent specs:**
- [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md), §10 and §11 (M3, M4);
- [2026-10-08-m1b-phase5-refresh-performance-design.md](2026-10-08-m1b-phase5-refresh-performance-design.md), whose §3 cache and §4 apply this phase changes;
- [2026-09-28-m1b-phase4-benchmark-design.md](2026-09-28-m1b-phase4-benchmark-design.md), the measurement protocol that §6 extends.

The conventions of the Phase 3a and 3b specs all still hold.

## 1. Scope

After Phase 5, a refresh spends most of its time on two fixed costs, not on the change it applies (§2). This phase removes them, and makes the benchmark measure what remains:

1. **Statement cache.** Each view keeps its statements prepared across refreshes (§3).
2. **Shared catalog snapshot.** A connection reads the catalog once per schema change and shares the snapshot among its views (§4).
3. **Progress index.** `__ivm_progress` gets an index, so the garbage collection inside a view's apply stops scanning every view's watermark (§5). The shadow-table format becomes **5**.
4. **Steady-state benchmark.** Matrix, confirmation and ablation cells time several batches per database, and report the first refresh and the steady state apart (§6).

**Not in scope** (the next phase decides on them once this one is measured):
- `ivm_refresh_all()`, batch savepoints, and one delta read shared by several views;
- a per-view fingerprint of the catalog;
- an in-memory arrangement cache;
- the write path.

## 2. Evidence (throwaway measurements, 2026-10-11)

**Setup.** Release builds, Python 3.13 with SQLite 3.53.0, in-memory databases. Each run used 100,000 base rows in 1,000 groups, with N views of `SELECT region, SUM(amount), COUNT(*) … WHERE amount > k GROUP BY region`, all refreshed after a 1-row insert. The scripts stay in the session scratchpad.

**Two different costs.** Per view, per refresh:

| views | steady refresh | first refresh after any DDL |
|---:|---:|---:|
| 10 | 0.11 ms | 0.23 ms |
| 50 | 0.12 ms | 0.43 ms |
| 200 | 0.13 ms | 1.23 ms |

Any DDL in `main` produces the second column, including an unrelated `CREATE TABLE`. Creating another view is DDL too, so a benchmark cell that creates its views and then refreshes once measures that second column.

**Cost 1: statements are compiled on every refresh.**
- Each `xUpdate` wraps the handle in a new `rusqlite::Connection` (`vtab.rs`, `connection`). Its statement cache dies with it.
- A `sample` profile at 200 views and batch 1 attributes 40.9% of `view::refresh` to preparing the arming `UPDATE`.
- Preparing a statement that fires a row trigger compiles the trigger's body into that statement as a sub-program (SQLite `src/trigger.c`, `getRowTrigger` → `codeRowTrigger`). A statement that is kept and reset never recompiles it.
- A cached statement is re-prepared automatically when the schema cookie moves (`sqlite3Reprepare`). It is therefore still correct after DDL.

**Cost 2: after a schema change, every view re-reads the catalog.**
- Phase 5's cache is per view and keyed by `main.schema_version`. A miss runs `check_capture` and `check_output_index`, which make about ten name lookups in `sqlite_schema`.
- `sqlite_schema` has no index, so each lookup is a scan: about 0.1 ms at 2,400 schema rows.
- The base-table checks (`check_table_capture`) are repeated for every view that reads the table, although their result is the same for all of them.
- Detecting the change costs almost nothing: inside a transaction, `PRAGMA schema_version` costs about as much as `SELECT 1`.

**Prior art.** Mature extensions keep their compiled statements for the object's lifetime:
- FTS5 (`fts5_storage.c`, `aStmt[]`, finalized in xDisconnect/xDestroy);
- R-Tree (`rtreeSqlInit`);
- cr-sqlite (per-connection `TableInfo` statements).

cr-sqlite and Datasette keep one snapshot of table metadata per connection or database, keyed by the schema version and shared by every object. IVM engines with a low fixed cost compile once per view and only bind and step during maintenance: Turso, Feldera, Materialize, Noria, DBToaster. pg_ivm and OpenIVM re-plan every round, and their cost grows with the number of views.

**A prototype confirmed cost 1.** The prototype kept one `Connection` in each `IvmTab` for the view's lifetime:

| prototype, 200 views, batch 1 | steady per view | SQLite memory |
|---|---:|---:|
| Phase 5 (master) | 128 µs | 38.3 MB |
| one `Connection` per view, cache capacity 4 | 65 µs | 43.0 MB |
| one `Connection` per view, cache capacity 16 | 66 µs | 63.7 MB |

The larger cache filled with remainder staging statements of varying length, which bought nothing, so §3 stages a remainder in a few fixed shapes. `DROP TABLE` of a view worked while the other views held cached statements.

**What remains in the prototype's profile:**
- the apply trigger's body, 48%;
- statements still prepared uncached on every refresh, about 25%: reading the deltas, the latch, the watermark, and emptying and arming the stage.

**Cost 3: garbage collection scans every view's watermark.** The body's GC computes `MIN(applied_seq) FROM __ivm_progress WHERE tbl = 't'`. The table's key is `(view, tbl)`, so the minimum scans one row per view of the database. That makes a refresh O(views) and refreshing every view O(views²). An index on `(tbl, applied_seq)` took the prototype from 66 to 58 µs per view at 200 views.

## 3. Statement cache

**The connection.**
- Each `IvmTab` owns one non-owning `rusqlite::Connection` over its handle, made in xCreate or xConnect.
- It lives as long as the `IvmTab`, so it is dropped, and its cached statements finalized, in xDisconnect and xDestroy. SQLite calls xDisconnect before it checks for unfinalized statements at close, so a statement cached here never makes `sqlite3_close` fail with `SQLITE_BUSY`.
- Every callback of the view uses this connection.
- The free function `connection` in `vtab.rs` is removed: the handle is wrapped once, in `open_view`.

**What is cached.**
- Every statement on the refresh path whose SQL text is fixed for the view is prepared at most once per view per connection, through `prepare_cached`. The list:
  - the schema-version read;
  - each base table's latch read;
  - the watermark read;
  - each base table's delta read;
  - `BufferedArrangement`'s lookups and scans;
  - emptying the stage;
  - staging the sentinel;
  - staging each watermark;
  - each kind's staging inserts, in the fixed shapes described next;
  - the arming `UPDATE`.
- **Staging shapes.** Rows are staged in full chunks, as in Phase 5 §6. The remainder after the last full chunk is split into its binary digits, chunks of 32, 16, 8, 4, 2 and 1 rows, each capped by the chunk size.
  - Every staging statement therefore has one of at most seven shapes per kind, all cached.
  - So a steady refresh prepares nothing, even at batch 1, where every chunk is a remainder.
  - The rows still go in the order they are pushed.
- **Capacity.** The cache capacity is set from the length of that list for the view, counting seven staging shapes per kind, plus a margin of 4. No fixed statement is evicted by another.
- **The bootstrap's shapes are not kept.** `create` stages the whole base table, so it fills the cache with large staging shapes that a steady refresh never reuses. It flushes the cache as its last step. Flushing finalizes statements and runs no SQL.
- **Catalog checks are not cached.** The checks of §4 run only after a schema change, so caching them would hold memory for nothing.

**Rules that keep the cache safe:**
1. **Reset before returning.** A cached statement is reset before the callback returns: rusqlite's `execute` steps then resets it, and its `Rows` reset it when dropped; returning it to the cache only clears its bindings, so every cached use goes through those APIs. A stepped statement that is not reset is an active reader, and it would block a `DROP TABLE`.
2. **No virtual table.** No cached statement, including any step of the apply trigger's body, reads or writes an ivmlite virtual table. A statement that touches a virtual table holds a reference that defers its xDisconnect. None of ivmlite's own SQL does, and a test pins this. A user trigger on an `__ivm_` shadow table that reads an ivmlite view is compiled into that view's cached arming statement, and pins the connection: `sqlite3_close` then returns `SQLITE_BUSY` (measured). Writing triggers on reserved tables is out of scope, like any other tampering with them (Phase 3b §9).
3. **No TEMP objects.** The refresh path creates and drops no TEMP object. TEMP DDL expires every statement on the connection.
4. **Renames.** An ivmlite view cannot be renamed (`refuse_rename`, Phase 3b §7), so no cached SQL text can name an old table.

**Correctness is unchanged.**
- A cached statement that SQLite finds stale re-prepares from its own text against the current schema.
- Every refresh still runs the checks of §4 before it steps any cached statement other than the schema-version and latch reads those checks themselves use, so a statement that reads the view's shadow tables never runs against a schema those checks have not passed.

**Tests:**
- **Steady refreshes prepare nothing.** On a test connection, an authorizer is installed before the view's first refresh; the authorizer is consulted whenever a statement is prepared. The first refreshes after it re-prepare what it expired and stage enough rows (127, then 5) to prepare every staging shape of both kinds. Its calls are then counted around each of three later refreshes, which stage 1, 37 and 130 rows, and the count is 0. The refresh command itself is prepared before the counted window.
- **Remainders are staged in order.** At 0, 1, 63, 64, 65 and 129 staged rows the stage holds every row, in push order, and the view matches the oracle.
- **After DDL, refreshes are correct again.** A refresh after `CREATE INDEX` on a base table, after an unrelated `CREATE TABLE`, and after a schema change made from a second connection, each matches the oracle.
- **Dropping works.** `DROP TABLE` of one view succeeds while other views on the same base table hold cached statements. Dropping the last view of a base table removes its delta table.
- **Closing works.** `sqlite3_close`, not `close_v2`, returns `SQLITE_OK` after views have refreshed.
- Every existing atomicity and fault-injection test stays green.

**Gate rows:**
- wrap the handle afresh on each refresh again;
- prepare the remainder as one uncached statement;
- set the capacity below the fixed list.

## 4. Shared catalog snapshot

**The snapshot.** One per connection:
- `main.schema_version` at the time it was read;
- every row of `main.sqlite_schema` (type, name, tbl_name), indexed by name in memory;
- a lazily filled map from each base table to its capture verdict.

The capture verdict is `check_table_capture` without the latch: shape, capture triggers, probe, delta and pend tables. It holds the schema read for the shape, or why the table is broken.

**Where it lives.**
- A process-wide registry maps a connection's handle to a `Weak` reference to its snapshot. It is held behind a `Mutex`, because SQLite may call one connection's callbacks from different threads over its life.
- Each `IvmTab` holds a strong reference to its connection's snapshot. When the last view of a connection disconnects, the snapshot is freed, and the dead `Weak` stays in the registry. A later connection that reuses the handle's address finds that dead `Weak` and creates a new snapshot.

**At refresh:**
1. Read `main.schema_version`.
2. If the snapshot's version differs, rebuild it: one scan of `sqlite_schema`, and an empty verdict map.
3. Check the view against the snapshot:
   - its own objects exist where they must, using in-memory lookups: state, output and stage tables, the output index, the apply trigger on its stage table, and the delta tables;
   - its dependency rows are recorded;
   - each base table's verdict passes. A verdict is computed with pragmas the first time any view of this connection asks for it at this version, and reused after that.

   On a pass, the view caches the version it passed at, as in Phase 5 §3.
4. When the version is unchanged and the view passed at it, skip step 3, as in Phase 5.
5. In every case, read each base table's latch, as in Phase 5 §3.

`verify` at connect uses the same snapshot, read afresh (below), and the same checks.

**A connect rebuilds it** (measured during implementation, 2026-10-11). Rolling back a schema change can bring `main.schema_version` back to a value it had before, with different objects. SQLite then reconnects every view, which is why Phase 5's per-view record was safe. A shared snapshot, though, can outlive that reconnect: a statement the application keeps prepared keeps a view, and its snapshot reference, alive. A version match alone would then pass a view whose capture trigger is missing. So every xConnect rebuilds the connection's snapshot before it checks.
- **What it costs.** After DDL from another connection, or a rollback, a mass reconnect makes one scan per view, about a fifth of Phase 5's `sqlite_schema` reads. DDL on the same connection keeps one scan per connection.
- **Reconnect time is unchanged.** A mass reconnect still takes about as long as before: 2.5 s at 200 views and 20,000 rows, measured. That time is dominated by work every connect does, which grows about quadratically with the view count and predates this phase. It is recorded for a later phase.
- **Read failures are not cached.** A failure to read the catalog behind a verdict, such as an interrupt or a lock, is returned but not cached.

**Why `main` alone.**
- The checks examine only objects in `main`, and TEMP DDL cannot create, drop or alter an object in `main`. A TEMP trigger on a base table is a user write to that table, which capture records like any other.
- A TEMP trigger that writes to an `__ivm_` table is tampering with a shadow table, which is out of scope like any direct write to it (Phase 3b §9).
- `temp.schema_version` is therefore not part of the key.

**Over-invalidation is accepted.**
- `ANALYZE` and unrelated DDL move the cookie and force a rebuild.
- After this change a rebuild is one scan per connection, not one per view, so a finer key such as a per-view fingerprint is not worth its risk.

**Error messages are unchanged.** Every broken-view message keeps its present text, so the existing tests still pin them.

**Tests:**
- **Phase 5 §3 tests.** Every test in `extension_refresh_cache.rs` stays green: DDL on the same connection, on another connection and on a connection without the extension, the latch, rolled-back checks, and data-only tampering of `__ivm_dep`.
- **Two views share one table.** Breaking the capture trigger of a base table that two views read breaks both views' next refreshes, with today's message.
- **Rebuild on DDL.** An unrelated `CREATE TABLE` rebuilds the snapshot; every view's next refresh still matches the oracle.
- **Shared within one connection only.** Two connections each build their own snapshot. DDL on one connection is seen by the other connection's next refresh.
- **Closing frees it.** The snapshot is freed when the connection closes; a second connection opened afterwards gets a new one.

**Gate rows:**
- skip the rebuild when the version changes;
- reuse a verdict across versions;
- keep a verdict computed for one table under another table's name.

## 5. Progress index (format 5)

The global tables gain `CREATE INDEX "main"."__ivm_progress_tbl" ON "__ivm_progress"(tbl, applied_seq)`. It is created with them, under `IF NOT EXISTS`. With it, the body's GC minimum becomes a search on the new index; the watermark update already was a search, on the table's `(view, tbl)` key, and stays one.

**Its absence breaks the view.** Every view's check (§4) requires the index, as Phase 4 §4 requires each view's output index. Without the index a refresh does not fail; it silently degrades to O(views), and a missing index is reported, not tolerated. A broken view can still be dropped. Creating any view recreates the index, and views that broke because it was missing work again after the next connect.

**The format becomes 5.** A format-4 database is refused, and its views still open as broken views that can be dropped (Phase 3a §5). There is no migration; this is an alpha.

**Tests:**
- The query plans of the GC statement and of the watermark update, read from the stored trigger: the GC's shows a SEARCH on `__ivm_progress_tbl`, the watermark update's a SEARCH on `__ivm_progress`, and neither scans it.
- Creating a view after the index was dropped recreates it, and the broken views work again after the next connect.
- The format test is tampered to 4.
- Dropping the index breaks the next refresh with a message naming it.

**Gate rows:**
- drop the index from the global tables;
- skip its check.

## 6. Steady-state benchmark

**The protocol.** The Phase 4 §3 protocol is otherwise unchanged. Each matrix, confirmation and ablation cell now runs:
1. bootstrap, then verify;
2. one warm-up batch, maintained and verified, outside every timer except its own maintenance, which is reported as `first_refresh_ms`;
3. **five** measured batches, each with a timed apply and a timed maintain. Each batch touches rows no other batch of the cell touches. The cell verifies after the last one.

**What is reported.**
- Each engine's `apply_ms`, `maintain_ms` and `apply_plus_maintain_ms` per run are medians over the five batches.
- `apply_plus_maintain_ms` is the median of each batch's own sum.
- `first_refresh_ms` is reported beside them.
- The engines that maintain nothing on refresh report their own first maintenance in that column.

**Comparisons.** The §10.2 comparisons against hand-written triggers, and the confirmation rule, use the steady-state columns from now on. Phase 4 and Phase 5 measured the first refresh after `CREATE`. Their numbers are not comparable column for column, and the report says so.

**The workload.**
- `Workload` gains the rounds it must fit. Its validation checks that base rows and group keys support six disjoint batches at each cell's batch size. Preflight replays the same deterministic operation generator and rejects any requested delete after the base rows are exhausted; bounding the expected delete count is insufficient because each operation is sampled independently.
- `workloads/m0-baseline.toml` is unchanged, unless a cell cannot fit six batches. Such a cell is reported and adjusted minimally.

**Write-amp is not re-run.** The write path does not change.

**Tests:**
- The cell runner applies six batches and verifies after the last.
- The median over batches is per step, and the per-batch sum for the total, as in the demo runner (`demo_bench::steady_state`).
- A workload that cannot fit six batches is refused.
- Each new test gets a gate row.

## 7. Measurement and release

**The ablation.** `scripts/bench-ablation.sh` runs over these reviewed commits:
- `before`: master at Phase 6 start;
- `stmtcache`: after §3;
- `snapshot`: after §4;
- `progidx`: after §5.

It uses the steady-state protocol of §6, built from this phase's harness for every label, and keeps Phase 5's ablation cells.

**Acceptance.**
- A change is kept only if it makes steady-state refresh at least 5% faster in the cell it targets:
  - §3: 10 and 200 views at batch 1;
  - §5: 200 views at batch 1.
- §4 is held to the same bar on `first_refresh_ms` at 200 views.
- A change that misses its bar is reverted, and this spec records why.

**Full runs.**
- The full matrix and the confirmation into `docs/bench/m1b-phase6.csv` and `docs/bench/m1b-phase6-confirm.csv`.
- The demo before/after: `scripts/bench-demos.sh before=<master at Phase 6 start> after=<last reviewed commit>`.
- `real_case_bench` at its documented settings.

**Reporting.** A new "M1b Phase 6" section in `docs/bench/README.md`, generated by a `phase6_tables.py`. It states:
- the §10.2 bars against hand-written triggers in the steady state;
- the first refresh beside them;
- the ablation, change by change;
- what still loses and why;
- anything that got worse.

**Release.** The phase ends with `v0.1.0-alpha.4`: a CHANGELOG entry, release notes, and format 5.
