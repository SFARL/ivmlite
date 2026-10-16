# Community demand and English reply queue

Checked on 2026-10-16. These are public workload reports, not adoption or
endorsement. The user subsequently authorized publication and further targeted
outreach. Four replies are now published and one older reply was corrected;
the receipts below supersede this note's original draft status.

## Published and verified

All four new replies were posted as SFARL after refreshing issue status,
comments and source revisions. Each returned public body was fetched again
and compared with the intended text. Times below are America/New_York on
2026-10-16. No maintainer response or adoption is claimed.

| Thread | Time | Contribution and public receipt |
|---|---|---|
| better-ccflare #480 | 19:04:16 | [Five-query timing attribution and a conditional model-count experiment](https://github.com/StartupBros-com/better-ccflare/issues/480#issuecomment-6226326569). Discloses ivmlite authorship, schema limits and lack of app measurements. |
| context-guru #588 | 19:06:31 | [Ordered ledger, filter and lookahead correctness](https://github.com/rossoctl/context-guru/issues/588#issuecomment-6226353795). A technical contribution without recommending ivmlite for this state machine. |
| OpenWhispr #84 | 19:17:01 | [Stored approximate word counts and per-note/speaker summaries](https://github.com/futuregerald/openwhispr/issues/84#issuecomment-6226477771). Keeps main-process availability separate; discloses ivmlite boundaries. |
| Garcon #38 | 19:19:30 | [Shared mutex, exact count and upsert-safe cache accounting](https://github.com/asieke/garcon/issues/38#issuecomment-6226504644). Technical diagnosis; no extension recommendation or measured benefit claim. |

At 19:31:09 the existing [OpenVibely #345 reply](https://github.com/openvibely/openvibely/issues/345#issuecomment-5901275016)
was edited in place to correct its opening expression-index recommendation;
the remainder was preserved. SQLite treats the `localtime` modifier as
non-deterministic, so the previous suggestion did not preserve those semantics
without additional policy/state. See the [official restrictions](https://www.sqlite.org/deterministic.html).
An in-memory probe with SQLite 3.53.0 reproduced rejection both for an
expression index over populated rows and an insert into a stored generated
column using `date(created_at, 'localtime')`.

## Resume here

1. Do not repost the four published replies. Check for substantive external
   responses before following up; a lack of replies is not evidence of demand.
2. **OpenWhispr #84** is the strongest new aggregate experiment. Revisit only
   if a maintainer wants the stored-word-count / per-note summary comparison,
   or there is concrete implementation or fixture evidence to add. Existing
   main-process blocking is a separate problem, and no app benchmark was run.
3. Remove **Postio #1508** from active outreach: it closed today after an
   upstream maintained-counter implementation merged. Historical demo docs
   saying it remains open are a dated snapshot, not current status.
4. Do not repost to noop, Zcash or setu without a substantive new reason or
   an external reply. The live noop/Zcash comment lists currently contain no
   external follow-up to our October 15 results.
5. Watch **photo-triage #24 / PR #60** and **Sub/Wave #1937** for meaningful
   results from their existing fixes. Neither received a comment in this
   batch. Do not repeatedly poll or propose an extension while their simpler
   fixes are still being evaluated; revisit after busy-worker / deployment
   evidence identifies a remaining aggregate cost.
6. Further outreach should find a distinct concrete workload, inspect its
   latest code and comments, and avoid the exclusions recorded below. The
   better-ccflare fixture review is complete: full-handler measurements exist
   in #481, but the large runnable harness and per-call attribution were not
   found in the inspected files. No live database or production endpoint was
   accessed.

The app heartbeat **ivmlite** is ACTIVE, hourly, attached to this chat. It
checks both usage windows, checkpoints before stopping at a limit, and resumes
the unfinished queue on the next successful scheduled run after quota returns.
It stays quiet without meaningful results. Unattended continuation prepares
new replies and does not post comments or push; the publication receipts above
record the separate user-authorized batch completed in this turn.
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

### Published English text — diagnosis and a small evaluation target

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

### Published English text — technical contribution, no extension recommendation

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

The initial research batch read live issue/comment APIs and pinned source.
The subsequent user-authorized batch published the replies listed above. It
did not execute upstream applications, deploy, push or add performance claims.
Only this research note was changed; existing M2a/M2b working files were left
untouched. Validate this documentation with `git diff --check` and review the
staged file list before committing.


## Additional source checks after publication authorization

### OpenWhispr #84 — new aggregate experiment, reply published

[Issue #84](https://github.com/futuregerald/openwhispr/issues/84) was OPEN with
zero comments before our reply. Main remained
`d5e50fed042598c43152a20b0bf62c0cd1baa251` at posting. The report measured the
actual DatabaseManager on a synthetic 20k-note / 2M-segment fixture: isolated
by-speaker query 1,200 ms, all-time stats including speakers 1,492 ms, and
one-month stats 38 ms. These are the reporter's measurements, not ours.

- [By-speaker SQL](https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L2861-L2886)
  joins segments to notes, excludes NULL speakers and deleted notes, optionally
  filters note dates, computes an approximate word count from text, and groups
  before LIMIT. The repeated text expression is a concrete contribution to
  investigate; no claim is made that it is the sole remaining cost.
- [Existing schemas and indexes](https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L840-L859)
  are non-STRICT. [Tool parameters](https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/mcp/tools.js#L475-L498)
  do not provide default date bounds. The current SUM expression and compound
  filter cannot be registered as-is in ivmlite.
- A smaller per-note/per-speaker summary would retain note-level eligibility
  and date filtering while avoiding repeated raw text counting. This is a
  hypothesis, not an implemented or measured integration. Preserve the exact
  upstream approximate counting rule, including its behavior on whitespace.
- Any comparison must include edits, speaker changes, note soft-delete/restore,
  date changes, bootstrap, write and refresh costs. Electron extension loading
  remains unverified. Moving synchronous reads off the main process addresses
  availability independently of aggregate acceleration.

Published body (verified against GitHub):

I checked the [current implementation at `d5e50fed`](https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L2861-L2886). The by-speaker path still reads segment text for the `trim`/`length`/`replace` word-count expression, joins notes for soft-delete/date eligibility, and groups before applying the speaker limit. A date index and an index-usable predicate should help bounded windows, but do not remove the work in an intentional all-time request. Moving this read off Electron's main process remains the availability fix.

For repeated summaries, a separate experiment would be to store the **existing approximate word count** when segment text changes and maintain per-note/per-speaker segment counts and word sums. Joining those smaller summaries to notes could preserve note-date and soft-delete filtering without rereading every segment's text. The comparison should cover text edits, speaker remapping, note deletion/restoration and changed dates against the current SQL, preserving its counting rule rather than switching tokenizers.

Disclosure: I maintain [ivmlite](https://github.com/SFARL/ivmlite), an alpha SQLite extension for grouped counts and integer sums. Stored scalar contributions are the kind of workload I am evaluating, but the existing query and non-STRICT schema are not a drop-in fit, and I have not verified Electron integration. Your 20k-note/2M-segment fixture would be useful for comparing indexed SQL, a hand-maintained summary and explicit batch refresh, including write/refresh costs. I have not benchmarked this application; that experiment should remain separate from fixing main-process blocking.

### Garcon #38 — useful diagnosis, not an ivmlite fit

[Issue #38](https://github.com/asieke/garcon/issues/38) was OPEN with zero
comments before our reply. Main remained
`5a0ec29c4b521531deea704c9cf5cfb13253a03e`. It reports dashboard interference
with proxy requests but supplies no public cardinality or quantified latency;
do not invent either.

- [Connection setup](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/database/database.go#L38-L43)
  uses one connection. [The store](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/store.go#L40-L139)
  also shares a mutex between polling/counting and saving; connection splitting
  alone leaves that dependency intact.
- SaveRecord reconciles imported records and upserts by request_id. A cached
  row count must distinguish a committed insertion from an update or skip.
- [Session inventory SQL](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/sessions.go#L17-L57)
  uses windows, JSON, DISTINCT and multiple joins before the final LIMIT.
  Current ivmlite does not maintain that query. This reply offers source-based
  debugging and correctness guidance without recommending an extension.

Published body (verified against GitHub):

I checked current `main` at `5a0ec29c`. There are two serialization layers here: the single database connection, and the store mutex. [`RequestFeed()`](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/store.go#L99-L139) holds `s.mu` while running the exact row count, and `SaveRecord()` takes that same mutex before beginning its transaction. A separate read pool would therefore help database admission but would not, by itself, remove this count-versus-recording dependency.

A cached total needs one correctness detail: [`SaveRecord()`](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/store.go#L54-L78) reconciles imported records and upserts by `request_id`, so incrementing the total for every save would overcount streaming-to-complete updates and repeated imports. I would test those transitions, reconciliation skips, rollback and restart/bootstrap against `COUNT(*)` before moving the count out of the polling path.

The [`sessions()` LIMIT](https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/sessions.go#L17-L57) also comes after the window/grouping work, so reducing the returned row count does not bound that scan. A useful benchmark would run proxy starts/completions with the dashboard closed versus polling, recording pool wait and mutex wait separately from SQL execution. That would distinguish connection separation from the remaining aggregation work. I have not run the application or measured those waits.

### Watch list — do not post a redundant promotion

| Candidate | Current evidence and concrete revisit trigger |
|---|---|
| [photo-triage #24](https://github.com/MusikPolice/photo-triage/issues/24), [PR #60](https://github.com/MusikPolice/photo-triage/pull/60) | Photos/job queue COUNT by stage/status is real. PR head `70b7abfefcb980b6adff4ede9addb2451c65a3e6` is OPEN, unmerged, and reports a 1M-row synthetic warm-read median of 46.1–47.7 ms after index/range-count changes, versus 493.6 ms before. Its data_version gate skips unchanged reads; the issue explicitly defers running totals. Wait for home-server and busy-worker evidence that the current approach still misses the target. No comment posted. |
| [Sub/Wave #1937](https://github.com/perminder-klair/subwave/issues/1937) | 150,291 tracks / 600,987 facet rows is real scale evidence. The [October 16 owner reply](https://github.com/perminder-klair/subwave/issues/1937#issuecomment-6225655379) identifies repeated wide tracks scans, explicitly correcting the earlier facet/prefetch attribution. A cache/index fix is written and in review. Wait for its linked PR and deployment results; vector-centroid cost is a separate problem. No comment posted. |
| [Tautulli #1962](https://github.com/Tautulli/Tautulli/issues/1962) | A recent report uses stable v2.18.2 and roughly 536k history rows, while inspected nightly `668ef00718faac90dcd18e68fa965bf38f3d9444` already aggregates before wide metadata joins and caches totals by history generation. Need current-nightly confirmation, not another promotion against an old query. DISTINCT/MAX/outer joins also exceed our subset. |
| [Unscreen #30](https://github.com/Sakth1/Unscreen/issues/30) | Summary feature idea without a measured slow-query case; current ADR says existing queries are adequate. REAL durations, open-session time-dependent contributions and Android packaging are unverified adaptations. Revisit only with a concrete scale/latency report. |
| [Orpheus #127](https://github.com/scottchronicity/orpheus/issues/127) | The reported 31.6-second problem was already fixed with SQL/index changes; a summary is a future escalation after larger-window evidence. The issue explicitly forbids triggers and write-path changes, incompatible with ivmlite capture. No comment posted. |

### What was validated in this follow-up

Four new comments and the corrected OpenVibely comment were fetched from
GitHub and matched to their intended bodies. Upstream source was inspected at
the fixed revisions above; no upstream app or benchmark was executed. The only
executable diagnostic was the SQLite localtime determinism probe, which
rejected both tested persistent-expression uses as documented. Existing
M2a/M2b code, workloads and untracked files were not changed. Documentation
validation covers local links and `git diff --check`; it does not establish
external adoption or application-level performance.
