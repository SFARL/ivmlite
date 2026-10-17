# OpenWhispr speaker counts and word sums

This synthetic demo follows the measured all-time `by_speaker` path in
[OpenWhispr #84](https://github.com/futuregerald/openwhispr/issues/84). It
tests whether a fixed grouped count and integer sum can be kept up to date
through the SQLite extension. It does not run Electron or measure UI blocking.

## Public workload and source

The reporter measured `get_stats` through the application's real
`DatabaseManager` on 20,000 notes and 2 million transcript segments. The
all-time by-speaker query alone took about 1,200 ms and the whole call about
1,492 ms; a one-month call took 38 ms. Those are the reporter's timings,
not results from this demo. At the pinned
[`d5e50fed` source](https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L2861-L2886),
the query joins segments to notes, omits NULL speakers and soft-deleted notes,
computes a word count from each segment's text, groups by speaker, then sorts
and returns at most 100 speakers. The app uses synchronous `better-sqlite3`
on Electron's main process, so even a faster query cannot guarantee that an
unbounded call never freezes the interface.

## Exact adaptation

The demo seeds 20,000 synthetic notes and 2 million synthetic segments, with
100 segments per note. It uses the source's note soft-delete and nullable
speaker concepts. The tables are reduced to the columns needed for this
slice and declared `STRICT` because that is required by ivmlite; the source
tables are not `STRICT`.

At insertion or text edit, the generator stores `word_count` using the same
ASCII-space rule as the source SQL expression. The test checks every stored
count against SQLite's expression after every mixed round. The maintained
query is:

```sql
SELECT s.speaker_name, COUNT(*), SUM(s.word_count)
FROM transcript_segments s JOIN notes n ON n.id = s.note_id
WHERE n.deleted_at IS NULL
GROUP BY s.speaker_name;
```

ivmlite's one-predicate SQL subset cannot also put
`s.speaker_name IS NOT NULL` in the view definition. The caller filters the
NULL group when reading; an integration test compares that result against
the source's full word expression and both filters. The benchmark itself
reads the whole grouped relation, including the NULL group, and does not
time source sorting or the top-100 limit.

The deterministic mixed batches insert and delete segments, edit segment
text and its stored count, remap speakers, and soft-delete or restore notes.
An active note is reserved for arrivals so every batch changes the visible
aggregate. The three local comparison modes are no maintenance, ordinary
recomputation and covering-index recomputation; there is no hand-written
trigger baseline for this case.

## Run

Build the release extension as in the [demos README](README.md), then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- openwhispr_speaker_words 2000000 200 3
```

The [shared benchmark protocol](README.md#benchmark-protocol) uses fresh
in-memory databases, an untimed warm-up, ten measured batches per repeat,
and a SQLite recomputation oracle after the final batch. The seeded text and
speaker distribution are synthetic, so its times are not a reproduction of
the reporter's production fixture.

## Measured result

The [raw output](results/2026-10-17-openwhispr-m2pro.txt) was collected on
2026-10-17 from commit `138792d` with the `0.1.0-alpha.5` release extension,
bundled SQLite 3.53.2 and an Apple M2 Pro with 16 GiB memory. Three repeats
each used a fresh in-memory database, 2 million segments, 20,000 notes,
200-operation batches, one warm-up and ten measured batches. The one-minute
system load average was about 9.82 at the start and 7.49 at the end; other
desktop work was running, so these are local measurements, not isolated
hardware results. The ivmlite result matched SQLite recomputation after the
final batch, and the covering-index query was checked for equivalent output.

| Mode | Bootstrap ms | End-to-end per batch ms (repeat range) | SQLite pages KiB |
|---|---:|---:|---:|
| No maintenance | 0 | 0.094 (0.094–0.094) | 85,732 |
| Unindexed recompute | 0 | 418.153 (415.981–429.203) | 85,732 |
| Covering-index recompute | 655.854 | 516.904 (513.402–552.557) | 130,192 |
| ivmlite | 8,044.412 | 14.864 (14.390–15.597) | 408,048 |

The faster recomputation mode was the unindexed one. On this adapted query,
ivmlite's end-to-end batch was about 28.1 times faster than recomputing every
batch. Its median write, refresh and view read steps were 1.926, 12.866 and
0.073 ms; the paired batch total is measured directly and need not equal the
sum of these separately selected medians. The first refresh was 17.032 ms.
The extension allocated about 4.76 times the base database's SQLite pages
and took about 8.04 seconds to bootstrap. No hand-maintained summary was
measured for this case, and these figures cannot be compared to the issue's
1,200 ms original text-expression timing.

## Boundaries

Real adoption would require a migration for stored word counts and a rule
that updates them atomically with text edits. Note soft-deletion changes all
joined segment contributions for that note. Bounded date ranges require a
separate plan: the source filters on `notes.created_at` and callers can
choose arbitrary ranges; this demo only maintains the all-time summary.
OpenWhispr would also need to verify native extension loading, explicit
refresh placement and response parity in its Electron process. Moving the
synchronous read off the main process is still the direct UI availability
fix discussed in the issue.
