# ivmlite

[![CI](https://github.com/SFARL/ivmlite/actions/workflows/ci.yml/badge.svg)](https://github.com/SFARL/ivmlite/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/SFARL/ivmlite?include_prereleases)](https://github.com/SFARL/ivmlite/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

ivmlite is an experimental SQLite loadable extension that incrementally
maintains grouped `COUNT`, `SUM`, `TOTAL`, `AVG`, `MIN` and `MAX` queries. It supports a fixed
filter and one two-table inner equi-join, captures ordinary SQL writes with
triggers, and updates a materialized result when the application explicitly
refreshes it.

**Current status: `v0.1.0-alpha.6` technical preview, in release preparation.** Use it to test real
workloads and help shape the SQL subset. The on-disk format and supported SQL
may change before a stable release.

## When it can help

ivmlite targets workloads with all of these properties:

- a large mutable SQLite table;
- one or more repeatedly-read grouped aggregates;
- changes arrive in batches that are small relative to the base data;
- the application can choose when the cached result becomes current.

For small tables, large update batches, one-off queries, arbitrary user-built
filters, or databases that cannot load extensions, direct SQLite queries are
usually a better fit.

## Quick start

Once published, download the archive for your platform from the
[v0.1.0-alpha.6 release](https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.6),
extract it, and start a SQLite CLI that supports loadable extensions.

On macOS, the system SQLite omits extension loading. Install SQLite with
Homebrew and use its CLI:

```sh
brew install sqlite
SQLITE="$(brew --prefix sqlite)/bin/sqlite3"
```

On Linux:

```sh
SQLITE=sqlite3
```

Then run:

```sh
$SQLITE demo.db
```

```sql
.load ./libivmlite_sqlite sqlite3_ivmlite_init

CREATE TABLE orders(
  id INTEGER PRIMARY KEY,
  region TEXT NOT NULL,
  amount INTEGER NOT NULL
);

INSERT INTO orders VALUES
  (1, 'north', 10),
  (2, 'north', 20),
  (3, 'south', 7);

CREATE VIRTUAL TABLE revenue USING ivm(
  'SELECT region, SUM(amount), COUNT(*)
   FROM orders
   GROUP BY region'
);

SELECT * FROM revenue;
-- north|30|2
-- south|7|1

INSERT INTO orders VALUES (4, 'south', 13);

-- Base-table writes are captured. Refresh applies all pending changes.
INSERT INTO revenue(revenue) VALUES ('refresh');

SELECT * FROM revenue;
-- north|30|2
-- south|20|2
```

The view is read-only. Drop it with `DROP TABLE revenue`; ivmlite removes its
private state and removes shared capture objects after the final dependent view
is dropped.

## Build from source

Rust 1.95 and SQLite development files are required.

```sh
git clone https://github.com/SFARL/ivmlite.git
cd ivmlite
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
```

The library is written to:

- Linux: `crates/ivmlite-sqlite/target/release/libivmlite_sqlite.so`
- macOS: `crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib`

## Supported view SQL

Base tables may be STRICT or non-STRICT with any declared type. SQLite affinity
rules determine stored values; NULL, INTEGER, REAL, TEXT and BLOB are captured,
persisted and returned without changing their storage class. STRICT `ANY` is
accepted. BINARY collation is required; the conservative `COLLATE` check can
also refuse an explicitly declared BINARY collation.

A maintained view must have:

- a non-empty `GROUP BY` over bare columns;
- bare group-by columns in its SELECT list, followed by one or more of
  `COUNT(*)`, `COUNT(column)`, `SUM(column)`, `TOTAL(column)`, `AVG(column)`,
  `MIN(column)` and `MAX(column)`;
- optionally one comparison or `IS NULL` / `IS NOT NULL` predicate;
- optionally one two-table inner equi-join on columns with the same affinity.
  Numerically equal INTEGER and REAL join keys match, so `1` matches `1.0`;
  NULL keys match nothing.

Comparison literals may be integers, reals, quoted strings or blobs (`X'…'`).
They are converted by the column's comparison affinity. A REAL literal against
a TEXT-affinity column is refused because SQLite's REAL-to-TEXT rendering
varies between versions; use a quoted string instead.

Notable unsupported features include global aggregates and views without
aggregates, expressions in SELECT / GROUP BY / aggregates, `AND`/`OR`,
subqueries, CTEs, outer/self/multi-table joins, `DISTINCT` aggregates,
`group_concat`, `HAVING`, `ORDER BY` and `LIMIT`. The supported SQLite behavior
is 3.45 and later; tests use bundled SQLite 3.53.2.

### Differences from SQLite

REAL `SUM`, `TOTAL` and `AVG` return the exact result rounded once. They can
differ from SQLite in the last bits: SQLite sums in scan order with
Kahan–Babuška–Neumaier and divides after rounding the sum. For example, AVG of
0.1, 0.2 and 0.3 is `0.2` here and `0.19999999999999998` in SQLite.
Finite inputs can also overflow SQLite's intermediate REAL accumulator to
infinity (or produce NULL after intermediate infinity cancellation) while
ivmlite's exact final result is finite. Named finite-input tests observe
infinity on SQLite 3.53.2; NULL compatibility is covered separately by synthetic
comparator tests. A sum with actual `+Inf` and `-Inf` inputs returns NULL, as
SQLite does.

Integer `SUM` fails refresh with `integer overflow in SUM` only when the final
exact sum is outside the signed 64-bit range. SQLite can fail when a scan
prefix overflows even if the final sum fits. For numerically equal INTEGER and
REAL values, MIN returns the INTEGER and MAX the REAL; SQLite's choice depends
on scan order.

### Format 6 upgrade

**alpha.6 cannot read alpha.5's format-5 maintained state.** Old views open as
broken views so they can be dropped. Drop every old ivmlite view, then recreate
it from its SELECT query to build format-6 state. There is no automatic
migration; the base tables remain the source of truth.

Refresh is intentionally explicit:

```sql
INSERT INTO view_name(view_name) VALUES ('refresh');
```

To refresh every view of the database in one call, use `ivm_refresh_all`.
Continuing the quick start:

```sql
CREATE VIRTUAL TABLE big_orders USING ivm(
  'SELECT region, COUNT(*) FROM orders WHERE amount > 10 GROUP BY region'
);
INSERT INTO orders VALUES (5, 'north', 40);

SELECT ivm_refresh_all();
-- 2

SELECT ivm_refresh_all('continue');
-- refreshed 0, skipped 2, failed 0
```

- `ivm_refresh_all()`, the same as `ivm_refresh_all('atomic')`, refreshes
  every view or none. It returns the number of views it refreshed. A view
  with nothing new since its last refresh still has its checks run, but is
  skipped and not counted. If any view fails, every view is rolled back and
  the call fails with
  `ivm_refresh_all: view <name>: <message>; no view was refreshed`.
- `ivm_refresh_all('continue')` refreshes each view on its own. A view that
  fails is rolled back alone and the call goes on. It returns
  `refreshed <r>, skipped <s>, failed <f>`, followed by
  `: <name>: <message>; ...` for each failed view. A failure that rolls
  back the whole transaction, such as a trigger's `RAISE(ROLLBACK)`, an
  interrupt or a full disk, cannot be undone for one view alone: it fails
  the whole call in either mode, with the view's name and message, and
  inside your own transaction it ends that transaction.
- Call it from a `SELECT`. From a statement that writes, such as
  `INSERT INTO t SELECT ivm_refresh_all()`, it fails with
  `ivm_refresh_all must be called from a SELECT, not from a statement that writes`.
  SQLite refuses it in the database schema's triggers, views and `CHECK`
  constraints. Inside `BEGIN … COMMIT` it works, and a later `ROLLBACK`
  undoes it.
- In autocommit it begins with `BEGIN IMMEDIATE` and ends with `COMMIT`, so
  it waits for another connection's write lock under `busy_timeout`, as a
  refresh statement does. Called as `SELECT ivm_refresh_all() FROM t`, whose
  read of `t` is already under way, it cannot wait, and fails at once with
  `database is locked`. If its commit is refused (for example `SQLITE_BUSY`
  while another connection reads), it rolls the whole call back and fails.
  Inside your own transaction the locking is yours: start it with
  `BEGIN IMMEDIATE` to wait for other writers.
- It is for atomicity and convenience, not speed. It prepares its
  statements again on every call, so with small batches a loop that keeps
  each view's refresh statement prepared is faster, by about 4–5 µs per view
  per call ([benchmark](docs/bench/README.md#m1b-phase-7-ivm_refresh_all)).

The full operational and semantic boundary is documented in
[Known limitations](docs/superpowers/specs/2026-09-18-ivmlite-design.md#13-known-limitations-v0).

## What the first case studies show

The repository includes source-backed slices of public Datasette, Org-roam,
and Taproot Assets workloads. Each executable slice loads the real extension,
applies base-table changes, refreshes, and checks every output row against
SQLite recomputing the same SQL.

Median same-host results on an Apple M2 Pro:

| Executable slice | 25k base + 1k inserted | 250k base + 100 inserted |
|---|---:|---:|
| Datasette county facet | ivmlite **5.45× slower** | ivmlite **9.75× faster** |
| Org-roam tag counts | ivmlite **2.94× slower** | ivmlite **36.70× faster** |
| Taproot event counts | ivmlite **2.72× slower** | ivmlite **30.62× faster** |

These are synthetic fixtures shaped by real reports, not reproductions of the
original applications. All three slices cross over: each loses to full
recomputation at 25k base rows with 1k inserted and wins at 250k with 100
inserted. The run was measured on a machine that was not idle (one-minute
load average 4.32–4.51), so read the ratios rather than the absolute times. Read the
[methodology, exact timings, adaptations, and missing evidence](docs/cases/evaluation/README.md)
before quoting the results.

## Development

Build the debug extension and run every test:

```sh
scripts/test-all.sh
```

Run only the source-backed cases:

```sh
scripts/build-extension.sh
cargo test --locked -p ivmlite-test --test real_world_cases
```

Run the release benchmark with `base_rows`, `batch_size`, and `repetitions`:

```sh
cargo build --release --locked \
  --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release --locked -p ivmlite-test \
  --example real_case_bench -- 250000 100 5
```

Run the M1b Phase 4 benchmark (the extension against hand-written triggers and
full recomputation, with confirmation repeats and a write-amplification
workload); [`docs/bench/README.md`](docs/bench/README.md) has the full
reproduce block and the results for Phase 4, for M1b Phase 5's
refresh-performance follow-up, and for M1b Phase 6's steady-state refresh:

```sh
scripts/bench.sh matrix > m1b-phase4.csv
scripts/bench.sh write-amp > m1b-phase4-write-amp.csv
```

If you have a real workload, open an issue with the table definitions, exact
query, approximate row count, read/write cadence, acceptable staleness, and
what you have already tried. A failing or unsupported workload is useful data.

The [design](docs/superpowers/specs/2026-09-18-ivmlite-design.md),
[case archive](docs/cases/README.md), and [benchmark notes](docs/bench/README.md)
record both positive and negative findings.

## License

MIT. See [LICENSE](LICENSE).
