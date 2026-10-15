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
Record raw output under
`docs/demos/results/YYYY-MM-DD-zenod-<machine>.txt`; do not compare the
upstream 2.9-second incident directly with this synthetic workload.

## Boundaries

The demo covers unfiltered all-time totals. It does not implement quota
decisions, null-quota fast paths, admission-control races, event-loop latency,
tenant-specific result lookup or Zenod's `since`-filtered summaries. The
upstream application updates its totals in the same write transaction;
ivmlite's explicit refresh has a different freshness contract. A correct
integration must choose a transaction and refresh boundary that meets its
quota semantics. The benchmark does not claim to outperform Zenod's own
running-totals implementation.
