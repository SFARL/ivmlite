# Kener response and cache source-boundary implementation plan

> **For agentic workers:** Use subagent-driven implementation with independent spec and quality review.

**Goal:** Execute the pinned response/controller/cache source path against native
SQLite and expose commit/publication, retry and cached-freshness semantics before
expanding into a real broker/application deployment.

**Architecture:** A separate scoped source loader imports unchanged upstream
`q.ts`, `monitorResponseQueue.ts`, selected `monitorsController.ts` exports and
the cache/Redis connector modules. Explicit facades replace the app DB singleton,
controller barrel, unrelated service imports, BullMQ driver, Redis transport and
alert sink. Fail closed on unused paths. Namespace imported app modules per run
to prevent cached singleton/facade leakage; reuse the actual canonical repository
worker context from the existing native runtime. No source extraction or edits.

**Scope:** This is source-boundary integration with controlled transports and
manual replay. It does not establish BullMQ scheduling/retries/deduplication,
Redis server TTL/network behavior, alerts delivered, full singleton/app startup,
or endpoint performance. Preserve all existing source/evidence and pinned deps.

## Files and contracts

- New `scripts/kener-native/source-boundary.mjs`: a loader/facade factory. Record
  every real upstream module/hash, facade/injection and transport boundary.
  Resolve only the reviewed app import graph and reject unexpected paths. Keep
  app modules distinct per invocation, restore hooks/global fixture state after
  close, and close every controlled queue/worker/cache boundary.
- The DB facade's `insertMonitoringData` wraps only the repository insert in
  `kenerTransaction`; its promise resolves after refresh and COMMIT. Wrapping the
  whole response processor would publish cache before commit and is forbidden.
  Reads preserve strict default; experimental numeric reads remain explicit and
  tested only with an isolated fresh cache profile and existing numeric limits.
- Controlled BullMQ classes capture the actual q.ts options and actual wrapped
  processor. `push` executes upstream queue construction/enqueue logic; the test
  explicitly invokes the recorded processor once or manually replays. Do not
  implement simulated automatic retries/dedup and call them broker acceptance.
- The actual Redis connector sees an injected IORedis class with explicit GET,
  SET/EX and DEL wire behavior, JSON payloads, logical TTL clock and injectable
  command failure. Clock advancement proves source caching policy, not a server
  wall-clock/expiry implementation. Unknown commands/unused app helpers throw.
- New `source-boundary-acceptance.mjs`, exported runner only, becomes the sixth
  serial section of guarded `acceptance.mjs`. All scratch/facade/evidence files
  remain outside both repositories. Existing adapter/benchmark/runtime/host/
  lifecycle/prepare code and their historical evidence remain byte-identical.

## Acceptance

- [x] Successful actual response push/processor: repository upsert, refresh and
  native outer COMMIT precede Redis last-status publication and alert invocation.
  A different native pool observes independently expected base and public view
  facts at publication, avoiding a same-transaction oracle.
- [x] Controller input failure and real native refresh failure roll back with
  no latest-status cache write/alert invocation. Cache-command failure after a
  successful commit rejects the processor but leaves correct committed base/view;
  manual replay is an idempotent upsert with one base row and correct aggregate.
- [x] Warm actual grouped controller cache, process a correction, and observe
  the same cached result until logical 60-second expiry. Then compare the miss
  result with an independent native oracle and maintained reader. Verify actual
  key construction (sorted copied tags, duplicate preservation and other window
  dimensions), wire prefix, JSON, 60/86400 TTL and malformed-JSON miss behavior.
- [x] Hold an alert enqueue promise unresolved: response still resolves. Process
  an older timestamp after a newer one: record unconditional last-status
  overwrite. These are source semantics, not corrected or claimed durable.
- [x] Capture configured worker concurrency, retry/backoff, deduplication and
  retention options from actual source; distinguish configuration observation
  from controlled-driver execution. Test actual upstream shutdown and explicit
  fixture cleanup without implying app-wide resource closure.
- [x] Verify successive loader invocations have isolated queues/cache/facades;
  unexpected substituted function/import paths fail closed. Include a precise
  focused control exposing publication before outer commit or skipped refresh,
  with independent intended assertion and intact counterpart passing.
- [ ] Independent spec, quality and final source/evidence reviews; run all six
  sections serially, archive raw JSON/logs and source/facade hashes, commit specific
  files, push PR #7 and inspect Linux/macOS latest-head core/native CI.

## Source findings retained as limits

Pinned grouped cache has TTL 60 and no write invalidation in inspected paths.
Last-status TTL is 86400 and setter has no timestamp guard. Response processor
awaits DB then latest-status cache but does not await `alertingQueue.push`;
execute queue likewise does not await response enqueue. The test must preserve
these semantics, rather than silently introduce a new consistency guarantee.

Native test ownership stays with one agent. Use the existing exact external
runtime and extension; no Redis service/package installation, upstream message,
release, merge or tag belongs in this bounded step. Real BullMQ/Redis acceptance
requires separately owned broker dependencies, service and lifecycle fixtures.
