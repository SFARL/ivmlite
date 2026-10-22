# Selective-read mutation evidence

The frozen checkpoint passed **876 tests, zero failed and 3 ignored**, across 34 result binaries. All **17** separate mutations compiled, made their named primary guard red through the full `scripts/test-all.sh`, and restored source bytes. The final clean suite reproduced the baseline inventory. No enumerated query-space source changed. Exact mappings, all red sets, per-binary totals and complete normalized logs are bound below.

The campaign used the external controller retained in the local artifact directory and the byte-identical pinned runner. `replay.py` uses the same runner; its standalone native campaign has not been duplicated solely for publication. Python-only parser/preflight/package fixtures and runner cleanup mocks passed. Native source SHA bindings describe this feature's final code, independently of the historical dirty HEAD/diff snapshot in each run.

Two prior-checkpoint issues are retained. The first `drop-usable-check` registration expected the wrong guard: its full suite compiled with 874 passed / 2 failed, and actual IN/join guards reported `xBestIndex malfunction`. The primary registration was corrected and rerun. Later, the `estimated-rows-million` mutation stayed green at 876/0: an equality-only JOIN test missed the historical Cartesian ordinary-JOIN regression. That existing test was strengthened to assert SUM(v.n)=1000 and one actual nested read; focused diagnostics proved the mutant performs 1000 reads. The complete final campaign was then restarted from a new source freeze and baseline, rather than reusing earlier acceptance. Original mappings/results/logs are in `rejected-attempts/`, superseded source/results/logs are in `superseded-checkpoint-a/`, and focused diagnostics are separate from the unfiltered full-suite gates. Production and benchmark code did not change for the test fix, so preserved native benchmark measurements remain applicable.

[Native SQLite 3.45 smoke and alpha.6 database compatibility](compatibility/README.md), and [scalar affinity/source evidence](affinity/README.md), retain separate runtimes, exact coverage and identities.

Package layout:

```
manifest.json
mapping.json
accepted.jsonl
results.jsonl
frozen-source.json
verify.py
replay.py
audited_runner.py
logs/<run-id>.log.gz
```

Copy `audited_runner.py` byte-for-byte from
`docs/validation/m2c-mutations/reproduce.py` at the measured checkpoint.
The original accepted records and frozen source are retained. Published `results.jsonl` selects accepted raw records from the unchanged complete `attempts.jsonl` stream. Rejected attempts are retained separately.
For the 17-group campaign both JSONL files contain baseline, all mappings in
mapping order, then clean-restored. `accepted.jsonl` adds `binaries` to each raw
result. All raw result fields must agree exactly with the accepted record.

`manifest.json` schema (SHA strings below are placeholders):

```json
{
  "schema": 1,
  "mutation_groups": 17,
  "files": {
    "mapping.json": "SHA256",
    "accepted.jsonl": "SHA256",
    "results.jsonl": "SHA256",
    "frozen-source.json": "SHA256",
    "verify.py": "SHA256",
    "replay.py": "SHA256",
    "audited_runner.py": "SHA256"
  },
  "source_files": {
    "Cargo.toml": "SHA256",
    "Cargo.lock": "SHA256",
    "scripts/test-all.sh": "SHA256",
    "scripts/build-extension.sh": "SHA256",
    "crates/ivmlite-test/Cargo.toml": "SHA256",
    "crates/ivmlite-test/examples/selective_read_bench.rs": "SHA256"
  },
  "logs": {
    "baseline": {
      "path": "logs/baseline.log.gz",
      "raw_sha256": "SHA256 of original unmodified log",
      "normalized_sha256": "SHA256 of decompressed public log",
      "gzip_sha256": "SHA256 of public gzip bytes"
    }
  }
}
```

Enumerate the complete `source_files` set with `verify.source_paths(root)`;
the abbreviated example above omits most entries. It covers tracked and
nonignored new Rust files, every Cargo manifest/lockfile, `.cargo` files, and
both suite build scripts. The verifier requires every enumerated path and
checks all additional declared paths. It skips ignored build trees.
Manifest `files` may include additional documentation but must include the
seven named files. Do not include `manifest.json` itself. Manifest `logs` must
have exactly baseline + every mutation ID + clean-restored. Log paths may
also appear in `files`.

Normalize public logs by replacing the checkout's absolute prefix with
`${WORKTREE}` and remaining user-home prefixes with `${HOME}`. Document these
replacements with the published evidence. Retain each original raw SHA from
`result.log_sha256`; do not rewrite raw evidence metadata. Hash compressed and
decompressed normalized bytes separately. The raw SHA is provenance metadata;
the raw-to-normalized transformation cannot be independently rechecked from
the normalized log alone. Retain original logs privately for that audit.

Verify after packaging (read-only; Git inventory plus Python only):

```sh
python3 -B docs/validation/selective-reads/verify.py /absolute/path/to/ivm
```

To verify a package outside the checkout:

```sh
python3 -B /absolute/path/to/package/verify.py /absolute/path/to/ivm
```

Replay all 17 groups, or specify one or more mapping IDs:

```sh
python3 -B docs/validation/selective-reads/replay.py /absolute/path/to/ivm /absolute/path/to/new-output
python3 -B docs/validation/selective-reads/replay.py /absolute/path/to/ivm /absolute/path/to/new-output ge-becomes-gt
```

Output must be outside the checkout and evidence package, and new or empty.
Each replay includes a full green baseline and full restored suite around
the selected mutants. Unknown IDs, source hashes, package hashes, and exact
matches for all edits are checked before output creation or source writes.
Replay uses the pinned runner's byte-restoring `finally`, with SIGINT/SIGTERM
translated to interruption. Cleanup sends SIGTERM to the process group and
escalates to SIGKILL if the direct process fails to exit. It waits for that
direct process; it does not independently prove that every descendant exited.
A descendant that ignores SIGTERM may survive after the direct process exits.
Inspect for surviving processes before resuming an interrupted replay. These
interruption paths were exercised with Python mocks, not native jobs.
Unrelated dirty
tracked/untracked file contents are snapshotted and checked across each run.
It does not require the historical HEAD or a clean checkout. As with any
process, SIGKILL or machine loss cannot execute Python cleanup.

Acceptance checks include every binary's passed+failed count, unchanged
ignored/measured counts, no filtering, compiler validity, exact primary red
test names, baseline/restored equality, edit/original-source hashes, and
recorded restoration. Historical diff/untracked snapshots are compared only
within the recorded campaign; current source bytes are checked independently.
The historical whole-ledger hash is not a dependency.

Focused diagnostic logs and the test-only patch are stored as gzip archives.
`test-gap-diagnostic/storage.json` binds compressed hashes to the exact
decompressed publication bytes; no diagnostic whitespace was removed.
