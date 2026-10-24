# Selective group-key reads

The native Rust/rusqlite example loads an explicitly selected ivmlite library
and checks the lifecycle of a file-backed maintained view. It uses the existing
host dependencies; it does not provide an SDK or establish support for other
language drivers.

From the repository root, build the debug extension and run the example with
an absolute library path:

```bash
scripts/build-extension.sh
cargo run --locked -p ivmlite-test --example selective_read_driver -- "$PWD/crates/ivmlite-sqlite/target/debug/libivmlite_sqlite.dylib"
```

On Linux, use `libivmlite_sqlite.so` at the same location. The executable requires
exactly one absolute library path and loads its `sqlite3_ivmlite_init` entry
point through `open_with_extension_at`. Its output records the selected library
path and the host SQLite version. To compare library compatibility, run this
same command separately with each preserved library path.

The example creates a temporary database, seeds three groups and creates a
maintained aggregate view. Within a transaction, a write leaves the existing
view rows stale until an explicit command refreshes them:

```sql
INSERT INTO roll(roll) VALUES ('refresh');
```

It reads the maintained rows with bound equality and range parameters:

```sql
SELECT monitor_id, bucket_ts, total FROM roll
WHERE monitor_id = ?1 AND bucket_ts >= ?2 AND bucket_ts < ?3
ORDER BY monitor_id, bucket_ts;
```

For parameters `(7, 10, 11)`, the exact row starts at `(7, 10, 5)`, stays stale
after inserting a contribution of 3, and becomes `(7, 10, 8)` after refresh.
After commit, a second transaction updates both contributions to 100, observes
the stale total 8, refreshes to 200, and rolls back. The view and base rows return
to the committed state, including after closing and reopening the database with
the same library. The example asserts all full-view rows at every read stage
and removes the temporary directory, database and journal files on exit.

The same lifecycle function runs as an example test against the rebuilt debug
library:

```bash
scripts/build-extension.sh
cargo test --locked -p ivmlite-test --example selective_read_driver
```

## Measured source-scale result

On local macOS ARM64, the same release executable loaded preserved alpha.6
and new release libraries in separate serial processes. Each library ran five
fresh 4,100,000-row fixtures, with 550,000 initial groups, one warmup and ten
measured batches of 500 mixed mutations per fixture. The synthetic Kener-shaped
fixture has **INTEGER monitor ids**. These are application-shape measurements,
not a production deployment or adoption result.

Milliseconds below are the median of five per-repeat medians; brackets give
the minimum and maximum of those five medians. End-to-end values sum parts
from the same round, including BEGIN, write, refresh, one read and COMMIT;
they exclude the other reads actually executed by the harness.

| Operation | Alpha.6 ms [min–max] | New source ms [min–max] |
| --- | ---: | ---: |
| Full read | 192.086 [188.698–193.009] | 190.798 [190.147–191.619] |
| Monitor 123 | 142.200 [141.268–143.456] | 1.086 [1.057–1.102] |
| Monitor 123 + bucket window | 142.641 [140.669–144.503] | 0.132 [0.127–0.150] |
| Batch write | 4.177 [4.125–4.245] | 4.234 [4.212–4.325] |
| Refresh | 8.856 [8.782–9.231] | 8.923 [8.718–9.619] |
| Modeled transaction + monitor read | 155.964 [154.203–157.439] | 14.664 [14.300–15.580] |
| Modeled transaction + window read | 155.786 [154.394–159.064] | 13.714 [13.428–14.734] |

Full-read, write and refresh ranges overlap. The selective reads materially
reduce the time spent reconstructing/returning the maintained snapshot.
These in-memory MEMORY-journal, synchronous-OFF results do not measure durable
file commits. All scheduled independent oracle checks passed. Corresponding returned row
counts and recorded page allocations agreed across libraries and modes. The final database
allocation was 1,803,042,816 bytes for both; this is page allocation, not process
RSS or a smaller persisted maintained state.

Separate work-only processes confirmed the mechanism. Ranges below cover the
ten measured rounds; both libraries returned identical outer rows.

| Read | Alpha.6 candidates | New candidates | Alpha.6 inner fullscan steps | New inner fullscan steps |
| --- | ---: | ---: | ---: | ---: |
| Full | 550,150–550,828 | 550,150–550,828 | 550,149–550,827 | 550,149–550,827 |
| Monitor | 550,150–550,828 | 2,558–2,562 | 550,149–550,827 | 0 |
| Monitor + window | 550,150–550,828 | 10–13 | 550,149–550,827 | 0 |
| Non-leading bucket window | 550,150–550,828 | 2,270–2,813 | 550,149–550,827 | 550,149–550,827 |

Captured EQP shows the existing output index's leading monitor seek and a
bounded TEXT-region branch. For monitor + window, only the observed lower
bucket bound appears in the numeric branch's index access; the remaining
candidate predicates still run. Zero fullscan steps does not imply exactly
one VM step per returned row: the new window read took 15,437–15,482 inner VM
steps for 10–13 candidates. No complete composite-range-seek claim is made.

Usable BINARY group-key comparisons `=`, `>`, `>=`, `<`, `<=` can narrow the
candidate snapshot. SQLite retains the original predicates, ORDER BY and LIMIT;
non-BINARY collations, aggregate-only predicates and expression keys use scan
fallbacks. TEXT arguments and non-leading-only constraints can still scan.
Reads retain owned snapshot, transaction and explicit-refresh behavior. Format
6 is unchanged and a database created by alpha.6 was reopened by the new
library. This feature is source-only and is absent from the published alpha.6
archive.

[Raw rounds and work counters](results/2026-10-21-selective-reads/),
[derived summary](results/2026-10-21-selective-reads/summary.json) and
[mutation/compatibility evidence](../validation/selective-reads/README.md)
retain reproduction and identities. The measured host SHA256 is
`59faaddda67668261b60b3651523a6d992391fe8f74b6e2b3c8798da11f0cf6f`;
old/new library SHA256 values are
`e777d54cd237a669376c8164188fe667bca7072052ba33e6d62a266f13dd1e77` and
`4bc5f14e81f786f3d3644fffbc9d8fc8c44ac0d768fc4eebc541312cbb589f20`.
The baseline was built at `4888480`, whose only difference from alpha.6 commit
`0ad47a9` is a proposal document; extension source is identical. Compiler was
Rust 1.95.0 and the host SQLite was 3.53.2. A separate native SQLite 3.45.0
smoke passed 74 comparisons/lifecycle checks and four alpha.6 reopen checks.
No new-branch Linux validation is claimed by this local result.

## Benchmark reproduction

The separate `selective_read_bench` example accepts exactly an absolute library
path, `latency` or `work`, and optional positive base-row and repeat counts.
Defaults are 100,000 base rows and one fresh repeat. Invalid modes, extra
arguments, overflowing ids/counts and overlapping Kener mutation ranges are
rejected before fixture setup. Build the host once, preserve that executable,
and run it serially against each preserved release library:

```bash
cargo build --release --locked -p ivmlite-test --example selective_read_bench
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library latency 100000 1 > before-quick.json 2> before-quick.stderr
target/release/examples/selective_read_bench /absolute/path/to/new-library latency 100000 1 > after-quick.json 2> after-quick.stderr
# Continue only after both quick runs validate successfully.
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library latency 4100000 5 > before-latency.json 2> before-latency.stderr
target/release/examples/selective_read_bench /absolute/path/to/new-library latency 4100000 5 > after-latency.json 2> after-latency.stderr
target/release/examples/selective_read_bench /absolute/path/to/alpha6-library work 4100000 1 > before-work.json 2> before-work.stderr
target/release/examples/selective_read_bench /absolute/path/to/new-library work 4100000 1 > after-work.json 2> after-work.stderr
```

Acceptance uses the existing deterministic `demand_cases::KENER` generator:
4,100,000 base rows produce 550,000 initial groups; each fresh repeat applies
one warmup and ten measured batches of 500 mixed mutations. Each repeat resets
the schedule to round zero. The in-memory connection explicitly sets
`journal_mode=MEMORY`, `synchronous=OFF`, `temp_store=MEMORY`,
`cache_size=-2000`, `foreign_keys=OFF`, and `recursive_triggers=OFF`. These
settings describe an ephemeral fixture and do not measure durable file commits.
Actual journal, synchronous and temporary-store settings are recorded alongside
page count, page size and their product before/after bootstrap and after the
final round. Allocated SQLite pages do not describe process RSS or a reduction
in persistent state.

Seeding, preparation, parameter binding and oracle checks are outside every
round timer. View creation has its own `bootstrap_ms`. All mutation, refresh,
transaction and read statements are prepared before the rounds. A round uses
one transaction: prepared `BEGIN`, Kener's prepared mutation statements,
explicit refresh, all reads, then prepared `COMMIT`. `PreparedBatch::execute`
is deliberately bypassed because it owns its own transaction. Latency mode
rotates full, monitor 123, and monitor 123 with bucket range `[0,9000)` reads.
Every read drains all rows, calls `get_ref` on all seven columns and passes
each value through `black_box`; it does not retain a large host row vector.

Each repeat also seeds an independent ordinary SQLite connection without a
maintained view. After each actual commit, the same deterministic mutations
are replayed there outside timers. Initial, after-warmup and final checks
compare ordered base rows by storage class and exact REAL bits with bounded
host memory, then compare full maintained rows to the reference base `GROUP BY`
as sorted multisets. Selected reference `GROUP BY` multisets are checked after
every round by reexecuting the actual prepared statements with their actual
retained bindings. Their expected filters are written separately from the timed
SQL. Each timed read's returned count is checked against the reference result
length; full counts are checked every round, with full multisets at the three
checkpoints. This detects a skipped actual mutation even if the actual base
and maintained view still agree. Oracle sorting and reference replay may
allocate memory or influence later cache conditions; both library runs use the
same protocol.

Stdout is one JSON document. Each repeat retains its initial group count,
allocations, all raw round/repeat indices, warmup marker, read order and
returned group counts. Latency `timings_ms` separates begin, write, refresh,
each read and commit. `write_refresh_read_commit_SHAPE` and
`end_to_end_SHAPE` sum that same round's parts; the latter includes begin.
They model one selected read and exclude the other reads that were also
executed in the round. Per-repeat `measured_round_summary_ms` gives median,
minimum and maximum after excluding warmup. Compare the medians across the
five fresh repeats; do not add independently computed step medians.

Work mode has no timing or latency-summary fields. It repeats the same
fixture/mutation schedule, adds a non-leading bucket-range query, and records
actual nested SQL, expanded SQL captured at PROFILE, VM steps, fullscan steps,
nested candidate-row events, outer returned rows and outer counters. After the
trace is removed it runs EQP on the captured expanded inner SQL. It then runs
a separate fully drained progress-only pass, recording the number of callbacks
at a 100-VM-instruction interval. Progress callbacks cover the whole query;
they are not nested-statement VM counters or timing results. SQLite statement
counters are signed 32-bit and can overflow for sufficiently large queries.

Metadata records exact requested/canonical library paths, executable and
arguments, SQLite version/source id/compile options, platform, observed source
HEAD and observed `rustc --version --verbose`. Observed source/compiler state
does not identify the bits of an already built artifact: separately retain
build commands and SHA256 hashes for the unchanged executable, harness source
and both libraries. Preserve complete stdout/stderr and run builds, tests and
benchmarks serially. The completed serial measurements above use these same settings and workload.
Published path strings and UTF-8 OS-argument byte arrays replace local prefixes
with `${WORKTREE}`, `${ARTIFACT_ROOT}` and `${HOME}`; original and normalized
hashes are distinguished in `origin-manifest.json`.

Focused tests include a native 20,000-row fixture smoke through the actual
runner in both modes. Rebuild the debug extension before running them:

```bash
scripts/build-extension.sh
cargo test --locked -p ivmlite-test --example selective_read_bench
```
