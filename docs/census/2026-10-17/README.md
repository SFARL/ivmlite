# SQL-subset census, 2026-10-17

**Question.** Which features missing from ivmlite's v0 subset (alpha.5) would let real workloads run?

**Two corpora:**
- **28 candidate applications.** These are deduplicated from the demo roadmap, the demo write-ups, the case evaluation and the 2026-10-16 community demand notes. Two more were excluded because they contain no workload.
- **The 24 Nexmark queries.** The Flink SQL variant at `nexmark/nexmark` revision `6b3646c3`, the same pin as the M2b preflight.

**Method.**
- Each item was classified against one fixed taxonomy of feature codes ([`taxonomy.md`](taxonomy.md)).
- Every feature the item needs was recorded, not only the first parse error.
- Upstream application code was read read-only at pinned revisions. Nothing was executed against upstream data, and nothing was posted.

This is demand evidence, not adoption.

| File | Contents |
|---|---|
| [`taxonomy.md`](taxonomy.md) | the v0 subset and the feature codes F1–F15 |
| [`apps.md`](apps.md), [`apps.json`](apps.json) | per-application query, schema, features, unlock sets, fit, sources |
| [`nexmark.md`](nexmark.md), [`nexmark.json`](nexmark.json) | per-query features, unlock sets and rewrites |

## Applications (28)

Nothing runs on v0 as it stands: no upstream schema is STRICT.

**Blocked whatever SQL is added: 17.** Some are blocked for more than one reason.

| Reason | Count | Candidates |
|---|---:|---|
| Not ivm-shaped: the real problem is an index, N+1 queries, a cache or similar | 11 | garcon, openvibely, orpheus, photo_triage, pocketbase, pulse, screenpipe, setu, stash, subwave, tautulli |
| The runtime cannot load a native extension (F14) | 8 | modernc (taproot), Turso's engine (postio), Android Room / iOS system SQLite (noop), and others |
| Aggregates must survive deletion of their raw rows (F13) | 2 | bifrost, claude_monitor |
| Semantics that remain after adaptation (F15), such as context_guru's ordered state machine | 3 | context_guru, orpheus, stash |

**Unlockable by SQL and schema work: 11.**

| Feature added | Candidates unlocked |
|---|---|
| **Non-STRICT tables (F10a) and REAL / BOOLEAN / NUMERIC columns (F10b), in place** | **9:** better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod |
| + MIN/MAX (F3c) | +1 (kener) |
| + `group_concat` (F3f) | +1 (org_roam) |

F10a alone is needed by 27 of the 28; F10b by 17.

**"In place" still assumes small documented rewrites in the application.** Examples:
- a stored derived column, such as a precomputed time bucket;
- a conditional aggregate pivoted into a group key;
- AVG served as SUM/COUNT;
- top-N computed by the reader.

These are listed per item in `apps.md`. What "in place" removes is the alternative: an application-maintained STRICT copy of each base table. That copy is itself trigger-maintained, so it would undercut the comparison against a hand-written aggregate trigger. Accepting the existing tables is the honest route.

**Frequent but adaptable.** F6 (a richer WHERE, 13 candidates), F9 (ordering and top-N, 11), F4a (expressions inside aggregates, 10) and F6t (time-relative predicates, 10) are common. They all disappear under stored buckets, flags, pivots and reader-side top-N, so none is on the critical path for these applications.

**Two P0 demos change status.** Taproot uses modernc, and noop runs on Android Room and iOS's system SQLite. Neither runtime can load the extension, so neither application can adopt ivmlite as it stands, whatever its SQL fit.

## Nexmark (24)

Nothing runs on v0 as written.

**Never fits: 7.** Q5, Q6, Q7, Q8, Q11, Q12 and Q13 need Flink's streaming time semantics (TUMBLE, HOP, SESSION, PROCTIME, temporal joins), or ordered or rolling frames (F12, F15).

**Unlockable as written: 17.** The features their minimal unlock sets include:

| Feature | Queries that need it |
|---|---:|
| F1 non-aggregated (bag) view | 14 |
| F6 richer WHERE | 9 |
| F8 subquery, CTE or nested aggregation | 8 |
| F7c multi-column / non-equi / self join | 7 |
| F3c MIN/MAX, F4b expression in SELECT | 5 each |
| F9 ordering / top-N / window functions | 4 |

**No single feature unlocks much; the gain comes in bundles:**

| Bundle | Queries unlocked, as written |
|---|---|
| F1 | 2 (Q0, Q20) |
| F1 + F4b | 4 |
| F1 + F4b + F6 | 7 (Q0, Q2, Q3, Q10, Q20, Q21, Q22) |
| + F10b (REAL) | 9 (adds Q1, Q14) |

**Rewrites that move logic into the application.**
- A bag view, `GROUP BY` every column with `COUNT(*)` as the multiplicity, already runs on v0. The preflight executed it for Q0 and Q20.
- Ten more queries would run on v0 if their expressions were moved into stored columns. These rewrites were not executed, and they turn the queries into adapted workloads rather than feature coverage.

## What this implies for the next phase

1. **For applications, the schema is the biggest lever.** Accepting existing non-STRICT tables and the REAL, BOOLEAN and NUMERIC column types (F10a + F10b) is the single change that unlocks the most: 9 of the 11 unlockable applications. Its difficulty is semantic, not syntactic. A non-STRICT column can hold values of several storage classes, and a maintained SUM must match SQLite's own result for them, or refuse them predictably.
2. **For Nexmark, and so for M2b, the lever is a bundle.** The bundle is non-aggregated views (F1), a richer WHERE (F6) and expressions in the SELECT list (F4b), plus REAL columns. Any one of them alone barely helps.
3. **Outside SQL work:** MIN/MAX helps one application. Retention or sealing helps two. Runtime reach (F14) blocks eight, half of which are not ivm-shaped anyway.

## Caveats

- Several application fits are low-confidence. Per item they are marked `confidence: low`: zcash, phoenix, datasette, org_roam, better_ccflare, openwhispr, unscreen, claude_monitor.
- A few queries were taken from issue text rather than pinned source: screenpipe, pulse, stash, tautulli, orpheus, and phoenix's SQL. Bifrost and openvibely were read at their current main, not at a pinned revision.
- Whether Python or Bun can load the extension depends on how the host built SQLite.
- Q21 (`REGEXP_EXTRACT`), Q22 (`SPLIT_INDEX`) and Q14 (`count_char`) use Flink-only functions. They were counted as unlockable because SQLite built-ins reproduce them; under a strict reading they are out of scope.
- Flink's `AVG` over BIGINT returns BIGINT, while SQLite's returns REAL (Q4, Q17).

## After M2c

**Compatibility projection, recomputed 2026-10-19.** The original alpha.5 census
above and its pinned JSON metadata remain historical. M2c implements F3a,
F3b, F3c, F3e, F10a and F10b. Recompute the counts from those snapshots with:

```sh
python3 docs/census/2026-10-17/after_m2c.py
```

The script checks each application's `in_place_unlock` set, excluding
`not_ivm_shaped` items and blockers F13/F14/F15; it does not mistake the full
unadapted `features` set for the documented in-place adaptation. **10 of 28
candidates fit after adaptation: four high-confidence and six low-confidence.**
Seventeen remain blocked for the original non-SQL reasons; among the eleven
SQL-eligible candidates, org_roam alone still requires `group_concat` (F3f).
These are workload compatibility projections, not newly executed upstream
queries, adoption or measured performance.

Native types remove the need for a STRICT copy or integer scaling. The remaining
application work still matters:

| Candidate | Confidence | Remaining rewrites and boundary |
|---|---|---|
| better_ccflare | low | Reader account CASE/fallback and small accounts join, window sums, percentages, ordering, LIMIT and NULL-model filtering; use a 0/1 success field for SUM. COUNT(id)=COUNT(*) is schema-specific. The stall's bottleneck is unattributed. |
| datasette | low | Flatten the subquery, retain one state predicate, discard NULL county and sort/top31 in the reader. Only predeclared facets/filter pairs fit; arbitrary request facets do not. |
| fluxflow | high | Stored day bucket and reader flow total; a constant key is still needed for global counts (or sum group counts). Height MIN/MAX may remain an index seek. Retention-enabled PR #45 needs F13 and remains blocked. Native REAL amount removes x1e8 scaling. |
| kener | high | Stored 15-minute bucket, status group key, reader monitor/window filter and viewer-timezone daily composition of 96 buckets. Native AVG/MIN/MAX remove scaled latency and SUM/COUNT adaptation; original CASE/time expressions remain outside the subset. |
| nicti | high | Drop COALESCE and merge NULL/empty model groups in the reader. Census volume/model/rating grain differs from the ADR demo model/rating/keyword grain; upstream's store direction may leave SQLite. |
| openwhispr | low | Store per-segment word_count, one deleted predicate, reader NULL-speaker removal/topN and stored day for date grouping. Native Electron packaging remains unverified. |
| phoenix | low | Store minute bucket; project selection, COALESCE and totals in the reader. Selected buckets are exact only for minute-aligned starts. Upstream removed the feature; extension availability remains conditional. |
| unscreen | low | Maintain closed sessions with stored day/duration; merge live open sessions and exclude system apps/topN in the reader. No slow query was reported; Android Python extension loading is unverified. |
| zcash | low | Maintain eligibility flags on receives, spends, rewinds and each block; re-evaluate unconfirmed outputs, use two views, map account_id to UUID in reader. The original multi-join/NOT IN/confirmation logic remains application work. |
| zenod | high | Lift tenant into group key; since=0 hot path only. Moving since needs buckets/F6t. Reader COALESCE/totals and explicit-refresh versus transactional quota semantics remain. Upstream already fixed it with counters. |

For Nexmark, M2c features alone unlock **0 queries as written**. Recomputed
hypothetical bundles preserve the original counts:

| Bundle | Queries unlocked as written |
|---|---|
| F1 | 2: Q0, Q20 |
| F1 + F4b | 4: Q0, Q10, Q20, Q22 |
| F1 + F4b + F6 | 7: Q0, Q2, Q3, Q10, Q20, Q21, Q22 |
| + F10b REAL | 9: adds Q1, Q14 |

After M2c, F1/F4b/F6 are still missing; the nine-query row is a hypothetical
bundle, not nine completed query implementations. The Flink-function rewrite
caveats for Q14/Q21/Q22 and BIGINT AVG caveat for Q4/Q17 above remain.
