# M1b Phase 5: refresh performance

**Status:** approved 2026-10-08. **Parent specs:**
- [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md), §10;
- [2026-09-28-m1b-phase4-benchmark-design.md](2026-09-28-m1b-phase4-benchmark-design.md), the measurement protocol, which is reused unchanged;
- the Phase 3a/3b extension specs, whose conventions all still hold.

## 1. Scope

Phase 4 met §10.2's "must" bar but missed "expected": on total apply + refresh time, ivmlite stays 4–15x slower than hand-written triggers in most confirmed cells. Phase 5 attacks the cause measured in §2, refresh cost, with four changes:

1. **Skip the per-refresh catalog checks when the schema has not changed** (§3).
2. **Apply a refresh with set-based statements fired once**, not with a trigger fired once per stage row (§4). The shadow-table format becomes **4**.
3. **Empty the stage table at the end of bootstrap** (§5).
4. **One profile-driven change** to whatever dominates refresh after 1–3 (§6).

Each change is measured with Phase 4's harness, by an ablation (§7), and Phase 4's full matrix and confirmation are re-run.

**Not in scope:**
- **The write path.** The latch's `sqlite_schema` scan grows with the view count. Fixing it means consolidating the per-view shadow tables, a larger restructuring. ivmlite's write cost is already below hand-written triggers from 50 views up (Phase 4).
- **A wider SQL subset.**
- **The M2 items.**

## 2. Evidence (throwaway measurements, 2026-10-08)

Release build, Python 3.13 with SQLite 3.53, in-memory databases, the m0 `orders` shape and views. The scripts stay in the session scratchpad; §7's ablation is the formal before/after.

**Refresh dominates.** It is 85–100% of ivmlite's apply + maintain in every 100,000-row cell of `docs/bench/m1b-phase4.csv`:

| views | batch | apply + maintain vs hand-written triggers | share that is maintain |
|---:|---:|---:|---:|
| 10 | 1 | 140x | 98% |
| 10 | 1000 | 7.1x | 91% |
| 200 | 1 | 4,572x | 100% |
| 200 | 1000 | 10.9x | 98% |

**A refresh's fixed cost grows with the schema.** An empty refresh, measured per view at 10,000 base rows and 1,000 groups:

| views | `sqlite_schema` rows | empty refresh per view | one `pragma_table_list` lookup | one `sqlite_schema` lookup |
|---:|---:|---:|---:|---:|
| 1 | 27 | 0.164 ms | 6.0 µs | 2.4 µs |
| 10 | 81 | 0.208 ms | 25.1 µs | 4.0 µs |
| 50 | 321 | 0.611 ms | 101.6 µs | 12.7 µs |
| 200 | 1,221 | 1.847 ms | 374.8 µs | 48.1 µs |

Every refresh repeats the catalog checks: base-table shape and unique-index metadata (`pragma_table_list`, `pragma_table_info`, `pragma_index_list`, `pragma_index_xinfo`), the five capture triggers, the probe, the pend table, the output index and the apply trigger. Each is a scan of the schema. Refreshing N views therefore costs O(N × schema) = O(N²).

**The apply is most of a refresh.** A `sample` profile of 10 views over 100,000 rows with 1,000 groups, refreshed after every 1,000-row insert (94.9 ms per batch), attributes:
- `view::apply` 76% of `view::refresh`, almost all of it the arming `UPDATE`, which fires the apply trigger once per stage row;
- `view::compute` 24%;
- `check_capture` about 2% at this view count.

One view's refresh stages about 1,900 rows. Each row runs the trigger's whole body of 7–8 statements, most of them no-ops behind `NEW.op = …` guards.

**A set-based apply is faster.** A pure-SQL replica applied the same 1,897 stage rows two ways:
- with today's row trigger: median 3.89 ms;
- with one trigger firing whose body runs set-based statements over the stage: median 1.74 ms.

That is 2.2x faster, with identical resulting state, output and watermark.

## 3. Schema-version-gated checks

Each open view keeps, in its `IvmTab`, the `PRAGMA schema_version` value at which its catalog checks last passed. On this connection that is the `verify` at connect or create, or the last full check in `refresh`. The view also caches its base tables' `Schema`s, which `refresh` needs to read deltas.

At refresh:
- read `PRAGMA schema_version`;
- **if it equals the cached value**, skip `check_capture`, `check_output_index` and the per-table `base_schema` reads, and use the cached `Schema`s;
- **otherwise**, run the full checks as today, and cache the new version and schemas only if they pass;
- **in either case**, still read each base table's `__ivm_tracked.broken` latch, one primary-key lookup per table: the latch is set by a data write and never changes the schema.

**Why this is sound.** SQLite increments the schema cookie on every schema change made by any connection to the database: `CREATE`, `DROP`, `ALTER`, `CREATE INDEX` and trigger changes alike. Reading `PRAGMA schema_version` reads that cookie; it does not expire prepared statements, unlike a *set* pragma (Phase 4 spec §3 and its RePrepare finding). An unchanged cookie means no object the checks inspect can have changed. The one exception is a user who sets `PRAGMA schema_version` by hand, which SQLite documents as corrupting; that is out of scope, like `writable_schema` (Phase 3b §9).

**Tests:**
- DDL on the same connection, on another connection, and on a connection without the extension (`DROP TRIGGER`, `CREATE UNIQUE INDEX`, `DROP INDEX __ivm_outidx_v`, base-table drop and recreate, `ALTER COLUMN`) still breaks the next refresh with today's messages.
- A latch set by a write, with no schema change, still breaks the next refresh.
- The cache never outlives a failed check.

Every new check gets a verified gate row.

## 4. Set-based apply (format 4)

**Mechanism.** The stage table keeps its columns and loses `armed`. Its apply trigger becomes

`CREATE TRIGGER "main"."__ivm_apply_<view>" AFTER INSERT ON "__ivm_stage_<view>" WHEN NEW.op = 'apply' BEGIN … END`

The arming statement becomes `INSERT INTO stage(op) VALUES ('apply')`. It is still one statement and still the last one of every apply (Phase 3a §5). The trigger fires once, for that row only. Staging inserts evaluate the `WHEN` and skip the body.

**The body.** The stage table carries the reserved alias `__ivm_s` everywhere, so no output-column name can capture a reference (the lesson of Phase 3b's `__ivm_b`). The body runs these set-based statements in this order:

1. Per arrangement *i*:

   ```sql
   INSERT INTO <state_i>(key, val, w)
     SELECT key, val, w FROM stage WHERE op = 'state' AND arr = i AND true
     ON CONFLICT(key, val) DO UPDATE SET w = w + excluded.w;

   DELETE FROM <state_i>
     WHERE w = 0 AND (key, val) IN (SELECT key, val FROM stage WHERE op = 'state' AND arr = i);
   ```

2. Fail if a retraction finds no row:

   ```sql
   SELECT RAISE(ABORT, 'ivmlite broken invariant: …')
     WHERE EXISTS (SELECT 1 FROM stage AS __ivm_s
                   WHERE __ivm_s.op = 'out-' AND NOT EXISTS (<lookup with __ivm_s.cN>));
   ```

3. Retract:

   ```sql
   DELETE FROM <out> WHERE rowid IN (
     SELECT (<lookup with __ivm_s.cN> LIMIT 1) FROM stage AS __ivm_s WHERE __ivm_s.op = 'out-');
   ```

4. Insert: `INSERT INTO <out>(<cols>, __w) SELECT c0 … cN, 1 FROM stage WHERE op = 'out+';`
5. Watermarks: `UPDATE <progress> SET applied_seq = (SELECT seq FROM stage WHERE op = 'progress' AND tbl = <progress>.tbl) WHERE view = '<view>' AND tbl IN (SELECT tbl FROM stage WHERE op = 'progress');`
6. GC, per base table *t*, after the watermark update:

   ```sql
   DELETE FROM <delta_t>
     WHERE __ivm_seq <= (SELECT MIN(applied_seq) FROM <progress> WHERE tbl = 't')
       AND EXISTS (SELECT 1 FROM stage WHERE op = 'progress' AND tbl = 't');
   ```

`<lookup …>` is `retraction_lookup`, the one function that builds the output-index lookup (Phase 4 §4). Its operand becomes `__ivm_s.cN`.

**Retraction semantics.** v0's output Z-set is consolidated, so each output row appears in the stage at most once, with weight ±1. Step 3 therefore removes exactly one copy for each `out-` row, as today. If two identical `out-` rows were ever staged they would both map to one rowid and remove one copy; that cannot happen in v0, and a comment says so.

**Format.** It becomes **4**. A format-3 database is refused, and its broken views can still be dropped (Phase 3a §5). Like format 3, this needs no migration, since this is an alpha.

**Tests:**
- Every existing atomicity and fault-injection test stays green. That covers a `RAISE(ABORT)` on a state table and on the output table, in autocommit and in an explicit transaction, a failing GC, and the "nothing after the apply" test.
- The missing-retraction invariant still raises.
- The plan test (`the_retraction_lookup_searches_the_output_index`) is adapted to the new trigger text. It must still extract the lookup from the stored trigger, show that both uses are byte-identical, and require a SEARCH on every output column.
- A new end-to-end test checks that one refresh fires the body exactly once. It counts with a test-only `AFTER INSERT` trigger on the output table, or an equivalent observable.

Gate rows: remove the `WHEN`; break the per-arrangement state delete; drop the RAISE.

## 5. Bootstrap empties its stage

`create` runs `DELETE FROM stage` after bootstrap's apply. This does not break "nothing after the apply": that rule protects a *refresh* inside an explicit transaction, where a callback's writes are not undone. A failing `CREATE VIRTUAL TABLE` writes `sqlite_schema` and is rolled back as a whole, in autocommit and in an explicit transaction alike (Phase 3a §5, measured). A comment states this distinction.

**Tests:**
- Right after `create`, the stage holds no rows.
- A fault injected on the stage's `DELETE` fails the `CREATE` and leaves nothing behind, in autocommit and in an explicit transaction.

Plus a gate row.

## 6. One profile-driven change

After §3–§5, profile refresh again, with `sample` on macOS or an equivalent, on the workload of §2. The profiling script is kept in `scripts/`. Pick the single largest remaining cost that is at least 20% of refresh, and apply the matching remedy:

| if it is | do |
|---|---|
| inserting stage rows one statement per row | stage with multi-row `INSERT … VALUES (…), (…), …`, using a fixed chunk size and a prepared statement per chunk shape |
| one state-table lookup per key in `BufferedArrangement::get` | prefetch each batch's keys with one query per arrangement |
| something else | record it and stop: a new spec amendment decides |

If nothing reaches 20%, record that and make no change.

**The decision is recorded.** The profile and the choice go into a dated amendment of this spec.

**Tests and gates.** The change gets tests and gate rows like any other.

## 7. Measurement

The protocol is Phase 4 spec §3, unchanged.

- **Ablation.** `scripts/bench-ablation.sh` runs over the reviewed commits:
  - `before`: master at Phase 5 start;
  - `schemaver`, after §3;
  - `setapply`, after §4;
  - `stage`, after §5;
  - `profiled`, after §6, if there is one.

  It uses Phase 4's ablation cells plus one batch=1 cell at 200 views, which isolates the fixed cost of §3. That cell is added to the `[ablation]` section of `workloads/m0-baseline.toml`.
- **Full runs.** Run the full matrix and the confirmation again into `docs/bench/m1b-phase5.csv` and `docs/bench/m1b-phase5-confirm.csv`. Write-amp is not re-run: the write path is unchanged.
- **Reporting.** A new "M1b Phase 5" section in `docs/bench/README.md`. Its tables are generated by a script, like Phase 4's `phase4_tables.py`. It must state:
  - the §10.2 bars again, against hand-written triggers, as medians and ranges from confirmed cells;
  - the ablation, per change;
  - what still loses and why;
  - anything that got worse.

**Release.** The phase ends with `v0.1.0-alpha.3`: a CHANGELOG entry, release notes, and format 4.
