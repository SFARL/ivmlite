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

Every number in this section is derived from the committed CSVs by
[`phase4_tables.py`](phase4_tables.py) (`python3 docs/bench/phase4_tables.py`);
nothing is typed in by hand.

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

From the repository root, on the commit that holds this README:

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
were above 2x. Four confirmed cells at 100,000 rows sit close to the 2x bar,
and all four stayed above it in every repeat:

| views | base rows | batch | groups | confirmed speedup median [min, max] |
|---:|---:|---:|---:|---:|
| 10 | 100,000 | 1,000 | 1,000 | 2.576x [2.563, 2.642] |
| 10 | 100,000 | 1,000 | 100,000 | 2.174x [2.107, 2.700] |
| 50 | 100,000 | 1,000 | 1,000 | 2.352x [2.332, 2.387] |
| 200 | 100,000 | 1,000 | 1,000 | 2.042x [2.024, 2.052] |

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

The transition is not one clean crossover. For example, at `views=1,
base_rows=100,000, batch=1000, groups=1000`, the confirmed median was 1.965x
but the five runs ranged from 1.703x to 3.808x. The charts therefore show a
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
