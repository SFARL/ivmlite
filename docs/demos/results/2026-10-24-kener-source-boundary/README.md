# Kener response/cache source-boundary acceptance

Local macOS ARM64 evidence from Node 24.22.1 / SQLite 3.51.2, against unchanged
Kener commit `5bc658b771d44f48b61a514cfdb5037468c5aa36`. This extends the native
repository experiment through actual queue wrapper, response processor, monitor
controller, cache and Redis connector source. BullMQ classes, Redis transport,
alert enqueue, DB singleton, controller barrel, dotenv and unused helpers are
explicit fixture boundaries. This is not real broker/application acceptance.

All six guarded sections passed serially, as did eight exact-runtime ownership
checks. The new section records 128 assertions: 14 independent base comparisons,
13 public-view comparisons and six successful publication snapshots, plus three
grouped comparisons, five isolation checks and three resource cleanup checks.
These are overlapping counters, not 128 additional independent scenarios. One
expected negative-control view comparison fails before incrementing its success
counter. Every completed publication snapshot contains intended and actual base
and view facts from a different native pool, with per-insert ordering events.

The source-boundary control commits the insert while deliberately skipping
refresh. The independent publication-view assertion detects the stale view;
its intact counterpart passes. Together with the earlier sections, eight focused
controls detect their intended regressions and all intact counterparts pass.
These are host/trigger controls, separate from the Rust core mutation ledger.

Verified source semantics include native refresh rollback before publication,
postcommit cache failure with committed facts, and idempotent manual replay.
Actual grouped-cache results remain stale after correction until 60 logical
seconds elapse. Latest-status TTL is 86400 seconds; an older processed timestamp
overwrites that cache. Response completion does not await pending alert enqueue.
Actual shutdown closes the worker only, followed by explicit fixture cleanup.
Captured retry/backoff/dedup/retention options prove configuration only.

`summary.json` gives counts, scope and runtime identities. `acceptance.json.gz`
retains the complete six-section JSON, SQL, publication facts and all four
namespaced profiles. Each profile records six real upstream module hashes,
canonical worker-context provenance, and the exact contents and hashes of its
14 generated facade modules. The generated bridge points to controlled classes
implemented in `source-boundary.mjs`, whose hash is also recorded. Logs and the
eight ownership results are archived separately. `manifest.json` binds all six
archives' compressed and exact raw bytes and the 17 script/workflow hashes.

The source snapshot is the tested working tree after base commit `776a37c`.
Existing adapter, runtime, host, lifecycle, benchmark and preparation code, and
all historical evidence remain byte-identical to that base. New acceptance
wiring adds only the awaited sixth section and combined evidence reporting.
The previous source-scale timings do not measure this path, real queueing,
cache/HTTP latency or end-user performance. Real broker retries/dedup, Redis
expiry/network behavior, alert delivery, application startup and deployment
coordination remain follow-up work.

Reproduce with the existing guarded `acceptance.mjs` command in
[`scripts/kener-native/README.md`](../../../../scripts/kener-native/README.md).
Decode exact raw bytes with:

```sh
gzip -dc docs/demos/results/2026-10-24-kener-source-boundary/acceptance.json.gz > /external/kener/acceptance-decoded.json
```
