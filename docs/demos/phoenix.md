# Phoenix project token rollup slice

This synthetic demo follows the expensive tracing-token query reported in
[Phoenix issue #8068](https://github.com/Arize-ai/phoenix/issues/8068). It uses
one join and integer token sums, with a stored time bucket to bound the
adaptation.

## Public evidence

Checked on 2026-10-15: the issue was opened on 2025-06-08 and closed on
2025-07-01. Its reporter published 61,006 traces and 4,952,148 spans. A
query joining spans to traces, filtering a time range and project IDs, then
grouping by project took about 169 seconds in the provided log. Its query
plan already used the trace project index and span trace index.

The query sums `llm_token_count_prompt` and
`llm_token_count_completion`. The reporter's local workaround returned zero
instead of running the query. The eventual fix,
[PR #8357](https://github.com/Arize-ai/phoenix/pull/8357), merged on
2025-07-01 and removed token-count fields from project summary views and
queries. Closure therefore means the costly UI feature was removed; it does
not demonstrate an aggregate implementation or adoption of ivmlite.

## Adaptation

The demo uses 4,952,148 synthetic span rows by default and approximately
81 spans per trace. Traces carry a project ID; spans carry integer prompt
and completion token counts and a precomputed integer time bucket. A
maintained group is keyed by project and bucket, with both token sums.

```sql
SELECT traces.project_id, spans.day_bucket,
       SUM(spans.prompt_tokens), SUM(spans.completion_tokens), COUNT(*)
FROM traces JOIN spans ON traces.id = spans.trace_id
GROUP BY traces.project_id, spans.day_bucket;
```

The fixture uses 16 projects and 30 initial day buckets. The batch inserts
spans, deletes spans, corrects token counts, reparents spans, moves a trace
to another project and corrects day buckets. Moving a trace affects its
joined children, exercising maintenance beyond individual span updates.

The application would combine buckets and apply display-time `COALESCE` or
zero filling. It must compute bucket assignments before writing the source
row. Project distribution, token values, bucket width and span-to-trace
mapping are demonstration choices. Token columns in this fixture are
`NOT NULL`; nullable upstream token fields would require an explicit
normalization policy (for example, storing zero for a missing count).
This is not a Phoenix database replay.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- phoenix_token_rollup 4952148 200 5
```

Use the [shared protocol](README.md#benchmark-protocol). The source-scale
fixture is memory intensive. The 169-second upstream query and a local
synthetic benchmark differ in schema, data, indexes, filters and hardware;
they are not a before/after comparison.
The indexed baseline uses `traces(project_id, id)` and
`spans(trace_id, day_bucket, prompt_tokens, completion_tokens)`; the base
schema already indexes `spans(trace_id)`. The covering mode uses `INDEXED BY`
for both covering indexes; the ordinary recomputation mode preserves the
natural planner choice. Use the faster recomputation result as the comparison.
A local hand-written trigger
baseline maintains the same project/day relation over both join inputs.

The [recorded run](results/2026-10-15-phoenix-m2pro.txt) on 2026-10-15 used
commit `1c61d6e`, the `0.1.0-alpha.4` extension, bundled SQLite 3.53.2 and an
Apple M2 Pro with 16 GiB memory. It used **three repeats**, rather than the
five in the example command, with fresh in-memory databases, 4,952,148 spans,
61,138 synthetic traces, 200-operation mixed batches and ten measured batches
per repeat. The trace count follows the demo's approximately 81 spans per
trace, rather than reproducing upstream's exact 61,006 traces. Values below
are medians across repeats. Total ranges are minimum–maximum across repeats;
totals use paired write + maintain + read batch sums.

| Mode | Bootstrap ms | End-to-end ms (range) | SQLite pages KiB |
|---|---:|---:|---:|
| no maintenance | 0 | 0.123 (0.119–0.125) | 157,596 |
| unindexed recompute | 0 | 1,844.186 (1,829.570–1,851.122) | 157,596 |
| indexed recompute | 1,330.246 | 1,788.722 (1,778.474–1,806.134) | 249,100 |
| hand-written triggers | 1,841.689 | 3.309 (3.289–3.404) | 157,632 |
| ivmlite | 22,632.161 | 8.702 (8.638–8.706) | 885,672 |

ivmlite's median write, steady-state refresh and read components were
1.998 ms, 6.394 ms and 0.129 ms. The first refresh was 9.279 ms.
Component medians do not sum exactly to the paired total. The faster
recomputation mode here was the covering-index mode: ivmlite was about
206 times faster end to end. Local hand-written triggers were about
2.63 times faster than ivmlite. ivmlite used about 5.62 times the base SQLite
page allocation and took 22.63 seconds to bootstrap, which are material costs
of maintaining this join. The one-minute load average fell from 4.83 to 4.46.
The command completed with SQLite oracle checks passing. These results cover
the adapted project/day relation, not the original sliding-window query or
the entire Phoenix dashboard.

## Boundaries

Current ivmlite does not maintain a dynamic `start_time >= ?` window or a
computed timestamp bucket. A stored bucket rollup can answer aligned ranges,
but a range cutting through a bucket needs separate raw-data handling or a
finer bucket. The demo reads all maintained groups rather than the original
filtered project/time query. It does not test GraphQL/DataLoader behavior,
latency percentiles, trace trees, retention deletion, production concurrency
or Phoenix's current performance.
