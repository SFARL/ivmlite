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

## Run and first result

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- zcash_transparent_balance 500000 500 3
```

Three-run medians on an Apple M2 Pro were:

| Mode | Bootstrap | Apply | Maintain/recompute | Read | End to end |
|---|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0.203 ms | 0 ms | 0 ms | 0.203 ms |
| full adapted recompute | 0 ms | 0.200 ms | 121.768 ms | 0 ms | 121.968 ms |
| ivmlite | 608.176 ms | 4.339 ms | 4.134 ms | 0.072 ms | 8.545 ms |

For this adapted query, ivmlite was about 14 times faster end to end after the
batch. The upstream 197 ms and this 121 ms recomputation are different queries
on different machines and must not be treated as a before/after comparison.

See the [raw transcript](results/2026-10-08-zcash-macos-m2-pro.txt).

## Boundaries

This is not a drop-in replacement for the source query. Current ivmlite does
not express the four-table join, spent-set subquery, confirmation arithmetic,
coinbase maturity or other wallet classifications. The application remains
responsible for changing `eligible` atomically on spend and rewind. A missing
group means zero balance and must be filled by the caller. No hand-written
wallet-counter baseline has been implemented yet.
