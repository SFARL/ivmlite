# Selective-read affinity evidence

These records were captured before publication. The scripts below are the executed
scripts, retained byte for byte. Run from a scratch directory so captured records
remain intact:

```sh
python3 /path/to/affinity/probe.py > affinity-rerun.json
python3 /path/to/affinity/plan_probe.py > expression-plans-rerun.json
```

Both probes use in-memory databases. `probe.py` records the actual Python SQLite
runtime; changing that runtime changes the evidence. `rejected-candidates.json`
is a retained diagnostic record; no standalone replay script is included for it.
File hashes and the original upstream source hashes are in
`../compatibility/provenance.json`. Upstream C files are linked, not bundled.

## Observed runtime evidence

Python SQLite 3.53.0, source ID
`2026-04-09 11:41:38 4525003a53a7fc63ca75c59b22c79608659ca12f0131f52c18637f829977f20b`.
The pool contains 379 entries: 41 INTEGER, 53 REAL, 276 TEXT, 8 BLOB, 1 NULL.
It includes adjacent values at 2^51, 2^53 and i64 boundaries, infinity,
subnormal REAL, signed zero, malformed/valid numeric TEXT, embedded NUL,
whitespace, and 64/512/4096-character parser cases. Duplicate pool entries
remain separate rows. Counts below are tested ordered row pairs, not unique
values or a claim to exhaustive SQLite coverage.

Every candidate is tested under all five RHS declared affinities, with both
an untyped-column LHS and a unary-plus no-affinity LHS. Every candidate except
the INTEGER equality specialization is tested for all five supported operators.
The no-affinity LHS is a conservative additional case, not a claim that
`xBestIndex` exposes constraints on that expression.

| Candidate | Eligible row pairs | Original true matches | False negatives |
| --- | ---: | ---: | ---: |
| Generic 7 arms, all RHS storage classes | 7,182,050 | 2,883,770 | 0 |
| NUMERIC RHS 3 arms, INTEGER or REAL values | 1,781,300 | 718,151 | 0 |
| BLOB RHS direct comparison | 151,600 | 60,560 | 0 |
| NULL RHS false placeholder | 18,950 | 0 | 0 |
| INTEGER RHS equality with TEXT-region numeric CAST | 124,312 | 1,499 | 0 |

`results.json` retains every affinity/operator/mode result and exact runtime
identity. These are scalar SQLite oracle probes. They do not execute an
ivmlite extension, a foreign virtual table, or SQLite 3.45.0.

The schema-declaration rewrite is only a fixture technique to keep storage
class independent of declared affinity. `automatic_index=OFF` is necessary:
an existing large INTEGER cell reinterpreted as a REAL column is not a valid
ordinary-table indexed-storage fixture. With automatic indexes enabled its
index-seek behavior need not equal its scalar REAL projection. The probe
deliberately checks scalar comparisons only.

## Candidate definitions and source reasoning

### Recommended simplification for ivmlite's actual BLOB-affinity keys

`crates/ivmlite-sqlite/src/view.rs::declaration` gives output column names
without declared types. Those columns have BLOB affinity. A selected ordinary
column comparison can therefore use only BLOB/no-conversion or NUMERIC
comparison affinity; TEXT RHS affinity cannot cause TEXT conversion here.
This permits a tighter design than the conservative generic candidates below:

| RHS storage class | Operator | Candidate |
| --- | --- | --- |
| INTEGER/REAL | `>`, `>=` | `g op ?` |
| INTEGER/REAL | `=`, `<`, `<=` | `g op ? OR (g >= '' AND g < x'' AND N(g) op ?)` |
| TEXT | all five | `g op ? OR N(g) op ? OR g op N(?) OR N(g) op N(?)` |
| BLOB | all five | `g op ?` |
| NULL | all five | `(? IS NOT NULL AND 0)` |

The probe tests the numeric 2-arm superset for all five operators, plus the
direct `>`/`>=` refinement, only in the actual typeless-column mode:

| Candidate | Eligible row pairs | Original true matches | False negatives |
| --- | ---: | ---: | ---: |
| BLOB-LHS numeric RHS 2 arms | 890,650 | 359,042 | 0 |
| BLOB-LHS TEXT RHS 4 numeric arms | 2,615,100 | 1,052,496 | 0 |
| BLOB-LHS numeric RHS direct `>` / `>=` | 356,260 | 244,656 | 0 |

For numeric RHS, affinity can convert only a TEXT LHS. For `>`/`>=`, raw
comparison already accepts every TEXT LHS (and BLOB LHS), so direct comparison
is a superset. For the other operators the added TEXT region retains successful
numeric conversions. TEXT RHS may itself convert under numeric affinity, so
all four raw/NUMERIC combinations are needed for the general safe construction.

`expression-plans.json` retains actual SQLite 3.53.0 `json_each` virtual-table
plans: direct hidden-column equality and COLLATE equality use index 1;
unary-plus and CAST expressions use index 0; row-value equality uses index 3;
the tested transitive join uses index 1. This is a built-in-vtab compiler probe,
not an ivmlite integration test. Official SQLite 3.45 `whereexpr.c`
`exprMightBeIndexed` returns a scalar column identity for `TK_COLUMN`; other
expressions require ordinary expression indexes, which virtual tables cannot
have. Unary-plus/CAST LHS constraints therefore fall back. Row-value expansion
and transitive constraints that reach `xBestIndex` retain the ordinary
BLOB-affinity vtab column identity.

### Conservative generic construction

Let `N(x)=+CAST(x AS NUMERIC)` and `T(x)=+CAST(x AS TEXT)`. All generated
comparisons retain BINARY collation and all original constraints remain residual.

Generic 7 arms for operator `op`:

```sql
g op ? OR N(g) op ? OR g op N(?) OR N(g) op N(?)
       OR T(g) op ? OR g op T(?) OR T(g) op T(?)
```

INTEGER or REAL RHS, any supported operator:

```sql
g op ? OR N(g) op ? OR T(g) op T(?)
```

INTEGER RHS, equality only:

```sql
g = ? OR (g >= '' AND g < x'' AND N(g) = ?)
```

BLOB RHS uses direct comparison for all five operators. NULL RHS uses
`(? IS NOT NULL AND 0)` to retain its placeholder.

The retained official SQLite `version-3.45.0` sources establish these cases:

- `vdbe.c`: comparison numeric affinity applies `applyNumericAffinity(p,0)`
  independently to both TEXT operands. Failed numeric conversion leaves the
  value unchanged. Successful conversion is equal by numeric comparison to
  its NUMERIC CAST. Integer syntax uses the exact integer parser when needed;
  decimal/exponent syntax may differ between INTEGER and REAL representation
  only at exactly representable values in the 51-bit CAST integer corridor.
- `vdbemem.c`: NUMERIC CAST uses the same floating parser and exact integer
  parser, while `sqlite3RealSameAsInt` prevents lossy REAL-to-INTEGER changes.
  Forced conversion of invalid strings/BLOBs only adds candidate positives;
  the raw arms preserve cases where affinity leaves them unchanged.
- TEXT affinity leaves TEXT and BLOB values unchanged and stringifies numeric
  values. The 7 arms enumerate raw/converted choices independently. For an
  already numeric RHS, the 3 arms suffice: numeric affinity leaves RHS alone;
  TEXT affinity either does nothing or uses `T(g) op T(?)`.
- BLOB RHS remains BLOB under these affinities. Any non-BLOB LHS remains
  below BLOB in storage-class order, and BLOB LHS is unchanged, so every
  direct comparison has the same truth value.
- For INTEGER equality, raw numeric equality preserves numeric LHS matches.
  Every convertible TEXT LHS is kept by the TEXT-region CAST branch. A TEXT
  affinity equality to an INTEGER requires its canonical decimal integer
  string, whose NUMERIC CAST exactly reproduces that INTEGER. NULL/BLOB LHS
  cannot contribute additional equality matches.

Untyped column affinity is BLOB, distinct from no expression affinity.
`expr.c::sqlite3CompareAffinity` uses NUMERIC if either column has numeric
affinity; for a BLOB-affinity LHS and TEXT-affinity RHS it uses BLOB, hence no
TEXT conversion. The unary-plus LHS mode additionally exercises genuine TEXT
comparison affinity and confirms the conservative candidate construction.

Primary source URLs:

- https://github.com/sqlite/sqlite/blob/version-3.45.0/src/vdbe.c
- https://github.com/sqlite/sqlite/blob/version-3.45.0/src/vdbemem.c
- https://github.com/sqlite/sqlite/blob/version-3.45.0/src/expr.c
- https://sqlite.org/datatype3.html

The existing design's raw/NUMERIC-LHS/TEXT-LHS 3-arm generic form and TEXT
equality specialization both miss a numeric LHS `1` against a stored TEXT RHS
`'01'` carrying NUMERIC affinity. The original equality is true; each old
candidate is false. `rejected-candidates.json` retains this observed result.
Its REAL-specialization counterexample uses a no-affinity LHS `'Inf'` versus
a REAL infinity RHS carrying TEXT affinity: original true, old candidate false.
That REAL counterexample does not claim a typeless-column LHS has no affinity.
