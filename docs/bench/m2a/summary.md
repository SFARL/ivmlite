# M2a report

## Definitions

- A run's total for a mode is the median over the five measured rounds of that round's apply+maintain+read sum, never a sum of component medians. Apply+maintain is the median of each round's apply+maintain sum.
- Fastest complete recompute of a run is the smaller of the `recompute` (ordinary) and `covering_recompute` totals of that same run. `no_maintenance` is never a numerator or a denominator of a speedup; its apply time is reported only as the write lower bound.
- Ratio = fastest-recompute total / ivmlite total, per run (seed). Per cell: min, median and max over the cell's complete runs; a cell with fewer complete runs than planned is `partial`. Seeds measure fixture sensitivity; they are not repeated trials or confidence bounds.
- Confirmation candidate: a cell whose [min, max] ratio range contains 1.0 or 2.0, or whose median is within 10% of 1.0 or 2.0.
- Space: per checkpoint, ivmlite / base-only and covering / base-only allocated-byte ratios at the same checkpoint key (base-only is the `no_maintenance` database), live-page proxies, logical state rows and `retained_stage_rows`. Page allocation is not RSS or peak memory.
- Targets: top-10% group row share of the initial fixture, realized hot target share, and the hot share of the groups matched by each batch, plus distinct primary keys, repeated touches, matched partners per batch and target-group counts.
- Target selection (`target_selection` = `group_then_row`): existing-row targets are chosen group-first (group by profile, then a uniform live row in it); uniform group selection is not uniform row selection.
- Churn: page_count, freelist_count and logical state rows per round, per profile.
- A case without `completed.json`, or whose completed artifacts cannot be read or no longer match their completion hashes, is listed in `losses.csv` with its phase and error and contributes to no ratio. Phase `interrupted` is a started case that left neither a completion marker nor a failure record; `not_run` is a planned case that never started.
- Confirmation (`confirmations.csv`): timing-only paired repeats of each candidate cell at seed 42 on an idle host (1-minute load average recorded before and after each repeat; the planned repeat count is `expected_repeats`). A repeat whose load after the run exceeds the `--max-load` that admitted it is `busy after`: it stays in the table, is flagged, and is counted per cell. `initial_build_differs` / `other build` names the fields (compiler, host) in which the initial run was made differently; a different source, runner, extension or SQLite is refused at run time. Ratios use the same definition as above. They are reported separately and never enter `timing.csv` or `ratios.csv`; the cell row gives the full range of the repeats and whether it still contains 1.0 or 2.0, next to the initial seed range for comparison.
- Smoke selections are reported on one line and are never mixed into the matrix tables.

## Completeness

98 cases complete, 0 listed in `losses.csv`.
Smoke selection (reported only here, never mixed into any table): 96 planned, 96 complete, 0 failed.


## Provenance of the run, churn and confirm cases

- source revision: 34787ec81072468fb82106be8807d9124f2b3ecf
- source sha256: fdeb52b3c1f32c5c55b3978cf1174c3446505209d63db2eb95ab84e0672e4e14
- runner sha256: 6914b070d5dc4fce6b83230b75a8ab5a4e49adb7ca26d92e957556ade3d264b5
- extension sha256: 8448448b6ea64ccf54fb59b3f8fc1aff8c0bef14dbaa37fd68e23ca2649a53d4
- config sha256: 2c782c19a883367a291a9b103bc44470c192dcc9a21d1cc048f3d9fb9d5544cb
- SQLite: 3.53.2
- rustc: rustc 1.95.0 (59807616e 2026-04-14); binary: rustc; commit-hash: 59807616e1fa2540724bfbac14d7976d7e4a3860; commit-date: 2026-04-14; host: aarch64-apple-darwin; release: 1.95.0; LLVM version: 22.1.2
- host: Darwin MacBook-Pro-8.local 27.0.0 Darwin Kernel Version 27.0.0: Tue Aug 11 21:20:40 PDT 2026; root:xnu-13432.1.9~1/RELEASE_ARM64_T6020 arm64
- cpu: Apple M2 Pro
- memory: 17179869184
- logical cpus: 10
- smoke gate: passed
- tracked source changes: no

## Ratio against the fastest complete recompute

| cell | runs | min | median | max | candidate | partial | ivmlite bootstrap ms | ivmlite/base-only bytes (final idle) |
|---|---|---|---|---|---|---|---|---|
| single_uniform_hot_n100000_g1000_f1_b10 | 3/3 | 9.863 | 10.107 | 10.232 | no |  | 55.170 | 1.232 |
| single_uniform_uniform_n100000_g1000_f1_b10 | 3/3 | 10.199 | 10.207 | 10.281 | no |  | 55.543 | 1.232 |
| single_zipf_hot_n100000_g1000_f1_b10 | 3/3 | 10.014 | 10.111 | 10.159 | no |  | 46.400 | 1.253 |
| single_zipf_uniform_n100000_g1000_f1_b10 | 3/3 | 9.794 | 9.841 | 9.888 | no |  | 46.381 | 1.253 |
| single_uniform_hot_n100000_g1000_f1_b1000 | 3/3 | 0.407 | 0.409 | 0.409 | no |  | 55.951 | 1.282 |
| single_uniform_uniform_n100000_g1000_f1_b1000 | 3/3 | 0.362 | 0.363 | 0.368 | no |  | 55.838 | 1.327 |
| single_zipf_hot_n100000_g1000_f1_b1000 | 3/3 | 0.461 | 0.467 | 0.475 | no |  | 46.605 | 1.263 |
| single_zipf_uniform_n100000_g1000_f1_b1000 | 3/3 | 0.378 | 0.379 | 0.411 | no |  | 46.508 | 1.326 |
| join_uniform_hot_n32768_g1024_f1_b10 | 3/3 | 15.699 | 16.038 | 16.148 | no |  | 155.377 | 5.777 |
| join_uniform_uniform_n32768_g1024_f1_b10 | 3/3 | 15.707 | 15.941 | 15.996 | no |  | 155.650 | 5.777 |
| join_zipf_hot_n32768_g1024_f1_b10 | 3/3 | 13.667 | 13.792 | 13.983 | no |  | 152.547 | 5.880 |
| join_zipf_uniform_n32768_g1024_f1_b10 | 3/3 | 13.259 | 13.479 | 13.819 | no |  | 153.697 | 5.880 |
| join_uniform_hot_n32768_g1024_f1_b1000 | 3/3 | 0.478 | 0.479 | 0.481 | no |  | 155.522 | 5.444 |
| join_uniform_uniform_n32768_g1024_f1_b1000 | 3/3 | 0.408 | 0.438 | 0.441 | no |  | 157.721 | 5.416 |
| join_zipf_hot_n32768_g1024_f1_b1000 | 3/3 | 0.453 | 0.457 | 0.458 | no |  | 153.033 | 5.549 |
| join_zipf_uniform_n32768_g1024_f1_b1000 | 3/3 | 0.404 | 0.405 | 0.405 | no |  | 152.801 | 5.520 |
| join_uniform_hot_n32768_g1024_f8_b10 | 3/3 | 24.470 | 24.557 | 25.242 | no |  | 270.434 | 5.789 |
| join_uniform_uniform_n32768_g1024_f8_b10 | 3/3 | 24.841 | 24.971 | 25.073 | no |  | 268.714 | 5.805 |
| join_zipf_hot_n32768_g1024_f8_b10 | 3/3 | 25.405 | 25.646 | 25.685 | no |  | 251.723 | 5.921 |
| join_zipf_uniform_n32768_g1024_f8_b10 | 3/3 | 25.161 | 25.205 | 25.492 | no |  | 250.558 | 5.921 |
| join_uniform_hot_n32768_g1024_f8_b1000 | 3/3 | 0.652 | 0.653 | 0.656 | no |  | 270.478 | 5.428 |
| join_uniform_uniform_n32768_g1024_f8_b1000 | 3/3 | 0.644 | 0.644 | 0.646 | no |  | 269.092 | 5.428 |
| join_zipf_hot_n32768_g1024_f8_b1000 | 3/3 | 0.705 | 0.711 | 0.713 | no |  | 250.653 | 5.555 |
| join_zipf_uniform_n32768_g1024_f8_b1000 | 3/3 | 0.642 | 0.669 | 0.676 | no |  | 257.152 | 5.584 |
| join_uniform_hot_n32768_g1024_f64_b10 | 3/3 | 39.296 | 39.418 | 39.509 | no |  | 1252.294 | 5.867 |
| join_uniform_uniform_n32768_g1024_f64_b10 | 3/3 | 38.881 | 39.015 | 39.315 | no |  | 1251.083 | 5.883 |
| join_zipf_hot_n32768_g1024_f64_b10 | 3/3 | 55.139 | 55.454 | 56.270 | no |  | 1200.660 | 5.975 |
| join_zipf_uniform_n32768_g1024_f64_b10 | 3/3 | 51.225 | 56.864 | 57.355 | no |  | 1198.622 | 5.992 |
| join_uniform_hot_n32768_g1024_f64_b1000 | 3/3 | 1.145 | 1.147 | 1.148 | no |  | 1255.053 | 5.523 |
| join_uniform_uniform_n32768_g1024_f64_b1000 | 3/3 | 1.117 | 1.131 | 1.132 | no |  | 1250.818 | 5.551 |
| join_zipf_hot_n32768_g1024_f64_b1000 | 3/3 | 1.201 | 1.206 | 1.216 | no |  | 1186.233 | 5.661 |
| join_zipf_uniform_n32768_g1024_f64_b1000 | 3/3 | 1.202 | 1.206 | 1.235 | no |  | 1206.572 | 5.691 |

## Does 2x survive

16 of 32 cells exceed 2x in every complete run; 0 more exceed it in some run; 16 never do; 0 have no result (every run lost). Cells below 1.0 in every run: 12.

## Confirmation of candidate cells

No confirmation run.
