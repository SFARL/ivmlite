# Benchmark results and conclusions

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

---

## M1b Phase 4: the SQLite extension

Phase 4 adds `ivmlite` as the fourth engine and answers the question M0 could
not: where does the real extension beat full recomputation, what does its
generality cost against hand-written triggers, and how much tax does it add to
ordinary writes?

Every measured number in this section is derived from the committed CSVs by
[`phase4_tables.py`](phase4_tables.py) (`python3 docs/bench/phase4_tables.py`).
The exceptions are the setup facts below — dates, hardware, versions and the
four wall times, which come from the run session and are in no CSV — and the
protocol constants, which come from the runner's code.

> **Note (2026-10-11, M1b Phase 6):** every cell in this section ran one batch
> in a fresh database, so its `maintain` is the **first refresh after
> `CREATE`**, which pays one-time costs a later refresh does not. Phase 6
> measures the steady state instead, and its numbers are not comparable column
> for column with these (see "M1b Phase 6" below).

### Setup and protocol

The measurements ran in one otherwise idle session on 2026-09-29–30 on a
10-core Apple M2 Pro MacBook Pro with 16 GB RAM, macOS 26.2, and the SQLite
3.53.2 that `rusqlite` bundles (the bench loads the extension into it). The
extension and runner were release builds. Wall times were 3,079.23 seconds for
the exploration matrix, 475.10 seconds for confirmation, 4,460.33 seconds for
the one `scripts/bench-ablation.sh` run (all three builds, each measured on the
ablation cells and on the ablation's write-amplification workload), and 577.00
seconds for the full write-amplification run.

Every matrix cell followed Phase 4 spec §3.2: bootstrap, verify the initial
state, prepare, time apply, time maintain with nothing between those regions,
then verify the final state. A mismatch would have aborted the run; all cells
passed. The exploration contains one observation per cell. The 13 cells the
confirmation rule selected were then independently repeated five times.

The measurement protocol's constants live in the runner, not in a workload
file (spec §3.5, §8):

- **Confirmation rule.** A cell is re-run if its exploration speedup
  (`naive_recompute`'s `apply + maintain` over `ivmlite`'s) lies in
  **[0.7, 1.4]** or **[1.4, 2.8]**, or if `ivmlite`'s `apply + maintain` is
  below `hand_written_trigger`'s. No cell met the third clause; the first two
  selected the 13 cells in `m1b-phase4-confirm.csv`, exactly.
- **K = 5** repeats per selected cell, each in a fresh database.
- **Engine rotation.** Matrix cell *i* runs `no_maintenance`,
  `hand_written_trigger`, `naive_recompute`, `ivmlite` rotated left by
  *i* mod 4. Repeat *r* of the *i*-th selected cell uses rotation *i* + *r*.
  The ablation alternates its two engines by repeat, and the
  write-amplification runner rotates its three engines by combination index.

The ablation's cells, its five repeats (`[ablation] repeats`) and its
write-amplification view counts (`ablation_view_counts`) are workload
parameters, in `workloads/m0-baseline.toml` and `workloads/write-amp.toml`.

These results cover one refresh policy: **refresh every view once after each
batch** (in the write-amplification workload, once after each op trace,
untimed). They are a slice of §10.4's surface, not a measurement of the
refresh-frequency dimension. The data is uniform and all databases are
in-memory.

### Reproducing

From the repository root, at v0.1.0-alpha.2 (c0f2123):

```bash
scripts/bench.sh matrix > docs/bench/m1b-phase4.csv
scripts/bench.sh confirm --from docs/bench/m1b-phase4.csv > docs/bench/m1b-phase4-confirm.csv
scripts/bench-ablation.sh before=a7c9ec7 index=756bb5b latch=9c6ff90
scripts/bench.sh write-amp > docs/bench/m1b-phase4-write-amp.csv
cargo run --release -p ivmlite-bench --locked -- plot --from docs/bench/m1b-phase4.csv
python3 docs/bench/phase4_tables.py
```

`scripts/bench.sh` builds the release extension and runner first;
`scripts/bench-ablation.sh` builds each named commit in its own worktree and
writes `m1b-phase4-ablation.csv` and `m1b-phase4-ablation-write-amp.csv`. The
labels map to commits in
[`m1b-phase4-ablation-builds.csv`](m1b-phase4-ablation-builds.csv).

### The result: the premise survives, with a clear losing region

The §10.4 falsification bar is met. In the exploration, 55 of 68 cells had
`naive_recompute / ivmlite > 2`. All 24 cells at one million base rows were
above 2x, ranging from 17.078x to 909.228x. At 100,000 rows, 22 of 24 cells
were above 2x. Five cells at 100,000 rows were confirmed near the 2x bar.
Four stayed above it in every repeat:

| views | base rows | batch | groups | confirmed speedup median [min, max] |
|---:|---:|---:|---:|---:|
| 10 | 100,000 | 1,000 | 1,000 | 2.576x [2.563, 2.642] |
| 10 | 100,000 | 1,000 | 100,000 | 2.174x [2.107, 2.700] |
| 50 | 100,000 | 1,000 | 1,000 | 2.352x [2.332, 2.387] |
| 200 | 100,000 | 1,000 | 1,000 | 2.042x [2.024, 2.052] |

The fifth missed it: `views=1, base_rows=100,000, batch=1000, groups=1000` had
a confirmed median of 1.965x, below 2x, although its five runs ranged from
1.703x to 3.808x.

The losing region is the small-table corner. Seven of the 20 exploration cells
at 10,000 base rows were below 1x. Two of them were close enough to the
boundary to be confirmed, and both remained losses: `views=10, batch=1000,
groups=10` had median 0.949x [0.939, 0.968], and `views=200, batch=10,
groups=1000` had median 0.938x [0.934, 0.955]. The other five are single
exploration runs, not repeated estimates: 0.168x at one view, 0.237x at 10
views, 0.223x at 50 views and 0.205x at 200 views, all with `batch=1000`, and
0.663x at `views=200, batch=100` (all at `groups=1000`).

Three more confirmed cells at 10,000 rows are too close to 1x to count as
wins. Two straddle it: `views=1, batch=100, groups=1000` had median 1.087x
[0.817, 1.780], and `views=50, batch=100, groups=1000` had median 1.087x
[0.899, 1.100]. The third, `views=200, batch=1, groups=1000`, had median
1.021x [1.008, 1.038]: above 1x in every repeat, but barely.

The transition is not one clean crossover: the fifth 100,000-row cell above,
whose runs spanned 1.703x to 3.808x around a median below 2x, is the clearest
example. The charts therefore show a
surface and explicit 1x/2x reference lines rather than declaring one row-count
threshold.

### The three bars against hand-written triggers

- **Must — much faster than full recomputation:** met over a broad region, and
  missed in the small-table corner described above.
- **Expected — close to hand-written triggers:** not met under this workload.
  The closest confirmed cell was still 1.92x slower (median, range
  1.85–1.97x) at `views=10, base_rows=10,000, batch=1000, groups=10`.
  Eight of the other twelve confirmed cells were 4.63–15.35x slower (medians).
  The four tiny-batch cells were far slower: 41.76x at 50 views and 106.48x at
  200 views with `batch=10`, and 1,134.22x at 50 views and 4,529.00x at 200
  views with `batch=1`.
- **Bonus — beat hand-written triggers:** not observed. No exploration cell and
  no confirmation repeat had a lower `apply + maintain` time than the
  hand-written trigger. The gap is in maintenance, not in writes: `ivmlite`'s
  `apply_ms` alone was lower than the trigger's in 41 of the 68 exploration
  cells (see the matrix write amplification below), but every refresh then pays
  fixed machinery that the special-purpose trigger avoids, which dominates for
  tiny batches and many views.

The outcome is useful but narrower than the optimistic story: ivmlite wins its
required comparison with recomputation as the base table grows, while this v0
implementation does not approach the special-purpose trigger closely enough.

### Ablation: output index and one-scan latch

The formal ablation used three reviewed builds: `before` (`a7c9ec7`, before the
output index), `index` (`756bb5b`, after it) and `latch` (`9c6ff90`, after the
one-scan latch). Each value below is `apply + maintain` in milliseconds,
reported as median [min, max] over five runs.

| build | views | base rows | batch | groups | median [min, max] ms |
|---|---:|---:|---:|---:|---:|
| before | 10 | 100,000 | 1,000 | 1,000 | 159.929 [154.841, 160.073] |
| index | 10 | 100,000 | 1,000 | 1,000 | 91.540 [91.082, 105.971] |
| latch | 10 | 100,000 | 1,000 | 1,000 | 87.390 [86.845, 88.216] |
| before | 10 | 100,000 | 1,000 | 100,000 | 17,615.413 [17,461.897, 17,676.747] |
| index | 10 | 100,000 | 1,000 | 100,000 | 133.716 [133.031, 137.998] |
| latch | 10 | 100,000 | 1,000 | 100,000 | 127.472 [126.729, 140.859] |
| before | 200 | 100,000 | 1,000 | 1,000 | 2,997.382 [2,976.901, 3,034.808] |
| index | 200 | 100,000 | 1,000 | 1,000 | 1,709.695 [1,700.689, 1,721.125] |
| latch | 200 | 100,000 | 1,000 | 1,000 | 1,655.998 [1,655.841, 1,664.103] |
| before | 200 | 100,000 | 100 | 100,000 | 19,150.636 [18,701.690, 20,460.172] |
| index | 200 | 100,000 | 100 | 100,000 | 1,329.633 [1,306.762, 1,394.654] |
| latch | 200 | 100,000 | 100 | 100,000 | 1,301.640 [1,243.658, 1,367.619] |

The output index is the decisive fix: it cuts the 10-view, high-cardinality
cell by a median 131.7x and the 200-view, high-cardinality cell by 14.4x.

The one-scan latch lowers the medians by only 4.5%, 4.7%, 3.1% and 2.1% (table
order). That is because the latch changes only `apply`, and `apply` is a small
part of `apply + maintain`: in the `index` build its median share was 11.1%,
7.5%, 4.1% and 0.5% in the four cells, and refresh (`maintain`) is the rest.
Within `apply` the effect is large — the median fell from 10.206 to 7.461 ms,
10.096 to 7.512 ms, 70.831 to 38.322 ms and 7.348 to 4.350 ms. End to end, two
cells cannot tell the `index` and `latch` builds apart, because their ranges
overlap: `views=10, groups=100,000` ([133.031, 137.998] against
[126.729, 140.859]) and `views=200, batch=100, groups=100,000`
([1,306.762, 1,394.654] against [1,243.658, 1,367.619]). In the other two
cells every `latch` run was faster than every `index` run.

The direct write effect is clearer, and it includes a regression the index
build introduced. The table gives µs per written row for each build, from the
ablation's write-amplification workload (100,000 base rows, traces of 1,000
writes; `ivmlite` only):

| views | op | OFF before | OFF index | OFF latch | ON before | ON index | ON latch |
|---:|---|---:|---:|---:|---:|---:|---:|
| 10 | insert | 17.982 | 20.332 | 15.477 | 18.580 | 20.079 | 15.370 |
| 10 | delete | 5.457 | 5.757 | 5.080 | 5.507 | 5.016 | 4.983 |
| 10 | update | 18.204 | 21.438 | 15.609 | 18.259 | 19.682 | 15.684 |
| 10 | replace rowid | 24.236 | 25.566 | 21.677 | 24.306 | 25.670 | 21.692 |
| 10 | replace unique | 24.375 | 25.982 | 22.092 | 24.063 | 25.332 | 21.391 |
| 10 | replace two | 28.560 | 30.227 | 26.473 | 27.766 | 29.985 | 25.409 |
| 200 | insert | 91.593 | 110.487 | 61.194 | 91.182 | 108.843 | 59.956 |
| 200 | delete | 5.808 | 4.985 | 5.068 | 5.180 | 5.028 | 5.114 |
| 200 | update | 92.982 | 110.283 | 61.925 | 92.487 | 111.537 | 60.471 |
| 200 | replace rowid | 98.189 | 116.182 | 67.171 | 98.003 | 116.003 | 67.373 |
| 200 | replace unique | 97.693 | 115.996 | 68.482 | 97.408 | 115.606 | 68.037 |
| 200 | replace two | 102.368 | 120.244 | 72.469 | 105.499 | 119.390 | 72.049 |

- **The index made writes slower.** From `before` to `index`, every operation
  that runs the latch (all but delete) rose by 13.89–19.05 µs per row at 200
  views, and by 1.27–3.23 µs at 10 views. The cause is the schema, not the
  index's own maintenance: each view's index is one more `sqlite_schema` row,
  and the four-scan latch reads all of `sqlite_schema` four times per written
  row, so its cost grows with every schema row (spec §2). The same rise shows
  in the end-to-end cells above, where median `apply` went from 9.543 to
  10.206 ms (10 views) and from 62.559 to 70.831 ms (200 views,
  `groups=1,000`).
- **The latch more than recovers it.** From `index` to `latch`, the same
  operations fell by 47.34–51.07 µs per row at 200 views (1.657–1.844x) and by
  3.75–5.83 µs at 10 views (1.142–1.373x).
- **The net effect of both fixes on writes** is the `before → latch` ratio:
  1.413–1.529x at 200 views and 1.079–1.209x at 10 views, not the larger
  `index → latch` ratio.
- **Deletes do not run the latch** and stay at 4.983–5.808 µs across builds,
  modes and view counts.

Each build's write-amplification run was measured **once**: every cell in this
table is a single observation. Its CSV's `extra_us_per_row` column is computed
against that run's `no_maintenance` rows, which the script measures but does
not publish (it keeps only the `ivmlite` rows).

### Write amplification

The full write-amplification workload uses 100,000 base rows and traces of
1,000 writes; it was run once. Each cell below is `engine.apply /
no_maintenance.apply`, with the extra µs per written row in parentheses, for
both engines and both `recursive_triggers` modes. At zero views the multiple
stays at 0.86–1.33x for `ivmlite` and 0.77–1.14x for the trigger, and the
extra cost within -0.73–+0.78 and -1.17–+0.63 µs/row: that is the run's noise
floor.

With `recursive_triggers` OFF, SQLite does not fire the hand-written `AFTER
DELETE` trigger for a row REPLACE removes by conflict, so the trigger's summary
is wrong after a REPLACE and those cells (marked †) are **cost only**; they
also understate the trigger's cost, since it skips work. All other cells were
verified against the oracle for both engines.

| views | op | ivmlite OFF | trigger OFF | ivmlite ON | trigger ON |
|---:|---|---:|---:|---:|---:|
| 1 | insert | 11.19x (+11.52) | 3.19x (+2.47) | 15.85x (+17.43) | 2.83x (+2.15) |
| 1 | delete | 2.24x (+2.87) | 2.14x (+2.65) | 1.99x (+2.24) | 1.89x (+2.01) |
| 1 | update | 13.65x (+15.87) | 2.71x (+2.14) | 14.01x (+12.39) | 3.69x (+2.56) |
| 1 | replace rowid | 4.40x (+14.99) | 1.39x (+1.74) † | 4.24x (+15.27) | 1.89x (+4.20) |
| 1 | replace unique | 4.52x (+15.27) | 1.24x (+1.05) † | 4.45x (+14.99) | 1.64x (+2.78) |
| 1 | replace two | 3.69x (+17.35) | 1.15x (+0.99) † | 3.19x (+16.24) | 1.43x (+3.20) |
| 10 | insert | 13.70x (+14.57) | 11.46x (+12.00) | 13.67x (+14.38) | 11.46x (+11.87) |
| 10 | delete | 2.50x (+2.90) | 8.01x (+13.54) | 2.60x (+2.94) | 8.44x (+13.69) |
| 10 | update | 15.66x (+14.82) | 23.27x (+22.51) | 18.70x (+14.96) | 27.57x (+22.45) |
| 10 | replace rowid | 5.02x (+17.67) | 3.51x (+11.04) † | 5.34x (+18.65) | 6.73x (+24.63) |
| 10 | replace unique | 4.15x (+16.97) | 2.86x (+9.98) † | 4.85x (+17.18) | 6.61x (+25.00) |
| 10 | replace two | 4.23x (+20.40) | 2.83x (+11.59) † | 4.18x (+19.89) | 7.00x (+37.58) |
| 50 | insert | 21.32x (+24.16) | 38.10x (+44.11) | 22.69x (+24.34) | 39.90x (+43.65) |
| 50 | delete | 2.93x (+3.64) | 28.01x (+51.00) | 2.56x (+2.92) | 28.06x (+50.71) |
| 50 | update | 30.77x (+24.98) | 106.83x (+88.80) | 30.35x (+24.98) | 109.13x (+92.02) |
| 50 | replace rowid | 7.44x (+27.88) | 11.01x (+43.35) † | 7.37x (+27.55) | 22.36x (+92.34) |
| 50 | replace unique | 7.50x (+28.28) | 10.84x (+42.84) † | 7.84x (+29.43) | 22.21x (+91.29) |
| 50 | replace two | 6.05x (+31.04) | 8.10x (+43.67) † | 5.61x (+29.00) | 23.23x (+139.72) |
| 200 | insert | 54.43x (+60.33) | 148.76x (+166.82) | 53.95x (+59.41) | 152.52x (+170.01) |
| 200 | delete | 1.93x (+2.48) | 75.55x (+198.16) | 2.59x (+3.08) | 102.25x (+195.93) |
| 200 | update | 67.88x (+60.13) | 397.10x (+356.09) | 65.08x (+59.85) | 388.25x (+361.69) |
| 200 | replace rowid | 16.19x (+64.59) | 40.80x (+169.22) † | 15.25x (+68.95) | 79.66x (+380.63) |
| 200 | replace unique | 14.56x (+64.29) | 38.93x (+179.80) † | 15.72x (+65.80) | 84.63x (+373.90) |
| 200 | replace two | 12.04x (+65.51) | 29.98x (+172.02) † | 11.50x (+64.62) | 88.97x (+541.18) |

**Where ivmlite's writes are cheaper than the trigger's.** Measured as extra
µs per row, `ivmlite` costs less than the hand-written trigger for every
operation in both modes from 50 views up; at 10 views, for delete and update in
both modes and for all three REPLACE forms with `recursive_triggers` ON (the
mode in which the trigger is correct); at one view, for no operation. At 200
views with the mode OFF, an insert costs 54.43x the unmaintained write under
`ivmlite` against 148.76x under the trigger, and a delete 1.93x against
75.55x. The hand-written trigger fires once per view for every written row, so
its cost grows with the number of views; `ivmlite` captures each written row
once for all views, so its cost grows only through the latch.

**What grows with the view count.** The latch's cost is not fixed: it grows
with the size of `sqlite_schema`, and every view adds rows to it. `ivmlite`'s
extra cost for insert, update and REPLACE grows from 11.52–17.43 µs/row at one
view to 14.38–20.40 at 10, 24.16–31.04 at 50 and 59.41–68.95 at 200. Deletes
stay at 2.24–3.64 µs/row, because the delete capture path does not run the
latch.

**REPLACE.** The UNIQUE-candidate lookup does not change the order of
magnitude: `ivmlite`'s three REPLACE forms cost 14.99–17.35 µs/row at one view
and 64.29–68.95 at 200, across both modes. `replace_two` has the
highest extra cost of the three at one and 10 views in both modes, and at 50 and
200 views with the mode OFF; with it ON, `replace_unique` (50 views) and
`replace_rowid` (200 views) are higher.

**The two modes.** For `ivmlite` the modes differ by at most 5.91 µs/row in any
cell (insert at one view) and by at most 4.36 µs/row from 10 views up; it is
correct in both. For the trigger the mode decides REPLACE's cost: ON makes it
fire its delete triggers too, and `replace_two` at 200 views rises from
+172.02 to +541.18 µs/row.

**Matrix write amplification.** The same two measures over the main matrix
(`m1b-phase4.csv`), where every write is a plain insert or delete through the
primary key, as medians over the cells with each view count and batch size,
with [min, max] for the extra µs per written row. With `batch=1` the single
written row carries the whole transaction's cost.

| views | batch | cells | ivmlite multiple | ivmlite +µs/row | trigger multiple | trigger +µs/row |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 3 | 25.00x | +51.00 [26.00, 96.00] | 6.00x | +15.00 [4.00, 20.00] |
| 1 | 10 | 3 | 12.46x | +9.50 [6.90, 14.90] | 4.20x | +3.20 [2.00, 3.80] |
| 1 | 100 | 3 | 13.14x | +5.95 [5.83, 6.96] | 4.53x | +1.76 [1.73, 2.18] |
| 1 | 1,000 | 3 | 11.03x | +5.78 [5.27, 5.80] | 3.79x | +1.63 [1.55, 1.67] |
| 10 | 1 | 8 | 19.08x | +62.00 [21.00, 138.00] | 5.62x | +16.50 [8.00, 25.00] |
| 10 | 10 | 8 | 14.81x | +12.45 [10.50, 18.80] | 21.96x | +17.65 [13.80, 37.50] |
| 10 | 100 | 8 | 13.50x | +8.17 [7.11, 8.83] | 26.39x | +12.25 [9.38, 28.61] |
| 10 | 1,000 | 8 | 14.72x | +7.61 [7.39, 7.90] | 28.60x | +11.97 [9.66, 28.93] |
| 50 | 1 | 3 | 35.50x | +138.00 [96.00, 507.00] | 13.00x | +36.00 [34.00, 83.00] |
| 50 | 10 | 3 | 28.33x | +24.60 [22.80, 27.20] | 73.67x | +65.40 [63.40, 118.00] |
| 50 | 100 | 3 | 24.56x | +14.88 [14.84, 15.98] | 80.76x | +50.25 [49.60, 59.56] |
| 50 | 1,000 | 3 | 35.41x | +15.14 [13.23, 15.35] | 107.80x | +46.99 [44.58, 51.27] |
| 200 | 1 | 3 | 66.67x | +197.00 [88.00, 275.00] | 28.33x | +78.00 [67.00, 82.00] |
| 200 | 10 | 3 | 74.83x | +54.80 [44.30, 62.60] | 346.00x | +248.70 [241.50, 329.10] |
| 200 | 100 | 3 | 88.34x | +38.43 [37.48, 39.50] | 451.00x | +206.84 [198.00, 209.85] |
| 200 | 1,000 | 3 | 97.84x | +37.77 [37.43, 38.61] | 444.01x | +177.86 [172.77, 181.97] |

`ivmlite`'s `apply_ms` was below the trigger's in 41 of the 68 cells: in none
of the 12 one-view cells, and in 23 of 32, 9 of 12 and 9 of 12 cells at 10, 50
and 200 views — every one of them with `batch` of 10 or more. The trigger wins
every `batch=1` cell and every one-view cell.

### Space amplification

Space is `(page_count - freelist_count) × page_size`, with 4,096-byte pages.
The representative matrix slice below reports every sample point against the
base-only database:

| views | base rows | batch | groups | base | bootstrapped | written | maintained |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | 100,000 | 100 | 10 | 1.27 MiB | 1.52 MiB (1.19x) | 1.52 MiB (1.19x) | 1.52 MiB (1.19x) |
| 10 | 100,000 | 100 | 1,000 | 1.45 MiB | 3.77 MiB (2.59x) | 3.77 MiB (2.59x) | 2.93 MiB (2.01x) |
| 10 | 100,000 | 100 | 100,000 | 1.64 MiB | 179.01 MiB (108.85x) | 179.01 MiB (108.85x) | 91.34 MiB (55.54x) |
| 200 | 100,000 | 1,000 | 1,000 | 1.45 MiB | 46.62 MiB (32.09x) | 46.65 MiB (32.10x) | 45.91 MiB (31.59x) |

Cardinality is the space risk: ten 100,000-group views retain more than 55x
the base table after refresh. More views scale state similarly.

**The `bootstrapped` and `written` samples are inflated.** Bootstrap writes the
whole initial state — every state row and every output row — through the
view's stage table, and the stage is emptied only at the start of the next
apply, so the bootstrapped database still holds a full copy of it. The first
refresh empties the stage and its pages go to the freelist. The drop to
`maintained` is that, not garbage collection of delta pages. At `views=10,
base_rows=100,000, batch=100, groups=100,000`, the bootstrapped sample uses
45,826 pages and the maintained one 23,384, with 22,442 pages on the freelist:
the bootstrapped figure is 1.96x the steady state. At `groups=1,000` it is
1.29x, and at `groups=10`, where the stage is tiny, 1.00x. The `maintained`
column is the footprint to plan for.

The output index costs space too: in the four ablation cells it added 6.6%,
10.1%, 11.2% and 10.2% to the bootstrapped page count (table order), from the
`before` build to the `index` build.

### Limits and artifacts

The exploration has one run per cell; only the 13 selected boundary cells have
five repeats. The full write-amplification run and each build's ablation
write-amplification run were each measured once. Data is uniform, databases are
in-memory, and refresh frequency is fixed to once per batch. These results do
not cover Zipf distributions, locality, disk durability, concurrent
connections, Turso, or Nexmark.

Follow-up for the next phase: **clear the stage table at the end of
bootstrap**, so a freshly created view does not hold a second copy of its
initial state until its first refresh. Phase 4 documents the effect and does
not change the extension.

Raw data:

- [`m1b-phase4.csv`](m1b-phase4.csv) — 68 cells × 4 engines.
- [`m1b-phase4-confirm.csv`](m1b-phase4-confirm.csv) — 13 selected cells × 5 repeats × 4 engines.
- [`m1b-phase4-ablation.csv`](m1b-phase4-ablation.csv) — three builds × four cells × five repeats × two engines.
- [`m1b-phase4-ablation-write-amp.csv`](m1b-phase4-ablation-write-amp.csv) — ivmlite at 10/200 views for all operations and both trigger modes, across three builds, one run each.
- [`m1b-phase4-ablation-builds.csv`](m1b-phase4-ablation-builds.csv) — the commit behind each ablation label.
- [`m1b-phase4-write-amp.csv`](m1b-phase4-write-amp.csv) — the complete write-amplification matrix, one run.
- [`phase4_tables.py`](phase4_tables.py) — derives every table and figure in this section from the CSVs above.

Charts:

- Total `apply + maintain` at 10 views and batch 100: [`groups=10`](m1b-phase4-card10.svg), [`groups=1,000`](m1b-phase4-card1000.svg), [`groups=100,000`](m1b-phase4-card100000.svg).
- Speedup surface with 1x/2x reference lines: [`groups=10`](m1b-phase4-speedup-card10.svg), [`groups=1,000`](m1b-phase4-speedup-card1000.svg), [`groups=100,000`](m1b-phase4-speedup-card100000.svg).

## M1b Phase 5: refresh performance

Phase 4 met §10.2's "must" bar but missed "expected": on apply + refresh time,
ivmlite stayed far slower than hand-written triggers, and almost all of its
time was refresh. Phase 5
([spec](../superpowers/specs/2026-10-08-m1b-phase5-refresh-performance-design.md))
made four changes to refresh, measured each one with Phase 4's protocol, and
re-ran the full matrix and confirmation:

1. `schemaver`: skip the per-refresh catalog checks while `PRAGMA
   schema_version` is unchanged (spec §3);
2. `setapply`: apply a refresh with set-based statements fired once, armed by
   an update of one sentinel row, instead of a trigger fired once per stage
   row; this is shadow-table format 4 (§4 and its amendment);
3. `stage`: empty the stage table at the end of bootstrap (§5);
4. `profiled`: stage rows with multi-row `INSERT … VALUES`, bounded by the
   connection's live variable limit (§6 and its second amendment).

> **Note (2026-10-11, M1b Phase 6):** like Phase 4's, this section's matrix,
> confirmation and ablation time the **first refresh after `CREATE`**; only the
> demos separate the steady state. Phase 6's tables time the steady state, so
> the two are not comparable column for column (see "M1b Phase 6" below).

Every measured number in this section comes from
[`phase5_tables.py`](phase5_tables.py) (`python3 docs/bench/phase5_tables.py`),
which reads the committed CSVs and uses only the standard library. The
exceptions are the setup facts below (dates, hardware, versions, wall times and
load averages, from the run session) and the §6 profile shares, which come
from the profile recorded in the spec's §6 amendment.

### Setup

All runs took place on the same 10-core Apple M2 Pro MacBook Pro with 16 GB
RAM as Phase 4, under macOS 26.2 (25C56), with Rust 1.95.0 and the SQLite
3.53.2 that `rusqlite` bundles. The extension, the runner and the demo
benchmarks were release builds. The first three runs took place in one session
on 2026-10-08, one after another, in this order. The demo run was repeated on
2026-10-10 with the ten-batch demo protocol (see the correction under "Demos
before and after"), and again later that day after a run's totals became the
median of per-batch sums; its row is that last run. Each wall time includes
its builds:

| run | wall time | load average at start (1/5/15 min) | at end |
|---|---:|---:|---:|
| `scripts/bench-ablation.sh`, five builds | 7,302.27 s | 4.88 / 6.05 / 5.61 | 1.68 / 1.72 / 1.76 |
| `scripts/bench.sh matrix` | 3,008.51 s | 2.01 / 1.80 / 1.79 | 1.33 / 1.37 / 1.54 |
| `scripts/bench.sh confirm` | 102.24 s | 1.40 / 1.39 / 1.54 | 2.35 / 1.75 / 1.67 |
| `scripts/bench-demos.sh`, two builds (2026-10-10) | 919.28 s | 5.50 / 5.73 / 7.84 | 2.99 / 4.14 / 5.91 |

**The machine was not fully idle.** Desktop applications stayed open
throughout, and the ablation started right after a test-suite run, which is
why its starting load average is high. Two drift controls are reported
below: the engines and modes that no build changes. They moved by at most a few
percent within the ablation, and by at most 8.6% within the demo run. Between
Phase 4's session and this one, they moved more (see "Phase 4 to Phase 5").

The protocol is Phase 4 spec §3, unchanged, except that each demo run times ten
steady-state batches (below). Every cell and demo mode verified its initial and
final state against SQLite's own evaluation of the view. None mismatched.

- **Ablation.** The five cells of `[ablation]` in
  `workloads/m0-baseline.toml`, five repeats each, over `naive_recompute` and
  `ivmlite`. These are Phase 4's four cells plus one Phase 5 adds:
  `views=200, base_rows=100,000, batch=1, groups=1,000`. In that cell a
  refresh's fixed per-view cost dominates, which isolates §3. Each build's
  ablation write-amplification workload ran once.
- **Matrix and confirmation.** The 68-cell exploration, one run per cell, then
  every cell the Phase 4 rule selects, five repeats each. The rule selects
  speedups in [0.7, 2.8] and every cell where `ivmlite` beats the trigger.
  It selected 8 cells (Phase 4: 13), all by speedup.
- **Demos.** `scripts/bench-demos.sh before=c0f2123 after=7288073` builds
  each commit's extension in its own worktree and runs this tree's
  `fluxflow_demo` and `demand_case_bench` against it with `--extension`, so
  only the extension differs. Each demo ran at its documented scale, with five
  repeats:
  - FluxFlow: 1,500,000 rows, batch 500;
  - noop: 518,400 rows, batch 200;
  - Zcash: 500,000 rows, batch 500;
  - Kener: 4,100,000 rows, batch 500.

  The demos' protocol is in [`docs/demos/README.md`](../demos/README.md).
  Unlike the matrix, it separates the first refresh after `CREATE`
  (`first_refresh_ms`) from the steady-state refresh (`maintain_ms`). Each
  run applies one warm-up batch and then ten measured batches, and reports
  each step as its median over the ten; this replaced one measured batch per
  run on 2026-10-09 (Phase 5 spec §7, amendment 2026-10-09). Its apply +
  maintain and end to end are the medians of each batch's own sum, since
  2026-10-10; before, they were sums of the per-step medians.

**One harness fact matters for reading the matrix and the ablation.** Each
matrix and ablation cell runs one batch in a fresh database, so its timed
`maintain` is the **first refresh after `CREATE`**. Before §5, that refresh
also emptied the stage that bootstrap had left full. Part of §5's effect on
those numbers is therefore a one-time cost that has moved out of the timed
region, not a steady-state saving. The demos show both costs separately.

### Reproducing

From the repository root, on the commit that holds this README:

```bash
OUT=docs/bench/m1b-phase5-ablation.csv WRITE_AMP_OUT=docs/bench/m1b-phase5-ablation-write-amp.csv \
  scripts/bench-ablation.sh before=c0f2123 schemaver=f511c65 setapply=1642e46 stage=fb5e39f profiled=7288073
scripts/bench.sh matrix > docs/bench/m1b-phase5.csv
scripts/bench.sh confirm --from docs/bench/m1b-phase5.csv > docs/bench/m1b-phase5-confirm.csv
scripts/bench-demos.sh before=c0f2123 after=7288073
cargo run --release -p ivmlite-bench --locked -- plot --from docs/bench/m1b-phase5.csv
python3 docs/bench/phase5_tables.py
```

The labels map to commits in
[`m1b-phase5-ablation-builds.csv`](m1b-phase5-ablation-builds.csv), which is
written by hand, as in Phase 4, and in
[`m1b-phase5-demos-builds.csv`](m1b-phase5-demos-builds.csv), which
`scripts/bench-demos.sh` writes. The extension at the branch head is the
`profiled` build's.

### Ablation: which changes matter

Each value is `ivmlite`'s `apply + maintain` in milliseconds, as the median
[min, max] over five runs.

| build | views | batch | groups | apply + maintain ms |
|---|---:|---:|---:|---:|
| before | 10 | 1,000 | 1,000 | 86.956 [86.403, 96.004] |
| schemaver | 10 | 1,000 | 1,000 | 86.040 [85.700, 86.510] |
| setapply | 10 | 1,000 | 1,000 | 66.425 [66.290, 66.659] |
| stage | 10 | 1,000 | 1,000 | 66.628 [66.377, 69.756] |
| profiled | 10 | 1,000 | 1,000 | 64.497 [64.256, 65.294] |
| before | 10 | 1,000 | 100,000 | 127.872 [127.794, 133.718] |
| schemaver | 10 | 1,000 | 100,000 | 127.665 [127.227, 129.487] |
| setapply | 10 | 1,000 | 100,000 | 105.809 [105.412, 107.216] |
| stage | 10 | 1,000 | 100,000 | 102.028 [97.812, 102.220] |
| profiled | 10 | 1,000 | 100,000 | 94.654 [94.415, 95.071] |
| before | 200 | 1 | 1,000 | 320.223 [319.171, 321.104] |
| schemaver | 200 | 1 | 1,000 | 206.199 [204.675, 207.626] |
| setapply | 200 | 1 | 1,000 | 211.552 [211.418, 213.200] |
| stage | 200 | 1 | 1,000 | 209.323 [208.620, 210.033] |
| profiled | 200 | 1 | 1,000 | 209.304 [208.396, 209.710] |
| before | 200 | 100 | 100,000 | 1,353.479 [1,331.238, 1,382.964] |
| schemaver | 200 | 100 | 100,000 | 1,210.262 [1,194.556, 1,236.139] |
| setapply | 200 | 100 | 100,000 | 1,182.072 [889.219, 1,234.362] |
| stage | 200 | 100 | 100,000 | 384.295 [376.153, 407.662] |
| profiled | 200 | 100 | 100,000 | 384.961 [377.646, 407.870] |
| before | 200 | 1,000 | 1,000 | 1,654.704 [1,643.907, 1,906.341] |
| schemaver | 200 | 1,000 | 1,000 | 1,541.158 [1,536.175, 1,593.112] |
| setapply | 200 | 1,000 | 1,000 | 1,237.414 [1,233.698, 1,250.234] |
| stage | 200 | 1,000 | 1,000 | 1,241.577 [1,235.665, 1,257.788] |
| profiled | 200 | 1,000 | 1,000 | 1,144.794 [1,142.448, 1,155.281] |

All cells have 100,000 base rows. The next table gives each change against the
build before it, as the ratio of medians (above 1 means faster). A change is
**indistinguishable** where the two builds' min–max ranges overlap.

| views | batch | groups | §3 schemaver | §4 setapply | §5 stage | §6 profiled | whole phase |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | 1,000 | 1,000 | indistinguishable (1.011x) | 1.295x | indistinguishable (0.997x) | 1.033x | 1.35x |
| 10 | 1,000 | 100,000 | indistinguishable (1.002x) | 1.207x | 1.037x | 1.078x | 1.35x |
| 200 | 1 | 1,000 | 1.553x | **0.975x (slower, at the noise floor)** | 1.011x | indistinguishable (1.000x) | 1.53x |
| 200 | 100 | 100,000 | 1.118x | indistinguishable (1.024x) | 3.076x | indistinguishable (0.998x) | 3.52x |
| 200 | 1,000 | 1,000 | 1.074x | 1.245x | indistinguishable (0.997x) | 1.085x | 1.45x |

The whole phase, `before` to `profiled`, is faster in every cell, with the
two builds' ranges apart. It left `apply`, the timed writes, within noise:
`before` over `profiled` is 0.926–1.044x. Write cost is covered under "What
got worse" below.

- **§3 matters only with many views.** It is indistinguishable at 10 views.
  At 200 views it is the decisive change for tiny batches: in the batch=1
  cell, refresh dropped from 320.223 to 206.199 ms. Per view, the whole phase
  took a refresh's fixed cost there from 1.600 to 1.045 ms. With larger
  batches, §3's share shrinks (1.118x and 1.074x).
- **§4 matters with large batches, and costs a little with tiny ones.** It is
  the largest change in the three batch=1,000 cells: 1.295x, 1.207x and
  1.245x. In the batch=1 cell at 200 views it is **slower**: 0.975x, from
  206.199 to 211.552 ms, with the ranges apart. When the stage holds only a
  few rows, the set-based body's fixed statements cost more than one trigger
  firing per row. Those are the sentinel insert, the arming update and the
  body's seven statements per view for these views, each of which scans the
  stage. §4 also
  made bootstrap faster at 100,000 groups (1.434x and 1.410x), because
  bootstrap applies its whole initial state through the same body.
- **§5 matters only where bootstrap leaves a large stage.** At 200 views and
  100,000 groups it is 3.076x, from 1,182.072 to 384.295 ms. As explained
  above, that is mostly the leftover stage leaving the first refresh, not a
  faster steady state. Bootstrap changed by at most 1.7% (setapply to stage:
  1.001x and 0.983x at 100,000 groups), within the drift control. §5's effect
  on steady-state refresh is indistinguishable in two cells, and 1.037x and
  1.011x in two others.
- **§6 gives the few percent it was kept for.** It is 1.033x, 1.078x and
  1.085x in the batch=1,000 cells, the cells with the most stage rows, and
  indistinguishable in the other two. The §6 amendment's own measurement was
  6.5%.

The drift control: `naive_recompute`, which no build changes, kept medians
within 1.017–1.040x (max/min across builds) in each cell. So a change under
about 1.04x is at the noise floor even where its ranges are apart: §3 at
200 views and batch=1,000 (1.074x) clears it, §4's 0.975x, §5's 1.037x and
1.011x, and §6's 1.033x do not.

**Space.** §5 halves the bootstrapped database where the stage was large. At
200 views and 100,000 groups, bootstrapped used pages went from 679,513
(`setapply`) to 347,433 (`stage`). The footprint after refresh did not change
there (348,046 pages in both builds).

### The §10.2 bars against hand-written triggers

- **Must: much faster than full recomputation.** Met over a broader region than
  in Phase 4.
  - 57 of 68 exploration cells were above 2x (Phase 4: 55), and 5 were below
    1x (Phase 4: 7).
  - All 24 cells at 100,000 rows were above 2x, from 2.338x to 144.086x.
  - All 24 at one million rows were above 2x, from 23.088x to 1,203.056x.
  - The one 100,000-row cell near 2x was confirmed above it in every repeat:
    `views=1, batch=1000, groups=1000` had 2.260x [2.160, 2.425]. In Phase 4
    it straddled the bar.
  - The losing region is still the small-table corner. At 10,000 rows, four
    batch=1,000 cells (`groups=1000`) were single-run losses at 0.217x (one
    view), 0.323x (10), 0.338x (50) and 0.298x (200). One confirmed cell stayed
    below 1x in every repeat: `views=200, batch=100, groups=1000` had 0.921x
    [0.911, 0.931]. One straddles 1x: `views=10, batch=1000, groups=10` had
    1.013x [0.997, 1.028].
  - The other confirmed 10,000-row cells were above 1x in every repeat and
    below 2x: 1.344x, 1.432x, 1.478x, 1.574x and 1.705x (medians).
- **Expected: close to hand-written triggers.** **Still not met.** Phase 5
  narrowed the gap in every cell confirmed in both phases, but did not close
  it. Below, `ivmlite`'s `apply + maintain` is given as a multiple of
  the trigger's, as the median [min, max] over five repeats, in every
  confirmed cell:

  | views | base rows | batch | groups | ivmlite / trigger |
  |---:|---:|---:|---:|---:|
  | 10 | 10,000 | 1,000 | 10 | 1.81x [1.80, 1.85] |
  | 1 | 100,000 | 1,000 | 1,000 | 6.64x [6.14, 6.78] |
  | 1 | 10,000 | 100 | 1,000 | 8.93x [7.55, 11.30] |
  | 10 | 10,000 | 100 | 1,000 | 9.57x [8.53, 10.49] |
  | 50 | 10,000 | 100 | 1,000 | 11.79x [11.59, 12.13] |
  | 200 | 10,000 | 100 | 1,000 | 17.55x [17.44, 18.06] |
  | 200 | 10,000 | 10 | 1,000 | 76.21x [69.60, 79.75] |
  | 200 | 10,000 | 1 | 1,000 | 3,091.20x [2,552.87, 3,615.23] |

  The median over the confirmed cells is 10.68x. Only one cell is within 2x:
  10 groups with large batches, where consolidation leaves few output
  changes. The exploration matrix, one run per cell, shows the same shape by
  batch size:

  | batch | cells | median | min | max |
  |---:|---:|---:|---:|---:|
  | 1 | 17 | 190.08x | 23.38x | 3,217.83x |
  | 10 | 17 | 26.12x | 13.00x | 97.20x |
  | 100 | 17 | 9.81x | 4.68x | 17.51x |
  | 1,000 | 17 | 5.88x | 1.79x | 6.80x |

  Seven cells were confirmed in both phases. In every one, the ratio fell:
  - 1.92x to 1.81x;
  - 7.90x to 6.64x;
  - 10.86x to 8.93x, although their ranges overlap;
  - 12.62x to 9.57x;
  - 15.35x to 11.79x;
  - 106.48x to 76.21x;
  - 4,529.00x to 3,091.20x.
- **Bonus: beat hand-written triggers.** Not observed. No exploration cell and
  no confirmation repeat had a lower `apply + maintain` than the trigger.
  `ivmlite`'s writes alone (`apply_ms`) were cheaper than the trigger's in 40
  of 68 cells (Phase 4: 41). The gap is still entirely refresh. Its median
  share of `ivmlite`'s `apply + maintain` was 98.5% at batch=1, 97.0% at
  batch=10, 93.4% at batch=100 and 88.5% at batch=1,000.

### What still loses, and why

The hand-written trigger does one in-place `UPDATE` of one summary row per
written row per view. It has no deltas, no stage and no second table. A refresh
of an `ivmlite` view does much more:
- it reads the captured deltas;
- it runs them through the general operator tree, with arrangement lookups and
  serialized values;
- it writes every state, output and watermark change into the stage;
- it applies the stage with a set-based trigger body. That body upserts the
  state table, deletes and inserts output rows (each also maintaining the
  output index), advances the watermarks and garbage-collects the deltas.

The §6 profile, on 10 views over 100,000 rows with 1,000 groups and refresh
after every 1,000-row insert (the spec's §6 amendment), shows where that time
goes after §3–§5:
- **the trigger body, about 40% of refresh**, nearly all of it B-tree seeks
  and inserts in the state and output tables. This is the maintenance itself,
  and it is already set-based;
- **`compute`, the operator tree, about 27%**: `AggState::absorb` 19% and
  arrangement reads 8%;
- **staging, 25%**, which §6 cut to about 20%.

None of the three is overhead that a further gate can skip. Closing the
remaining 1.81–17.55x of the confirmed cells with batches of 100 or more
would need fewer B-tree writes per
changed group, cheaper operator evaluation, or both, rather than less
machinery around them.

Tiny batches with many views lose for a different reason: a fixed cost per view
per refresh. At 200 views and batch=1 the whole phase took that cost from
1.600 to 1.045 ms per view, but the trigger does not pay it at all. So
`ivmlite` stays thousands of times slower there, and refreshing every view after
every one-row write is the worst case for its explicit-refresh design.

The small-table corner loses to recomputation because a 10,000-row recompute is
cheap, and a 1,000-row batch over 1,000 groups changes most groups anyway.

### Demos before and after

Each value is the median over five repeats. Steady-state refresh is
`maintain_ms`: in each repeat, the median refresh over the ten measured
batches. End to end is apply + refresh + reading the view: in each repeat,
the median over the same batches of each batch's own sum. The two right-hand
columns are `indexed_recompute`'s end to end divided by `ivmlite`'s, before
and after.

| demo | refresh before → after | end to end before → after | first refresh before → after | vs indexed recompute before | after |
|---|---:|---:|---:|---:|---:|
| FluxFlow | 2.789 → 2.340 ms (1.19x) | 9.428 → 9.061 ms (1.04x) | 2.960 → 2.211 ms (1.34x) | 14.14x | 15.08x |
| noop | 1.008 → 0.688 ms (1.47x) | 3.075 → 2.764 ms (1.11x) | 1.119 → 0.771 ms (1.45x) | 6.01x | 6.82x |
| Zcash | 4.190 → 2.915 ms (1.44x) | 8.933 → 7.654 ms (1.17x) | 4.262 → 3.069 ms (1.39x) | 2.70x | 3.20x |
| Kener | 2.872 → 2.354 ms (1.22x) | 147.761 → 149.362 ms (0.99x) | 31.845 → 4.921 ms (6.47x) | 2.31x | 2.33x |

- In all four demos, steady-state refresh got 1.19–1.47x faster, and the
  before and after ranges do not overlap. End to end, the change is smaller
  (0.99–1.17x), because the writes and the read did not change.
- FluxFlow is the one demo with a hand-written trigger baseline. There,
  `ivmlite` stays 5.03x slower end to end (before: 5.30x).
- **Kener's steady-state refresh got faster**: from 2.872 [2.650, 3.052] to
  2.354 [2.216, 2.453] ms. Its end to end did not move (0.99x, with the ranges
  overlapping, within the drift control): at 550,000 groups, reading the view
  is almost all of it. Its first refresh after `CREATE` got 6.47x faster, as
  §5 intends, but that ratio is not stable: the before build's first refresh
  ranged from 18.788 to 40.792 ms in this run, and was 10.756 ms in the
  previous one. Its bootstrap got 1.27x faster (10,436.780 to 8,216.870 ms),
  in line with §4's faster bootstrap at high cardinality in the ablation.
- The drift control: the modes no build changes stayed within 0.953–1.086x
  end to end. The two extremes are both `no_maintenance`, a 0.2 ms batch
  (FluxFlow 0.953x, noop 1.086x); the recomputations moved by 0.991–1.025x.
  So an end-to-end change under about 1.09x, such as FluxFlow's and Kener's,
  is not a finding.
- **The run behind this table is the second of two that day.** The first
  run of the same command, with the same harness commit, had a drift control
  of 0.810–1.010x: its before build ran while the machine was busier, so
  Kener's `indexed_recompute`, which no build changes, was 1.23x slower
  before than after. It was replaced because its drift control was more than
  twice as wide; the choice rests on that control, not on its `ivmlite`
  results, which agree with this table's: steady-state refresh 1.22–1.51x
  faster in all four demos, with the ranges apart.

**Correction, 2026-10-10.** The first publication of this section, on
2026-10-08 with `v0.1.0-alpha.3`, reported Kener's steady-state refresh as
**slower** after Phase 5: 0.80x (2.547 → 3.179 ms), and 0.90x in a re-run with
the builds in reverse order. That finding was wrong. Each demo run then timed
one steady-state refresh, so every median rested on five single refreshes,
and a single Kener refresh is too noisy to resolve a change of that size (the
old run's own range reached 8.828 ms). The demo protocol now times ten
(Phase 5 spec §7, amendment 2026-10-09), and the table above is the re-run
with it. The single-refresh re-run's CSV, `m1b-phase5-demos-kener-recheck.csv`,
was removed. Later on 2026-10-10 a run's apply + maintain and end to end
became the medians of each batch's own sum instead of sums of per-step
medians, and the table was re-run again. The version it replaced, from the
same day, reported Kener's refresh 2.772 → 2.235 ms (1.24x) and steady-state
refresh 1.14–1.45x faster in all four demos; the conclusions are unchanged.

### Phase 4 to Phase 5

The two matrices were measured in different sessions, one run per cell each,
so the per-cell change carries the sessions' drift. `ivmlite`'s
`apply + maintain` fell in every one of the 68 cells, by a median of 1.43x
(range 1.10x to 8.93x). The largest falls were at 100,000 groups:
- 8.93x at `views=10, base_rows=1,000,000, batch=1`;
- 5.60x at batch=10;
- 4.65x at `base_rows=100,000, batch=1`.

These are cells where Phase 4's first refresh emptied a large bootstrap stage.

The drift control is the engines this phase did not change, Phase 4 over
Phase 5:
- `naive_recompute` 1.046x [0.973, 1.151];
- `hand_written_trigger` 1.148x [0.824, 2.559];
- `no_maintenance` 1.213x [0.667, 2.571].

The last two time sub-millisecond batches in many cells, so their ratios are
noisy. This session ran about 5% faster than Phase 4's for the same work, so a
per-cell change under about 1.15x is not a finding. The ablation, which ran
every build in one session, is the formal before and after.

The confirmed `ivmlite / trigger` figures above come from five repeats in each
phase, and are less exposed to that drift.

### What got worse

- **One ablation cell, from §4.** In the batch=1 cell at 200 views, `setapply`
  was 0.975x of `schemaver` (206.199 to 211.552 ms), with the ranges apart but
  within the ablation's noise floor of about 1.04x. The whole phase is still
  1.53x faster there, because §3 came first.
- **Space after refresh, at 1,000 groups with large batches.** The maintained
  database's used pages grew from `schemaver` to `setapply`: 1,019 to 1,084
  pages in the 10-view cell, and 11,752 to 12,849 in the 200-view cell. They
  barely moved at 100,000 groups. The cause has not been investigated.
- **Writes did not get worse.** Writes were not a target, and the ablation's
  write-amplification run shows them unchanged within the run's noise:
  - `profiled` over `before` per written row was 0.942–1.072x at 10 views and
    0.962–1.007x at 200 views;
  - each build ran once.

  At 200 views the `profiled` build still takes 59.299–70.772 µs per
  inserted, updated or REPLACEd row, against 4.986–5.053 µs per deleted row,
  because the latch scans `sqlite_schema` (Phase 5 spec §1 leaves the write
  path out of scope).

### Limits

- The exploration has one run per cell; only the 8 selected cells have five
  repeats. The ablation's write-amplification runs were measured once per
  build.
- The selected cells are all at 10,000 base rows except one, because the
  confirmation rule selects by recompute speedup. The 100,000-row cells near
  the old 2x bar now lie above 2.8x and are single runs.
- The machine was not fully idle (see the setup above). The cross-session
  comparison with Phase 4 carries about 5–15% drift.
- In the matrix and the ablation, `maintain` is the first refresh after
  `CREATE`, so §5's effect there is partly a one-time cost.
- Data is uniform, databases are in-memory, and refresh runs once per batch.
  Zipf distributions, locality, durability, concurrent connections, Turso and
  Nexmark are still not covered.
- The demos are synthetic fixtures shaped after public reports
  ([`docs/demos/`](../demos/README.md)), not production workloads.

Raw data:

- [`m1b-phase5.csv`](m1b-phase5.csv): 68 cells × 4 engines.
- [`m1b-phase5-confirm.csv`](m1b-phase5-confirm.csv): 8 selected cells × 5 repeats × 4 engines.
- [`m1b-phase5-ablation.csv`](m1b-phase5-ablation.csv): five builds × five cells × five repeats × two engines.
- [`m1b-phase5-ablation-write-amp.csv`](m1b-phase5-ablation-write-amp.csv): ivmlite at 10/200 views for all operations and both trigger modes, across five builds, one run each.
- [`m1b-phase5-ablation-builds.csv`](m1b-phase5-ablation-builds.csv): the commit behind each ablation label.
- [`m1b-phase5-demos.csv`](m1b-phase5-demos.csv) and [`m1b-phase5-demos-builds.csv`](m1b-phase5-demos-builds.csv): the four demos against two builds, as median, min and max per metric over five repeats, each repeat's apply, maintain and read a median over ten measured batches; and, per label, the extension commit, the harness commit (`8402b48`, with no uncommitted changes under `crates/` or the Cargo files) and the SQLite version. Re-run on 2026-10-10; see the correction under "Demos before and after".
- [`phase5_tables.py`](phase5_tables.py): derives every table and figure in this section from the CSVs above.

Charts:

- Total `apply + maintain` at 10 views and batch 100: [`groups=10`](m1b-phase5-card10.svg), [`groups=1,000`](m1b-phase5-card1000.svg), [`groups=100,000`](m1b-phase5-card100000.svg).
- Speedup surface with 1x/2x reference lines: [`groups=10`](m1b-phase5-speedup-card10.svg), [`groups=1,000`](m1b-phase5-speedup-card1000.svg), [`groups=100,000`](m1b-phase5-speedup-card100000.svg).

---

## M1b Phase 6: the fixed cost of a refresh

After Phase 5, most of a refresh's time at small batches was two fixed costs:
compiling its statements again on every refresh, and re-reading the catalog
in every view after any schema change. Phase 6
([spec](../superpowers/specs/2026-10-11-m1b-phase6-fixed-cost-design.md))
removed both, fixed one O(views) scan, and changed the benchmark to measure
the steady state:

1. `stmtcache`: each view keeps its own connection wrapper and prepares every
   fixed statement of its refresh at most once, staging rows in a few fixed
   shapes (spec §3);
2. `snapshot`: a connection reads `sqlite_schema` once per schema change and
   shares that snapshot, and each base table's capture verdict, among its
   views (§4);
3. `progidx`: an index on `__ivm_progress(tbl, applied_seq)`, so the garbage
   collection inside a view's apply no longer scans every view's watermark;
   this is shadow-table format 5 (§5);
4. the steady-state benchmark: every matrix, confirmation and ablation cell
   applies one warm-up batch, reported as `first_refresh_ms`, and then five
   measured batches, reported as medians (§6).

Every measured number in this section comes from
[`phase6_tables.py`](phase6_tables.py) (`python3 docs/bench/phase6_tables.py`),
which reads the committed CSVs and uses only the standard library. The
exceptions are the setup facts below (dates, hardware, versions, wall times and
load averages, from the run session) and the memory figures under "What got
worse", which come from a throwaway probe that is not committed.

### Setup

All runs took place on the same 10-core Apple M2 Pro MacBook Pro with 16 GB
RAM as Phases 4 and 5, under macOS 26.2 (25C56), with Rust 1.95.0 and the
SQLite 3.53.2 that `rusqlite` bundles. The extension, the runner and the demo
benchmarks were release builds. Every run took place on 2026-10-11, one after
another, in this order, each under `caffeinate -i`. Each wall time includes its
builds:

| run | wall time | load average at start (1/5/15 min) | at end |
|---|---:|---:|---:|
| `scripts/bench-ablation.sh`, four builds | 8,340.14 s | 5.72 / 7.25 / 6.80 | 3.43 / 4.05 / 4.37 |
| the same, the supplementary 10-view cell (below) | 210.16 s | 3.44 / 3.52 / 3.42 | 6.71 / 5.54 / 4.34 |
| `scripts/bench.sh matrix` | 5,559.39 s | 4.19 / 3.92 / 3.92 | 1.63 / 1.76 / 2.48 |
| `scripts/bench.sh confirm` | 70.71 s | 1.63 / 1.76 / 2.48 | 1.98 / 1.87 / 2.46 |
| `scripts/bench-demos.sh`, two builds | 874.93 s | 1.98 / 1.87 / 2.46 | 10.20 / 5.00 / 3.52 |

**The machine was not idle.** Desktop applications stayed open throughout.
The drift controls below are the engines and modes no build changes: within
the ablation they moved by up to 1.082x, within the demo run by at most 2.3%.

Every cell verified its views against SQLite's own evaluation after the
warm-up batch and after the last measured batch, and the trigger and `ivmlite`
also after bootstrap, as before; every demo mode verified as in Phase 5. None
mismatched.

- **Ablation.** The five cells of `[ablation]` in
  `workloads/m0-baseline.toml`, unchanged from Phase 5, five repeats each, over
  `naive_recompute` and `ivmlite`. Spec §7 holds §3 to its bar at 10 views and
  batch 1 too, which none of those cells is. So the same script ran once more
  over the same four builds with
  [`workloads/m1b-phase6-ablation-views10-batch1.toml`](../../workloads/m1b-phase6-ablation-views10-batch1.toml),
  a copy of the workload whose only ablation cell is
  `views=10, base_rows=100,000, batch=1, groups=1,000`; its rows are in
  [`m1b-phase6-ablation-views10-batch1.csv`](m1b-phase6-ablation-views10-batch1.csv).
  The ablation's write-amplification output was discarded: no change touched
  the write path's code (spec §6). The timed writes inside the ablation cells
  did move in one cell; see the acceptance paragraph below.
- **Matrix and confirmation.** The 68-cell exploration, one run per cell, then
  every cell the Phase 4 rule selects, five repeats each. The rule now reads
  the steady-state `apply_plus_maintain_ms`. It selected 6 cells
  (Phase 5: 8), all by speedup.
- **Demos.** `scripts/bench-demos.sh before=ebd716b after=a92a570`, each demo
  at its documented scale with five repeats, as in Phase 5. The demos already
  measured a warm-up and then ten steady-state batches; their protocol did not
  change.

**The protocol change.** Phase 4 and Phase 5 ran one batch per cell in a fresh
database, so their `maintain` was the **first refresh after `CREATE`**.
Creating a view is a schema change, so that refresh paid for re-reading the
catalog and compiling every statement, costs a later refresh does not pay.
Phase 6's `apply_ms`, `maintain_ms` and `apply_plus_maintain_ms` are medians
over five later batches, and `first_refresh_ms` is the warm-up's refresh. The
Phase 4 and Phase 5 tables are therefore **not comparable column for column**
with these. "Phase 5 to Phase 6" below compares them anyway, with this caveat
attached.

### Reproducing

From the repository root, on the commit that holds this README:

```bash
OUT=docs/bench/m1b-phase6-ablation.csv WRITE_AMP_OUT=/tmp/m1b-phase6-ablation-write-amp.csv \
  scripts/bench-ablation.sh before=ebd716b stmtcache=5bd851f snapshot=a0cf490 progidx=a92a570
# The supplementary cell (10 views, batch 1), with a minimal write-amp workload
# whose output is discarded.
WORKLOAD=workloads/m1b-phase6-ablation-views10-batch1.toml \
  WRITE_WORKLOAD=workloads/m1b-phase6-write-amp-minimal.toml \
  OUT=docs/bench/m1b-phase6-ablation-views10-batch1.csv WRITE_AMP_OUT=/tmp/discard.csv \
  scripts/bench-ablation.sh before=ebd716b stmtcache=5bd851f snapshot=a0cf490 progidx=a92a570
scripts/bench.sh matrix > docs/bench/m1b-phase6.csv
scripts/bench.sh confirm --from docs/bench/m1b-phase6.csv > docs/bench/m1b-phase6-confirm.csv
OUT=docs/bench/m1b-phase6-demos.csv scripts/bench-demos.sh before=ebd716b after=a92a570
cargo run --release -p ivmlite-bench --locked -- plot --from docs/bench/m1b-phase6.csv
python3 docs/bench/phase6_tables.py
```

The labels map to commits in
[`m1b-phase6-ablation-builds.csv`](m1b-phase6-ablation-builds.csv), written by
hand as in Phases 4 and 5, and in
[`m1b-phase6-demos-builds.csv`](m1b-phase6-demos-builds.csv), which
`scripts/bench-demos.sh` writes. `before` is master at Phase 6 start. The
extension at the branch head is the `progidx` build's.

### Ablation: which changes matter

Each value is `ivmlite`'s steady-state `maintain_ms` (a refresh of every view)
and its `first_refresh_ms` (the warm-up's refresh, the first after `CREATE`),
in milliseconds, as the median [min, max] over five runs. All cells have
100,000 base rows.

| build | views | batch | groups | steady refresh ms | first refresh ms |
|---|---:|---:|---:|---:|---:|
| before | 10 | 1 | 1,000 | 1.117 [1.102, 1.142] | 2.551 [2.285, 2.584] |
| stmtcache | 10 | 1 | 1,000 | 0.342 [0.334, 0.393] | 2.546 [2.270, 2.781] |
| snapshot | 10 | 1 | 1,000 | 0.337 [0.327, 0.361] | 1.289 [1.239, 1.546] |
| progidx | 10 | 1 | 1,000 | 0.366 [0.343, 0.396] | 1.350 [1.288, 1.647] |
| before | 10 | 1,000 | 1,000 | 56.165 [55.716, 57.896] | 57.399 [57.057, 58.704] |
| stmtcache | 10 | 1,000 | 1,000 | 57.559 [56.672, 58.350] | 61.247 [60.380, 79.762] |
| snapshot | 10 | 1,000 | 1,000 | 53.613 [53.294, 55.420] | 56.184 [55.638, 56.774] |
| progidx | 10 | 1,000 | 1,000 | 58.453 [57.380, 58.941] | 60.529 [59.518, 61.110] |
| before | 10 | 1,000 | 100,000 | 90.426 [89.400, 94.716] | 93.804 [91.214, 96.416] |
| stmtcache | 10 | 1,000 | 100,000 | 95.731 [95.287, 98.571] | 101.398 [98.702, 128.962] |
| snapshot | 10 | 1,000 | 100,000 | 94.581 [93.717, 95.641] | 94.921 [93.396, 96.662] |
| progidx | 10 | 1,000 | 100,000 | 98.144 [97.608, 98.410] | 101.331 [98.686, 103.598] |
| before | 200 | 1 | 1,000 | 23.547 [23.127, 23.765] | 220.637 [217.814, 225.394] |
| stmtcache | 200 | 1 | 1,000 | 7.490 [7.359, 7.675] | 214.322 [210.076, 218.571] |
| snapshot | 200 | 1 | 1,000 | 7.154 [7.087, 7.391] | 21.694 [21.541, 22.286] |
| progidx | 200 | 1 | 1,000 | 6.730 [6.334, 7.030] | 22.919 [22.434, 24.056] |
| before | 200 | 100 | 100,000 | 258.253 [238.737, 284.323] | 503.700 [489.108, 539.677] |
| stmtcache | 200 | 100 | 100,000 | 249.445 [219.323, 267.851] | 528.519 [417.508, 580.753] |
| snapshot | 200 | 100 | 100,000 | 227.959 [208.976, 246.449] | 292.987 [283.682, 296.744] |
| progidx | 200 | 100 | 100,000 | 241.211 [218.087, 307.353] | 347.973 [284.325, 385.171] |
| before | 200 | 1,000 | 1,000 | 985.437 [970.153, 992.974] | 1,240.271 [1,185.356, 1,257.037] |
| stmtcache | 200 | 1,000 | 1,000 | 1,008.500 [970.419, 1,055.862] | 1,235.284 [1,202.097, 1,267.599] |
| snapshot | 200 | 1,000 | 1,000 | 890.099 [887.486, 947.147] | 922.615 [912.556, 993.486] |
| progidx | 200 | 1,000 | 1,000 | 979.805 [962.824, 987.609] | 994.646 [987.727, 1,044.280] |

Each change against the build before it, as the ratio of medians (above 1
means faster). A change is **indistinguishable** where the two builds' min–max
ranges overlap.

| views | batch | groups | §3 stmtcache, steady | §4 snapshot, first refresh | §4, steady | §5 progidx, steady | whole phase, steady | whole phase, first refresh |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | 1 | 1,000 | **3.266x** | **1.975x** | indistinguishable (1.015x) | indistinguishable (0.921x) | 3.05x | 1.89x |
| 10 | 1,000 | 1,000 | indistinguishable (0.976x) | 1.090x | 1.074x | 0.917x (slower) | 0.96x, ranges overlap | 0.95x |
| 10 | 1,000 | 100,000 | 0.945x (slower) | 1.068x | indistinguishable (1.012x) | 0.964x (slower) | 0.92x | 0.93x |
| 200 | 1 | 1,000 | **3.144x** | **9.879x** | indistinguishable (1.047x) | **1.063x** | 3.50x | 9.63x |
| 200 | 100 | 100,000 | indistinguishable (1.035x) | 1.804x | indistinguishable (1.094x) | indistinguishable (0.945x) | 1.07x, ranges overlap | 1.45x |
| 200 | 1,000 | 1,000 | indistinguishable (0.977x) | 1.339x | 1.133x | 0.908x (slower) | 1.01x, ranges overlap | 1.25x |

**The noise floor.** `naive_recompute`, which no build changes, kept its
steady-state medians within 1.012–1.082x (max/min across builds) in each
cell, and its first refresh within 1.015–1.077x. Its steady medians were
lowest under `snapshot` in four of six cells and highest under `progidx` in
three, among them 200 views and batch 1 (3,423.333 against 3,704.452 ms). So a ratio within about
1.08x is not a finding, even where the ranges are apart. The session's drift
probably flatters the `snapshot` column above and penalizes the `progidx`
column.

- **§3 removed most of a steady refresh at batch 1.** At 200 views a refresh
  of every view went from 23.547 to 7.490 ms, from 117.7 to 37.5 µs per view;
  at 10 views from 1.117 to 0.342 ms. It did nothing for the first refresh, as
  expected: every statement is still compiled once. With batches of 100 or
  1,000 it is within the noise floor (0.945–1.035x).
- **§4 removed most of the first refresh after `CREATE` with many views.** At
  200 views and batch 1, the first refresh went from 214.322 to 21.694 ms, from
  1.07 ms to 0.11 ms per view; at 10 views, 1.975x. It is 1.068–1.804x in the
  other cells. Its steady-state effect is 1.012–1.133x; only 1.133x, at
  200 views and batch 1,000, clears both the noise floor and the ranges, and the drift control
  favoured this build.
- **§5 met its bar narrowly, and its other cells read slower.** In its target
  cell, 200 views and batch 1, steady refresh went from 7.154 to 6.730 ms, 5.9%
  saved, with the ranges apart. Elsewhere it reads 0.908–0.964x, inside the
  drift control's 1.08x except perhaps at 10 views and batch 1,000 with 1,000
  groups (0.917x, where the control moved 1.048x). The index adds one entry to
  maintain per watermark update; whether that or the session's drift explains
  the rest was not investigated.
- **The whole phase, `before` to `progidx`,** made steady refresh 3.05x and
  3.50x faster at batch 1 and the first refresh 1.25–9.63x faster at 200 views.
  With batches of 1,000 at 10 views it is 0.96x and 0.92x, the second with the
  ranges apart; the drift control moved 1.030x and 1.037x in those cells. Timed
  writes (`apply`) stayed within 0.936–1.018x.

**Acceptance (spec §7).** Every change met its bar:

| change | cell (100,000 rows, 1,000 groups) | metric | before it | after it | saved |
|---|---|---|---:|---:|---:|
| §3 statement cache | 10 views, batch 1 | steady refresh | 1.117 ms | 0.342 ms | 69.4% |
| §3 statement cache | 200 views, batch 1 | steady refresh | 23.547 ms | 7.490 ms | 68.2% |
| §4 shared snapshot | 200 views, batch 1 | first refresh | 214.322 ms | 21.694 ms | 89.9% |
| §5 progress index | 200 views, batch 1 | steady refresh | 7.154 ms | 6.730 ms | 5.9% |

§5's margin is thin. Read together, the facts are:
- **On the metric the bar names, it passes.** Steady refresh saved 5.9%, with
  the ranges apart.
- **The drift ran against it, so 5.9% is the conservative reading.** In that
  cell the unchanged engine ran 1.082x slower under `progidx` than under
  `snapshot`. If `ivmlite` slowed as much, §5 saved 13.1%. That correction
  assumes the drift hit both engines alike, and other cells do not show it:
  at 10 views, batch 1,000 and 100,000 groups the control moved 1.004x while
  §5 read 0.964x.
- **On steady apply + maintain it saves only 4.3%** (7.160 → 6.850 ms), below
  5%. The difference is the timed write of the one-row batch: `apply_ms` went
  from 0.069 [0.066, 0.074] to 0.164 [0.126, 0.180] ms. That one-row write
  varied as much across all four builds (0.167, 0.085, 0.069 and 0.164 ms),
  although the capture triggers' SQL, the only extension code a base write
  runs, is identical in all four, and a base write does not touch
  `__ivm_progress`. The movement is unexplained.

So the bar is met as worded and probably by more than 5.9%, but a margin
within the session's noise would need a quieter re-run to settle.

**Space.** The bootstrapped and maintained page counts are identical across
the builds, except one page more under `progidx` for the new index. The
`maintained_*` and `written_*` columns are now taken after the sixth batch, not
the first, so they **cannot be compared column for column** with Phase 4's and
Phase 5's.

### The §10.2 bars against hand-written triggers

All comparisons below are in the steady state.

- **Must: much faster than full recomputation.** Met, over a broader region
  than in Phase 5, but the first refresh no longer weighs on `ivmlite`'s side,
  which accounts for part of the change.
  - 62 of 68 exploration cells were above 2x (Phase 5: 57), and 4 were below
    1x (Phase 5: 5).
  - All 24 cells at 100,000 rows were above 2x, from 2.276x to 1,032.757x.
  - All 24 at one million rows were above 2x, from 23.453x to 7,507.890x.
  - The one 100,000-row cell near 2x was confirmed above it in every repeat:
    `views=1, batch=1000, groups=1000` had 2.369x [2.266, 2.443].
  - The losing region is still the small-table corner, 10,000 rows with
    batches of 1,000 over 1,000 groups: 0.224x (one view), 0.365x (10), 0.390x
    (50) and 0.392x (200), single runs. Phase 5's one confirmed loss,
    `views=200, base_rows=10,000, batch=100, groups=1000`, is now 2.710x
    [2.685, 2.712].
  - The other confirmed cells, all at 10,000 rows, were 1.304x (10 groups,
    batch 1,000), 1.589x, 2.540x and 2.731x (medians), each with every repeat
    on the same side of 1x and of 2x.
- **Expected: close to hand-written triggers.** **Still not met, but the gap
  is now the maintenance, not the overhead.** Below, `ivmlite`'s
  `apply + maintain` is given as a multiple of the trigger's, as the median
  [min, max] over five repeats, in every confirmed cell:

  | views | base rows | batch | groups | ivmlite / trigger |
  |---:|---:|---:|---:|---:|
  | 10 | 10,000 | 1,000 | 10 | 1.56x [1.54, 1.56] |
  | 1 | 100,000 | 1,000 | 1,000 | 6.65x [5.93, 7.15] |
  | 200 | 10,000 | 100 | 1,000 | 6.82x [6.79, 6.93] |
  | 50 | 10,000 | 100 | 1,000 | 6.86x [6.81, 6.88] |
  | 10 | 10,000 | 100 | 1,000 | 7.15x [7.02, 7.26] |
  | 1 | 10,000 | 100 | 1,000 | 8.01x [6.08, 8.54] |

  The median over the confirmed cells is 6.84x. The exploration matrix, one
  run per cell, by batch size:

  | batch | cells | median | min | max |
  |---:|---:|---:|---:|---:|
  | 1 | 17 | 22.53x | 7.64x | 55.79x |
  | 10 | 17 | 7.65x | 5.36x | 10.41x |
  | 100 | 17 | 6.61x | 2.23x | 8.67x |
  | 1,000 | 17 | 5.21x | 1.53x | 7.10x |

  - **Where ivmlite comes close:** few groups with large batches, where
    consolidation leaves few output changes. Three cells are within 2x, all at
    10 views, 10 groups and batch 1,000: 1.56x, 1.53x and 1.56x at 10,000,
    100,000 and 1,000,000 rows. None is within 1.5x.
  - **Where it still loses:** everywhere else. The medians are 5.21x at batch
    1,000, 6.61x at 100, 7.65x at 10, and 22.53x at batch 1 (7.64–55.79x).
  - **The cost per view no longer grows with the view count.** At batch 1
    the steady refresh costs 33.6–38.0 µs per view (median by view count),
    whether there are 1 or 200 views. The batch=1 ratio still rises with the
    view count, from a median 11.00x at one view to 32.25x at 200, but by tens:
    in Phase 5's first-refresh numbers it was in the thousands at 200 views.
    At batch 1,000 the medians by view count are 4.46–6.35x; the 10-view group
    includes the three 10-group cells at 1.53–1.56x.
  - The six cells confirmed in both phases all fell or held: 8.93x to 8.01x;
    6.64x to 6.65x; 9.57x to 7.15x; 1.81x to 1.56x; 11.79x to 6.86x; 17.55x to
    6.82x. Part of each fall is the protocol change, not the code.
  - The first refresh after `CREATE` still costs more than a steady one:
    3.83x at batch 1, 2.23x at 10, 1.40x at 100 and 1.05x at 1,000 (medians
    over the exploration). Taken with the apply, it puts `ivmlite` at a median
    82.76x the trigger's steady-state time at batch 1 and 5.37x at batch 1,000.
- **Bonus: beat hand-written triggers.** Not observed. No exploration cell and
  no confirmation repeat had a lower `apply + maintain` than the trigger.
  `ivmlite`'s writes alone (`apply_ms`) were cheaper than the trigger's in 50
  of 68 cells (Phase 5: 40). Refresh is still most of `ivmlite`'s time: its
  median share was 96.1% at batch 1, 92.1% at 10, 91.3% at 100 and 87.8% at
  1,000.

At batch 1 the trigger's time is a single row's: 3 to 681 µs in this matrix,
written to the CSV at 1 µs resolution, and it depends on how many views' `WHEN`
clauses that row passes. The batch=1 ratios are therefore coarse.

### What still loses, and why

The fixed costs this phase targeted are gone from the steady state. The data
shows it in two ways: a steady refresh at batch 1 costs about 34 µs per view at
every view count, and at batch 1,000 the first refresh is within 1.05x of a
steady one, so almost no per-refresh overhead is left to amortize. What
remains grows with the batch, and it is the maintenance itself:

- **the apply trigger's body**, which upserts the state table, deletes and
  inserts output rows (each also maintaining the output index), advances the
  watermarks and garbage-collects the deltas. It was 48% of a refresh in the
  spec §2 prototype profile, after the statement cache, and about 40% in Phase
  5's profile;
- **`compute`**, the operator tree, with arrangement lookups and serialized
  values: about 27% in Phase 5's profile.

The hand-written trigger does one in-place `UPDATE` of one summary row per
written row per view, with no deltas, no stage and no second table. Closing the
remaining 6.65–8.01x of the confirmed cells with 1,000 groups would need fewer
B-tree writes per changed group, cheaper operator evaluation, or both. Batching several views' refreshes into one
statement or savepoint, the next lever on the fixed side, is out of this
phase's scope (spec §1).

The small-table corner still loses to recomputation for the same reason as
before: a 10,000-row recompute is cheap, and a 1,000-row batch over 1,000
groups changes most groups anyway.

### Demos before and after

Each value is the median over five repeats. Steady-state refresh is
`maintain_ms`, in each repeat the median over ten measured batches; end to end
is apply + refresh + reading the view, the median of each batch's own sum. The
two right-hand columns are `indexed_recompute`'s end to end divided by
`ivmlite`'s.

| demo | refresh before → after | end to end before → after | first refresh before → after | vs indexed recompute before | after |
|---|---:|---:|---:|---:|---:|
| FluxFlow | 2.110 → 1.842 ms (1.15x) | 8.529 → 8.311 ms (1.03x) | 2.348 → 2.251 ms (1.04x) | 15.32x | 15.78x |
| noop | 0.641 → 0.468 ms (1.37x) | 2.527 → 2.393 ms (1.06x) | 0.726 → 0.782 ms (0.93x) | 6.82x | 7.33x |
| Zcash | 2.633 → 2.493 ms (1.06x) | 7.200 → 7.127 ms (1.01x) | 2.897 → 3.013 ms (0.96x) | 3.16x | 3.24x |
| Kener | 2.090 → 1.938 ms (1.08x) | 144.990 → 148.019 ms (0.98x) | 3.338 → 5.003 ms (0.67x) | 2.34x | 2.29x |

- Steady-state refresh got 1.06–1.37x faster in all four demos, with the
  before and after ranges apart in each. The gain is far smaller than the
  ablation's batch=1 cells, because each demo refreshes one view after a
  200- or 500-row batch, so a refresh's fixed cost was a small share of it.
- End to end moved 0.98–1.06x. In FluxFlow, `ivmlite`'s writes now cost more
  than its refresh (3.944 ms of apply against 1.842 ms of refresh), and it
  stays 4.74x slower than the hand-written trigger end to end (before: 4.85x).
- The first refresh read 1.04x in FluxFlow, 0.93x in noop and 0.96x in Zcash,
  and 0.67x in Kener, 3.338 → 5.003 ms, with a range of 3.733 to 17.204 ms
  after: that cell is noisy, and one view gains nothing from a shared
  snapshot.
- The drift control, the modes no build changes, stayed within 0.995–1.023x end
  to end, so the end-to-end changes are at most just above it.

### Phase 5 to Phase 6

The two matrices differ in protocol as well as in code, so this is the weakest
comparison in this section. Against Phase 5's single batch (the first refresh
after `CREATE`), Phase 6's steady `ivmlite` `apply + maintain` was a median
2.22x faster over the 68 cells (0.91x to 31.64x). The largest falls are at
batch 1 with many views: 30.66x and 31.64x at 200 views, where the old number
was almost all first-refresh cost. The nearest like-for-like figure, Phase 6's
apply plus its own first refresh, was a median 1.17x faster (0.67x to 8.91x),
and slower in 25 cells: all 12 one-view cells, and 13 with batches of 100 or
1,000.

Nine cells are slower in Phase 6's steady state than in Phase 5's single
batch, all at batch 1,000, by 0.912x to 0.998x: within the drift between the
sessions. The drift control is the one recompute both phases time identically,
Phase 5's single `naive_recompute` against Phase 6's warm-up recompute:
0.951x [0.826, 1.040]. This session ran about 5% slower than Phase 5's for the
same work.

### What got worse

- **Memory.** SQLite's own memory use (`sqlite3_memory_used`) after refreshing
  200 views over 100,000 rows rose from 38.3 MB (`before`) to 46.3 MB
  (`progidx`), about 40 KB per view, held by each view's cached statements.
  This is a throwaway probe, not part of the committed harness: a Python 3.14
  script with SQLite 3.53.0 that creates 200 views of the matrix's shape,
  refreshes each 31 times after one-row inserts, and reads the counter through
  `ctypes`. It gave the same two figures in two runs each.
- **Large batches with few views.** At 10 views and batch 1,000, the whole
  phase is 0.96x and 0.92x of `before` in steady refresh. §3 and §5 each read
  a few percent slower there. Both cells are near the 1.08x noise floor, and
  the cause has not been isolated.
- **§5 at 10 views and batch 1.** `snapshot` to `progidx` reads 0.921x in
  steady refresh (0.337 → 0.366 ms), with the ranges overlapping. §5 is not
  held to a bar there.
- **The one-row write in §5's own cell**, 0.069 → 0.164 ms, unexplained (see
  the acceptance paragraph).
- **Bootstrap.** `before` over `progidx` bootstrap was 0.923–1.002x across the
  ablation cells: up to 8% slower, at the noise floor.
- **The first refresh in some demos**, as above.
- **Writes** were not re-measured with the write-amplification workload: no
  change touched the write path's code (spec §6). The ablation's timed writes
  stayed within 0.936–1.018x of `before` over the whole phase, but moved
  between builds in one cell, as above.

### Limits

- **A mass reconnect is still slow.** Reconnecting 200 views over 20,000 rows
  takes about 2.5 s, measured during implementation (spec §4). After DDL from
  another connection, or after a rolled-back schema change, every view
  reconnects, and that work grows about quadratically with the view count. It
  predates this phase and is recorded for a later one; this benchmark does not
  measure it.
- **A user trigger can pin a connection.** A user's trigger on an `__ivm_`
  table that reads an ivmlite view is compiled into that view's cached arming
  statement, so `sqlite3_close` then returns `SQLITE_BUSY` (spec §3, rule 2).
  Writing triggers on reserved tables is out of scope, like other tampering.
- **Space snapshots now cover six batches.** The `written_*` and
  `maintained_*` pages are taken after the last of six batches, not after one,
  so they cannot be compared column for column with Phase 4's and Phase 5's.
- The exploration has one run per cell; only the 6 selected cells have five
  repeats, and all but one of them are at 10,000 rows.
- The machine was not idle, and the ablation's own drift was up to 8%. §5's
  acceptance margin is inside it.
- Data is uniform, databases are in-memory, and every view refreshes once per
  batch. Zipf distributions, locality, durability, concurrent connections,
  Turso and Nexmark are still not covered.
- The demos are synthetic fixtures shaped after public reports
  ([`docs/demos/`](../demos/README.md)), not production workloads.

Raw data:

- [`m1b-phase6.csv`](m1b-phase6.csv): 68 cells × 4 engines, steady state.
- [`m1b-phase6-confirm.csv`](m1b-phase6-confirm.csv): 6 selected cells × 5 repeats × 4 engines.
- [`m1b-phase6-ablation.csv`](m1b-phase6-ablation.csv): four builds × five cells × five repeats × two engines.
- [`m1b-phase6-ablation-views10-batch1.csv`](m1b-phase6-ablation-views10-batch1.csv): the same four builds on the supplementary cell (10 views, batch 1), five repeats × two engines.
- [`m1b-phase6-ablation-builds.csv`](m1b-phase6-ablation-builds.csv): the commit behind each ablation label.
- [`m1b-phase6-demos.csv`](m1b-phase6-demos.csv) and [`m1b-phase6-demos-builds.csv`](m1b-phase6-demos-builds.csv): the four demos against two builds, and per label the extension commit, the harness commit (`a92a570`, with no uncommitted changes under `crates/` or the Cargo files) and the SQLite version.
- [`phase6_tables.py`](phase6_tables.py): derives every table and figure in this section from the CSVs above.

Charts:

- Total steady-state `apply + maintain` at 10 views and batch 100: [`groups=10`](m1b-phase6-card10.svg), [`groups=1,000`](m1b-phase6-card1000.svg), [`groups=100,000`](m1b-phase6-card100000.svg).
- Speedup surface with 1x/2x reference lines: [`groups=10`](m1b-phase6-speedup-card10.svg), [`groups=1,000`](m1b-phase6-speedup-card1000.svg), [`groups=100,000`](m1b-phase6-speedup-card100000.svg).

## M1b Phase 7: `ivm_refresh_all()`

Phase 7 ([spec](../superpowers/specs/2026-10-12-m1b-phase7-refresh-all-design.md))
added `ivm_refresh_all()`, which refreshes every view in one call, either
atomically or view by view (spec §2). It also made a refresh with nothing new
return early, after its checks (§4). This section measures what the function
costs against an application's own loop of per-view refreshes, and what the
early return saves.

Every measured number in this section comes from
[`phase7_tables.py`](phase7_tables.py) (`python3 docs/bench/phase7_tables.py`),
which reads the two committed CSVs and uses only the standard library. The
exceptions are the setup facts below, from the run session, and the one
throwaway probe named under "Limits".

### Setup

**A different driver from the matrix.** The numbers come from
[`scripts/bench-refresh-all.py`](../../scripts/bench-refresh-all.py), a
Python driver using the standard library's `sqlite3`, not from
`ivmlite-bench`. The matrix compares engines, while this driver compares
ways of calling a refresh (spec §6). It loads the extension into Python
3.13.13's SQLite 3.53.0, not into the SQLite 3.53.2 that `rusqlite` bundles.
Its rows and batches have the m0 workload's shape but come from Python's own
seeded generator, so they are not the matrix's rows:
- 100,000 base rows in 1,000 groups, with amounts in [0, 200);
- the matrix's views, `SELECT region, SUM(amount), COUNT(*) ... WHERE amount > k GROUP BY region`,
  with `k = (i * 7) % 150`;
- batches that delete with probability 0.33 and otherwise insert.

**Protocol.** Each case runs in a fresh in-memory database. It applies one
warm-up batch and refreshes every view, then applies five measured batches.
Only the refresh of every view is timed. Each batch's writes run untimed, in
one transaction, just before it. After the last batch, every view is checked
against SQLite's own evaluation of its query, and none mismatched. The whole
set ran three times (`--rounds 3`), so each value below is the median
[min, max] over 15 measured batches, in milliseconds. `before` and `after`
alternate per case and per round.

**The shapes.** Each shape refreshes every view, in `name` order:
- `app_cached`: one `INSERT INTO v(v) VALUES ('refresh')` per view, with the
  same SQL text every time. The connection's Python statement cache holds
  1,024 statements, so each view's statement is prepared once and reused;
- `reprepared`: the same loop on a connection opened with
  `cached_statements=0`, so each statement is prepared and finalized on every
  call;
- `refresh_all`: `SELECT ivm_refresh_all()`, on the same connection setup as
  `app_cached`;
- `refresh_all_continue`: `SELECT ivm_refresh_all('continue')`, on the same
  setup;
- `reprepared_with_checks`: `reprepared`, plus the statements the function
  runs per view to count skips (§4), each prepared afresh from Python.

**The cases:**
- 10 and 200 views over one table, at batch 1 and batch 1,000;
- the skip cases: 200 views over two tables of 100,000 rows each, 100 views
  per table. `write=one` writes a one-row batch to the first table only, so
  half the views have nothing new. `write=none` writes nothing.

**Builds.** `before` is `8b57dc5`, master at Phase 7 start, which has no
`ivm_refresh_all` and no early return. `after` is `768f757`, the end of Phase
7's Task 2. The extension code at the branch head is the same.
[`m1b-phase7-refresh-all-builds.csv`](m1b-phase7-refresh-all-builds.csv)
gives each label's full commit and the SHA-256 of its library. Both are
release builds, with Rust 1.95.0.

**The machine** is the same 10-core Apple M2 Pro with 16 GB RAM as Phases 4
to 6, under macOS 26.2 (25C56). The run took place on 2026-10-12 under
`caffeinate -i`. It took 1,979.53 s. The load average was
1.72 / 3.31 / 4.50 at the start and 2.21 / 2.09 / 2.38 at the end. The
1-minute load at the start of each case ranged from 1.37 to 3.77 (median
2.10). Desktop applications stayed open.

**The noise floor.** In the four view cases every view has new deltas, so the
early return never fires and both builds do the same refresh work there. The
loop shapes' `after / before` stayed within 0.969–1.016x, with every range
overlapping.

### Reproducing

From the repository root, with each build's release library copied out of
its own scratch `CARGO_TARGET_DIR`:

```bash
python3 scripts/bench-refresh-all.py before.dylib after.dylib \
  --before-commit 8b57dc5935e5af5fe714b57bb48c7963aa3a620f \
  --after-commit 768f75703e90afa79da6b20bf749816541ea3359 --rounds 3
python3 docs/bench/phase7_tables.py
```

The driver needs a Python whose `sqlite3` can load extensions. macOS's system
Python cannot; Homebrew's can.

### The shapes side by side

`after` build. `before` has only the two loop shapes; its numbers are in the
next two subsections.

| case | app_cached | reprepared | refresh_all | refresh_all_continue | reprepared_with_checks |
|---|---:|---:|---:|---:|---:|
| 10 views, batch 1 | 0.380 [0.324, 0.508] | 0.354 [0.301, 0.469] | 0.434 [0.384, 0.555] | 0.448 [0.396, 0.570] | 0.455 [0.400, 0.566] |
| 10 views, batch 1,000 | 58.261 [55.300, 61.346] | 55.859 [52.921, 58.058] | 56.924 [53.857, 59.832] | 57.019 [54.603, 63.081] | 56.059 [52.908, 60.973] |
| 200 views, batch 1 | 8.157 [5.776, 12.191] | 7.432 [5.339, 9.972] | 8.965 [6.744, 11.705] | 9.154 [7.024, 11.634] | 9.354 [7.292, 11.825] |
| 200 views, batch 1,000 | 986.169 [949.280, 1,008.915] | 950.692 [906.848, 986.449] | 944.844 [910.722, 1,012.434] | 951.873 [918.308, 973.678] | 942.897 [907.816, 975.577] |
| skip, write=one | 4.471 [3.367, 5.752] | 4.275 [3.141, 5.761] | 5.394 [4.337, 6.561] | 5.806 [4.581, 7.113] | 6.053 [4.950, 7.455] |
| skip, write=none | 0.866 [0.834, 0.986] | 0.929 [0.915, 0.954] | 1.719 [1.696, 1.754] | 1.967 [1.942, 2.001] | 2.672 [2.629, 2.747] |

### What the function costs

**`ivm_refresh_all` is slower than the application's own loop of cached
statements whenever the refresh work is small.** The table compares it with
`app_cached`, on the same connection setup. The ratio is of the medians, and
above 1 means the function is slower. The per-round ratios are each round's
median against the other shape's in the same round.

| case | app_cached | refresh_all | ratio | extra µs per view | per-round ratios | continue ratio |
|---|---:|---:|---:|---:|---:|---:|
| 10 views, batch 1 | 0.380 | 0.434 | 1.142x | +5.4 | 1.139–1.197x | 1.179x |
| 200 views, batch 1 | 8.157 | 8.965 | 1.099x | +4.0 | 1.079–1.153x | 1.122x |
| skip, write=one | 4.471 | 5.394 | 1.206x | +4.6 | 1.198–1.218x | 1.299x |
| skip, write=none | 0.866 | 1.719 | 1.985x | +4.3 | 1.886–2.056x | 2.271x |
| 10 views, batch 1,000 | 58.261 | 56.924 | 0.977x | −133.7 | 0.974–0.980x | 0.979x |
| 200 views, batch 1,000 | 986.169 | 944.844 | 0.958x | −206.6 | 0.958–0.976x | 0.965x |

- **The fixed cost is 4.0–5.4 µs per view per call.** It is the same with
  10 or 200 views, and with one-row or empty batches. Every round agrees: the
  function was slower in every round of these four cases. The batch ranges
  overlap in all but `write=none`, where every view is idle and the function
  takes 1.99x the loop's time.
- **Why: its nested statements are prepared on every call.** For each view,
  the function prepares and finalizes the nested `INSERT INTO v(v)`, plus
  the statements that count skips: the view's watermark and each base table's
  latest delta (§4). A statement that names an ivmlite virtual table holds a
  reference on it, which defers its xDisconnect and makes `sqlite3_close`
  fail with `SQLITE_BUSY`. So Phase 6 §3 rule 2 forbids caching the nested
  `INSERT`, and the function finalizes it before it returns (spec §3). The
  application's loop keeps its statements prepared for as long as it likes,
  and closing them is its own job.
- **Where the cost goes.** `reprepared_with_checks` issues the function's
  per-view statements from Python. It adds 8.7–10.1 µs per view over
  `reprepared` at batch 1 and in the skip cases, more than the function's
  whole extra cost. That is because each of its statements also pays
  Python's own per-call overhead. So the driver cannot split the function's
  4–5 µs between the nested `INSERT` and the skip checks. The skip checks do
  not name a virtual table, so rule 2 would not forbid caching them; that is
  a lever for a later phase.
- **The `'continue'` mode costs a little more**: one savepoint per view, at
  1.12–1.30x the loop at batch 1 and 2.27x when every view is idle.
- **At batch 1,000 the function was 2.3–4.2% faster** than the loop, in every
  round (0.958–0.980x), with the batch ranges overlapping. The work per view
  is the same. The likely difference is that the function runs every view in
  one transaction, while the loop commits once per view in autocommit. This
  was not investigated.

**Its value is atomicity and the API, not speed.** With one call, every view
refreshes or none does, failures are reported by name, and idle views are
counted. An application that only wants speed and does not need the views
to move together should keep a prepared refresh statement per view.

**The `reprepared` loop is not a measure of the prepare cost.** It was
*faster* than `app_cached` in 11 of the 12 build-and-case pairs (0.902–0.971x),
including at batch 1,000 (0.942–0.964x), where a prepare is a negligible
share of the work. The exception is the `after` build with every view idle
(1.073x), where a refresh is cheapest. Its first refresh was faster too. The two
loops differ in the connection's Python statement cache as well as in
statement reuse, and the cache-off connection ran every refresh faster. A
throwaway probe (10 views, batch 1,000, three fresh databases each) found
the same gap with the refresh statements prepared afresh on a cache-on
connection, so it is the connection's setup, not the reuse. The cause was not
isolated. So the only like-for-like comparisons here are on the same setup:
`refresh_all` against `app_cached`, and `reprepared_with_checks` against
`reprepared`.

### What the early return saves

Phase 7 §4 made a refresh with nothing new return after its checks, before it
computes, stages or applies anything. The skip cases measure it with the same
loop in both builds:

| case | shape | before | after | before / after | saved per idle view |
|---|---|---:|---:|---:|---:|
| write=none | app_cached | 3.748 [3.696, 3.949] | 0.866 [0.834, 0.986] | 4.33x | 14.4 µs |
| write=none | reprepared | 3.641 [3.502, 4.369] | 0.929 [0.915, 0.954] | 3.92x | 13.6 µs |
| write=one | app_cached | 5.940 [4.795, 7.351] | 4.471 [3.367, 5.752] | 1.33x, ranges overlap | 14.7 µs |
| write=one | reprepared | 5.425 [4.412, 6.760] | 4.275 [3.141, 5.761] | 1.27x, ranges overlap | 11.5 µs |

- **An idle view's refresh fell from 18.7 to 4.3 µs** (`app_cached`, every
  view idle). What remains is the statement itself and the checks, which
  always run (§4).
- **With half the views idle**, refreshing all 200 views is 1.33x faster.
  The ranges overlap, because the 100 written views' refreshes vary more
  than the 100 idle ones save.
- **`ivm_refresh_all` against the `before` build's loop:** when every view is
  idle, the function takes 1.719 ms, against 3.748 ms for `before`'s cached
  loop (2.18x faster). With half the views idle, it takes 5.394 ms against
  5.940 (1.10x, ranges overlap). Both gains are the early return's, which a
  plain loop gets too in the `after` build.
- The function counts a view as skipped by reading the watermarks and the
  delta tables, and every call returned the expected counts:
  `refreshed 100, skipped 100, failed 0` with one table written,
  `refreshed 0, skipped 200, failed 0` with none.

### Limits

- **A different driver from the matrix.** These numbers come from Python's
  `sqlite3` and SQLite 3.53.0, with Python's own per-statement overhead
  inside every loop timing, and they are not comparable column for column
  with the Rust runner's. The loop shapes' per-view costs include that
  overhead; the function's nested statements do not pay it.
- **The re-prepared loop ran on a different connection setup**, and that
  setup was faster for reasons not isolated (above). The cost of re-preparing
  one nested `INSERT` was therefore not measured on its own.
- **The machine was not idle**, with a 1-minute load of 1.37–3.77 at case
  starts. The noise floor between builds was 0.969–1.016x. Within one case,
  batch-to-batch ranges are wide at batch 1: 5.8–12.2 ms at 200 views.
- **One refresh policy.** In-memory databases, uniform data, and every view
  refreshed after every batch. The function was measured with no failing
  view; its failure paths are covered by tests (spec §5), not timed.

Raw data:

- [`m1b-phase7-refresh-all.csv`](m1b-phase7-refresh-all.csv): one row per timed refresh of every view: two builds × six cases × their shapes × three rounds × six batches (batch 0 is the warm-up), with the function's return value and the 1-minute load average at the case's start.
- [`m1b-phase7-refresh-all-builds.csv`](m1b-phase7-refresh-all-builds.csv): the commit, the library's SHA-256 and the SQLite version behind each label.
- [`phase7_tables.py`](phase7_tables.py): derives every table and figure in this section from the CSVs above.
