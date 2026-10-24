# Lifecycle acceptance

This records the full five-section native driver, adapter, worker, connection and
lifecycle acceptance, plus eight runtime ownership checks. Local execution used
Node 24.22.1/SQLite 3.51.2 and the unchanged extension.

Lifecycle acceptance independently compares 69 complete base-row sets, 59
projection sets, 59 public grouped-view sets, 15 schemas and 20 complete main/TEMP
schema/data snapshots. It checks 27 context observations, 8 dependency refusals,
6 ownership refusals, 3 TEMP-shadow refusals and 2 real child-process SIGKILLs.
The seven focused mutation controls across the complete harness detect their
intended regressions and pass intact counterparts.

The uninstall path retains source rows and unrelated objects. A fresh bare pool
with no extension executes actual pinned repository writes/reads, and reinstall
backfills those changes. Compatible source migrations exercise added fields,
source rowid/key changes, nested rollback, callback/reinstall failure and repeated
rebuild. Unrelated maintained views over the source survive; foreign dependencies
and drifted ownership fail atomically. TEMP shadows are checked before work and
again before reinstall so the unchanged installer cannot read the wrong source.

For recovery, the parent kills a child after rebuild/refresh but before the outer
COMMIT, or after COMMIT while the native handle is still open. Captured SQL and
IPC readiness establish the commit boundary; exit must report SIGKILL. A fresh
connection verifies independently expected old/new schema and facts, then a valid
write and refresh. The scope is compatible schema rebuild and abrupt local
process death under SQLite's default rollback journal.

`summary.json` records counts and crash barriers. Compressed raw JSON/logs retain
captured SQL, schemas/recovery facts and mutation failures. The complete-snapshot
comparisons run in memory; JSON records their counts. `manifest.json`
binds script, compressed and exact uncompressed hashes/sizes; its base commit
precedes this integration, while source hashes identify the tested snapshot.
Reproduce with the guarded acceptance command in
[`scripts/kener-native/README.md`](../../../../scripts/kener-native/README.md).
All earlier evidence directories and measured adapter/benchmark source remain
unchanged. Deployment coordination, extension format upgrades and application
queue/cache/UI lifecycle remain separate host work.
