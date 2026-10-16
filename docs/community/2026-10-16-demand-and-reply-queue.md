# Community demand and English reply queue

Checked on 2026-10-16. These are public workload reports, not adoption or
endorsement. Drafts below have **not been posted**. Current task scope is
research, source verification and reply preparation.

## Resume here

1. Prioritize **better-ccflare #480**: a real unresolved deployment symptom,
   with a small model-count slice worth evaluating after per-query profiling.
   The draft below is ready for review; it does not promise a fix.
2. **context-guru #588** merits a technical design reply, but its ordered
   state machine is outside ivmlite's grouped COUNT/integer SUM model.
3. Remove **Postio #1508** from active outreach: it closed today after an
   upstream maintained-counter implementation merged. Historical demo docs
   saying it remains open are a dated snapshot, not current status.
4. Do not repost to noop, Zcash or setu without a substantive new reason or
   an external reply. The live noop/Zcash comment lists currently contain no
   external follow-up to our October 15 results.
5. Next useful work: inspect better-ccflare's existing stats regression
   fixtures and split timing coverage; determine whether its model-count path
   actually contributes material latency before building another benchmark.
   Extension loading and typed projection costs remain unverified. No live
   database or production endpoint has been accessed.

The app heartbeat **ivmlite** is ACTIVE, hourly, attached to this chat. It
checks both usage windows, checkpoints before stopping at a limit, and resumes
the unfinished queue on the next successful scheduled run after quota returns.
It stays quiet without meaningful results and does not post comments or push.
At a usage check in this batch: 5-hour usage 25%, weekly usage 44%;
resets were 2026-10-16 20:21:50 and 2026-10-22 09:01:02, America/New_York.
These are snapshots: refresh usage rather than treating them as future facts.
No quota reset credit was used.

## 1. better-ccflare: unresolved demand, conditional fit

Source: [issue #480](https://github.com/StartupBros-com/better-ccflare/issues/480),
OPEN, last updated 2026-10-14 01:02:44 UTC. The report gives ordinary uncached
stats samples of 2,943.3 and 2,655.9 ms, versus 89.7 ms cached. Its initial
24-second sample was confounded by startup work. None is an ivmlite result.

The [post-deployment report](https://github.com/StartupBros-com/better-ccflare/issues/480#issuecomment-6158527029)
says the account-query optimization merged, but authenticated stats still had
no response status after two bounded 30-second waits. It does not establish
which query or runtime condition caused the stall.

Current main inspected: `386c67c02136b7ba9b41fad3a9f690cae51c8742`.

| Verified source | Implication |
|---|---|
| [Handler, lines 14–21](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/http-api/src/handlers/stats.ts#L14-L21) | Five repository paths; account/model counts intentionally keep lifetime scope despite `since=1`. |
| [Cache, lines 47–52](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/http-api/src/handlers/stats.ts#L47-L52) | Ten-second TTL; request deletion invalidates the cache. |
| [SQLite adapter, lines 270–306](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/adapters/bun-sql-adapter.ts#L270-L306) | SQLite uses synchronous `.all()` / `.get()`; wrapping the calls in `Promise.all` does not execute those statements in parallel. |
| [Account query, lines 234–285](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/repositories/stats.repository.ts#L234-L285) | CTEs, LEFT JOIN, conditional expressions and window calculations exceed ivmlite's current subset. |
| [Model query, lines 440–457](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/repositories/stats.repository.ts#L440-L457) | The inner non-NULL model COUNT is a possible maintained slice; percentages, sorting and top-N remain outside it. |
| [Requests schema, lines 464–509](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/migrations.ts#L464-L509) | Non-STRICT, with BOOLEAN/REAL columns; the existing base table cannot be registered unchanged. |

There is no ivmlite run on an upstream fixture or verified host integration.
A typed projection would require maintenance and consistent bootstrap, and
its costs belong in any comparison. Do not advertise the existing Zenod or
other synthetic demo speedups as an expected result here.

The existing [PR #481](https://github.com/StartupBros-com/better-ccflare/pull/481)
already reports isolated **complete in-process cache-miss handler** comparisons
with full JSON equality. Do not misdescribe them as account-query-only tests.
They do not reproduce the deployed authenticated-server stall. The PR also
preserves a roughly 1.9–2.2x slowdown with 100k real accounts and one request
each. Its three-file diff does not include the large handler timing harness.
The inspected [model-count test](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/__tests__/stats-query-optimizations.test.ts#L122-L132)
uses four rows for correctness; the [account suite](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/__tests__/stats-lifetime-account-aggregation.test.ts#L186-L211)
checks plan shape and semantics. No model-count or five-call segmented timings
were found in the inspected stats/performance files. The [general SQL timing
script](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/analyze-performance.ts#L17-L87)
does not execute the handler's five actual paths. Rebuilding a public runnable
handler measurement is the next evidence gap; a model-count speedup remains
hypothetical.

### English draft — diagnosis and a small evaluation target

I checked the [current handler](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/http-api/src/handlers/stats.ts#L14-L21).
It still runs five statistics paths: the overall totals and errors are
time-bounded, while account/model counts intentionally remain lifetime-wide.
The SQLite adapter executes synchronous statements, so `Promise.all` does not
parallelize those scans. This does not identify the cause of the deployment
stall. The isolated full-handler comparisons in #481 are useful; the next
step would be per-repository-call timings alongside total cache-miss latency
to attribute the remaining symptom before another rewrite.

For an incremental experiment, the [model-count CTE](https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/repositories/stats.repository.ts#L440-L457)
is a small target: maintain non-NULL model counts, then compute percentages and
top-N over the groups. Account statistics need separate equivalence tests for
their NULL/orphan/`no_account` behavior and rates.

Disclosure: I maintain [ivmlite](https://github.com/SFARL/ivmlite), an alpha SQLite
extension for grouped counts and integer sums. The current non-STRICT schema
is not a drop-in target, and extension integration is unverified. A useful
comparison would include projection maintenance, writes, refresh, reads and
deletions, checking the full response against the existing query. I have not
benchmarked this application or established that model counts cause #480.

## 2. context-guru: real need, ordered semantics outside current support

Source: [issue #588](https://github.com/rossoctl/context-guru/issues/588),
OPEN, updated 2026-10-15 20:58:16 UTC, no comments at inspection. It proposes
removing sorting or precomputing session state. It supplies relative timing
observations, not a public large database or exact wide-window latency.

Current main inspected: `1151c36eca75e1fdf5530293b64f93d573728bd2`.

| Verified source | Implication |
|---|---|
| [Replay query, lines 298–307](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L298-L307) | LEFT JOIN plus ordered request stream; an order-matching index can address sorting but does not eliminate walking the selected rows. |
| [Session walk, lines 184–199](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L184-L199) | Bypass evidence spans models within a session; a model-only counter loses this information. |
| [Lookahead/state transitions, lines 213–290](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L213-L290) | Three later turns can affect whether a drop counts as compaction; ordinary grouped SUM cannot encode this state machine. |
| [Filter, lines 73–104](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/query.go#L73-L104) | The filter is applied before walking; partial-session windows need explicit equivalence treatment. |
| [Credit deduplication, lines 418–433](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L418-L433) | Duplicate request/component credits keep the larger value; summing every joined row would change results. |
| [Existing tests, lines 308–324 and 476–495](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap_test.go#L308-L495) | Persistent-versus-recovered drops and lean/full agreement already have tests to preserve. These tests were read, not executed in this batch. |
| [Driver dependency](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/go.mod#L33) | `modernc.org/sqlite`; native ivmlite loading is not established. |

### English draft — technical contribution, no extension recommendation

Looking at [the replay query and walk](https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L213-L307),
I would separate removing the SQL sort from incrementalizing the ledger. An
order-matching index may remove sorting; the walk still processes every
selected request.

A whole-session total also needs a precise filter contract. The current walk
starts after the SQL filter, so a mid-session window must not silently inherit
a close from outside its selected rows. The three-turn recovery lookahead
means that newly arriving turns can also change the interpretation of the
trailing turns. Bypass evidence crosses models, and duplicate component
credits currently retain the larger credit rather than summing both rows.

I would keep the existing walk as the oracle and test full versus partial
sessions, interleaved models/subagents, duplicate credits, late inserts and
recovery after an observed drop. Model-window and pricing changes also need
an invalidation policy. A finalized prefix with a replayed mutable suffix may
be useful, but that remains a design to validate against these semantics,
rather than a replacement by ordinary grouped counters.

## 3. Postio: newly resolved, stop active outreach

[Issue #1508](https://github.com/dlapiduz/postio/issues/1508) closed as completed
on 2026-10-16 at 13:39:04 America/New_York, with zero comments. The associated
[PR #1871](https://github.com/dlapiduz/postio/pull/1871) merged one second earlier,
merge commit `ebf913ed8cceffe641263ec1a50f6c95e83a2876`.

Inspected HEAD `5e6e34f478f53fb8c7f53b779655df390d4c86d9`:

- [The footprint reader](https://github.com/dlapiduz/postio/blob/5e6e34f478f53fb8c7f53b779655df390d4c86d9/crates/postio-storage/src/repository/messages.rs#L2150-L2215)
  now reads one account summary row, with a separate completeness check.
- [Message/attachment triggers](https://github.com/dlapiduz/postio/blob/5e6e34f478f53fb8c7f53b779655df390d4c86d9/crates/postio-storage/src/schema/footprint.sql#L22-L134)
  maintain the account counters; [migration code](https://github.com/dlapiduz/postio/blob/5e6e34f478f53fb8c7f53b779655df390d4c86d9/crates/postio-storage/src/schema.rs#L247-L281)
  backfills existing data.
- [Storage dependencies](https://github.com/dlapiduz/postio/blob/5e6e34f478f53fb8c7f53b779655df390d4c86d9/crates/postio-storage/Cargo.toml#L41-L55)
  still use Turso's Rust engine. Native SQLite extension compatibility has not
  been demonstrated.

This is evidence that maintained aggregates solve a real application pattern,
not evidence of demand for ivmlite. Do not claim the GTK UI now meets its
latency budget: the PR explicitly leaves GTK frontend validation outstanding.

## Other exclusions and existing outreach

| Candidate | Decision at this check |
|---|---|
| [noop #2314](https://github.com/ryanbr/noop/issues/2314#issuecomment-6204360817) | Our runnable-case follow-up already exists; no external reply in the live comment list. |
| [Zcash #2476](https://github.com/zcash/librustzcash/issues/2476#issuecomment-6204361147) | Our receive/spend/rewind follow-up already exists; no external reply to it in the live comment list. |
| [setu #588](https://github.com/withsetu/setu/issues/588) | Existing SFARL reply, no new follow-up warranting another promotion. |
| [Zenod #1391](https://github.com/zenod-ai/zenod/issues/1391) | OPEN is misleading as a need signal: linked [#1393](https://github.com/zenod-ai/zenod/pull/1393) already merged hot-path fixes. Remaining all-row export/search work is a separate requirement. |
| [screenpipe #7364](https://github.com/screenpipe/screenpipe/issues/7364) | Reporter confirmed the runaway-query fix; existing SFARL reply. |
| [Pulse #2465](https://github.com/rcourtman/Pulse/issues/2465) | Latest-observation indexed-read fix released; the reported rollup itself was not the bottleneck. |
| [Stash #7222](https://github.com/stashapp/stash/issues/7222) | Query/index and per-card N+1 investigation comes before an incremental extension. |

## ivmlite contract and validation boundary

Local source inspected at `2f15933` (published alpha.5 line):
[catalog.rs](../../crates/ivmlite-sqlite/src/catalog.rs) rejects non-STRICT
tables; [compile.rs](../../crates/ivmlite-sql/src/compile.rs) limits maintained
queries to grouped bare-column COUNT/integer SUM, a single predicate and at
most one two-table inner equi-join. Outer joins, CTEs, windows, computed
aggregate expressions and the ordered ledger above are not supported.
The GitHub release list confirms `v0.1.0-alpha.5` is a published prerelease;
the `releases/latest` endpoint returns 404 because there is no stable release.

This batch read live issue/comment APIs and pinned source. It did not execute
upstream applications, deploy anything, publish replies or add new performance
claims. Only this research note was added; the existing M2a/M2b working files
were left untouched. Validate this documentation with `git diff --check` and
review the staged file list before committing.
