# Zenod tenant usage totals

This synthetic demo follows the all-time usage-ledger aggregation in
[Zenod PR #1380](https://github.com/zenod-ai/zenod/pull/1380). It exercises the
real ivmlite extension; it does not replay a production database or establish
quota-enforcement correctness.

## Public evidence

Checked on 2026-10-15: the PR was opened on 2026-10-11 and merged on
2026-10-12. The author reports a production ledger of 1.76 million rows,
including 1.23 million for one tenant. Each `/mcp` or `/api` request called
`checkQuota` and `record`; quota checks synchronously scanned the tenant's
ledger even when the quota was null. The reported per-request blocking time
was about 2.9 seconds, with 69–97% of CPU profiles inside those calls.

The upstream fix skips ledger access for null quotas and maintains
`tenant_usage_totals(tenant_id, kind, events, units)` transactionally with each
record. Its author reports about 65 ms for 1,000 quota-check-and-record pairs,
0.2 ms for an unfiltered summary, and 134 ms for a filtered summary. Those
numbers describe the upstream implementation, not ivmlite. The PR's migration
measurement used a synthetic 1.76-million-row, 314 MB ledger; the production
incident and the synthetic migration experiment are separate evidence.

The [merged implementation](https://github.com/zenod-ai/zenod/blob/9b036915af84f0f7e6e34cd49e2d6879158fd16c/packages/mcp-chassis/src/usage.ts)
backfills totals with `GROUP BY tenant_id, kind`, `COUNT(*)` and `SUM(units)`.
This is a concrete example of a project building maintained aggregates for a
hot path. The project already solved its problem; it has not adopted ivmlite.

## Adaptation

The demo keeps the all-time tenant/kind count and integer unit sum. Its
default fixture has 1,760,000 deterministic synthetic ledger rows. The
fixture's tenant and kind distribution, integer units, STRICT schema and
mutation sequence are demonstration choices, not a copy of Zenod's data.

```sql
SELECT tenant_id, kind, COUNT(*), SUM(units)
FROM tenant_usage
GROUP BY tenant_id, kind;
```

Seventy percent of seeded rows belong to tenant `0`, approximating the
published tenant skew without reproducing its distribution. Four initial
usage kinds are synthetic. The mixed batch inserts events, deletes events,
corrects units, moves tenants, changes kinds and sets units to zero.

The maintained result is one row per `(tenant_id, kind)`. A caller combines
the kind rows to obtain a tenant's total and supplies zero for absent groups.
An explicit refresh is required before reading current totals.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- zenod_usage_totals 1760000 200 5
```

Use the [shared benchmark protocol](README.md#benchmark-protocol) to interpret
bootstrap, write capture, refresh, result read and SQLite page allocation.
The indexed baseline uses `tenant_usage(tenant_id, kind, units)`. A local
hand-written trigger baseline maintains the same grouped relation.

The [recorded run](results/2026-10-15-zenod-m2pro.txt) on 2026-10-15 used
commit `1c61d6e`, the `0.1.0-alpha.4` extension, bundled SQLite 3.53.2 and an
Apple M2 Pro with 16 GiB memory. It used **three repeats**, rather than the
five requested by the example command above, with fresh in-memory databases,
1,760,000 base rows, 200-operation mixed batches and ten measured batches per
repeat. Values below are medians across the three repeats. The total is the
median of paired write + maintain + read batch totals; its range is the
minimum–maximum across repeats.

| Mode | Bootstrap ms | End-to-end ms (range) | SQLite pages KiB |
|---|---:|---:|---:|
| no maintenance | 0 | 0.066 (0.065–0.069) | 32,936 |
| unindexed recompute | 0 | 757.665 (755.375–759.445) | 32,936 |
| indexed recompute | 922.363 | 95.347 (95.233–96.397) | 66,580 |
| hand-written triggers | 748.196 | 1.593 (1.586–1.597) | 32,952 |
| ivmlite | 1,066.544 | 2.817 (2.809–2.931) | 33,104 |

ivmlite's median write, steady-state refresh and read components were
1.648 ms, 1.116 ms and 0.062 ms, respectively. Its first refresh was
1.186 ms. Component medians do not sum exactly to the paired total.
The local ivmlite path was about 33.8 times faster than indexed recomputation
end to end, while the local hand-written trigger path was about 1.77 times
faster than ivmlite. The one-minute load average fell from 9.04 to 5.80, so
this was not an idle-machine run. The source-scale command completed with
the SQLite oracle checks passing. The upstream 2.9-second incident is not a
before/after baseline for these synthetic timings.

## Boundaries

The demo covers unfiltered all-time totals. It does not implement quota
decisions, null-quota fast paths, admission-control races, event-loop latency,
tenant-specific result lookup or Zenod's `since`-filtered summaries. The
upstream application updates its totals in the same write transaction;
ivmlite's explicit refresh has a different freshness contract. A correct
integration must choose a transaction and refresh boundary that meets its
quota semantics. The benchmark does not claim to outperform Zenod's own
running-totals implementation.
