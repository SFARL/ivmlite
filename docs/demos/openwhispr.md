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
