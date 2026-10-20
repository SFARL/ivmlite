# M2c confirmation follow-up

This bundle preserves one follow-up of the original flagged confirmation cell:
`views=1, base_rows=10000, batch_size=100, group_cardinality=1000`.
The selection is the four original engine rows from `../m1b-phase6.csv`.
Each label contains five fresh runs of all four engines (20 rows per label).
The original full confirmation captures remain in `../m2c-confirm-before.csv`
and `../m2c-confirm-after.csv` without replacement or filtering.

Each frozen checkout used its own existing release benchmark binary and extension
library, with the workload at that same commit. `capture.py` checked each checkout's
HEAD and clean Git status before invocation, recorded binary/library hashes, and
ran `caffeinate -i` around the command. There were no builds in this follow-up.
`runs.json` records the command, source/harness commit, timestamp, host platform,
load averages, exit code, duration, and selection/output hashes. The captured host
was macOS ARM64; this is local application evidence, not Linux validation.

The CSV files preserve their captured bytes. Local root paths in `runs.json` and
the archival `capture.py` were replaced with `${FROZEN_ROOT}`,
`${FROZEN_ROOT_PREFIX}`, `${MAIN_ROOT}`, or `${EVIDENCE_DIR}` placeholders.
The archival driver requires those paths to be restored before execution.
Published metadata also adds each CSV output hash. These are the exact hash identities:

| Artifact | Original SHA-256 | Published SHA-256 |
|---|---|---|
| runs.json | `d0a9dd237ef8924837826b8dc1ccb608802949b3090273b6a6fb5c825edfa1cd` | `feacb37ef70a7a7d5c0fb55c6aef6d64d0d30044ace2ba0678618c5a1e7b0d77` |
| capture.py | `efc6686ed15b51bef697aa4d7a101aad89954e8b1ceb9f8df397f2015dc1627c` | `690a145bd78e4c6905b2c8e82d4f03b1a428df99ba2024654a517932fe673c76` |

Run `python3 -B docs/bench/m2c_tables.py` from the repository root to validate the
complete bundle and display both the original comparisons and this follow-up.
The reporter rejects partial bundles, a changed cell/repeat/engine inventory,
nonfinite values, changed exploration rows, mismatched hashes, and wrong provenance.
Its `--self-check` exercises temporary fixtures without producing benchmark evidence.

The original naive-recompute maintenance and combined-cost flags (1.061 and 1.062)
did not reproduce: their follow-up ratios are 1.027 and 1.028. The ivmlite follow-up
ratios are 1.016 and 1.042, both within the unchanged 1.05 threshold. A new
hand-written-trigger combined-cost flag remains visible: 0.170 to 0.179 ms, ratio
1.053 (an absolute increase of 0.009 ms). Its original ratio was 1.022; this is
not a persistent flag. No additional rerun was chosen to seek a passing result.
