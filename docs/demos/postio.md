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
Record raw output under
`docs/demos/results/YYYY-MM-DD-postio-<machine>.txt`. Compare the local modes
against each other; upstream's 1.48-second full-footprint timing is context.

## Boundaries

This does not benchmark the GTK settings interaction, main-thread scheduling,
attachments, local payload accounting, read-receipt count or mail sync. It
does not prove that extension loading is available in Postio's chosen database
engine. The synthetic STRICT schema differs from the source schema and is
not a drop-in migration. Actual integration would need to preserve refresh
freshness, account isolation and delete/restore semantics.
