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
