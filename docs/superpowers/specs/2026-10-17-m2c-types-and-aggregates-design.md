# M2c: column types, non-STRICT tables and more aggregates

**Status:** approved 2026-10-17.

**Parent specs:**
- [2026-09-18-ivmlite-design.md](2026-09-18-ivmlite-design.md):
  - §7.1, the two semantic traps in row encoding;
  - §11 M2;
  - §11 M4: "floating-point aggregation + a tolerance strategy" and MIN/MAX are brought forward here;
  - §13, items 2, 5, 6, 8 and 10, which this phase lifts.
- The Phase 3a, 3b, 5, 6 and 7 extension specs. Their conventions all still hold.

**Evidence:** [the 2026-10-17 SQL-subset census](../../census/2026-10-17/README.md).

## 1. Scope

**Motivation.** 27 of the 28 candidate applications in the census use non-STRICT tables, and 17 use column types outside INTEGER and TEXT. Accepting them, with floating-point aggregation, is the change that unlocks the most applications.

**This phase does five things:**
1. **Accept non-STRICT base tables** and every column type, judged by SQLite's affinity rules (§3).
2. **Carry every storage class** (NULL, INTEGER, REAL, TEXT, BLOB) through capture, state and output, unchanged (§2, §3).
3. **Group, compare and join** with SQLite's own value semantics (§2, §5).
4. **Aggregates:**
   - `SUM` over any column, with exact accumulation;
   - `TOTAL`;
   - `AVG`;
   - `COUNT(col)`;
   - `MIN` / `MAX`, retraction-safe (§4).
5. **Widen the differential tests** to three columns per table with mixed-type values. This answers the parent spec's §9.2 item 4, the last open M2 item (§7).

**Not in scope:**
- Collations other than BINARY.
- Expressions in GROUP BY or SELECT.
- Views without aggregates.
- A richer WHERE.

  These three belong to M2d, which builds on this phase's value semantics.
- `DISTINCT` aggregates and `group_concat`.
- Joins between columns of different affinity.
- Retention or sealing.

## 2. Values

**The value domain.** `ivmlite_core::Value` gains `Real(f64)` and `Blob(Vec<u8>)`. SQLite never stores a NaN; it stores NULL instead. So a `Real` here is never NaN, and the decoder refuses one as corrupted state.

**Two equalities, kept apart:**
- **Row identity**, which Z-set consolidation uses, is exact by storage class. `Int(1)`, `Real(1.0)` and `Text("1")` are three different rows, and two `Real`s are equal only bit for bit. A captured DELETE carries exactly the values its INSERT carried, so they cancel.
- **Group equality** is SQLite's GROUP BY equality: integers and reals compare numerically, `'1'` differs from `1`, and NULLs form one group. It is implemented by **canonicalizing** each group-key value before it reaches the aggregate's arrangement:
  - a `Real` that is integral and within the `i64` range becomes `Int`;
  - `-0.0` becomes `Int(0)`;
  - every other value is unchanged.

  Canonical values then compare by exact equality.

**The type a group key is output as.**
- **The rule:** for each numeric group-key column, the group keeps a count of its rows whose key value was a `Real`. If that count is positive, the key is output as `Real`; otherwise, as `Int`.
- **Agreement with SQLite:** SQLite outputs the key value of an arbitrary row of the group. So this matches SQLite whenever SQLite's own answer is determined, that is, when every row stores the key as the same type. When a group mixes the two, ivmlite outputs `Real`.
- **Testing:** the oracle comparison treats an `Int` and a `Real` with equal numeric values as equal in that case.

**Order** (§4 MIN/MAX and §5 comparisons) is SQLite's cross-class order:

NULL < numeric < TEXT < BLOB

- numerics compare by exact numeric value, `Int` against `Real` included;
- TEXT compares with `memcmp` (BINARY);
- BLOB compares with `memcmp`.

## 3. Tables and storage

**Base tables.**
- **STRICT is no longer required.** Every declared type is accepted, and its affinity is determined by SQLite's rules (datatype3.html §3.1). `VARCHAR(255)` is TEXT; `DATETIME`, `BOOLEAN` and `NUMERIC` are NUMERIC; `FLOAT(8,2)` and `DOUBLE` are REAL; a column with no type, or `BLOB`, has BLOB affinity. STRICT tables, including those with `ANY` columns, are accepted too.
- **Collations.** A non-BINARY collation is still refused, by the existing conservative `COLLATE` check.
- **The table's shape** records each column's declared type and affinity. Phase 3b's latch therefore still breaks a view whose base table changes its definition.

**Shadow tables.** These columns are declared with no type, so SQLite applies no affinity and stores every value with its storage class unchanged:
- every column of the delta tables, the stage, the state tables and the output table that holds a value, as opposed to bookkeeping such as `seq` or `w`;
- the pend tables' columns.

**Encoding.** `encode.rs` gains two tags:
- `REAL`: 8 bytes of the `f64`'s bits, big-endian;
- `BLOB`: a 4-byte length, then the bytes.

**Format.** The shadow-table format becomes **6**. A format-5 database is refused, and its views still open as broken views that can be dropped (Phase 3a §5).

## 4. Aggregates

All of the following follow SQLite's `sumStep` / `sumFinalize`, `avgFinalize`, `totalFinalize`, `countStep` and `minmaxStep`. The behaviour tested is SQLite 3.45 and later, and §6 pins the exact versions.

**How a value counts.** Each non-NULL value is classified by `sqlite3_value_numeric_type`. This applies numeric affinity to a TEXT value without changing what is stored:
- an INTEGER, or a TEXT value that converts to an integer, adds its integer value;
- anything else adds `sqlite3_value_double(value)`, and the group is then *approximate*. Examples: `'1.5'`, `'abc'`, which counts as 0.0, and a BLOB.

**Accumulator state per group and per SUM/TOTAL/AVG argument:**
- `non_null`: the number of non-NULL inputs, by weight;
- `approx`: the number of inputs classified as non-integer, by weight;
- `int_sum: i128`, the exact sum of the inputs classified as integers. Weights and values both fit in 64 bits, so their products fit in 128.
- `frac`: the exact sum of the inputs classified as non-integer. It is a fixed-point superaccumulator:
  - its range covers every finite `f64` (2^-1074 to 2^1024), with headroom for 2^64 terms, about 2,200 bits;
  - it is stored as a BLOB in the state row;
  - it is present only while `approx > 0`. When `approx` returns to 0, every non-integer input has been retracted, so it is exactly zero and is dropped;
  - adding and subtracting are exact.

The state therefore depends only on the multiset of live inputs, never on their order or on retractions. The exact sum is `int_sum + frac`.

**Outputs:**

| Aggregate | Output |
|---|---|
| `SUM(x)` | NULL if `non_null = 0`. Otherwise, if `approx = 0`: `int_sum` as INTEGER, or the refresh fails with `integer overflow in SUM` if it is outside `i64`. Otherwise: the exact sum, rounded once to the nearest `f64` (ties to even), as REAL. |
| `TOTAL(x)` | The exact sum rounded once, as REAL; `0.0` when `non_null = 0`. |
| `AVG(x)` | NULL if `non_null = 0`. Otherwise the exact sum divided by `non_null`, as REAL. The division is exact long division of the fixed-point sum, and the quotient is rounded once. |
| `COUNT(x)` | `non_null`, as INTEGER. |
| `COUNT(*)` | Unchanged. |

**Where this differs from SQLite.**
- **Last bits of floating-point results.** SQLite sums doubles with Kahan–Babuška–Neumaier, which depends on scan order, and divides after rounding the sum. ivmlite returns the correctly rounded exact result, which is order-independent. The two can differ in the last bits of a REAL result, and the README and §7 say so.
- **Integer overflow.** SQLite reports `integer overflow` when an intermediate integer prefix overflows, even if the final sum would fit. ivmlite judges only the final exact sum.

**MIN / MAX.**
- **State.** Each `MIN(x)` or `MAX(x)` keeps its own state table, an arrangement from the group key to a multiset of the live non-NULL values. Its primary key is `(key, val)` and it holds a weight, like the other state tables.
- **Encoding of `val`.** `val` is encoded so that `memcmp` order equals §2's cross-class order:
  1. a class byte: numeric, then TEXT, then BLOB;
  2. for numerics, 8 bytes of order-preserving `f64` (the value, rounded to nearest for a large `Int`), then 8 bytes that break ties exactly: the `i64` for an `Int`, or a marker for a `Real`;
  3. for TEXT and BLOB, the raw bytes, with a terminator that keeps prefixes ordered.

  `Int` and `Real` entries that are numerically equal stay distinct entries.
- **Reading the result.** The group's result is read with one indexed `SELECT val … WHERE key = ? ORDER BY val LIMIT 1`, or `DESC` for MAX. It is output as its original storage class.
- **Ties.** When the smallest value appears both as `Int` and as `Real`, the `Int` entry sorts first and is output. SQLite's choice depends on scan order there.
- **Empty groups.** NULL, when the group has no non-NULL value.

## 5. Filters and joins

**Filters.** A predicate compares a column with a literal. The literal's affinity conversion is applied at compile time, following SQLite's comparison rules (datatype3.html §4.2):
- for a column of INTEGER, REAL or NUMERIC affinity, a TEXT literal gets numeric affinity;
- for a TEXT-affinity column, a numeric literal becomes TEXT;
- a BLOB-affinity column converts nothing.

The predicate is then evaluated with §2's order. Every operator v0 accepts keeps its meaning. A REAL literal is now accepted, and so is a BLOB literal (`X'…'`).

**Joins.**
- **Affinity.** The two join columns must have the same affinity. Otherwise the view is refused, with a message naming both affinities.
- **Key equality.** Join keys are canonicalized like group keys, so `1` matches `1.0`, as in SQLite.
- **NULL keys** still match nothing.

## 6. Tests

**Differential testing (layer 2), widened.**
- The schema becomes three columns per table, with declared types drawn from INTEGER, REAL, TEXT, NUMERIC and none.
- Values are drawn across storage classes, including:
  - integral reals;
  - `-0.0`;
  - large integers near 2^53 and 2^63;
  - TEXT that converts to a number, and TEXT that does not;
  - BLOB.
- The enumeration covers every new aggregate.
- Each case is checked after every batch, against SQLite's own evaluation of the same SQL and against an independent reference:
  - INTEGER and TEXT results, counts and NULLs must equal SQLite's exactly.
  - Each REAL SUM, TOTAL or AVG result must equal, bit for bit, the correctly rounded exact result. That result comes from an independent reference in the test crate: Shewchuk's exact summation, the algorithm behind Python's `math.fsum`. It is not the extension's superaccumulator.
  - Each REAL result must agree with SQLite's within KBN's error bound, `|ivmlite − sqlite| ≤ 4ε · Σ|xᵢ|`. A difference beyond that bound is a test failure to investigate, not a tolerance to widen.

  The report records how much larger the enumeration became, which answers parent §9.2 item 4.

**Order independence.**
- Insert a multiset, delete part of it, and reinsert part of it in a random order. The REAL output must be bit-identical to that of a fresh view over the same final multiset.
- The same with retractions of the largest and smallest magnitudes.

**The superaccumulator.** Property tests:
- `add(x)` then `sub(x)` is the identity;
- results are commutative;
- rounding is correct against an arbitrary-precision reference built in the test from `i128` pieces;
- extremes: subnormals, ±max finite, cancellation to zero, and 2^60 equal terms.

**MIN/MAX:**
- retracting the current min or max, ties, and a min across classes (TEXT vs numeric vs BLOB);
- the encoding's order, as a property test against §2's comparator;
- the result must be read through the index: check the query plan.

**Non-STRICT capture:**
- a value of each storage class written through an INSERT, an UPDATE and a REPLACE into a column of each affinity is captured exactly. Check `typeof()` and the value in the delta table.

**Format.** The format test is tampered to 5.

**Gate rows.** Every new rule gets a verified gate row, including:
- dropping the canonicalization;
- dropping the real-key count;
- the overflow check;
- the `approx` switch;
- `exact`'s subtraction;
- the MIN/MAX tie order;
- the literal's affinity conversion;
- the join-affinity check.

## 7. Measurement and release

**Performance.**
- **No regression on integer workloads.** Run the Phase 6 ablation cells and the matrix's confirm cells, `before` (master at M2c start) against `after`. Integer-only workloads must not get slower by more than 5% in steady state; if they do, the spec records why.
- **The demos in place.** The FluxFlow demo runs `SUM(amount)` over its REAL column, without the integer-scaled projection. The Kener demo runs AVG, MIN and MAX over its REAL latency column, keeping the docs' slice, and the docs report the before/after.

**Reporting.** A new section in `docs/bench/README.md`, and an update to the census. The census lists which candidates now fit.

**Release:** `v0.1.0-alpha.6`, with a CHANGELOG entry, release notes and format 6. The README's limitations list and the parent spec §13 are amended. They must state the REAL-result difference from SQLite and the overflow rule plainly.

## Amendments 2026-10-17 (from implementation planning)

**Amended 2026-10-18 (intermediate REAL overflow):** SQLite's running KBN
accumulator can produce ±Inf, or NULL after intermediate infinity cancellation,
from finite inputs whose exact final SUM, TOTAL or AVG is finite. These explicit
overflow cases are checked against the exact reference alone when SQLite returns
±Inf or NULL, the reference result is a finite REAL, and the reference magnitude
Σ|xᵢ| is at least `f64::MAX / 4`. This narrowly bounded exception implements the
pre-flight controller's R26, R33 and R34; it does not change the finite KBN error
bound or permit an ordinary-magnitude finite result to match SQLite infinity or
NULL. Named overflow tests construct the exact expected result separately because
`evaluate`'s `fsum` partials must remain finite. Normal generated REAL values stay
within |x| ≤ 2^1000 (the current pools are below 2^64).
The named finite-input overflow cases return ±Inf on bundled SQLite 3.53.2;
NULL handling is covered by synthetic comparator tests for compatibility, without
claiming a finite-input NULL reproducer on that version.

Planning found seven places where this spec is incomplete or cannot be implemented as worded. Each is resolved in `docs/superpowers/plans/2026-10-17-m2c-types-and-aggregates.md` ("Plan decisions"); this section makes those resolutions binding.

1. **§4, MIN/MAX encoding.** A fixed 8-byte "marker for a Real" cannot order every pair exactly (`Int(i64::MAX)` against `Real(2^63)`, or `Real(2^60)` between `Int(2^60)` and `Int(2^60 + 1)`), and `-0.0` needs its own entry. Numeric entries are instead: class byte, 8-byte order-preserving `f64`, 8-byte order-preserving `i64` tie (a Real's tie is its value truncated toward zero and saturated), then one byte: `0x00` Int, `0x01` Real, `0x02` Real `-0.0`. TEXT and BLOB escape each `0x00` payload byte as `0x00 0xFF` and end with `0x00 0x00`. MAX in a numeric tie outputs the entry that sorts last. (Plan decisions 1–3.)
2. **§4, versions.** The tests run on the bundled SQLite 3.53.2; the supported behaviour remains "3.45 and later".
3. **§4, the MIN/MAX read** steps `ORDER BY val` lazily and stops at the first live entry instead of `LIMIT 1`, because a refresh's buffered writes may already retract the stored extremum (Phase 3a §5). (Plan decision 11.)
4. **§5, literals against a TEXT-affinity column.** An INTEGER literal becomes its decimal text; a REAL literal is refused at compile time, because SQLite's REAL-to-TEXT rendering differs between versions. (Plan decision 7.)
5. **§4, infinities.** SQLite stores ±Inf in REAL columns. SUM, TOTAL and AVG count +Inf and −Inf inputs by weight outside the fixed point: both signs present gives NULL (SQLite's NaN), one sign gives that infinity. (Plan decision 5.)
6. **§6, integer results near 2^63.** "INTEGER results equal SQLite's exactly" applies when SQLite completes. When SQLite reports an intermediate (prefix) overflow, the case is checked against the independent reference only; when the reference's final exact sum overflows, ivmlite must fail with `integer overflow in SUM`. (Plan decision 14.)
7. **§6, the AVG bound.** For AVG the KBN check is `4ε·Σ|xᵢ|/n + 2ε·|avg|`, with ε = 2^-53. (Plan decision 12.)
