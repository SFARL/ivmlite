# PocketBase author book counts

This synthetic demo follows the grouped-count view in
[PocketBase discussion #7592](https://github.com/pocketbase/pocketbase/discussions/7592).
It isolates maintaining book counts per author from the larger back-relation
expand problem.

## Public evidence

Checked on 2026-10-15: the discussion was opened on 2026-03-23, last updated
on 2026-03-24, and remains open and unanswered. The reporter provided a
5,000-author, roughly 18,000-book test schema and the view:

```sql
SELECT author AS id, author, COUNT(*) AS cnt
FROM books
GROUP BY author;
```

The reported API timings were 65 ms for authors, 70 ms with books expanded,
75 ms for the direct count view, and about 1,500 ms with books and the count
view both expanded. These are request timings, not isolated SQLite aggregate
timings. The author later said the actual dataset has hundreds of thousands
of rows; its exact scale and measurements were not supplied.

The maintainer confirmed the back-relation expand behavior and published a
reproducer with 5,000 authors and 18,000 books. On PocketBase v0.36.7 with
the default pure Go SQLite driver, the maintainer measured 158 ms for the
count-view expand and 425 ms for both expands. CTE/UNION rewrites worsened
the latter to 798 ms. The maintainer recommended smaller pages and noted
that `--dev` added roughly 30% overhead. The reporter's 1,500 ms and the
maintainer's 425 ms must not be treated as the same experiment or as
ivmlite results.

## Adaptation

The maintained slice is `GROUP BY author` with `COUNT(*)`. The default
fixture has 18,000 synthetic books distributed across 5,000 synthetic author
IDs. The author keys are TEXT; book primary keys are INTEGER. The
distribution and STRICT schema are demonstration
choices. An author without books has no group and the caller supplies zero.

```sql
SELECT author, COUNT(*)
FROM books
GROUP BY author;
```

The benchmark covers book insertion, reassignment and deletion while reading
the maintained count relation. It does not run PocketBase or its HTTP API.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- pocketbase_author_counts 18000 40 5
```

The [shared protocol](README.md#benchmark-protocol) reports bootstrap, writes,
refresh, result reads and page allocation. Record raw output under
`docs/demos/results/YYYY-MM-DD-pocketbase-<machine>.txt`. Only the local
benchmark modes form a controlled performance comparison.
The indexed baseline uses `books(author)`. A local hand-written trigger
baseline maintains the same count relation. Reassignments and inserts create
new synthetic author IDs, so the result cardinality can grow beyond the
5,000 seed authors.

## Boundaries

Maintaining the count view can remove repeated aggregation work, but the
discussion's broader bottleneck includes per-record relation-ID queries,
access checks, response assembly and a per-record relation limit. This demo
does not establish how much of that cost is aggregation, eliminate those
queries, or prove that an ivmlite extension can load in PocketBase's default
pure Go SQLite driver. It is a semantic count-view demonstration, not an
end-to-end fix or an adoption claim.
