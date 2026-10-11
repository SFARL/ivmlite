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

This run, on 2026-10-10, used an Apple M2 Pro, macOS 26.2, Rust 1.95.0,
SQLite 3.53.2, the `0.1.0-alpha.3` extension, an in-memory database, 500,000
base rows, a 500-operation mixed batch and five independent repetitions.
Apply, maintain and read are each a repetition's median over its ten measured
batches. Each cell is the median over the repetitions, with their min–max in
parentheses. Desktop applications stayed open, and the one-minute load average
was 12.40 at the start and 10.38 at the end, so the machine was not idle and
only the shape of each comparison should be read from these timings. The
before/after comparison of M1b Phase 5 is in the "M1b Phase 5" section of
[`docs/bench/README.md`](../bench/README.md).

| Mode | Bootstrap | First refresh | Apply | Maintain | Read | Apply + maintain | End to end | SQLite pages |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| no maintenance | 0 ms | 0 ms | 0.164 (0.161–0.174) ms | 0 ms | 0 ms | 0.164 (0.162–0.174) ms | 0.164 (0.162–0.174) ms | 12,328 KiB |
| unindexed recompute | 0 ms | 119 (117–137) ms | 0.189 (0.187–0.195) ms | 118 (117–119) ms | 0 ms | 119 (117–119) ms | 119 (117–119) ms | 12,328 KiB |
| indexed recompute | 225 (218–256) ms | 23.6 (22.9–24.2) ms | 1.13 (1.02–1.27) ms | 23.2 (22.9–23.7) ms | 0 ms | 24.3 (24.1–25.0) ms | 24.3 (24.1–25.0) ms | 25,772 KiB |
| ivmlite | 578 (570–634) ms | 3.31 (3.09–3.49) ms | 4.75 (4.69–4.76) ms | 2.98 (2.90–3.04) ms | 0.057 (0.053–0.058) ms | 7.73 (7.59–7.81) ms | 7.79 (7.65–7.86) ms | 12,536 KiB |

For this adapted query, ivmlite's write, refresh and read were about 3.1 times
faster than indexed recomputation, the headline baseline, and about 15 times
faster than unindexed recomputation. ivmlite's first refresh after `CREATE`
took 3.31 ms, about 1.1 times the steady-state refresh of 2.98 ms, with their
ranges apart. Since format 4, bootstrap empties its own stage, so the first
refresh no longer drains it. The upstream 197 ms and this 118 ms unindexed
recomputation are different queries on different machines and must not be
treated as a before/after comparison.

See the [raw results](results/2026-10-10-zcash-macos-m2-pro.txt).

## Boundaries

This is not a drop-in replacement for the source query. Current ivmlite does
not express the four-table join, spent-set subquery, confirmation arithmetic,
coinbase maturity or other wallet classifications. The application remains
responsible for changing `eligible` atomically on spend and rewind. A missing
group means zero balance and must be filled by the caller. No hand-written
wallet-counter baseline has been implemented yet.
