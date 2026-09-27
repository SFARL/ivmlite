# M1b Phase 3b: shared capture, delta GC, REPLACE capture, rename refusal

**Status:** approved 2026-09-27. Amended 2026-09-27 after implementation: §2, §4, §6.2, §6.4, §8 (see below); and after the final review: §3, §6.1, §6.2, §6.3, §6.4 (the unique-index latch, final review C1), §4 and §6.3 (dropping views whose shared capture is broken, final review I2). **Parent specs:** [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md) (§7.2 GC, §7.3 bootstrap watermark, §8.1 triggers) and [2026-09-26-m1b-phase3a-sqlite-extension-design.md](2026-09-26-m1b-phase3a-sqlite-extension-design.md), whose conventions (§4: quoting, `"main"` qualification, unqualified names inside trigger bodies; §5: one-statement apply, broken views still connect, every callback guarded) all still hold. Where this document and Phase 3a disagree, this document is newer and wins.

## 1. Scope

Phase 3b lifts Phase 3a's "one view per base table" and closes its capture gaps:

1. **Shared capture.** Any number of views may read the same base table. A base table has one delta table and one set of capture triggers, created by the first view that reads it and dropped with the last.
2. **Delta GC.** Delta rows every view has consumed are deleted, atomically with the watermark that makes them consumable (parent spec §7.2).
3. **REPLACE capture.** Rows removed by REPLACE conflict resolution are captured without `PRAGMA recursive_triggers`, within the support boundary of §6.4.
4. **Rename refusal.** `ALTER TABLE v RENAME` on an ivmlite view fails with a message, instead of leaving the view undroppable.

Out of scope: benchmarks (M1b Phase 4), which also measure every write-path cost this phase adds; `ALTER TABLE t ADD COLUMN` still breaks every view of `t` (Phase 3a §5).

## 2. Evidence this design rests on

All measured on SQLite 3.51 (CLI) and 3.53 (Python), 2026-09-27, with throwaway scripts that stay out of the repository.

- `INSERT OR IGNORE` on a conflicting row, and an upsert that takes its `DO UPDATE` path, fire BEFORE INSERT but not AFTER INSERT. A row that REPLACE removes fires no trigger unless the writing connection has `recursive_triggers` ON. A trigger body cannot read `pragma_recursive_triggers` ("unsafe use of virtual table").
- With a `NOT NULL` column that has a default, REPLACE substitutes the default for a NULL: BEFORE INSERT sees `NEW.k = NULL`, AFTER INSERT sees the default (external review; reproduced).
- A separate `CREATE UNIQUE INDEX … (k COLLATE NOCASE)` makes REPLACE remove `'A'` for a new `'a'`, which `k = NEW.k` (BINARY) does not find (external review).
- A foreign-key `ON DELETE CASCADE` triggered by a REPLACE deletion fires the child table's DELETE trigger with `recursive_triggers` OFF and ON alike. Cascades need nothing new.
- **The outer statement's conflict clause overrides the clause of statements inside its triggers**: under an outer `INSERT OR IGNORE`, a user trigger's `INSERT OR REPLACE` behaves as `OR IGNORE`.
- SQLite does not specify the order in which several triggers on one event fire, so a user's AFTER trigger may run before ivmlite's. **Measured on SQLite 3.53 (task review, Task 4): the trigger created most recently fires first.** So a user trigger created before the view's capture triggers fires *after* them, and one created after the view fires *before* them.
- **Prototype measurements (pre-probe prototype, before the shipped §6.2 mechanism was implemented).** Random differentials over three table variants (rowid with `INTEGER PRIMARY KEY`, plain rowid, `WITHOUT ROWID`; each with a `NOT NULL DEFAULT 'd'` unique column, a nullable unique column and a composite unique key) and 14 statement kinds (plain, `OR REPLACE`, `REPLACE`, `OR IGNORE`, both upserts, `UPDATE` in plain / `OR REPLACE` / `OR IGNORE`, `DELETE`, `INSERT … SELECT`, key swaps, omitted and NULL-assigned defaulted keys), 300 seeds × 50 statements per variant, 900 runs per cell, refreshing at random points and comparing the view with the base table:

  | writer's `recursive_triggers` | no re-entry | same-table re-entry, never conflicting | same-table re-entry, conflicting |
  |---|---|---|---|
  | always ON, Phase 3a triggers only | 0 | 0 | 0 |
  | always ON, §6 design | 0 | 0 | 0 |
  | always OFF, §6 design | 0 | 672 | 728 |
  | random per statement, §6 design | 0 | 478 | 521 |

  (Divergent runs of 900, **prototype figures**.) Removing each part of the prototype's §6 mechanism in turn — the candidate confirmation, the DELETE trigger's candidate removal, the `rowid = NEW.rowid` confirmation — made the no-re-entry column fail (400/400, 400/400, 194/400 runs in an earlier 400-seed run, **prototype figures**). Two rejected alternatives, also measured against the prototype: a shared candidate table **without** the recursive-triggers probe broke the "always ON, conflicting re-entry" cell (227 of 900) that plain Phase 3a triggers get right; weight-0 markers in the delta resolved at refresh by a per-rowid timeline failed even non-conflicting re-entry (187 of 900), because trigger order scrambles the timeline.

  **What the shipped design's gate mutations actually measured** (Task 4 report, both fix rounds; `scripts/test-all.sh`, baseline 329/0 unless noted): removing the candidate confirmation (recording no −1 at all), or dropping the `NOT NULL` key's COALESCE default, or dropping the rowid-replacement disjunct (`p.__ivm_rid = NEW.rowid OR …`), each turns the random differential red **without any re-entry** (326/3, 328/1 and 326/3 respectively, against the original baseline 329/0). By contrast, each of the following three parts is caught only by a targeted test, never by either random differential — these three were re-measured in the fix round, against its new baseline **331/0** (11 extension unit tests plus 320 in the workspace):
  - the confirmation-when-gone check (`WHERE {gone}` → `WHERE 1`): caught only by a real row at rowid −1 (330/1, baseline 331/0);
  - `forget` (the AFTER DELETE trigger's candidate removal): caught only by a same-table BEFORE trigger's explicit delete, with `recursive_triggers` OFF and the user trigger created *before* the view (330/1, baseline 331/0);
  - the recursive-triggers probe gate (`= 2` → `>= 2`): caught only by the always-ON re-entry differential, with the user triggers created *after* the view (330/1, baseline 331/0).

  **Without re-entry, `forget` alone and the probe gate alone are redundant with each other**: with `recursive_triggers` ON the probe records nothing, so there is nothing for `forget` to forget; with it OFF and no re-entry, every BEFORE empties the pend table before any DELETE trigger can fire between a BEFORE and its AFTER. Mutating both at once, by contrast, turns both differentials red. This contradicts the prototype's single-removal figures above (400/400 for removing `forget`, 227/900 for no probe), which came from a design that differed in some other part; the shipped design's mechanism is correct in every measured cell, and the gap is recorded, not silently resolved, in `docs/mutation-gates.md`.

## 3. Persistence (format 2)

`__ivm_meta`'s format becomes **2**. A database whose stored format is 1 is refused with the existing message ("… has format 1; this extension reads format 2"); there is no migration, since no release ever wrote format 1.

**Global tables**, created with the first view and dropped with the last:

```sql
__ivm_meta(key TEXT PRIMARY KEY, value)                        -- ('format', 2)
__ivm_view(name TEXT PRIMARY KEY, sql TEXT NOT NULL, plan TEXT NOT NULL,
           declaration TEXT NOT NULL, format INTEGER NOT NULL)
__ivm_tracked(tbl TEXT PRIMARY KEY, shape TEXT NOT NULL,
              broken TEXT)                                     -- new: one row per captured base table
__ivm_dep(view TEXT NOT NULL, tbl TEXT NOT NULL, PRIMARY KEY(view, tbl))   -- shape moved to __ivm_tracked
__ivm_progress(view TEXT NOT NULL, tbl TEXT NOT NULL, applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl))
__ivm_probe(n INTEGER NOT NULL)                                -- new, with trigger __ivm_probe_step (§6.2)
```

The shape moves from `__ivm_dep` to `__ivm_tracked` because the capture triggers are per table: they are generated once from the table's shape when the table is first tracked, and every view of the table relies on that same shape.

**Per tracked base table `t`:**

- `__ivm_delta_<t>` — unchanged from Phase 3a.
- `__ivm_pend_<t>(__ivm_rid INTEGER, <every column of t>)` — REPLACE candidates between a BEFORE and its AFTER trigger (§6.2). Empty between statements, apart from the stale leftovers §6.2 explains.
- five triggers `__ivm_trig_<t>_{ins, del, upd, preins, preupd}`. Every event name is free of `_`, so `<t>_<event>` still parses uniquely at its last `_` and no two (table, event) pairs spell the same trigger name.

**Shape** (stored in `__ivm_tracked`, recomputed and compared at every create over a tracked table, every connect and every refresh) is text that covers everything the generated triggers depend on:

- the columns' names and types in order (Phase 3a's shape);
- whether the table is `WITHOUT ROWID`;
- every unique index — every `pragma_index_list` row with `unique = 1`, whatever its `origin` (`pk`, `u` or `c`) — as the list of its key columns, each with its collation, plus the index's partial flag and whether any key is an expression. Indexes are listed in a canonical order (sorted by that text), so an index's name never matters;
- for each `NOT NULL` column that is a key of some unique index, its default's text as `pragma_table_info` reports it (absent → none).

The shape is text so a mismatch can be printed. Any difference makes every view of the table a broken view (Phase 3a §5), including a unique index added after the view was created: triggers built without it would miss its REPLACE deletions.

**The shape alone is only a snapshot.** It is compared when a view is created, connected or refreshed, so it cannot see a unique index that was added, used by a REPLACE and dropped again between two refreshes: the shape is equal again, and the removed row was never captured (final review C1, measured: the refresh succeeded with a count of 2 against SQLite's 1). The same holds after a refresh has already reported the added index: once the user drops it, the next refresh succeeded with a wrong count. So the unique-index set is **also latched at write time**, in `__ivm_tracked.broken` (§6.2, §6.3). `broken` is NULL until a capture trigger sees unique indexes other than the ones it was generated from; it then holds the reason, and nothing ever clears it.

## 4. Sharing a base table

**xCreate** changes from Phase 3a §5:

- Step 3 ("a tracked table is refused") is removed. For each base table of the view:
  - **untracked:** create its delta table, pend table and five triggers, and insert its `__ivm_tracked` row;
  - **tracked:** its current shape must equal the stored shape, and its five triggers must exist on it (the `tbl_name` check). If not, creation fails with the reason and the names of the views that read the table, telling the user to drop them first.
- **Watermark.** Each progress row starts at the table's current high sequence number, `SELECT seq FROM "main".sqlite_sequence WHERE name = '__ivm_delta_<t>'`, or 0 when there is no row. It is not `MAX(__ivm_seq)`: after GC has emptied the delta table `MAX` is NULL, although AUTOINCREMENT will never reuse a consumed number. The sequence is read inside xCreate, in the `CREATE VIRTUAL TABLE` statement's own write transaction. No other connection can write between that read and the bootstrap's scan of the base table, so the parent spec's §7.3 atomicity holds.
- `__ivm_dep` rows no longer carry a shape.

**xDestroy** (`DROP TABLE v`), from the recorded metadata alone as in Phase 3a:

1. delete v's rows from `__ivm_view`, `__ivm_dep` and `__ivm_progress`; drop its state, output and stage tables (the apply trigger goes with the stage table);
2. for each base table v read:
   - **no view reads it any more:** drop its five triggers first, so the table stays writable, then its delta and pend tables and its `__ivm_tracked` row (when `__ivm_tracked` still exists);
   - **another view still reads it:** run the table's GC delete (§5), since v may have been the slowest reader — when its delta table still exists;
3. when no view is left, drop the global tables, including `__ivm_probe`, each with `DROP TABLE IF EXISTS`: a view whose probe table (or another global table) was dropped by hand is a broken view (§6.3's checks), and it must still be droppable.

The two "when … still exists" conditions in step 2 serve the same rule (final review I2, measured): with two views over `t`, a user who dropped `__ivm_delta_t` could drop neither view (`SQL logic error` from the GC delete), and one who dropped `__ivm_tracked` could not drop the last one.

## 5. Delta GC

The apply trigger (Phase 3a §5, `__ivm_apply_<view>`) gains one statement per base table `t` of the view, directly after the watermark `UPDATE`:

```sql
DELETE FROM "__ivm_delta_<t>"
 WHERE NEW.op = 'progress' AND NEW.tbl = '<t>'
   AND __ivm_seq <= (SELECT MIN(applied_seq) FROM "__ivm_progress" WHERE tbl = '<t>');
```

It runs only for the stage row that advances `t`'s watermark, inside the one arming statement, so GC commits with state, output and watermarks or not at all, and nothing runs after the apply (Phase 3a §5). `__ivm_seq` is the delta table's `INTEGER PRIMARY KEY`, so this is a range delete on the rowid.

A view that is never refreshed holds back every delta row after its watermark: GC follows the slowest reader by design, and there is no expiry.

## 6. REPLACE capture

### 6.1 Schema rules

The catalog (`SqliteCatalog::table`) additionally refuses:

| refused | why |
|---|---|
| a unique index with a non-BINARY collation on any key | the candidate lookup compares with `=` (BINARY) and would miss rows the index considers equal |
| a partial unique index | ignoring its `WHERE` stays correct, but the lookup then cannot use the index and scans the table on every write; not in v0 |
| a unique index with an expression key (`cid = -2`) | no candidate lookup can be generated |
| a `NOT NULL` column that is a key of a unique index, with a default that is not a literal | REPLACE substitutes the default for a NULL (§2); a literal can be substituted in the lookup, and anything else (`CURRENT_TIMESTAMP`, `random()`, …) cannot be re-evaluated deterministically |
| a base column named `rowid`, `oid` or `_rowid_` (any case) | the triggers address rows by `rowid` |

A **literal default** is, as `pragma_table_info.dflt_value` reports it: `NULL` in any case, an optionally signed decimal integer, a hexadecimal integer (`0x…`), or a single-quoted string whose embedded quotes are doubled. `DEFAULT (5)` is reported as `5` and is accepted.

Phase 3a's refusal of `ON CONFLICT REPLACE` in a table's DDL is **lifted**: such a table's REPLACE deletions are captured like statement-level ones.

Because these rules live in the catalog, a unique index added later also makes the view's SQL fail to compile at connect, and the shape check catches it at refresh while the index exists. An index that is added, written through and dropped again between two refreshes leaves nothing for either to see; the write-time latch (§6.2, §6.3) catches it instead. Together they keep the view broken, never silently wrong: every write that could remove a row through an index the triggers were not generated for runs a BEFORE trigger, and that trigger latches the change before the write happens.

### 6.2 The mechanism

For base table `t` with columns `C`, the **candidates** of a new row are the existing rows that the row could replace. For a rowid table they are the union of:

- `SELECT __ivm_b.rowid, C FROM t AS __ivm_b WHERE __ivm_b.rowid = NEW.rowid`;
- for each unique index with keys `k1 … kn`, `SELECT __ivm_b.rowid, C FROM t AS __ivm_b WHERE __ivm_b.k1 = e1 AND … AND __ivm_b.kn = en`, where `ei` is `COALESCE(NEW.ki, <literal default>)` for a `NOT NULL` key with a literal default and `NEW.ki` otherwise.

The branches are joined with `UNION` so each can use its own index. `NEW.rowid` is −1 in a BEFORE INSERT without an explicit rowid (measured), which can only add a harmless extra candidate. For a `WITHOUT ROWID` table, the rowid branch is absent (its primary key is one of the unique indexes) and `NULL` stands in for `rowid`. In BEFORE UPDATE, each branch also excludes the row being updated: `AND __ivm_b.rowid <> OLD.rowid`, or for `WITHOUT ROWID`, `AND NOT (__ivm_b.p1 = OLD.p1 AND …)` over the primary-key columns.

**Every candidate subquery aliases the base table as `__ivm_b` (the confirmation below aliases the pend table as `__ivm_p`), and every column it selects is qualified through that alias.** Without it, a base table literally named `p`, `old` or `new` (any case) would capture the `p.`, `OLD.` or `NEW.` references the generated SQL means for the pend table or the trigger's own pseudo-rows — measured (task review): without the alias, a `WITHOUT ROWID` table named `p` or `P` silently diverged after all three writes (`INSERT OR REPLACE` and both `UPDATE OR REPLACE`), and one named `old` or `OLD` silently diverged after the two `UPDATE OR REPLACE` writes only. A rowid table under any of these names, and a table of either layout named `new`/`New`, did not diverge; for `new`, the unaliased cost is only an avoidable full scan, not a wrong result. The catalog refuses `__ivm_*` base tables outright (Phase 3a §5, xCreate step 2), so `__ivm_b` and `__ivm_p` cannot collide with a base table's own name.

**The recursive-triggers probe.** `__ivm_probe` has one trigger, `__ivm_probe_step AFTER INSERT ON __ivm_probe WHEN NEW.n < 2 BEGIN INSERT INTO __ivm_probe VALUES (NEW.n + 1); END`. With `recursive_triggers` OFF, the trigger fires for the first row but not for the row it inserts itself, so inserting a 0 leaves 2 rows; with it ON, 3.

Triggers, for a rowid table (`WITHOUT ROWID`: the identity comparisons use the primary-key columns and `__ivm_rid` stays NULL):

```sql
-- BEFORE INSERT (preins); BEFORE UPDATE (preupd) is the same with the exclusion above
UPDATE __ivm_tracked SET broken = 'the unique indexes of <t> changed after its capture was generated'
 WHERE tbl = '<t>' AND broken IS NULL
   AND (<fingerprint of t's explicit unique indexes>) IS NOT <the fingerprint when t was tracked>;
DELETE FROM pend;
INSERT INTO probe(n) SELECT 0 WHERE EXISTS (<candidates>);
INSERT INTO pend(__ivm_rid, C) SELECT * FROM (<candidates>) WHERE (SELECT count(*) FROM probe) = 2;
DELETE FROM probe;

-- AFTER INSERT (ins)
INSERT INTO delta(__ivm_w, C) SELECT -1, C FROM pend AS __ivm_p
  WHERE __ivm_p.__ivm_rid = NEW.rowid
     OR NOT EXISTS (SELECT 1 FROM t AS __ivm_b WHERE __ivm_b.rowid = __ivm_p.__ivm_rid);
DELETE FROM pend;
INSERT INTO delta(__ivm_w, C) VALUES (1, NEW.C);

-- AFTER UPDATE (upd): the same confirmation and DELETE FROM pend, then −1 OLD and +1 NEW as in Phase 3a

-- AFTER DELETE (del)
DELETE FROM pend WHERE __ivm_rid = OLD.rowid;
INSERT INTO delta(__ivm_w, C) VALUES (-1, OLD.C);
```

(`delta`, `pend` and `probe` stand for `__ivm_delta_<t>`, `__ivm_pend_<t>` and `__ivm_probe`, unqualified as every trigger body requires. `t` is aliased `__ivm_b` and `pend` is aliased `__ivm_p`, as above.)

- **The latch.** The fingerprint is `SELECT group_concat(sql, char(10)) FROM (SELECT sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = '<t>' COLLATE NOCASE AND sql LIKE 'CREATE UNIQUE INDEX%' ORDER BY name)`: every explicit unique index's statement, in name order, or NULL for none. It is computed by running that query when `t` is tracked and embedded in the trigger as a literal; the trigger runs the same query and compares with `IS NOT`. SQLite stores every such statement with its prefix normalized to `CREATE UNIQUE INDEX ` (measured, for `create  unique index if not exists …` and for a leading comment), so the `LIKE` needs nothing more. Autoindexes (`sql IS NULL`) are left out: a table's `UNIQUE` and `PRIMARY KEY` constraints change only through a rebuild, which drops the triggers too. A trigger in `main` reads `main`'s `sqlite_schema` (measured, with an attached database holding a same-named table and unique index). The latch is conservative: dropping and recreating an index under another name, or with other whitespace, also latches, although the triggers would still be right. Non-unique indexes never latch, and neither does an index added and dropped with no write to `t` in between, since nothing ran without it.
- **Recording.** With `recursive_triggers` OFF, BEFORE records the candidates.
- **Confirmation.** AFTER confirms a candidate as removed when it is gone from `t`, or when the new row now holds its rowid (a rowid REPLACE, including one with identical values). That is when the −1 is recorded.
- **Rows that are not written.** A row that `OR IGNORE`, `DO NOTHING` or an upsert's `DO UPDATE` path does not write fires no AFTER INSERT. Its candidates are never confirmed, and the next BEFORE discards them.
- **Recursive triggers ON.** Nothing is recorded, and SQLite's own DELETE trigger captures each removed row, exactly as in Phase 3a. The AFTER DELETE removes the row's candidate in either mode, so no removal is counted twice.

### 6.3 Checks

At every connect and refresh, and at every create over a tracked table, the view is checked:

- all five capture triggers of each base table exist on that table (the `tbl_name` check from the 3c287f6 review);
- `__ivm_probe_step` exists on `__ivm_probe`;
- the table's delta and pend tables exist (the delta table was checked only at connect before the final review, so a refresh on an already-connected connection failed with a bare "no such table" instead of as a broken view);
- the shape equals `__ivm_tracked`'s;
- `__ivm_tracked.broken` is NULL: no capture trigger has latched a unique-index change (§6.2). It is checked first, and its text is the reason the view reports. A latched table also refuses every new view over it.

### 6.4 Support boundary

Capture is exact:

- with `recursive_triggers` ON, for every statement (as in Phase 3a);
- with it OFF, for every statement that does not write the same base table again from inside its own trigger program — directly, or through triggers on other tables.

**One case is not supported:** a writer with `recursive_triggers` OFF whose REPLACE-style deletion happens while such a same-table re-entry is running. Neither the candidate table nor any trigger-only design measured (§2) can capture or detect it. The view may silently diverge.

Because SQLite does not specify trigger firing order (§2), the re-entry differential (§8 scenario 4) runs with the user's re-entrant triggers created **both before and after** the view — on SQLite 3.53 this is the only way to exercise both firing orders, since the trigger created most recently fires first (§2).

The documentation says so plainly and recommends `PRAGMA recursive_triggers = ON` for any writer of a database whose tracked tables have triggers that write back to the same table. Triggers that write only to other tables are unaffected. A foreign-key cascade is not a re-entry (§2).

**Write cost.** Every INSERT and UPDATE of a tracked table now runs the candidate lookup (one lookup per unique index, plus the rowid lookup), the latch's read of `sqlite_schema` (a scan of the whole schema, once per written row) and a few statements on small tables. No performance claim is made here: Phase 4 measures it against Phase 3a's triggers.

## 7. Rename refusal

The module is registered with an `xRename` that refuses: *"ivmlite views cannot be renamed; drop the view and create it again under the new name"*. SQLite fails `ALTER TABLE v RENAME TO w` when `xRename` returns an error, so the rename never happens.

`rusqlite` 0.40 exposes no `xRename`. Its `Module<'vtab, T>` is `#[repr(transparent)]` over `ffi::sqlite3_module`. The extension therefore:

- builds `Module::<IvmTab>::update_module()` in a **`static` initializer** (a `const` context), transmutes it to `ffi::sqlite3_module` and sets `xRename`;
- registers that static with `ffi::sqlite3_create_module_v2`.

The module struct therefore lives for the whole process, as SQLite requires of a registered module: never a stack temporary, never a copy dropped after registration. The error message is allocated with `sqlite3_malloc64` into `sqlite3_vtab.zErrMsg`, after freeing any earlier message with `sqlite3_free`, as SQLite expects. The transmute is sound only while `Module` stays `repr(transparent)`. The `SAFETY` comment names the pinned `rusqlite` version, and a test proves the registration works. The implementation first confirms that SQLite calls `xRename` for a virtual table and aborts the rename on an error; if it does not, the task stops and reports.

Renaming a **base** table keeps making its views broken (the `tbl_name` check). Dropping those views then drops the triggers by name from whichever table they now sit on.

## 8. Acceptance scenarios

Each has at least one test against the real loaded extension, landing with the task that implements it.

1. **Multi-view differential.** The harness's `SqliteExtensionEngine` gains a mode with **sibling views**. Besides the view under test, `create_view` creates:
   - a sibling with the same SQL;
   - one sibling per base table from the harness's single-table query space.

   Siblings refresh at every second harness refresh, so they lag behind and hold back GC. Each sibling refresh compares the sibling with SQLite evaluating its SQL directly, and a mismatch fails the refresh. The same-SQL sibling is dropped after the second refresh, exercising a drop with readers remaining. The mode runs over the single-table space and at a stride over the join spaces, in memory and in reopen mode.
2. **Sharing lifecycle.**
   - Two views over one table, then a join view over it and a second table: each matches its oracle across writes and refreshes, including after a reopen.
   - Dropping them in any order leaves the others correct. After the last drop only the user's objects remain, apart from `sqlite_sequence`.
   - A view created while another view's unconsumed deltas exist starts from the current sequence and never replays older rows.
   - **GC then create:** once GC has emptied the delta table, a second view created over it starts from `sqlite_sequence`, not from `MAX(__ivm_seq)`.
3. **GC.**
   - After every reader has refreshed, the table's delta table is empty. A lagging view keeps exactly the rows after its watermark.
   - Dropping the slowest view deletes what only it held back.
   - A fault injected on the delta table's deletion (a test `BEFORE DELETE` trigger with `RAISE(ABORT)`) fails the refresh with delta, state, output and watermarks unchanged, in autocommit mode and in an explicit transaction, and a retry succeeds.
4. **REPLACE differential.** The §2 generator ported to a Rust test over the three table variants, with `recursive_triggers` chosen at random per statement, some statements written by a connection without the extension, and refreshes at random points. After each refresh, two views are checked against SQLite's own evaluation: a view that groups by every column (so it holds the table's full contents) and a coarse `GROUP BY x` view — the full-contents view cannot see a row that was retracted twice, since a doubly-retracted group vanishes exactly like a singly-retracted one, so the coarse view is needed to catch that failure mode. The re-entry variants also run with `recursive_triggers` always ON and must stay exact, and with the re-entrant user triggers created both before and after the view (§6.4), since SQLite's trigger firing order is unspecified.
5. **REPLACE rules.** Each §6.1 refusal fails creation with a message that names it; a table declaring `ON CONFLICT REPLACE` is now accepted and maintained. Adding a unique index after creation makes the view report the change and still drop.
6. **Rename.** `ALTER TABLE v RENAME TO w` fails with the §7 message, in autocommit mode and in an explicit transaction; v still refreshes and reads, and can be dropped.
7. **Format.** A database whose `__ivm_meta` says format 1 is refused by create and by connect.

Every new check gets a row in `docs/mutation-gates.md`, verified with `scripts/test-all.sh`.

## 9. Known limitations after Phase 3b

- The §6.4 case: `recursive_triggers` OFF plus a REPLACE-style deletion during same-table re-entry.
- `ALTER TABLE t ADD COLUMN` breaks every view of `t` (unchanged).
- A retraction scans the output table (unchanged; Phase 4 measures it).
- A never-refreshed view holds back GC of its tables (§5).
- `sqlite_sequence` stays after the last view is dropped (unchanged).
