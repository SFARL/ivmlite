# M2c mutation evidence

This directory records source-specific correctness experiments. A mutation is a temporary implementation change, followed by the complete `scripts/test-all.sh --release` run and restoration of the original files. These measurements are not performance comparisons or proof that a released archive was mutation-tested.

The [mapping](mapping.json) defines each mutation's exact edits, expected occurrence counts, source commit, original file hashes and primary guards. [Results](results.jsonl) contain the full observed red set, per-binary outcomes and actual totals for each accepted mutation. [Attempts](attempts.jsonl) additionally preserve clean baselines and rejected attempts. [The manifest](manifest.json) binds the files and distinguishes source inventories; [the CSV](ledger-map.csv) connects mutation IDs to their original ledger anchors.

The inventories are distinct:

- `d5dd2b9`: 830 tests, 28 result binaries; Task11 and supplemental semantic gates.
- `84e4c07`: 836 tests, 29 result binaries; Task12 order independence and retained-state gates.
- `b8207a8`: 839 tests, 29 result binaries; Task13 REAL demo gates.
- `aba0fd7`: 841 tests, 30 result binaries; Task14 diagnostics and native feature smoke gates.

Counts are summed over both cargo invocations made by the script, including zero-test documentation binaries. The script rebuilds the native extension in its dev profile; `--release` selects the workspace harness profile. A result's inventory belongs to its recorded source snapshot. The later 841-green production check does not change earlier mutation totals.

To reproduce one mutation in a clean checkout at its recorded commit:

```sh
python3 /path/to/evidence/reproduce.py \
  /path/to/clean-checkout \
  /path/to/evidence/mapping.json \
  /path/to/new-results \
  row416 --release
```

Use a separate checkout and keep the runner, mapping/evidence and output directory outside it. The runner rejects unknown IDs before touching the checkout, requires tracked source to be clean with no untracked files, and verifies the source commit and original file hashes. It applies all edits in memory before writing, runs the full script and restores files in `finally`. It does not switch commits for the caller. Inspect the completed log, compile status, full binary inventory, primary failures and restoration result before accepting a reproduction. Run the actual script, not a bare cargo test that could load a stale extension. A nonzero mutation exit is expected only because the intended tests fail.

Validate the published evidence without running native tests:

```sh
python3 docs/validation/m2c-mutations/verify.py
```

The first `row390` attempt produced invalid Rust after a partial expression replacement. It is retained as `rejected_compile` with its exact original mapping and complete terminal output. Only the corrected complete-expression rerun is accepted. Neither a compilation error nor its 21 passing extension unit tests verifies the intended batch-invariance guard.

`row407`, `row407_naive_half` and `row407_sql_half` are separate experiments. The shared bug leaves naive-only join sweeps green because the naive reference and SQL oracle agree on the wrong key. Each half alone exposes different disagreements. Do not use one variant's counts or red set as another variant's evidence.

Removing `float_roundtrip` requires editing both workspace users because Cargo unifies features. Its observed primary failure is difficult finite mantissas, with a second failure in the m2a fixture round trip. The basic finite-format and signed-infinity tests remain green under that mutation; they guard different contracts.

Full terminal output is stored as deterministic gzip files under `logs/`. Public copies replace local checkout prefixes with `${WORKTREE}` and preserve test names, source lines, failure messages and numeric values. `raw_log_sha256` identifies the untouched original log; `published_log_sha256` identifies normalized decompressed bytes; `published_log_gzip_sha256` identifies the published compressed file. These hashes intentionally differ. The original logs were not rewritten. The manifest documents normalization and checksums the published metadata and reproduction tools.

Elapsed wall times are recorded per run, including build and contention. They are not benchmark samples. Earlier 1.5-second and 40-second small-suite timings are historical; the enlarged sweeps can take minutes. No new complexity timing experiment is claimed for historical n/a rows.
