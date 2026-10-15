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
mapping are demonstration choices. This is not a Phoenix database replay.

## Run and result

Build the release extension as described in the [demos README](README.md),
then run:

```sh
cargo run --release --locked -p ivmlite-test \
  --example demand_case_bench -- phoenix_token_rollup 4952148 200 5
```

Use the [shared protocol](README.md#benchmark-protocol) and record raw output
under `docs/demos/results/YYYY-MM-DD-phoenix-<machine>.txt`. The source-scale
fixture is memory intensive. The 169-second upstream query and a local
synthetic benchmark differ in schema, data, indexes, filters and hardware;
they are not a before/after comparison.
The indexed baseline uses `traces(project_id, id)` and
`spans(trace_id, day_bucket, prompt_tokens, completion_tokens)`; the base
schema already indexes `spans(trace_id)`. The covering mode uses `INDEXED BY`
for both covering indexes; the ordinary recomputation mode preserves the
natural planner choice. Use the faster recomputation result as the comparison. A local hand-written trigger
baseline maintains the same project/day relation over both join inputs.

## Boundaries

Current ivmlite does not maintain a dynamic `start_time >= ?` window or a
computed timestamp bucket. A stored bucket rollup can answer aligned ranges,
but a range cutting through a bucket needs separate raw-data handling or a
finer bucket. The demo reads all maintained groups rather than the original
filtered project/time query. It does not test GraphQL/DataLoader behavior,
latency percentiles, trace trees, retention deletion, production concurrency
or Phoenix's current performance.
