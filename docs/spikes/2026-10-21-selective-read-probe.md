# Selective reads after alpha.6

Date: 2026-10-21. Status: measured diagnostic and proposed next scope.
This document does not change the supported SQL or promise an implementation.

## Starting point

[Alpha.6](https://github.com/SFARL/ivmlite/releases/tag/v0.1.0-alpha.6) is
published at `0ad47a9259ee7acee411b46b7f64d50d268b2096`. The local checkout
was fast-forwarded to that tag, both extension profiles were rebuilt, and the
release-library lifecycle smoke passed. The release's Linux and macOS CI each
reported 848 passed, 0 failed and 3 explicitly ignored capability tests.

The next useful work should reduce a demonstrated application cost or remove
a concrete schema/query adaptation. The three ignored complete-query tests
are separate capabilities, rather than a single next-release specification.

## Observed read gap

At the release source, [`best_index`](../../crates/ivmlite-sqlite/src/vtab.rs)
accepts only a full scan. `IvmCursor::filter` ignores index information and
arguments, selects all output columns and rows, and collects them into a
`Vec` before returning rows to SQLite. An outer `WHERE` can express a
selective read, but currently cannot restrict that internal work.

The output table already has an index over its output columns, created by
[`create_out_table`](../../crates/ivmlite-sqlite/src/view.rs). Kener's leading
output keys are `monitor_id, bucket_ts, status`. A monitor equality followed
by a bucket range therefore has an existing index prefix to investigate.
Conditions on other column orders may have different access costs.

The [Kener capture](../demos/kener.md#m2c-source-scale-result-2026-10-20)
reads all 550,000 groups in 155.472 ms. It is a synthetic full-result read,
not a page-specific application measurement. The same capture's 1,760,784
KiB is total database page allocation, including retained extrema state.
Selective reads would address scanning and result materialization; they do
not establish a reduction in that persistent allocation.

## Small executable diagnostic

This run used Python 3.14.4, SQLite 3.53.0, macOS 27.0.1 ARM64, and a locally
built alpha.6 release library with SHA-256
`be3b3b73576c6f51de6255be4191c793bb3944e01214c2c3989d97da98a2766d`.
It creates 20,000 groups in memory and selects one existing integer key.
The progress handler runs every SQLite VM instruction, including nested SQL.

| Read path | Returned rows | Progress callbacks | Query plan |
|---|---:|---:|---|
| Public virtual table, `WHERE g=?` | 1 | 160,018 | `SCAN roll VIRTUAL TABLE INDEX 0:` |
| Internal indexed output reference, same predicate | 1 | 15 | `SEARCH __ivm_out_roll USING COVERING INDEX __ivm_outidx_roll (g=?)` |

The internal reference is a diagnostic, not a supported application API.
It bypasses the virtual-table adapter. These are work counts for a small
fixture, not latency measurements or a predicted speedup. Instrumentation
would itself distort timings. The SQLite version here also differs from
the bundled 3.53.2 used in the release acceptance suite.

Reproduce after building the extension. On this Mac, set
`IVMLITE_EXTENSION` to the absolute path of
`crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib`; on Linux use
the corresponding `.so`. The script uses an in-memory database only.

```python
import hashlib
import json
import os
from pathlib import Path
import sqlite3

library = Path(os.environ["IVMLITE_EXTENSION"])
group_count = 20_000
selected_key = 123
connection = sqlite3.connect(":memory:")
connection.enable_load_extension(True)
try:
    connection.execute("SELECT load_extension(?, ?)",
                       (str(library), "sqlite3_ivmlite_init"))
finally:
    connection.enable_load_extension(False)
connection.execute("CREATE TABLE facts(g INTEGER NOT NULL, v INTEGER NOT NULL)")
connection.executemany("INSERT INTO facts VALUES (?, ?)",
                       ((key, key) for key in range(group_count)))
connection.commit()
connection.execute("CREATE VIRTUAL TABLE roll USING ivm("
                   "'SELECT g, COUNT(*) AS n FROM facts GROUP BY g')")
results = {}
for name, sql in (
    ("virtual_view", "SELECT g,n FROM roll WHERE g=?"),
    ("indexed_shadow_reference",
     "SELECT g,n FROM __ivm_out_roll WHERE g=?"),
):
    callbacks = [0]
    def tick():
        callbacks[0] += 1
        return 0
    connection.set_progress_handler(tick, 1)
    try:
        rows = connection.execute(sql, (selected_key,)).fetchall()
    finally:
        connection.set_progress_handler(None, 0)
    assert rows == [(selected_key, 1)], rows
    results[name] = {
        "rows": rows,
        "sqlite_vm_progress_callbacks": callbacks[0],
        "plan": connection.execute("EXPLAIN QUERY PLAN " + sql,
                                   (selected_key,)).fetchall(),
    }
print(json.dumps({"sqlite": sqlite3.sqlite_version,
                  "library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
                  "groups": group_count, "results": results}, indent=2))
connection.close()
```

## Proposed next scope: group-key constraints at read time

Start with usable group-key equality and range comparisons for a query such as:

```sql
SELECT *
FROM kener_bucket_rollup
WHERE monitor_id = ? AND bucket_ts >= ? AND bucket_ts < ?;
```

`kener_bucket_rollup` is the maintained relation in the existing Kener demo.
The conjunction and bound parameters belong to the
outer SQLite read. This is distinct from adding AND/OR to maintained-view
definitions or making a view's time boundary advance with the clock.

The initial design should preserve SQLite's residual filtering and push
only constraints whose affinity, collation and NULL behavior are proven
equivalent on the output table. Residual rechecking can remove false
positives, but cannot recover a row incorrectly excluded by pushdown.
Unusable or unsupported constraints must keep the existing scan behavior.

Keep ordering and LIMIT with SQLite initially. Do not claim uniqueness,
consume ordering, or push a limit before residual filtering without a
separate correctness argument. Reuse the current owned-row cursor model
for the filtered subset; streaming cursors introduce additional overlapping
read/write and statement-lifetime questions.

Acceptance should include:

1. Independent result comparison with ordinary SQLite filtering, including
   NULL, INTEGER/REAL numeric equality, TEXT/BLOB values, comparison bounds,
   collations, rebound parameters, duplicate and unusable constraints.
2. The same reads after refresh, deletion, rekey, rollback and reopen;
   existing alpha.6 databases and broken-view diagnostics must retain their
   current behavior. Explicit refresh and cursor snapshot semantics remain.
3. Evidence of the executed internal access path and scan work. A public
   virtual-table plan label or elapsed time alone is insufficient. Test
   leading equality/range prefixes and non-leading constraints separately.
4. Matching Kener fixtures for full-result, one-monitor and monitor-plus-time
   reads. Retain uninstrumented read latency, returned groups and end-to-end
   costs, and separately capture scan work. Preserve full-read and maintenance
   regression comparisons; do not infer a production result from this probe.
5. One native-driver example covering load, transaction writes, refresh,
   parameterized reads, rollback and reopen. Avoid a multi-language SDK until
   that complete path is demonstrated on a specific host.

## Order after that experiment

| Candidate | Why it follows | Boundary |
|---|---|---|
| Limited aggregate-argument addition | dark-xray currently stores `total_bytes` instead of maintaining `SUM(up_bytes + down_bytes)` directly. | Match SQLite's per-row coercion, NULL and integer-overflow-to-REAL behavior before applying the documented aggregate contract. CASE and date expressions remain separate. |
| Static compound filtering, starting with AND | Reduces adaptation for fixed multi-column conditions when a selected query requires it. | Request-time account/time parameters may instead belong in outer reads. OR/IN/NOT and clock-driven expiry require their own scope. |
| Narrow usage facts versus sealing | Compare retaining small correctable facts while deleting large payloads before designing permanent rollup-only history. | DELETE continues to retract contributions. Sealing requires coverage, retries, late corrections and raw/archive merge semantics. |

Jot can use outer SQLite for title association, sorting and LIMIT over a
supported bounds projection, but its source end-time expression and native
driver loading still need validation. This does not establish a complete
Swift application integration. Exact P95, DISTINCT collection, core outer
joins and incremental top-N remain separately justified projects.
