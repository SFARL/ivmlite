# Taproot Assets: universe event statistics

[Issue 642](https://github.com/lightninglabs/taproot-assets/issues/642) describes
problems once the event log reaches roughly ten million rows: repeated joins
and aggregation for statistics, plus a goroutine per logged proof query.
The [original report](extracted/issue-642-body.md) considers log compaction,
materialized views and atomic counters. Its [comments](issue-642-comments.json)
discuss SQLite/PostgreSQL portability. The issue was open at capture.

## Original code

All code here is pinned to the report's cited commit,
`e569cf553d324297c303d190678320fcc50b9c10`.

- [000010_universe_stats.up.sql](upstream/000010_universe_stats.up.sql) contains
  the cited view: JOIN events to roots, count `SYNC` and `NEW_PROOF` with
  `COUNT(CASE ...)`, group by asset ID, group key and proof type.
- [000007_universe.up.sql](upstream/000007_universe.up.sql) defines roots,
  events, indexes and the previous version of the view.
- All ten `*.up.sql` migrations at that commit are retained in `upstream/`,
  including prerequisite asset/tree tables and the event timestamp migration.
  Migration order and original types, including binary identifiers, are preserved.
- [universe.sql](upstream/universe.sql) retains the actual consumer queries and
  write paths, including event inserts, deletes, aggregate reads, filtering,
  sorting and pagination. It includes sqlc parameters and annotations.
- [tree.json](tree.json) records the pinned repository tree;
  [LICENSE](upstream/LICENSE) preserves the upstream license.

## Evidence and gaps

The code provides substantially more than an isolated aggregation query.
However, these files are not a standalone replay harness: production data,
event frequency, key skew, expected outputs, dependency setup and comparable
timing measurements remain absent. Original migration dialect and sqlc
placeholders have not been adapted or executed.

The report identifies two architectural problems. Faster statistics alone
would not establish a solution for goroutine pressure or continued log growth.
Its proposed materialized-view option describes periodic refresh; adopting
incremental maintenance would be a later hypothesis to test.

Phase support: not evaluated.
