# Connection policy and independent writer acceptance

The guarded `acceptance.mjs` CLI runs driver, adapter, worker and connection
acceptance serially. This local run used Node 24.22.1/SQLite 3.51.2 and the
unchanged extension. It adds 26 invalid-policy checks, 11 native timeout checks,
13 comparisons per state layer, 10 context checks and 28 SQL connection identity
checks. The existing 5,362 exact repository comparisons and all six focused
mutants pass their intended red/green controls.

Two independent processes share one SQLite database. An IPC-controlled writer
holds a real write lock; another gets `SQLITE_BUSY`, rolls back, then explicitly
retries after release. In the second case the child arms its own release timer
when the parent submits its actual write, allowing the synchronous waiting write
to succeed. Same-key corrections verify state after ownership switches.
Independent base/projection/public-view facts, context restoration, native reopen
and clean subsequent refresh are checked. SQLite keeps its default rollback
journal; these checks do not establish WAL behavior or parallel writer commits.

Artifact checks include mismatch/missing-file handle cleanup and replacing an
external copied artifact between native opens. The exact expected hash in
positive controls is derived from the supplied file; a deployment must instead
obtain its approved hash from its trusted artifact manifest and use an immutable
artifact path for the process lifetime. This is file identity, not a version/ABI
handshake, loaded-image attestation or database format migration.

`summary.json` records counts, limits and diagnostic elapsed times.
`acceptance.json.gz` retains full raw evidence, including child messages/SQL,
lifecycle stages and precise mutation failures; stdout/stderr are also archived.
`manifest.json` binds source, compressed and uncompressed byte hashes and sizes.
The base commit precedes this integration; source hashes identify the tested
snapshot. Reproduce using the full acceptance command in
[`scripts/kener-native/README.md`](../../../../scripts/kener-native/README.md).
Historical benchmark and worker-only archives remain unchanged. No new
performance result or application singleton/queue/cache/UI execution is claimed.
