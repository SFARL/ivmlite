# M2d query-shape acceptance

This is unreleased source-build validation for arithmetic aggregate arguments,
static AND filters, nonaggregated row bags, and final SELECT expressions. The
[design](../../superpowers/specs/2026-10-24-m2d-query-shapes-design.md) defines
the bounded grammar; [README](../../../README.md#supported-view-sql) describes
the public contract. Published alpha.6 archives and historical M2b/M2c evidence
are unchanged. Source builds use **format 7** and require rebuilding old views
from their base tables.

## Recorded result (2026-10-24)

Local macOS arm64 acceptance passed:

| Gate | Result |
|---|---|
| Fresh-extension full suite | 907 passed, 0 failed, 3 existing ignored |
| M2d native integration | 24 tests, including 361 scalar input pairs |
| Compiled fault controls | 6 compiled red/green pairs; exact source hashes restored |
| Format and clippy | Both workspaces passed, warnings denied |
| Minimum SQLite 3.45.0 CLI | 84 executed CHECKs, recursive triggers OFF and ON; 72 committed check rows |
| Pinned Nexmark / Python SQLite 3.53.1 | 24 original SELECT attempts; 3 SQLite ports, 24 typed checkpoints |
| Existing Kener / native SQLite 3.51.2 | All 6 acceptance sections passed; source-boundary 128 assertions and 8 existing focused controls |

The six new compiled controls independently break scalar addition, right-hand
AND validation, negative multiplicity, INTEGER/REAL identity, coherent key
selection and host numeric-literal parsing. Every mutation compiled, its focused
test failed, then restoration rebuilt and passed. The complete suite above ran
after restoration. The two review reproducers' original failures are retained.
No new performance measurement or production-readiness claim is made.

The [compressed evidence](evidence.json.gz) contains structured results and
runtime/library identities; [captured logs and SQL](logs.json.gz) retain 80
records. [Provenance](provenance.json) binds tested Rust/Cargo inputs and each
original/normalized record; [SHA256SUMS](SHA256SUMS) binds publication files.
Only local path spellings are replaced with named placeholders. Binaries and
databases remain outside the repository. These were working-tree executions
before the delivery commit; file hashes bind the code independently of HEAD.

## Nexmark coverage and M2b boundary

All Q0–Q23 original defining SELECTs from
`6b3646c3baec701f1fa74baf938d235f742e5d3c` were submitted under the declared
SQLite fixture schema. **2/24 compile as written: Q0 and Q1**. Q20 compiles after
adding unique output aliases. All three ports passed duplicate/fanout updates,
bulk retraction, savepoint rollback, pending-delta reopen, explicit refresh and
DROP cleanup against a separate SQLite oracle and independent intended bases.

| Query or group | Remaining boundary |
|---|---|
| Q0 | SELECT unchanged; Flink runtime/sink and event time are replaced by SQLite fixtures |
| Q1 | SELECT unchanged; SQLite REAL multiplication differs from Flink DECIMAL sink rounding |
| Q20 | Duplicate dateTime/extra result names require unique sink-field aliases |
| Q2 | MOD function and expression predicate |
| Q3 | OR/IN predicates |
| Q10 | DATE_FORMAT and filesystem sink semantics |
| Q14 | CASE, time/string functions and richer predicates |
| Q21 | CASE, lower/regex functions and OR/IN |
| Q22 | SPLIT_INDEX |
| Remaining queries | Subqueries, windows, expression group keys, filtered/distinct aggregates, temporal or multi-table joins |

The historical seven-to-nine-query feature bundle assumed entire F4b/F6
categories. Bounded M2d arithmetic/static AND does not implement those categories
in full. These are SQLite relational ports, **not canonical Nexmark coverage or
a benchmark score**. The [historical pinned Turso preflight](../../bench/m2b-compatibility.md)
remains separate; no new Turso execution or general cross-engine semantic-domain
proof was performed. M2b still needs an explicitly verified common workload and
comparable complete write/maintenance/commit costs.

## Semantic boundaries

The native catalog parses REAL literals using the host SQLite, and the scalar
evaluator executes only resolved, bound arithmetic projections. Exact aggregate
accumulation retains its documented rounding/overflow differences from SQLite.

New grouped projections retain full typed key tuples with live counts and
choose the least tuple in ivmlite's exact typed `Row` order. All final SELECT
key references use that same actual live tuple. For numerically equal
INTEGER/REAL group keys this can differ from SQLite's chosen row, including
its single MIN/MAX rule; type-sensitive key arithmetic can differ numerically.
The tests check this declared rule explicitly rather than normalize away the
disagreement. Existing aggregate-only plans retain their previous layout and
REAL-key preference.

Two independent review counterexamples became regression tests: synthetic
`(REAL,REAL)` keys from `(REAL,INTEGER)` / `(INTEGER,REAL)` source tuples, and
one-ULP differences between Rust and host SQLite parsing a long decimal.
Retractions, duplicate counts and corruption checks include the additional
representative arrangement.

## Reproduce

Build and run the integration suite using the repository wrapper so the tests
load a fresh extension:

```sh
scripts/test-all.sh
cargo fmt --all -- --check
cargo fmt --manifest-path crates/ivmlite-sqlite/Cargo.toml -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --manifest-path crates/ivmlite-sqlite/Cargo.toml --all-targets --locked -- -D warnings
```

The following probes require external scratch/evidence directories. Set
`PYTHON` to a Python linked to SQLite >=3.45 with loadable extensions enabled,
`EXTENSION` to the freshly built native library, and `NEXMARK` to a clean
checkout at `6b3646c3baec701f1fa74baf938d235f742e5d3c`:

```sh
"$PYTHON" scripts/probe-m2d.py --nexmark-repo "$NEXMARK" \
  --extension "$EXTENSION" --scratch "$SCRATCH" --output "$EVIDENCE/nexmark.json"
"$PYTHON" scripts/probe-m2d-minimum.py --sqlite-cli "$SQLITE345" \
  --extension "$EXTENSION" --output "$EVIDENCE/minimum.json"
```

The minimum probe requires the native SQLite **3.45.0** CLI, not the Python
module's SQLite. The [existing source-verification recipe](../selective-reads/compatibility/README.md#reproduce-in-a-scratch-directory)
pins the official archive, amalgamation hash and source ID. The two runners
record separate runtime/library identities; their checks must not be described
as one campaign on every SQLite version.

The Nexmark runner submits all 24 original defining SELECTs, then executes the
Q0/Q1/Q20 SQLite ports against a separate file-backed ordinary SQLite oracle.
Every checkpoint also compares both databases' complete base rows with an
independently maintained intended ledger. A deliberate wrong base write must
be caught even while IVM agrees with recomputation on the same wrong database.
There is no Flink generator, watermark, sink, stream-time execution or timing
score. Q1 uses SQLite REAL instead of Flink DECIMAL sink rounding; Q20 only adds
unique output aliases, without the old GROUP BY/COUNT multiplicity rewrite.

Minimum-version assertions use typed bags with multiplicities and direct REAL
values, plus encoded TEXT/BLOB payloads. `.bail on` makes every failed SQL
CHECK fatal. Assertions inside rolled-back transactions execute but their check
rows are rolled back too; executed checks and surviving check rows are recorded
separately. Expected underflow failures are explicit nonzero CLI stages, followed
by state/output/watermark/delta equality checks and a successful retry.
