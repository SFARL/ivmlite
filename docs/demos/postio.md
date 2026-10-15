# Postio mailbox footprint slice

This synthetic demo follows the message-count and message-byte aggregate in
[Postio issue #1508](https://github.com/dlapiduz/postio/issues/1508). It covers
the first query inside `MessageRepository::footprint`, not every figure in
Postio's settings panel.

## Public evidence

Checked on 2026-10-15: the issue was opened on 2026-09-12 and remains open.
It reports that `footprint` previously measured 1.48 seconds on a real mail
store and was moved from startup to panel opening. The issue describes a
store of about 81,000 messages and a 16 ms interaction budget. Deferring the
scan improves startup but leaves the settings interaction dependent on
mailbox size.

The issue distinguishes two remedies. Its read-receipt count has a selective
predicate and can use a partial index. The whole-account `COUNT(*)` and
`SUM(size)` need a maintained aggregate or asynchronous loading. The demo
targets the latter, rather than treating every reported count as an IVM need.

The [source at the research snapshot](https://github.com/dlapiduz/postio/blob/c32a6f4af191fb07c48176c7b77a1e502817c600/crates/postio-storage/src/repository/messages.rs#L2088)
first runs:

```sql
SELECT count(*), coalesce(sum(size), 0)
FROM messages
WHERE account_id = ?1 AND deleted_locally = 0;
```

It then runs separate attachment/local-byte aggregates. The 1.48-second
measurement belongs to upstream `footprint`; it is not an isolated timing of
only the first query. The
[upstream investigation note](https://github.com/dlapiduz/postio/blob/c32a6f4af191fb07c48176c7b77a1e502817c600/docs/notes/2026-09-11-an-aggregate-hides-from-a-row-count.md)
also explains why counting returned rows fails to detect aggregate scans.

## Adaptation

The demo changes the account-specific scalar aggregate into grouped results
for all accounts, restricted to `deleted_locally = 0`. It uses integer byte
sizes and a STRICT table. The default fixture contains 81,000 synthetic
messages; account distribution, deletion flags and sizes are demo choices.
Initially there are eight accounts, and half of messages are marked
deleted. The adapted query is:

```sql
SELECT account_id, COUNT(*), SUM(size)
FROM messages
WHERE deleted_locally = 0
GROUP BY account_id;
```

The mixed batch inserts messages, marks them deleted, restores them, corrects
sizes, moves accounts and physically deletes rows.
The caller selects an account's group, maps an absent group to zero, and
uses explicit refresh to obtain current results.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- postio_mailbox_footprint 81000 200 5
```

Interpret results using the [shared protocol](README.md#benchmark-protocol).
The indexed baseline uses
`messages(deleted_locally, account_id, size)`. A local hand-written trigger
baseline maintains the same filtered count and sum.

The [recorded run](results/2026-10-15-postio-m2pro.txt) on 2026-10-15 used
commit `1c61d6e`, the `0.1.0-alpha.4` extension, bundled SQLite 3.53.2 and an
Apple M2 Pro with 16 GiB memory. It used **three repeats**, rather than the
five in the example command, with fresh in-memory databases, 81,000 base
rows, 200-operation mixed batches and ten measured batches per repeat. Values
below are medians across repeats. Total ranges are minimum–maximum across
repeats; totals use paired write + maintain + read batch sums.

| Mode | Bootstrap ms | End-to-end ms (range) | SQLite pages KiB |
|---|---:|---:|---:|
| no maintenance | 0 | 0.049 (0.049–0.049) | 1,136 |
| unindexed recompute | 0 | 5.921 (5.874–5.979) | 1,136 |
| indexed recompute | 27.033 | 1.997 (1.965–2.067) | 2,284 |
| hand-written triggers | 6.131 | 1.076 (1.059–1.114) | 1,140 |
| ivmlite | 46.044 | 1.913 (1.904–1.964) | 1,236 |

ivmlite's median write, steady-state refresh and read components were
1.619 ms, 0.283 ms and 0.012 ms. The first refresh was 0.569 ms.
Component medians do not sum exactly to the paired total. At this fixture size,
indexed recomputation and ivmlite were close end to end: 1.997 versus 1.913 ms,
about a 4% difference. Local hand-written triggers were about 1.78 times faster
than ivmlite. This is not evidence that Postio needs an extension to meet its
16 ms interaction budget: even local indexed recomputation was below it for
this isolated query. The one-minute load average remained about 4.83. The
command completed with SQLite oracle checks passing. Upstream's 1.48-second
full-footprint timing is context, not a baseline for this isolated query.

## Boundaries

This does not benchmark the GTK settings interaction, main-thread scheduling,
attachments, local payload accounting, read-receipt count or mail sync. It
does not prove that extension loading is available in Postio's chosen database
engine. The synthetic STRICT schema differs from the source schema and is
not a drop-in migration. Actual integration would need to preserve refresh
freshness, account isolation and delete/restore semantics.
