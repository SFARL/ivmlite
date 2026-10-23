# Native Kener repository harness

This optional, ivmlite-owned experiment executes unchanged Kener 4.1.8 source
at `5bc658b771d44f48b61a514cfdb5037468c5aa36`. It imports the actual
`MonitoringRepository` through scoped Node resolution hooks and native type
stripping. It does not run Kener's database singleton, endpoint/response builder,
Redis cache or UI. The smoke establishes driver compatibility and the indexed
repository baseline; it establishes no adapter correctness, performance benefit
or upstream adoption.

Use Node >=24.14, npm, Git, an extension built for the host, and a clean upstream
checkout at the pinned commit. Keep upstream, the npm runtime, evidence and
scratch databases outside the ivmlite checkout. Substitute absolute paths below;
the harness commits no local environment paths. The first npm run can download
or build the native better-sqlite3 addon (a compiler toolchain may be needed).

```sh
export PATH="/absolute/path/to/node/bin:$PATH"
node --version
git clone https://github.com/rajnandan1/kener.git /external/kener/upstream
git -C /external/kener/upstream checkout --detach 5bc658b771d44f48b61a514cfdb5037468c5aa36
node scripts/kener-native/prepare.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime
node scripts/kener-native/acceptance.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --extension /absolute/path/to/libivmlite_sqlite.dylib \
  --scratch /external/kener/scratch \
  --output /external/kener/driver-smoke.json
node scripts/kener-native/prepare.test.mjs \
  --upstream /external/kener/upstream \
  --runtime /external/kener/runtime \
  --output /external/kener/ownership-tests.json
```

Use the host's `.so`/`.dylib` extension path. `prepare.mjs` derives the exact
dependency closure of upstream's locked Knex 3.1.0, better-sqlite3 12.6.2 and
dotenv, preserving complete package records including nested dependencies,
integrity, resolved URLs and metadata. It uses `npm ci` in the owned external
runtime. Repeating preparation accepts an empty directory or a runtime with a
valid ownership marker, matching manifest and exact lock; other nonempty
directories are rejected before writing or installing. Repeating preparation
reinstalls that runtime; it never installs into
upstream. Peer database drivers are optional and are not required by SQLite.
The run checks installed package versions and rejects manifest/lock drift.

The acceptance command makes a fresh directory per run (`--scratch` is its
parent, defaulting to the OS temporary directory) and retains databases for
inspection. It runs five original migration `up()` functions, including the
initial migration's unrelated tables required by later migrations. Its evidence
contains source hashes, dependency/addon/extension identities, SQLite versions,
native schema/storage classes, original SQL and bindings, covering-index plans,
upsert results, failed extension loading and load-on-reopen checks. Both interval
methods are called without copying their SQL. Migrations swallowing duplicate
index errors are compensated by schema and actual plan assertions.

Run native preparation/acceptance serially. The JSON records failure and the
command exits nonzero if a check fails. The SQLite runtime uses one Knex pool
connection and loads `sqlite3_ivmlite_init` on every newly opened connection;
load failure closes the native handle and rejects connection creation. The
ownership regression command checks byte-preserving rejection of unowned and
corrupt directories, installs an empty runtime, then reinstalls the valid owned
runtime. `runtime.mjs` exports `loadRuntime`
and migration/covering-index constants for further independent replay tests.
