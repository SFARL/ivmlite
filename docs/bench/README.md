# M0 baseline benchmark: results and conclusions

`ivmlite-bench` (`crates/ivmlite-bench`) runs three same-host baselines (spec §10.2):

- `no_maintenance` — the lower bound: write the base table and maintain no views at all.
- `hand_written_trigger` — the skeptic: summary tables maintained incrementally by hand-written SQLite triggers.
- `naive_recompute` — the baseline: re-run every view's SQL after each batch of deltas (read only, results not written back).

M0 has no real IVM engine (that is M1's work), so what is filled in here is the relationship **between the three baselines**; the table's structure is fixed now, and M1 adds the incremental system's numbers to the same matrix.

## The matrix that was run

The **full matrix**, not reduced:

- `BASE_ROWS = [10_000, 100_000, 1_000_000]`
- `BATCH_SIZES = [1, 10, 100, 1000]`
- `VIEW_COUNTS = [1, 10, 50, 200]`
- `GROUP_CARDINALITIES = [10, 1_000, 100_000]`
- Two sweeps (the group-cardinality sweep fixes `views=10`; the view-count sweep fixes `cardinality=1000`), 32 + 36 = 68 cells per baseline, 204 rows of data across the three baselines (`docs/bench/m0-baseline.csv`, 205 lines with the header).

End to end, in a full release build, the run took 557 seconds (about 9.3 minutes).

One combination, `card=100000 > base_rows=10000`, is skipped, leaving out **12 rows** of CSV data (3 baselines × 4 batch sizes). The guard exists only in sweep one (the group-cardinality sweep): sweep two fixes `cardinality=1000`, which is not larger than any `base_rows` tested, so sweep two never triggers the skip.

This is not a missing run but a configuration that cannot exist — a 10,000-row table cannot hold 100,000 distinct group keys. `ivmlite-workload::Workload::validate` rejects such a configuration, and `Workload::cells()` simply does not produce it when expanding the matrix, so the runner has, and should have, no second copy of the rule.

## What changed in this run: statement compilation is no longer timed

The numbers here come from a re-run after fixing how cells were timed (external review P2-3, 2026-09-22). Previously `apply` and `recompute_all` started the timer and *then* prepared their statements. Creating a trigger changes the schema and invalidates statements compiled before it, and SQLite compiles a trigger's body into the statement that fires it — so the hand-written-trigger baseline paid statement recompilation inside the timed region, and because each cell runs exactly once, that first-call cost was all a cell recorded. Statements are now prepared after every trigger exists and before the timer starts, for all three baselines.

The effect is large exactly where it should be and nowhere else:

| `hand_written_trigger.apply_ms`, median over the 17 cells at each batch size | batch=1 | batch=10 | batch=100 | batch=1000 |
|---|---|---|---|---|
| this run / previous run | 0.10 | 0.76 | 1.17 | 1.28 |

At batch=1 the trigger's time fell to a tenth: the previous run was mostly measuring compilation there. At batch=100 and batch=1000, where compilation is negligible next to the work, the fix cannot make anything slower — yet the ratios are above 1. That is not an effect of the fix; see the next section.

## Limits of the measurement

**Each cell was measured once**, with no warm-up and no median over repeats. This run also gives a direct estimate of what that costs. Where the fix cannot plausibly matter, comparing this run with the previous one shows how far two runs of the same work drift apart:

| Cells the fix barely affects | min | Q1 | median | Q3 | max |
|---|---|---|---|---|---|
| `naive_recompute`, base_rows=1M (the scan dwarfs compilation), 24 cells | 1.03 | 1.04 | 1.07 | 1.11 | 1.59 |
| `no_maintenance`, batch=1000, 17 cells | 0.58 | 1.01 | 1.13 | 1.27 | 1.47 |
| `hand_written_trigger`, batch=1000, 17 cells | 1.01 | 1.24 | 1.28 | 1.32 | 1.79 |

The drift is not symmetric noise around 1: nearly every such cell was slower this run, so this run sat about 5–30% slower overall than the previous one — a property of the machine and session, not the code. Two consequences for reading everything below:

- Comparisons **within one run** are the ones to trust; comparisons **across runs** are trustworthy only where the effect dwarfs a ~1.3x shift.
- A difference between two cells smaller than roughly 1.3x is not a finding.

The smallest cells show the noise plainly: `no_maintenance`'s `apply_ms` at `views=10, batch=1, card=10` is 0.005 ms at `base_rows=10,000`, 0.003 ms at `100,000`, and 0.006 ms at `1,000,000` — the middle, larger table cheaper than the smallest, which cannot happen physically. These microsecond readings carry noise, and not every wiggle is a signal.

## Sanity check from the M0 smoke run

*This section is a historical record from M0 development: a reduced matrix (`BASE_ROWS=[1_000,10_000]`, `VIEW_COUNTS=[1,10]`, `GROUP_CARDINALITIES=[10,1_000]`) run before the P2-3 fix. `base_rows=1,000` is not in the published matrix, so these numbers cannot be re-derived from the CSV.*

It checked the key expectation: at a fixed `base_rows`, `naive_recompute.maintain_ms` should barely move with `group_cardinality` (it always scans the whole table), while `hand_written_trigger.apply_ms` should rise with `group_cardinality`. Observed at `batch=1000, views=10`:

| base_rows | naive_recompute.maintain_ms (card=10 → 1000) | hand_written_trigger.apply_ms (card=10 → 1000) |
|---|---|---|
| 1,000  | 2.281 → 2.558 ms (+12%) | 9.227 → 12.610 ms (+37%) |
| 10,000 | 16.533 → 18.956 ms (+15%) | 7.492 → 10.828 ms (+45%) |

`naive_recompute` barely moved (a single-digit-to-low-teens percentage, with its magnitude set by `base_rows`), while `hand_written_trigger` clearly rose (30–45%) — the cardinality dimension really does take effect.

It also confirmed that locating rows by primary key does not produce a `SCAN`:

```
$ sqlite3 :memory: "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT; EXPLAIN QUERY PLAN DELETE FROM orders WHERE id = 1;"
QUERY PLAN
`--SEARCH orders USING INTEGER PRIMARY KEY (rowid=?)
```

No `SCAN`, as expected.

## Conclusion table: full recompute vs hand-written triggers

Fixing `views=10` and `base_rows=1,000,000` (the largest size in the matrix, which best shows "incremental cost does not grow with the base table"), with `Δ size` at its two extremes, `1` and `1000`:

| Group cardinality | Δ size | Full recompute / hand-written trigger | Hand-written trigger's write amplification |
|---|---|---|---|
| 10     | 1 / 1000 | 314,251.5 / 240.6 | 1.8x / 12.6x |
| 1k     | 1 / 1000 | 146,665.2 / 216.7 | 4.5x / 14.9x |
| 100k   | 1 / 1000 | 225,644.1 / 128.8 | 2.8x / 38.9x |

The "ratio" is `naive_recompute`'s total time (apply_ms + maintain_ms) divided by `hand_written_trigger`'s total time (the trigger's whole cost is in apply_ms; its maintain_ms is always 0). "Write amplification" is `hand_written_trigger.apply_ms` as a multiple of `no_maintenance.apply_ms` — the extra write cost the triggers add to maintain the summary tables.

Read the Δ=1 column with the measurement limits in mind: at batch=1 the trigger's total time is on the order of hundredths of a millisecond, so those ratios divide a multi-second recompute by a denominator that single-run noise moves easily. The magnitude — five orders — is the finding; the differences between the three Δ=1 ratios are not. The previous run's Δ=1 write amplifications (7.2x / 8.5x / 16.7x) were mostly statement compilation; see "What changed in this run".

## What the surface actually looks like (not a single crossover)

Across the whole 204-row matrix, the lowest total-time ratio of `naive_recompute` over `hand_written_trigger` is **1.56** (`views=200, base_rows=10,000, batch=1000, card=1000`: naive 358.150 ms vs trigger 230.045 ms), and **there is no cell where `naive_recompute` is faster**. The ratio moves along three dimensions:

- **The larger `base_rows`, the more extreme the ratio**: at `views=10, card=10, batch=1` it grows from 2,680.6x at `base_rows=10,000` to 314,251.5x at `base_rows=1,000,000`. `naive_recompute`'s cost grows linearly with the base table while `hand_written_trigger`'s barely moves — incremental cost not growing with the base table is exactly what the benchmark exists to show.
- **The larger the batch, the narrower the ratio**: at the same `views=10, card=10, base_rows=10,000`, going from batch 1 to 1000 takes the ratio from 2,680.6x down to 1.87x — the trigger's one `ON CONFLICT` update per row starts to approach the cost of one full scan.
- **View count: no reproducible shape.** At `base_rows=10,000, batch=1000, card=1,000`, view counts 200 / 50 / 10 / 1 give ratios of **1.56 / 1.80 / 2.69 / 1.58** in this run. The previous run gave 1.73 / 2.00 / 1.96 / 1.42 for the same cells, and the earlier version of this README cautioned that its non-monotonic shape "might be single-measurement noise". A second run settles it: the shape does not reproduce, and every one of these ratios sits within a factor of two of the others at this size, well inside the drift measured above. Only the endpoints' shared fact survives both runs — at this small base table and large batch, the ratio stays under 3x for every view count.

### Group cardinality: what reproduces across two runs

The one sweep that isolates cardinality (fixed `views=10`, cardinality over 10 / 1,000 / 100,000) gives, in this run:

| base_rows | batch | card=10 → 1,000 → 100,000 |
|---|---|---|
| 10,000 | 1 | 2,680.6 → 2,070.5 (no `card=100,000` cell) |
| 10,000 | 10 | 146.7 → 125.8 |
| 10,000 | 100 | 15.4 → 24.6 |
| 10,000 | 1000 | 1.9 → 2.7 |
| 100,000 | 1 | 18,764.0 → 41,895.3 → 22,592.4 |
| 100,000 | 10 | 1,574.9 → 2,296.8 → 1,060.4 |
| 100,000 | 100 | 205.2 → 211.5 → 96.1 |
| 100,000 | 1000 | 29.6 → 21.1 → 9.5 |
| 1,000,000 | 1 | 314,251.5 → 146,665.2 → 225,644.1 |
| 1,000,000 | 10 | 21,899.6 → 14,974.5 → 10,006.9 |
| 1,000,000 | 100 | 2,340.4 → 1,930.6 → 939.2 |
| 1,000,000 | 1000 | 240.6 → 216.7 → 128.8 |

**The earlier version of this README drew a directional conclusion that this run contradicts.** From one run it concluded "narrows at large Δ, widens at Δ=1, with all three base-table sizes moving the same way at Δ=1". Here, at `base_rows=10,000`, Δ=1 *narrows* (2,680.6 → 2,070.5), Δ=100 and Δ=1000 *widen* (15.4 → 24.6, 1.9 → 2.7), and at Δ=1 the two larger tables are non-monotonic. That conclusion was an over-reading of one noisy run, and it is withdrawn.

What **does** reproduce, in both runs, in every cell that qualifies — and it is insensitive to the P2-3 fix, since compilation is negligible at these batch sizes:

- **At `base_rows ≥ 100,000` and batch ≥ 100, raising cardinality from 1,000 to 100,000 cuts the ratio roughly in half.** This run: 211.5 → 96.1, 21.1 → 9.5, 1,930.6 → 939.2, 216.7 → 128.8. Previous run: 215.0 → 88.8, 24.0 → 10.0, 2,254.6 → 957.2, 258.8 → 149.7. Every one of these eight steps is a factor of 1.7–2.4, far outside the run-to-run drift.

Everything else in the table — the direction at `base_rows=10,000`, anything at Δ=1, and the 10 → 1,000 step — either changes between runs or moves by less than the drift, and supports no directional claim.

- The view-count observation and the cardinality observation above are **two independent observations and cannot be combined**: the matrix is two independent sweeps (one fixing `views=10` and sweeping cardinality, one fixing `cardinality=1,000` and sweeping views), not a full four-way cross, so the "few views + low cardinality" corner was never covered by any single measurement — this data says nothing about it either way.
- **Noted separately, and not an M0 finding**: the `ivmlite` project's own hypothesis is that "large Δ + low group cardinality" will be where M1's real incremental engine (not the hand-written triggers here) has its largest advantage over full recomputation — because at large batches, repeated changes to the same group can be merged when the delta is consumed (delta consolidation), a mechanism triggers do not have, since every row goes through its own `ON CONFLICT`. That is a hypothesis left for M1 to test, not a conclusion of this M0 baseline data.

**M0 has no incremental engine to measure**, so the criterion "if no region of this surface gives incremental maintenance a substantial advantage over full recomputation (a ratio > 2), the project's premise does not hold" can only be applied to the "incremental system vs full recompute" pair once M1 exists. What can be reported honestly here is that `hand_written_trigger` (a hand-written, non-general incremental scheme) beats `naive_recompute` in all 204 cells tested, with the ratio still above 1.5 at its narrowest.

Under spec §10.2's three-tier bar, `hand_written_trigger` is the **skeptic**, not a threshold, and **not** something "v0 must beat":

| Tier | Bar | Meaning |
|---|---|---|
| **Must** | `ivmlite ≪ full recompute` | If not met, the project's premise does not hold |
| **Expected** | `ivmlite` close to hand-written triggers | The price of generality is acceptable |
| **Bonus** | `ivmlite` **faster than** hand-written row-level triggers on large Δ | The structural advantage consolidation brings |

`no_maintenance` is the **lower bound** spec §10.2 assigns (pure write cost, no views maintained at all); `hand_written_trigger` is the **special-purpose upper bound** — one of the best implementations of this one query, compiled by hand. Once M1 plugs in a real engine, it must prove the "must" tier on this same matrix (a substantial advantage over `naive_recompute`); the "expected" and "bonus" tiers are extra credit, not the pass mark.

## Charts

`docs/bench/m0-baseline-card{10,1000,100000}.svg`, one each: fixed `views=10, batch=100`, with `base_rows` on the x axis (log) and total `apply_ms + maintain_ms` on the y axis (log), one line per baseline. The three charts correspond to the three group cardinalities — the crossover and the gaps move sharply with cardinality, and mixing them on one chart would draw a meaningless line (spec §10.1).

## Raw data

`docs/bench/m0-baseline.csv`: `baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms`, 204 rows of data plus a header.
