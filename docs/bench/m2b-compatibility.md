# M2b compatibility preflight

The executed preflight is **blocked for general Turso cross-validation**.
SQLite full recomputation remains the correctness oracle. The pinned Nexmark
Flink variant has 24 queries: none compiles unchanged in alpha.5. Two explicitly
adapted bag workloads run correctly; they are not canonical Nexmark coverage or
a standard benchmark score. No Turso performance result is reported.

## Reproduce

Use standard-library Python with loadable SQLite extensions enabled, Git, a built
ivmlite library, and a built Turso CLI. The script downloads or installs nothing.
Pin clean upstream checkouts:

- [nexmark/nexmark at 6b3646c3](https://github.com/nexmark/nexmark/tree/6b3646c3baec701f1fa74baf938d235f742e5d3c),
  Flink SQL `nexmark-flink/src/main/resources/queries`, Q0–Q23.
- [tursodatabase/turso at 1f48d31b](https://github.com/tursodatabase/turso/tree/1f48d31b11effb623c2ebd5df6216bb6753d6fe5),
  workspace/CLI version `0.9.0-pre.1`.

The Turso CLI used for the recorded run was built following its pinned
[contributor instructions](https://github.com/tursodatabase/turso/blob/1f48d31b11effb623c2ebd5df6216bb6753d6fe5/CONTRIBUTING.md#getting-started),
using its Rust 1.88 toolchain and a dev build, with two build jobs and debug info
disabled. A dev build is sufficient for these semantic checks. In the Turso
checkout, the build command was:

```sh
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 cargo build --locked --package turso_cli --bin tursodb
```

From the ivmlite checkout, using local paths to those pinned checkouts:

```sh
python3 scripts/probe-m2b.py \
  --nexmark-repo target/m2b-probes/nexmark \
  --turso-bin target/m2b-probes/turso/target/debug/tursodb \
  --extension target/alpha5-validation/artifact-smoke/ivmlite-v0.1.0-alpha.5-macos-arm64/libivmlite_sqlite.dylib \
  --out target/m2b-probes/replay \
  --fail-on-incompatibility
```

Choose the library filename for the host. `--out` must be new or empty. Both
upstream HEAD SHAs and tracked cleanliness are checked before execution. The
Turso source checkout is found above its binary; use `--turso-repo` if stored
elsewhere. The CLI version and executable digest are recorded. A supplied binary
and matching source checkout are not a reproducible-build attestation.

The runner writes `provenance.json`, `nexmark.json`, `turso.json`, and
`verdict.json`. SQL fixtures and all raw CLI stdout/stderr are retained in the
JSON. Replay commands use path placeholders; no personal paths are archived.

Exit codes:

- `0`: collection completed; **read the verdict**, which can still be `blocked`.
- `2`: completed with incompatibilities and `--fail-on-incompatibility` was set.
- `1`: invalid paths/pins/version, unavailable host capability, failed oracle,
  timeout, or incomplete collection.

Nexmark rejections are an inventory, not task failure. The two adapted workloads
must match their SQLite multisets. Turso semantic errors, changed unsupported
boundaries, failed ordinary controls, and typed output differences block the
preflight. The committed [evidence snapshot](m2b-preflight/evidence/verdict.json)
records the observed blocked verdict. Its negative-input checks are in
[negative-checks.json](m2b-preflight/evidence/negative-checks.json).

## Nexmark inventory

This is the official repository's Flink SQL variant, not Apache Beam, Feldera,
or `queries-cep`. All files contain Flink DDL and INSERT-to-sink wrappers; the
probe extracts the defining SELECT. Sources are mapped to named SQLite STRICT
tables with INTEGER/TEXT fields; timestamps use INTEGER. No source generator,
Flink job, watermark, blackhole sink, or filesystem output was executed.

Classification concerns whether the defining SELECT can become an ivmlite view:
unchanged, dialect adaptation, semantic rewrite, or unsupported. The observed
counts are **0 unchanged, 0 dialect-only, 2 semantic rewrites, 22 unsupported**.
All 24 original SELECTs were actually submitted and rejected; first diagnostics
and source/SELECT digests are in [nexmark.json](m2b-preflight/evidence/nexmark.json).

Query links below refer to the exact pinned revision.

| Query | Purpose and operators | Classification and boundary |
|---|---|---|
| [Q0](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q0.sql) | Pass-through bag projection | Semantic rewrite: GROUP BY every output field plus COUNT(*) multiplicity; executed |
| [Q1](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q1.sql) | Currency conversion; decimal multiply | Unsupported arithmetic/decimal projection and aggregate-less root |
| [Q2](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q2.sql) | Selection; MOD(auction,123) | Unsupported scalar modulo and aggregate-less root |
| [Q3](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q3.sql) | Local item suggestion; equijoin, category AND state OR predicates | Unsupported multiple predicates and aggregate-less root |
| [Q4](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q4.sql) | Category average winning price; join, time range, MAX then AVG | Unsupported subquery, nested aggregation, MAX/AVG, BETWEEN and multiple predicates |
| [Q5](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q5.sql) | Hot items; HOP window, COUNT, MAX, derived-table join | Unsupported windows, subqueries, MAX and composite join conditions |
| [Q6](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q6.sql) | Seller average; winning bids, ROW_NUMBER, ordered AVG OVER | Unsupported ranking/window/subqueries/AVG; upstream also marks Flink retraction support missing |
| [Q7](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q7.sql) | Highest bid; TUMBLE, MAX, derived join, time range | Unsupported window/MAX/subquery/timestamp operations |
| [Q8](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q8.sql) | New users; two TUMBLE windows and join on id/window boundaries | Unsupported windows/subqueries/composite join and aggregate-less grouping |
| [Q9](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q9.sql) | Winning bids; join/time range, ROW_NUMBER top1 | Unsupported subquery/ranking/order and multiple predicates |
| [Q10](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q10.sql) | Filesystem logging; DATE_FORMAT projection and partitioned sink | Unsupported scalar/date projection; filesystem sink outside ivmlite |
| [Q11](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q11.sql) | User sessions; SESSION windows and COUNT | Unsupported session state/time functions and expression group keys |
| [Q12](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q12.sql) | Processing-time windows; PROCTIME, TUMBLE, COUNT | Unsupported clock/window/table-function semantics |
| [Q13](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q13.sql) | Side input; PROCTIME, SYSTEM_TIME AS OF, modulo join | Unsupported temporal join, subquery and scalar modulo |
| [Q14](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q14.sql) | Calculation; decimal multiply, CASE/HOUR, UDF, AND/OR | Unsupported arithmetic/scalars/UDF/CASE and multiple predicates |
| [Q15](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q15.sql) | Bidding statistics; day expression, COUNT DISTINCT/FILTER | Unsupported expression group keys, distinct state and aggregate FILTER |
| [Q16](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q16.sql) | Channel statistics; day group, MAX minute, DISTINCT/FILTER | Unsupported date expressions/MAX/distinct/filtered aggregates |
| [Q17](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q17.sql) | Auction statistics; day group, filtered COUNT, MIN/MAX/AVG/SUM | Unsupported date expression/FILTER/MIN/MAX/AVG; reducing to COUNT/SUM changes workload |
| [Q18](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q18.sql) | Last bid; ROW_NUMBER partition bidder/auction, time DESC | Unsupported ranking/window/subquery |
| [Q19](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q19.sql) | Auction top10; ROW_NUMBER partition auction, price DESC | Unsupported ranking/window/subquery |
| [Q20](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q20.sql) | Bid/auction enrichment; equijoin, category=10, bag projection | Semantic rewrite: alias duplicate names, group all output fields, COUNT(*) multiplicity; executed |
| [Q21](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q21.sql) | Add channel id; LOWER, CASE, REGEXP_EXTRACT, OR/IN | Unsupported scalar/regex/CASE/multiple predicates/projection |
| [Q22](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q22.sql) | URL directories; SPLIT_INDEX projection | Unsupported scalar string splitting and aggregate-less root |
| [Q23](https://github.com/nexmark/nexmark/blob/6b3646c3baec701f1fa74baf938d235f742e5d3c/nexmark-flink/src/main/resources/queries/q23.sql) | Bid/person/auction enrichment; three-way join | Unsupported more than two tables and aggregate-less root |

The local compiler contract is in
[compile.rs](../../crates/ivmlite-sql/src/compile.rs): exactly one SELECT,
physical tables only, at most one inner equijoin on a single column pair, one
column/literal WHERE comparison or IS [NOT] NULL, bare-column group keys, and
COUNT(*)/SUM(column). Projected bare columns must exactly match group keys and
precede aggregates. The root restrictions in
[plan.rs](../../crates/ivmlite-core/src/plan.rs) additionally require nonempty
GROUP BY and at least one aggregate. Projection-only queries do not fit.

The Q0/Q20 rewrites group by every projected field and expose multiplicity.
Expanding that count reconstructs the original relational bag at a checkpoint;
the physical output shape and maintenance workload change. Both passed four
multiset checkpoints: bootstrap, category/price updates, deletion of one
duplicate bid, and insertion plus deletion of one duplicate auction. Fixtures
include NULL payloads, duplicate rows and join fanout. This tests a relational
adaptation, without claiming stream timing or Flink sink semantics.

## Executed Turso boundaries

The pinned [SQL reference](https://github.com/tursodatabase/turso/blob/1f48d31b11effb623c2ebd5df6216bb6753d6fe5/docs/sql-reference/statements/create-materialized-view.mdx)
and [CLI manual](https://github.com/tursodatabase/turso/blob/1f48d31b11effb623c2ebd5df6216bb6753d6fe5/cli/manuals/materialized-views.md)
describe `CREATE MATERIALIZED VIEW name AS SELECT ...`, automatic same-transaction
maintenance, and DROP VIEW. Runtime probes confirmed STRICT schema support,
the experimental flag gate, MVCC rejection, and absence of native manual
REFRESH syntax. MVCC exclusion is also explicit in
[view.rs](https://github.com/tursodatabase/turso/blob/1f48d31b11effb623c2ebd5df6216bb6753d6fe5/core/translate/view.rs#L65).
These explicit unsupported cases are expected rejections, not correctness bugs.

The recorded binary reports `Turso 0.9.0-pre.1`, with SHA256
`712af68f3f5980491f1c070a56cbd52d266a760a9bb689e86b81280e04884063`.
Its `sqlite_version()` is 3.50.4; the host Python SQLite oracle is 3.51.0.
Queried defaults are WAL, synchronous=2, page_size=4096, cache_size=-2000.
Every failure below also has a **same Turso ordinary VIEW control**, removing
SQLite host-version differences as an explanation.

| Accepted operation | Observed MV result | Ordinary Turso and SQLite result |
|---|---|---|
| [All-NULL SUM](m2b-preflight/sql/null-sum.sql) | COUNT=1, SUM=0.0 | COUNT=1, SUM=NULL |
| [Integer precision](m2b-preflight/sql/integer-precision.sql) | Input 9007199254740993 becomes 9007199254740992 | Exact integer 9007199254740993 |
| [NULL equijoin](m2b-preflight/sql/null-join.sql) | NULL keys match, adding a phantom `null-side` group | NULL keys do not match |
| [Transaction after rollback](m2b-preflight/sql/rollback-then-begin.sql) | Second BEGIN/UPDATE/COMMIT fails: `cannot commit - no transaction is active` | Sequence succeeds |

Raw results and controls are in [turso.json](m2b-preflight/evidence/turso.json).
The SUM NULL failure was also tested after inserting/deleting a non-NULL row;
precision loss also occurs with incremental inserts of 2^53 and 1. Transaction
failure was previously reproduced with SUM, COUNT-only and projection MVs, in
memory and on disk; normal VIEW controls and two consecutive COMMITs succeed.
The reusable minimal transaction fixture uses a projection MV. After the
reported COMMIT error, a disk reopen did not contain that second update; this
is not a claim of losing an acknowledged successful commit.

Numeric-value checks passed automatic visibility inside the first transaction,
first ROLLBACK, consecutive COMMITs, zero SUM with positive row count, deletion
and recreation of a group's last row, reopen and subsequent UPDATE/INSERT,
DROP cleanup, and duplicate/fanout joins with **non-NULL keys**, including updates
to values and keys on both sides. Nevertheless integer SUM outputs are REAL in
the MV and INTEGER in ordinary recomputation. Numeric matches and typed output
differences are recorded separately; neither NULL nor imprecise large sums are
normalized away. The pinned
[aggregate implementation](https://github.com/tursodatabase/turso/blob/1f48d31b11effb623c2ebd5df6216bb6753d6fe5/core/incremental/aggregate_operator.rs#L462)
uses f64 SUM state, consistent with the observed type and precision boundary.

## M2 decision

M2a nonuniform validation can proceed independently. M2b should preserve these
pin-specific gates and retain SQLite full recomputation as its oracle. Further
Nexmark SQL coverage needs an explicit scope decision; reducing a query or
precomputing unsupported expressions must remain visible as an adaptation.

Turso performance comparison needs a verified common semantic domain and
comparable optimized embedded execution. Its maintenance runs on writes while
ivmlite defers maintenance until refresh: compare identical DML plus all
maintenance plus COMMIT. Report writer/refresh phases only as secondary
measurements. CLI startup per transaction versus prepared embedded statements,
and refresh-only versus total Turso write time, cannot establish relative
maintenance cost. This preflight produces no performance claim and no upstream
issue or message was posted.
