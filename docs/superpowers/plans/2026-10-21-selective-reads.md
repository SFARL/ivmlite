# Selective Reads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce the output rows materialized for outer group-key equality and range reads while preserving SQLite comparison, transaction and cursor semantics.

**Architecture:** Add a private, strict xBestIndex/xFilter read-plan protocol and bind owned SQLite values in the cursor's existing uncached nested SELECT. Leave residual filtering active. Use disjoint UNION ALL SELECTs for one leading numeric EQ/LT/LE anchor, repeat all other predicates in each branch, keep direct GT/GE and four numeric TEXT arms; report actual index work separately from candidate materialization and latency.

**Tech Stack:** Rust 1.95, rusqlite 0.40.2, SQLite 3.45 minimum (test host bundled SQLite 3.53.2), existing host `load_extension`/`hooks`, standalone extension `functions`/`loadable_extension`/`vtab`, no new dependencies.

**Binding spec:** `docs/superpowers/specs/2026-10-21-selective-reads-design.md`. Read `docs/spikes/2026-10-21-selective-read-probe.md` and the repository rules in `docs/superpowers/plans/2026-10-17-m2c-types-and-aggregates.md`. This plan does not authorize release/version changes, tag, push or publication.

## File map and execution rules

| Path | Responsibility |
| --- | --- |
| `crates/ivmlite-sqlite/src/read_plan.rs` (new) | Operators, eligibility, ephemeral descriptor, storage-class predicate construction and unit tests |
| `crates/ivmlite-sqlite/src/lib.rs` | Register the private module |
| `crates/ivmlite-sqlite/src/vtab.rs` | Collect usable constraints, retain owned key metadata, execute filtered snapshots |
| `crates/ivmlite-test/tests/extension_selective_reads.rs` (new) | Ordinary SQLite oracle, storage classes, collations, joins, IN/OR, lifecycle and actual internal work |
| `crates/ivmlite-test/examples/selective_read_driver.rs` (new) | File-backed native Rust lifecycle example with explicit library path |
| `crates/ivmlite-test/examples/selective_read_bench.rs` (new) | Identical bounded Kener host for old/new libraries; instrumentation separate from timings |
| `docs/demos/selective-reads.md` (new), `README.md` | Measured usage, boundaries, reproduction and link |
| `docs/demos/results/2026-10-21-selective-reads/` (new) | Raw rounds, work counters, identities, compatibility and acceptance logs |
| `docs/mutation-gates.md` | New invariant rows with observed mutation results |

Preserve unrelated dirty work. All identifiers use `names::quote`; shadow
queries use `main_qualified`. Keep every existing error text. Every callback
body stays inside `guard`. Do not change persisted `Plan::canonical`, format
6, versions, or Cargo dependencies/features. No ordering, limit or uniqueness
claims. Do not invoke gated `modern_sqlite` IN methods.

Run native builds/tests serially: `scripts/test-all.sh` rebuilds the standalone
extension before running both extension and workspace test suites. A bare
workspace test is permitted only after the matching library has been rebuilt.
The recorded unmodified baseline is 848 passed, 0 failed and 3 ignored. Run the
five required checks before committing the completed implementation:

```bash
cargo fmt --all -- --check
cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings
scripts/test-all.sh
```

Commit messages retain the repository trailer:
`Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Stage specific paths,
never skip hooks, and do not push. The controller may consolidate all four
tasks into one final commit after independent reviews and final checks. Task
checkpoints do not require duplicate full-suite runs solely for doc commits.

## Task 1: Prove safe comparisons, then connect the private read plan

**Files:** create `crates/ivmlite-sqlite/src/read_plan.rs`; modify
`crates/ivmlite-sqlite/src/{lib,vtab}.rs`; create the initial
`crates/ivmlite-test/tests/extension_selective_reads.rs`.

- [x] **Step 1: Add an affinity regression before implementing pushdown.**

Start the integration file with an ordinary table holding the same typeless
values as the virtual key. This fixture catches the false-negative failure of
naive `WHERE key = ?` pushdown:

```rust
mod common;
use ivmlite_test::open_with_extension;
use common::{create, rows};

#[test]
fn join_rhs_affinity_keeps_numeric_text_matches() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE facts(g); INSERT INTO facts VALUES ('42'),(42),('x');
        CREATE TABLE ints(k INTEGER); INSERT INTO ints VALUES (42);
        CREATE TABLE texts(k TEXT); INSERT INTO texts VALUES ('42');
        CREATE TABLE expected(g,n);
        INSERT INTO expected SELECT g,COUNT(*) FROM facts GROUP BY g;").unwrap();
    create(&c, "roll", "SELECT g,COUNT(*) AS n FROM facts GROUP BY g").unwrap();
    for table in ["ints", "texts"] {
        for op in ["=", ">", ">=", "<", "<="] {
            for (left,right) in [("v.g","r.k"),("r.k","v.g")] {
                let tail = format!("JOIN {table} r ON {left} {op} {right}");
                assert_eq!(rows(&c,&format!("SELECT v.g,v.n FROM roll v {tail}")),
                           rows(&c,&format!("SELECT v.g,v.n FROM expected v {tail}")));
            }
        }
    }
}
```

Run `scripts/build-extension.sh`, then
`cargo test -p ivmlite-test --test extension_selective_reads --locked`.
The baseline passes; temporarily apply direct nested `g op ?` filtering to
confirm this regression turns red, then restore. A failing semantic oracle is
not an acceptable performance tradeoff.

- [x] **Step 2: Define and unit-test a strict descriptor without changing the persisted plan.**

The private module's interfaces are:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op { Eq, Gt, Ge, Lt, Le }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Term { pub column: usize, pub op: Op }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadPlan { pub terms: Vec<Term> }
impl ReadPlan {
    pub fn encode(&self) -> String;
    pub fn decode(idx: i32, text: Option<&str>, keys: usize, argc: usize)
        -> Result<Self, String>;
    pub fn predicates(&self, quoted_columns: &[String], args: &[SqlValue]) -> Vec<String>;
}
```

`Op` maps only the five `IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_*`
variants to fixed SQL tokens and descriptor tokens. Descriptor rules are
`idx=0, text=None, argc=0` for empty plans and `idx=1,
text=Some("sr1:0=eq,1=ge,1=lt")` for selected constraints. Reject invalid
integers including `-1`, `+1`, `01`, overflow, unknown operators, empty terms,
out-of-key positions and mismatched argument counts with exactly
`ivmlite internal error: invalid selective-read plan`.

Use these roundtrip and malformed cases in module tests:

```rust
let p = ReadPlan { terms: vec![Term { column: 0, op: Op::Eq },
                              Term { column: 1, op: Op::Ge }] };
assert_eq!(ReadPlan::decode(1, Some(&p.encode()), 2, 2).unwrap(), p);
for text in ["sr2:0=eq", "sr1:", "sr1:0=ne", "sr1:01=eq",
             "sr1:-1=eq", "sr1:2=eq", "sr1:0=eq,"] {
    assert!(ReadPlan::decode(1, Some(text), 2, 1).is_err());
}
assert!(ReadPlan::decode(1, Some("sr1:0=eq"), 2, 0).is_err());
assert!(ReadPlan::decode(0, Some("sr1:0=eq"), 2, 0).is_err());
assert!(ReadPlan::decode(9, None, 2, 0).is_err());
```

- [x] **Step 3: Implement affinity-safe candidate selection and bind arguments.**

The output key is a bare BLOB-affinity column, not an expression with no
affinity. SQLite comparison affinity is NUMERIC if either operand has numeric
affinity and otherwise BLOB/no conversion. TEXT RHS affinity cannot propagate
TEXT conversion to this key. SQLite 3.45 `exprMightBeIndexed` identifies a scalar
column through `TK_COLUMN`; unary-plus/CAST keys are expressions and do not
become selected vtab constraints. Add scan-fallback oracles for those forms.
Do not add TEXT CAST candidate branches.

INTEGER and REAL RHS use the same indexed TEXT-region candidate for all five
operators, with `=` shown below. Substitute only the fixed comparison token;
keep the region's constant bounds unchanged:

```sql
("g" COLLATE BINARY = ?1
 OR ("g" COLLATE BINARY >= '' AND "g" COLLATE BINARY < x''
     AND +CAST("g" AS NUMERIC) COLLATE BINARY = ?1))
```

The raw branch preserves unconverted values; only TEXT keys can acquire numeric
conversion. Unary `+` strips CAST affinity. Numeric RHS GT/GE may simplify to
`"g" COLLATE BINARY >= ?1` (or `>`): every TEXT/BLOB key already sorts above a
numeric bound, so conversion can only remove matches. This is a safe superset
because SQLite retains residual filtering. Both INTEGER and REAL equality may
use the indexed numeric match plus TEXT region; no compound range-seek claim.

For TEXT RHS use four arms for every operator, shown with `>=`:

```sql
("g" COLLATE BINARY >= ?1
 OR +CAST("g" AS NUMERIC) COLLATE BINARY >= ?1
 OR "g" COLLATE BINARY >= +CAST(?1 AS NUMERIC)
 OR +CAST("g" AS NUMERIC) COLLATE BINARY >= +CAST(?1 AS NUMERIC))
```

These are all Cartesian pairs of raw/NUMERIC values, covering numeric-looking
TEXT returned with NUMERIC affinity by compound subqueries or foreign virtual
tables. BLOB RHS uses the direct comparison for all operators: affinity does
not convert BLOB. NULL RHS uses `(?1 IS NOT NULL AND 0)` for every operator;
retain its placeholder to preserve bind count. Reuse one argument index within
every arm of a term, conjoin terms, and quote all key identifiers. No raw
adapter, feature, expression index or persisted format change is needed.

Pin NUMERIC CAST-versus-affinity equivalence on numeric strings around 2^51,
2^53 and i64 bounds, exponents, whitespace and infinities. Raw arms retain
values numeric affinity leaves unchanged. `rhs_value` and
`sqlite3_value_frombind` do not prove comparison affinity. Use the source and
probe evidence under
`/Users/weijianfeng/.codex/artifacts/ivmlite-selective-reads-2026-10-21/affinity/`;
record probe totals only from the completed final output.

`ReadPlan::predicates` chooses exactly one numeric EQ/LT/LE constraint on
output column 0 as an anchor, preferring equality. Return the anchor's direct
comparison in one predicate and its TEXT-region CAST branch in the other,
conjoining every remaining selected term into each. Use unchanged argument
numbers in both. No non-leading constraint is split. The empty plan returns
`vec![String::new()]`; non-split plans return a single conjunction. At most two
SELECTs prevent exponential query growth. Bind and execute the rendered UNION
ALL as one statement, without forcing an index.

The two anchor branches are disjoint for numeric EQ/LT/LE: raw comparison
cannot match TEXT, and the second branch is TEXT-only. Preserve rowids and
multiplicity with UNION ALL, never UNION. Do not split GT/GE because their raw
branch already contains TEXT. Unit-test equality preference when LT/LE appears
first, at-most-two growth for repeated constraints, non-leading one-branch
behavior, empty-plan one full scan, and complete repetition of non-anchor
predicates. On a seven-output-column fixture a combined OR can scan even when
only one candidate is returned, so internal work must guard this SQL shape.

In `best_index`, collect `(constraint_index, Term)` from an immutable iteration
first. Require `is_usable`, `0 <= column < key_count`, allowed operator and
`info.collation(index)?.eq_ignore_ascii_case("BINARY")`. Then mutate usage:

```rust
for (arg, (constraint, _)) in selected.iter().enumerate() {
    let mut usage = info.constraint_usage(*constraint);
    usage.set_argv_index(i32::try_from(arg + 1).map_err(|e| e.to_string())?);
    usage.set_omit(false);
}
```

Obtain key count by matching `CompiledView.plan`'s root
`ivmlite_core::Plan::Aggregate { group_by, .. }`. Broken views choose the
existing full-scan estimate and reach their original diagnostic on read.
Copy key count into `IvmCursor` beside `cols`, without borrowing the table.
Decode before reading `Filters`; gather values only after length validation:

```rust
let mut plan = ReadPlan::decode(idx, idx_str, self.keys, args.len())?;
let values = match (0..args.len())
    .map(|i| args.get::<SqlValue>(i))
    .collect::<rusqlite::Result<Vec<_>>>()
{
    Ok(values) => values,
    Err(rusqlite::Error::Utf8Error(_, _)) => {
        plan = ReadPlan::default();
        Vec::new()
    }
    Err(error) => return Err(error.to_string()),
};
let n = cols.len();
let select = format!("SELECT rowid, {} FROM {}",
    cols.join(", "), main_qualified(&self.out_table));
let sql = plan.predicates(cols, &values).into_iter()
    .map(|predicate| if predicate.is_empty() {
        select.clone()
    } else {
        format!("{select} WHERE {predicate}")
    })
    .collect::<Vec<_>>()
    .join(" UNION ALL ");
let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |r| {
    let values = (1..=n).map(|i| r.get::<_, SqlValue>(i)).collect::<rusqlite::Result<_>>()?;
    Ok((r.get::<_, i64>(0)?, values))
}).map_err(|e| e.to_string())?;
self.rows = rows.collect::<rusqlite::Result<_>>().map_err(|e| e.to_string())?;
self.at = 0;
```

Keep `info.set_estimated_cost(1_000_000.0)` for every plan. Do not call
`set_estimated_rows`: preserve SQLite's existing default of 25, including every
fallback. xBestIndex cannot know the bound storage class. A reviewer observed
that replacing the default with 1,000,000 reversed a one-group/1,000-RHS-row
join and increased nested SELECT executions from one to 1,000. Do not claim a
compound monitor/time index seek.
Never set unique flags, order consumed or limit arguments.

- [x] **Step 4: Verify the implementation checkpoint.**

Run `scripts/test-all.sh`, both fmt and both clippy commands above. Expect no
failed tests. Retain this checkpoint for the consolidated final commit unless
the controller chooses a separate implementation commit. Later evidence may
state a materialization improvement even when affinity safety prevents an
index-search improvement.

## Task 2: Verify semantics, lifecycle, actual work and mutation gates

**Files:** extend `crates/ivmlite-test/tests/extension_selective_reads.rs`;
modify `docs/mutation-gates.md`; create raw acceptance evidence under
`docs/demos/results/2026-10-21-selective-reads/`.

- [x] **Step 1: Expand ordinary SQLite oracle coverage.**

Create `facts(id INTEGER PRIMARY KEY,g,h)` and a view grouping by `g,h` with
COUNT. Materialize `expected(g,h,n)` with typeless columns. Insert NULL,
INTEGER/REAL numeric ties, -0.0, infinity, TEXT `42`, `042`, `3.0e+5`, `a`, `A`,
`a `, embedded NUL and empty/nonempty BLOB. Bind each class through
`Statement::query_map(params_from_iter(...))`. Compare sorted vectors including
storage class, and REAL `to_bits()` where expected REAL values are returned.
Use all five operators, reversed operands, duplicate constraints, conflicting
bounds, rebound parameters, aggregate-only predicates and key-plus-aggregate
predicates. Rebuilding `expected` after writes must select from the base facts,
not ivmlite shadow tables.

Exercise these SQL forms with an identical replacement of `roll` by `expected`:

```sql
SELECT * FROM roll WHERE g = ?1 AND h >= ?2 AND h < ?3;
SELECT * FROM roll WHERE g = ?1 AND g = ?2;
SELECT * FROM roll WHERE g > ?1 AND g <= ?1;
SELECT * FROM roll WHERE g COLLATE NOCASE = ?1;
SELECT * FROM roll WHERE g COLLATE RTRIM >= ?1;
SELECT * FROM roll WHERE g IN (?1,?2,?3,NULL);
SELECT * FROM roll WHERE g IN (SELECT k FROM rhs);
SELECT * FROM roll WHERE g IN ();
SELECT * FROM roll WHERE g = ?1 OR h >= ?2;
SELECT * FROM roll WHERE n > ?1 ORDER BY g LIMIT 2;
SELECT * FROM roll WHERE rowid > ?1;
```

The rowid test compares against a snapshot containing captured virtual rowids,
since fresh GROUP BY materialization need not assign identical rowids. Register
a host custom collation through existing SQLite FFI if needed; do not add a
rusqlite feature. Use a deliberately different comparison order and assert its
results against ordinary SQL. Join to INTEGER, REAL, NUMERIC, TEXT and BLOB RHS
tables in both operand orders and every operator. Also exercise compound
subquery RHS declarations that yield TEXT `42` under NUMERIC affinity and REAL
42.0 under TEXT affinity. A minimal FFI-registered foreign virtual table can
make those declaration/runtime-class mismatches deterministic without adding
dependencies or host features. Include both operand orders and every operator,
plus numeric strings around 2^51, 2^53 and i64 bounds. For example, compare the
virtual and ordinary-table LHS with exactly the same compound RHS:

```sql
SELECT v.g,v.n FROM roll v JOIN
  (SELECT k FROM numeric_rhs WHERE 0 UNION ALL SELECT g FROM raw_rhs) r
  ON v.g >= r.k;
SELECT v.g,v.n FROM expected v JOIN
  (SELECT k FROM numeric_rhs WHERE 0 UNION ALL SELECT g FROM raw_rhs) r
  ON v.g >= r.k;
```

Populate `numeric_rhs(k NUMERIC)` and `raw_rhs(g)` explicitly and inspect RHS
runtime storage classes. Repeat with the compound's first column declared
TEXT and REAL values in the raw branch; pin foreign-vtab output if compound
affinity choice does not cover the desired case deterministically.
Add predicates on `+v.g` and `CAST(v.g AS TEXT)` and assert correct scan
fallback rather than selected bare-column pushdown. Compare `g =`, `g <` and
`g > CAST(x'80' AS TEXT)` with ordinary SQLite: copying the RHS may raise
`Error::Utf8Error(_, _)`, which must trigger a full scan with no bound arguments.
Propagate all other conversion errors; do not reject malformed TEXT generally
or reinterpret it as BLOB. The fixture's returned values stay valid UTF-8.

Include correlated joins
which make some constraints unusable; do not depend on SQLite choosing only
one join order.

- [x] **Step 2: Exercise unchanged refresh/snapshot/format behavior.**

Run selective assertions before refresh (old output), after explicit refresh,
after delete and group rekey, after `BEGIN`/refresh/`ROLLBACK`, after
`SAVEPOINT s`/refresh/`ROLLBACK TO s`/`RELEASE s`, and after file reopen. Keep a
stepped cursor open during a later write/refresh and collect its remaining
owned rows; compare with the snapshot from before the refresh. Verify a new
cursor sees the new output. Use view/column names containing quotes.

Preserve alpha.6 release library identity before rebuilding; create a file
with it through `open_with_extension_at`, close it, then reopen with the new
library. Assert format remains 6 and exercise selective reads, refresh and
rollback. Reuse the broken-metadata patterns in
`crates/ivmlite-test/tests/extension_lifecycle.rs`; assert the existing exact
read diagnostic and successful DROP, including a selective WHERE read.

- [x] **Step 3: Trace the actual nested SELECT and capture work outside timing.**

Install `rusqlite::ffi::sqlite3_trace_v2` for `SQLITE_TRACE_STMT |
SQLITE_TRACE_PROFILE` on the host connection. At STMT copy `sqlite3_sql(stmt)`;
at PROFILE read `sqlite3_stmt_status(stmt, SQLITE_STMTSTATUS_FULLSCAN_STEP, 0)`
and `SQLITE_STMTSTATUS_VM_STEP`. Filter to the actual nested
`SELECT rowid, ... FROM "main"."__ivm_out_..."`; include bindings via
`sqlite3_expanded_sql` and free its result with `sqlite3_free`. Contain callback
panics with `catch_unwind` and report them after returning to Rust. A scope
guard unregisters tracing before the state goes out of scope, even on errors.
Do not retain statement pointers after the callback.

On 20,000 groups, record full read, leading equality, leading equality plus
range, and non-leading-only range. Run EXPLAIN on the captured SQL with the
same bound values. Assert selected SQL references only eligible keys and does
not interpolate argument values or add ORDER BY or LIMIT. Record whether safe
broadening scans and how many candidates are materialized; only require
bounded VM work if the measured implementation actually offers an indexed
path. Include seven output columns in the 20,000-group leading-access fixture:
collapsing the two SELECTs to OR previously produced 19,999 fullscan steps and
100,016 VM steps for one candidate. Record candidate and outer result counts
separately, since TEXT candidate supersets may be rejected by residual checks.
Separately use the existing `hooks` progress handler to count whole-query
VM callbacks. Add a one-group view joined with 1,000 ordinary RHS rows,
assert its SQL oracle result, and count nested output SELECTs to catch the
observed one-to-1,000 regression from overwriting estimatedRows. Preserve the
existing default row estimate and compare with the baseline plan/read count.
The before-fix repro is
`/Users/weijianfeng/.codex/artifacts/ivmlite-selective-reads-2026-10-21/quality-review-probe.py`
and its `.txt` output. Turn tracing/progress off for every latency run.

- [x] **Step 4: Mutation-test the new invariants and restore.**

Register separate mutations for dropping the usable check, accepting NOCASE,
removing the affinity-safe branch/fallback, shifting an argument index,
changing GE to GT, permitting aggregate output columns, accepting a malformed
descriptor, replacing the UTF-8 fallback with an error, removing anchor
branch disjointness, collapsing the split anchor into OR, setting estimatedRows
to 1,000,000, and removing WHERE generation. Also mutate the benchmark actual
write call and timed read selection, independently of its reference replay.
Existing owned snapshot semantics are unchanged and remain covered by lifecycle
regressions; no new live-cursor implementation is introduced for mutation.
Deduplicating UNION is not registered as a separate mutation: physical rowid
is projected and the branches are disjoint, so it is equivalent on this path. The disjointness gate checks exact rowid/copy multiplicity;
the OR-collapse gate checks VM/fullscan work on seven output columns and must
turn red even if result rows still agree. These are draft requirements until
actually mutated, compiled, run and restored. Each must name its red test. Where one code mutation proves
several invariant rows, link the same measured evidence without inventing
independent runs. Execute each compilable mutation serially through
`scripts/test-all.sh`; preserve full logs and actual per-binary counts, restore,
rebuild and rerun clean. Do not record a compile failure as a semantic red.

```bash
python3 scripts/count-mutation-gates.py --fix
python3 scripts/count-mutation-gates.py
```

Expect `consistent`. Run all five checks at the final checkpoint; retain the
test, ledger and raw evidence for the consolidated final commit after review.

## Task 3: Deliver the native Rust lifecycle example

**Files:** create `crates/ivmlite-test/examples/selective_read_driver.rs` and
initial `docs/demos/selective-reads.md`.

- [x] **Step 1: Implement the complete host path with an explicit library argument.**

Use `ivmlite_test::open_with_extension_at`, `rusqlite::params` and standard
library temporary-path cleanup, following `examples/release_smoke.rs`. The
executable accepts one absolute library path and rejects additional arguments.
Use a unique process/time-based temporary file and an RAII cleanup guard.
This is the transaction/read core; wrap it in `fn main() ->
Result<(), Box<dyn std::error::Error>>` and reopen using the same path/library:

```rust
let c = open_with_extension_at(Some(&database), &library)?;
c.execute_batch("CREATE TABLE facts(g,h,v); INSERT INTO facts VALUES(7,10,5);
    CREATE VIRTUAL TABLE roll USING ivm(
      'SELECT g,h,SUM(v) AS total FROM facts GROUP BY g,h');")?;
c.execute_batch("BEGIN; INSERT INTO facts VALUES(7,10,3);
    INSERT INTO roll(roll) VALUES('refresh');")?;
let select = "SELECT total FROM roll WHERE g=?1 AND h>=?2 AND h<?3";
let total: i64 = c.query_row(select, params![7,10,11], |r| r.get(0))?;
assert_eq!(total,8);
c.execute_batch("COMMIT; BEGIN; UPDATE facts SET v=100;
    INSERT INTO roll(roll) VALUES('refresh'); ROLLBACK;")?;
assert_eq!(c.query_row(select, params![7,10,11], |r| r.get::<_,i64>(0))?,8);
drop(c);
let reopened = open_with_extension_at(Some(&database), &library)?;
assert_eq!(reopened.query_row(select,params![7,10,11],|r|r.get::<_,i64>(0))?,8);
```

- [x] **Step 2: Run the example against both preserved alpha.6 and new libraries.**

```bash
cargo run --locked -p ivmlite-test --example selective_read_driver -- /absolute/path/to/preserved-alpha6-library
cargo run --locked -p ivmlite-test --example selective_read_driver -- /absolute/path/to/new-library
```

Both must report the same asserted lifecycle rows. Record SQLite version and
the exact libraries. Document the extension entry point, the explicit refresh
requirement and rollback/reopen behavior. Explain that this is a native Rust
example, with no claims for other language drivers.

- [x] **Step 3: Verify the example and document the checkpoint.**

Retain the example and demo document for the consolidated final commit.
Register both examples with `test=true` so the full workspace test command
executes their lifecycle and native harness regression tests.

## Task 4: Run identical quick and source-scale Kener comparisons

**Files:** create `crates/ivmlite-test/examples/selective_read_bench.rs`;
update `docs/demos/selective-reads.md` and `README.md`; create results artifacts.

- [x] **Step 1: Add a configurable fixture harness with separate latency/work modes.**

CLI: `selective_read_bench LIBRARY latency|work [BASE_ROWS] [REPEATS]`. Resolve
`LIBRARY` explicitly; reject invalid mode, zero/overflowing counts and extra
args. Defaults are 100,000 rows and one fresh repeat for quick validation.
Import `ivmlite_test::demand_cases::KENER` and
`ivmlite_test::demo_bench::{Workload, PreparedBatch, create_view, drain}`.
Use constants `BATCH=500`, `WARMUPS=1`, `ROUNDS=10`; base rows and repeats come
from validated arguments. Acceptance uses `BASE_ROWS=4_100_000`, `REPEATS=5`,
which yields 550,000 initial groups. Call
`KENER.validate(base_rows,BATCH,WARMUPS+ROUNDS)` before setup. Seed through
`KENER.seed`, create its virtual table with `create_view`, prepare the batch,
refresh statement and these reads outside timers:

```rust
const FULL: &str = "SELECT * FROM kener_bucket_rollup";
const MONITOR: &str = "SELECT * FROM kener_bucket_rollup WHERE monitor_id=?1";
const WINDOW: &str = "SELECT * FROM kener_bucket_rollup WHERE monitor_id=?1 \
    AND bucket_ts>=?2 AND bucket_ts<?3";
const NONLEADING: &str = "SELECT * FROM kener_bucket_rollup WHERE bucket_ts>=?1 \
    AND bucket_ts<?2";
```

Bind monitor 123 and `[0,9000)` bucket window. Each round begins a transaction,
times only batch execution, explicit refresh, each fully drained read, and
commit as separate fields. Keep write+refresh+selected read+commit totals as
well as individual durations. Rotate read order per round. Compare selected
rows with `SELECT * FROM (<KENER.view_sql>) WHERE ...` outside every timed
region, using existing sorted multiset helpers; retain returned group counts.
Warmup applies the same mutation schedule but is excluded from summaries.
Every repeat creates a fresh seeded fixture and resets the round schedule;
retain repeat and round indices in raw output. Do not time fixture setup.
Use an in-memory fixture with a fixed documented journal configuration; write
the same config and SQLite/library identifiers into each result.

In work mode execute the same fixture and reads with the Task 2 nested trace,
statement counters and progress instrumentation; print those counts and actual
SQL. Do not print latency summaries from work mode. JSON/CSV output must retain
all raw rounds and separate schema fields for timings and counters.

- [x] **Step 2: Preserve identity and run sequential before/after processes.**

Build the host once and run that identical executable against both release
libraries. Alpha.6 source is tag `v0.1.0-alpha.6` at
`0ad47a9259ee7acee411b46b7f64d50d268b2096`; verify live before using it.
Record `git rev-parse HEAD`, `rustc --version`, `sw_vers` or `uname -a`,
`shasum -a 256 LIBRARY`, fixture constants and SQLite version. Preserve the old
release library before building new code. Use absolute library paths:

```bash
cargo build --release --locked -p ivmlite-test --example selective_read_bench
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library latency 100000 1
target/release/examples/selective_read_bench /absolute/path/to/new-library latency 100000 1
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library latency 4100000 5
target/release/examples/selective_read_bench /absolute/path/to/new-library latency 4100000 5
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library work 4100000 1
target/release/examples/selective_read_bench /absolute/path/to/new-library work 4100000 1
```

Capture stdout and stderr per command into distinctly named results files.
Do not run builds, tests or competing benchmarks concurrently. Do not run a
source-scale fixture until the quick run validates successfully. The quick run
is diagnostic; it does not replace 4.1-million-row acceptance.

- [x] **Step 3: Publish local evidence with honest limits and finish checks.**

The demo document includes a table of old/new medians and ratios for full,
monitor, monitor/window reads, write, refresh and end-to-end costs; raw rounds
and returned groups link next to it. A separate table records internal SQL,
VM/fullscan/progress counts and observed access path. Report scan fallback or
affinity-broadening cost directly. State that synthetic data and bounded scale
do not prove Kener production latency or smaller persistent state. Explain
that non-leading constraints can still scan and IN uses SQLite per-value EQ.
If full read or maintenance regresses materially, record it and investigate
before claiming the feature complete; do not hide an unfavorable cell.

Add a README link next to existing demo links, with no version/release changes.
Run both fmt, both clippy, `scripts/test-all.sh`, then
`scripts/test-all.sh --release` serially and retain complete logs and counts.
Run the driver example against the final release library. After independent
reviews pass, stage specific authorized implementation, tests, examples, ledger,
documents and results paths. Make one consolidated descriptive commit with the
required coauthor trailer; never skip hooks, release, tag or push. The final report names every material limitation, test
result, library/source identity and artifact path; it does not publish a
release or promise an index speedup unsupported by the captured nested work.

## Execution checkpoint

2026-10-21: Task 1's candidate semantics passed separate specification and code
quality reviews after fixing invalid-UTF-8 RHS and row-estimate regressions.
The pre-change full suite passed 848 tests, zero failed and three ignored.
The later seven-column work fixture exposed a combined-OR scan; the current
implementation splits one leading numeric anchor with disjoint UNION ALL.
Focused selective-read tests passed 14/14 and standalone extension tests
passed 27/27. Tasks 1 and 2 passed independent specification and quality reviews.
The native Rust lifecycle example passed against the preserved alpha.6 and new
libraries, including commit/rollback/reopen, and passed both reviews. Task 4's
native benchmark harness passed seven focused tests and separate specification
and quality reviews after connecting actual prepared/bound reads and timed row
counts to the independent reference oracle. Both format checks and both full
clippy checks passed. All four final serial quick benchmark modes passed;
all eight serial quick/source-scale benchmark runs passed, including five fresh
latency repeats per library at 4,100,000 base rows and separate work processes.
Raw-round math, states, allocations and cross-mode consistency audits passed.
The changed-source full suite and mutation campaign remain pending; no such result is claimed yet.

Observed internal work on the seven-column, 20,000-group fixture:

| Read | VM steps | Fullscan steps | Materialized candidates | Outer rows |
| --- | ---: | ---: | ---: | ---: |
| Full snapshot | 200,006 | 19,999 | 20,000 | 20,000 |
| Numeric EQ, INTEGER or REAL RHS | 23 | 0 | 1 | 1 |
| Numeric EQ plus time bounds | 32 | 0 | 1 | 1 |
| Leading numeric GE | 108 | 0 | 10 | 10 |
| Leading numeric LT | 124 | 0 | 10 | 10 |
| Leading numeric LE | 135 | 0 | 11 | 11 |
| Non-leading constraint | 122,010 | 19,999 | 2,000 | 2,000 |
| TEXT EQ candidate superset | 220,015 | 19,999 | 1 | 0 |

These are instrumented fixture work counts, not application latency. The
replaced combined-OR equality shape took 100,016 VM steps and 19,999 fullscan
steps despite materializing one candidate. Keep that case as an OR-collapse
mutation target; final mutation verification is still deferred.

Independent checks retained outside the checkout:

- `ivmlite-selective-reads-2026-10-21/affinity/` contains executable candidate
  probes, observed results, official SQLite 3.45 source references and the
  BLOB-affinity reasoning. The recommended numeric, TEXT and direct lower-bound
  candidates each produced zero false negatives in the recorded pools.
- `quality-review-probe.py` reproduced malformed-UTF-8 RHS failure and a
  Cartesian join materializing its snapshot 1,000 times. Re-review of the
  rebuilt library confirmed both now match baseline, including one snapshot
  execution for that join.
- `sqlite345/smoke.json` records a native SQLite 3.45.0 CLI run: 74 checks passed,
  plus four reopen/write/refresh/rollback checks on a database created with the
  preserved alpha.6 library. The source amalgamation's SHA3-256 matched the
  official 3.45.0 release. Its first smoke attempt had an incorrect expected
  UTF-8 comparison count (five instead of four); that fixture expectation was
  corrected, with the initial stdout retained.

Mutation verification starts after the source-scale benchmark, with the code
and test inventory frozen. Seventeen exact mutation patterns are prepared but
remain unverified until compiled, run through the full suite and restored.
The final commit follows complete evidence, documentation and required checks.
No new release is in scope. If quota is exhausted, preserve this checkpoint
and wait five hours before retrying, as the user authorized.

### 2026-10-22 mutation checkpoint correction

The initial full baseline passed 876/0 with three ignored tests across 34
result binaries. Seven mutation groups were accepted before the row-estimate
mutation remained green: an equality-only JOIN guard did not exercise the
original ordinary Cartesian-JOIN regression. The existing test now retains
equality coverage and adds SUM(v.n)=1000 plus one actual nested read for the
Cartesian query. An exact million-row estimate mutation preserved the sum but
performed 1,000 reads, turning the strengthened guard red. Clean focused
integration tests passed 14/14. Production and benchmark source hashes, the
unchanged host and both preserved release libraries were rechecked; the
source-scale measurements remain applicable.

The full mutation campaign restarts from a fresh frozen source and baseline;
earlier acceptance is retained as a superseded checkpoint, not reused as
final acceptance. The first unusable-constraint registration attempt also
remains retained: two actual IN/join guards failed, but the initially named
collation guard stayed green. Its primary registration was corrected and
rerun before the later restart. Final evidence distinguishes both corrections.

### Final acceptance: 2026-10-22

All four tasks passed acceptance. On the strengthened frozen checkpoint,
`scripts/test-all.sh` passed 876 tests, zero failed and three ignored across
34 result binaries. All 17 separate mutations compiled, failed their named
primary guard through that same complete command, and restored source bytes.
The final clean run reproduced all 876/0/3 and the full per-binary inventory.
No filtered or compile-invalid result is counted as a verified gate.

Both format checks and both full clippy checks passed on the final native
source. The native Rust lifecycle example passed against preserved old/new
libraries; native SQLite 3.45 smoke passed 74 checks plus four alpha.6 reopen
checks. Eight serial quick/source-scale benchmark runs and the independent
raw-round/state/allocation audit passed. Full-read latency was 192.086 to
190.798 ms; monitor reads 142.199896 to 1.085917 ms; monitor/window reads
142.640687 to 0.1324375 ms. Those figures are medians of five fresh repeat
medians. The modeled transaction with one window read was 155.786354 to
13.713605 ms. Work traces confirm leading seeks and non-leading scans.

[Measured demo and reproduction](../../demos/selective-reads.md),
[mutation evidence](../../validation/selective-reads/README.md), and all raw
rounds/logs are retained. The mutation ledger is mechanically consistent at
688 rows: 653 verified, zero unverified and 35 n/a. Historical M2c evidence
still validates; only its whole-ledger binding changed. Final independent
whole-change code and documentation/evidence reviews approved without further
findings. The consolidated local commit retains the source-only scope; no
version change, new release, tag or push is part of this feature.
