# Selective group-key reads

Date: 2026-10-21. Scope: implementation and measured acceptance after alpha.6;
no version change, release, tag or publication.
The recorded unmodified acceptance baseline is 848 passed, 0 failed and 3
ignored tests. Retain one consolidated final commit after review and checks;
do not require separate document commits or authorize a push.

## 1. Goal and boundary

Restrict the existing owned-row snapshot materialized by an ivmlite cursor when
SQLite supplies usable BINARY comparisons on maintained output group keys.
Support `=`, `>`, `>=`, `<` and `<=`, including bound parameters. For Kener:

```sql
SELECT * FROM kener_bucket_rollup
WHERE monitor_id = ?1 AND bucket_ts >= ?2 AND bucket_ts < ?3;
```

This changes request-time reads only. Maintained-view SQL, explicit refresh,
aggregate semantics, shadow format 6 and release version alpha.6 remain as they
are. Do not add dependencies. Do not consume ordering, push LIMIT/OFFSET, mark
the scan unique, or infer an allocation reduction from fewer returned rows.

## 2. Eligibility and SQLite semantics

`CompiledView.columns` is ordered as group keys followed by aggregates. Derive
the key count from the root `Plan::Aggregate { group_by, .. }.group_by.len()`;
the entries in `group_by` are input indices and must never be interpreted as
output-column indices. Only output positions `0..key_count` are eligible.

A constraint is selected only when `is_usable()`, its output position is a key,
its operator is one of the five listed operators, and
`IndexInfo::collation(index)` reports BINARY (ASCII case insensitive). Rowid,
hidden command, aggregate columns, `IS`, NULL predicates, LIKE, and other
operators are left for SQLite. Non-BINARY and unusable constraints remain
residual predicates. A constraint collation lookup failure propagates as an
error through the existing callback guard rather than permitting unsafe
pushdown.

The virtual declaration and shadow output columns have no declared type and
therefore have **BLOB affinity**, which differs from an expression having no
affinity. For a selected bare-column comparison, SQLite uses NUMERIC affinity
if either operand has numeric affinity, and otherwise BLOB/no conversion.
TEXT RHS affinity cannot propagate TEXT conversion to this BLOB-affinity LHS.
For example, TEXT `42` can join INTEGER 42 after numeric conversion, so a direct
nested `key = ?1` alone would still be unsafe. `xBestIndex` exposes no
RHS-column-affinity API. Preserve candidate supersets for both numeric and
unconverted cases; do not implement comparison/coercion in Rust.

SQLite 3.45's `exprMightBeIndexed` requires `TK_COLUMN` for this scalar column
identity. Unary `+g` and `CAST(g AS ...)` have no selected virtual-table
constraint here: they fall back to SQLite's outer expression evaluation.
Virtual tables do not supply expression indexes. Row-value expansion still
names ordinary BLOB-affinity output columns. Keep explicit expression-fallback
oracles so a future planner change cannot silently invalidate this boundary.

Candidate generation dispatches on runtime RHS storage class. INTEGER and
REAL RHS use the same two-arm candidate for all five supported operators:

```sql
("g" COLLATE BINARY = ?1
 OR ("g" COLLATE BINARY >= '' AND "g" COLLATE BINARY < x''
     AND +CAST("g" AS NUMERIC) COLLATE BINARY = ?1))
```

Replace `=` only with the fixed operator token; the storage-class region bounds
remain `>= ''` and `< x''`. The raw arm preserves unconverted values. Only a
TEXT key can acquire numeric conversion, and the second arm captures its
numeric comparison. Unary `+` strips the CAST expression's affinity so the
branch does not additionally coerce its bind. For numeric RHS `>` and `>=`,
the direct raw comparison alone is already a superset: all TEXT/BLOB keys sort
above the numeric bound, while numeric conversion can only remove candidates.
Using this direct safe simplification is allowed. Equality for both INTEGER
and REAL RHS can seek numeric matches plus the indexed TEXT region. Do not
claim a compound monitor/time index seek.

TEXT RHS uses four raw/NUMERIC Cartesian pairs for every supported operator:

```sql
("g" COLLATE BINARY >= ?1
 OR +CAST("g" AS NUMERIC) COLLATE BINARY >= ?1
 OR "g" COLLATE BINARY >= +CAST(?1 AS NUMERIC)
 OR +CAST("g" AS NUMERIC) COLLATE BINARY >= +CAST(?1 AS NUMERIC))
```

These branches cover actual numeric-looking TEXT RHS values carrying NUMERIC
affinity, as compound subqueries or foreign virtual tables may yield, as well
as ordinary unconverted TEXT. NUMERIC CAST is value-equivalent under SQLite's
numeric comparison where numeric affinity converts a value; raw branches keep
values affinity leaves unchanged. Pin parser/boundary behavior against SQLite.
No TEXT CAST branches are necessary for the selected BLOB-affinity keys.
Do not use `sqlite3_value_frombind` or a known RHS value as an affinity proof.

BLOB RHS never acquires affinity conversion, so every operator uses its direct
comparison. NULL cannot satisfy any supported comparison:

```sql
-- BLOB RHS (operator shown as >=): "g" COLLATE BINARY >= ?1
-- NULL RHS: (?1 IS NOT NULL AND 0)
```

The false NULL expression retains its numbered placeholder so the statement's
bind count remains correct even when it is the last term. The indexed TEXT
region uses SQLite's NULL < numeric < TEXT < BLOB storage-class order.
Non-leading constraints and TEXT candidate predicates may still scan.

Owned `SqlValue::Text` requires valid UTF-8, but SQLite can compare an invalid
UTF-8 TEXT RHS without returning it, for example `CAST(x'80' AS TEXT)`. If
copying any selected filter argument fails specifically with
`rusqlite::Error::Utf8Error(_, _)`, execute the existing full scan with no bound
arguments and let SQLite evaluate all predicates. Do not reject the query,
reinterpret the bytes as BLOB, or discard only part of the predicate. Propagate
all other argument-conversion errors normally. This narrow compatibility
fallback does not broaden ivmlite's persisted TEXT value model.

Set `omit=false` for every selected constraint. SQLite still evaluates its
original expression. Residual filtering removes false positives but cannot
repair false negatives, which is why affinity and collation equivalence are
acceptance requirements. Duplicate constraints get separate arguments; their
conjunction, including conflicting bounds and NULL arguments, is preserved.

SQLite may offer IN as equality and call `xFilter` once per RHS value. Accept
that default behavior and test duplicate values, NULLs, empty lists, bound
lists and subqueries. The extension's standalone rusqlite 0.40.2 configuration
does not enable `modern_sqlite`; do not call its gated batch IN APIs or claim
batch IN support. SQLite controls OR decomposition and unusable join terms.

## 3. Private read plan and execution

Add a private `read_plan` module with a small operator enum and validated plan
descriptor. It is an ephemeral xBestIndex/xFilter protocol, not persisted plan
state. Use a versioned descriptor such as `sr1:0=eq,1=ge,1=lt` and `idxNum=1`;
the no-constraint path uses `idxNum=0`, no descriptor and zero arguments.

The decoder rejects unsupported index numbers, missing or unexpected plan
strings, unknown prefixes/operators, malformed or noncanonical column numbers,
empty terms, positions outside the group-key prefix, and an argument count
that differs from the number of terms. All failures use
`ivmlite internal error: invalid selective-read plan`. Do not interpret invalid
metadata as a full scan or interpolate descriptor text into SQL.

`best_index` first collects selected constraints while borrowing `IndexInfo`
immutably, then records contiguous 1-based argument positions. Each selected
term carries its original constraint index. `filter` decodes against the copied
key count and copies owned values before choosing SQL branches.

`ReadPlan::predicates(cols, args) -> Vec<String>` returns one or two conjunctive
predicates. The empty plan returns `vec![String::new()]`. If a numeric RHS
constraint on output column 0 has EQ/LT/LE, choose exactly one anchor, preferring
EQ over LT/LE. Split that anchor's direct numeric branch and TEXT-region CAST
branch into disjoint SELECTs joined by UNION ALL. Add every other selected
predicate to **both** SELECTs unchanged, retaining the same parameter numbers.
No non-leading anchor is split. Split at most one term, so SQL grows to at most
two SELECTs, never exponentially. Other numeric predicates retain their
candidate expressions; GT/GE remains direct and TEXT keeps four arms.

For a two-key view and numeric arguments, a leading equality and an upper bound
render the complete compound query below. The second-key bound is deliberately
repeated in both branches:

```sql
SELECT rowid, "g", "h", "n" FROM "main"."__ivm_out_roll"
WHERE ("g" COLLATE BINARY = ?1)
  AND ("h" COLLATE BINARY < ?2
       OR ("h" COLLATE BINARY >= '' AND "h" COLLATE BINARY < x''
           AND +CAST("h" AS NUMERIC) COLLATE BINARY < ?2))
UNION ALL
SELECT rowid, "g", "h", "n" FROM "main"."__ivm_out_roll"
WHERE ("g" COLLATE BINARY >= '' AND "g" COLLATE BINARY < x''
       AND +CAST("g" AS NUMERIC) COLLATE BINARY = ?1)
  AND ("h" COLLATE BINARY < ?2
       OR ("h" COLLATE BINARY >= '' AND "h" COLLATE BINARY < x''
           AND +CAST("h" AS NUMERIC) COLLATE BINARY < ?2));
```

For numeric EQ/LT/LE, the direct branch cannot match a TEXT key, while the
second branch matches TEXT only. Their disjointness permits UNION ALL to
retain every candidate row, rowid and duplicate copy exactly once. Do not use
UNION deduplication, force an index, or split GT/GE (their raw branch includes
TEXT). A wide output index may cause a combined OR to scan even when few
candidates are materialized; the disjoint compound shape permits SQLite to
plan bounded access independently for the two branches.

Prepare the compound query as **one** uncached statement and bind the original
owned values once. On `Error::Utf8Error(_, _)`, replace the plan with
`ReadPlan::default()` and clear values before rendering: one full SELECT with
no arguments is the compatibility fallback. All other errors propagate.

All emitted key comparisons use explicit BINARY collation. Use existing
`names::quote` and `main_qualified`; emit operator SQL only from the validated
enum. Actual output names come from `CompiledView`, not the illustration above.
Retain uncached statements and collect
`Vec<(i64, Vec<SqlValue>)>` before returning from `filter`. The cursor owns its
connection, names, key count and rows; it must not borrow `IvmTab` or retain a
live SQLite statement across callbacks. Reset position for every filter call.

A broken view keeps its existing diagnostic, can connect and be dropped, and
does not acquire a new best-index error that masks its existing failure.
Derive the key count only for a successfully compiled view. The read path must
not refresh implicitly or modify durable state.

## 4. Planner estimates

The existing output index spans every output column in SELECT order, but
`best_index` cannot know the bound storage class and therefore cannot promise
the numeric equality path. Keep estimated cost 1,000,000 for **every** plan.
Do not set `estimatedRows`: preserve SQLite's existing default of 25, including
for leading equality and fallback plans. Setting it to 1,000,000 changed an
observed one-group/1,000-RHS-row join from one nested SELECT to 1,000 by reversing
the join order. Preserve the existing planning behavior rather than replacing
it with an unmeasured cardinality guess. Do not query data
or prepare SQL in `best_index`. Equality on every key still does not imply
uniqueness. Actual nested statement evidence, rather than an optimistic
estimate or virtual plan label, establishes any observed access improvement.

## 5. Correctness and compatibility acceptance

Compare filtered virtual output with independently materialized ordinary
SQLite output having typeless BINARY columns. Compare sorted multisets,
storage classes and REAL bits; use dyadic values for aggregate-source oracles
so exact aggregates do not accidentally introduce an unrelated rounding issue.

Cover every operator and parameter class; NULL, INTEGER/REAL numeric ties,
negative zero, infinite REAL, numeric-looking TEXT, embedded-NUL TEXT, empty and
nonempty BLOB; both BINARY and residual NOCASE/RTRIM/custom collations; reversed
operands; duplicate/conflicting bounds; aggregate/rowid/hidden unsupported
terms; repeated prepared execution; IN/OR; correlated joins with unusable
constraints. Join typeless mixed-class keys to ordinary INTEGER, REAL, NUMERIC,
TEXT and BLOB columns, with both operand orders and every supported inequality;
explicitly include TEXT `42` against INTEGER 42 and INTEGER 42 against TEXT `42`.
Also join against compound subqueries whose declared affinity disagrees with
runtime storage class: a NUMERIC-affinity result carrying TEXT `42`, and a
TEXT-affinity result carrying REAL 42.0. Include a minimal foreign virtual-table
fixture with those declarations/values when compound affinity choice alone
would not deterministically exercise both cases. Cross both operand orders and
all five operators, with values around 2^51, 2^53 and i64 boundaries. Compare
with a materialized ordinary-table oracle, not merely the virtual table with a
different spelling. A collation fixture must contain values such as `a`, `A` and
`a ` so an accidental BINARY pushdown loses observable matches.

Repeat selective oracle reads after insert, delete, group rekey, refresh,
transaction rollback, savepoint rollback and file reopen. Keep a cursor alive
while a later refresh changes output and prove its original snapshot remains
stable. Exercise quoted identifiers and broken-view diagnostics. Create a
format-6 fixture with the unmodified alpha.6 library and open, read, refresh,
roll back and reopen it with the new library. Existing lifecycle and full-read
tests must stay green. Add invalid-UTF-8 RHS regressions for equality and both
range directions using `CAST(x'80' AS TEXT)` and compare against ordinary
SQLite; they must return matching rows rather than an argument-copy error.
Add a one-group view joined to 1,000 RHS rows and count actual nested reads:
preserving the default row estimate must avoid the observed 1-to-1,000 read
regression. Verify `+g` and CAST-key predicates remain correct scan fallbacks.

Use actual nested statement tracing and statement work counters to prove
pushdown. Record the executed internal SELECT and bindings, `FULLSCAN_STEP`
and `VM_STEP`, then run EXPLAIN QUERY PLAN for that captured query with the same
bindings as supplementary evidence. A public virtual-table plan label alone
is insufficient. Report whether leading equality/range produces bounded
internal work on a 20,000-key fixture; if affinity broadening prevents index
use, record that limit and measure materialized candidate reduction separately.
Non-leading key evidence must honestly show whether it scans. Include actual
seven-output-column, 20,000-group fixtures: the former OR shape materialized
one candidate but still scanned 19,999 rows. Guard the UNION access path using
internal VM/fullscan work, not merely candidate counts. Instrumentation runs
are separate from latency runs.

The source/probe artifacts used for the affinity boundary live under
`/Users/weijianfeng/.codex/artifacts/ivmlite-selective-reads-2026-10-21/affinity/`.
`quality-review-probe.py` and `quality-review-probe.txt` in that artifact parent
retain the invalid-UTF-8 and cardinality-estimate regressions before their
fixes. Preserve their runtime/source identities. Report probe case counts only
after the final probe completes and its output is read; do not infer totals.

Add a disjointness gate that would fail if a row can be emitted by both UNION
branches or if duplicate copies/rowids are collapsed. Add an OR-collapse work
mutation which keeps SQL results equal but destroys the bounded leading access
path on a seven-column output fixture. Those gate rows remain drafts until
compile, full mutation run, exact red set and restoration are observed.

Every new invariant is registered in `docs/mutation-gates.md` with its concrete
mutation, red test and measured full-suite result. Compile each mutation before
claiming a verified red result; rebuild after restoring it. Preserve raw logs
and count both cargo invocations. Do not widen differential enumeration here.

## 6. Native host and bounded measurement

Add one Rust/rusqlite example using existing host dependencies. Accept an
explicit library path; load with `open_with_extension_at`. Demonstrate a
file-backed create, transaction write, explicit refresh, parameterized read,
commit, a second write/refresh/read rolled back, and close/reopen. Assert exact
rows at each stage and remove its temporary database. This is one tested
native driver example, without an SDK or claims for Swift integration.

Add a separate Kener harness reusing `demand_cases::KENER` and
`demo_bench::Workload`, configurable by base-row count and fresh-repeat count.
A 100,000-row default permits a quick validation run. Acceptance uses 4,100,000
base rows yielding 550,000 initial groups, 500 mutations per round, five fresh
repeats per library, one warmup and ten measured rounds per repeat, matching
the existing native application protocol. Run the identical host binary
against preserved alpha.6 and new release libraries in separate processes.
Each repeat starts with a fresh seeded database; do not reuse the post-mutation
fixture of a previous repeat.

Seed, create and prepare outside timers. Each measured round records write,
refresh, full read, one-monitor read, monitor/time read and their end-to-end
combinations; drain all rows and retain returned-group counts. Compare with
the ordinary SQL oracle outside timing. Capture full/non-leading/selective
internal work in a separate instrumented run. Record machine, SQLite,
compiler, source and library identity and exact commands. Run a smaller fixture
first, then the full acceptance fixture after it validates; a quick result does
not substitute for the source-scale acceptance result.

Report medians and the raw rounds, full-read and maintenance regression
comparisons, and observed limits. A synthetic bounded fixture cannot establish
production page performance or lower persistent state size. Do not relabel
instrumented durations as latency or test-suite duration as application speed.
