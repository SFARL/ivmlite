# M1b Phase 3a: the SQLite extension, one view end to end

**Status:** approved 2026-09-26. **Parent spec:** [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md) — this document refines its §4.3 (`ivmlite-sqlite`), §4.4 (state ownership and its open questions), §7 (shadow tables), §7.3 (bootstrap atomicity), §8.1–§8.4 (CDC, refresh, control surface) for Phase 3a. Where the two disagree, this document is newer and wins for Phase 3a; the parent spec is amended to point here.

**Amended 2026-09-27:** Phase 3b ([2026-09-27-m1b-phase3b-shared-capture-design.md](2026-09-27-m1b-phase3b-shared-capture-design.md)) supersedes §4's `__ivm_dep.shape` and one-view-per-table rule, §5 step 3, the REPLACE and rename limitations, and scenario 5's "already tracked" and `ON CONFLICT REPLACE` rejections.

## 1. Scope

Phase 3a delivers a real loadable extension that maintains **one view at a time per base table**, including a two-table inner equi-join view, through its whole life:

- `CREATE VIRTUAL TABLE v USING ivm('<SELECT …>')` compiles the view, creates its shadow tables and triggers, and bootstraps it;
- writes to the base tables — from any connection, with or without the extension loaded — are captured (statement-level REPLACE conflict resolution only when the writing connection has `PRAGMA recursive_triggers = ON`; see the known limitations at the end of §5);
- `INSERT INTO v(v) VALUES('refresh')` brings the view up to date;
- `SELECT * FROM v` reads it;
- closing and reopening the database and refreshing again continues where the last refresh stopped;
- a refresh that fails part way leaves operator state, output and watermarks exactly as they were, and a retry succeeds;
- `DROP TABLE v` removes everything the view created.

Phase 3b (not here): several views sharing a base table's delta table, per-view progress on a shared table, GC by the slowest watermark (§7.2), and dropping a table's triggers only when its last view goes. There is no Phase 3c until a concrete task needs one.

## 2. Build and test layout

`crates/ivmlite-sqlite` is a `cdylib` **excluded from the Cargo workspace**, with its own `Cargo.lock`. It depends on `ivmlite-core` and `ivmlite-sql` by path and on `rusqlite` 0.40 with features `loadable_extension` and `vtab`. It cannot share a build with the test host: `ivmlite-test` needs `rusqlite`'s `bundled` and `load_extension` features, and Cargo's feature unification across workspace members makes the combined build fail (measured 2026-09-26). Two builds work: the bundled-SQLite test host loads the extension's dynamic library at run time.

- `scripts/build-extension.sh` builds the extension. `scripts/test-all.sh` builds it, runs its own unit tests, then runs `cargo test --workspace --locked --no-fail-fast`. The mutation-gate protocol (`docs/mutation-gates.md`) uses `scripts/test-all.sh` instead of the bare `cargo test`.
- The extension tests in `ivmlite-test` load the library from one fixed path under the extension's own target directory. If the library is missing, or older than any manifest or source file of `ivmlite-sqlite`, `ivmlite-sql` or `ivmlite-core` or than the extension's own `Cargo.lock` (which decides its dependency versions), the tests **fail** with an instruction to run the script. They never skip: a skipped extension test would be a false green.

## 3. Core changes: fallible state

A SQLite-backed arrangement can fail, so failure becomes part of the interfaces that touch state (this settles §4.4's deferred signature question):

- `Arrangement::get(&self, key) -> Result<Vec<(Row, i64)>, StateError>`, `update(&mut self, key, val, weight_delta) -> Result<(), StateError>`, `scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError>`. Reads are collected eagerly; a lazy iterator borrowing a prepared statement is not worth its lifetimes and `unsafe` in v0.
- The provider becomes `&mut dyn FnMut(ArrangementId) -> Result<Box<dyn Arrangement>, StateError>`; `Node::build` and `Node::delta` return `Result`.
- Decoding a corrupted aggregate state returns a `StateError` instead of panicking.
- `MemArrangement` wraps its results in `Ok`; `IncrementalEngine` maps `StateError` to `EngineError`.
- `Plan` gains a canonical text rendering (`Plan::canonical`), stable across Rust versions because it is written by hand, not derived.

## 4. Persistence conventions

All identifiers are double-quoted in every statement the extension issues. A statement that reads, writes, creates or drops a base table or any table or trigger the extension owns — the global tables, the delta, state, output and stage tables, and every trigger — additionally schema-qualifies the name to `"main"`, and the catalog reads `"main".sqlite_schema` and `pragma_table_info(<table>, 'main')`. SQLite resolves an unqualified name against `temp` before `main`, so a same-named TEMP table would otherwise be read, written, dropped or given the triggers instead (measured on the connection that owns a view: without its schema argument, `pragma_table_info` read a TEMP table's columns, and a refresh read a TEMP state table). Two places cannot be qualified: a trigger's `ON` clause, and the tables named inside a trigger body, where SQLite rejects a schema-qualified name on `INSERT`/`UPDATE`/`DELETE`. Both still resolve to `main`: the trigger's own name is qualified to it, a qualified trigger's `ON` table must live in the same schema, and a trigger in `main` resolves its body's names there (§6 scenario 7 tests this with TEMP tables named like a base table and like every shadow table).

**Global tables**, created with the first view and dropped with the last:

```sql
__ivm_meta(key TEXT PRIMARY KEY, value) -- ('format', 1)
__ivm_view(name TEXT PRIMARY KEY, sql TEXT NOT NULL, plan TEXT NOT NULL,
           declaration TEXT NOT NULL, format INTEGER NOT NULL)
__ivm_dep(view TEXT, tbl TEXT, shape TEXT NOT NULL, PRIMARY KEY(view, tbl))
__ivm_progress(view TEXT, tbl TEXT, applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl))
```

`plan` is the lowered plan's canonical text — the plan fingerprint §4.4 asked for. It is stored as text rather than a hash so a mismatch can be read. `declaration` is the `CREATE TABLE` statement the virtual table was declared with, kept so a view whose SQL no longer compiles can still be opened, and so dropped (§5). `shape` is the base table's column shape at create — its columns' names and types, in order, as the catalog reports them (`"region" TEXT, "amount" INTEGER`) — compared on every connect and refresh (§5): the plan names columns by position only, so a table dropped and recreated with other column types could otherwise compile to the same plan.

> **Superseded by Phase 3b §3:** `shape` moves off `__ivm_dep` onto a new per-table `__ivm_tracked` row, since Phase 3b's capture triggers are shared by every view of a table rather than owned by one.

**Per base table:** `__ivm_delta_<t>(__ivm_seq INTEGER PRIMARY KEY AUTOINCREMENT, __ivm_w INTEGER NOT NULL, <every column of t>)` — its own two columns are prefixed so a base column literally named `seq` or `w` is not shadowed — and three triggers `__ivm_trig_<t>_ins`, `__ivm_trig_<t>_del`, `__ivm_trig_<t>_upd` (the `trig_` infix so no base table name and event can spell the same string as an apply trigger's `__ivm_apply_<view>`), exactly as §8.1 (an UPDATE writes −1 OLD then +1 NEW). A base column whose name starts with `__ivm_`, matched case-insensitively, is rejected at `xCreate`: it would collide with the delta table's own columns or a future one.

**Per stateful operator:** `__ivm_state_<view>_<node>_<role>(key BLOB NOT NULL, val BLOB NOT NULL, w INTEGER NOT NULL, PRIMARY KEY(key, val)) WITHOUT ROWID`. `<node>` is the operator's pre-order index in the plan (§4.4, unchanged); `<role>` is a fixed string — `join_left`, `join_right`, `agg_groups` — never the enum's `Debug` output. A row whose weight reaches 0 is deleted (§5.1). State tables are created with the view; on reopen a missing state table is an error, never an empty arrangement.

**Row encoding** for `key` and `val`: each value is a tag byte followed by its payload — `0x00` NULL; `0x01` INTEGER, 8 bytes big-endian two's complement; `0x02` TEXT, 4-byte big-endian length then the UTF-8 bytes. The same row always encodes to the same bytes (§7's canonical-encoding requirement). Decoding anything else is a `StateError`.

**Staging:** `__ivm_stage_<view>(op, arr, key, val, w, tbl, seq, c0 … cN, armed)` and its trigger `__ivm_apply_<view>`: every change a refresh makes is first written here, then applied in one statement (§5).

**Output:** `__ivm_out_<view>(<output columns>, __w INTEGER NOT NULL)` — a plain table, readable without the extension. Its columns carry the compiled view's names and types. `__w` is always 1 (§5.2's root-aggregate rule); any other weight is reported as a broken invariant.

## 5. Lifecycle

**xCreate** (`CREATE VIRTUAL TABLE v USING ivm('<sql>')`), inside the statement's own transaction. First, a view name that starts with `__ivm_`, matched case-insensitively, is refused: it would collide with, or be mistaken for, a shadow table. Then:

1. The database encoding must be UTF-8. TEXT comparison is by UTF-8 byte order (the BINARY collation of §7.1 holds only then): measured, `'Ā' > 'a'` is true in a UTF-8 database and false in a UTF-16LE one.
2. Compile the SQL with `ivmlite-sql` against the SQLite catalog, which reads `pragma_table_list`, `PRAGMA table_info` and the table's `CREATE` statement — matching the table name case-insensitively, the way SQLite's own catalog does — and rejects: an unknown table, a virtual table, an `__ivm_*` table, a table that is not STRICT, a base column whose name starts with `__ivm_` (case-insensitively), an `ANY` column, any `COLLATE` in the DDL (§7.1's conservative rule), any column type other than INTEGER or TEXT, and a table whose DDL declares `ON CONFLICT REPLACE` (see the capture limitation at the end of this section). Both DDL checks scan the whole statement but match keywords as whole tokens — whitespace and comments separate tokens, and a quoted string or identifier is one opaque token — so a column named `collateral` is not a COLLATE clause.
3. Phase 3a only: if a base table already has a delta table (another view tracks it), fail — Phase 3b §4 lifts this.
4. Create the global tables if missing (checking the stored format), the delta tables and triggers, the state tables and the output table; record `__ivm_view`, `__ivm_dep`, `__ivm_progress`.
5. Bootstrap: read each base table in full, push it through the operator tree as the first batch, write the output, and set each progress row to the delta table's current high watermark. The delta table and triggers were created in the same transaction, so the high watermark is 0 and every later write is captured — §7.3's atomicity holds by construction.
6. Declare the table: the output columns plus a hidden column named after the view, the FTS5 command idiom. A result column is therefore rejected if it is named like the view (case-insensitively), like `__w` (the output table's own weight column), or like one of SQLite's rowid aliases `rowid`, `oid`, `_rowid_` (any case, since SQLite's own name matching is case-insensitive) — a result column that shadowed one of these otherwise left the output empty after an UPDATE, reproduced with `SELECT k AS rowid, SUM(x) FROM t GROUP BY k`.

A `CREATE VIRTUAL TABLE` that fails part way leaves nothing behind, in autocommit mode and inside an explicit transaction alike: the statement writes `sqlite_schema`, so SQLite rolls it back as a whole (measured).

**xConnect** (reopen): declare the stored `declaration`, then check the view: the stored SQL must recompile, its canonical plan text and the format must equal the stored ones, every shadow table must exist, each base table must still have the shape recorded in `__ivm_dep` and its three capture triggers `__ivm_trig_<t>_{ins,del,upd}`, and the view's apply trigger `__ivm_apply_<view>` must exist on the stage table (all looked up in `sqlite_schema`, each trigger's `tbl_name` included). `DROP TABLE t` drops t's triggers but not its delta table, so without the trigger check a recreated `t` verified and every later write went uncaptured, and without the apply trigger a refresh applied nothing — both silently. The trigger's table is checked, not only its name: `ALTER TABLE t RENAME TO u` carries t's capture triggers to `u` under their old names, so a new `t` of the same shape passed a name check while the view went on following `u` (external review of 3c287f6, measured). A view that fails a check still connects — SQLite must connect a virtual table before it can drop it, so failing here would make a broken view impossible to drop — and every read and refresh of it then fails with the reason and the advice to drop and recreate it.

**xBestIndex / xFilter / xColumn:** a full scan of `__ivm_out_<view>`.

**xUpdate:** only `INSERT INTO v(v) VALUES('refresh')` is accepted; any other insert, update or delete, or another command, is an error — including `INSERT INTO v(v) VALUES('refresh')` itself if any of the row's other, output-column values is non-NULL (`INSERT INTO v(col, v) VALUES (1, 'refresh')` fails rather than silently dropping the `1`). Refresh first repeats xConnect's shape and trigger checks — a mismatch is the same broken-view error — and then:

> **Amended 2026-10-08, M1b Phase 5:** a refresh repeats those checks only when `PRAGMA schema_version` has changed since they last passed on this connection; it always reads each base table's latch ([Phase 5 spec §3](2026-10-08-m1b-phase5-refresh-performance-design.md)).

1. for each base table of the compiled view, in the order of its `tables` (the anchor table first), read the deltas with `seq` above its progress, in `seq` order;
2. push them through the operator tree, whose arrangements read the state tables and **buffer** every write in memory, overlaying it on what later reads see;
3. write every buffered state change, every output change and every new watermark into the stage table (emptied first);
4. apply them all with **one** statement, `UPDATE __ivm_stage_<view> SET armed = 1`, whose trigger performs each row's change. This is the refresh's last statement: the stage keeps the applied rows until the next refresh (or the bootstrap's until the first) empties it in step 3.

   > **Amended 2026-10-08, M1b Phase 5:** format 4 changes step 4 and its last sentence. The stage has no `armed` column. Each apply stages one sentinel row with `op = 'arm'` and arms with `UPDATE … SET op = 'apply' WHERE rowid = <the sentinel's rowid>`. That fires the stage's `AFTER UPDATE OF op` trigger once, and its body applies the whole stage with set-based statements ([Phase 5 spec §4](2026-10-08-m1b-phase5-refresh-performance-design.md), with its amendment). It is still one statement and the last of the refresh. Bootstrap now empties the stage after its apply (Phase 5 spec §5), so only a refresh's rows stay in the stage, until the next refresh empties it in step 3.

State, output and watermarks therefore commit together or not at all. The obvious design — wrap the refresh in a `SAVEPOINT` — does not work, and neither does relying on SQLite (both measured while prototyping): inside `xUpdate` a `SAVEPOINT` fails with `cannot open savepoint - SQL statements in progress`, and when `xUpdate` returns an error inside an explicit transaction SQLite does **not** undo the writes the callback made through its own statements. A single statement, by contrast, is atomic on its own: if any row of step 4 fails, SQLite rolls the whole statement back. A failure before step 4 changes only the stage table, which the next refresh empties. Nothing may run after step 4: inside an explicit transaction, a failure there would report an error for changes that stay applied (an earlier step 5 that emptied the stage did exactly that when the `DELETE` was refused — external review of 3c287f6, measured).

This atomicity covers failures that roll a statement back — every ordinary error, and the ABORT and ROLLBACK resolutions. It does not survive a user trigger on a shadow table that resolves otherwise (measured on a one-statement `UPDATE … SET armed = 1` apply): `RAISE(FAIL)` stops the statement but keeps the rows it already changed, and `RAISE(IGNORE)` silently skips one row's change while the statement succeeds. Adding triggers to shadow tables, or otherwise tampering with them, is unsupported; §6 scenario 3's fault-injection triggers use `RAISE(ABORT)`.

**xDestroy** (`DROP TABLE v`): drop the triggers first, then the delta tables, the state tables, the output and stage tables and the view's metadata rows; drop the global tables when no view is left. The base tables stay writable (M-1 scenario 9). It works from the recorded metadata alone — the base tables from `__ivm_dep`, the state tables by their names, matched exactly as `__ivm_state_<view>_<node>_<role>` so a view `v` never claims a view `v_1`'s tables — so a view that can no longer be compiled can still be dropped.

**Every callback** runs inside `catch_unwind`; a panic becomes an SQLite error, never an unwind across the FFI boundary. Guarding only the entry point is not enough. One input does reach a panic: a SUM past `i64::MAX` panics in a debug build ("attempt to add with overflow" in `agg.rs`), and create and refresh report it as an SQLite error — tested, and removing a callback's guard makes that test abort the host process. A release build has no overflow checks and wraps silently instead, which stays within the parent spec's "overflow is undefined" (§6.1).

**Known limitations:**

- **REPLACE conflict resolution needs `PRAGMA recursive_triggers = ON` on the writing connection.** `INSERT OR REPLACE`, `REPLACE INTO`, `UPDATE OR REPLACE` and an `ON CONFLICT REPLACE` constraint delete the conflicting row without a DELETE statement, and SQLite fires the DELETE trigger for such a row only when the connection doing the write has `PRAGMA recursive_triggers = ON` (off by default). With it on, rowid and UNIQUE conflicts are captured and the view stays equal to the oracle (tested); with it off, the delta table records the new row's +1 but never the removed row's −1, and the view **silently diverges** (measured: after `INSERT OR REPLACE` and `REPLACE INTO` over a table with a UNIQUE column, the view kept the removed rows' contributions). A base table whose DDL declares `ON CONFLICT REPLACE` is refused at xCreate, since every plain INSERT or UPDATE on it could remove rows; statement-level REPLACE on any other table remains a known v0 limitation that Phase 3b §6 addresses. An upsert (`INSERT … ON CONFLICT(…) DO UPDATE`) performs an ordinary UPDATE and is captured without the pragma (tested).
- **`ALTER TABLE t ADD COLUMN` on a base table breaks its view.** The scan reads every column, so the view's SQL now compiles to a different plan (and the table has a different shape); every read and refresh reports it, and the view must be dropped and recreated (tested).
- **A retraction scans the output table.** The apply trigger finds an `out-` row with `SELECT rowid FROM __ivm_out_<view> WHERE <every column> IS …`, and the output table has no index, so each retracted row costs a full scan of the output. A known cost for the M1b Phase 4 benchmark to measure, not fixed here.
- `ALTER TABLE v RENAME TO w` on an ivmlite view leaves it undroppable. `rusqlite` 0.40 exposes no `xRename`, so SQLite renames the table in `sqlite_schema` without telling the extension; `__ivm_view` still holds the row under the old name `v`, so a later `DROP TABLE w` calls `xConnect` for `w`, finds no matching row, and fails with "ivmlite has no record of the view w". Phase 3b §7 revisits this.

## 6. Acceptance scenarios

Each has at least one test in `ivmlite-test` against the real loaded extension:

1. **Differential.** `SqliteExtensionEngine` implements the harness's `Engine`: `create_view` creates the STRICT base tables, loads the initial rows and creates the view from `view_query_to_sql`; `apply` performs real `INSERT`s and `DELETE`s on the base tables; `refresh` sends the command; `materialize` reads `SELECT * FROM v`. It runs the single-table sweep in full and the join sweeps in full or at a stride, decided by measured cost.
2. **Reopen.** The same engine in a mode that closes and reopens the database before every refresh, over a sweep; and a scenario where a connection without the extension writes the base tables before a refresh.
3. **Failure and retry.** A fault injected with a test-only SQL trigger (`RAISE(ABORT)`) on the output table, and another on a state table, makes a refresh fail part way — in autocommit mode and inside an explicit transaction. Afterwards the state tables, the output table and the progress rows are identical to before, and the transaction's earlier statements survive; once the trigger is dropped, a retry matches the oracle.
4. **Atomic create.** Creating a view inside a transaction that is rolled back leaves nothing; creating a view over tables that already hold rows bootstraps them.
5. **Rejections.** Non-STRICT table, `ANY` column, `COLLATE`, UTF-16 database, unknown table, a base column named with ivmlite's own `__ivm_` prefix, a view over an SQL view or an `__ivm_*` table, a base table another view already tracks (**superseded by Phase 3b §4**: sharing is now allowed, subject to its shape and trigger checks), a result column named like the view or `__w` or one of the rowid aliases (`rowid`, `oid`, `_rowid_`, any case), a view over a virtual table, a table declaring `ON CONFLICT REPLACE` (**superseded by Phase 3b §6.1**: such a table is now accepted and maintained), a view named with the `__ivm_` prefix (any case), an unknown command, a refresh command that carries a value in an output column, and `INSERT`/`UPDATE`/`DELETE` of rows on the view each fail with a message that names the problem; a column named `collateral` is accepted. A tampered stored plan, a missing state table, a base table dropped and recreated (its capture triggers gone), recreated with another shape or given a new column, and a missing apply trigger make every read and refresh fail with the reason, and the view can still be dropped. An overflowing SUM fails create and refresh with an SQLite error.
6. **Drop.** After `DROP TABLE v`, `sqlite_master` holds only the user's objects and the base tables stay writable — apart from `sqlite_sequence`, which the delta tables' AUTOINCREMENT creates and SQLite never drops (measured: "table sqlite_sequence may not be dropped"); the tests' `NOT LIKE 'sqlite_%'` filter hides it.
7. **TEMP shadowing.** On the connection that owns the view, a TEMP table named like a base table is neither read by create or refresh nor given the triggers, and TEMP tables named like every shadow table (global, delta, output, stage, state) are never read, written or dropped.
8. **Conflict resolution.** With `PRAGMA recursive_triggers = ON` on the writing connection, `INSERT OR REPLACE`, `REPLACE INTO` and `UPDATE OR REPLACE` (rowid and UNIQUE conflicts) keep the view equal to the oracle; so does an upsert without it.

## 7. Front-end fixes carried into 3a

Found by an external review of the merged front end and reproduced against SQLite 3.53:

- An unaliased result name keeps trailing whitespace inside a line comment (`COUNT(*) -- hi\r\n` gives a name ending in `\r`; `-- hi  ` keeps the spaces). SQLite trims trailing whitespace from the name; the front end must too.
- `1L` is accepted as the integer 1; SQLite rejects the token. Numeric literals with sqlparser's long suffix are rejected.
- `FROM t0 GLOBAL JOIN t1` is accepted with `GLOBAL` ignored; SQLite reads `GLOBAL` as an alias. A join with `global` set is rejected.

## 8. Measured while prototyping (debug build, 2026-09-26)

The whole design above was prototyped and run before the implementation plan was written. The workspace plus the extension's own tests passed at 286 tests. Every join query of both harness databases (2 × 900) ran green against the loaded extension once. Per case, the extension costs about 9 ms for a single-table query and 14 ms for a join, in memory, and about 60 ms when the database file is reopened before every refresh. The plan's sweeps take their strides from these numbers.
