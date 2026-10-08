# Zcash transparent-balance slice

This demo evaluates an incremental balance shaped by
[librustzcash issue #2476](https://github.com/zcash/librustzcash/issues/2476).
It uses synthetic data and a denormalized eligibility flag; it is not a replay
of a wallet database and does not establish wallet-accounting correctness.

## Public evidence

The issue reports about 197 ms warm-cache at 500,000 transparent UTXOs in the
high-frequency `get_wallet_summary` path. The source query scans received
outputs, joins transactions, addresses and accounts, materializes a spent set,
then groups by account. It may run twice when confirmations are requested.
No production database or matching public large fixture is available.

## Adapted query

The demo assumes the application has already reduced spentness, transaction
validity and confirmation rules to `eligible`:

```sql
SELECT account_uuid, SUM(value_zat)
FROM transparent_outputs
WHERE eligible = 1
GROUP BY account_uuid;
```

The 500,000-row fixture uses 256 synthetic accounts and marks ten percent of
outputs initially ineligible, matching the related public 50,000-spend scale.
Account cardinality is a demo choice because the issue does not publish it.
The mixed batch models receive, spend, rewind and account/value correction.

## Run and result

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- zcash_transparent_balance 500000 500 5
```

The covering index of `indexed_recompute` is `transparent_outputs(eligible,
account_uuid, value_zat)`. The protocol and columns are in the [demos
README](README.md#benchmark-protocol).

This run used an Apple M2 Pro, macOS 26.2, Rust 1.95.0, SQLite 3.53.2, an
in-memory database, 500,000 base rows, a 500-operation mixed batch and five
independent repetitions. Each cell is the median, with the min–max over the
repetitions in parentheses. Other builds and tests were running on the machine
at the time, so these timings are noisier than an idle run and only the shape
of each comparison should be read from them; the authoritative before/after
comparison of M1b Phase 5 comes from its own measurement run.

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.164 (0.161–0.178) ms | 0 ms | 0 ms | 0.164 (0.162–0.178) ms | 0.164 (0.162–0.178) ms | 12,284 KiB |
| unindexed recompute | 0 ms | 117 (111–120) ms | 0.181 (0.169–0.184) ms | 113 (111–117) ms | 0 ms | 113 (111–118) ms | 113 (111–118) ms | 12,284 KiB |
| indexed recompute | 215 (209–221) ms | 22.3 (22.0–23.1) ms | 1.21 (1.10–1.28) ms | 23.0 (21.9–23.9) ms | 0 ms | 24.2 (23.0–25.2) ms | 24.2 (23.0–25.2) ms | 25,700 KiB |
| ivmlite | 566 (537–589) ms | 4.39 (4.22–4.60) ms | 4.63 (4.55–4.89) ms | 4.09 (3.99–4.27) ms | 0.053 (0.052–0.062) ms | 8.72 (8.55–9.16) ms | 8.77 (8.60–9.22) ms | 12,484 KiB |

For this adapted query, ivmlite's write, refresh and read were about 2.8 times
faster than indexed recomputation, the headline baseline, and about 13 times
faster than unindexed recomputation. ivmlite's first refresh after `CREATE`,
which also drains the stage the bootstrap left behind, took 4.39 ms: within
run-to-run noise of the steady-state refresh of 4.09 ms. The upstream 197 ms
and this 113 ms unindexed recomputation are different queries on different
machines and must not be treated as a before/after comparison.

See the [raw results](results/2026-10-08-zcash-macos-m2-pro.txt).

## Boundaries

This is not a drop-in replacement for the source query. Current ivmlite does
not express the four-table join, spent-set subquery, confirmation arithmetic,
coinbase maturity or other wallet classifications. The application remains
responsible for changing `eligible` atomically on spend and rewind. A missing
group means zero balance and must be filled by the caller. No hand-written
wallet-counter baseline has been implemented yet.
