# M2c original application capture provenance

This bundle publishes the existing Task 13 native macOS capture; it does not
run or replace a measurement. [Machine metadata](machine.json),
[run records](runs.jsonl), and the [progress log](application-progress-2.log)
record the host, commands, timestamps, load averages, exit codes and durations.
The machine is an Apple M2 Pro (10 logical cores), 16 GiB RAM, macOS 27.0.1,
Rust 1.95.0, with SQLite 3.53.2 bundled by the harness. The original machine
file's pending SQLite field is resolved here using the actual `# sqlite:`
lines in all four raw demo outputs and the scoped build CSVs; its original
byte hash remains in [the hash manifest](hashes.json).

Dates come from UTC run records, interpreted in America/New_York (EDT):

| Capture | UTC interval | Local date and time | Outcome |
|---|---|---|---|
| ablation and write amplification | Oct 19 22:26:29–23:35:02 | Oct 19 18:26:29–19:35:02 | success |
| initial confirmation before | Oct 19 23:35:03 | Oct 19 19:35:03 | exit 101; retained failed attempt |
| recovered confirmation before | Oct 20 12:54:33–12:55:50 | Oct 20 08:54:33–08:55:50 | success |
| confirmation after | Oct 20 12:55:50–12:57:00 | Oct 20 08:55:50–08:57:00 | success |
| eight unchanged integer demos | Oct 20 12:57:00–13:41:27 | Oct 20 08:57:00–09:41:27 | success |
| FluxFlow before | Oct 20 13:41:27–13:43:55 | Oct 20 09:41:27–09:43:55 | success |
| FluxFlow after | Oct 20 13:43:55–13:46:30 | Oct 20 09:43:55–09:46:30 | success |
| Kener before | Oct 20 13:46:30–13:50:05 | Oct 20 09:46:30–09:50:05 | success |
| Kener after | Oct 20 13:50:05–13:56:23 | Oct 20 09:50:05–09:56:23 | success |

The combined raw demo files are named for the actual October 20 capture,
correcting their former October 19 filenames:
[FluxFlow](../../demos/results/2026-10-20-fluxflow-m2c.txt) and
[Kener](../../demos/results/2026-10-20-kener-m2c.txt). Their four constituents
are also published here: [FluxFlow before](fluxflow-before.txt),
[FluxFlow after](fluxflow-after.txt), [Kener before](kener-before.txt),
[Kener after](kener-after.txt). Each original constituent hash is retained in
[raw capture provenance](raw-capture-provenance.json) and the hash manifest.
The combined files retain their captured CSV fields and raw hash annotations.

The frozen sources are before `b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa`
and after `84e4c07c2ebfc9db8b6bafb57378627414df5524`.
The common clean harness is `aba0fd730ddd52e2a3700ff6381a17c40e2bdf29` for
ablation, write amplification and the eight unchanged demos. Confirmation
uses each frozen source's own clean harness. Changed FluxFlow/Kener before
captures use their original integer `b647bcb` harness; their after captures
use the new `aba0fd7` harness. The
[benchmark build CSV](../m2c-ablation-builds.csv) and
[demo build CSV](../m2c-demos-builds.csv) record these per-scope distinctions.
Raw `--extension` output reports that its generic staleness/version check was
skipped; source identities are established by these explicit frozen build
records rather than inferred from a harness package version.

## Normalization and original identities

The original capture remains the input to [normalize.py](normalize.py).
Its published text replaces only recorded local root paths, except for the
explicit machine SQLite correction above. The placeholders are:

| Placeholder | Meaning |
|---|---|
| `${BEFORE_TREE}` | frozen before source, harness and extension checkout |
| `${AFTER_EXTENSION_TREE}` | frozen after extension checkout |
| `${AFTER_HARNESS_TREE}` | shared current harness checkout |
| `${CAPTURE_ROOT}` | original measurement capture directory |

Restore local roots to replay commands. The driver only normalizes and
hashes existing files; it performs no builds or native benchmark runs.
To regenerate from an available original capture directory:

```sh
python3 -B docs/bench/m2c-capture/normalize.py --source /path/to/original/capture
```

[hashes.json](hashes.json) distinguishes `original_sha256` from
`published_sha256` for every normalized artifact, and records unchanged CSV
and combined-demo byte hashes. The original raw demo identities are:

| Capture | Original SHA-256 |
|---|---|
| FluxFlow before | `c72300df17720f6dfd6fcfdf6e0ac87f7e369018a0847cbe71fbe9297b3de04b` |
| FluxFlow after | `5c957cac9d6b2f6db514ecae43593f40ee78a3ba121aacbf891de209751374a3` |
| Kener before | `1ffc16fe26baffe64514183c8a353154fb601ec5e39849158ce002b773c11e89` |
| Kener after | `b359cbae550ab5b25e419deecb30d50797850be88be3be150a59e476dae1525a` |

The failed October 19 confirmation remains historical provenance; it is not a
complete measured cell or one of the successful confirmation rows. The
[single flag follow-up](../m2c-confirm-rerun/README.md) is a separate later
capture with its own run and binary/library hashes. Original observations
are never replaced by follow-up medians. The
[benchmark summary](../README.md#m2c-column-types-and-aggregates) and
[complete generated report](../m2c-report.md) retain the original two flags
and the new trigger-control flag.
