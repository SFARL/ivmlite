# Nicti keyword facet counts

This synthetic demo follows the maintained facet-count experiment in
[Nicti issue #103](https://github.com/jordanfelle/nicti/issues/103). It measures
a fixed join-and-group workload rather than a complete photo-catalog UI.

## Public evidence

Checked on 2026-10-15: the issue was opened and closed on 2026-09-24; its
implementation was merged in [PR #111](https://github.com/jordanfelle/nicti/pull/111).
The authoritative evidence is now
[ADR-0103](https://github.com/jordanfelle/nicti/blob/c3ec874a3b5827c89326890dc0323817774e013c/docs/adr/0103-facet-count-cache.md),
formerly ADR-0011 before the repository renumbered its ADRs.

The ADR's original SQLite experiment used **2 million synthetic assets**,
not a production catalog. Its indexed faceted query took 151 ms p95, with
re-measurements between 134.5 and 160 ms, against a 100 ms budget. In a later
contended session, the ordinary SQLite baseline measured 411.8/513.2 ms
p50/p95 and the trigger-maintained facet table measured 2.479/3.054 ms. The
ADR explicitly warns that concurrent builds inflated absolute timings.

The chosen upstream design maintains counts at `(model, rating, keyword)`
grain with SQLite triggers. Its 2-million-asset run measured 0.173/0.272 ms
p50/p95 per rating update and 8.973/11.421 ms per 100-rating burst. These
measurements include a corrected benchmark that changes the rating on every
timed call. An earlier version accidentally measured unchanged ratings and
understated maintenance cost. All of these are Nicti's measurements, not
ivmlite results.

## Adaptation

The demo maintains keyword memberships joined to asset model/rating, grouped
by those three dimensions. The default fixture has 2,000,000 synthetic
assets and two distinct keyword memberships per asset. The two-keyword
distribution and vocabulary are demo choices; they are not a reproduction
of Nicti's generator.

```sql
SELECT assets.model, assets.rating, asset_keywords.keyword, COUNT(*)
FROM assets JOIN asset_keywords ON assets.id = asset_keywords.asset_id
GROUP BY assets.model, assets.rating, asset_keywords.keyword;
```

The fixture starts with eight model names, six integer ratings and 64
keywords. Its mixed batch inserts assets with memberships, deletes keywords,
changes asset model/rating, retags keywords, moves a membership to a different
asset and deletes parent assets. Parent deletion leaves unmatched memberships;
the inner join excludes them.

Counts represent joined memberships. The fixture initially gives each asset
two distinct keywords, but the demo schema does not prohibit duplicate
`(asset_id, keyword)` pairs. Exact-keyword counts equal distinct-asset counts
only if the application also guarantees one membership per asset and keyword.
Summing over several keywords can count one asset several times.
The source ADR independently documents this same limitation: its facet table
does not produce a distinct asset count when the keyword restriction is
removed. This demo therefore makes no claim about arbitrary overlapping
keyword prefixes or no-keyword distinct counts.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- nicti_facet_counts 2000000 200 5
```

The [shared protocol](README.md#benchmark-protocol) includes writes,
maintenance and reading the maintained groups. The fixture scale is assets,
with twice as many initial membership rows.
The indexed baseline uses `assets(model, rating, id)` and
`asset_keywords(asset_id, keyword)`; the base schema already indexes
`asset_keywords(asset_id)`. A local hand-written trigger baseline covers both
join inputs and maintains the same membership counts.

The [recorded run](results/2026-10-15-nicti-m2pro.txt) on 2026-10-15 used
commit `1c61d6e`, the `0.1.0-alpha.4` extension, bundled SQLite 3.53.2 and an
Apple M2 Pro with 16 GiB memory. It used **three repeats**, rather than the
five in the example command, with fresh in-memory databases, 2,000,000 assets,
4,000,000 initial keyword memberships, 200-operation mixed batches and ten
measured batches per repeat. Values below are medians across repeats. Total
ranges are minimum–maximum across repeats; totals use paired batch sums.

| Mode | Bootstrap ms | End-to-end ms (range) | SQLite pages KiB |
|---|---:|---:|---:|
| no maintenance | 0 | 0.138 (0.137–0.145) | 180,052 |
| unindexed recompute | 0 | 2,847.376 (2,833.964–3,034.455) | 180,052 |
| indexed recompute | 2,059.449 | 3,059.668 (2,889.982–3,207.913) | 312,352 |
| hand-written triggers | 2,883.293 | 2.508 (2.464–2.585) | 180,136 |
| ivmlite | 25,351.129 | 8.101 (7.712–8.230) | 885,736 |

ivmlite's median write, steady-state refresh and read components were
2.501 ms, 5.337 ms and 0.229 ms. The first refresh was 10.291 ms.
Component medians do not sum exactly to the paired total. The faster
recomputation baseline here was the unindexed mode, which still has the base
join-key index; use its 2,847.376 ms rather than the slower covering-index
mode as the comparison. End to end, ivmlite was about 351 times faster than
that baseline, while local hand-written triggers were about 3.23 times faster
than ivmlite. ivmlite also used about 4.92 times the base SQLite page allocation
and took 25.35 seconds to bootstrap. These are material space and startup
costs of this join workload. The one-minute load average fell from 5.80 to
4.83. The command completed with SQLite oracle checks passing; the upstream
facet-query timings describe a different query and experiment.

## Boundaries

The maintained grouped relation is the reusable slice. It does not reproduce
the source's interactive `model = ? AND rating >= ?` keyword-filtered query,
photo metadata distribution, UI, DuckDB alternative, or write latency gates.
Application-side filtering and combining of group rows remain separate work.
Nicti's chosen triggers maintain results inside writes; ivmlite needs an
explicit refresh. Even a local hand-written trigger baseline is a synthetic
counterpart, not a claim to have ported or beaten Nicti's implementation.
