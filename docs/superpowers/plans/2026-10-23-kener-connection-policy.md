# Kener connection policy implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development to implement this plan with spec and quality review.

**Goal:** Reject an unexpected extension artifact and explicitly configure/test
SQLite busy timeout and independent writers through the existing native host.

**Architecture:** Extend the local runtime's `createDatabase` with an optional
fourth policy argument, preserving its existing callers. Configure every new
native handle in `afterCreate`, before returning it to the pool. Add native
connection acceptance and a child-process lock holder so the synchronous driver
can wait while another process commits. The host does not retry work callbacks.

**Tech stack:** Node >=24.14, pinned Knex/better-sqlite3, existing ivmlite release
extension, actual unchanged Kener repository/context, SQLite rollback journal.

## Contract and files

- `scripts/kener-native/runtime.mjs`: explicit policy with
  `expectedExtensionSHA256` (optional lowercase 64-digit SHA-256) and
  `busyTimeoutMs` (nonnegative integer <= 2147483647, default 5000). Reject bad
  policy, non-object inputs and unknown keys before allocating a pool. Check expected file identity before loading,
  load the extension, set and verify `PRAGMA busy_timeout`, then hand off the
  connection. Close native handles after any initialization failure; preserve
  existing lifecycle events and add failure stage/observed policy as needed.
- `scripts/kener-native/connection-acceptance.mjs`: independent native facts,
  projection and public grouped-view assertions; lifecycle/identity observations.
- `scripts/kener-native/lock-holder.mjs`: bounded child-process fixture running
  actual repository/host work, signals after obtaining a write lock; supports
  explicit release and timed release on its own event loop. Always clean up
  processes and handles, including failures and unexpected exits.
- `scripts/kener-native/acceptance.mjs`: run connection acceptance serially after
  existing driver/adapter/worker checks; current native CI inherits the checks.
- README, report and new result directory: usage, source/byte hashes, precise
  scope and historical benchmark separation.

The artifact pin is exact file identity, not a semantic-version/ABI handshake or
database format migration. Without a supplied expected hash, there is no version
pin. Deployment must keep the extension file immutable while opening connections.
No Rust/public SQL changes, implicit retries, upstream edits, journal tuning or
changes to the historical adapter/benchmark source belong in this task.

## Tasks

- [x] Add policy and focused failing/green regression tests. Verify invalid
  policy rejection, valid pin on initial open/reopen, mismatched pin rejection
  before loading/writes and native handle closure, per-handle timeout and
  zero timeout. Keep existing missing-extension cleanup acceptance passing.
- [x] Add independent writer tests on one database: held-lock timeout preserves
  base/projection/view state and restores worker context; after release caller
  explicitly retries a new transaction and commits; timed child commit allows
  a waiting writer to succeed. Warm pools before contention and use child
  readiness messages, not startup sleeps. Verify actual SQLITE_BUSY, callback
  counts, independent expected rows and clean subsequent refresh/reopen.
- [x] Complete independent spec and quality review, fix findings, run complete
  serial native acceptance and archive exact evidence with new source hashes.
- [x] Update docs, commit specific files, push PR #7, and inspect latest-head
  Linux/macOS core/native CI. No merge, release or tag.

Local completion: all four serial acceptance sections passed on Node 24.22.1 /
SQLite 3.51.2. Connection checks cover 26 invalid policies, 11 native timeout
observations, 13 comparisons per state layer, 10 context checks and 28 native SQL
connection identity assertions. Six focused controls across the whole harness
detect their intended mutants and pass intact counterparts. Independent spec,
quality and final integrated reviews approved the implementation and evidence.
Only the unified guarded acceptance CLI writes evidence; the connection runner
is an exported function. Historical archives and measured adapter/benchmark code
remain unchanged.

Implementation source was pushed as `dc2417d`. The final checkpoint removes an
extra EOF blank line and refreshes the evidence from another complete guarded
acceptance run, preserving exact source/byte provenance. The current
[PR #7 checks](https://github.com/SFARL/ivmlite/pull/7/checks) track both workflows
for each pushed revision. Source/runtime semantics match the independently
reviewed implementation; no release, tag or upstream application change is made.

## Validation commands

Use the bundled Node binary (currently 24.22.1 locally; CI 24.19.0). Upstream,
owned runtime, scratch and output stay outside checkouts.

```sh
node --check scripts/kener-native/runtime.mjs
node --check scripts/kener-native/connection-acceptance.mjs
node --check scripts/kener-native/lock-holder.mjs
node scripts/kener-native/acceptance.mjs --upstream /external/kener/upstream --runtime /external/kener/runtime --extension /absolute/path/libivmlite_sqlite.dylib --scratch /external/kener/connection-policy --output /external/kener/connection-policy/acceptance.json
git diff --check
```

Expected: all four serial acceptance sections pass. Native build/test ownership
stays with one agent. Historical October 22 and worker-only October 23 archives
remain unchanged and continue to identify their original source snapshots.
