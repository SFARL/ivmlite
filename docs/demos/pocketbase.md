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
refresh, result reads and page allocation. Only the local benchmark modes
form a controlled performance comparison.
The indexed baseline uses `books(author)`. A local hand-written trigger
baseline maintains the same count relation. Reassignments and inserts create
new synthetic author IDs, so the result cardinality can grow beyond the
5,000 seed authors.

The [recorded run](results/2026-10-15-pocketbase-m2pro.txt) on 2026-10-15 used
commit `1c61d6e`, the `0.1.0-alpha.4` extension, bundled SQLite 3.53.2 and an
Apple M2 Pro with 16 GiB memory. It used **three repeats**, rather than the
five in the example command, with fresh in-memory databases, 18,000 books,
40-operation mixed batches and ten measured batches per repeat. Values below
are medians across repeats. Total ranges are minimum–maximum across repeats;
totals use paired write + maintain + read batch sums.

| Mode | Bootstrap ms | End-to-end ms (range) | SQLite pages KiB |
|---|---:|---:|---:|
| no maintenance | 0 | 0.012 (0.012–0.012) | 344 |
| unindexed recompute | 0 | 2.853 (2.812–2.945) | 344 |
| indexed recompute | 2.599 | 1.069 (1.058–1.090) | 688 |
| hand-written triggers | 4.104 | 0.393 (0.388–0.396) | 572 |
| ivmlite | 30.293 | 1.678 (1.641–1.796) | 1,396 |

ivmlite's median write, steady-state refresh and read components were
0.297 ms, 0.447 ms and 0.912 ms. The first refresh was 0.731 ms.
Component medians do not sum exactly to the paired total. At this fixture
size, **ivmlite was about 1.57 times slower than indexed recomputation end
to end**. Reading all count groups cost more than maintaining them; the
benchmark does not push author filters into the materialized scan. Local
hand-written triggers were about 4.27 times faster than ivmlite. ivmlite used
about 4.06 times the base SQLite page allocation. The one-minute load average
remained about 4.46. The command completed with SQLite oracle checks passing.
This is a useful semantic demonstration and a measured limit of the current
read path, rather than performance evidence for replacing this small indexed
aggregate or fixing the PocketBase API request.

## Boundaries

Maintaining the count view can remove repeated aggregation work, but the
discussion's broader bottleneck includes per-record relation-ID queries,
access checks, response assembly and a per-record relation limit. This demo
does not establish how much of that cost is aggregation, eliminate those
queries, or prove that an ivmlite extension can load in PocketBase's default
pure Go SQLite driver. It is a semantic count-view demonstration, not an
end-to-end fix or an adoption claim.
