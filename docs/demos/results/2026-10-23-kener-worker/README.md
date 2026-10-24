# Worker transaction acceptance

This is new correctness evidence for the Kener host entry point and actual
upstream AsyncLocalStorage context. The serial acceptance command runs driver
smoke, independent adapter replay, then worker acceptance. Local execution used
Node 24.22.1 (ABI 137), SQLite 3.51.2 and the unchanged release extension.

`summary.json` records assertions and limits. `acceptance.json.gz` retains the
complete raw JSON, including emitted SQL/native connection identities, context
checks, failure paths, red/green mutation results and upstream/runtime identities.
`manifest.json` binds compressed and uncompressed bytes and the tested script
snapshot with SHA-256. Its base commit precedes this integration; script hashes
identify the tested source. Reproduction uses the acceptance command in
[`scripts/kener-native/README.md`](../../../../scripts/kener-native/README.md).

The original October 22 source-scale timings remain bound to commit `3a17bc5`
and their original manifests. No performance claim is added for the worker
entry point. Application singleton, queue startup/shutdown, endpoint, cache and
UI execution remain outside this evidence. The existing native CI entry point
also runs the worker checks on Linux/macOS with Node 24.19.0.
