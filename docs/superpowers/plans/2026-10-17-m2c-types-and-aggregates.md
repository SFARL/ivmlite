# M2c: Column Types, Non-STRICT Tables and More Aggregates — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept non-STRICT base tables and every column type, carry all five SQLite storage classes through capture, state and output unchanged, group/compare/join with SQLite's value semantics, and add exact `SUM`, `TOTAL`, `AVG`, `COUNT(col)` and retraction-safe `MIN`/`MAX`, checked by a widened differential test with an independent bit-exact reference.

**Architecture:**
- **Core (`crates/ivmlite-core`)** gets the value model (`Value::Real`, `Value::Blob`, identity equality vs. SQL order vs. group canonicalization), a hand-written fixed-point superaccumulator (`exact.rs`), SQLite's numeric classification of TEXT/BLOB (`numeric.rs`), an order-preserving MIN/MAX value encoding (`sortkey.rs`), and the new aggregate state in `agg.rs`.
- **The SQL front end (`crates/ivmlite-sql`)** parses the new aggregates and REAL/BLOB literals; `ivmlite_core::lower` converts literals by the column's affinity and checks join affinity.
- **The extension (`crates/ivmlite-sqlite`)** declares every value-holding shadow column with no type (format 6), reads affinity from the declared type, reads MIN/MAX through an indexed per-aggregate state table, and stores the new state layout.
- **The test crate (`crates/ivmlite-test`)** gets an independent reference (Shewchuk summation, an error-free exact mean, SQLite-classified terms), a comparator that is bit-exact against the reference and within KBN's bound against SQLite, and a widened three-column, mixed-type enumeration.

**Tech Stack:** Rust 1.95, rusqlite 0.40 (bundled SQLite 3.53.2 in the test host; the extension builds with `loadable_extension`), sqlparser 0.63, rand 0.10 (test crate only), Python 3.12 standard library for the bench tables script.

**Spec:** `docs/superpowers/specs/2026-10-17-m2c-types-and-aggregates-design.md` (binding). Its parent specs' conventions (Phase 3a, 3b, 5, 6 and 7) still hold. Read the spec before any task.

## Global Constraints

**From the spec, verbatim:**
- "**Accept non-STRICT base tables** and every column type, judged by SQLite's affinity rules (§3)."
- "**Carry every storage class** (NULL, INTEGER, REAL, TEXT, BLOB) through capture, state and output, unchanged (§2, §3)."
- "SQLite never stores a NaN; it stores NULL instead. So a `Real` here is never NaN, and the decoder refuses one as corrupted state."
- "**Row identity**, which Z-set consolidation uses, is exact by storage class. `Int(1)`, `Real(1.0)` and `Text("1")` are three different rows, and two `Real`s are equal only bit for bit."
- "**Collations.** A non-BINARY collation is still refused, by the existing conservative `COLLATE` check." (BINARY collation only.)
- "**Format.** The shadow-table format becomes **6**. A format-5 database is refused, and its views still open as broken views that can be dropped (Phase 3a §5)."
- "The behaviour tested is SQLite 3.45 and later".
- "A difference beyond that bound is a test failure to investigate, not a tolerance to widen."
- "**No regression on integer workloads.** … Integer-only workloads must not get slower by more than 5% in steady state; if they do, the spec records why."
- "**Gate rows.** Every new rule gets a verified gate row".
- **Not in scope** (spec §1): collations other than BINARY; expressions in GROUP BY or SELECT; views without aggregates; a richer WHERE; `DISTINCT` aggregates and `group_concat`; joins between columns of different affinity; retention or sealing.
- **Release:** "`v0.1.0-alpha.6`, with a CHANGELOG entry, release notes and format 6."

**Repository rules:**
- **Language.** English only in every file, comment, error message, doc and commit message (ivmlite ships as a public SQLite extension).
- **Commits.** Stage specific files (never `git add -A`), never use `--no-verify`, end every commit message with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never merge, push, tag or publish: **publishing, tagging and pushing are done only after the controller asks the user first.**
- **Dependencies.** No new dependencies, in any crate, including dev-dependencies. The superaccumulator, the Shewchuk reference, the exact mean and the property-test random source are written by hand; core's property tests use a hand-written SplitMix64 in `crates/ivmlite-core/src/test_support.rs`, because `rand` is not a core dependency. Any task that believes it needs a crate stops and asks the controller, who asks the user.
- **Extension conventions** (Phase 3a §4–§5, 3b, 5, 6, 7): every identifier is quoted; statements outside trigger bodies are `"main"`-qualified; the arming statement is the single last statement of every apply; every callback runs inside `guard`; a broken view still connects and can be dropped; every cached statement is reset before its callback returns, and no cached statement names an ivmlite virtual table.
- **Messages.** Every existing message keeps its exact text unless a task names the change. New messages are exactly the texts this plan gives.
- **Build and test.**
  - `crates/ivmlite-sqlite` is outside the workspace, with its own `Cargo.lock`. Build it with `scripts/build-extension.sh` before any `ivmlite-test` test; never build a copy into the repository's `target/`.
  - Put SQL-executing tests in `crates/ivmlite-test`.
  - Tests that compare floats exactly compare `to_bits()`, or put `#[allow(clippy::float_cmp)]` on the test module, as `data.rs` already does; never loosen an exact comparison to satisfy clippy.
  - **Before every commit**, run and pass:
    - `cargo fmt --all -- --check`
    - `cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check`
    - `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - `cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings`
    - `scripts/test-all.sh`
- **Mutation gates** (`docs/mutation-gates.md`, "How to use it" and "Registration requirements"):
  - Every new spec invariant gets a row **in the task that adds it**: mutate, confirm it compiles, run `scripts/test-all.sh`, record `N passed / M failed (baseline X/0)` read off the terminal (sum both cargo invocations), name the red tests, restore, rebuild.
  - A row whose check this phase removes is not deleted: its status cell becomes `n/a — superseded by M2c Task N (spec §k): <what replaced it>`, so `scripts/count-mutation-gates.py` still classifies it.
  - **Any change to the enumerated query space** (Task 11) re-runs every row whose red set names or excludes a differential sweep.
  - `python3 scripts/count-mutation-gates.py --fix`, then `python3 scripts/count-mutation-gates.py` must print `consistent`.
  - Comments cite spec sections ("M2c spec §4"), never ledger rulings.

## Plan decisions (where the spec leaves a choice)

Each is repeated in the task that implements it.

1. **MIN/MAX value encoding (spec §4).** Numeric entries are `0x01`, 8 bytes order-preserving `f64`, 8 bytes order-preserving `i64` tie, then **one more byte**: `0x00` Int, `0x01` Real, `0x02` Real `-0.0`. A Real's tie is its value truncated toward zero and saturated to the `i64` range; `-0.0`'s `f64` prefix is that of `+0.0`. *Rationale:* the spec's 8-byte "marker for a Real" cannot order exactly (see Spec defects); this is the smallest change that orders every pair exactly and keeps `Int` first in ties.
2. **TEXT and BLOB in the MIN/MAX encoding.** Class byte `0x02` (TEXT) or `0x03` (BLOB); each `0x00` byte of the payload is written `0x00 0xFF`; the terminator is `0x00 0x00`. *Rationale:* the standard escape that keeps `memcmp` order equal to the raw bytes' order, prefixes included.
3. **MAX in a numeric tie** outputs the entry that sorts last (a `Real` over an `Int`, `-0.0` over `+0.0`). *Rationale:* the spec pins only MIN's tie; one encoding read in both directions keeps one comparator and one property test.
4. **The superaccumulator** is 35 little-endian two's-complement `u64` limbs whose least significant bit is worth 2^-1074 (2,240 bits: |x·w| < 2^1087, 2^64 such terms < 2^1151, plus 1074 fractional bits and a sign). *Rationale:* the spec's "about 2,200 bits" rounded up to whole limbs.
5. **Infinities.** SQLite stores ±Inf in a REAL column, and `'1e400'` converts to Inf, but the superaccumulator covers finite values only. Each SUM/TOTAL/AVG accumulator counts +Inf and −Inf inputs by weight; with both present the result is NULL (SQLite's NaN, which it returns as NULL), with one sign present it is that infinity. *Rationale:* matches SQLite (measured: `sum` of `1e999` and `-1e999` is NULL) without widening the fixed point.
6. **Stored aggregate layout.** `[rows, real_keys, per-aggregate values…]` where `real_keys` is NULL when every count is 0, else a BLOB of one big-endian `i64` per group-by column; `COUNT(*)` stores nothing; `COUNT(x)` stores `[non_null]`; `SUM`/`TOTAL`/`AVG` store `[non_null, int_sum, approx]` with `int_sum` an `Int` when it fits `i64`, else a 16-byte big-endian two's-complement BLOB, and `approx` NULL when the approximate count is 0, else a 304-byte BLOB (approx count, +Inf count, −Inf count as big-endian `i64`, then the 280 limb bytes, most significant limb first); `MIN`/`MAX` store nothing in the group row. *Rationale:* an integer-only group stores fewer bytes than format 5 (29 instead of 45 bytes for one key, one SUM, one COUNT), which protects the 5% budget; the layout is fixed, so the encoding stays canonical.
7. **A REAL literal compared with a TEXT-affinity column is refused** at compile time (`a REAL literal cannot be compared with the TEXT-affinity column <c>: SQLite's REAL-to-TEXT rendering differs between versions (M2c spec §5); compare with a quoted string`). An INTEGER literal becomes its decimal text. *Rationale:* SQLite 3.53 renders `0.1+0.2` as `0.30000000000000004` where older versions used `%.15g`; the conversion cannot be reproduced at compile time.
8. **Text-to-number conversion** is SQLite's `sqlite3AtoF` grammar implemented in Rust, with the digits converted by Rust's correctly rounded `f64` parser; a test pins it against the host SQLite on a corpus. The differential domain keeps convertible TEXT at ≤ 15 significant digits, where every conforming conversion agrees.
9. **`ColumnType` becomes the affinity**: variants `Integer`, `Real`, `Numeric`, `Text`, `Blob`. The declared type string lives only in the extension's table shape (`CaptureInfo::declared_types`). A STRICT `ANY` column has `Blob` affinity (SQLite stores it unchanged). *Rationale:* least churn; nothing in core needs the declared text.
10. **Every value column of every shadow table and of the virtual table's declaration has no type**, including the state tables' `key`/`val` and the stage's `cN`; bookkeeping columns keep theirs. Output `Column::ty` in `CompiledView` becomes informational: `COUNT` → `Integer`, `SUM` → `Numeric`, `TOTAL`/`AVG` → `Real`, `MIN`/`MAX` → the argument's affinity, a group key → its column's.
11. **The MIN/MAX read** is `SELECT val, w FROM <t> WHERE key = ?1 ORDER BY val [DESC]` stepped lazily and stopped at the first live entry, without `LIMIT 1`, because Phase 3a buffers the refresh's writes and the first stored row may be retracted by a pending change. In the steady state it reads one row.
12. **ε in the KBN check is the unit roundoff 2^-53.** For AVG the bound is `4ε·Σ|xᵢ|/n + 2ε·|avg_sqlite|` (SQLite rounds the sum, then the quotient).
13. **The widened enumeration** crosses every aggregate with every group-by only under no predicate, and crosses predicates with two aggregate sets; join aggregates rotate with the column (Task 11 gives the exact sets and counts).
14. **Integer overflow in the differential harness.** When SQLite reports `integer overflow` (a prefix overflow) the case is checked against the reference only; when the reference's final exact sum overflows, the engine must fail with `integer overflow in SUM`, and the case ends there. Integers near ±2^63 are drawn with probability 1/64 so few cases end early.
15. **The generator draws stored-form values**: one pool per affinity, each value a fixed point of that affinity, checked by a test against SQLite. *Rationale:* the in-memory engines see exactly what SQLite stores, with no normalization at run time.
16. **The demos' REAL values are dyadic** (FluxFlow `amount = (sat mod 2^32) / 256`, Kener `latency_ms` a multiple of 0.25), so every summation order is exact and the demos' exact multiset oracle stays valid in every mode; rounding is the differential tests' job.
17. **`AVG` over integers only** is also the exact sum divided once (the spec's AVG row has no integer branch).
18. **The arrangement reading MIN/MAX** is a new `Arrangement::first(key, descending)` with a default implementation over `get`.

## Spec defects found while planning

1. **§4, MIN/MAX encoding:** "8 bytes that break ties exactly: the `i64` for an `Int`, or a marker for a `Real`" cannot order exactly. `Int(i64::MAX)` and `Real(2^63)` share their `f64` prefix, and the Real must sort after `0xFF…FF`; `Real(2^60)` must sort between `Int(2^60)` and `Int(2^60 + 1)`, whose ties are adjacent. Any constant marker collides or misorders. Also `-0.0` and `0.0` are distinct entries that must tie with `Int(0)`. Plan decision 1 fixes it with a value-dependent tie plus one class byte.
2. **§4 says "§6 pins the exact versions"**, but §6 pins none. Plan: the tests run on the bundled SQLite 3.53.2; the README records it and "3.45 and later".
3. **§4, "one indexed `SELECT val … LIMIT 1`"** conflicts with Phase 3a §5's buffered writes: during a refresh the stored minimum may already be retracted by a pending change, which `LIMIT 1` cannot skip. Plan decision 11.
4. **§5, "for a TEXT-affinity column, a numeric literal becomes TEXT"** is not reproducible for REAL literals at compile time, because SQLite's REAL-to-TEXT rendering is version-dependent. Plan decision 7 narrows it.
5. **§4 covers "every finite `f64`"**, but SQLite stores ±Inf in REAL columns and converts `'1e400'` to Inf. The spec is silent; plan decision 5.
6. **§6, "INTEGER … results … must equal SQLite's exactly"** contradicts §4's overflow rule whenever the domain holds integers near 2^63 (which §6 requires): SQLite fails on an intermediate prefix overflow that ivmlite does not report. Plan decision 14.
7. **§6's KBN bound is stated for sums only**; AVG divides after rounding. Plan decision 12.

## Review Focus

1. **Canonical persisted state.** The same live multiset always encodes to the same bytes: `approx` is dropped exactly when its count returns to 0, `int_sum` switches between `Int` and BLOB only by range, `real_keys` is NULL exactly when all counts are 0. (Tasks 5, 6)
2. **Exactness of `ExactSum`** at subnormals, ±max finite, cancellation and 2^60 equal terms, and in `div_round`'s rounding with the remainder as sticky bit. (Task 3)
3. **The buffered MIN/MAX read** sees a refresh's own pending writes: retracting the current minimum and adding a smaller one in the same refresh, across two base tables of a join. (Task 8)
4. **Non-STRICT capture.** `-0.0`, integral reals, convertible TEXT and BLOB written through INSERT, UPDATE and REPLACE into every affinity, captured with the class SQLite stored, and cancelling on DELETE. (Task 7)
5. **The 5% budget.** Integer workloads before/after, with the reason recorded in the spec if any cell regresses. (Task 13)
6. **The reference is independent.** It shares no summation code with the extension, classifies terms through SQLite itself, and the comparator never widens a tolerance. (Task 10)

## File map

| File | Responsibility | Tasks |
|---|---|---|
| `crates/ivmlite-core/src/value.rs` | Value domain, identity order, SQL order, canonical group key | 1 |
| `crates/ivmlite-core/src/exact.rs` (new) | Fixed-point superaccumulator | 3 |
| `crates/ivmlite-core/src/numeric.rs` (new) | `sqlite3AtoF` grammar, term classification, numeric affinity of text | 4 |
| `crates/ivmlite-core/src/sortkey.rs` (new) | MIN/MAX order-preserving encoding | 8 |
| `crates/ivmlite-core/src/agg.rs` | Group state, key canonicalization, accumulators, extremes | 5, 6, 8 |
| `crates/ivmlite-core/src/arrangement.rs` | `Arrangement::first`, `AggregateExtremum` role | 8 |
| `crates/ivmlite-core/src/node.rs` | Builds extremum arrangements; `passes` by SQL order | 1, 8 |
| `crates/ivmlite-core/src/plan.rs` | Canonical text, `lower` checks, literal affinity | 1, 6, 8, 9 |
| `crates/ivmlite-core/src/query.rs`, `schema.rs`, `join.rs`, `lib.rs` | `AggFn`, `ColumnType`, canonical join keys, exports | 6, 7, 8, 9 |
| `crates/ivmlite-sql/src/compile.rs` | New aggregates, REAL/BLOB literals, output types | 6, 7, 8, 9 |
| `crates/ivmlite-sqlite/src/encode.rs` | REAL and BLOB tags | 1 |
| `crates/ivmlite-sqlite/src/view.rs` | Format 6, typeless shadow tables, shape, value conversion | 1, 2, 7, 8 |
| `crates/ivmlite-sqlite/src/catalog.rs` | Affinity, non-STRICT acceptance, declared types | 7 |
| `crates/ivmlite-sqlite/src/state.rs`, `names.rs` | Value codecs, `first`, extremum table names | 8 |
| `crates/ivmlite-test/src/exact_reference.rs` (new) | Shewchuk `fsum`, error-free exact mean | 10 |
| `crates/ivmlite-test/src/compare.rs` (new) | Reference/SQLite comparator, KBN bound | 10 |
| `crates/ivmlite-test/src/naive.rs` | Reference evaluator over SQLite-classified terms | 10 |
| `crates/ivmlite-test/src/{data,query,sql,oracle,differential,extension,invariants}.rs` | Pools, enumeration, rendering, overflow outcome, exact delete | 1, 10, 11 |
| `crates/ivmlite-test/tests/extension_types.rs` (new) | Storage classes, affinity, capture, aggregates end to end | 2, 6, 7, 8, 9 |
| `crates/ivmlite-test/tests/extension_order_independence.rs` (new) | Spec §6 order independence | 12 |
| `crates/ivmlite-test/src/{fluxflow,demand_cases}.rs` | Demos in place | 13 |
| `docs/…`, `README.md`, `CHANGELOG.md`, `Cargo.toml`s, `Cargo.lock`s | Measurement, docs, release prep | 13, 14 |

---
### Task 1: The value domain carries five storage classes (spec §2, §3 "Encoding")

**Files:**
- Modify: `crates/ivmlite-core/src/value.rs` (whole file, 29 lines), `crates/ivmlite-core/src/node.rs:195-212` (`passes`), `crates/ivmlite-core/src/plan.rs:148-171` (`canonical_predicate`)
- Modify: `crates/ivmlite-sqlite/src/encode.rs:1-65`, `crates/ivmlite-sqlite/src/view.rs:939-951` (`value_of`) and `:1203-1209` (`sql_value`)
- Modify: `crates/ivmlite-test/src/oracle.rs:9-31` (`Bound`, `from_sqlite`), `crates/ivmlite-test/src/sql.rs:32-39` (`sql_literal`), `crates/ivmlite-test/src/extension.rs:177-188` (`sql_value`) and every other site the compiler reports as a non-exhaustive `match` on `Value` (known: `crates/ivmlite-test/src/demo_bench.rs`, `crates/ivmlite-test/examples/demand_demos.rs`, `crates/ivmlite-test/tests/common/mod.rs`, `crates/ivmlite-test/tests/extension_refresh_all.rs`)
- Modify: `docs/mutation-gates.md`
- Test: unit tests in `value.rs`, `encode.rs`, `plan.rs`, `node.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  // crates/ivmlite-core/src/value.rs
  pub enum Value { Null, Int(i64), Real(f64), Text(String), Blob(Vec<u8>) }
  // PartialEq/Eq/Hash/PartialOrd/Ord: identity (spec §2 "Row identity").
  impl Value {
      /// Spec §2 group equality: integral in-range Real → Int, -0.0 → Int(0).
      pub fn canonical(&self) -> Value;
      /// Spec §2 order: NULL < numeric < TEXT < BLOB, numerics exact.
      pub fn sql_cmp(&self, other: &Value) -> std::cmp::Ordering;
  }
  ```
  Row-encoding tags in `encode.rs`: `0x03` REAL (8 bytes of `f64::to_bits`, big-endian), `0x04` BLOB (4-byte big-endian length, then the bytes). Plan decision: the next two free tags.

No format bump in this task: no input path can produce a `Real` or a `Blob` yet (the catalog still refuses every type but INTEGER and TEXT), and INTEGER/TEXT/NULL encode exactly as in format 5.

- [ ] **Step 1: Write the failing tests** in `crates/ivmlite-core/src/value.rs`'s test module (replace the two existing tests' module body with this, keeping both existing tests):

```rust
use std::cmp::Ordering;
use std::collections::BTreeSet;

#[test]
fn int_real_and_text_with_same_digits_are_three_rows() {
    // Spec §2: row identity is exact by storage class.
    let set: BTreeSet<Value> = [Value::Int(1), Value::Real(1.0), Value::Text("1".into())]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 3);
}

#[test]
fn real_identity_is_bitwise() {
    // Spec §2: two Reals are equal only bit for bit, so 0.0 and -0.0 are two rows.
    assert_ne!(Value::Real(0.0), Value::Real(-0.0));
    assert_eq!(Value::Real(0.1 + 0.2), Value::Real(0.1 + 0.2));
}

#[test]
fn canonical_maps_integral_reals_and_negative_zero_to_int() {
    assert_eq!(Value::Real(1.0).canonical(), Value::Int(1));
    assert_eq!(Value::Real(-0.0).canonical(), Value::Int(0));
    assert_eq!(Value::Real(-9_223_372_036_854_775_808.0).canonical(), Value::Int(i64::MIN));
    // Outside the i64 range, non-integral, infinite: unchanged.
    assert_eq!(Value::Real(9_223_372_036_854_775_808.0).canonical(), Value::Real(9_223_372_036_854_775_808.0));
    assert_eq!(Value::Real(1.5).canonical(), Value::Real(1.5));
    assert_eq!(Value::Real(f64::INFINITY).canonical(), Value::Real(f64::INFINITY));
    // TEXT never becomes a number: '1' differs from 1 (spec §2).
    assert_eq!(Value::Text("1".into()).canonical(), Value::Text("1".into()));
}

#[test]
fn sql_order_is_null_then_numeric_then_text_then_blob() {
    let ordered = [
        Value::Null,
        Value::Real(f64::NEG_INFINITY),
        Value::Int(-5),
        Value::Real(2.5),
        Value::Int(3),
        Value::Text("".into()),
        Value::Text("a".into()),
        Value::Blob(vec![]),
        Value::Blob(vec![0]),
    ];
    for (i, a) in ordered.iter().enumerate() {
        for (j, b) in ordered.iter().enumerate() {
            assert_eq!(a.sql_cmp(b), i.cmp(&j), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn int_and_real_compare_exactly_near_2_pow_53_and_2_pow_63() {
    let two53 = 9_007_199_254_740_992_i64;
    assert_eq!(Value::Int(two53 + 1).sql_cmp(&Value::Real(two53 as f64)), Ordering::Greater);
    assert_eq!(Value::Int(two53).sql_cmp(&Value::Real(two53 as f64)), Ordering::Equal);
    assert_eq!(Value::Int(i64::MAX).sql_cmp(&Value::Real(9_223_372_036_854_775_808.0)), Ordering::Less);
    assert_eq!(Value::Int(i64::MIN).sql_cmp(&Value::Real(-9_223_372_036_854_775_808.0)), Ordering::Equal);
    assert_eq!(Value::Int(1).sql_cmp(&Value::Real(1.5)), Ordering::Less);
    assert_eq!(Value::Int(-1).sql_cmp(&Value::Real(-1.5)), Ordering::Greater);
    assert_eq!(Value::Real(0.0).sql_cmp(&Value::Real(-0.0)), Ordering::Equal);
}

#[test]
fn text_and_blob_compare_by_bytes() {
    // BINARY collation: 'Ā' (0xC4 0x80) sorts after 'b'.
    assert_eq!(Value::Text("Ā".into()).sql_cmp(&Value::Text("b".into())), Ordering::Greater);
    assert_eq!(Value::Blob(vec![1, 2]).sql_cmp(&Value::Blob(vec![1, 2, 0])), Ordering::Less);
}
```

In `crates/ivmlite-sqlite/src/encode.rs`'s tests, extend `row()` with `Value::Real(-0.0)`, `Value::Real(f64::MAX)`, `Value::Blob(vec![])`, `Value::Blob(vec![0, 0xff])`, and add:

```rust
#[test]
fn real_and_blob_have_the_documented_layout() {
    let bytes = encode(&Row::new(vec![Value::Real(1.0), Value::Blob(vec![7])]));
    assert_eq!(
        bytes,
        vec![0x03, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0, 0x04, 0, 0, 0, 1, 7]
    );
}

#[test]
fn a_nan_real_is_corrupted_state() {
    let mut bytes = vec![0x03];
    bytes.extend_from_slice(&f64::NAN.to_bits().to_be_bytes());
    let e = decode(&bytes).expect_err("spec §2: SQLite never stores a NaN");
    assert!(e.0.contains("corrupted state row encoding: REAL is NaN"), "{e}");
}

#[test]
fn negative_zero_round_trips_bit_for_bit() {
    let back = decode(&encode(&Row::new(vec![Value::Real(-0.0)]))).unwrap();
    match back.get(0) {
        Value::Real(x) => assert_eq!(x.to_bits(), (-0.0f64).to_bits()),
        other => panic!("{other:?}"),
    }
}
```

and add to the `corrupt_bytes_are_an_error_not_a_panic` list `vec![0x03, 0, 0]` (truncated REAL) and `vec![0x04, 0, 0, 0, 5, 1]` (BLOB shorter than its length).

In `crates/ivmlite-core/src/plan.rs`'s tests add:

```rust
#[test]
fn real_and_blob_literals_have_exact_canonical_texts() {
    let p = |value: Value| canonical_predicate(&Predicate::Compare { column: 0, op: CmpOp::Eq, value });
    assert_eq!(p(Value::Real(1.5)), "eq(0,real:3ff8000000000000)");
    assert_eq!(p(Value::Real(-0.0)), "eq(0,real:8000000000000000)");
    assert_eq!(p(Value::Blob(vec![0xab, 0x01])), "eq(0,blob:2:ab01)");
}
```

In `crates/ivmlite-core/src/node.rs`'s tests add:

```rust
#[test]
fn comparisons_use_sqlites_cross_class_order() {
    // Spec §2/§5: numerics compare exactly across Int and Real; TEXT sorts above numbers.
    let gt_2 = Predicate::Compare { column: 0, op: CmpOp::Gt, value: Value::Int(2) };
    assert!(passes(&gt_2, &row(vec![Value::Real(2.5)])));
    assert!(!passes(&gt_2, &row(vec![Value::Real(2.0)])));
    assert!(passes(&gt_2, &row(vec![Value::Text("1".into())])));
    let eq_1 = Predicate::Compare { column: 0, op: CmpOp::Eq, value: Value::Int(1) };
    assert!(passes(&eq_1, &row(vec![Value::Real(1.0)])));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked value:: 2>&1 | tail -5`
Expected: FAIL to compile — `no variant or associated item named Real found for enum Value`.

- [ ] **Step 3: Implement `value.rs`.** Replace the file's non-test part with:

```rust
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// The value domain (M2c spec §2): SQLite's five storage classes.
///
/// Equality, hashing and `Ord` are **row identity**, which Z-set
/// consolidation uses: exact by storage class, and a `Real` equals another
/// only bit for bit, so `Int(1)`, `Real(1.0)`, `Text("1")`, `Real(0.0)` and
/// `Real(-0.0)` are five different rows. `Ord` is a total order consistent
/// with that equality and is **not** SQL's order: use `sql_cmp` for
/// comparisons and MIN/MAX, and `canonical` for grouping and join keys.
///
/// A `Real` is never NaN: SQLite stores NULL instead, and every decoder
/// refuses a NaN as corrupted state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// 2^63 as an `f64`: the first value above the `i64` range.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

impl Value {
    /// The storage class's position in the identity order.
    fn rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Int(_) => 1,
            Value::Real(_) => 2,
            Value::Text(_) => 3,
            Value::Blob(_) => 4,
        }
    }

    /// M2c spec §2: the group-key form. An integral `Real` within the `i64`
    /// range becomes `Int`, and `-0.0` becomes `Int(0)`; every other value is
    /// unchanged. Canonical values then compare by exact equality, which is
    /// SQLite's GROUP BY equality.
    pub fn canonical(&self) -> Value {
        match self {
            Value::Real(x) if x.fract() == 0.0 && (-TWO_POW_63..TWO_POW_63).contains(x) => {
                Value::Int(*x as i64)
            }
            other => other.clone(),
        }
    }

    /// M2c spec §2: SQLite's cross-class order, NULL < numeric < TEXT < BLOB.
    /// Numerics compare by exact value, `Int` against `Real` included; TEXT
    /// and BLOB compare with `memcmp` (BINARY). Two NULLs compare equal here;
    /// a predicate treats NULL as UNKNOWN before it ever calls this.
    pub fn sql_cmp(&self, other: &Value) -> Ordering {
        let class = |v: &Value| match v {
            Value::Null => 0,
            Value::Int(_) | Value::Real(_) => 1,
            Value::Text(_) => 2,
            Value::Blob(_) => 3,
        };
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Real(a), Value::Real(b)) => a.partial_cmp(b).expect("a Real is never NaN"),
            (Value::Int(a), Value::Real(b)) => cmp_int_real(*a, *b),
            (Value::Real(a), Value::Int(b)) => cmp_int_real(*b, *a).reverse(),
            (Value::Text(a), Value::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            _ => class(self).cmp(&class(other)),
        }
    }
}

/// `i` against `r`, exactly: no rounding of `i` to `f64`.
fn cmp_int_real(i: i64, r: f64) -> Ordering {
    if r >= TWO_POW_63 {
        return Ordering::Less;
    }
    if r < -TWO_POW_63 {
        return Ordering::Greater;
    }
    let whole = r.trunc();
    match i.cmp(&(whole as i64)) {
        // Same integer part: the fraction decides. `r - whole` is exact.
        Ordering::Equal => 0.0f64.partial_cmp(&(r - whole)).expect("a Real is never NaN"),
        other => other,
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            // `total_cmp` is equality exactly when the bits are equal.
            (Value::Real(a), Value::Real(b)) => a.total_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rank().hash(state);
        match self {
            Value::Null => {}
            Value::Int(n) => n.hash(state),
            Value::Real(x) => x.to_bits().hash(state),
            Value::Text(s) => s.hash(state),
            Value::Blob(b) => b.hash(state),
        }
    }
}
```

The identity order keeps `Null < Int < Text` as before, so every existing `ZSet`'s iteration order is unchanged.

- [ ] **Step 4: Implement the other match sites.**
  - `node.rs` `passes`: replace the `Compare` arm's inner `match` with
    ```rust
    Predicate::Compare { column, op, value } => match row.get(*column) {
        // UNKNOWN for every operator, `!=` included: `NULL != 4` is not true.
        Value::Null => false,
        // M2c spec §2/§5: SQLite's cross-class order; TEXT and BLOB by bytes (BINARY).
        cell => op.holds(cell.sql_cmp(value)),
    },
    ```
    and update the doc comment: the old `unreachable!` arm is gone.
  - `plan.rs` `canonical_predicate`: add
    ```rust
    Value::Real(x) => format!("real:{:016x}", x.to_bits()),
    Value::Blob(b) => format!(
        "blob:{}:{}",
        b.len(),
        b.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
    ),
    ```
  - `encode.rs`: add `const REAL: u8 = 0x03; const BLOB: u8 = 0x04;`, update the module comment's layout sentence, encode `Real(x)` as `REAL` + `x.to_bits().to_be_bytes()` and `Blob(b)` as `BLOB` + `u32` big-endian length + bytes (`expect("a BLOB value SQLite stored fits in 4 GiB")`). Decode `REAL` with `split_first_chunk::<8>()` (`corrupt("truncated REAL")`), and refuse NaN with `corrupt("REAL is NaN")`; decode `BLOB` like TEXT without the UTF-8 check (`corrupt("truncated BLOB length")`, `corrupt("truncated BLOB")`).
  - `view.rs` `value_of`: `ValueRef::Real(x) if x.is_nan() => Err("a NaN REAL value, which SQLite never stores".into())`, `ValueRef::Real(x) => Ok(Value::Real(x))`, `ValueRef::Blob(b) => Ok(Value::Blob(b.to_vec()))`; the function is now exhaustive, so delete the `other =>` arm. `sql_value`: `Value::Real(x) => Real(*x)`, `Value::Blob(b) => Blob(b.clone())`.
  - `ivmlite-test` `oracle.rs`: `Bound` gains the two arms; `from_sqlite` maps `ValueRef::Real(x)` to `Value::Real(x)` (an `EngineError("SQLite returned a NaN")` if NaN) and `ValueRef::Blob(b)` to `Value::Blob(b.to_vec())`.
  - `sql.rs` `sql_literal`: `Value::Real(x)` renders `real_literal(*x)` and `Value::Blob(b)` renders `X'…'` in upper-case hex:
    ```rust
    /// A REAL as SQL text that reads back to the same bits: Rust's shortest
    /// round-trip form, with `.0` for an integral value so SQLite parses a
    /// REAL, and `1e999` for an infinity (SQLite's own spelling of overflow).
    fn real_literal(x: f64) -> String {
        if x.is_infinite() {
            return if x > 0.0 { "1e999".into() } else { "-1e999".into() };
        }
        let text = format!("{x:?}");
        if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") }
    }
    ```
    `-0.0` renders `-0.0`; a test in Task 10 pins that SQLite's constant folding of `-0.0` is not relied on (the oracle binds values, it never renders data rows as literals).
  - `extension.rs` `sql_value` and every remaining compiler-reported site: map the two new variants to `rusqlite::types::Value::Real` / `Blob`, or, where the code reads values back, the reverse.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core --locked && scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS; the full suite green (record the count as this task's baseline).

- [ ] **Step 6: Gate rows** (section `## ivmlite-core`, then `## ivmlite-sqlite` for the encoding row):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §2: two Reals are equal only bit for bit | In `Ord for Value`, `a.total_cmp(b)` → `a.partial_cmp(b).unwrap()` | `real_identity_is_bitwise` |
| M2c §2: Int, Real and Text with the same digits are three rows | In `Ord for Value`, add the arm `(Value::Int(a), Value::Real(b)) => cmp_int_real(*a, *b),` before `_` (and its mirror) | `int_real_and_text_with_same_digits_are_three_rows` |
| M2c §2: group canonical form | `canonical` returns `self.clone()` | `canonical_maps_integral_reals_and_negative_zero_to_int` |
| M2c §2: numerics compare exactly | In `cmp_int_real`, return `(i as f64).partial_cmp(&r).unwrap()` | `int_and_real_compare_exactly_near_2_pow_53_and_2_pow_63` |
| M2c §2: the decoder refuses a NaN | Delete the NaN check in `decode` | `a_nan_real_is_corrupted_state` |
| M2c §2/§5: a comparison uses the cross-class order | In `passes`, `cell.sql_cmp(value)` → `cell.cmp(value)` | `comparisons_use_sqlites_cross_class_order` |

  Also: the `passes` "mixed-type arm is unreachable" row's status becomes `n/a — superseded by M2c Task 1 (spec §2): the arm is gone; passes compares by Value::sql_cmp`, and the sentence "(`Value` having no Real/Blob is guarded by the type system rather than a test: adding a variant fails to compile.)" in "Totals and outstanding debt" is deleted. Run each mutation with `scripts/test-all.sh`, record counts, restore; then `python3 scripts/count-mutation-gates.py --fix` and check it prints `consistent`.

- [ ] **Step 7: Commit**

```bash
git add crates/ivmlite-core/src/value.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/plan.rs \
  crates/ivmlite-sqlite/src/encode.rs crates/ivmlite-sqlite/src/view.rs \
  crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/sql.rs crates/ivmlite-test/src/extension.rs \
  docs/mutation-gates.md   # plus each other file the compiler made you touch, by name
git commit -m "feat(core): REAL and BLOB values with exact row identity and SQLite's order

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Format 6 — shadow tables store every storage class unchanged (spec §3 "Shadow tables", "Format")

**Files:**
- Modify: `crates/ivmlite-sqlite/src/view.rs` — `FORMAT` and its doc comment (`:22-43`), `sql_type` (`:177-182`, deleted), `shape` (`:194-236`, keeps types via the schema for now), `declaration` (`:238-268`), `track` (`:687-721`), `create_out_table` (`:903-937`), `create_stage` (`:1060-1067`, the `defs`), the state-table DDL in `create` (`:1470-1480`)
- Modify: `crates/ivmlite-test/tests/extension_sharing.rs:153-179` (format test), `docs/mutation-gates.md`
- Test: create `crates/ivmlite-test/tests/extension_types.rs`

**Interfaces:**
- Consumes: Task 1's `Value::Real`/`Value::Blob` conversions in `value_of`/`sql_value`.
- Produces: `pub const FORMAT: i64 = 6;`. Every value column of the delta, pend, stage, state and output tables, and every output column of the virtual table's declaration, is declared with no type. `shape` still renders `"<name>" INTEGER|TEXT` per column until Task 7 changes it; move `sql_type`'s two arms into `shape` as a local closure so no other function can call it.

- [ ] **Step 1: Write the failing tests.** Create `crates/ivmlite-test/tests/extension_types.rs`:

```rust
//! M2c spec §2–§5 against the real loaded extension: storage classes,
//! affinity, capture, the new aggregates and MIN/MAX.

mod common;
use common::*;

use ivmlite_test::open_with_extension;
use rusqlite::Connection;

/// `pragma_table_info(<table>)`'s `(name, type)` rows, in column order.
fn declared(c: &Connection, table: &str) -> Vec<(String, String)> {
    let mut s = c
        .prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap();
    s.query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn types_of(c: &Connection, table: &str, columns: &[&str]) -> Vec<String> {
    let all = declared(c, table);
    columns
        .iter()
        .map(|name| {
            all.iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{table} has no column {name}: {all:?}"))
                .1
                .clone()
        })
        .collect()
}

#[test]
fn shadow_value_columns_have_no_declared_type() {
    // Spec §3: SQLite applies no affinity to these columns, so it stores every
    // value with its storage class unchanged.
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    assert_eq!(types_of(&c, "__ivm_delta_orders", &["region", "amount"]), ["", ""]);
    assert_eq!(types_of(&c, "__ivm_delta_orders", &["__ivm_seq", "__ivm_w"]), ["INTEGER", "INTEGER"]);
    assert_eq!(types_of(&c, "__ivm_pend_orders", &["region", "amount"]), ["", ""]);
    assert_eq!(types_of(&c, "__ivm_out_sums", &["region", "SUM(amount)", "COUNT(*)"]), ["", "", ""]);
    assert_eq!(types_of(&c, "__ivm_out_sums", &["__w"]), ["INTEGER"]);
    assert_eq!(types_of(&c, "__ivm_stage_sums", &["c0", "c1", "c2", "key", "val"]), ["", "", "", "", ""]);
    assert_eq!(types_of(&c, "__ivm_state_sums_0_agg_groups", &["key", "val"]), ["", ""]);
    assert_eq!(types_of(&c, "sums", &["region", "SUM(amount)", "COUNT(*)"]), ["", "", ""]);
}
```

In `extension_sharing.rs`, retarget the format test at format 5: the doc comment says "M2c spec §3: format 6 declares every value column with no type. A database in format 5 (the format alpha.5 wrote) is refused …"; the tamper becomes `UPDATE __ivm_meta SET value = 5; UPDATE __ivm_view SET format = 5;`, the `TempFile` tag `"format5"`, and all four assertions check `contains("format 5") && contains("reads format 6")`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --locked --test extension_types --test extension_sharing 2>&1 | tail -8`
Expected: FAIL — `shadow_value_columns_have_no_declared_type` (left `["TEXT", "INTEGER"]`), and the format test (format 5 is accepted).

- [ ] **Step 3: Implement.**
  - `FORMAT = 6`; append to its doc comment: "Format 6 (M2c spec §3) declares every value column of the delta, pend, stage, state and output tables, and the virtual table's output columns, with no type, so SQLite stores each value with its storage class unchanged; the row encoding gains the REAL and BLOB tags, and the aggregate state its M2c layout. A format-5 database is refused, not migrated."
  - `track`: `defs` becomes `schema.columns.iter().map(|c| quote(&c.name))`.
  - `create_out_table`: `defs` becomes `quote(&c.name)` per column; `{WEIGHT} INTEGER NOT NULL` unchanged.
  - `create_stage`: `defs` becomes `format!("c{i}")`; `CREATE TABLE …(op TEXT NOT NULL, arr INTEGER, key, val, w INTEGER, tbl TEXT, seq INTEGER, {})`.
  - `create`: `CREATE TABLE {}(key NOT NULL, val NOT NULL, w INTEGER NOT NULL, PRIMARY KEY(key, val)) WITHOUT ROWID`.
  - `declaration`: each column is `quote(&c.name)`, the hidden one stays `{} HIDDEN`.
  - `shape`: inline the old `sql_type` match as `let ty = |c: &Column| match c.ty { ColumnType::Integer => "INTEGER", ColumnType::Text => "TEXT" };` and delete `sql_type`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS, whole suite.

- [ ] **Step 5: Gate rows** (`## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §3: delta and pend value columns have no type | In `track`, `defs` back to `format!("{} {}", quote(&c.name), "INTEGER")` | `shadow_value_columns_have_no_declared_type` |
| M2c §3: output value columns have no type | In `create_out_table`, `defs` back to `format!("{} TEXT", …)` | `shadow_value_columns_have_no_declared_type` |
| M2c §3: format 6, format 5 refused | `FORMAT = 5` | `a_database_in_an_older_format_is_refused_by_create_and_by_connect` |

  Task 7 adds the capture test that makes the first row observable through data as well; re-run it there and append the new red test.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/tests/extension_types.rs \
  crates/ivmlite-test/tests/extension_sharing.rs docs/mutation-gates.md
git commit -m "feat(sqlite): format 6 declares every shadow value column without a type

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The exact superaccumulator (spec §4 "Accumulator state", §6 "The superaccumulator")

**Files:**
- Create: `crates/ivmlite-core/src/exact.rs`
- Modify: `crates/ivmlite-core/src/lib.rs` (`mod exact; pub use exact::{ExactSum, EXACT_BYTES, EXACT_LIMBS};`), `crates/ivmlite-core/src/test_support.rs` (add `SplitMix64`), `docs/mutation-gates.md`
- Test: `exact.rs`'s test module

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub const EXACT_LIMBS: usize = 35;
  pub const EXACT_BYTES: usize = EXACT_LIMBS * 8; // 280
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct ExactSum { /* limbs: [u64; EXACT_LIMBS] */ }
  impl ExactSum {
      pub fn zero() -> ExactSum;
      pub fn is_zero(&self) -> bool;
      /// Add `x * weight` exactly; `x` finite (infinities are counted by the caller).
      pub fn add(&mut self, x: f64, weight: i64);
      pub fn add_int(&mut self, n: i128);
      pub fn add_sum(&mut self, other: &ExactSum);
      /// The sum, rounded once to nearest, ties to even; ±Inf beyond the f64 range.
      pub fn round(&self) -> f64;
      /// The sum divided by `divisor` (> 0), by exact long division, rounded once.
      pub fn div_round(&self, divisor: i64) -> f64;
      pub fn to_be_bytes(&self) -> [u8; EXACT_BYTES];
      pub fn from_be_bytes(bytes: &[u8]) -> Option<ExactSum>;
  }
  // test_support.rs
  pub(crate) struct SplitMix64(pub u64);
  impl SplitMix64 { pub(crate) fn next_u64(&mut self) -> u64; pub(crate) fn below(&mut self, n: u64) -> u64; }
  ```

Plan decision 4: 35 two's-complement `u64` limbs, least significant first, least significant bit worth 2^-1074. Range check: |x·w| < 2^1024·2^63 = 2^1087; 2^64 terms < 2^1151; in units of 2^-1074 that is < 2^2225, below the sign bit 2^2239. No crate: written by hand.

- [ ] **Step 1: Write the failing tests.** In `test_support.rs` add:

```rust
/// A deterministic random source for property tests (Steele, Lea and Flood's
/// SplitMix64). Hand-written: `rand` is not a dependency of ivmlite-core.
pub(crate) struct SplitMix64(pub u64);

impl SplitMix64 {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, `n > 0` (the modulo bias is irrelevant here).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}
```

`exact.rs` test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SplitMix64;

    /// A random dyadic term `m * 2^k`, |m| < 2^40, k in [-30, 30], and a weight.
    fn term(rng: &mut SplitMix64) -> (i64, i32, i64) {
        let m = rng.below(1 << 40) as i64 - (1 << 39);
        let k = rng.below(61) as i32 - 30;
        let w = rng.below(7) as i64 - 3;
        (m, k, w)
    }

    fn value((m, k, _): (i64, i32, i64)) -> f64 {
        m as f64 * 2f64.powi(k)
    }

    /// The reference built from i128 pieces (spec §6): every term is a whole
    /// number of 2^-30 units below 2^71, so the exact sum fits an i128, and
    /// `i128 as f64` rounds to nearest, ties to even.
    fn reference(terms: &[(i64, i32, i64)]) -> f64 {
        let units: i128 = terms
            .iter()
            .map(|&(m, k, w)| (i128::from(m) << (k + 30)) * i128::from(w))
            .sum();
        units as f64 * 2f64.powi(-30)
    }

    fn sum_of(terms: &[(i64, i32, i64)]) -> ExactSum {
        let mut s = ExactSum::zero();
        for &t in terms {
            s.add(value(t), t.2);
        }
        s
    }

    #[test]
    fn rounding_matches_an_i128_reference() {
        let mut rng = SplitMix64(1);
        for _ in 0..2000 {
            let n = 1 + rng.below(40) as usize;
            let terms: Vec<_> = (0..n).map(|_| term(&mut rng)).collect();
            let got = sum_of(&terms).round();
            assert_eq!(got.to_bits(), reference(&terms).to_bits(), "{terms:?}");
        }
    }

    #[test]
    fn add_then_sub_is_the_identity() {
        let mut rng = SplitMix64(2);
        let base: Vec<_> = (0..20).map(|_| term(&mut rng)).collect();
        let before = sum_of(&base);
        let mut s = before.clone();
        let extra: Vec<f64> = vec![0.1, -3.75e300, 5e-324, f64::MAX, -f64::MIN_POSITIVE];
        for &x in &extra {
            s.add(x, 3);
        }
        for &x in extra.iter().rev() {
            s.add(x, -3);
        }
        assert_eq!(s, before);
    }

    #[test]
    fn the_result_does_not_depend_on_the_order() {
        let mut rng = SplitMix64(3);
        let mut terms: Vec<_> = (0..50).map(|_| term(&mut rng)).collect();
        let forward = sum_of(&terms);
        terms.reverse();
        assert_eq!(sum_of(&terms), forward);
        terms.swap(3, 40);
        assert_eq!(sum_of(&terms), forward);
    }

    #[test]
    fn cancellation_reaches_exact_zero() {
        let mut s = ExactSum::zero();
        s.add(1e300, 1);
        s.add(1.0, 1);
        s.add(-1e300, 1);
        s.add(-1.0, 1);
        assert!(s.is_zero());
        assert_eq!(s.round().to_bits(), 0.0f64.to_bits(), "an exact zero rounds to +0.0");
    }

    #[test]
    fn subnormal_sums_are_exact() {
        let tiny = f64::from_bits(1); // 2^-1074
        let mut s = ExactSum::zero();
        s.add(tiny, 3);
        assert_eq!(s.round(), f64::from_bits(3));
        s.add(f64::MIN_POSITIVE, 1); // 2^-1022, the smallest normal
        assert_eq!(s.round(), f64::MIN_POSITIVE + f64::from_bits(3));
        let mut t = ExactSum::zero();
        t.add(tiny, -1);
        assert_eq!(t.round(), -tiny);
    }

    #[test]
    fn max_finite_and_infinity() {
        let mut s = ExactSum::zero();
        s.add(f64::MAX, 1);
        assert_eq!(s.round(), f64::MAX);
        s.add(f64::MAX, 1);
        assert_eq!(s.round(), f64::INFINITY);
        s.add(f64::MAX, -1);
        assert_eq!(s.round(), f64::MAX, "exact, so the overflow is undone");
        s.add(f64::MAX, -2);
        assert_eq!(s.round(), -f64::MAX);
    }

    #[test]
    fn two_pow_60_equal_terms() {
        let mut s = ExactSum::zero();
        s.add(1.5, 1 << 60);
        assert_eq!(s.round(), 1.5 * 2f64.powi(60));
        s.add(f64::MAX, 1 << 60);
        assert_eq!(s.round(), f64::INFINITY);
        s.add(f64::MAX, -(1 << 60));
        s.add(1.5, -(1 << 60));
        assert!(s.is_zero());
    }

    #[test]
    fn ties_round_to_even() {
        // 2^53 + 1 is exactly half-way between 2^53 and 2^53 + 2: to even, 2^53.
        let mut s = ExactSum::zero();
        s.add(2f64.powi(53), 1);
        s.add(1.0, 1);
        assert_eq!(s.round(), 2f64.powi(53));
        // 2^53 + 3 is half-way between 2^53 + 2 and 2^53 + 4: to even, 2^53 + 4.
        s.add(2.0, 1);
        assert_eq!(s.round(), 2f64.powi(53) + 4.0);
        // Just above half-way rounds up: 2^53 + 1 + 2^-60.
        let mut t = ExactSum::zero();
        t.add(2f64.powi(53), 1);
        t.add(1.0, 1);
        t.add(2f64.powi(-60), 1);
        assert_eq!(t.round(), 2f64.powi(53) + 2.0);
    }

    #[test]
    fn add_int_is_exact_across_the_i128_range() {
        let mut s = ExactSum::zero();
        s.add_int(i128::from(i64::MAX) * 4);
        s.add_int(-(i128::from(i64::MAX) * 4));
        assert!(s.is_zero());
        s.add_int(9_007_199_254_740_993); // 2^53 + 1
        assert_eq!(s.round(), 9_007_199_254_740_992.0);
        s.add(0.5, 1);
        assert_eq!(s.round(), 9_007_199_254_740_994.0, "2^53 + 1.5 rounds up");
    }

    #[test]
    fn mean_is_correctly_rounded_against_an_i128_reference() {
        let mut rng = SplitMix64(4);
        for _ in 0..2000 {
            // Positive terms with |m| in [2^39, 2^40), so the quotient keeps
            // more than 64 bits after the reference's shift.
            let n = 1 + rng.below(30) as usize;
            let terms: Vec<_> = (0..n)
                .map(|_| ((1 << 39) + rng.below(1 << 39) as i64, rng.below(41) as i32 - 20, 1))
                .collect();
            let units: i128 = terms.iter().map(|&(m, k, _)| i128::from(m) << (k + 30)).sum();
            let count = 1 + rng.below(1000) as i64;
            let want = mean_reference(units, count);
            assert_eq!(sum_of(&terms).div_round(count).to_bits(), want.to_bits(), "{terms:?} / {count}");
        }
    }

    /// `units * 2^-30 / count`, correctly rounded: shift the numerator up so
    /// the quotient has > 64 bits, keep the remainder as one sticky bit below
    /// it, and let `u128 as f64` round to nearest, ties to even.
    fn mean_reference(units: i128, count: i64) -> f64 {
        let magnitude = units.unsigned_abs();
        let shift = magnitude.leading_zeros() - 1;
        let shifted = magnitude << shift;
        let q = shifted / count as u128;
        let sticky = u128::from(shifted % count as u128 != 0);
        let value = ((q << 1) | sticky) as f64 * 2f64.powi(-30 - shift as i32 - 1);
        if units < 0 { -value } else { value }
    }

    #[test]
    fn a_subnormal_mean_rounds_at_the_smallest_subnormal() {
        let mut s = ExactSum::zero();
        s.add(f64::from_bits(3), 1); // 3 * 2^-1074
        assert_eq!(s.div_round(2), f64::from_bits(2), "1.5 ulp rounds to even, 2");
        assert_eq!(s.div_round(4), f64::from_bits(1), "0.75 ulp rounds to 1");
        assert_eq!(s.div_round(8).to_bits(), 0.0f64.to_bits(), "0.375 ulp rounds to +0.0");
    }

    #[test]
    fn bytes_round_trip() {
        let mut s = ExactSum::zero();
        s.add(-0.1, 7);
        let bytes = s.to_be_bytes();
        assert_eq!(bytes.len(), EXACT_BYTES);
        assert_eq!(ExactSum::from_be_bytes(&bytes), Some(s));
        assert_eq!(ExactSum::from_be_bytes(&bytes[1..]), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked exact:: 2>&1 | tail -3`
Expected: FAIL to compile — `ExactSum` not found.

- [ ] **Step 3: Implement `exact.rs`.**

```rust
//! M2c spec §4: the exact sum of the inputs classified as non-integer — a
//! fixed-point superaccumulator covering every finite `f64` (2^-1074 to
//! 2^1024) with headroom for 2^64 terms. Adding and subtracting are exact, so
//! the state depends only on the multiset of live inputs. Written by hand: the
//! project takes no new dependency for it.

/// 64-bit limbs, least significant first, two's complement (plan decision:
/// 35 limbs, 2,240 bits).
pub const EXACT_LIMBS: usize = 35;
/// The stored size: big-endian, most significant limb first.
pub const EXACT_BYTES: usize = EXACT_LIMBS * 8;
/// The least significant bit is worth 2^-1074, the smallest subnormal.
const LSB_SCALE: u32 = 1074;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSum {
    limbs: [u64; EXACT_LIMBS],
}

impl ExactSum {
    pub fn zero() -> ExactSum {
        ExactSum { limbs: [0; EXACT_LIMBS] }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Add `x * weight` exactly. `x` must be finite: the aggregate counts
    /// infinities itself (M2c plan decision 5).
    pub fn add(&mut self, x: f64, weight: i64) {
        assert!(x.is_finite(), "ExactSum holds finite values only");
        if x == 0.0 || weight == 0 {
            return;
        }
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as u32;
        let fraction = bits & ((1 << 52) - 1);
        // x = mantissa * 2^(shift - 1074).
        let (mantissa, shift) = if exponent == 0 {
            (fraction, 0)
        } else {
            (fraction | (1 << 52), exponent - 1)
        };
        // < 2^53 * 2^63: fits a u128.
        let magnitude = u128::from(mantissa) * u128::from(weight.unsigned_abs());
        let negative = (bits >> 63 == 1) != (weight < 0);
        self.add_shifted(magnitude, shift, negative);
    }

    /// Add the integer `n` exactly.
    pub fn add_int(&mut self, n: i128) {
        if n != 0 {
            self.add_shifted(n.unsigned_abs(), LSB_SCALE, n < 0);
        }
    }

    /// Add `other` exactly (two's-complement addition, modulo 2^2240).
    pub fn add_sum(&mut self, other: &ExactSum) {
        let mut carry = false;
        for (a, &b) in self.limbs.iter_mut().zip(&other.limbs) {
            let (s1, c1) = a.overflowing_add(b);
            let (s2, c2) = s1.overflowing_add(u64::from(carry));
            *a = s2;
            carry = c1 || c2;
        }
    }

    /// `magnitude * 2^shift` units, added or — `negative` — subtracted.
    fn add_shifted(&mut self, magnitude: u128, shift: u32, negative: bool) {
        let parts = shifted_parts(magnitude, shift % 64);
        let first = (shift / 64) as usize;
        if negative {
            self.sub_parts(first, parts);
        } else {
            self.add_parts(first, parts);
        }
    }

    fn add_parts(&mut self, first: usize, parts: [u64; 3]) {
        let mut carry = 0u64;
        for i in first..EXACT_LIMBS {
            let part = parts.get(i - first).copied().unwrap_or(0);
            if part == 0 && carry == 0 && i >= first + parts.len() {
                break;
            }
            let (s1, c1) = self.limbs[i].overflowing_add(part);
            let (s2, c2) = s1.overflowing_add(carry);
            self.limbs[i] = s2;
            carry = u64::from(c1 || c2);
        }
    }

    fn sub_parts(&mut self, first: usize, parts: [u64; 3]) {
        let mut borrow = 0u64;
        for i in first..EXACT_LIMBS {
            let part = parts.get(i - first).copied().unwrap_or(0);
            if part == 0 && borrow == 0 && i >= first + parts.len() {
                break;
            }
            let (d1, b1) = self.limbs[i].overflowing_sub(part);
            let (d2, b2) = d1.overflowing_sub(borrow);
            self.limbs[i] = d2;
            borrow = u64::from(b1 || b2);
        }
    }

    fn is_negative(&self) -> bool {
        self.limbs[EXACT_LIMBS - 1] >> 63 == 1
    }

    /// |sum|, as unsigned limbs.
    fn magnitude(&self) -> [u64; EXACT_LIMBS] {
        let mut limbs = self.limbs;
        if self.is_negative() {
            let mut carry = true;
            for limb in &mut limbs {
                let (v, c) = (!*limb).overflowing_add(u64::from(carry));
                *limb = v;
                carry = c;
            }
        }
        limbs
    }

    pub fn round(&self) -> f64 {
        let value = round_magnitude(&self.magnitude(), LSB_SCALE, false);
        if self.is_negative() { -value } else { value }
    }

    pub fn div_round(&self, divisor: i64) -> f64 {
        assert!(divisor > 0, "a mean divides by a positive count");
        let mut limbs = self.magnitude();
        // Two more fractional bits (units of 2^-1076), so the quotient carries
        // the guard bit even for a subnormal result; |sum| < 2^2226 leaves room.
        for i in (1..EXACT_LIMBS).rev() {
            limbs[i] = (limbs[i] << 2) | (limbs[i - 1] >> 62);
        }
        limbs[0] <<= 2;
        let d = u128::from(divisor as u64);
        let mut remainder: u128 = 0;
        for limb in limbs.iter_mut().rev() {
            let current = (remainder << 64) | u128::from(*limb);
            *limb = (current / d) as u64;
            remainder = current % d;
        }
        let value = round_magnitude(&limbs, LSB_SCALE + 2, remainder != 0);
        if self.is_negative() { -value } else { value }
    }

    pub fn to_be_bytes(&self) -> [u8; EXACT_BYTES] {
        let mut out = [0u8; EXACT_BYTES];
        for (chunk, limb) in out.chunks_exact_mut(8).zip(self.limbs.iter().rev()) {
            chunk.copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    pub fn from_be_bytes(bytes: &[u8]) -> Option<ExactSum> {
        if bytes.len() != EXACT_BYTES {
            return None;
        }
        let mut limbs = [0u64; EXACT_LIMBS];
        for (limb, chunk) in limbs.iter_mut().rev().zip(bytes.chunks_exact(8)) {
            *limb = u64::from_be_bytes(chunk.try_into().expect("chunks of 8"));
        }
        Some(ExactSum { limbs })
    }
}

/// `magnitude << offset` (offset < 64) as three limbs, low first.
fn shifted_parts(magnitude: u128, offset: u32) -> [u64; 3] {
    let lo = magnitude as u64;
    let hi = (magnitude >> 64) as u64;
    if offset == 0 {
        return [lo, hi, 0];
    }
    [lo << offset, (lo >> (64 - offset)) | (hi << offset), hi >> (64 - offset)]
}

fn highest_bit(limbs: &[u64; EXACT_LIMBS]) -> Option<u32> {
    limbs
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &l)| l != 0)
        .map(|(i, &l)| i as u32 * 64 + 63 - l.leading_zeros())
}

fn bit(limbs: &[u64; EXACT_LIMBS], pos: u32) -> bool {
    (limbs[(pos / 64) as usize] >> (pos % 64)) & 1 == 1
}

/// The 53 bits starting at `start`.
fn bits53(limbs: &[u64; EXACT_LIMBS], start: u32) -> u64 {
    let (i, off) = ((start / 64) as usize, start % 64);
    let lo = limbs[i] >> off;
    let hi = if off == 0 || i + 1 >= EXACT_LIMBS { 0 } else { limbs[i + 1] << (64 - off) };
    (lo | hi) & ((1 << 53) - 1)
}

/// Whether any bit below `pos` is set.
fn any_below(limbs: &[u64; EXACT_LIMBS], pos: u32) -> bool {
    let (i, off) = ((pos / 64) as usize, pos % 64);
    limbs[..i].iter().any(|&l| l != 0) || (off > 0 && limbs[i] & ((1 << off) - 1) != 0)
}

/// `limbs * 2^-scale` (plus a sticky fraction below it), rounded once to the
/// nearest `f64`, ties to even (M2c spec §4). Non-negative.
fn round_magnitude(limbs: &[u64; EXACT_LIMBS], scale: u32, sticky_in: bool) -> f64 {
    let Some(top) = highest_bit(limbs) else {
        return 0.0;
    };
    // The result's least significant bit: 53 significant bits, never finer
    // than 2^-1074.
    let drop = (i64::from(top) - 52).max(i64::from(scale) - 1074);
    if drop <= 0 {
        // Fewer than 53 bits at a multiple of 2^-1074: exact (scale 1074 only).
        return compose(limbs[0], -1074);
    }
    let drop = drop as u32;
    let mut q = bits53(limbs, drop);
    let guard = bit(limbs, drop - 1);
    let sticky = sticky_in || any_below(limbs, drop - 1);
    let mut exponent = i64::from(drop) - i64::from(scale);
    if guard && (sticky || q & 1 == 1) {
        q += 1;
        if q == 1 << 53 {
            q >>= 1;
            exponent += 1;
        }
    }
    compose(q, exponent)
}

/// `q * 2^exponent` as an `f64`; `q < 2^53`, and `q < 2^52` only at the
/// subnormal exponent -1074.
fn compose(q: u64, exponent: i64) -> f64 {
    if q < 1 << 52 {
        debug_assert_eq!(exponent, -1074, "a short significand is subnormal");
        return f64::from_bits(q);
    }
    let biased = exponent + 52 + 1023;
    if biased >= 0x7ff {
        return f64::INFINITY;
    }
    f64::from_bits(((biased as u64) << 52) | (q & ((1 << 52) - 1)))
}
```

Check the shift bounds by hand once: `add` uses `shift ≤ 2045` (limb 31, three parts end at limb 33); `add_int` uses `shift = 1074` (limb 16); both stay inside 35 limbs.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core --locked exact:: && scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Gate rows** (`## ivmlite-core`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §4/§6: `exact`'s subtraction is exact | In `add_shifted`, call `self.add_parts(first, parts)` on the `negative` branch too | `add_then_sub_is_the_identity`, `cancellation_reaches_exact_zero`, `rounding_matches_an_i128_reference` |
| M2c §4: rounded once, ties to even | `q & 1 == 1` → `true` | `ties_round_to_even` |
| M2c §4: bits below the guard decide | `let sticky = sticky_in \|\| any_below(…)` → `let sticky = sticky_in` | `ties_round_to_even` (the 2^-60 case), `rounding_matches_an_i128_reference` |
| M2c §6: subnormals | `(i64::from(top) - 52).max(…)` → `i64::from(top) - 52` | `subnormal_sums_are_exact`, `a_subnormal_mean_rounds_at_the_smallest_subnormal` |
| M2c §6: ±max finite | Delete `if biased >= 0x7ff { return f64::INFINITY; }` | `max_finite_and_infinity`, `two_pow_60_equal_terms` |
| M2c §4: AVG is exact long division, rounded once | Pass `false` for `sticky_in` in `div_round` | `mean_is_correctly_rounded_against_an_i128_reference` |

  Each prediction is unverified until run: record what really goes red.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-core/src/exact.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-core/src/test_support.rs docs/mutation-gates.md
git commit -m "feat(core): an exact fixed-point superaccumulator for floating-point aggregates

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: How a value counts — SQLite's numeric classification (spec §4 "How a value counts", §5 literal affinity)

**Files:**
- Create: `crates/ivmlite-core/src/numeric.rs`
- Modify: `crates/ivmlite-core/src/lib.rs` (`mod numeric; pub use numeric::{numeric_affinity, numeric_term, NumericTerm};`), `docs/mutation-gates.md`
- Test: `numeric.rs`'s test module; create `crates/ivmlite-test/tests/numeric_terms.rs` (against the bundled SQLite)

**Interfaces:**
- Consumes: Task 1's `Value`.
- Produces:
  ```rust
  /// How SUM/TOTAL/AVG count one non-NULL value (M2c spec §4).
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub enum NumericTerm { Integer(i64), Approx(f64) }
  /// `None` for NULL.
  pub fn numeric_term(value: &Value) -> Option<NumericTerm>;
  /// SQLite's numeric affinity applied to a TEXT value: `Some(Int | Real)`
  /// for a well-formed number, `None` otherwise (M2c spec §5).
  pub fn numeric_affinity(text: &str) -> Option<Value>;
  ```

**The grammar** (plan decision 8 — SQLite's `sqlite3AtoF`, as measured on 3.53): skip leading whitespace (`' '`, `\t`, `\n`, `\x0b`, `\x0c`, `\r`); an optional `+`/`-`; digits; optionally `.` and digits; at least one digit in total, else the value is `0.0` and the text is not well formed; optionally `e`/`E`, an optional sign and **at least one** digit — without a digit the exponent is not consumed; then trailing whitespace. The text is **well formed** when that consumes everything. Its **value** is the consumed prefix (sign, digits, fraction, exponent) converted by Rust's `f64` parser after normalizing it to `<sign><int or 0>.<frac or 0>e<exp or 0>`; Rust's parser never sees `inf`/`nan`, so `'inf'` is `0.0` as in SQLite. A well-formed text with no `.` and no exponent whose digits fit `i64` (sign included, `"-0"` → 0) is an **integer**.
- `numeric_term`: `Int(n)` → `Integer(n)`; `Real(x)` → `Approx(x)`; `Text` integer → `Integer`, else `Approx(value)`; `Blob(b)` → `Approx(value of b read as text)` (SQLite's `sqlite3_value_double` of a BLOB); `Null` → `None`.
- `numeric_affinity`: well formed → `Some(Int)` for an integer, else `Some(Real(value))` (a pure integer that overflows `i64` is `Real`); not well formed → `None`.

- [ ] **Step 1: Write the failing tests.** `numeric.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use NumericTerm::{Approx, Integer};

    fn text(s: &str) -> Option<NumericTerm> {
        numeric_term(&Value::Text(s.into()))
    }

    #[test]
    fn integers_and_reals_count_as_themselves() {
        assert_eq!(numeric_term(&Value::Int(-3)), Some(Integer(-3)));
        assert_eq!(numeric_term(&Value::Real(1.0)), Some(Approx(1.0)));
        assert_eq!(numeric_term(&Value::Null), None);
    }

    #[test]
    fn text_follows_sqlites_measured_classification() {
        // Each pair measured with `SELECT sum(?1), typeof(sum(?1))` on SQLite 3.53.
        assert_eq!(text("12"), Some(Integer(12)));
        assert_eq!(text(" 12 "), Some(Integer(12)));
        assert_eq!(text("+7"), Some(Integer(7)));
        assert_eq!(text("-0"), Some(Integer(0)));
        assert_eq!(text("12abc"), Some(Approx(12.0)));
        assert_eq!(text("abc"), Some(Approx(0.0)));
        assert_eq!(text("1.0"), Some(Approx(1.0)));
        assert_eq!(text("1."), Some(Approx(1.0)));
        assert_eq!(text(".5"), Some(Approx(0.5)));
        assert_eq!(text("1e2"), Some(Approx(100.0)));
        assert_eq!(text("1e"), Some(Approx(1.0)));
        assert_eq!(text(" 0x10"), Some(Approx(0.0)));
        assert_eq!(text("inf"), Some(Approx(0.0)));
        assert_eq!(text("1e400"), Some(Approx(f64::INFINITY)));
        assert_eq!(text("9223372036854775808"), Some(Approx(9_223_372_036_854_775_808.0)));
        assert_eq!(text("-9223372036854775808"), Some(Integer(i64::MIN)));
        assert_eq!(text(""), Some(Approx(0.0)));
    }

    #[test]
    fn a_blob_counts_as_its_bytes_read_as_text_and_is_approximate() {
        assert_eq!(numeric_term(&Value::Blob(b"12".to_vec())), Some(Approx(12.0)));
        assert_eq!(numeric_term(&Value::Blob(vec![0xff])), Some(Approx(0.0)));
    }

    #[test]
    fn numeric_affinity_converts_only_well_formed_numbers() {
        assert_eq!(numeric_affinity(" 3 "), Some(Value::Int(3)));
        assert_eq!(numeric_affinity("3.0"), Some(Value::Real(3.0)));
        assert_eq!(numeric_affinity("2.5e1"), Some(Value::Real(25.0)));
        assert_eq!(numeric_affinity("3x"), None);
        assert_eq!(numeric_affinity("abc"), None);
    }
}
```

`crates/ivmlite-test/tests/numeric_terms.rs` (pins the Rust grammar against the host SQLite; plain `rusqlite`, no extension):

```rust
//! M2c spec §4: `numeric_term` classifies a value exactly as SQLite's `sum`
//! does, on the SQLite the tests link (plan decision 8).

use ivmlite_core::{numeric_term, NumericTerm, Value};
use rusqlite::types::Value as Sql;
use rusqlite::Connection;

const CORPUS: &[&str] = &[
    "0", "12", " 12 ", "\t12\n", "+7", "-0", "007", "12abc", "abc", "", " ", "1.0", "1.", ".5", "-.5",
    "1e2", "1E+2", "1e-2", "1e", "1e+", " 0x10", "inf", "nan", "1e400", "-1e400",
    "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809",
    "123456789012345", "0.1", "2.5", "-2.5e-3", "1 2", "١٢",
];

fn sqlite_sum(c: &Connection, v: &Sql) -> Sql {
    c.query_row("SELECT sum(?1)", [v], |r| r.get(0)).unwrap()
}

#[test]
fn text_and_blob_terms_match_sqlite_sum() {
    let c = Connection::open_in_memory().unwrap();
    for &s in CORPUS {
        for (value, sql) in [
            (Value::Text(s.into()), Sql::Text(s.into())),
            (Value::Blob(s.as_bytes().to_vec()), Sql::Blob(s.as_bytes().to_vec())),
        ] {
            let want = sqlite_sum(&c, &sql);
            let got = numeric_term(&value).expect("non-NULL");
            match (got, &want) {
                (NumericTerm::Integer(n), Sql::Integer(m)) => assert_eq!(n, *m, "{value:?}"),
                (NumericTerm::Approx(x), Sql::Real(y)) => assert_eq!(x.to_bits(), y.to_bits(), "{value:?}"),
                other => panic!("{value:?}: ivmlite {:?}, SQLite {want:?}", other.0),
            }
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked numeric:: 2>&1 | tail -3`
Expected: FAIL to compile — `numeric_term` not found.

- [ ] **Step 3: Implement `numeric.rs`.**

```rust
//! M2c spec §4 "How a value counts": SQLite's `sqlite3_value_numeric_type`
//! for TEXT (numeric affinity without changing what is stored) and
//! `sqlite3_value_double` for TEXT and BLOB, after `sqlite3AtoF`'s grammar
//! (M2c plan decision 8).

use crate::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericTerm {
    Integer(i64),
    Approx(f64),
}

/// What `sqlite3AtoF` reads from `bytes`.
struct Scan {
    /// The consumed prefix's value.
    value: f64,
    /// Every byte consumed, up to trailing whitespace.
    well_formed: bool,
    /// Well formed, no `.`, no exponent, and the digits fit `i64`.
    integer: Option<i64>,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The run of ASCII digits at `*i`, advancing `*i` past it.
fn digits<'a>(bytes: &'a [u8], i: &mut usize) -> &'a [u8] {
    let start = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    &bytes[start..*i]
}

fn scan(bytes: &[u8]) -> Scan {
    let mut i = 0;
    while i < bytes.len() && is_space(bytes[i]) {
        i += 1;
    }
    let negative = match bytes.get(i) {
        Some(b'-') => { i += 1; true }
        Some(b'+') => { i += 1; false }
        _ => false,
    };
    let whole = digits(bytes, &mut i);
    let mut fraction: &[u8] = &[];
    let mut dot = false;
    if bytes.get(i) == Some(&b'.') {
        dot = true;
        i += 1;
        fraction = digits(bytes, &mut i);
    }
    if whole.is_empty() && fraction.is_empty() {
        return Scan { value: 0.0, well_formed: false, integer: None };
    }
    let mut exponent: &[u8] = &[];
    let mut exponent_negative = false;
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        let sign = match bytes.get(j) {
            Some(b'-') => { j += 1; true }
            Some(b'+') => { j += 1; false }
            _ => false,
        };
        let e = digits(bytes, &mut j);
        if !e.is_empty() {
            exponent = e;
            exponent_negative = sign;
            i = j;
        }
    }
    let mut end = i;
    while end < bytes.len() && is_space(bytes[end]) {
        end += 1;
    }
    let well_formed = end == bytes.len();
    let ascii = |d: &[u8]| std::str::from_utf8(d).expect("ASCII digits").to_string();
    let normalized = format!(
        "{}{}.{}e{}{}",
        if negative { "-" } else { "" },
        if whole.is_empty() { "0".into() } else { ascii(whole) },
        if fraction.is_empty() { "0".into() } else { ascii(fraction) },
        if exponent_negative { "-" } else { "" },
        if exponent.is_empty() { "0".into() } else { ascii(exponent) },
    );
    let value: f64 = normalized.parse().expect("the normalized form always parses");
    let integer = (well_formed && !dot && exponent.is_empty())
        .then(|| format!("{}{}", if negative { "-" } else { "" }, ascii(whole)).parse::<i64>().ok())
        .flatten();
    Scan { value, well_formed, integer }
}

pub fn numeric_term(value: &Value) -> Option<NumericTerm> {
    Some(match value {
        Value::Null => return None,
        Value::Int(n) => NumericTerm::Integer(*n),
        Value::Real(x) => NumericTerm::Approx(*x),
        Value::Text(s) => {
            let s = scan(s.as_bytes());
            match s.integer {
                Some(n) => NumericTerm::Integer(n),
                None => NumericTerm::Approx(s.value),
            }
        }
        Value::Blob(b) => NumericTerm::Approx(scan(b).value),
    })
}

pub fn numeric_affinity(text: &str) -> Option<Value> {
    let s = scan(text.as_bytes());
    if !s.well_formed {
        return None;
    }
    Some(match s.integer {
        Some(n) => Value::Int(n),
        None => Value::Real(s.value),
    })
}
```

(`-0` with no `.`: the integer branch parses `"-0"` as 0, matching SQLite's integer 0. A `-` before an `i64::MIN` magnitude parses directly, so `"-9223372036854775808"` is `Integer(i64::MIN)`.)

If `text_and_blob_terms_match_sqlite_sum` disagrees with SQLite on a corpus entry, SQLite is the authority: fix the grammar, and add the entry to the core unit test with the measured result. Do not delete corpus entries.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core --locked numeric:: && cargo test -p ivmlite-test --locked --test numeric_terms`
Expected: PASS.

- [ ] **Step 5: Gate rows** (`## ivmlite-core`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §4: a TEXT that converts to an integer adds its integer value | In `numeric_term`'s `Text` arm, always return `Approx(s.value)` | `text_follows_sqlites_measured_classification`, `text_and_blob_terms_match_sqlite_sum` |
| M2c §4: a TEXT with trailing junk is approximate | `let well_formed = end == bytes.len();` → `true` | both tests above |
| M2c §4: whitespace around a number is allowed | Delete the trailing-whitespace loop | both tests above (`" 12 "`) |
| M2c §4: an integer that overflows `i64` is approximate | Replace `.parse::<i64>().ok()` with `.parse::<i64>().ok().or(Some(i64::MAX))` | both tests above |
| M2c §4: a BLOB is always approximate | `Value::Blob(b) => match scan(b).integer { Some(n) => Integer(n), None => Approx(scan(b).value) }` | `a_blob_counts_as_its_bytes_read_as_text_and_is_approximate`, `text_and_blob_terms_match_sqlite_sum` |

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-core/src/numeric.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-test/tests/numeric_terms.rs docs/mutation-gates.md
git commit -m "feat(core): classify TEXT and BLOB values the way SQLite's sum does

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 5: Group keys are canonicalized, and remember whether they were REAL (spec §2 "Group equality", "The type a group key is output as")

**Files:**
- Modify: `crates/ivmlite-core/src/agg.rs` — `GroupState` (`:18-120`), `AggState::absorb` (`:160-216`), and the tests' stored-value fixtures (`:632-681`)
- Modify: `docs/mutation-gates.md`
- Test: `agg.rs`'s test module

**Interfaces:**
- Consumes: Task 1's `Value::canonical`.
- Produces: no new public API. The stored group row becomes `[Int(rows), real_keys, <per-agg values as before: Int(sum), Int(non_null)>]` where `real_keys` is `Value::Null` when every count is 0, else `Value::Blob` of one big-endian `i64` per group-by column (plan decision 6). `GroupState` gains `real_keys: Vec<i64>` (one per group-by column, by weight). The arrangement is keyed by the **canonical** key; the emitted key column `j` is `Real(i as f64)` when its canonical value is `Int(i)` and `real_keys[j] > 0`, else the canonical value.

The count is kept for every group-by column (plan decision: no affinity is known in core, and a TEXT column's count simply stays 0). A non-integral `Real` key is never canonicalized, so it is emitted as `Real` regardless of the count.

- [ ] **Step 1: Write the failing tests** (in `agg.rs`'s test module):

```rust
fn real(x: f64) -> Value {
    Value::Real(x)
}

#[test]
fn integral_reals_and_ints_share_a_group() {
    // Spec §2: 1 and 1.0 are one group; -0.0 and 0 are one group; a group
    // with a REAL key row outputs its key as REAL.
    let mut s = count_state();
    let d = s
        .absorb(&ZSet::from_rows([
            (row(vec![int(1), int(0)]), 1),
            (row(vec![real(1.0), int(0)]), 1),
            (row(vec![real(-0.0), int(0)]), 1),
            (row(vec![int(0), int(0)]), 1),
            (row(vec![real(1.5), int(0)]), 1),
            (row(vec![txt("1"), int(0)]), 1),
        ]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![real(1.0), int(2)]), 1),
            (row(vec![real(0.0), int(2)]), 1),
            (row(vec![real(1.5), int(1)]), 1),
            (row(vec![txt("1"), int(1)]), 1),
        ])
    );
}

#[test]
fn a_group_without_real_key_rows_outputs_an_int_key() {
    let mut s = count_state();
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![int(1), int(0)]), 2)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]));
}

#[test]
fn the_key_turns_back_to_int_when_its_last_real_row_leaves() {
    // The count follows weights: two REAL rows in, one out, still REAL.
    let mut s = count_state();
    s.absorb(&ZSet::from_rows([
        (row(vec![int(1), int(0)]), 1),
        (row(vec![real(1.0), int(0)]), 2),
    ]))
    .unwrap();
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![real(1.0), int(0)]), -1)]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![real(1.0), int(3)]), -1),
            (row(vec![real(1.0), int(2)]), 1),
        ])
    );
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![real(1.0), int(0)]), -1)]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![real(1.0), int(2)]), -1),
            (row(vec![int(1), int(1)]), 1),
        ])
    );
}

#[test]
fn the_stored_state_counts_real_keys_and_omits_them_when_zero() {
    let mirrors = Mirrors::default();
    let mut s = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
    s.absorb(&ZSet::from_rows([(row(vec![int(1), int(5)]), 1)]))
        .unwrap();
    let stored = mirrors.snapshot(groups_id()).scan().unwrap();
    assert_eq!(stored[0].1, row(vec![int(1), Value::Null, int(5), int(1)]));
    s.absorb(&ZSet::from_rows([(row(vec![real(1.0), int(5)]), 1)]))
        .unwrap();
    let stored = mirrors.snapshot(groups_id()).scan().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, row(vec![int(1)]), "keyed by the canonical key");
    assert_eq!(
        stored[0].1,
        row(vec![int(2), Value::Blob(1i64.to_be_bytes().to_vec()), int(10), int(2)])
    );
}

#[test]
fn a_state_rebuilt_from_its_arrangement_keeps_its_real_keys() {
    let mirrors = Mirrors::default();
    let mut old = sum_state_on(mirrors.arrangement(groups_id()).unwrap());
    old.absorb(&ZSet::from_rows([(row(vec![real(2.0), int(1)]), 1)]))
        .unwrap();
    let mut rebuilt = sum_state_on(Box::new(mirrors.snapshot(groups_id())));
    let d = rebuilt
        .absorb(&ZSet::from_rows([(row(vec![int(2), int(1)]), 1)]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![real(2.0), int(1)]), -1),
            (row(vec![real(2.0), int(2)]), 1),
        ])
    );
}
```

Update the three corrupted-state tests' fixtures to the new layout: a valid value is `row(vec![int(1), Value::Null, int(5), int(1)])`; the "wrong shape" list becomes `row(vec![int(1), Value::Null, int(5)])`, `row(vec![int(1), Value::Null, txt("x"), int(1)])` and the new `row(vec![int(1), Value::Blob(vec![1, 2, 3]), int(5), int(1)])` (a `real_keys` BLOB of the wrong length). All still expect `corrupted aggregate state`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked agg:: 2>&1 | tail -5`
Expected: FAIL — `integral_reals_and_ints_share_a_group` (left has separate `Int(1)` and `Real(1.0)` groups), and the stored-layout tests.

- [ ] **Step 3: Implement.**
  - `GroupState { rows: i64, real_keys: Vec<i64>, accs: Vec<Acc> }`; `empty(keys, aggs)`; `is_empty` also requires every `real_keys` entry 0; `plus` adds `real_keys` pointwise.
  - `encode`: push `Int(rows)`, then `real_keys_value(&self.real_keys)`, then the accumulators as today.
    ```rust
    /// Plan decision 6: NULL when no key row was REAL, so integer workloads pay one byte.
    fn real_keys_value(counts: &[i64]) -> Value {
        if counts.iter().all(|&c| c == 0) {
            Value::Null
        } else {
            Value::Blob(counts.iter().flat_map(|c| c.to_be_bytes()).collect())
        }
    }
    ```
  - `decode(row, keys, aggs)`: length `2 + 2 * aggs`; value 1 is `Null` → `vec![0; keys]`, or a `Blob` of exactly `8 * keys` bytes that is not all zero → the counts; anything else is `StateError("corrupted aggregate state: the REAL key counts are {other:?}")`.
  - `output`: the key columns become
    ```rust
    let mut values: Vec<Value> = key
        .0
        .iter()
        .zip(&self.real_keys)
        .map(|(value, &reals)| match value {
            // M2c spec §2: SQLite outputs an arbitrary row's key; a group with
            // any REAL key row outputs REAL.
            Value::Int(i) if reals > 0 => Value::Real(*i as f64),
            other => other.clone(),
        })
        .collect();
    ```
  - `absorb`: the key is `Row::new(self.group_by.iter().map(|&c| row.get(c).canonical()).collect())`, and after `d.rows += w`:
    ```rust
    for (j, &c) in self.group_by.iter().enumerate() {
        if matches!(row.get(c), Value::Real(_)) {
            d.real_keys[j] += w;
        }
    }
    ```
  - `load`/`decode` callers pass `self.group_by.len()`.
  - Update `AggState`'s doc comment: the arrangement is keyed by the canonical group key (M2c spec §2).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core --locked && scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS (the extension reads and writes the new layout through the same code; no extension test sees a REAL key yet).

- [ ] **Step 5: Gate rows** (`## ivmlite-core`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §2: group keys are canonicalized ("dropping the canonicalization") | In `absorb`, `row.get(c).canonical()` → `row.get(c).clone()` | `integral_reals_and_ints_share_a_group`, `the_key_turns_back_to_int_when_its_last_real_row_leaves` |
| M2c §2: the real-key count decides the output type ("dropping the real-key count") | In `output`, delete the `Value::Int(i) if reals > 0` arm | `integral_reals_and_ints_share_a_group`, `a_state_rebuilt_from_its_arrangement_keeps_its_real_keys` |
| M2c §2: the count follows weights | `d.real_keys[j] += w` → `d.real_keys[j] += 1` | `the_key_turns_back_to_int_when_its_last_real_row_leaves` |
| M2c §2: the count is persisted | In `decode`, always return `vec![0; keys]` for `real_keys` | `a_state_rebuilt_from_its_arrangement_keeps_its_real_keys` |

  Task 7 re-runs the first two rows once the extension can see REAL keys and appends its red tests.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-core/src/agg.rs docs/mutation-gates.md
git commit -m "feat(core): group by SQLite's equality and output a REAL key when a row had one

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: SUM, TOTAL, AVG and COUNT(column), exactly (spec §4 "Accumulator state", "Outputs", "Where this differs")

**Files:**
- Modify: `crates/ivmlite-core/src/query.rs:3-16` (`AggFn`, `Agg` doc), `crates/ivmlite-core/src/schema.rs:1-6` (`ColumnType`), `crates/ivmlite-core/src/agg.rs` (`Acc`, `GroupState`, `absorb`), `crates/ivmlite-core/src/plan.rs` (`write_canonical`'s aggregate arm `:122-131`; `lower`'s column check `:300-312` and `require_integer` `:341-366`), `crates/ivmlite-core/src/lib.rs` (export `SUM_OVERFLOW`)
- Modify: `crates/ivmlite-sql/src/compile.rs` — `Named::column` (`:475-490`), `aggregate` (`:543-606`), the `compile` doc comment (`:66-83`), tests `lower_s_checks_apply_to_sql` (`:1082-1107`) and `everything_outside_the_subset_is_rejected_by_name` (`:1109-…`)
- Modify (for the new `ColumnType` variants only): `crates/ivmlite-sqlite/src/view.rs` (`shape`'s type closure), `crates/ivmlite-test/src/sql.rs:10-15`, `crates/ivmlite-test/src/data.rs:44-47`, `crates/ivmlite-test/src/query.rs:146-156`
- Modify: `docs/mutation-gates.md`
- Test: `agg.rs`, `plan.rs`, `compile.rs` test modules; `crates/ivmlite-test/tests/extension_types.rs`

**Interfaces:**
- Consumes: Task 3's `ExactSum`, Task 4's `numeric_term`/`NumericTerm`, Task 5's group state.
- Produces:
  ```rust
  pub enum AggFn { Count, Sum, Total, Avg }        // Task 8 adds Min, Max
  pub enum ColumnType { Integer, Real, Numeric, Text, Blob }  // M2c: a column's affinity
  impl ColumnType { pub fn name(self) -> &'static str }       // "INTEGER" "REAL" "NUMERIC" "TEXT" "BLOB"
  pub const SUM_OVERFLOW: &str = "integer overflow in SUM";
  ```
  `Agg { func: Count, column: Some(c) }` is `COUNT(c)`; every other function requires `Some`.
  Stored per-aggregate values (plan decision 6): `COUNT(*)` none; `COUNT(x)` `[Int(non_null)]`; `SUM`/`TOTAL`/`AVG` `[Int(non_null), int_sum, approx]`.
  `CompiledView::columns[i].ty` for an aggregate (plan decision 10): `COUNT` → `Integer`; `SUM` → `Numeric`; `TOTAL`, `AVG` → `Real`. `nullable`: `COUNT` and `TOTAL` false, `SUM` and `AVG` true.

The new `ColumnType` variants appear here because output columns need them; the catalog still produces only `Integer`/`Text` until Task 7. In `view.rs`'s `shape`, Task 2's type closure becomes `c.ty.name()` (the same text for INTEGER and TEXT, so no recorded shape changes). In the test crate, `sql_type` renders `REAL`, `NUMERIC` and the empty declared type for `Blob` (rename it `declared_type`), `gen_row` and `literal` get arms that `unreachable!("no generated database declares {ty:?} affinity before M2c Task 11")`; Task 11 replaces them.

- [ ] **Step 1: Write the failing tests.** Core (`agg.rs` tests):

```rust
fn agg(func: AggFn, column: usize) -> Agg {
    Agg { func, column: Some(column) }
}

fn state_with(aggs: Vec<Agg>) -> AggState {
    AggState::new(vec![0], aggs, Box::new(MemArrangement::new()))
}

#[test]
fn sum_of_integers_stays_integer_and_a_real_makes_it_real() {
    let mut s = state_with(vec![agg(AggFn::Sum, 1), agg(AggFn::Total, 1), agg(AggFn::Avg, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1), (row(vec![txt("a"), int(2)]), 1)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(3), real(3.0), real(1.5)]), 1)]));
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), 1)])).unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![txt("a"), int(3), real(3.0), real(1.5)]), -1),
            (row(vec![txt("a"), real(3.5), real(3.5), real(3.5 / 3.0)]), 1),
        ])
    );
    // Retracting the only REAL input switches back to INTEGER (the `approx` switch).
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), -1)])).unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![txt("a"), real(3.5), real(3.5), real(3.5 / 3.0)]), -1),
            (row(vec![txt("a"), int(3), real(3.0), real(1.5)]), 1),
        ])
    );
}

#[test]
fn sum_outside_i64_is_an_error_and_total_and_avg_are_not() {
    let mut s = state_with(vec![agg(AggFn::Total, 1), agg(AggFn::Avg, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 1), (row(vec![txt("a"), int(1)]), 1)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), real(9_223_372_036_854_775_808.0), real(4_611_686_018_427_387_904.0)]), 1)]));
    let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
    let e = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 1), (row(vec![txt("a"), int(1)]), 1)]))
        .expect_err("spec §4: the final exact sum is outside i64");
    assert_eq!(e.0, SUM_OVERFLOW);
}

#[test]
fn only_the_final_sum_is_judged_for_overflow() {
    // Spec §4: SQLite fails on this prefix; ivmlite judges the final sum.
    let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(i64::MAX)]), 1),
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), int(-1)]), 1),
        ]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 1)]));
}

#[test]
fn count_of_a_column_skips_nulls_and_follows_weights() {
    let mut s = state_with(vec![agg(AggFn::Count, 1), Agg { func: AggFn::Count, column: None }]);
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 3), (row(vec![txt("a"), Value::Null]), 2)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(3), int(5)]), 1)]));
}

#[test]
fn avg_divides_by_the_non_null_count() {
    let mut s = state_with(vec![agg(AggFn::Avg, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
            (row(vec![txt("a"), int(2)]), 1),
        ]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), real(1.5)]), 1)]));
}

#[test]
fn avg_is_the_exact_mean_rounded_once() {
    // 0.1 + 0.2 + 0.3, exactly, divided by 3, is closer to 0.2 than to
    // 0.19999999999999998, which is what rounding the sum first gives.
    let mut s = state_with(vec![agg(AggFn::Avg, 1), agg(AggFn::Sum, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([
            (row(vec![txt("a"), real(0.1)]), 1),
            (row(vec![txt("a"), real(0.2)]), 1),
            (row(vec![txt("a"), real(0.3)]), 1),
        ]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), real(0.2), real(0.6)]), 1)]));
}

#[test]
fn nothing_non_null_gives_null_sum_and_avg_and_zero_total() {
    let mut s = state_with(vec![agg(AggFn::Sum, 1), agg(AggFn::Total, 1), agg(AggFn::Avg, 1), agg(AggFn::Count, 1)]);
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)])).unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), Value::Null, real(0.0), Value::Null, int(0)]), 1)]));
}

#[test]
fn opposite_infinities_sum_to_null() {
    // Plan decision 5: +Inf + -Inf is NaN, which SQLite returns as NULL.
    let mut s = state_with(vec![agg(AggFn::Sum, 1), agg(AggFn::Avg, 1)]);
    s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(f64::INFINITY)]), 1), (row(vec![txt("a"), real(1.0)]), 1)]))
        .unwrap();
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(f64::NEG_INFINITY)]), 1)])).unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![txt("a"), real(f64::INFINITY), real(f64::INFINITY)]), -1),
            (row(vec![txt("a"), Value::Null, Value::Null]), 1),
        ])
    );
}

#[test]
fn text_that_reads_as_an_integer_sums_as_an_integer() {
    let mut s = state_with(vec![agg(AggFn::Sum, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), txt("12")]), 1), (row(vec![txt("a"), txt(" 3 ")]), 1)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(15)]), 1)]));
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), txt("abc")]), 1)])).unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([(row(vec![txt("a"), int(15)]), -1), (row(vec![txt("a"), real(15.0)]), 1)])
    );
}

#[test]
fn the_state_forgets_frac_when_approx_returns_to_zero() {
    let mirrors = Mirrors::default();
    let mut s = AggState::new(vec![0], vec![agg(AggFn::Sum, 1)], mirrors.arrangement(groups_id()).unwrap());
    s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1), (row(vec![txt("a"), real(0.5)]), 1)]))
        .unwrap();
    let stored = mirrors.snapshot(groups_id()).scan().unwrap();
    assert!(matches!(stored[0].1.get(4), Value::Blob(b) if b.len() == 304), "{stored:?}");
    s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(0.5)]), -1)])).unwrap();
    let stored = mirrors.snapshot(groups_id()).scan().unwrap();
    // [rows, real_keys, non_null, int_sum, approx]
    assert_eq!(stored[0].1, row(vec![int(1), Value::Null, int(1), int(1), Value::Null]));
}

#[test]
fn the_stored_state_does_not_depend_on_the_order_of_inputs() {
    let values = [0.1, 1e300, -1e300, 3.25, 7e-310];
    let stored = |order: &[usize]| {
        let mirrors = Mirrors::default();
        let mut s = AggState::new(vec![0], vec![agg(AggFn::Sum, 1)], mirrors.arrangement(groups_id()).unwrap());
        for &i in order {
            s.absorb(&ZSet::from_rows([(row(vec![txt("a"), real(values[i])]), 1)])).unwrap();
        }
        mirrors.snapshot(groups_id()).scan().unwrap()
    };
    assert_eq!(stored(&[0, 1, 2, 3, 4]), stored(&[4, 2, 0, 3, 1]));
}

#[test]
fn an_int_sum_outside_i64_is_stored_as_sixteen_bytes() {
    let mirrors = Mirrors::default();
    let mut s = AggState::new(vec![0], vec![agg(AggFn::Total, 1)], mirrors.arrangement(groups_id()).unwrap());
    s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(i64::MAX)]), 2)])).unwrap();
    let stored = mirrors.snapshot(groups_id()).scan().unwrap();
    let want = (i128::from(i64::MAX) * 2).to_be_bytes().to_vec();
    assert_eq!(stored[0].1.get(3), &Value::Blob(want));
}
```

Rewrite the stored-value fixtures of the corrupted-state tests for the SUM layout `[rows, real_keys, non_null, int_sum, approx]`: valid is `row(vec![int(1), Value::Null, int(1), int(5), Value::Null])`; the wrong shapes are `row(vec![int(1), Value::Null, int(1), int(5)])`, `row(vec![int(1), Value::Null, txt("x"), int(5), Value::Null])`, `row(vec![int(1), Value::Null, int(1), Value::Blob(5i128.to_be_bytes().to_vec()), Value::Null])` (a 16-byte `int_sum` that fits `i64` is not canonical) and `row(vec![int(1), Value::Null, int(1), int(5), Value::Blob(vec![0; 304])])` (an `approx` whose count is 0). Existing tests asserting `SUM` outputs keep their meaning; update only the stored layouts they inspect.

`plan.rs` tests: rename `sum_over_a_text_column_is_rejected_at_the_boundary` to `sum_total_and_avg_accept_any_column` and assert `lower` accepts `SUM`, `TOTAL`, `AVG` and `COUNT(c)` over the TEXT column; add `every_aggregate_but_count_needs_a_column` (`Agg { func: Total, column: None }` and `Avg` are rejected with `names no column`); add `the_canonical_text_names_each_aggregate` asserting `aggregate([0],[count,count(1),sum(1),total(1),avg(1)],…` for a plan with those five.

`compile.rs` tests: in `everything_outside_the_subset_is_rejected_by_name` delete the `COUNT(amount)` and `AVG(amount)` entries; keep `MIN(amount)` (still rejected with a message containing `MIN`). In `lower_s_checks_apply_to_sql` delete the `SUM(region)` rejection. Add:

```rust
#[test]
fn the_new_aggregates_compile_with_sqlites_names_and_types() {
    let v = ok("SELECT region, COUNT(amount), SUM(region), TOTAL(amount), avg(amount) FROM orders GROUP BY region");
    let got: Vec<(String, ColumnType, bool)> =
        v.columns.iter().map(|c| (c.name.clone(), c.ty, c.nullable)).collect();
    assert_eq!(
        got,
        vec![
            ("region".to_string(), ColumnType::Text, true),
            ("COUNT(amount)".to_string(), ColumnType::Integer, false),
            ("SUM(region)".to_string(), ColumnType::Numeric, true),
            ("TOTAL(amount)".to_string(), ColumnType::Real, false),
            ("avg(amount)".to_string(), ColumnType::Real, true),
        ]
    );
}
```

Extension (`extension_types.rs`), STRICT tables (the catalog widens in Task 7):

```rust
use rusqlite::types::Value as Sql;

fn one_row(c: &Connection, sql: &str) -> Vec<Sql> {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    s.query_row([], |r| (0..n).map(|i| r.get::<_, Sql>(i)).collect()).unwrap()
}

#[test]
fn sum_total_avg_and_count_over_integers_and_text() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER, s TEXT) STRICT;
         INSERT INTO t VALUES ('a', 1, '1.5'), ('a', 2, 'abc'), ('a', NULL, NULL), ('b', NULL, '12');",
    )
    .unwrap();
    let sql = "SELECT k, SUM(i), TOTAL(i), AVG(i), COUNT(i), SUM(s), TOTAL(s), AVG(s), COUNT(s), COUNT(*) \
               FROM t GROUP BY k";
    create(&c, "v", sql).unwrap();
    // Every sum here is exact in f64, so SQLite's own result is the exact one.
    assert_matches_oracle(&c, "v", sql);
    c.execute_batch("DELETE FROM t WHERE s = '1.5'; INSERT INTO t VALUES ('b', 4, '0.25');").unwrap();
    refresh(&c, "v").unwrap();
    assert_matches_oracle(&c, "v", sql);
}

#[test]
fn avg_differs_from_sqlite_only_by_rounding_the_exact_mean_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, s TEXT) STRICT;
         INSERT INTO t VALUES ('a', '0.1'), ('a', '0.2'), ('a', '0.3');",
    )
    .unwrap();
    create(&c, "v", "SELECT k, AVG(s), SUM(s) FROM t GROUP BY k").unwrap();
    assert_eq!(one_row(&c, "SELECT * FROM v"), vec![Sql::Text("a".into()), Sql::Real(0.2), Sql::Real(0.6)]);
    // SQLite rounds the sum, then divides (spec §4, "Where this differs").
    assert_eq!(one_row(&c, "SELECT avg(s) FROM t"), vec![Sql::Real(0.19999999999999998)]);
}

#[test]
fn sum_overflow_fails_the_refresh_with_its_message_and_changes_nothing() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER) STRICT;
         INSERT INTO t VALUES ('a', 9223372036854775807);",
    )
    .unwrap();
    create(&c, "v", "SELECT k, SUM(i), TOTAL(i) FROM t GROUP BY k").unwrap();
    c.execute_batch("INSERT INTO t VALUES ('a', 1)").unwrap();
    let e = refresh(&c, "v").expect_err("the final exact sum is outside i64");
    assert!(e.to_string().contains("integer overflow in SUM"), "{e}");
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![Sql::Text("a".into()), Sql::Integer(i64::MAX), Sql::Real(9_223_372_036_854_775_807.0)]
    );
    // Back in range: SQLite's own `sum` would fail on this prefix; ivmlite judges the final sum.
    c.execute_batch("INSERT INTO t VALUES ('a', -1)").unwrap();
    refresh(&c, "v").unwrap();
    assert_eq!(
        one_row(&c, "SELECT * FROM v"),
        vec![Sql::Text("a".into()), Sql::Integer(i64::MAX), Sql::Real(9_223_372_036_854_775_807.0)]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked agg:: 2>&1 | tail -3`
Expected: FAIL to compile — `no variant Total in AggFn`.

- [ ] **Step 3: Implement the accumulator** in `agg.rs` (replacing `Acc`):

```rust
use crate::{numeric_term, ExactSum, NumericTerm, EXACT_BYTES};

/// M2c spec §4: the refresh fails with this when a SUM over integers only
/// ends outside `i64`.
pub const SUM_OVERFLOW: &str = "integer overflow in SUM";

/// The bytes of a stored `Approx`: count, +Inf count, -Inf count, then the limbs.
const APPROX_BYTES: usize = 24 + EXACT_BYTES;

/// One SUM/TOTAL/AVG argument's non-integer inputs (M2c spec §4), present
/// only while `count != 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Approx {
    /// The spec's `approx`: inputs classified as non-integer, by weight.
    count: i64,
    /// ±Inf inputs, by weight; not in `frac` (plan decision 5).
    pos_inf: i64,
    neg_inf: i64,
    /// The spec's `frac`: the exact sum of the finite non-integer inputs.
    frac: ExactSum,
}

impl Approx {
    fn zero() -> Approx {
        Approx { count: 0, pos_inf: 0, neg_inf: 0, frac: ExactSum::zero() }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(APPROX_BYTES);
        for n in [self.count, self.pos_inf, self.neg_inf] {
            out.extend_from_slice(&n.to_be_bytes());
        }
        out.extend_from_slice(&self.frac.to_be_bytes());
        out
    }

    fn from_bytes(bytes: &[u8]) -> Option<Approx> {
        if bytes.len() != APPROX_BYTES {
            return None;
        }
        let int = |i: usize| i64::from_be_bytes(bytes[8 * i..8 * i + 8].try_into().expect("8 bytes"));
        Some(Approx { count: int(0), pos_inf: int(1), neg_inf: int(2), frac: ExactSum::from_be_bytes(&bytes[24..])? })
    }
}

/// One aggregate's accumulator. `COUNT(*)` uses none (it is the group's
/// `rows`); `COUNT(x)` uses only `non_null`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    non_null: i64,
    /// The spec's `int_sum`: inputs classified as integers, exactly.
    int_sum: i128,
    approx: Option<Approx>,
}

fn accumulator_overflow() -> StateError {
    StateError("aggregate accumulator overflow: the exact integer sum left the i128 range".into())
}

impl Acc {
    /// Fold one input of weight `w` into a delta.
    fn absorb_term(&mut self, value: &Value, w: i64) -> Result<(), StateError> {
        let Some(term) = numeric_term(value) else {
            return Ok(());
        };
        self.non_null += w;
        match term {
            NumericTerm::Integer(n) => {
                self.int_sum = self
                    .int_sum
                    .checked_add(i128::from(n) * i128::from(w))
                    .ok_or_else(accumulator_overflow)?;
            }
            NumericTerm::Approx(x) => {
                let a = self.approx.get_or_insert_with(Approx::zero);
                a.count += w;
                if x == f64::INFINITY {
                    a.pos_inf += w;
                } else if x == f64::NEG_INFINITY {
                    a.neg_inf += w;
                } else {
                    a.frac.add(x, w);
                }
            }
        }
        Ok(())
    }

    fn plus(&self, d: &Acc) -> Result<Acc, StateError> {
        let int_sum = self.int_sum.checked_add(d.int_sum).ok_or_else(accumulator_overflow)?;
        let approx = match (&self.approx, &d.approx) {
            (None, None) => None,
            (a, b) => {
                let mut sum = a.clone().unwrap_or_else(Approx::zero);
                if let Some(b) = b {
                    sum.count += b.count;
                    sum.pos_inf += b.pos_inf;
                    sum.neg_inf += b.neg_inf;
                    sum.frac.add_sum(&b.frac);
                }
                // M2c spec §4: with every non-integer input retracted, frac is
                // exactly zero and is dropped.
                if sum.count == 0 {
                    if sum.pos_inf != 0 || sum.neg_inf != 0 || !sum.frac.is_zero() {
                        return Err(StateError(
                            "corrupted aggregate state: no non-integer input is live, but their sum is not zero".into(),
                        ));
                    }
                    None
                } else {
                    Some(sum)
                }
            }
        };
        Ok(Acc { non_null: self.non_null + d.non_null, int_sum, approx })
    }

    /// The REAL result of `finish` over the exact sum, after the infinities.
    fn approx_value(&self, finish: impl Fn(&ExactSum) -> f64) -> Value {
        let (pos, neg) = self.approx.as_ref().map_or((0, 0), |a| (a.pos_inf, a.neg_inf));
        match (pos > 0, neg > 0) {
            // +Inf + -Inf is NaN, which SQLite returns as NULL.
            (true, true) => Value::Null,
            (true, false) => Value::Real(f64::INFINITY),
            (false, true) => Value::Real(f64::NEG_INFINITY),
            (false, false) => {
                let mut exact = self.approx.as_ref().map_or_else(ExactSum::zero, |a| a.frac.clone());
                exact.add_int(self.int_sum);
                Value::Real(finish(&exact))
            }
        }
    }
}
```

Then:
- `GroupState::plus` returns `Result<GroupState, StateError>` and calls `Acc::plus`; `absorb` uses `old.plus(&delta)?`.
- `absorb`'s per-aggregate loop:
  ```rust
  for (i, agg) in self.aggs.iter().enumerate() {
      // `lower` gives every aggregate but COUNT(*) a column.
      let Some(col) = agg.column else { continue };
      let value = row.get(col);
      match agg.func {
          AggFn::Count => {
              if *value != Value::Null {
                  d.accs[i].non_null += w;
              }
          }
          AggFn::Sum | AggFn::Total | AggFn::Avg => d.accs[i].absorb_term(value, w)?,
      }
  }
  ```
- `encode`/`decode`, by aggregate (the layout above): `fn width(agg: &Agg) -> usize` (0, 1 or 3) gives the expected row length `2 + Σ width`; `int_sum` encodes as `Int` when `i64::try_from` succeeds, else `Blob(int_sum.to_be_bytes().to_vec())`; decoding a 16-byte BLOB whose value fits `i64`, or an `Approx` whose count is 0, or any other shape is `StateError("corrupted aggregate state: …")` naming the value.
- `output` returns `Result<Option<Row>, StateError>`:
  ```rust
  values.push(match agg.func {
      AggFn::Count => match agg.column {
          None => Value::Int(self.rows),
          Some(_) => Value::Int(acc.non_null),
      },
      AggFn::Sum if acc.non_null == 0 => Value::Null,
      AggFn::Sum => match &acc.approx {
          // The `approx` switch (M2c spec §4): integers only, so an INTEGER.
          None => Value::Int(i64::try_from(acc.int_sum).map_err(|_| StateError(SUM_OVERFLOW.into()))?),
          Some(_) => acc.approx_value(ExactSum::round),
      },
      AggFn::Total if acc.non_null == 0 => Value::Real(0.0),
      AggFn::Total => acc.approx_value(ExactSum::round),
      AggFn::Avg if acc.non_null == 0 => Value::Null,
      AggFn::Avg => acc.approx_value(|s| s.div_round(acc.non_null)),
  });
  ```
- `plan.rs`: the aggregate arm of `write_canonical` becomes
  ```rust
  let name = match a.func {
      AggFn::Count => "count",
      AggFn::Sum => "sum",
      AggFn::Total => "total",
      AggFn::Avg => "avg",
  };
  match a.column {
      None => name.to_string(),
      Some(c) => format!("{name}({c})"),
  }
  ```
  (the existing texts `count`, `count(c)`, `sum(c)` are unchanged). In `lower`, the "must carry a column" check applies to every function but `Count`, with the message `spec §5.2: agg {i} is {func:?} but names no column; only COUNT(*) may omit one`; delete `require_integer` and its call, and the comment block that justified it (now superseded by M2c spec §4).
- `compile.rs` `aggregate`:
  ```rust
  let func = match func_name.to_ascii_lowercase().as_str() {
      "count" => AggFn::Count,
      "sum" => AggFn::Sum,
      "total" => AggFn::Total,
      "avg" => AggFn::Avg,
      _ => {
          return Err(unsupported(&format!(
              "the function `{func_name}` (v0's aggregates are COUNT, SUM, TOTAL and AVG)"
          )))
      }
  };
  match (func, args.as_slice()) {
      (AggFn::Count, [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)]) => Ok(Agg { func, column: None }),
      (_, [FunctionArg::Unnamed(FunctionArgExpr::Expr(e))]) => Ok(Agg {
          func,
          column: Some(scope.require_column(e, &format!("{} over", func_name.to_ascii_uppercase()))?),
      }),
      _ => Err(unsupported(&format!("`{call}`"))),
  }
  ```
  `Named::column` for an aggregate uses plan decision 10's types and nullability. Update `compile`'s doc comment: an aggregate is `COUNT(*)`, `COUNT(<column>)`, `SUM`, `TOTAL` or `AVG` of a column.
- `schema.rs`:
  ```rust
  /// A column's affinity (M2c spec §3), from SQLite's rules (datatype3.html §3.1).
  pub enum ColumnType { Integer, Real, Numeric, Text, Blob }
  impl ColumnType {
      pub fn name(self) -> &'static str {
          match self {
              ColumnType::Integer => "INTEGER",
              ColumnType::Real => "REAL",
              ColumnType::Numeric => "NUMERIC",
              ColumnType::Text => "TEXT",
              ColumnType::Blob => "BLOB",
          }
      }
  }
  ```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core -p ivmlite-sql --locked && scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Gate rows** (`## ivmlite-core`; the end-to-end ones under `## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §4: the `approx` switch | In `output`'s `Sum` arm, `match &acc.approx` → `match &None::<Approx>` | `sum_of_integers_stays_integer_and_a_real_makes_it_real`, `text_that_reads_as_an_integer_sums_as_an_integer`, `sum_total_avg_and_count_over_integers_and_text` |
| M2c §4: the overflow check | `i64::try_from(acc.int_sum).map_err(…)?` → `acc.int_sum as i64` | `sum_outside_i64_is_an_error_and_total_and_avg_are_not`, `sum_overflow_fails_the_refresh_with_its_message_and_changes_nothing` |
| M2c §4: frac is dropped when `approx` returns to 0 | In `Acc::plus`, return `Some(sum)` when `sum.count == 0` | `the_state_forgets_frac_when_approx_returns_to_zero` |
| M2c §4: `COUNT(x)` counts non-NULL inputs by weight | `d.accs[i].non_null += w` → `+= 1` in the `Count` arm | `count_of_a_column_skips_nulls_and_follows_weights` |
| M2c §4: AVG divides by `non_null` | `s.div_round(acc.non_null)` → `s.div_round(self.rows)` | `avg_divides_by_the_non_null_count` |
| M2c §4: TOTAL of nothing is 0.0 | `Value::Real(0.0)` → `Value::Null` | `nothing_non_null_gives_null_sum_and_avg_and_zero_total` |
| M2c §4 / plan decision 5: opposite infinities | Delete the `(true, true) => Value::Null` arm (falls to `+Inf`) | `opposite_infinities_sum_to_null` |
| M2c §4: AVG is the exact mean rounded once | `s.div_round(acc.non_null)` → `s.round() / acc.non_null as f64` | `avg_is_the_exact_mean_rounded_once`, `avg_differs_from_sqlite_only_by_rounding_the_exact_mean_once` |

  The row "§6.1 `SUM` over a non-INTEGER column must be rejected at `create_view`" becomes `n/a — superseded by M2c Task 6 (spec §4): SUM accepts every column, classified as SQLite's sum does`.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-core/src/agg.rs crates/ivmlite-core/src/query.rs crates/ivmlite-core/src/schema.rs \
  crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/lib.rs crates/ivmlite-sql/src/compile.rs \
  crates/ivmlite-sqlite/src/view.rs crates/ivmlite-test/src/sql.rs crates/ivmlite-test/src/data.rs \
  crates/ivmlite-test/src/query.rs crates/ivmlite-test/tests/extension_types.rs docs/mutation-gates.md
git commit -m "feat: exact SUM, TOTAL, AVG and COUNT(column)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Non-STRICT tables and every declared type, judged by affinity (spec §3 "Base tables", §6 "Non-STRICT capture")

**Files:**
- Modify: `crates/ivmlite-sqlite/src/catalog.rs` — `CaptureInfo` (`:37-41`), `table` (`:82-86` the STRICT refusal; `:127-165` the type mapping and rowid rule), `capture` (`:210-291`); add `affinity`
- Modify: `crates/ivmlite-sqlite/src/view.rs` — `shape` (`:194-236`)
- Modify: `crates/ivmlite-test/tests/extension_lifecycle.rs:241-271` (`what_v0_cannot_maintain_is_rejected_by_name`), `docs/mutation-gates.md`
- Test: `catalog.rs` tests; `crates/ivmlite-test/tests/extension_types.rs`

**Interfaces:**
- Consumes: Task 6's `ColumnType` variants; Task 2's typeless shadow tables; Task 5's canonical keys.
- Produces:
  ```rust
  /// M2c spec §3: a declared type's affinity, by SQLite's rules in order
  /// (datatype3.html §3.1). A STRICT table's `ANY` stores values unchanged.
  pub fn affinity(declared: &str, strict: bool) -> ColumnType;
  pub struct CaptureInfo { pub without_rowid: bool, pub unique_keys: Vec<UniqueKey>, pub declared_types: Vec<String> }
  ```
  `shape` renders each column as `"<name>" <declared type> <AFFINITY>` (e.g. `"v" VARCHAR(10) TEXT`, `"x"  BLOB` for no type), so Phase 3b's latch breaks a view whose base table changes a declared type even within one affinity.

- [ ] **Step 1: Write the failing tests.** `catalog.rs`:

```rust
#[test]
fn affinity_follows_sqlites_rules_in_order() {
    use ColumnType::{Blob, Integer, Numeric, Real, Text};
    for (declared, want) in [
        ("INTEGER", Integer),
        ("INT", Integer),
        ("TINYINT", Integer),
        ("BIGINT UNSIGNED", Integer),
        ("FLOATING POINT", Integer), // rule 1 wins: "POINT" contains INT
        ("CHARINT", Integer),
        ("VARCHAR(255)", Text),
        ("NCHAR(55)", Text),
        ("CLOB", Text),
        ("text", Text),
        ("BLOB", Blob),
        ("", Blob),
        ("REAL", Real),
        ("DOUBLE", Real),
        ("DOUBLE PRECISION", Real),
        ("FLOAT(8,2)", Real),
        ("NUMERIC", Numeric),
        ("DECIMAL(10,5)", Numeric),
        ("BOOLEAN", Numeric),
        ("DATE", Numeric),
        ("DATETIME", Numeric),
        ("STRING", Numeric),
    ] {
        assert_eq!(affinity(declared, false), want, "{declared}");
    }
    assert_eq!(affinity("ANY", true), Blob);
    assert_eq!(affinity("ANY", false), Numeric);
}
```

`extension_types.rs`:

```rust
use ivmlite_core::Value;
use std::collections::BTreeMap;

/// A SQLite value as ivmlite's value, exactly (REAL bit for bit).
fn exact(v: Sql) -> Value {
    match v {
        Sql::Null => Value::Null,
        Sql::Integer(n) => Value::Int(n),
        Sql::Real(x) => Value::Real(x),
        Sql::Text(s) => Value::Text(s),
        Sql::Blob(b) => Value::Blob(b),
    }
}

/// `sql`'s rows, its first column a weight, summed by exact identity of the rest.
fn weighted(c: &Connection, sql: &str) -> BTreeMap<Vec<Value>, i64> {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    let mut out: BTreeMap<Vec<Value>, i64> = BTreeMap::new();
    let rows: Vec<(i64, Vec<Value>)> = s
        .query_map([], |r| {
            Ok((r.get(0)?, (1..n).map(|i| r.get::<_, Sql>(i).map(exact)).collect::<rusqlite::Result<_>>()?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (w, row) in rows {
        *out.entry(row).or_insert(0) += w;
    }
    out.retain(|_, w| *w != 0);
    out
}

#[test]
fn non_strict_capture_keeps_every_storage_class() {
    // Spec §6: a value of each storage class, written through INSERT, UPDATE
    // and REPLACE into a column of each affinity, is captured exactly as SQLite
    // stored it; a DELETE then cancels it by row identity.
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, i INTEGER, r REAL, n NUMERIC, x TEXT, b)").unwrap();
    create(&c, "v", "SELECT id, COUNT(*) FROM t GROUP BY id").unwrap();
    let values = [
        Sql::Null,
        Sql::Integer(1),
        Sql::Integer(i64::MAX),
        Sql::Real(1.0),
        Sql::Real(1.5),
        Sql::Real(-0.0),
        Sql::Real(f64::INFINITY),
        Sql::Text("12".into()),
        Sql::Text(" 3.0 ".into()),
        Sql::Text("abc".into()),
        Sql::Blob(vec![0, 0xff]),
    ];
    for (id, v) in values.iter().enumerate() {
        c.execute("INSERT INTO t VALUES (?1, ?2, ?2, ?2, ?2, ?2)", rusqlite::params![id as i64, v]).unwrap();
    }
    for (id, v) in values.iter().rev().enumerate() {
        c.execute("UPDATE t SET i = ?2, r = ?2, n = ?2, x = ?2, b = ?2 WHERE id = ?1", rusqlite::params![id as i64, v])
            .unwrap();
    }
    for (id, v) in values.iter().enumerate().step_by(2) {
        c.execute("REPLACE INTO t VALUES (?1, ?2, ?2, ?2, ?2, ?2)", rusqlite::params![id as i64, v]).unwrap();
    }
    let delta = "SELECT __ivm_w, id, i, r, n, x, b FROM __ivm_delta_t";
    assert_eq!(weighted(&c, delta), weighted(&c, "SELECT 1, id, i, r, n, x, b FROM t"));
    // Row 7 ends with REAL 1.0 (the reversed UPDATE loop); pinned as SQLite
    // 3.53 stores it in each affinity (measured).
    assert_eq!(
        one_row(&c, "SELECT typeof(i), typeof(r), typeof(n), typeof(x), x, typeof(b) FROM t WHERE id = 7"),
        vec![
            Sql::Text("integer".into()),
            Sql::Text("real".into()),
            Sql::Text("integer".into()),
            Sql::Text("text".into()),
            Sql::Text("1.0".into()),
            Sql::Text("real".into()),
        ]
    );
    c.execute_batch("DELETE FROM t").unwrap();
    assert!(weighted(&c, delta).is_empty(), "every captured DELETE cancels its INSERT");
}

#[test]
fn every_declared_type_is_accepted_and_one_and_one_point_zero_are_one_group() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k, v VARCHAR(255), d DATETIME, f FLOAT(8,2), z BOOLEAN);
         CREATE TABLE s(k ANY, v INT) STRICT;
         INSERT INTO t VALUES (1, 'a', 1, 1.5, 1), (1.0, 'b', 2, 2.5, 0), ('1', 'c', 3, 3.5, 1);",
    )
    .unwrap();
    let sql = "SELECT k, COUNT(*), SUM(f) FROM t GROUP BY k";
    create(&c, "g", sql).unwrap();
    create(&c, "h", "SELECT k, COUNT(*) FROM s GROUP BY k").unwrap();
    // Two groups, as in SQLite: 1 with 1.0, and '1'. A group with a REAL key row is REAL.
    assert_eq!(
        rows(&c, "SELECT typeof(k), k, \"COUNT(*)\", \"SUM(f)\" FROM g"),
        rows(&c, "SELECT 'real', 1.0, 2, 4.0 UNION ALL SELECT 'text', '1', 1, 3.5")
    );
    assert_eq!(count(&c, "SELECT count(*) FROM (SELECT k FROM t GROUP BY k)"), 2);
}

#[test]
fn changing_a_declared_type_with_the_same_affinity_breaks_the_view() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k VARCHAR(10), v INTEGER)").unwrap();
    create(&c, "g", "SELECT k, COUNT(*) FROM t GROUP BY k").unwrap();
    c.execute_batch("DROP TABLE t; CREATE TABLE t(k TEXT, v INTEGER)").unwrap();
    let e = refresh(&c, "g").expect_err("the shape records the declared type");
    assert!(e.to_string().contains("changed shape"), "{e}");
}

#[test]
fn a_sum_over_a_real_column_is_the_exact_sum_rounded_once() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x REAL); INSERT INTO t VALUES ('a', 0.1), ('a', 0.2), ('a', 0.3)").unwrap();
    create(&c, "v", "SELECT k, SUM(x), AVG(x) FROM t GROUP BY k").unwrap();
    assert_eq!(one_row(&c, "SELECT * FROM v"), vec![Sql::Text("a".into()), Sql::Real(0.6), Sql::Real(0.2)]);
}
```

The identity assertion is the binding check; the pinned classes document it. If the bundled SQLite stores a class differently, re-measure and pin what it stores, never weaken the identity check.

In `what_v0_cannot_maintain_is_rejected_by_name`, delete the entries `"not STRICT"`, `"type ANY"` and `"type REAL"`; the `COLLATE` and every other entry stays.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --locked --test extension_types 2>&1 | tail -8`
Expected: FAIL — `table t: is not STRICT; …`.

- [ ] **Step 3: Implement.**
  - `catalog.rs`:
    ```rust
    pub fn affinity(declared: &str, strict: bool) -> ColumnType {
        let upper = declared.to_ascii_uppercase();
        let has = |part: &str| upper.contains(part);
        if strict && upper == "ANY" {
            ColumnType::Blob
        } else if has("INT") {
            ColumnType::Integer
        } else if has("CHAR") || has("CLOB") || has("TEXT") {
            ColumnType::Text
        } else if has("BLOB") || upper.is_empty() {
            ColumnType::Blob
        } else if has("REAL") || has("FLOA") || has("DOUB") {
            ColumnType::Real
        } else {
            ColumnType::Numeric
        }
    }
    ```
  - `table`: delete the `if !strict { … }` refusal and its message. The type mapping becomes `let ty = affinity(&declared_type, strict);`. The rowid rule: `let rowid = pk == 1 && declared_type.eq_ignore_ascii_case("INTEGER");` (an INTEGER PRIMARY KEY is a rowid alias only when the declared type is exactly `INTEGER`). Update the struct doc comment: "refuses what ivmlite cannot represent" without the STRICT clause.
  - `capture`: also read `SELECT type FROM "main".pragma_table_info(?1, 'main') ORDER BY cid` into `declared_types`.
  - `view.rs` `shape`: `format!("{} {} {}", quote(&c.name), capture.declared_types[i], c.ty.name())` for column `i`; delete the type closure Task 2 inlined. The doc comment says the shape records "each column's declared type and affinity (M2c spec §3)".
  - The `ivmlite-sql` trait doc in `lib.rs` (`Catalog`) still says the SQLite catalog reports "a non-STRICT table, an `ANY` column" as errors: rewrite it to "a `COLLATE` clause (spec §7.1) or anything else the extension cannot capture".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Gate rows** (`## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §3: affinity by SQLite's rules, in order | In `affinity`, test `REAL`/`FLOA`/`DOUB` before `INT` | `affinity_follows_sqlites_rules_in_order` |
| M2c §3: a STRICT `ANY` stores values unchanged | Delete the `strict && upper == "ANY"` branch | `affinity_follows_sqlites_rules_in_order` |
| M2c §3: the shape records each column's declared type | In `shape`, drop `capture.declared_types[i]` from the column text | `changing_a_declared_type_with_the_same_affinity_breaks_the_view` |
| M2c §3/§6: delta values are captured with their storage class | Re-run Task 2's "delta and pend value columns have no type" row | append `non_strict_capture_keeps_every_storage_class` to its red set |
| M2c §2 end to end | Re-run Task 5's canonicalization and real-key rows | append `every_declared_type_is_accepted_and_one_and_one_point_zero_are_one_group` |

  The rows "§7.1: a non-STRICT table is refused" and "§7.1: an `ANY` column is refused" become `n/a — superseded by M2c Task 7 (spec §3): every declared type is accepted by affinity`.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-sqlite/src/catalog.rs crates/ivmlite-sqlite/src/view.rs crates/ivmlite-sql/src/lib.rs \
  crates/ivmlite-test/tests/extension_types.rs crates/ivmlite-test/tests/extension_lifecycle.rs docs/mutation-gates.md
git commit -m "feat(sqlite): accept non-STRICT tables and every declared type by affinity

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 8: Retraction-safe MIN and MAX (spec §4 "MIN / MAX", §6 "MIN/MAX")

**Files:**
- Create: `crates/ivmlite-core/src/sortkey.rs`
- Modify: `crates/ivmlite-core/src/arrangement.rs` (`Arrangement` `:17-21`, `ArrangementRole` `:49-57`), `crates/ivmlite-core/src/agg.rs` (`AggState`), `crates/ivmlite-core/src/node.rs:101-118` (`Plan::Aggregate` build), `crates/ivmlite-core/src/query.rs` (`AggFn::Min`, `AggFn::Max`), `crates/ivmlite-core/src/plan.rs` (canonical names), `crates/ivmlite-core/src/lib.rs` (`mod sortkey; pub use sortkey::{from_sort_key, sort_key};`)
- Modify: `crates/ivmlite-sqlite/src/names.rs:132-141` (`state_table`), `crates/ivmlite-sqlite/src/state.rs` (codec, `first`), `crates/ivmlite-sqlite/src/view.rs` (`statement_capacity` `:95-112`, `Changes` `:1164-1171`, `compute` `:1174-1201`, `apply` `:1374-1386`, `is_state_table_of` `:1833-1844`)
- Modify: `crates/ivmlite-sql/src/compile.rs` (`aggregate`, `Named::column`), `docs/mutation-gates.md`
- Test: `sortkey.rs`, `agg.rs`, `state.rs`, `view.rs` (unit), `crates/ivmlite-test/tests/extension_types.rs`

**Interfaces:**
- Consumes: Task 1's `Value::sql_cmp`, Task 3's `SplitMix64`, Task 6's `AggState` layout (MIN/MAX add no group-row values).
- Produces:
  ```rust
  pub enum AggFn { Count, Sum, Total, Avg, Min, Max }
  pub enum ArrangementRole { JoinLeft, JoinRight, AggregateGroups, AggregateExtremum(usize) /* the agg's index */ }
  pub trait Arrangement {
      fn get(&self, key: &Row) -> Result<Vec<(Row, i64)>, StateError>;
      fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) -> Result<(), StateError>;
      fn scan(&self) -> Result<Vec<(Row, Row, i64)>, StateError>;
      /// The smallest live value under `key` in `Row` order, the largest when `descending`.
      fn first(&self, key: &Row, descending: bool) -> Result<Option<Row>, StateError> { /* default over get */ }
  }
  pub fn sort_key(value: &Value) -> Option<Vec<u8>>;          // None for NULL
  pub fn from_sort_key(key: &[u8]) -> Result<Value, StateError>;
  impl AggState {
      pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>, state: Box<dyn Arrangement>, extremes: Vec<Box<dyn Arrangement>>) -> AggState;
  }
  // crates/ivmlite-sqlite/src/state.rs
  pub enum ValueCodec { Row, SortKey }
  pub fn codec(id: ArrangementId) -> ValueCodec;
  impl ValueCodec { pub fn encode_val(self, val: &Row) -> Result<Vec<u8>, StateError>; pub fn decode_val(self, bytes: &[u8]) -> Result<Row, StateError>; }
  pub fn first_sql(table: &str, descending: bool) -> String;
  impl BufferedArrangement { pub fn new(conn: Rc<Connection>, table: &str, pending: Pending, codec: ValueCodec) -> Self; }
  ```
  A MIN/MAX arrangement maps the canonical group key to `[Value::Blob(sort_key)]` rows with weights; its table is `__ivm_state_<view>_<node>_agg_extremum_<agg index>`, with the usual `PRIMARY KEY(key, val)`, and stores `val` raw (`ValueCodec::SortKey`), so `ORDER BY val` is `memcmp` order.

**The encoding** (plan decisions 1 and 2):
- numeric: `0x01`, then 8 bytes of the order-preserving `f64` (`bits | 1<<63` if non-negative, `!bits` if negative; an `Int` uses `i as f64`, rounded to nearest; `-0.0` uses `+0.0`'s bits), then 8 bytes of the `i64` tie (`(tie as u64) ^ 1<<63`, big-endian; an `Int`'s own value; a `Real`'s value truncated toward zero and saturated, which is Rust's `x as i64`; `-0.0`'s tie is 0), then `0x00` Int, `0x01` Real, `0x02` Real `-0.0`;
- TEXT `0x02`, BLOB `0x03`: the bytes with each `0x00` written `0x00 0xFF`, then `0x00 0x00`.

Why this orders exactly: rounding is monotone, so the `f64` prefix orders any two numerics whose prefixes differ; with equal prefixes, the truncated tie orders an `Int` against a `Real` exactly (a large prefix is integral, so the Real's tie is its value), and equal ties fall to the class byte, which puts `Int` first (spec §4 "Ties").

- [ ] **Step 1: Write the failing tests.** `sortkey.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SplitMix64;
    use std::cmp::Ordering;

    fn domain() -> Vec<Value> {
        let two53 = 9_007_199_254_740_992_i64;
        let mut v = vec![
            Value::Int(0), Value::Int(1), Value::Int(-1), Value::Int(i64::MAX), Value::Int(i64::MIN),
            Value::Int(i64::MAX - 1), Value::Int(two53), Value::Int(two53 + 1), Value::Int(-two53 - 1),
            Value::Int(1 << 60), Value::Int((1 << 60) + 1),
            Value::Real(0.0), Value::Real(-0.0), Value::Real(1.0), Value::Real(-1.0), Value::Real(0.5),
            Value::Real(-0.5), Value::Real(two53 as f64), Value::Real((1i64 << 60) as f64),
            Value::Real(9_223_372_036_854_775_808.0), Value::Real(-9_223_372_036_854_775_808.0),
            Value::Real(1e300), Value::Real(-1e300), Value::Real(f64::INFINITY), Value::Real(f64::NEG_INFINITY),
            Value::Real(5e-324), Value::Real(-5e-324),
            Value::Text(String::new()), Value::Text("a".into()), Value::Text("a\0".into()),
            Value::Text("ab".into()), Value::Text("\u{100}".into()),
            Value::Blob(vec![]), Value::Blob(vec![0]), Value::Blob(vec![0, 0]), Value::Blob(vec![0, 1]),
            Value::Blob(vec![1]), Value::Blob(vec![0xff]),
        ];
        let mut rng = SplitMix64(11);
        while v.len() < 400 {
            v.push(match rng.below(4) {
                0 => Value::Int((rng.next_u64() as i64) >> rng.below(64)),
                1 => {
                    let x = f64::from_bits(rng.next_u64());
                    if x.is_nan() { continue; }
                    Value::Real(x)
                }
                2 => Value::Text((0..rng.below(4)).map(|_| ['\0', 'a', 'b'][rng.below(3) as usize]).collect()),
                _ => Value::Blob((0..rng.below(4)).map(|_| [0u8, 1, 0xff][rng.below(3) as usize]).collect()),
            });
        }
        v
    }

    #[test]
    fn sort_keys_order_like_sql_cmp() {
        // Spec §4: memcmp order equals §2's order; equal numerics stay distinct entries.
        let d = domain();
        for a in &d {
            for b in &d {
                let (ka, kb) = (sort_key(a).unwrap(), sort_key(b).unwrap());
                match a.sql_cmp(b) {
                    Ordering::Less => assert!(ka < kb, "{a:?} < {b:?}"),
                    Ordering::Greater => assert!(ka > kb, "{a:?} > {b:?}"),
                    Ordering::Equal => assert_eq!(ka == kb, a == b, "{a:?} vs {b:?}"),
                }
            }
        }
    }

    #[test]
    fn sort_keys_round_trip() {
        for a in domain() {
            assert_eq!(from_sort_key(&sort_key(&a).unwrap()).unwrap(), a);
        }
        assert_eq!(sort_key(&Value::Null), None);
    }

    #[test]
    fn min_prefers_the_int_in_a_numeric_tie() {
        let k = |v: Value| sort_key(&v).unwrap();
        assert!(k(Value::Int(1)) < k(Value::Real(1.0)));
        assert!(k(Value::Int(0)) < k(Value::Real(0.0)));
        assert!(k(Value::Real(0.0)) < k(Value::Real(-0.0)));
        assert!(k(Value::Int(i64::MAX)) < k(Value::Real(9_223_372_036_854_775_808.0)));
        assert!(k(Value::Real((1i64 << 60) as f64)) < k(Value::Int((1 << 60) + 1)));
    }

    #[test]
    fn corrupt_keys_are_errors_not_panics() {
        for bad in [vec![], vec![0x01, 0], vec![0x02, b'a'], vec![0x02, 0, 7], vec![0x09], vec![0x02, 0xff, 0, 0]] {
            let e = from_sort_key(&bad).expect_err("corrupt");
            assert!(e.0.contains("corrupted MIN/MAX state"), "{bad:?}: {e}");
        }
        let mut nan = sort_key(&Value::Real(1.0)).unwrap();
        nan[1..9].copy_from_slice(&(f64::NAN.to_bits() | (1 << 63)).to_be_bytes());
        assert!(from_sort_key(&nan).is_err());
    }
}
```

(`vec![0x02, 0xff, 0, 0]` is the TEXT `"\u{ff}"`'s bytes — `0xff` alone is not UTF-8, so it is corrupt TEXT.)

`agg.rs` tests:

```rust
fn extremes_state(aggs: Vec<Agg>) -> AggState {
    let n = aggs.iter().filter(|a| matches!(a.func, AggFn::Min | AggFn::Max)).count();
    AggState::new(
        vec![0],
        aggs,
        Box::new(MemArrangement::new()),
        (0..n).map(|_| Box::new(MemArrangement::new()) as Box<dyn Arrangement>).collect(),
    )
}

#[test]
fn min_and_max_retract_the_current_extreme() {
    let mut s = extremes_state(vec![agg(AggFn::Min, 1), agg(AggFn::Max, 1)]);
    s.absorb(&ZSet::from_rows([
        (row(vec![txt("a"), int(5)]), 1),
        (row(vec![txt("a"), int(3)]), 1),
        (row(vec![txt("a"), int(9)]), 1),
    ]))
    .unwrap();
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), int(3)]), -1), (row(vec![txt("a"), int(9)]), -1)]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([(row(vec![txt("a"), int(3), int(9)]), -1), (row(vec![txt("a"), int(5), int(5)]), 1)])
    );
}

#[test]
fn a_duplicated_minimum_survives_one_retraction() {
    let mut s = extremes_state(vec![agg(AggFn::Min, 1)]);
    s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(2)]), 2), (row(vec![txt("a"), int(7)]), 1)]))
        .unwrap();
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(2)]), -1)])).unwrap();
    assert!(d.is_empty(), "MIN is still 2: {d:?}");
}

#[test]
fn min_of_only_nulls_is_null_and_classes_order_numeric_text_blob() {
    let mut s = extremes_state(vec![agg(AggFn::Min, 1), agg(AggFn::Max, 1)]);
    let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)])).unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), Value::Null, Value::Null]), 1)]));
    let d = s
        .absorb(&ZSet::from_rows([
            (row(vec![txt("a"), Value::Blob(vec![0])]), 1),
            (row(vec![txt("a"), txt("z")]), 1),
            (row(vec![txt("a"), real(2.5)]), 1),
        ]))
        .unwrap();
    assert_eq!(
        d,
        ZSet::from_rows([
            (row(vec![txt("a"), Value::Null, Value::Null]), -1),
            (row(vec![txt("a"), real(2.5), Value::Blob(vec![0])]), 1),
        ])
    );
}

#[test]
fn a_numeric_tie_gives_min_the_int_and_max_the_real() {
    // Spec §4 "Ties" for MIN; plan decision 3 for MAX.
    let mut s = extremes_state(vec![agg(AggFn::Min, 1), agg(AggFn::Max, 1)]);
    let d = s
        .absorb(&ZSet::from_rows([(row(vec![txt("a"), real(1.0)]), 1), (row(vec![txt("a"), int(1)]), 1)]))
        .unwrap();
    assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(1), real(1.0)]), 1)]));
}
```

Update every existing `AggState::new(...)` call in core tests to pass `vec![]` as `extremes`.

`state.rs` test (pins the read's text; the plan itself is checked in `ivmlite-test`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extreme_read_is_one_ordered_primary_key_search() {
        assert_eq!(
            first_sql("__ivm_state_m_0_agg_extremum_1", true),
            "SELECT val, w FROM \"main\".\"__ivm_state_m_0_agg_extremum_1\" WHERE key = ?1 ORDER BY val DESC"
        );
        assert_eq!(
            first_sql("t", false),
            "SELECT val, w FROM \"main\".\"t\" WHERE key = ?1 ORDER BY val"
        );
    }
}
```

`view.rs`'s `a_views_state_tables_are_recognized_exactly` gains `assert!(is_state_table_of("__ivm_state_v_0_agg_extremum_3", "v"));`, `assert!(!is_state_table_of("__ivm_state_v_0_agg_extremum_", "v"));`, `assert!(!is_state_table_of("__ivm_state_v_0_agg_extremum_x", "v"));`.

`extension_types.rs`:

```rust
#[test]
fn min_and_max_survive_retracting_the_current_extreme() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 5), ('a', 3), ('a', 9), ('b', 'x');")
        .unwrap();
    let sql = "SELECT k, MIN(v), MAX(v), COUNT(*) FROM t GROUP BY k";
    create(&c, "m", sql).unwrap();
    assert_matches_oracle(&c, "m", sql);
    c.execute_batch("DELETE FROM t WHERE v = 3; DELETE FROM t WHERE v = 9;").unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
    // Retract the current minimum and add smaller ones in the same refresh:
    // the read must see the refresh's own pending writes.
    c.execute_batch("DELETE FROM t WHERE v = 5; INSERT INTO t VALUES ('a', 1), ('a', 2);").unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
    c.execute_batch("INSERT INTO t VALUES ('c', NULL)").unwrap();
    refresh(&c, "m").unwrap();
    assert_matches_oracle(&c, "m", sql);
}

#[test]
fn min_and_max_order_across_storage_classes() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', X'00'), ('a', 'text'), ('a', 2.5), ('a', 2), ('a', NULL);")
        .unwrap();
    let sql = "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k";
    create(&c, "m", sql).unwrap();
    for delete in ["v = 2", "v = 2.5", "v = 'text'"] {
        assert_matches_oracle(&c, "m", sql);
        c.execute_batch(&format!("DELETE FROM t WHERE {delete}")).unwrap();
        refresh(&c, "m").unwrap();
    }
    assert_matches_oracle(&c, "m", sql);
}

#[test]
fn a_numeric_tie_gives_min_the_integer_and_max_the_real() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 1.0), ('a', 1);").unwrap();
    create(&c, "m", "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k").unwrap();
    assert_eq!(
        one_row(&c, "SELECT typeof(\"MIN(v)\"), typeof(\"MAX(v)\") FROM m"),
        vec![Sql::Text("integer".into()), Sql::Text("real".into())]
    );
}

#[test]
fn the_extreme_is_read_through_the_state_tables_primary_key() {
    // Spec §6: the plan of the exact statement `state::first_sql` builds
    // (pinned by `the_extreme_read_is_one_ordered_primary_key_search`).
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v); INSERT INTO t VALUES ('a', 1)").unwrap();
    create(&c, "m", "SELECT k, MIN(v), MAX(v) FROM t GROUP BY k").unwrap();
    let tables: Vec<String> = rows(&c, "SELECT name FROM sqlite_schema WHERE name LIKE '__ivm_state_m_%_agg_extremum_%'")
        .into_iter()
        .map(|r| match &r[0] { Sql::Text(t) => t.clone(), other => panic!("{other:?}") })
        .collect();
    assert_eq!(tables.len(), 2, "{tables:?}");
    for table in tables {
        for order in ["", " DESC"] {
            let sql = format!("SELECT val, w FROM \"main\".\"{table}\" WHERE key = ?1 ORDER BY val{order}");
            let plan: Vec<String> = c
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map([Sql::Blob(vec![])], |r| r.get::<_, String>(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert!(plan.iter().any(|d| d.contains("USING PRIMARY KEY (key=?)")), "{sql}: {plan:?}");
            assert!(!plan.iter().any(|d| d.contains("TEMP B-TREE")), "{sql}: {plan:?}");
        }
    }
}

#[test]
fn min_over_a_join_sees_both_tables_changes_in_one_refresh() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE a(k TEXT, x); CREATE TABLE b(k TEXT, y);
         INSERT INTO a VALUES ('p', 5), ('q', 1); INSERT INTO b VALUES ('p', 1), ('q', 2);",
    )
    .unwrap();
    let sql = "SELECT a.k, MIN(a.x), MAX(b.y) FROM a JOIN b ON a.k = b.k GROUP BY a.k";
    create(&c, "j", sql).unwrap();
    c.execute_batch(
        "INSERT INTO a VALUES ('p', 3); DELETE FROM a WHERE x = 5; INSERT INTO b VALUES ('p', 7); DELETE FROM b WHERE y = 2;",
    )
    .unwrap();
    refresh(&c, "j").unwrap();
    assert_matches_oracle(&c, "j", sql);
}

#[test]
fn dropping_a_min_view_removes_its_extremum_tables() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, v)").unwrap();
    let before = objects(&c);
    create(&c, "m", "SELECT k, MIN(v) FROM t GROUP BY k").unwrap();
    c.execute_batch("DROP TABLE m").unwrap();
    assert_eq!(objects(&c), before);
}
```

In `compile.rs`, move `MIN(amount)` from the rejected list to `the_new_aggregates_compile_with_sqlites_names_and_types` (`MIN(amount)` → `Integer`, nullable; `max(region)` → `Text`, nullable).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core --locked sortkey:: 2>&1 | tail -3`
Expected: FAIL to compile — `sort_key` not found.

- [ ] **Step 3: Implement `sortkey.rs`.**

```rust
//! M2c spec §4 "Encoding of `val`": MIN/MAX state values, encoded so that
//! `memcmp` order is spec §2's cross-class order (plan decisions 1 and 2).

use crate::{StateError, Value};

const NUMERIC: u8 = 0x01;
const TEXT: u8 = 0x02;
const BLOB: u8 = 0x03;
const INT_ENTRY: u8 = 0x00;
const REAL_ENTRY: u8 = 0x01;
const NEGATIVE_ZERO_ENTRY: u8 = 0x02;

/// `None` for NULL, which MIN and MAX skip.
pub fn sort_key(value: &Value) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match value {
        Value::Null => return None,
        Value::Int(i) => numeric(&mut out, *i as f64, *i, INT_ENTRY),
        Value::Real(x) if *x == 0.0 && x.is_sign_negative() => numeric(&mut out, 0.0, 0, NEGATIVE_ZERO_ENTRY),
        // `as` truncates toward zero and saturates, infinities included.
        Value::Real(x) => numeric(&mut out, *x, *x as i64, REAL_ENTRY),
        Value::Text(s) => escaped(&mut out, TEXT, s.as_bytes()),
        Value::Blob(b) => escaped(&mut out, BLOB, b),
    }
    Some(out)
}

fn numeric(out: &mut Vec<u8>, approx: f64, tie: i64, entry: u8) {
    out.push(NUMERIC);
    let bits = approx.to_bits();
    let ordered = if bits >> 63 == 1 { !bits } else { bits | (1 << 63) };
    out.extend_from_slice(&ordered.to_be_bytes());
    out.extend_from_slice(&((tie as u64) ^ (1 << 63)).to_be_bytes());
    out.push(entry);
}

fn escaped(out: &mut Vec<u8>, class: u8, payload: &[u8]) {
    out.push(class);
    for &b in payload {
        out.push(b);
        if b == 0 {
            out.push(0xff);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

pub fn from_sort_key(key: &[u8]) -> Result<Value, StateError> {
    let corrupt = |what: &str| StateError(format!("corrupted MIN/MAX state: {what}"));
    let (&class, rest) = key.split_first().ok_or_else(|| corrupt("an empty value"))?;
    match class {
        NUMERIC => {
            if rest.len() != 17 {
                return Err(corrupt("a numeric value is not 17 bytes"));
            }
            let ordered = u64::from_be_bytes(rest[..8].try_into().expect("8 bytes"));
            let approx = f64::from_bits(if ordered >> 63 == 1 { ordered ^ (1 << 63) } else { !ordered });
            let tie = (u64::from_be_bytes(rest[8..16].try_into().expect("8 bytes")) ^ (1 << 63)) as i64;
            let value = match rest[16] {
                INT_ENTRY => Value::Int(tie),
                REAL_ENTRY if approx.is_nan() => return Err(corrupt("REAL is NaN")),
                REAL_ENTRY => Value::Real(approx),
                NEGATIVE_ZERO_ENTRY => Value::Real(-0.0),
                other => return Err(corrupt(&format!("unknown numeric entry {other:#04x}"))),
            };
            if sort_key(&value).as_deref() != Some(key) {
                return Err(corrupt("a numeric value is not in its canonical encoding"));
            }
            Ok(value)
        }
        TEXT | BLOB => {
            let mut payload = Vec::new();
            let mut i = 0;
            loop {
                match (rest.get(i), rest.get(i + 1)) {
                    (Some(0), Some(0)) if i + 2 == rest.len() => break,
                    (Some(0), Some(0xff)) => {
                        payload.push(0);
                        i += 2;
                    }
                    (Some(0), _) | (None, _) => return Err(corrupt("a TEXT or BLOB value is not terminated")),
                    (Some(&b), _) => {
                        payload.push(b);
                        i += 1;
                    }
                }
            }
            if class == TEXT {
                String::from_utf8(payload).map(Value::Text).map_err(|_| corrupt("TEXT is not UTF-8"))
            } else {
                Ok(Value::Blob(payload))
            }
        }
        other => Err(corrupt(&format!("unknown class {other:#04x}"))),
    }
}
```

- [ ] **Step 4: Implement the arrangement and the aggregate.**
  - `arrangement.rs`: add `AggregateExtremum(usize)` with doc "A MIN or MAX's live values, keyed by the group key; the `usize` is the aggregate's index in `aggs`." Add the default `first`:
    ```rust
    fn first(&self, key: &Row, descending: bool) -> Result<Option<Row>, StateError> {
        let live = self.get(key)?.into_iter().filter(|(_, w)| *w > 0).map(|(v, _)| v);
        Ok(if descending { live.max() } else { live.min() })
    }
    ```
    (`Row`'s order over `[Value::Blob(..)]` rows is byte order, which is `memcmp`.)
  - `node.rs`, `Plan::Aggregate`:
    ```rust
    let input = Node::build_at(input, next, arrangements)?;
    let groups = arrangements(ArrangementId { node, role: ArrangementRole::AggregateGroups })?;
    let extremes = aggs
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.func, AggFn::Min | AggFn::Max))
        .map(|(i, _)| arrangements(ArrangementId { node, role: ArrangementRole::AggregateExtremum(i) }))
        .collect::<Result<Vec<_>, _>>()?;
    Node::Aggregate {
        input: Box::new(input),
        state: crate::AggState::new(group_by.clone(), aggs.clone(), groups, extremes),
    }
    ```
  - `agg.rs`: `AggState` gains `extremes: Vec<Box<dyn Arrangement>>` (one per MIN/MAX, in `aggs` order) and a private `slots: Vec<Option<usize>>` computed in `new`. In `absorb` phase one, for a `Min`/`Max` aggregate with column `c`, when `sort_key(row.get(c))` is `Some(k)`, add `w` to `extreme_deltas[key][slot][Row::new(vec![Value::Blob(k)])]` (a `BTreeMap<Row, Vec<BTreeMap<Row, i64>>>`). In phase two, per touched key, in this order:
    ```rust
    let old = self.load(&key)?;
    let new = old.plus(&delta)?;
    let old_extremes = self.extreme_values(&key)?;
    if let Some(changes) = extreme_deltas.remove(&key) {
        for (slot, values) in changes.into_iter().enumerate() {
            for (value, w) in values {
                self.extremes[slot].update(&key, &value, w)?;
            }
        }
    }
    let new_extremes = self.extreme_values(&key)?;
    let old_out = old.output(&key, &self.aggs, &old_extremes)?;
    let new_out = new.output(&key, &self.aggs, &new_extremes)?;
    ```
    with
    ```rust
    /// Each MIN/MAX's current value for `key`, indexed like `aggs` (None elsewhere).
    fn extreme_values(&self, key: &Row) -> Result<Vec<Option<Value>>, StateError> {
        self.aggs
            .iter()
            .zip(&self.slots)
            .map(|(agg, slot)| {
                let Some(slot) = slot else { return Ok(None) };
                match self.extremes[*slot].first(key, agg.func == AggFn::Max)? {
                    None => Ok(None),
                    Some(row) => match row.0.as_slice() {
                        [Value::Blob(k)] => from_sort_key(k).map(Some),
                        other => Err(StateError(format!("corrupted MIN/MAX state for group {key:?}: {other:?}"))),
                    },
                }
            })
            .collect()
    }
    ```
    `output` takes `extremes: &[Option<Value>]` and emits `extremes[i].clone().unwrap_or(Value::Null)` for `Min`/`Max`; `width` is 0 for both; `absorb_term` is not called for them.
  - `query.rs`: `Min`, `Max`; `plan.rs` canonical names `min`, `max`; `lower`'s column check already covers them.

- [ ] **Step 5: Implement the extension side.**
  - `names.rs` `state_table`: the role becomes a `String`; `ArrangementRole::AggregateExtremum(agg) => format!("agg_extremum_{agg}")`.
  - `view.rs` `is_state_table_of`: accept `role` in the three fixed names, or `role.strip_prefix("agg_extremum_")` that is non-empty and all ASCII digits.
  - `state.rs`:
    ```rust
    /// How an arrangement's values are stored in its table's `val` column.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ValueCodec {
        /// `encode.rs`'s row encoding.
        Row,
        /// A MIN/MAX value's sort key, stored raw, so that `ORDER BY val` is
        /// spec §2's order (M2c spec §4). Its value rows are `[Value::Blob(key)]`.
        SortKey,
    }

    pub fn codec(id: ArrangementId) -> ValueCodec {
        match id.role {
            ArrangementRole::AggregateExtremum(_) => ValueCodec::SortKey,
            _ => ValueCodec::Row,
        }
    }

    impl ValueCodec {
        pub fn encode_val(self, val: &Row) -> Result<Vec<u8>, StateError> {
            match (self, val.0.as_slice()) {
                (ValueCodec::Row, _) => Ok(encode(val)),
                (ValueCodec::SortKey, [Value::Blob(key)]) => Ok(key.clone()),
                (ValueCodec::SortKey, other) => Err(StateError(format!(
                    "broken invariant: a MIN/MAX value row is {other:?}, not one sort key"
                ))),
            }
        }

        pub fn decode_val(self, bytes: &[u8]) -> Result<Row, StateError> {
            match self {
                ValueCodec::Row => decode(bytes),
                ValueCodec::SortKey => Ok(Row::new(vec![Value::Blob(bytes.to_vec())])),
            }
        }
    }

    /// M2c spec §4: the indexed read of a MIN (ascending) or a MAX (descending).
    pub fn first_sql(table: &str, descending: bool) -> String {
        format!(
            "SELECT val, w FROM {} WHERE key = ?1 ORDER BY val{}",
            main_qualified(table),
            if descending { " DESC" } else { "" }
        )
    }
    ```
    `BufferedArrangement` gains `codec`, `first_asc_sql`, `first_desc_sql`; `get` and `scan` decode `val` with `codec.decode_val`. Override `first` (plan decision 11: no `LIMIT 1`, because the stored minimum may be retracted by a pending change of this refresh):
    ```rust
    fn first(&self, key: &Row, descending: bool) -> Result<Option<Row>, StateError> {
        let pending = self.pending.borrow();
        let changes = pending.get(key);
        let change = |val: &Row| changes.and_then(|c| c.get(val)).copied().unwrap_or(0);
        let sql = if descending { &self.first_desc_sql } else { &self.first_asc_sql };
        let mut stmt = self.conn.prepare_cached(sql).map_err(state_error)?;
        let mut rows = stmt.query([encode(key)]).map_err(state_error)?;
        // Stored values passed over because a pending change retracted them.
        let mut passed: Vec<Row> = Vec::new();
        let mut stored = None;
        while let Some(r) = rows.next().map_err(state_error)? {
            let val = self.codec.decode_val(&r.get::<_, Vec<u8>>(0).map_err(state_error)?)?;
            let w: i64 = r.get(1).map_err(state_error)?;
            if w + change(&val) > 0 {
                stored = Some(val);
                break;
            }
            passed.push(val);
        }
        // Dropping `rows` resets the cached statement before the callback returns (Phase 6 §3).
        drop(rows);
        let earlier = |v: &Row| match &stored {
            None => true,
            Some(s) => if descending { v > s } else { v < s },
        };
        // A value only this refresh added, ordered before the stored candidate.
        let added = changes
            .into_iter()
            .flatten()
            .filter(|(v, &w)| w > 0 && !passed.contains(v) && earlier(v))
            .map(|(v, _)| v.clone());
        let added = if descending { added.max() } else { added.min() };
        Ok(added.or(stored))
    }
    ```
    A pending value that is also stored and was passed over has a total weight ≤ 0, so `passed` excludes it; a stored value after `stored` cannot win.
  - `view.rs`: `compute` passes `codec(id)` to `BufferedArrangement::new`; `Changes` gains `codecs: Vec<ValueCodec>` (filled from `ids` in `create` and `refresh`), and `apply` stages `SqlValue::Blob(changes.codecs[i].encode_val(val).map_err(|e| e.0)?)`. `statement_capacity` adds one per `SortKey` arrangement: `+ ids.iter().filter(|id| codec(**id) == ValueCodec::SortKey).count() // BufferedArrangement first`.
  - `compile.rs`: `"min" => AggFn::Min`, `"max" => AggFn::Max`; the unknown-function message lists `COUNT, SUM, TOTAL, AVG, MIN and MAX`; `Named::column` gives MIN/MAX the argument column's `ty`, nullable.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ivmlite-core --locked && scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 7: Gate rows** (`## ivmlite-core` and `## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §4: the MIN/MAX tie order | Swap the values of `INT_ENTRY` and `REAL_ENTRY` | `min_prefers_the_int_in_a_numeric_tie`, `a_numeric_tie_gives_min_the_int_and_max_the_real`, `a_numeric_tie_gives_min_the_integer_and_max_the_real` |
| M2c §4: the encoding orders like §2 (property) | In `numeric`, `if bits >> 63 == 1 { !bits }` → `if bits >> 63 == 1 { bits }` | `sort_keys_order_like_sql_cmp` |
| M2c §4: numerically equal entries stay distinct and exact | In `sort_key`, the `Int` tie `*i` → `0` | `sort_keys_order_like_sql_cmp`, `sort_keys_round_trip` |
| Plan decision 1: `-0.0` is its own entry | Delete the `-0.0` arm of `sort_key` | `sort_keys_order_like_sql_cmp` (with `Real(0.0)`), `sort_keys_round_trip` |
| M2c §4: the read sees the refresh's pending writes | In `first`, `w + change(&val) > 0` → `w > 0` | `min_and_max_survive_retracting_the_current_extreme` (extension) |
| M2c §4: MAX reads descending | In `extreme_values`, `agg.func == AggFn::Max` → `false` | `min_and_max_retract_the_current_extreme`, `min_and_max_order_across_storage_classes` |
| M2c §6: the result is read through the index | In `first_sql`, `ORDER BY val` → `ORDER BY +val` | `the_extreme_read_is_one_ordered_primary_key_search` (the plan test reads its own copy of the text, so it stays green; the text-pin test is the gate) |
| M2c §4: a MIN/MAX's own state table is dropped with its view | `is_state_table_of` without the `agg_extremum_` branch | `dropping_a_min_view_removes_its_extremum_tables` |

- [ ] **Step 8: Commit**

```bash
git add crates/ivmlite-core/src/sortkey.rs crates/ivmlite-core/src/arrangement.rs crates/ivmlite-core/src/agg.rs \
  crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/query.rs crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/lib.rs \
  crates/ivmlite-sqlite/src/names.rs crates/ivmlite-sqlite/src/state.rs crates/ivmlite-sqlite/src/view.rs \
  crates/ivmlite-sql/src/compile.rs crates/ivmlite-test/tests/extension_types.rs docs/mutation-gates.md
git commit -m "feat: retraction-safe MIN and MAX through an indexed per-aggregate state table

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Filters convert their literal by affinity; joins require one affinity and match canonical keys (spec §5)

**Files:**
- Modify: `crates/ivmlite-core/src/plan.rs` — the comparison check in `lower` (`:367-394`), `check_join` (`:466-505`), tests `an_integer_literal_compared_with_a_text_column_is_rejected` (`:927`), `a_text_literal_compared_with_an_integer_column_is_rejected` (`:944`), `a_literal_of_the_columns_own_type_is_accepted` (`:976`), `join_keys_of_different_types_are_rejected` (`:1185`), `a_comparison_on_the_right_tables_text_column_checks_its_type` (`:1198`)
- Modify: `crates/ivmlite-core/src/join.rs:109-114` (`join_key`)
- Modify: `crates/ivmlite-sql/src/compile.rs:753-797` (`literal`), its rejected-list entries for `1.5` and `9223372036854775808`
- Modify: `docs/mutation-gates.md`
- Test: `plan.rs`, `join.rs`, `compile.rs` tests; `crates/ivmlite-test/tests/extension_types.rs`

**Interfaces:**
- Consumes: Task 4's `numeric_affinity`, Task 6's `ColumnType`.
- Produces: `lower` stores the **converted** literal in `Plan::Filter`'s predicate (so the canonical plan text shows it). Messages:
  - `a REAL literal cannot be compared with the TEXT-affinity column {name}: SQLite's REAL-to-TEXT rendering differs between versions (M2c spec §5); compare with a quoted string` (plan decision 7);
  - `join: the key columns must have the same affinity, but {a}.{l} has {L} affinity and {b}.{r} has {R} affinity (M2c spec §5)`, with `L`/`R` from `ColumnType::name()`.
  The SQL front end accepts REAL literals (a numeric token with `.` or an exponent, or an integer token outside `i64`, as SQLite does) and BLOB literals `X'…'` (even-length hex).

**The conversion** (spec §5, datatype3.html §4.2), at compile time:
- column of `Integer`, `Real` or `Numeric` affinity, `Text` literal → `numeric_affinity(text)`, or the text unchanged when it is not a well-formed number;
- `Text` affinity: `Int(n)` → `Text(n.to_string())`; `Real` → refused (plan decision 7); `Text`, `Blob` unchanged;
- `Blob` affinity: nothing converts.

- [ ] **Step 1: Write the failing tests.** `plan.rs` (replace the five tests named above; keep their bounds-checking siblings):

```rust
fn schema_of(types: &[ColumnType]) -> Schema {
    Schema {
        table: "t".into(),
        columns: types
            .iter()
            .enumerate()
            .map(|(i, &ty)| Column { name: format!("c{i}"), ty, nullable: true })
            .collect(),
    }
}

/// The literal `lower` stored for `column <op> literal` over `ty`.
fn converted(ty: ColumnType, literal: Value) -> Result<Value, PlanError> {
    let db = Database::single(schema_of(&[ty]));
    let plan = lower_query(&q(vec![0], vec![count()], cmp(0, CmpOp::Eq, literal)), &db)?;
    let Plan::Aggregate { input, .. } = plan else { panic!() };
    let Plan::Project { input, .. } = *input else { panic!() };
    let Plan::Filter { predicate: Predicate::Compare { value, .. }, .. } = *input else { panic!() };
    Ok(value)
}

#[test]
fn a_text_literal_gets_numeric_affinity_for_a_numeric_column() {
    for ty in [ColumnType::Integer, ColumnType::Real, ColumnType::Numeric] {
        assert_eq!(converted(ty, Value::Text(" 3 ".into())).unwrap(), Value::Int(3), "{ty:?}");
        assert_eq!(converted(ty, Value::Text("2.5".into())).unwrap(), Value::Real(2.5), "{ty:?}");
        assert_eq!(converted(ty, Value::Text("abc".into())).unwrap(), Value::Text("abc".into()), "{ty:?}");
        assert_eq!(converted(ty, Value::Int(3)).unwrap(), Value::Int(3), "{ty:?}");
    }
}

#[test]
fn an_integer_literal_becomes_text_for_a_text_column() {
    assert_eq!(converted(ColumnType::Text, Value::Int(-12)).unwrap(), Value::Text("-12".into()));
    assert_eq!(converted(ColumnType::Text, Value::Blob(vec![1])).unwrap(), Value::Blob(vec![1]));
}

#[test]
fn a_real_literal_against_a_text_column_is_refused() {
    let e = converted(ColumnType::Text, Value::Real(2.5)).unwrap_err();
    assert!(e.0.contains("REAL literal cannot be compared with the TEXT-affinity column c0"), "{e}");
}

#[test]
fn a_blob_affinity_column_converts_nothing() {
    for literal in [Value::Text("3".into()), Value::Int(3), Value::Real(1.5), Value::Blob(vec![])] {
        assert_eq!(converted(ColumnType::Blob, literal.clone()).unwrap(), literal);
    }
}

#[test]
fn join_keys_of_different_affinity_are_rejected_naming_both() {
    let left = schema_of(&[ColumnType::Integer]);
    let mut right = schema_of(&[ColumnType::Real]);
    right.table = "u".into();
    let e = lower(&ResolvedView {
        anchor: &left,
        join: Some(ResolvedJoin { right: &right, left_column: 0, right_column: 0 }),
        group_by: vec![0],
        aggs: vec![count()],
        predicate: Predicate::None,
    })
    .unwrap_err();
    assert!(e.0.contains("t.c0 has INTEGER affinity and u.c0 has REAL affinity"), "{e}");
}
```

`join.rs`:

```rust
#[test]
fn canonical_keys_match_one_with_one_point_zero_but_not_text() {
    let mut j = empty_join();
    let r = |k: Value| Row::new(vec![k, Value::Int(0)]);
    j.absorb(&z(&[(r(Value::Int(1)), 1)]), &ZSet::new()).unwrap();
    let out = j
        .absorb(&ZSet::new(), &z(&[(r(Value::Real(1.0)), 1), (r(Value::Text("1".into())), 1)]))
        .unwrap();
    assert_eq!(out, z(&[(joined(&r(Value::Int(1)), &r(Value::Real(1.0))), 1)]));
}
```

`compile.rs`: remove the rejected entries `amount > 1.5` and `amount > 9223372036854775808`; add

```rust
#[test]
fn real_and_blob_literals_compile_as_sqlite_reads_them() {
    let lit = |sql: &str| match ok(sql).plan {
        Plan::Aggregate { input, .. } => match *input {
            Plan::Project { input, .. } => match *input {
                Plan::Filter { predicate: Predicate::Compare { value, .. }, .. } => value,
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    let w = |cond: &str| format!("SELECT region, COUNT(*) FROM orders WHERE {cond} GROUP BY region");
    assert_eq!(lit(&w("amount > 1.5")), Value::Real(1.5));
    assert_eq!(lit(&w("amount > -2.5e1")), Value::Real(-25.0));
    assert_eq!(lit(&w("amount > 9223372036854775808")), Value::Real(9_223_372_036_854_775_808.0));
    assert_eq!(lit(&w("amount > -9223372036854775808")), Value::Int(i64::MIN));
    assert_eq!(lit(&w("amount > '7'")), Value::Int(7), "numeric affinity for an INTEGER column");
    assert_eq!(lit(&w("region = 7")), Value::Text("7".into()), "TEXT affinity for a TEXT column");
    assert_eq!(lit(&w("region = X'00ff'")), Value::Blob(vec![0, 0xff]));
    assert!(err(&w("region = X'0'")).contains("hex"));
}
```

`extension_types.rs`:

```rust
#[test]
fn filters_follow_sqlites_comparison_affinity() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k TEXT, i INTEGER, x TEXT, b);
         INSERT INTO t VALUES ('a', 3, '3', 3), ('a', 1, 'abc', '3'), ('b', 2, '2', X'00'), ('b', NULL, NULL, 2.5);",
    )
    .unwrap();
    for (n, cond) in [
        "i > '2'", "i = 2.5", "i < 9223372036854775808", "x = 3", "x > 2", "x != 'abc'",
        "b = 3", "b = '3'", "b > X'00'", "b < 'a'", "b >= 2.5", "i IS NULL",
    ]
    .iter()
    .enumerate()
    {
        let sql = format!("SELECT k, COUNT(*) FROM t WHERE {cond} GROUP BY k");
        let name = format!("f{n}");
        create(&c, &name, &sql).unwrap();
        assert_matches_oracle(&c, &name, &sql);
    }
    c.execute_batch("INSERT INTO t VALUES ('a', 7, '7', 'z'), ('c', 2, '1e1', 3.0); DELETE FROM t WHERE i = 1;").unwrap();
    for n in 0..12 {
        let name = format!("f{n}");
        refresh(&c, &name).unwrap();
        let sql: String = c.query_row("SELECT sql FROM __ivm_view WHERE name = ?1", [&name], |r| r.get(0)).unwrap();
        assert_matches_oracle(&c, &name, &sql);
    }
    let e = create(&c, "bad", "SELECT k, COUNT(*) FROM t WHERE x > 2.5 GROUP BY k").unwrap_err();
    assert!(e.to_string().contains("REAL literal cannot be compared"), "{e}");
}

#[test]
fn joins_match_canonical_keys_and_refuse_mixed_affinity() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE a(k, v TEXT); CREATE TABLE b(k, w TEXT);
         CREATE TABLE ai(k INTEGER, v TEXT); CREATE TABLE br(k REAL, w TEXT);
         INSERT INTO a VALUES (1, 'x'), (2.5, 'y'); INSERT INTO b VALUES (1.0, 'p'), ('1', 'q'), (2.5, 'r');",
    )
    .unwrap();
    let sql = "SELECT a.v, COUNT(*) FROM a JOIN b ON a.k = b.k GROUP BY a.v";
    create(&c, "j", sql).unwrap();
    assert_matches_oracle(&c, "j", sql);
    c.execute_batch("INSERT INTO a VALUES (1.0, 'x'); DELETE FROM b WHERE w = 'r';").unwrap();
    refresh(&c, "j").unwrap();
    assert_matches_oracle(&c, "j", sql);
    let e = create(&c, "bad", "SELECT ai.v, COUNT(*) FROM ai JOIN br ON ai.k = br.k GROUP BY ai.v").unwrap_err();
    assert!(e.to_string().contains("INTEGER affinity") && e.to_string().contains("REAL affinity"), "{e}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-core -p ivmlite-sql --locked 2>&1 | grep -E "FAILED|panicked" | head`
Expected: FAIL — the conversion tests (today's `lower` rejects a mismatched literal), and `canonical_keys_match_one_with_one_point_zero_but_not_text`.

- [ ] **Step 3: Implement.**
  - `plan.rs`: replace the `matches` block with the NULL check (unchanged message) followed by
    ```rust
    /// M2c spec §5: SQLite's comparison affinity, applied to the literal at
    /// compile time (datatype3.html §4.2).
    fn convert_literal(column: &Column, literal: &Value) -> Result<Value, PlanError> {
        Ok(match (column.ty, literal) {
            (ColumnType::Integer | ColumnType::Real | ColumnType::Numeric, Value::Text(t)) => {
                numeric_affinity(t).unwrap_or_else(|| literal.clone())
            }
            (ColumnType::Text, Value::Int(n)) => Value::Text(n.to_string()),
            (ColumnType::Text, Value::Real(_)) => {
                return Err(PlanError(format!(
                    "a REAL literal cannot be compared with the TEXT-affinity column {}: SQLite's \
                     REAL-to-TEXT rendering differs between versions (M2c spec §5); compare with a quoted string",
                    column.name
                )))
            }
            _ => literal.clone(),
        })
    }
    ```
    and build the `Filter` from a predicate whose `value` is the converted literal. Rewrite the doc comment above `lower`'s type checks: SQLite's conversion now happens here, so the comparison agrees with SQLite.
  - `check_join`: compare `l.ty != r.ty` with the new message.
  - `join.rs` `join_key`: `value => Some(Row::new(vec![value.canonical()]))`, doc "M2c spec §5: canonical, so `1` matches `1.0` as in SQLite". The arrangement's value rows stay the original input rows, so the output carries the stored values.
  - `compile.rs` `literal`:
    ```rust
    /// A numeric token as SQLite reads it: INTEGER when it has no `.` or
    /// exponent and fits `i64`, else REAL.
    fn number(text: &str) -> Result<Value, SqlError> {
        let integral = !text.contains(['.', 'e', 'E']);
        if integral {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(Value::Int(n));
            }
        }
        text.parse::<f64>()
            .map(Value::Real)
            .map_err(|_| unsupported(&format!("the literal {text}")))
    }

    fn blob(hex: &str) -> Result<Value, SqlError> {
        if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SqlError(format!("the BLOB literal X'{hex}' is not an even number of hex digits")));
        }
        Ok(Value::Blob(
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect(),
        ))
    }
    ```
    `SqlValue::Number(text, false)` → `number(text)`, negated → `number(&format!("-{text}"))`; `SqlValue::HexStringLiteral(h)` → `blob(h)`. A REAL literal is converted by Rust's correctly rounded parser (plan decision 8). Update `compile`'s doc comment: a literal is a number, a single-quoted string or `X'…'`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Gate rows** (`## ivmlite-core`, `## ivmlite-sql`, `## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §5: the literal's affinity conversion | `convert_literal` returns `Ok(literal.clone())` for every input (keep the REAL refusal) | `a_text_literal_gets_numeric_affinity_for_a_numeric_column`, `an_integer_literal_becomes_text_for_a_text_column`, `filters_follow_sqlites_comparison_affinity` |
| Plan decision 7: REAL vs TEXT-affinity refused | Delete the `(ColumnType::Text, Value::Real(_))` arm | `a_real_literal_against_a_text_column_is_refused` |
| M2c §5: the join-affinity check | In `check_join`, `if l.ty != r.ty` → `if false && l.ty != r.ty` | `join_keys_of_different_affinity_are_rejected_naming_both`, `joins_match_canonical_keys_and_refuse_mixed_affinity` |
| M2c §5: join keys are canonicalized | In `join_key`, `value.canonical()` → `value.clone()` | `canonical_keys_match_one_with_one_point_zero_but_not_text`, `joins_match_canonical_keys_and_refuse_mixed_affinity` |
| M2c §5: a numeric literal is INTEGER when it fits, as SQLite reads it | In `number`, delete the `if integral { … }` block, so every number parses as REAL | `real_and_blob_literals_compile_as_sqlite_reads_them` (the `i64::MIN` case), `negative_literals_reach_i64_min` |

  Superseded (`n/a — superseded by M2c Task 9 (spec §5)`): "§6.1 A comparison over a non-INTEGER column with an INTEGER literal must be rejected", "§6.1 / Ruling 2: a literal must have its column's type", "Ruling 2 must not over-reject". Rewrite "§6.1 / Ruling 4: a join's two key columns must have the same type" as the affinity check and re-verify it with the new test names.

- [ ] **Step 6: Commit**

```bash
git add crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/join.rs crates/ivmlite-sql/src/compile.rs \
  crates/ivmlite-test/tests/extension_types.rs docs/mutation-gates.md
git commit -m "feat: comparison affinity for filter literals, REAL and BLOB literals, canonical join keys

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 10: The independent reference and the comparator (spec §6 "Differential testing", bullets 1–3)

**Files:**
- Create: `crates/ivmlite-test/src/exact_reference.rs`, `crates/ivmlite-test/src/compare.rs`
- Modify: `crates/ivmlite-test/src/naive.rs` (evaluation moves to `evaluate`; `input_rows` `:47-66` and `passes` `:78-107` become free functions; `materialize` `:149-…` calls `evaluate`), `crates/ivmlite-test/src/oracle.rs` (`pub(crate) fn to_sql(value: &Value) -> rusqlite::types::Value`, used by `Bound`), `crates/ivmlite-test/src/differential.rs` (`run` `:176-238`, `check_batch_invariance` `:243-290`), `crates/ivmlite-test/src/sql.rs:108-113` (aggregate rendering), `crates/ivmlite-test/src/invariants.rs:10-50` (canonical key uniqueness), `crates/ivmlite-test/src/lib.rs` (modules and exports)
- Modify: `docs/mutation-gates.md`
- Test: unit tests in the new modules, `sql.rs`, `differential.rs`

**Interfaces:**
- Consumes: `ivmlite_core::{Value::canonical, Value::sql_cmp, numeric_affinity, SUM_OVERFLOW, AggFn::*}`; nothing from `ExactSum` (independence, spec §6).
- Produces:
  ```rust
  // exact_reference.rs
  pub const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0; // 2^-53, plan decision 12
  pub fn fsum(terms: &[f64]) -> f64;                  // Shewchuk / CPython math.fsum, correctly rounded
  pub fn int_pieces(n: i128) -> [f64; 3];             // three f64s whose exact sum is n
  pub fn exact_mean(terms: &[f64], count: i64) -> f64; // Σterms / count, rounded once, error-free
  // naive.rs
  pub struct SqliteTerms { /* in-memory Connection + cache */ }
  impl SqliteTerms { pub fn new() -> Result<SqliteTerms, EngineError>; }
  pub struct Evaluated {
      pub rows: ZSet,
      /// (Σ|xᵢ|, non-NULL count) of each SUM/TOTAL/AVG, by canonical group key and aggregate index.
      pub magnitudes: BTreeMap<(Row, usize), (f64, i64)>,
  }
  /// The view as M2c specifies it. `Err(EngineError(SUM_OVERFLOW))` when a final exact SUM leaves i64.
  pub fn evaluate(db: &Database, query: &ViewQuery, bases: &BTreeMap<String, ZSet>, terms: &SqliteTerms) -> Result<Evaluated, EngineError>;
  // compare.rs
  pub fn compare_with_oracles(got: &ZSet, reference: &Evaluated, sqlite: Option<&ZSet>, query: &ViewQuery) -> Result<(), String>;
  // differential.rs
  pub enum Outcome { Completed, EndedOnOverflow }
  pub fn run_outcome<E: Engine>(engine: &mut E, case: &TestCase) -> Result<Outcome, Failure>;
  pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure>; // run_outcome, outcome dropped
  ```

**What the reference is** (spec §6): `evaluate` recomputes the view from the base multisets with no incremental state. It classifies each SUM/TOTAL/AVG input by asking SQLite (`SELECT sum(?1)` on its own connection, cached per value), so it never calls `numeric_term`; it sums with `fsum` and divides with `exact_mean`, so it never touches `ExactSum`. Keys follow spec §2 (canonical, REAL when a row's key was REAL); MIN/MAX follow `sql_cmp` with the tie rank `Int` < `Real` < `Real(-0.0)` (plan decisions 1, 3). A comparison literal is converted by the column's affinity with the reference's own small `affinity_literal(ty, literal)` (spec §5's three rules, via `numeric_affinity`). Join keys match when their canonical values are equal and not NULL. Shared helpers (`canonical`, `sql_cmp`, `numeric_affinity`) are also checked by every comparison against SQLite, which shares nothing.

**What a check accepts** (`compare_with_oracles`):
1. `got` equals `reference.rows` exactly: every value by identity, REAL bit for bit.
2. When SQLite produced a result (it is `None` after SQLite's own prefix `integer overflow`, plan decision 14): both sides grouped by canonical key have the same groups, and per aggregate column the reference value equals SQLite's by identity, or — MIN/MAX — both are numeric and `sql_cmp` equal, or — SUM/TOTAL/AVG, both finite REAL — `|ref − sqlite| ≤ 4ε·Σ|xᵢ|` (AVG: `4ε·Σ|xᵢ|/n + 2ε·|sqlite|`). Anything else fails, with the message "…; investigate, do not widen it".

**How `run_outcome` handles overflow** (plan decision 14): at bootstrap and after each batch, the reference is evaluated on the new bases. If the engine's `create_view`, `refresh` or `materialize` fails with a message containing `SUM_OVERFLOW` and the reference also overflows, the case ends with `Outcome::EndedOnOverflow`; an engine overflow without a reference overflow, or a reference overflow the engine did not report, is a `Failure`. `check_batch_invariance` compares the final states of the modes that `Completed` only.

- [ ] **Step 1: Write the failing tests.** `exact_reference.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsum_rounds_half_way_cases_correctly() {
        // CPython's test_math.test_fsum vectors (checked with math.fsum).
        let p53 = 2f64.powi(53);
        assert_eq!(fsum(&[p53, -0.5, -2f64.powi(-54)]), p53 - 1.0);
        assert_eq!(fsum(&[p53, 1.0, 2f64.powi(-100)]), p53 + 2.0);
        assert_eq!(fsum(&[p53 + 10.0, 1.0, 2f64.powi(-100)]), p53 + 12.0);
        assert_eq!(fsum(&[p53 - 4.0, 0.5, 2f64.powi(-54)]), p53 - 3.0);
        assert_eq!(fsum(&[1e16, 1.0, 1e-16]), 10_000_000_000_000_002.0);
    }

    #[test]
    fn fsum_is_exact_where_naive_summation_is_not() {
        assert_eq!(fsum(&[1e16, 1.0, -1e16, 1.0]), 2.0);
        assert_eq!(fsum(&[0.1, 0.2, 0.3]), 0.6);
        assert_eq!(fsum(&[]), 0.0);
    }

    #[test]
    fn exact_mean_is_correctly_rounded() {
        assert_eq!(exact_mean(&[0.1, 0.2, 0.3], 3), 0.2);
        assert_eq!(exact_mean(&[1.0, 2.0], 2), 1.5);
        assert_eq!(exact_mean(&[1.0], 3), 1.0 / 3.0);
        let p54 = 2f64.powi(54);
        // Exactly half-way: to even.
        assert_eq!(exact_mean(&[p54, 2.0], 2), 2f64.powi(53));
        assert_eq!(exact_mean(&[p54, 6.0], 2), 2f64.powi(53) + 4.0);
        assert_eq!(exact_mean(&[-0.1, -0.2, -0.3], 3), -0.2);
    }

    #[test]
    fn int_pieces_sum_exactly() {
        for n in [0i128, 1, -1, i128::from(i64::MAX) * 3, -(1i128 << 126) + 7] {
            let sum: i128 = int_pieces(n).iter().map(|&p| p as i128).sum();
            assert_eq!(sum, n);
        }
    }
}
```

`compare.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Sum, column: Some(1) }, Agg { func: AggFn::Min, column: Some(1) }],
            predicate: Predicate::None,
            join: None,
        }
    }

    fn z(rows: Vec<Vec<Value>>) -> ZSet {
        ZSet::from_rows(rows.into_iter().map(|r| (Row::new(r), 1)))
    }

    fn reference(sum: f64) -> Evaluated {
        Evaluated {
            rows: z(vec![vec![Value::Real(1.0), Value::Real(sum), Value::Int(1)]]),
            magnitudes: BTreeMap::from([((Row::new(vec![Value::Int(1)]), 0), (0.6, 3))]),
        }
    }

    #[test]
    fn a_one_ulp_difference_from_the_reference_is_a_failure() {
        let r = reference(0.6);
        let got = z(vec![vec![Value::Real(1.0), Value::Real(0.6f64.next_up()), Value::Int(1)]]);
        assert!(compare_with_oracles(&got, &r, None, &q()).is_err());
        assert!(compare_with_oracles(&r.rows, &r, None, &q()).is_ok());
    }

    #[test]
    fn sqlite_may_differ_within_the_kbn_bound_and_not_beyond() {
        let r = reference(0.6);
        // SQLite's key is INTEGER and its MIN a REAL of equal value: one group, a tie.
        let near = z(vec![vec![Value::Int(1), Value::Real(0.6f64.next_up()), Value::Real(1.0)]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&near), &q()).is_ok());
        let far = z(vec![vec![Value::Int(1), Value::Real(0.6 + 1e-10), Value::Int(1)]]);
        let e = compare_with_oracles(&r.rows, &r, Some(&far), &q()).unwrap_err();
        assert!(e.contains("do not widen it"), "{e}");
    }

    #[test]
    fn a_class_difference_outside_ties_is_a_failure() {
        let r = reference(0.6);
        let text_min = z(vec![vec![Value::Int(1), Value::Real(0.6), Value::Text("1".into())]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&text_min), &q()).is_err());
        let int_sum = z(vec![vec![Value::Int(1), Value::Int(1), Value::Int(1)]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&int_sum), &q()).is_err());
    }
}
```

`sql.rs`:

```rust
#[test]
fn every_aggregate_renders_with_its_column() {
    let db = Database::single(schema_of_orders()); // the module's existing (region TEXT, amount INTEGER) fixture
    let q = |func: AggFn, column: Option<usize>| ViewQuery {
        group_by: vec![0],
        aggs: vec![Agg { func, column }],
        predicate: Predicate::None,
        join: None,
    };
    for (func, column, want) in [
        (AggFn::Count, None, "COUNT(*)"),
        (AggFn::Count, Some(1), "COUNT(\"orders\".\"amount\")"),
        (AggFn::Total, Some(1), "TOTAL(\"orders\".\"amount\")"),
        (AggFn::Avg, Some(1), "AVG(\"orders\".\"amount\")"),
        (AggFn::Min, Some(1), "MIN(\"orders\".\"amount\")"),
        (AggFn::Max, Some(1), "MAX(\"orders\".\"amount\")"),
    ] {
        assert!(view_query_to_sql(&q(func, column), &db).contains(want), "{func:?}");
    }
}
```

(use the module's existing orders fixture under its real name.)

`differential.rs` tests (with the module's `single_table_case` helper; write the rows over `Schema { table: "orders", columns: [region TEXT, amount INTEGER] }`):

```rust
#[test]
fn sqlite_prefix_overflow_falls_back_to_the_reference() {
    // The oracle inserts a base in `ZSet` order (ascending here), so SQLite's
    // sum overflows on -MAX + -MAX; the final exact sum, 0, fits.
    let r = |a: i64| Row::new(vec![Value::Text("a".into()), Value::Int(a)]);
    let rows = vec![r(-i64::MAX), r(-i64::MAX), r(i64::MAX), r(i64::MAX)];
    let mut case = single_table_case(1, rows, vec![], Batching::All);
    case.query.aggs = vec![Agg { func: AggFn::Sum, column: Some(1) }];
    assert!(matches!(run_outcome(&mut NaiveRecompute::new(), &case), Ok(Outcome::Completed)));
    assert!(matches!(run_outcome(&mut IncrementalEngine::new(), &case), Ok(Outcome::Completed)));
}

#[test]
fn a_final_overflow_ends_the_case_when_the_reference_agrees() {
    let r = |a: i64| Row::new(vec![Value::Text("a".into()), Value::Int(a)]);
    let mut case = single_table_case(2, vec![r(i64::MAX)], vec![Op::Insert(r(1))], Batching::One);
    case.query.aggs = vec![Agg { func: AggFn::Sum, column: Some(1) }];
    assert!(matches!(run_outcome(&mut IncrementalEngine::new(), &case), Ok(Outcome::EndedOnOverflow)));
}
```

(If `single_table_case` hard-codes a query, give it the query as a parameter; keep its existing callers' behaviour. `IncrementalEngine` is the test crate's `Engine` impl in `incremental.rs`; import whatever name that module exports.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-test --locked --lib 2>&1 | tail -5`
Expected: FAIL to compile — `fsum`, `compare_with_oracles`, `run_outcome` not found.

- [ ] **Step 3: Implement `exact_reference.rs`.**

```rust
//! M2c spec §6: the independent reference for REAL results — Shewchuk's exact
//! summation, the algorithm behind Python's `math.fsum`, and an error-free
//! correctly rounded mean. It shares no code with `ivmlite_core::ExactSum`.

use std::cmp::Ordering;

pub const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;

/// The sum of `terms`, rounded once (CPython's `msum` plus its half-way
/// correction). The terms and every partial must stay finite: the
/// differential domain keeps |x| < 2^64.
pub fn fsum(terms: &[f64]) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for &term in terms {
        let mut x = term;
        let mut kept = 0;
        for i in 0..partials.len() {
            let mut y = partials[i];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[kept] = lo;
                kept += 1;
            }
            x = hi;
        }
        partials.truncate(kept);
        partials.push(x);
    }
    let Some(mut n) = partials.len().checked_sub(1) else {
        return 0.0;
    };
    let mut hi = partials[n];
    let mut lo = 0.0;
    while n > 0 {
        let x = hi;
        n -= 1;
        let y = partials[n];
        hi = x + y;
        let yr = hi - x;
        lo = y - yr;
        if lo != 0.0 {
            break;
        }
    }
    if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
        let y = lo * 2.0;
        let x = hi + y;
        let yr = x - hi;
        if y == yr {
            hi = x;
        }
    }
    hi
}

pub fn int_pieces(n: i128) -> [f64; 3] {
    let m = n.unsigned_abs();
    let mask = (1u128 << 43) - 1;
    let pieces = [
        ((m >> 86) as f64) * 2f64.powi(86),
        (((m >> 43) & mask) as f64) * 2f64.powi(43),
        ((m & mask) as f64),
    ];
    if n < 0 { pieces.map(|p| -p) } else { pieces }
}

/// `a * b` as an exact pair (FMA).
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    (p, a.mul_add(b, -p))
}

/// The sign of `2·Σterms − count·a − count·b`, computed exactly.
fn twice_sum_minus(terms: &[f64], count: f64, a: f64, b: f64) -> Ordering {
    let (pa, ea) = two_product(count, a);
    let (pb, eb) = two_product(count, b);
    let mut all: Vec<f64> = terms.iter().map(|t| 2.0 * t).collect();
    all.extend([-pa, -ea, -pb, -eb]);
    fsum(&all).partial_cmp(&0.0).expect("finite")
}

pub fn exact_mean(terms: &[f64], count: i64) -> f64 {
    assert!(count > 0 && count < 1 << 53, "a mean of a positive count below 2^53");
    let sum = fsum(terms);
    if sum == 0.0 {
        return 0.0;
    }
    let n = count as f64;
    let even = |a: f64, b: f64| if a.to_bits() & 1 == 0 { a } else { b };
    let mut q = sum / n;
    loop {
        let up = q.next_up();
        match twice_sum_minus(terms, n, q, up) {
            Ordering::Greater => { q = up; continue; }
            Ordering::Equal => return even(q, up),
            Ordering::Less => {}
        }
        let down = q.next_down();
        match twice_sum_minus(terms, n, q, down) {
            Ordering::Less => { q = down; continue; }
            Ordering::Equal => return even(q, down),
            Ordering::Greater => return q,
        }
    }
}
```

- [ ] **Step 4: Implement the reference in `naive.rs`.** Keep `NaiveRecompute`'s apply/refresh contract; it gains a `terms: SqliteTerms` field (implement `Default` by hand: `SqliteTerms::new().expect("an in-memory SQLite opens")`).

```rust
/// SQLite's own classification of a SUM/TOTAL/AVG input (M2c spec §4).
#[derive(Debug)]
pub struct SqliteTerms {
    conn: Connection,
    cache: RefCell<BTreeMap<Value, Term>>,
}

#[derive(Debug, Clone, Copy)]
enum Term {
    Integer(i64),
    Approx(f64),
}

impl SqliteTerms {
    pub fn new() -> Result<SqliteTerms, EngineError> {
        Ok(SqliteTerms {
            conn: Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?,
            cache: RefCell::new(BTreeMap::new()),
        })
    }

    fn term(&self, value: &Value) -> Result<Term, EngineError> {
        if let Some(t) = self.cache.borrow().get(value) {
            return Ok(*t);
        }
        let got: rusqlite::types::Value = self
            .conn
            .query_row("SELECT sum(?1)", [crate::oracle::to_sql(value)], |r| r.get(0))
            .map_err(|e| EngineError(e.to_string()))?;
        let t = match got {
            rusqlite::types::Value::Integer(n) => Term::Integer(n),
            rusqlite::types::Value::Real(x) => Term::Approx(x),
            other => return Err(EngineError(format!("SQLite's sum({value:?}) is {other:?}"))),
        };
        self.cache.borrow_mut().insert(value.clone(), t);
        Ok(t)
    }
}

#[derive(Default)]
struct Slot {
    non_null: i64,
    int_sum: i128,
    /// Each finite non-integer term, repeated by weight.
    approx: Vec<f64>,
    pos_inf: i64,
    neg_inf: i64,
    /// MIN/MAX's live values.
    values: Vec<Value>,
}

impl Slot {
    fn is_approx(&self) -> bool {
        !self.approx.is_empty() || self.pos_inf > 0 || self.neg_inf > 0
    }

    fn terms(&self) -> Vec<f64> {
        let mut all = self.approx.clone();
        all.extend(int_pieces(self.int_sum));
        all
    }

    fn real(&self, finish: impl Fn(&[f64]) -> f64) -> Value {
        match (self.pos_inf > 0, self.neg_inf > 0) {
            (true, true) => Value::Null,
            (true, false) => Value::Real(f64::INFINITY),
            (false, true) => Value::Real(f64::NEG_INFINITY),
            (false, false) => Value::Real(finish(&self.terms())),
        }
    }
}

/// Plan decisions 1 and 3: equal numerics order Int, then Real, then -0.0.
fn tie_rank(v: &Value) -> u8 {
    match v {
        Value::Real(x) if *x == 0.0 && x.is_sign_negative() => 2,
        Value::Real(_) => 1,
        _ => 0,
    }
}

fn extreme(values: &[Value], max: bool) -> Value {
    let order = |a: &&Value, b: &&Value| a.sql_cmp(b).then(tie_rank(a).cmp(&tie_rank(b)));
    let it = values.iter();
    (if max { it.max_by(order) } else { it.min_by(order) }).cloned().unwrap_or(Value::Null)
}
```

`evaluate`: take the rows from `input_rows` (the anchor's base, or the nested-loop join on `l_key.canonical() == r_key.canonical()` with both non-NULL), keep those with weight > 0 that pass the predicate — `passes` compares `cell.sql_cmp(&affinity_literal(column_ty, literal))`, NULL is UNKNOWN — and group by canonical key, counting REAL key rows by weight. Per aggregate: `Count` with a column and `Min`/`Max` skip NULL; `Sum`/`Total`/`Avg` classify each non-NULL input with `terms.term`, adding `i128::from(n) * i128::from(w)` to `int_sum`, `w` to an infinity count, or `w` copies of the term to `approx`. Emit per group: the keys by spec §2's rule; `Count(None)` → rows; `Count(Some)` → `non_null`; `Sum` → `Null` if `non_null == 0`, else `Real` via `slot.real(fsum)` if `is_approx()`, else `i64::try_from(int_sum)` or `Err(EngineError(SUM_OVERFLOW.into()))`; `Total` → `Real(0.0)` if `non_null == 0`, else `slot.real(fsum)`; `Avg` → `Null` if 0, else `slot.real(|t| exact_mean(t, non_null))`; `Min`/`Max` → `extreme`. Record `magnitudes[(canonical key, i)] = (fsum(&terms.map(f64::abs)), non_null)` for each SUM/TOTAL/AVG. `materialize` returns `evaluate(…)?.rows`.

`affinity_literal(ty, literal)`: `Integer | Real | Numeric` with `Text(t)` → `numeric_affinity(t).unwrap_or(literal)`; `Text` with `Int(n)` → `Text(n.to_string())`; otherwise unchanged. The column's `ty` comes from `db` (anchor columns, then the joined table's).

- [ ] **Step 5: Implement `compare.rs`, `run_outcome`, the SQL rendering and invariants.**
  - `compare.rs`:
    ```rust
    //! M2c spec §6: what one differential check accepts.

    use std::collections::BTreeMap;

    use ivmlite_core::{AggFn, Row, Value, ZSet};

    use crate::exact_reference::UNIT_ROUNDOFF;
    use crate::naive::Evaluated;
    use crate::ViewQuery;

    pub fn compare_with_oracles(got: &ZSet, reference: &Evaluated, sqlite: Option<&ZSet>, query: &ViewQuery) -> Result<(), String> {
        if got != &reference.rows {
            return Err(format!(
                "the engine disagrees with the exact reference\n  engine:    {got:?}\n  reference: {:?}",
                reference.rows
            ));
        }
        let Some(sqlite) = sqlite else { return Ok(()) };
        let keys = query.group_by.len();
        let ours = by_group(&reference.rows, keys)?;
        let theirs = by_group(sqlite, keys)?;
        if !ours.keys().eq(theirs.keys()) {
            return Err(format!(
                "the reference's groups differ from SQLite's\n  reference: {:?}\n  SQLite:    {:?}",
                ours.keys().collect::<Vec<_>>(),
                theirs.keys().collect::<Vec<_>>()
            ));
        }
        for (key, row) in &ours {
            for (i, agg) in query.aggs.iter().enumerate() {
                agree(agg.func, row.get(keys + i), theirs[key].get(keys + i), reference.magnitudes.get(&(key.clone(), i)).copied())
                    .map_err(|why| format!("group {key:?}, aggregate {i} ({:?}): {why}", agg.func))?;
            }
        }
        Ok(())
    }

    /// Rows by canonical group key (spec §2: SQLite's key may be another row's class).
    fn by_group(z: &ZSet, keys: usize) -> Result<BTreeMap<Row, Row>, String> {
        let mut out = BTreeMap::new();
        for (row, &w) in z.iter() {
            if w != 1 {
                return Err(format!("row {row:?} has weight {w}"));
            }
            let key = Row::new(row.0[..keys].iter().map(Value::canonical).collect());
            if out.insert(key.clone(), row.clone()).is_some() {
                return Err(format!("two rows share the group {key:?}"));
            }
        }
        Ok(out)
    }

    fn agree(func: AggFn, ours: &Value, sqlite: &Value, magnitude: Option<(f64, i64)>) -> Result<(), String> {
        let numeric = |v: &Value| matches!(v, Value::Int(_) | Value::Real(_));
        if ours == sqlite {
            return Ok(());
        }
        match (func, ours, sqlite) {
            // Spec §4 "Ties": SQLite's pick among equal numerics follows scan order.
            (AggFn::Min | AggFn::Max, a, b) if numeric(a) && numeric(b) && a.sql_cmp(b).is_eq() => Ok(()),
            (AggFn::Sum | AggFn::Total | AggFn::Avg, Value::Real(a), Value::Real(b)) if a.is_finite() && b.is_finite() => {
                let (abs, n) = magnitude.ok_or("no magnitude recorded for this aggregate")?;
                let bound = match func {
                    AggFn::Avg => 4.0 * UNIT_ROUNDOFF * abs / n as f64 + 2.0 * UNIT_ROUNDOFF * b.abs(),
                    _ => 4.0 * UNIT_ROUNDOFF * abs,
                };
                if (a - b).abs() <= bound {
                    Ok(())
                } else {
                    Err(format!(
                        "{a:?} and SQLite's {b:?} differ by more than KBN's bound {bound:e}; investigate, do not widen it"
                    ))
                }
            }
            _ => Err(format!("{ours:?} against SQLite's {sqlite:?}")),
        }
    }
    ```
  - `differential.rs` `run_outcome`: as described above; `compare` is a closure over `(engine, bases, stage)` returning `Result<Option<Outcome>, Failure>`; `recompute_via_sqlite`'s error containing `integer overflow` becomes `None` for `compare_with_oracles`; any other oracle error is a `Failure` as today. The failure messages keep the existing stage names (`create_view`, `refresh[i]`, `diff[stage]`, `oracle[stage]`, `invariants[stage]`, `materialize[stage]`) and add `reference[stage]`.
  - `sql.rs`: aggregates render `COUNT(*)`, `COUNT(col)`, `SUM(col)`, `TOTAL(col)`, `AVG(col)`, `MIN(col)`, `MAX(col)`; a non-COUNT without a column panics as today.
  - `invariants.rs`: duplicate group keys are judged on the canonical key.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS. The existing sweeps (still the two-column space) now check against the reference too; investigate any red result before touching the comparator.

- [ ] **Step 7: Gate rows** (`## ivmlite-test: semantic contracts`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §6: the reference rounds once (Shewchuk) | Delete `fsum`'s half-way correction block | `fsum_rounds_half_way_cases_correctly` |
| M2c §6: the reference mean is exact | `exact_mean` returns `fsum(terms) / count as f64` | `exact_mean_is_correctly_rounded` |
| M2c §6: REAL results equal the reference bit for bit | Delete the `got != &reference.rows` check | `a_one_ulp_difference_from_the_reference_is_a_failure` |
| M2c §6: within KBN's bound of SQLite | `bound` → `f64::INFINITY` | `sqlite_may_differ_within_the_kbn_bound_and_not_beyond` |
| M2c §6: only numeric MIN/MAX ties are tolerated | Drop `numeric(a) && numeric(b) &&` from the tie arm | `a_class_difference_outside_ties_is_a_failure` |
| Plan decision 14: SQLite's prefix overflow falls back to the reference | Treat an `integer overflow` oracle error as a `Failure` | `sqlite_prefix_overflow_falls_back_to_the_reference` |
| Plan decision 14: a final overflow must be reported | In `run_outcome`, treat a reference overflow as `Completed` | `a_final_overflow_ends_the_case_when_the_reference_agrees` |
| M2c §4: `COUNT(col)` reaches the oracle as written | Render `(AggFn::Count, Some(_))` as `COUNT(*)` | `every_aggregate_renders_with_its_column` |

- [ ] **Step 8: Commit**

```bash
git add crates/ivmlite-test/src/exact_reference.rs crates/ivmlite-test/src/compare.rs crates/ivmlite-test/src/naive.rs \
  crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/sql.rs \
  crates/ivmlite-test/src/invariants.rs crates/ivmlite-test/src/lib.rs docs/mutation-gates.md
git commit -m "test: an independent exact reference and a comparator bound by KBN against SQLite

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The widened differential test — three columns, five types, every aggregate (spec §1 item 5, §6 "Differential testing")

**Files:**
- Modify: `crates/ivmlite-test/src/data.rs` (`gen_row` `:30-51`, `gen_database` `:57-88`, `gen_database_with_swapped_right_table` `:90-102`, tests), `crates/ivmlite-test/src/query.rs` (whole enumeration, `:1-200` and tests), `crates/ivmlite-test/src/sql.rs:43-56` (`create_table_sql`), `crates/ivmlite-test/src/lib.rs` (export `gen_real_numeric_database`, `value_pool`)
- Modify: `crates/ivmlite-test/src/differential.rs` (naive sweeps), `crates/ivmlite-test/tests/harness_catches_bugs.rs` (sweeps and query selections), `crates/ivmlite-test/tests/extension_differential.rs` (sweeps and strides), `crates/ivmlite-test/src/ops.rs` tests that pin "exactly two columns"
- Modify: `docs/superpowers/specs/2026-09-18-ivmlite-design.md` §9.2 item 4 (dated amendment), `docs/mutation-gates.md`
- Test: as above

**Interfaces:**
- Consumes: Task 10's `run`/`run_outcome`, reference and comparator; every engine.
- Produces:
  ```rust
  pub struct Pool { pub narrow: Vec<Value>, pub edge: Vec<Value>, pub big: Vec<Value> }
  pub fn value_pool(ty: ColumnType) -> Pool;
  pub fn gen_database(table_count: usize) -> Database;          // t{i}(k TEXT, v INTEGER, x)
  pub fn gen_database_with_swapped_right_table() -> Database;   // t1's columns reversed
  pub fn gen_real_numeric_database() -> Database;               // t0(r REAL, n NUMERIC, k TEXT), t1(n NUMERIC, k TEXT, r REAL)
  pub fn create_table_sql(schema: &Schema) -> String;           // non-STRICT, declared type = ColumnType::name() or none for Blob
  ```
  Every column of the generated databases is nullable. The three databases together declare INTEGER, REAL, TEXT, NUMERIC and no type (spec §6), and their same-affinity join pairs cover TEXT, INTEGER, none, REAL and NUMERIC keys.

**Value pools** (plan decision 15 — each value is a fixed point of its affinity):

| Affinity | `narrow` | `edge` (1/16) | `big` (1/64) |
|---|---|---|---|
| INTEGER, NUMERIC | `Int(0..=5)`, `Real(2.5)`, `Text("v1")` | `Int(2^53 + 1)`, `Int(-2^53)`, `Real(0.1)`, `Blob([1])` | `Int(i64::MAX - 1)`, `Int(i64::MIN + 1)` |
| REAL | `Real(0.0)`, `Real(1.0)`, `Real(2.0)`, `Real(3.0)`, `Real(4.0)`, `Real(2.5)`, `Real(0.1)`, `Text("v1")` | `Real(2^53)`, `Real(1e15 + 0.5)`, `Real(-0.25)`, `Blob([1])` | — |
| TEXT | `Text("v0")`…`Text("v5")`, `Text("12")`, `Text("1.5")` | `Text(" 7 ")`, `Text("1e2")`, `Text("9223372036854775808")`, `Blob([1])` | — |
| none (BLOB) | `Int(0..=3)`, `Real(1.0)`, `Real(-0.0)`, `Real(2.5)`, `Text("3")`, `Text("1.5")`, `Text("v1")`, `Blob([0])`, `Blob(b"12")` | `Int(2^53 + 1)`, `Real(0.1)`, `Text("12abc")` | `Int(i64::MAX - 1)`, `Int(i64::MIN + 1)` |

`gen_row` draws NULL with `null_rate`, then `big` with probability 1/64 (if non-empty), `edge` with 1/16, else `narrow[..min(distinct.max(1), len)]`, uniformly. Every REAL has |x| < 2^64 and no subnormal, so `fsum` and `exact_mean` stay exact; convertible TEXT has ≤ 15 significant digits (plan decision 8).

**Literals** per affinity, chosen by an index `i` (`literals(ty)[i % len]`): INTEGER and NUMERIC `[Int(3), Real(2.5), Text("3")]`; REAL `[Real(2.0), Text("2.5"), Int(3)]`; TEXT `[Text("v3"), Int(12)]`; none `[Int(2), Text("v1"), Blob([1])]`. Single-table predicates use `i` = the operator's index in `CmpOp::ALL`; join predicates use `i` = the column. No REAL literal ever meets a TEXT-affinity column (plan decision 7).

**The enumeration** (plan decision 13):
- `AGGREGATE_FUNCTIONS = [Sum, Total, Avg, Count (with column), Min, Max]`.
- Single table (3 columns): group-bys `[0] [1] [2] [0,1] [0,2] [1,2]` (6); aggregate sets `[COUNT(*)]` plus, for each column and each function, `[f(c)]` and `[f(c), COUNT(*)]` (37); predicates `None`, the six operators on each column, `IS NULL` and `IS NOT NULL` on each column (25). The space is `G × A × {None}` ∪ `G × {[COUNT(*)], [SUM(c0), MIN(c1), AVG(c2), COUNT(c0)]} × (P \ {None})` = 222 + 288 = **510** queries (was 153).
- Join (6 joined columns): every same-affinity key pair; group-bys over the joined row, 1 or 2 columns (21); aggregate sets `[COUNT(*)]` plus, for joined column `c`, `[AGGREGATE_FUNCTIONS[c % 6](c), COUNT(*)]` (7); the join predicates of today (`None`, `CmpOp::ALL[c % 6]` per column, `IS NULL` on even and `IS NOT NULL` on odd columns: 13). Per key pair `G × A × {None}` ∪ `G × {[COUNT(*)]} × (P \ {None})` = 147 + 252 = **399**; each of the three databases has 3 same-affinity pairs: **1,197** per database (was 900 over two databases' 2 pairs each).

- [ ] **Step 1: Write the failing tests.** `data.rs`:

```rust
#[test]
fn every_pool_value_is_stored_unchanged() {
    // Plan decision 15, against SQLite itself: a pool value inserted into a
    // column of its affinity reads back with the same class and bits.
    let c = rusqlite::Connection::open_in_memory().unwrap();
    for ty in [ColumnType::Integer, ColumnType::Real, ColumnType::Numeric, ColumnType::Text, ColumnType::Blob] {
        c.execute_batch(&format!("DROP TABLE IF EXISTS p; CREATE TABLE p(c {})", ty_sql(ty))).unwrap();
        let pool = value_pool(ty);
        for v in pool.narrow.iter().chain(&pool.edge).chain(&pool.big) {
            c.execute("DELETE FROM p", []).unwrap();
            c.execute("INSERT INTO p VALUES (?1)", [crate::oracle::to_sql(v)]).unwrap();
            let back: rusqlite::types::Value = c.query_row("SELECT c FROM p", [], |r| r.get(0)).unwrap();
            assert_eq!(crate::test_support::from_sql(back), *v, "{ty:?} {v:?}");
        }
    }
}

fn ty_sql(ty: ColumnType) -> &'static str {
    if ty == ColumnType::Blob { "" } else { ty.name() }
}

#[test]
fn the_generated_databases_declare_all_five_types() {
    let mut seen: Vec<ColumnType> = [gen_database(2), gen_real_numeric_database()]
        .iter()
        .flat_map(|db| db.tables().iter().flat_map(|t| t.columns.iter().map(|c| c.ty)).collect::<Vec<_>>())
        .collect();
    seen.sort_by_key(|t| t.name());
    seen.dedup();
    assert_eq!(seen.len(), 5, "{seen:?}");
    assert!(gen_database(3).tables().iter().all(|t| t.columns.len() == 3 && t.columns.iter().all(|c| c.nullable)));
}

#[test]
fn rare_values_are_drawn_but_rarely() {
    let schema = gen_database(1).tables()[0].clone();
    let mut rng = rand::rngs::StdRng::seed_from_u64(5);
    let rows = gen_rows(&mut rng, &schema, &Domain::default(), 6400);
    let big = rows.iter().filter(|r| matches!(r.get(1), Value::Int(n) if n.unsigned_abs() > 1 << 62)).count();
    assert!((40..=160).contains(&big), "about 1/64 of 6400 * 0.8 non-NULL values, got {big}");
}
```

(add `pub(crate) fn from_sql(v: rusqlite::types::Value) -> Value` to `test_support.rs` if no such helper exists; `oracle::from_sqlite` takes a `ValueRef`, which also works.)

`query.rs`:

```rust
#[test]
fn the_enumeration_has_the_recorded_size() {
    // Recorded in the parent spec §9.2 amendment (M2c spec §6).
    let db = gen_database(2);
    assert_eq!(enumerate(&db.tables()[0]).len(), 510);
    for db in [gen_database(2), gen_database_with_swapped_right_table(), gen_real_numeric_database()] {
        assert_eq!(enumerate_join(&db.tables()[0], &db.tables()[1]).len(), 1197);
    }
}

#[test]
fn the_enumeration_covers_every_aggregate_on_every_column() {
    let db = gen_database(2);
    let queries = enumerate(&db.tables()[0]);
    for func in [AggFn::Sum, AggFn::Total, AggFn::Avg, AggFn::Count, AggFn::Min, AggFn::Max] {
        for c in 0..3 {
            assert!(
                queries.iter().any(|q| q.aggs.contains(&Agg { func, column: Some(c) })),
                "{func:?}({c})"
            );
        }
    }
    let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
    for func in [AggFn::Sum, AggFn::Total, AggFn::Avg, AggFn::Count, AggFn::Min, AggFn::Max] {
        assert!(joins.iter().any(|q| q.aggs.iter().any(|a| a.func == func && a.column.is_some())), "{func:?}");
    }
}

#[test]
fn every_literal_is_one_lower_accepts() {
    for db in [gen_database(2), gen_real_numeric_database()] {
        for q in enumerate_database(&db) {
            ivmlite_core::lower_query(&q, &db).unwrap_or_else(|e| panic!("{q:?}: {e}"));
        }
    }
}
```

Rewrite `every_comparison_literal_has_its_columns_type` (the old invariant no longer holds) into `every_literal_is_one_lower_accepts` above, and every test that pinned "two columns" (`generated_database_tables_have_exactly_two_columns` in `ops.rs`, the counts in `query.rs`) to the new shape.

`sql.rs`: replace `create_table_sql_is_strict` with

```rust
#[test]
fn create_table_sql_declares_each_affinity_without_strict() {
    let sql = create_table_sql(&gen_real_numeric_database().tables()[0]);
    assert_eq!(sql, "CREATE TABLE \"t0\" (\"r\" REAL, \"n\" NUMERIC, \"k\" TEXT)");
    assert_eq!(create_table_sql(&gen_database(1).tables()[0]), "CREATE TABLE \"t0\" (\"k\" TEXT, \"v\" INTEGER, \"x\")");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ivmlite-test --locked --lib 2>&1 | tail -5`
Expected: FAIL — `value_pool`, `gen_real_numeric_database` missing; sizes 153/900.

- [ ] **Step 3: Implement** the pools, databases, rendering and enumeration exactly as specified above. `create_table_sql` renders `"<name>" <TYPE>` (no type for `Blob`), ` NOT NULL` when not nullable, and no `STRICT`; update its doc comment (M2c spec §3: the harness's tables are non-STRICT, so SQLite's affinity applies, and the pools hold only fixed points of it). Replace Task 6's `unreachable!` arms.

- [ ] **Step 4: Widen the sweeps.**
  - `differential.rs` and `harness_catches_bugs.rs`: every join sweep runs over `gen_database(2)`, `gen_database_with_swapped_right_table()` and `gen_real_numeric_database()`; add the single-table sweep over `gen_real_numeric_database().tables()[0]` for the incremental and naive engines. Tests that select queries by position (`queries_covering_sum_and_two_column_group_by` and its relatives) select by property (`q.aggs` / `q.group_by`) instead, with the same meaning.
  - `extension_differential.rs`: add `gen_real_numeric_database()` to each join loop and a single-table sweep over it; strides: single 4, join 7, reopening single 12 and join 35, siblings single 8 and join 15, siblings-reopening as its single/join ratio today. Record the case counts in the file's doc comment.
  - Regression fixtures (`tests/regressions/*.json`) keep their two-column schemas and must still pass: `create_table_sql` renders them non-STRICT with INTEGER/TEXT, whose values are fixed points.

- [ ] **Step 5: Run and measure.**

Run: `scripts/test-all.sh 2>&1 | tail -3`
Expected: PASS. A REAL value beyond KBN's bound is a test failure to investigate, not a tolerance to widen (spec §6). Then measure each sweep three times in a debug build (`cargo test -p ivmlite-test --locked --test harness_catches_bugs <name> -- --exact`, and the extension sweeps), and record the medians.

- [ ] **Step 6: Record the enumeration growth** (spec §6, answering parent §9.2 item 4): append to parent spec §9.2 item 4 a paragraph headed `**Amended 2026-10-17 (M2c):**` with the counts (single 153 → 510; join 900 over 2 databases → 1,197 over each of 3), each sweep's measured median before (from the existing text) and after, the total `scripts/test-all.sh` wall time before and after, and the decision the numbers support: the three-column schema is adopted, with the cross restricted as plan decision 13 describes.

- [ ] **Step 7: Gate rows.**
  - New (`## ivmlite-test: generators`):

    | Spec requirement | Mutation | Test that goes red |
    |---|---|---|
    | Plan decision 15: pool values are stored unchanged | Add `Text("12")` to the INTEGER `narrow` pool | `every_pool_value_is_stored_unchanged` |
    | M2c §6: the enumeration covers every new aggregate | Drop `AggFn::Min` from `AGGREGATE_FUNCTIONS` | `the_enumeration_covers_every_aggregate_on_every_column`, `the_enumeration_has_the_recorded_size` |
    | M2c §6: declared types come from the affinity, non-STRICT | In `create_table_sql`, render `Numeric` as `REAL` | `create_table_sql_declares_each_affinity_without_strict` and the sweeps over `gen_real_numeric_database` (record which) |
    | M2c §6: integers near 2^63 are drawn | Empty the `big` pools | `rare_values_are_drawn_but_rarely` |

  - "§7.1 Tables must be created `STRICT`" becomes `n/a — superseded by M2c Task 11 (spec §3)`.
  - **Re-run every row whose red set names or excludes a differential sweep** (the "How to use it" rule for a change to the enumerated query space), including the "`enumerate_join` must only pair same-typed key columns" row (now same-affinity). Update each row's counts and red set from the terminal; mark a row whose red set changed `verified — re-verified in M2c Task 11`.

- [ ] **Step 8: Commit** (two commits: the code, then the measurements and spec amendment)

```bash
git add crates/ivmlite-test/src/data.rs crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/sql.rs \
  crates/ivmlite-test/src/lib.rs crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/ops.rs \
  crates/ivmlite-test/src/test_support.rs crates/ivmlite-test/tests/harness_catches_bugs.rs \
  crates/ivmlite-test/tests/extension_differential.rs docs/mutation-gates.md
git commit -m "test: widen the differential schema to three mixed-type columns and every aggregate

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add docs/superpowers/specs/2026-09-18-ivmlite-design.md
git commit -m "docs: record the widened enumeration's size and cost (parent spec 9.2 item 4)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Order independence of REAL results (spec §6 "Order independence")

**Files:**
- Create: `crates/ivmlite-test/tests/extension_order_independence.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: the extension (Tasks 2–9), `ivmlite_test::open_with_extension`, Task 10's `fsum` and `exact_mean` (exported from `ivmlite_test`; add `pub use exact_reference::{exact_mean, fsum};` to `lib.rs` if Task 10 did not).
- Produces: tests only.

- [ ] **Step 1: Write the tests.**

```rust
//! M2c spec §6 "Order independence": REAL results depend only on the final
//! multiset, never on insertion order or on retractions.

mod common;
use common::*;

use ivmlite_test::{exact_mean, fsum, open_with_extension};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use rusqlite::{params, Connection};

const VIEW: &str = "SELECT k, SUM(x), TOTAL(x), AVG(x) FROM t GROUP BY k";

fn values(rng: &mut StdRng, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| {
            let mantissa = rng.random_range(-(1i64 << 52)..(1i64 << 52)) as f64;
            mantissa * 2f64.powi(rng.random_range(-80..40))
        })
        .collect()
}

fn insert(c: &Connection, xs: &[f64]) {
    for x in xs {
        c.execute("INSERT INTO t VALUES ('a', ?1)", params![x]).unwrap();
    }
}

fn delete_one(c: &Connection, x: f64) {
    let n = c
        .execute("DELETE FROM t WHERE rowid = (SELECT rowid FROM t WHERE x = ?1 LIMIT 1)", params![x])
        .unwrap();
    assert_eq!(n, 1);
}

/// The view's three REAL results, as bits.
fn bits(c: &Connection) -> [u64; 3] {
    c.query_row("SELECT \"SUM(x)\", \"TOTAL(x)\", \"AVG(x)\" FROM v", [], |r| {
        Ok([r.get::<_, f64>(0)?.to_bits(), r.get::<_, f64>(1)?.to_bits(), r.get::<_, f64>(2)?.to_bits()])
    })
    .unwrap()
}

/// A fresh view over exactly `xs`, in the given order.
fn fresh(xs: &[f64]) -> [u64; 3] {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x REAL)").unwrap();
    insert(&c, xs);
    create(&c, "v", VIEW).unwrap();
    bits(&c)
}

fn check(seed: u64, mut all: Vec<f64>, retract: Vec<f64>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x REAL)").unwrap();
    create(&c, "v", VIEW).unwrap();
    insert(&c, &all);
    refresh(&c, "v").unwrap();
    for &x in &retract {
        delete_one(&c, x);
    }
    refresh(&c, "v").unwrap();
    // Reinsert part of what was retracted, in a random order, over several refreshes.
    let mut back: Vec<f64> = retract.iter().copied().filter(|_| rng.random_bool(0.5)).collect();
    back.shuffle(&mut rng);
    for chunk in back.chunks(7) {
        insert(&c, chunk);
        refresh(&c, "v").unwrap();
    }
    for &x in &retract {
        let i = all.iter().position(|&y| y.to_bits() == x.to_bits()).unwrap();
        all.remove(i);
    }
    all.extend(&back);
    let got = bits(&c);
    all.shuffle(&mut rng);
    assert_eq!(got, fresh(&all), "seed {seed}");
    // And both equal the independent reference, bit for bit.
    let n = all.len() as i64;
    assert_eq!(got, [fsum(&all).to_bits(), fsum(&all).to_bits(), exact_mean(&all, n).to_bits()], "seed {seed}");
}

#[test]
fn reinsertion_in_another_order_is_bit_identical() {
    for seed in 0..10 {
        let mut rng = StdRng::seed_from_u64(seed);
        let all = values(&mut rng, 120);
        let mut retract = all.clone();
        retract.shuffle(&mut rng);
        retract.truncate(50);
        check(seed, all, retract);
    }
}

#[test]
fn retracting_the_largest_and_smallest_magnitudes_is_bit_identical() {
    for seed in 0..10 {
        let mut rng = StdRng::seed_from_u64(100 + seed);
        let mut all = values(&mut rng, 80);
        all.extend([1e300, -1e300, 2.5e299, 1e-300, -3e-310, 5e-324]);
        let mut by_magnitude = all.clone();
        by_magnitude.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
        let retract: Vec<f64> = by_magnitude[..4].iter().chain(&by_magnitude[by_magnitude.len() - 4..]).copied().collect();
        check(seed, all, retract);
    }
}
```

The second test's values leave `fsum`'s safe domain only through `1e300`-scale terms, whose partials stay finite (|Σ| < 2^1000); `exact_mean`'s neighbour test stays exact while the mean is normal. If `exact_mean`'s precondition fails for a subnormal mean, drop `5e-324` and `-3e-310` from the reference assertion only (keep them in the bit-identity assertion) and say so in a comment.

- [ ] **Step 2: Run them**

Run: `scripts/build-extension.sh && cargo test -p ivmlite-test --locked --test extension_order_independence`
Expected: PASS (Tasks 3–8 already implement the behaviour; this task is the spec's acceptance test). If one fails, it is a bug in the implementation, to be fixed in the task that owns it, not here.

- [ ] **Step 3: Gate row** (`## ivmlite-sqlite`):

| Spec requirement | Mutation | Test that goes red |
|---|---|---|
| M2c §4/§6: REAL results are order-independent | In `Acc::plus`, replace `sum.frac.add_sum(&b.frac)` with `let r = sum.frac.round() + b.frac.round(); sum.frac = ExactSum::zero(); sum.frac.add(r, 1);` | `reinsertion_in_another_order_is_bit_identical`, `retracting_the_largest_and_smallest_magnitudes_is_bit_identical` |

- [ ] **Step 4: Commit**

```bash
git add crates/ivmlite-test/tests/extension_order_independence.rs crates/ivmlite-test/src/lib.rs docs/mutation-gates.md
git commit -m "test: REAL aggregates are bit-identical whatever the order of inserts and retractions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 13: Measure — integer workloads before/after, and the FluxFlow and Kener demos in place (spec §7 "Performance", "Reporting")

**Files:**
- Modify: `crates/ivmlite-test/src/fluxflow.rs` (DDL `:8-20`, `Flow` `:30-40`, `generated_flow` `:54-76`, `insert_flow` `:78-88`, the handwritten rollup `:182-237`, the covering index `:257-260`, tests), `crates/ivmlite-test/src/demand_cases.rs` (`KENER` `:172-200`, `seed_kener` `:454-…`, `apply_kener`), `docs/demos/fluxflow.md`, `docs/demos/kener.md`
- Create: `docs/bench/m2c-ablation.csv`, `docs/bench/m2c-ablation-builds.csv`, `docs/bench/m2c-ablation-write-amp.csv`, `docs/bench/m2c-confirm-before.csv`, `docs/bench/m2c-confirm-after.csv`, `docs/bench/m2c-demos.csv`, `docs/bench/m2c-demos-builds.csv`, `docs/bench/m2c_tables.py`, `docs/demos/results/2026-10-17-fluxflow-m2c.txt`, `docs/demos/results/2026-10-17-kener-m2c.txt`
- Modify: `docs/bench/README.md` (new section "M2c: column types and aggregates")
- Test: the demos' own unit tests; the demos' multiset oracle

**Interfaces:**
- Consumes: the finished extension (Tasks 1–12). `before` is master at M2c start (`git merge-base master HEAD`; record the SHA), `after` is this branch's head after Task 12.
- Produces: data, scripts and docs only. `m2c_tables.py` (Python 3.12 standard library) prints, per ablation and confirm cell, `after/before` of the steady-state median, flags every integer cell above **1.05**, and exits 1 if its label self-check fails.

**The demos in place** (plan decision 16):
- **FluxFlow:** `flow_facts` becomes non-STRICT with `amount REAL NOT NULL` and `confidence REAL` in place of `sat INTEGER NOT NULL`; `amount = ((hash >> 24) % (1 << 32)) as f64 / 256.0`, `confidence = (hash % 1000) as f64 / 1000.0`; the maintained query is `SELECT flow_type, day_bucket, counterparty_kind, exchange_key, SUM(amount) AS amount, COUNT(*) AS count FROM flow_facts GROUP BY flow_type, day_bucket, counterparty_kind, exchange_key`, with no integer-scaled projection. Every amount is a multiple of 2^-8 below 2^24, so every group's sum stays below 2^53 · 2^-8 and is exact in every summation order: the hand-written trigger rollup (now `amount REAL`), SQLite's recompute and ivmlite agree bit for bit, and the existing exact multiset oracle stays valid. The update statement adds `1.0` to `amount` in place of `+ 7` sat.
- **Kener:** `monitoring_facts` becomes non-STRICT with `latency_ms REAL NOT NULL`; seeds are `(20 + id % 980) as f64 + 0.25 * (id % 4) as f64`, the correction adds `25.0`; the query is `SELECT monitor_id, bucket_ts, status, AVG(latency_ms), MIN(latency_ms), MAX(latency_ms), COUNT(*) FROM monitoring_facts GROUP BY monitor_id, bucket_ts, status`, the docs' slice otherwise unchanged (groups, batch mix, covering index). Group sums are exact, so SQLite's `avg` (rounded sum, then division) equals ivmlite's exact mean, and MIN/MAX have no Int/Real ties.

- [ ] **Step 1: Adapt the demos and their tests.** Update the unit tests in `fluxflow.rs` (`generator_is_deterministic_and_has_all_flow_types`, `handwritten_rollup_tracks_both_rounds_of_mixed_changes`) to the REAL column, and add:

```rust
#[test]
fn every_generated_amount_is_a_dyadic_rational_below_two_pow_24() {
    // Plan decision 16: exact sums in every order keep the exact oracle valid.
    for id in 0..10_000 {
        let a = generated_flow(id).amount;
        assert!(a >= 0.0 && a < 16_777_216.0, "{a}");
        assert_eq!((a * 256.0).fract(), 0.0, "{a}");
    }
}
```

Run: `cargo test -p ivmlite-test --locked fluxflow && cargo run --release --locked -p ivmlite-test --example fluxflow_demo -- 20000 50 1 && cargo run --release --locked -p ivmlite-test --example demand_case_bench -- kener_quarter_hour_rollup 50000 100 1`
Expected: PASS, and both demos verify every mode against SQLite's own evaluation.

- [ ] **Step 2: Integer workloads, before/after.** From the repository root, after building nothing into `target/` by hand:

```bash
caffeinate -i env OUT=docs/bench/m2c-ablation.csv WRITE_AMP_OUT=docs/bench/m2c-ablation-write-amp.csv \
  scripts/bench-ablation.sh before=<before-sha> after=<after-sha>
# The matrix's confirm cells (Phase 6's selection), once per tree:
git worktree add --detach /tmp/ivmlite-m2c-before <before-sha>
(cd /tmp/ivmlite-m2c-before && caffeinate -i scripts/bench.sh confirm --from docs/bench/m1b-phase6.csv) > docs/bench/m2c-confirm-before.csv
caffeinate -i scripts/bench.sh confirm --from docs/bench/m1b-phase6.csv > docs/bench/m2c-confirm-after.csv
git worktree remove /tmp/ivmlite-m2c-before
```

Write `docs/bench/m2c-ablation-builds.csv` (label → commit) by hand, as Phases 4–6 did. Record the one-minute load average at the start and end of each run.

- [ ] **Step 3: The demos, before/after.** The eight integer demos run on both builds from this tree; FluxFlow and Kener changed shape, so their `before` comes from the `before` tree's own harness (its integer slices) and their `after` from this tree:

```bash
DEMOS="demand_case_bench noop_gravity_witness 518400 200
demand_case_bench zcash_transparent_balance 500000 500
demand_case_bench zenod_usage_totals 1760000 200
demand_case_bench nicti_facet_counts 2000000 200
demand_case_bench postio_mailbox_footprint 81000 200
demand_case_bench phoenix_token_rollup 4952148 200
demand_case_bench pocketbase_author_counts 18000 40
demand_case_bench openwhispr_speaker_words 2000000 200" \
  OUT=docs/bench/m2c-demos.csv caffeinate -i scripts/bench-demos.sh before=<before-sha> after=<after-sha>
```

(these are `scripts/bench-demos.sh`'s `default_demos` without `fluxflow_demo` and `kener_quarter_hour_rollup`; if that list changed, use its current entries). Then run FluxFlow and Kener at their documented scale with `DEMOS="fluxflow_demo 1500000 500\ndemand_case_bench kener_quarter_hour_rollup 4100000 500"` once in a `before` worktree (`OUT` into the scratchpad, then append its rows to `docs/bench/m2c-demos.csv` with the label `before`) and once here (label `after`). Save each demo's printed table under `docs/demos/results/2026-10-17-{fluxflow,kener}-m2c.txt`.

- [ ] **Step 4: Tables and the 5% rule.** Write `docs/bench/m2c_tables.py` (standard library only; read the CSVs, print per cell `before`, `after`, `after/before` of the steady-state medians, with min–max). Run it.
  - If every integer cell is ≤ 1.05: say so in the README section.
  - If any integer cell exceeds 1.05 in steady state (and the excess survives one re-run of that cell): profile it (`scripts/profile-refresh.sh`), name the cause, and **stop and report to the controller**, who asks the user before the spec is amended. The spec says "if they do, the spec records why": the amendment is a dated paragraph in spec §7 written only after that approval.

- [ ] **Step 5: Docs.** Every number comes from the CSVs or the script.
  - `docs/bench/README.md`, new section "M2c: column types and aggregates": setup (builds, machine, SQLite version, load averages), the integer-workload before/after table and the 5% verdict, what costs what (the state row is smaller for integer-only groups — plan decision 6 — and the REAL path's cost per group), the demos before/after, and the limits (different shapes for FluxFlow and Kener before and after; machine load).
  - `docs/demos/fluxflow.md` and `docs/demos/kener.md`: the "Adaptation" sections describe the non-STRICT REAL tables and the new queries (no integer scaling; Kener's AVG/MIN/MAX in the view instead of in the application); add the new result tables with their run conditions; keep the old tables, labelled as the alpha.5 integer slices.

- [ ] **Step 6: Checks and commits.** Run the Global Constraints checks. Commit the demo code, then the data and scripts, then the docs:

```bash
git add crates/ivmlite-test/src/fluxflow.rs crates/ivmlite-test/src/demand_cases.rs
git commit -m "feat(demos): FluxFlow and Kener run on their REAL columns, non-STRICT

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add docs/bench/m2c-ablation.csv docs/bench/m2c-ablation-builds.csv docs/bench/m2c-ablation-write-amp.csv \
  docs/bench/m2c-confirm-before.csv docs/bench/m2c-confirm-after.csv docs/bench/m2c-demos.csv \
  docs/bench/m2c-demos-builds.csv docs/bench/m2c_tables.py \
  docs/demos/results/2026-10-17-fluxflow-m2c.txt docs/demos/results/2026-10-17-kener-m2c.txt
git commit -m "bench: M2c integer workloads and demos, before and after

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add docs/bench/README.md docs/demos/fluxflow.md docs/demos/kener.md
git commit -m "docs: M2c measurements

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Documentation, census and release preparation (spec §7 "Reporting", "Release")

**Files:**
- Modify: `README.md` ("Supported view SQL" `:110-…`, the quick start's `STRICT` example `:55-70`, limitations), `CHANGELOG.md` (`[Unreleased]` → `[0.1.0-alpha.6]`), `docs/README.md` (index: the spec, this plan, the new bench files), `docs/superpowers/specs/2026-09-18-ivmlite-design.md` §13 (dated amendments), `docs/census/2026-10-17/README.md` (new section)
- Create: `docs/releases/v0.1.0-alpha.6.md`
- Modify (version bump): `Cargo.toml` (`[workspace.package] version`), `crates/ivmlite-sqlite/Cargo.toml` (`version`), `Cargo.lock`, `crates/ivmlite-sqlite/Cargo.lock`, and every other file `grep -rn "0\.1\.0-alpha\.5" --include='*.rs' --include='*.toml' --include='*.md' .` shows to carry the package version (not history: CHANGELOG entries and old release notes keep their versions)

**Interfaces:**
- Consumes: everything; Task 11's §9.2 amendment; Task 13's numbers.
- Produces: docs and the version `0.1.0-alpha.6`. **No tag, push, merge or publish: the controller asks the user first.**

- [ ] **Step 1: README.**
  - **Supported view SQL:** any base table, STRICT or not, any declared type (affinity by SQLite's rules); aggregates `COUNT(*)`, `COUNT(col)`, `SUM`, `TOTAL`, `AVG`, `MIN`, `MAX`; a comparison's literal may be an integer, a real, a string or `X'…'` and is converted by the column's affinity as SQLite does, except that a REAL literal against a TEXT-affinity column is refused; join columns must have the same affinity, and `1` matches `1.0`.
  - **Differences from SQLite, stated plainly** (spec §7): "REAL results of `SUM`, `TOTAL` and `AVG` are the exact result rounded once, so they can differ from SQLite's in the last bits: SQLite sums in scan order with Kahan–Babuška–Neumaier and divides after rounding the sum (for example `AVG` of 0.1, 0.2 and 0.3 is `0.2` in ivmlite and `0.19999999999999998` in SQLite)." "Integer overflow: ivmlite's `SUM` fails the refresh with `integer overflow in SUM` only when the final exact sum is outside the 64-bit range; SQLite reports `integer overflow` when a prefix of its scan overflows, even if the final sum fits." Also: MIN of an INTEGER and an equal REAL returns the INTEGER and MAX the REAL, where SQLite's choice depends on scan order; `+Inf` and `−Inf` in one sum give NULL, as in SQLite.
  - **Limitations:** BINARY collation only; no expressions in GROUP BY or SELECT, no views without aggregates, at most one predicate (M2d); no `DISTINCT` aggregates or `group_concat`; tested on SQLite 3.53.2 and documented for SQLite 3.45 and later.
  - Update the quick start: its table may drop `STRICT`.
- [ ] **Step 2: Parent spec §13.** Add `**Amended 2026-10-17 (M2c):**` notes: item 2 lifted (non-STRICT and every type, by affinity; `COLLATE` still refused); item 5 lifted (exact floating-point aggregation, rounded once); item 6 replaced by the final-sum overflow rule; item 8 lifted (aggregates over any column; literals converted by affinity, except REAL vs TEXT); item 9 amended ("of the same affinity", canonical key equality); item 10 amended (MIN/MAX supported; DISTINCT still not).
- [ ] **Step 3: Census.** Add "After M2c" to `docs/census/2026-10-17/README.md`: for each application in `apps.json`, it fits when every feature it still requires is among F3a, F3b, F3c, F3e, F10a and F10b (and its blocking reasons are none of F13, F14, F15, not-ivm-shaped). The README's table predicts 10: better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod, and kener; org_roam still needs `group_concat` (F3f). Recompute from `apps.json` and list what the data says, with each candidate's remaining in-place rewrites from `apps.md`; mark low-confidence items as the census does. For Nexmark, recompute the "+ F10b (REAL)" row the same way.
- [ ] **Step 4: CHANGELOG, release notes, version.**
  - `CHANGELOG.md`: move `[Unreleased]` to `## [0.1.0-alpha.6] - <date of the bump>` with **Breaking** (format 6: databases created by alpha.5 are refused, and their views open broken and can be dropped; recreate them), **Added** (non-STRICT tables and every type; REAL and BLOB values; `TOTAL`, `AVG`, `COUNT(col)`, `MIN`, `MAX`; exact REAL `SUM`; REAL and BLOB literals with comparison affinity; the widened differential tests), **Changed** (SUM over any column; integer overflow judged on the final sum; group keys by SQLite's equality), and the headline numbers from Task 13.
  - `docs/releases/v0.1.0-alpha.6.md` in alpha.5's style, with the breaking note and the two documented differences from SQLite.
  - Bump the version to `0.1.0-alpha.6` in both `Cargo.toml`s, then refresh both lock files with `cargo update -p ivmlite-core -p ivmlite-sql -p ivmlite-test -p ivmlite-bench -p ivmlite-workload --offline` (workspace) and `cargo update -p ivmlite-sqlite -p ivmlite-core -p ivmlite-sql --offline --manifest-path crates/ivmlite-sqlite/Cargo.toml`; check with `git diff --stat` that only version lines changed in the lock files.
  - `docs/README.md`: index the M2c spec, this plan, the bench CSVs and `m2c_tables.py`, and the release notes.
- [ ] **Step 5: Checks.** The Global Constraints checks; `python3 scripts/count-mutation-gates.py` prints `consistent`; `grep -rnP '[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]' --include='*.rs' --include='*.md' --include='*.py' --include='*.toml' . | grep -v '^./target'` prints nothing (English only); `cargo run --release --locked -p ivmlite-test --example release_smoke` passes if that example covers the release.
- [ ] **Step 6: Commit** (docs, then the version bump):

```bash
git add README.md CHANGELOG.md docs/README.md docs/releases/v0.1.0-alpha.6.md \
  docs/superpowers/specs/2026-09-18-ivmlite-design.md docs/census/2026-10-17/README.md
git commit -m "docs: M2c — supported SQL, differences from SQLite, census, alpha.6 notes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add Cargo.toml Cargo.lock crates/ivmlite-sqlite/Cargo.toml crates/ivmlite-sqlite/Cargo.lock   # plus each file the grep showed, by name
git commit -m "chore: version 0.1.0-alpha.6

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Hand back to the controller.** Tagging `v0.1.0-alpha.6`, pushing the branch, opening the PR and publishing the release are **not** done here: the controller asks the user first.

---

## Self-review

### Spec coverage

| Spec section / requirement | Tasks |
|---|---|
| §1 item 1: accept non-STRICT tables and every type by affinity | 7 |
| §1 item 2 / §3 "Shadow tables": carry every storage class unchanged | 1 (values, encoding), 2 (typeless shadow tables), 7 (capture) |
| §1 item 3 / §2 "Two equalities", group canonicalization | 1 (`canonical`, identity order), 5 (group keys), 9 (join keys) |
| §2 "The type a group key is output as" (real-key count; oracle tolerance) | 5, 10 (comparator matches by canonical key) |
| §2 order NULL < numeric < TEXT < BLOB | 1 (`sql_cmp`), 8 (encoding), 9 (filters) |
| §2 a Real is never NaN; decoder refuses one | 1 (`encode.rs`, `value_of`), 8 (`from_sort_key`) |
| §3 affinity rules (VARCHAR, DATETIME, BOOLEAN, NUMERIC, FLOAT, DOUBLE, none, BLOB; STRICT and ANY) | 7 |
| §3 non-BINARY collation still refused | 7 (unchanged check, kept in the rejected list), 14 (README) |
| §3 the shape records declared type and affinity | 7 |
| §3 encoding tags REAL and BLOB | 1 |
| §3 format 6, format 5 refused, broken views droppable | 2 |
| §4 "How a value counts" (`sqlite3_value_numeric_type`, `'1.5'`, `'abc'`, BLOB) | 4, 6 |
| §4 accumulator state: `non_null`, `approx`, `int_sum: i128`, `frac` superaccumulator (range, BLOB, present only while `approx > 0`, exact add/sub) | 3, 6 |
| §4 outputs: SUM (overflow message), TOTAL, AVG (exact long division), COUNT(x), COUNT(*) | 6 |
| §4 "Where this differs" (last bits; final-sum overflow) | 6 (tests), 14 (README, release notes) |
| §4 MIN/MAX state table, encoding, indexed read, ties, empty groups | 8 |
| §5 filters: literal affinity conversion, REAL and BLOB literals, operators keep meaning | 1 (`passes`), 9 |
| §5 joins: same affinity (message names both), canonical key equality, NULL matches nothing | 9 |
| §6 differential: three columns, five declared types, mixed values (integral reals, -0.0, near 2^53 and 2^63, convertible and non-convertible TEXT, BLOB), every new aggregate, checked after every batch | 11 |
| §6 INTEGER/TEXT/count/NULL equal SQLite exactly; REAL bit-exact vs Shewchuk reference; KBN bound vs SQLite | 10 |
| §6 report how much larger the enumeration became (parent §9.2 item 4) | 11 |
| §6 order independence (reinsertion; largest and smallest magnitudes) | 12 |
| §6 superaccumulator property tests (identity, commutativity, i128-piece reference, subnormals, ±max, cancellation, 2^60 terms) | 3 |
| §6 MIN/MAX tests (retract current extreme, ties, cross-class, encoding order property, query plan) | 8 |
| §6 non-STRICT capture through INSERT, UPDATE, REPLACE, each affinity, `typeof()` and value | 7 |
| §6 format test tampered to 5 | 2 |
| §6 gate rows: canonicalization, real-key count, overflow check, `approx` switch, `exact`'s subtraction, MIN/MAX tie order, literal affinity conversion, join-affinity check | 5, 5, 6, 6, 3, 8, 9, 9 (plus every other new rule in its task) |
| §7 no regression > 5% on integer workloads (ablation and confirm cells, before/after) | 13 |
| §7 FluxFlow SUM over REAL without scaling; Kener AVG/MIN/MAX over REAL; docs report before/after | 13 |
| §7 bench README section; census update listing what now fits | 13, 14 |
| §7 release alpha.6: CHANGELOG, release notes, format 6; README limitations and parent §13 amended, REAL difference and overflow rule stated plainly | 14 |

### Placeholder scan

Searched for "TBD", "TODO", "implement later", "appropriate", "similar to Task", "fill in": none. One place names files the implementer finds with a given command rather than a list: the files carrying the package version (Task 14's `grep`). Measured numbers (gate counts, timings, the census list) are produced by the tasks, never written in advance.

### Type consistency

Checked across tasks: `Value::{Null, Int, Real, Text, Blob}`, `Value::canonical`, `Value::sql_cmp` (Task 1); `ExactSum::{zero, is_zero, add, add_int, add_sum, round, div_round, to_be_bytes, from_be_bytes}`, `EXACT_LIMBS`, `EXACT_BYTES` (Task 3, used in 6); `NumericTerm::{Integer, Approx}`, `numeric_term`, `numeric_affinity` (Task 4, used in 6, 9, 10); `ColumnType::{Integer, Real, Numeric, Text, Blob}` and `ColumnType::name` (Task 6, used in 7, 9, 11); `AggFn::{Count, Sum, Total, Avg}` (6) plus `Min`, `Max` (8); `SUM_OVERFLOW` (6, used in 10); `AggState::new(group_by, aggs, state, extremes)` (8; Tasks 5–6 use the three-argument form that Task 8 extends, and Task 8 updates every caller); `ArrangementRole::AggregateExtremum(usize)`, `Arrangement::first` (8); `sort_key`, `from_sort_key` (8); `ValueCodec`, `codec`, `first_sql`, `BufferedArrangement::new(conn, table, pending, codec)` (8); `affinity`, `CaptureInfo::declared_types` (7); `fsum`, `int_pieces`, `exact_mean`, `UNIT_ROUNDOFF` (10, used in 12); `SqliteTerms`, `Evaluated`, `evaluate`, `compare_with_oracles`, `Outcome`, `run_outcome` (10, used in 11); `value_pool`, `Pool`, `gen_real_numeric_database`, `create_table_sql` (11). Stored layouts: group row `[rows, real_keys, …]` from Task 5, SUM/TOTAL/AVG `[non_null, int_sum, approx]` from Task 6 (the Task 5 fixtures are rewritten in Task 6, said there).
