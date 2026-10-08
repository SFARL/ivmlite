# ivmlite

[![CI](https://github.com/SFARL/ivmlite/actions/workflows/ci.yml/badge.svg)](https://github.com/SFARL/ivmlite/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/SFARL/ivmlite?include_prereleases)](https://github.com/SFARL/ivmlite/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

ivmlite is an experimental SQLite loadable extension that incrementally
maintains grouped `COUNT(*)` and integer `SUM` queries. It supports a fixed
filter and one two-table inner equi-join, captures ordinary SQL writes with
triggers, and updates a materialized result when the application explicitly
refreshes it.

**Current status: `v0.1.0-alpha.2` technical preview.** Use it to test real
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

Download the archive for your platform from the
[v0.1.0-alpha.2 release](https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.2),
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
) STRICT;

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

A maintained view must currently have:

- a non-empty `GROUP BY` over bare columns;
- `COUNT(*)`, integer `SUM(column)`, or both;
- optionally one comparison or `IS NULL` / `IS NOT NULL` predicate;
- optionally one two-table inner equi-join on same-typed columns;
- `STRICT` base tables containing only `INTEGER`, `TEXT`, and `NULL`, with
  binary collation.

Notable unsupported features include global aggregates, `COUNT(column)`,
`MIN`, `MAX`, `AVG`, expressions in grouping or aggregates, `AND`/`OR`,
subqueries, CTEs, outer/self/multi-table joins, `DISTINCT`, `HAVING`,
`ORDER BY`, and `LIMIT`.

Refresh is intentionally explicit:

```sql
INSERT INTO view_name(view_name) VALUES ('refresh');
```

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
| Datasette county facet | ivmlite **6.43× slower** | ivmlite **7.75× faster** |
| Org-roam tag counts | ivmlite **24.97× slower** | ivmlite **2.03× slower** |
| Taproot event counts | ivmlite **4.42× slower** | ivmlite **4.66× faster** |

These are synthetic fixtures shaped by real reports, not reproductions of the
original applications. They show a crossover for two slices and a current
performance problem for the Org-roam join slice. Read the
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
reproduce block and the results:

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
