# M2a: non-uniform workload validation

Spec: [`2026-10-15-m2a-nonuniform-validation-design.md`](../superpowers/specs/2026-10-15-m2a-nonuniform-validation-design.md). Plan: [`2026-10-17-m2a-completion.md`](../superpowers/plans/2026-10-17-m2a-completion.md).

**Question.** Does ivmlite's advantage over full recomputation survive skewed group sizes, concentrated updates and join fanout? Where does it disappear, and how much state does it keep?

**Answer, in one line.** Batch size decides the verdict; skew does not.
- **Small batches:** ivmlite wins every cell, by 10× to 57×.
- **Large batches:** at 1,000 row operations per batch, it loses on the single table and at join fanout 1 and 8 (0.36× to 0.71×). At fanout 64 it holds 1.12× to 1.24×, under 2×.

Zipf groups and hot updates move the ratios by at most a factor of about 1.46 (fanout 64, batch 10), and they flip no verdict.

## What was run

The engine is the alpha.5 extension at commit `34787ec`, release build. Its SHA-256 and the full provenance are in [`m2a/summary.md`](m2a/summary.md).

**The matrix.** 32 cells × 3 seeds (1, 42, 2026), with one warm-up and five measured batches per mode and run.
- **Single table:** 100,000 rows and 1,000 groups, `SELECT g, COUNT(*), SUM(amount) FROM events GROUP BY g`.
- **Join:** 32,768 rows per side and 1,024 output groups, fanout f ∈ {1, 8, 64}. The initial join has 32,768·f rows, up to 2,097,152.
- **Profiles:**
  - uniform data with uniform updates;
  - Zipf(1.2) data with uniform updates;
  - uniform data with hot updates;
  - Zipf data with hot updates.

  "Hot" sends 80% of existing-row targets to the first 10% of group ranks.
- **Batch sizes:** 10 and 1,000 SQL row operations. Every ten operations are two INSERTs, two DELETEs, three amount UPDATEs and three group-key UPDATEs.

**Modes.** Each mode runs on a fresh in-memory connection over the same exported fixture and trace:
- **no maintenance:** the write cost alone, the lower bound;
- **ordinary recompute;**
- **covering recompute:** extra covering indexes, whose write cost is timed too;
- **ivmlite.**

The ratio is the faster complete recompute total over the ivmlite total. Each total is the median of per-round apply+maintain+read sums. In every cell the faster recompute was the covering one.

**Checks outside the timers.** A separate correctness replay compared every view with SQLite's full recomputation, as a multiset, after bootstrap and after every batch. It also checked the decoded ivmlite state against independently counted live base rows, and confirmed that a second idle refresh changes nothing.

**Completeness.**
- All 98 cases completed: 96 matrix runs and 2 churn probes. Nothing is in [`losses.csv`](m2a/losses.csv).
- The smoke selection (96 cases) passed before the matrix.
- No cell met the confirmation rule (a seed range containing 1.0 or 2.0, or a median within 10% of either), so [`confirm-candidates.json`](m2a/confirm-candidates.json) is empty and no confirmation repeats were run.
- The 1-minute load average was between 1.6 and 2.0 when each step started.

## Ratios against the fastest complete recompute

Each range is min to max of the per-seed ratios across the four profiles of that row (12 runs). Per-cell values are in [`ratios.csv`](m2a/ratios.csv).

| Shape | Batch 10 | Batch 1,000 |
|---|---|---|
| single table | 9.8× – 10.3× | **0.36× – 0.48×** |
| join, f = 1 | 13.3× – 16.1× | **0.40× – 0.48×** |
| join, f = 8 | 24.5× – 25.7× | **0.64× – 0.71×** |
| join, f = 64 | 38.9× – 57.4× | 1.12× – 1.24× |

The counts:
- 16 of 32 cells exceed 2× in every run; all of them are batch-10 cells.
- 12 cells are below 1× in every run.
- The remaining 4 cells (f = 64, batch 1,000) win, but by less than 2×.

**Absolute times.** Medians over seeds, in ms, for one batch's apply+maintain+read:

| Shape | Batch | Ordinary recompute | Covering recompute | ivmlite |
|---|---:|---:|---:|---:|
| single | 10 | 21.8 – 24.2 | 3.8 – 3.9 | 0.38 |
| single | 1,000 | 22.2 – 24.7 | 4.9 – 5.1 | 10.7 – 13.8 |
| join f = 1 | 1,000 | 12.2 – 13.1 | 8.2 – 9.6 | 18.2 – 21.7 |
| join f = 8 | 1,000 | 57.6 – 60.1 | 18.4 – 19.9 | 26.0 – 30.9 |
| join f = 64 | 10 | 413.6 – 429.2 | 94.6 – 97.1 | 1.7 – 2.5 |
| join f = 64 | 1,000 | 413.6 – 430.9 | 96.1 – 98.9 | 79.7 – 87.2 |

### Where the advantage fails, and why

At batch 1,000, ivmlite's cost grows with the batch while recomputation's barely moves. With the uniform profile, the medians split as:

| Shape | no-maintenance apply | ivmlite apply | ivmlite maintain | ivmlite read |
|---|---:|---:|---:|---:|
| single | 0.45 | 7.43 | 6.16 | 0.17 |
| join f = 1 | 0.92 | 9.19 | 12.38 | 0.17 |
| join f = 8 | 0.94 | 9.22 | 21.54 | 0.17 |
| join f = 64 | 0.98 | 9.34 | 77.75 | 0.18 |

- **Write capture costs 10–17× the bare write**, about 7–9 µs per row operation. That alone exceeds a covering-index recompute of the whole 100,000-row aggregate (about 5 ms).
- **Maintenance then adds roughly another 6 µs per operation on the single table.** At higher fanout it grows with the matched partners each operation touches.
- **Recomputation cost depends on the table, not the batch.** A covering index makes the single-table aggregate cheap enough that 1,000 changes (1% of the table) are not worth maintaining incrementally.

The crossover sits between the two batch sizes measured. This matrix does not locate it more precisely.

### How skew and hot updates change it

- **Zipf data with fanout 64 at batch 10** made ivmlite faster: 1.7 ms against 2.5 ms, which raises the ratio from about 39× to about 55×. Fewer, larger groups mean fewer distinct output rows change per batch.
- **Hot updates at batch 1,000** helped by 7–17%, for example 10.7 ms (zipf_hot) against 12.9 ms (zipf_uniform) on the single table. Concentrated targets touch fewer groups.
- Neither effect moves a cell across 1× or 2×.

The realized hot target share was 0.786 under the hot policy, and 0.094–0.100 without it. The initial top-10% group row share was 0.098–0.101 for uniform data and 0.81–0.83 for Zipf data ([`targets.csv`](m2a/targets.csv)).

## Bootstrap and space

**Bootstrap.** Creating the view takes:
- 46–56 ms on the single table;
- 153–158 ms at f = 1, 251–270 ms at f = 8 and 1,186–1,255 ms at f = 64.

All of this is outside the per-batch timings ([`ratios.csv`](m2a/ratios.csv), `ivmlite bootstrap ms`).

**Allocated bytes**, relative to the base-only database at the same checkpoint (final idle refresh; [`space.csv`](m2a/space.csv)):

| Shape | ivmlite / base-only | covering recompute / base-only |
|---|---:|---:|
| single | 1.22 – 1.33 | 2.02 – 2.19 |
| join, any f | 5.39 – 5.99 | 1.54 – 1.61 |

On the single table, ivmlite keeps less than the covering index costs. On the join it keeps 5–6× the base tables, against about 1.6× for covering indexes. The join's retained state grows with the base rows, not with the join output. Its arrangements hold about 66,000 entries for 65,536 base rows at every fanout, while the joined row count reaches 2,097,152 at f = 64.

**Retained stage rows.** The stage table keeps the last applied batch until the next apply empties it. This follows format 5's design (spec §5, amendment 2026-10-17), and the numbers here include it: at most 2,982 rows on the single table and at most 5,695 rows on the join.

## Bounded churn

The churn probe runs 100 balanced batches of 1,000 operations at f = 64 and seed 42, for uniform/uniform and Zipf/hot. Each probe was then replayed in reverse to restore the initial bases ([`churn.csv`](m2a/churn.csv)).
- The oracle and decoded-state checks passed after every round in both directions.
- **Allocation never grew.** `page_count` stayed at 2,165 (uniform) and 2,157 (Zipf/hot) for all 100 rounds and the inverse replay. Only the freelist moved.
- **The inverse replay restored the bootstrap state exactly:** 66,560 arrangement entries and 1,024 aggregate groups. No entry was missing and no stale entry remained.
- **The Zipf/hot probe drifts by design.** Its live group count fell from 1,024 to 432 over the forward replay. Group-key updates draw from the Zipf distribution and empty the cold groups. The spec calls this profile drift: the profile describes the initial fixture and the target policy, not a stationary distribution.

This probes logical retention. It is not a proof that memory cannot leak.

## Methodology notes

- **Mode order** rotates by cell, seed and repeat: `(cell + seed + repeat) mod 4` cyclic shifts of one fixed order. That covers every position but not every permutation.
- **The workload hash** in each manifest hashes the parsed workload configuration, not the TOML bytes. Formatting changes to `workloads/m2a.toml` do not change it.
- **Targets are chosen group-first:** a group by the profile, then a uniform live row within it. Uniform group selection is therefore not uniform row selection on Zipf data, and every result here measures group locality.
- **No speedup is claimed for incomplete runs,** and there were none. No new comparison with hand-written triggers is made (spec §4).

## Files

- [`m2a/summary.md`](m2a/summary.md): definitions, completeness, provenance and the per-cell ratio table, generated by `m2a report`.
- [`m2a/timing.csv`](m2a/timing.csv), [`ratios.csv`](m2a/ratios.csv), [`space.csv`](m2a/space.csv), [`targets.csv`](m2a/targets.csv), [`churn.csv`](m2a/churn.csv), [`losses.csv`](m2a/losses.csv), [`confirmations.csv`](m2a/confirmations.csv), [`confirm-candidates.json`](m2a/confirm-candidates.json): the derived tables.
- [`m2a/raw/`](m2a/raw/): each full-size case's `manifest.json`, `timing.json` (every raw round), `space.json` and `completed.json`, plus the selection files.
- **Omitted raw artifacts.** The full raw set is 552 MB, so the fixtures, traces, per-batch correctness records and target records are left out. [`m2a/raw-sha256.txt`](m2a/raw-sha256.txt) lists the SHA-256 of every file in the full set. The fixtures are deterministic: `m2a export` regenerates them, and each manifest records the expected fixture hash.

### Reproducing

```bash
scripts/build-extension.sh
cargo build --release --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked
cargo build --release -p ivmlite-bench --bin m2a --locked
./target/release/m2a run --smoke --out target/m2a
./target/release/m2a run --out target/m2a
./target/release/m2a churn --out target/m2a
./target/release/m2a report --out target/m2a --report target/m2a/report
```

On an Apple M2 Pro, the matrix took 22.5 minutes and the churn probes 19 minutes.
