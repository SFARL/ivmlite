import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { loadRuntime } from './runtime.mjs';
import { installAdapter, PROJECTION, VIEW } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';
import { inside, sha256, validateUpstream } from './prepare.mjs';

const CHILD_DEADLINE_MS = 20000;
const SHORT_BUSY_MS = 40;
const WAIT_BUSY_MS = 4000;
const TIMED_RELEASE_MS = 200;
const baseFields = ['monitor_tag', 'timestamp', 'latency', 'status', 'type', 'raw_status', 'error_message'];
const viewFields = ['bucket_ts', 'monitor_tag', 'status', 'sample_count', 'latency_sum', 'latency_count', 'min_latency', 'max_latency'];
const sample = (tag, timestamp = 901, latency = 1.25, status = 'UP') => ({
  monitor_tag: tag, timestamp, latency, status, type: 'REALTIME', raw_status: status, error_message: null,
});
const canonical = (rows) => rows.map((row) => JSON.stringify(row)).sort();

function expectedView(rows) {
  const groups = new Map();
  for (const row of rows) {
    const bucket_ts = Math.floor(row.timestamp / 900) * 900;
    const key = JSON.stringify([bucket_ts, row.monitor_tag, row.status]);
    if (!groups.has(key)) groups.set(key, {
      bucket_ts, monitor_tag: row.monitor_tag, status: row.status,
      sample_count: 0, latency_sum: null, latency_count: 0, min_latency: null, max_latency: null,
    });
    const group = groups.get(key);
    group.sample_count++;
    if (row.latency !== null) {
      group.latency_sum = (group.latency_sum ?? 0) + row.latency;
      group.latency_count++;
      group.min_latency = group.min_latency === null ? row.latency : Math.min(group.min_latency, row.latency);
      group.max_latency = group.max_latency === null ? row.latency : Math.max(group.max_latency, row.latency);
    }
  }
  return [...groups.values()];
}

// Every awaited child event has a deadline and observes spawn, IPC and exit
// errors. Cleanup escalates to a bounded kill even if the fixture is broken.
function lockHolder(config) {
  const child = fork(fileURLToPath(new URL('./lock-holder.mjs', import.meta.url)), [JSON.stringify(config)], {
    execPath: process.execPath, stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
  });
  const messages = [], waiters = new Set();
  let failure, exitStatus, stdout = '', stderr = '';
  child.stdout.on('data', (data) => { stdout = (stdout + data).slice(-65536); });
  child.stderr.on('data', (data) => { stderr = (stderr + data).slice(-65536); });
  function flush() {
    for (const waiter of waiters) {
      const found = messages.find((message) => message.type === waiter.type);
      if (found) waiter.finish(null, found);
      else if (failure || exitStatus) waiter.finish(failure ?? new Error(`Lock holder exited before ${waiter.type}: ${JSON.stringify(exitStatus)} ${stderr}`));
    }
  }
  const exited = new Promise((resolve) => child.once('exit', (code, signal) => {
    exitStatus = { code, signal }; resolve(exitStatus); flush();
  }));
  child.on('error', (error) => { failure = error; flush(); });
  child.on('message', (message) => {
    messages.push(message);
    if (message.type === 'failed') failure = new Error(message.error);
    flush();
  });
  function send(type) {
    assert(child.connected, `Lock holder IPC unavailable for ${type}`);
    child.send({ type }, (error) => { if (error) { failure = error; flush(); } });
  }
  function waitFor(type) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => waiter.finish(new Error(`Lock holder deadline waiting for ${type}: ${stderr}`)), CHILD_DEADLINE_MS);
      const waiter = { type, finish(error, value) {
        clearTimeout(timer); waiters.delete(waiter);
        if (error) reject(error); else resolve(value);
      } };
      waiters.add(waiter); flush();
    });
  }
  async function boundedExit(milliseconds) {
    let timer;
    try { return await Promise.race([exited, new Promise((resolve) => { timer = setTimeout(() => resolve(null), milliseconds); })]); }
    finally { clearTimeout(timer); }
  }
  return {
    send, waitFor,
    async complete() {
      const committed = await waitFor('committed');
      const status = await boundedExit(CHILD_DEADLINE_MS);
      assert(status, 'Lock holder did not exit after commit');
      assert.deepEqual(status, { code: 0, signal: null });
      return { ...committed, exit: status, stdout, stderr };
    },
    async close() {
      if (!exitStatus) {
        if (child.connected) { try { send('abort'); } catch {} }
        if (!await boundedExit(1000)) child.kill('SIGTERM');
        if (!await boundedExit(1000)) child.kill('SIGKILL');
        assert(await boundedExit(2000), 'Lock holder survived SIGKILL');
      }
      for (const waiter of waiters) waiter.finish(new Error('Lock holder closed'));
    },
  };
}

export async function connectionAcceptance(args) {
  args = { ...args, upstream: validateUpstream(args.upstream) };
  mkdirSync(args.scratch ?? os.tmpdir(), { recursive: true });
  const parent = realpathSync(args.scratch ?? os.tmpdir());
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(args.upstream, parent) && !inside(repositoryRoot, parent), 'Scratch storage must be external to source checkouts');
  const scratch = mkdtempSync(path.join(parent, 'kener-connection-'));
  const database = path.join(scratch, 'shared.sqlite');
  const extension = realpathSync(args.extension), expectedExtensionSHA256 = sha256(extension);
  const policy = { expectedExtensionSHA256, busyTimeoutMs: SHORT_BUSY_MS };
  const runtime = await loadRuntime({ ...args, database });
  const lifecycle = [], opened = new Set(), children = new Set(), queries = [], transactions = [], checks = [], mutations = [];
  const assertions = { invalidPolicies: 0, nativeTimeouts: 0, baseComparisons: 0, projectionComparisons: 0, viewComparisons: 0, contextComparisons: 0, nativeQueryIdentities: 0 };
  function open(filename = database, selectedPolicy = policy) {
    const db = runtime.createDatabase(filename, extension, lifecycle, selectedPolicy);
    db.on('query', (query) => queries.push({ filename, sql: query.sql, nativeConnection: query.__knexUid, transaction: query.__knexTxId }));
    opened.add(db); return db;
  }
  async function close(db) { await db.destroy(); opened.delete(db); }
  function context(expected, label) { assert.equal(runtime.getWorkerKnex(), expected, label); assertions.contextComparisons++; }
  async function timeout(db, expected) {
    assert.deepEqual(await db.raw('PRAGMA busy_timeout'), [{ timeout: expected }]);
    const native = await db.client.acquireConnection();
    try { assert.equal(native.pragma('busy_timeout', { simple: true }), expected); }
    finally { await db.client.releaseConnection(native); }
    assertions.nativeTimeouts++;
  }
  async function state(db, expected, label) {
    assert.deepEqual(canonical(await db('monitoring_data').select(baseFields)), canonical(expected), `${label}: independent full base facts`);
    assertions.baseComparisons++;
    const nativeKeys = await db.raw('SELECT rowid AS source_rowid,monitor_tag,timestamp FROM monitoring_data ORDER BY rowid');
    const projection = nativeKeys.map(({ source_rowid, monitor_tag, timestamp }) => {
      const fact = expected.find((row) => row.monitor_tag === monitor_tag && row.timestamp === timestamp);
      assert(fact, `${label}: unexpected native key`);
      return { source_rowid, bucket_ts: Math.floor(fact.timestamp / 900) * 900, monitor_tag: fact.monitor_tag, status: fact.status, latency: fact.latency };
    });
    assert.deepEqual(await db(PROJECTION).select('*').orderBy('source_rowid'), projection, `${label}: independent projection facts`);
    assertions.projectionComparisons++;
    assert.deepEqual(canonical(await db(VIEW).select(viewFields)), canonical(expectedView(expected)), `${label}: independent public grouped view`);
    assertions.viewComparisons++;
  }
  async function transaction(db, label, work) {
    const records = [];
    const listen = (query) => records.push({ sql: query.sql, nativeConnection: query.__knexUid, transaction: query.__knexTxId });
    db.on('query', listen);
    let nativeConnection, callbackCount = 0;
    try {
      return await kenerTransaction(db, runtime, async (scope) => {
        callbackCount++;
        context(scope.connection, `${label}: bound actual upstream context`);
        assert.equal(scope.repository.knex, scope.connection);
        const native = await scope.connection.client.acquireConnection();
        nativeConnection = native.__knexUid;
        return work(scope);
      });
    } finally {
      db.removeListener('query', listen);
      assert.equal(callbackCount, 1, `${label}: no automatic callback retry`);
      assert.equal(typeof nativeConnection, 'string');
      for (const record of records) {
        assert.equal(record.nativeConnection, nativeConnection, `${label}: SQL escaped transaction`);
        assert.equal(typeof record.transaction, 'string');
        assertions.nativeQueryIdentities++;
      }
      transactions.push({ label, callbackCount, nativeConnection, queries: records });
    }
  }
  let db;
  try {
    context(undefined, 'initial context absent');
    const invalidPolicies = [null, [], 'policy', 1, true, new Date(),
      { unexpected: 1 }, { [Symbol('unknown')]: 1 }, Object.defineProperty({}, 'unknown', { value: 1 }), { expectedExtensionSha256: expectedExtensionSHA256 },
      ...[null, '', 'A'.repeat(64), 'a'.repeat(63), 'g'.repeat(64), `${expectedExtensionSHA256}\n`, `${expectedExtensionSHA256}\r`, 1, {}].map((value) => ({ expectedExtensionSHA256: value })),
      ...[null, -1, 0.5, '40', NaN, Infinity, 2147483648].map((value) => ({ busyTimeoutMs: value }))];
    const invalidDatabase = path.join(scratch, 'invalid-policy.sqlite');
    for (const invalid of invalidPolicies) {
      assert.throws(() => runtime.createDatabase(invalidDatabase, extension, lifecycle, invalid), TypeError);
      assert(!existsSync(invalidDatabase), 'Malformed policy must fail before a pool/native database is allocated');
      assert.equal(lifecycle.length, 0);
      assertions.invalidPolicies++;
    }
    checks.push('malformed policy rejects synchronously before Knex/pool creation');

    db = open(); await runtime.migrate(db); await installAdapter(db);
    await timeout(db, SHORT_BUSY_MS);
    const seed = [sample('shared'), sample('neighbor', 902, 0.5), sample('null-only', 1801, null, null)];
    await transaction(db, 'initial seed', async ({ repository }) => { for (const row of seed) await repository.insertMonitoringData(row); });
    await state(db, seed, 'initial pinned handle');
    const initialNative = transactions.at(-1).nativeConnection;
    await close(db); db = open(); await timeout(db, SHORT_BUSY_MS);
    await state(db, seed, 'pinned reopen');
    assert.equal(lifecycle.filter((event) => event.filename === database && event.event === 'extension-loaded').length, 2);
    checks.push('valid exact artifact pin and configured timeout on initial native handle and reopen');

    // Reinitialize the caller's pool and prove the replacement native handle
    // also runs afterCreate, independently of a new createDatabase call.
    const discarded = await db.client.acquireConnection();
    const discardedId = discarded.__knexUid;
    await db.client.releaseConnection(discarded);
    await db.client.destroy();
    db.client.initializePool();
    assert.equal(discarded.open, false);
    await timeout(db, SHORT_BUSY_MS);
    const replacement = await db.client.acquireConnection();
    assert.notEqual(replacement.__knexUid, discardedId);
    assert.notEqual(replacement.__knexUid, initialNative);
    await db.client.releaseConnection(replacement);
    assert.equal(lifecycle.filter((event) => event.filename === database && event.event === 'extension-loaded').length, 3);
    checks.push('replacement native handle after explicit pool recreation rechecks identity and timeout');

    const zero = open(path.join(scratch, 'zero.sqlite'), { expectedExtensionSHA256, busyTimeoutMs: 0 });
    await timeout(zero, 0); await close(zero);
    const negativeZero = open(path.join(scratch, 'negative-zero.sqlite'), { expectedExtensionSHA256, busyTimeoutMs: -0 });
    await timeout(negativeZero, 0); await close(negativeZero);
    const maximum = open(path.join(scratch, 'maximum.sqlite'), { busyTimeoutMs: 2147483647 });
    await timeout(maximum, 2147483647); await close(maximum);
    const unpinned = open(path.join(scratch, 'unpinned.sqlite'), {});
    await timeout(unpinned, 5000); await close(unpinned);
    assert(!Object.hasOwn(lifecycle.find((event) => event.filename.endsWith('/unpinned.sqlite')).policy, 'expectedExtensionSHA256'));
    checks.push('timeout zero (including normalized negative zero), maximum integer boundary and legacy unpinned 5000ms default applied natively');

    const wrongHash = expectedExtensionSHA256 === '0'.repeat(64) ? '1'.repeat(64) : '0'.repeat(64);
    const mismatched = open(database, { expectedExtensionSHA256: wrongHash, busyTimeoutMs: 0 });
    try {
      await assert.rejects(() => mismatched('monitoring_data').insert(sample('pin-must-not-write')), /Extension artifact identity mismatch/);
    } finally { await close(mismatched); }
    const identityFailure = lifecycle.find((event) => event.event === 'extension-load-failed');
    assert.equal(identityFailure.stage, 'identity');
    assert.equal(identityFailure.open, false, 'Mismatched artifact closes unowned native handle');
    await state(db, seed, 'mismatched pin cannot write');
    // Existing missing-extension cleanup remains covered by driverSmoke. A
    // missing file with a supplied pin also fails before extension loading.
    const missingEvents = [];
    const missing = runtime.createDatabase(path.join(scratch, 'missing-pin.sqlite'), path.join(scratch, 'missing-extension'), missingEvents, policy);
    try { await assert.rejects(() => missing.raw('SELECT 1'), /ENOENT/); }
    finally { await missing.destroy(); }
    assert.equal(missingEvents[0].stage, 'identity'); assert.equal(missingEvents[0].open, false);
    checks.push('mismatched/missing pinned artifact rejected at identity stage, before load/writes, with raw handle closed');

    // Deliberately replace a disposable copy between native opens. Production
    // callers must keep artifact paths immutable; this verifies no pool-wide
    // cached identity can bypass checking the next actual native handle.
    const replacedExtension = path.join(scratch, path.basename(extension));
    copyFileSync(extension, replacedExtension);
    const replacedEvents = [];
    const replacedDb = runtime.createDatabase(path.join(scratch, 'replaced-artifact.sqlite'), replacedExtension, replacedEvents, policy);
    opened.add(replacedDb);
    await timeout(replacedDb, SHORT_BUSY_MS);
    await replacedDb.client.destroy();
    writeFileSync(replacedExtension, Buffer.concat([readFileSync(extension), Buffer.from('\nidentity replacement fixture\n')]));
    replacedDb.client.initializePool();
    await assert.rejects(() => replacedDb.raw('SELECT 1'), /Extension artifact identity mismatch/);
    assert.equal(replacedEvents.at(-1).stage, 'identity');
    assert.equal(replacedEvents.at(-1).open, false);
    await replacedDb.client.destroy();
    copyFileSync(extension, replacedExtension);
    replacedDb.client.initializePool();
    await timeout(replacedDb, SHORT_BUSY_MS);
    await close(replacedDb);
    checks.push('artifact replacement between native handles is detected; restoring exact bytes allows a fresh valid handle');

    const childCorrection = sample('shared', 901, 2.5, 'DOWN');
    const retryCorrection = sample('shared', 901, 4.5, 'DEGRADED');
    let holder = lockHolder({ ...args, database, extension, policy, row: childCorrection }); children.add(holder);
    const ready = await holder.waitFor('ready');
    assert.notEqual(ready.pid, process.pid);
    assert.equal(ready.nativeBusyTimeoutMs, SHORT_BUSY_MS);
    await state(db, seed, 'child holds real write lock; uncommitted correction invisible');
    let busyFailure;
    const failedStart = performance.now();
    // Use a real ambient context and verify restoration after the native error.
    await runtime.runWithWorkerKnex(db, async () => {
      await assert.rejects(() => transaction(db, 'held writer SQLITE_BUSY', ({ repository }) => repository.insertMonitoringData(retryCorrection)), (error) => {
        busyFailure = { code: error.code, message: error.message };
        return error.code === 'SQLITE_BUSY';
      });
      context(db, 'native busy failure restores ambient pool');
    });
    const failedElapsedMs = performance.now() - failedStart;
    context(undefined, 'native busy failure restores absent parent context');
    await state(db, seed, 'failed parent write changed no committed layer');
    holder.send('release');
    const heldChild = await holder.complete(); await holder.close(); children.delete(holder);
    assert.equal(heldChild.releaseReason, 'ipc');
    const afterChild = [childCorrection, ...seed.slice(1)];
    await state(db, afterChild, 'child commit visible on preexisting parent native handle');
    await transaction(db, 'explicit caller new transaction retry', ({ repository }) => repository.insertMonitoringData(retryCorrection));
    const afterRetry = [retryCorrection, ...seed.slice(1)];
    await state(db, afterRetry, 'explicit caller retry corrects child committed/refreshed row');
    checks.push('one file/two independent native writers: held lock produces SQLITE_BUSY once; rollback preserves state/context; explicit new transaction retry succeeds');

    await close(db); db = open(database, { expectedExtensionSHA256, busyTimeoutMs: WAIT_BUSY_MS });
    await timeout(db, WAIT_BUSY_MS); await state(db, afterRetry, 'warm waiting writer pool');
    const timedCorrection = sample('neighbor', 902, 6.5, 'DOWN');
    const waitingCorrection = sample('neighbor', 902, 7.5, 'MAINTENANCE');
    holder = lockHolder({ ...args, database, extension, policy, row: timedCorrection, timedReleaseMs: TIMED_RELEASE_MS }); children.add(holder);
    const timedReady = await holder.waitFor('ready');
    assert.notEqual(timedReady.pid, ready.pid);
    await state(db, afterRetry, 'timed child uncommitted state invisible');
    // Arm the child only when Knex emits the actual parent write, after BEGIN
    // and context setup. The synchronous native call follows this event; the
    // independent child's timer can run while that call waits for the lock.
    let timedReleaseArmed = 0;
    const armTimedRelease = (query) => {
      if (/^insert into `monitoring_data`/.test(query.sql)) { timedReleaseArmed++; holder.send('arm-timer'); }
    };
    db.on('query', armTimedRelease);
    const waitingStart = performance.now();
    try {
      await transaction(db, 'waiting writer succeeds after child timer commit', ({ repository }) => repository.insertMonitoringData(waitingCorrection));
    } finally { db.removeListener('query', armTimedRelease); }
    assert.equal(timedReleaseArmed, 1, 'Child timed release is armed by the actual waiting write');
    const waitingElapsedMs = performance.now() - waitingStart;
    const timedChild = await holder.complete(); await holder.close(); children.delete(holder);
    assert.equal(timedChild.releaseReason, 'child-timer');
    const finalFacts = [retryCorrection, waitingCorrection, seed[2]];
    await state(db, finalFacts, 'waiting writer corrects newly committed child row');
    context(undefined, 'waiting writer restores context');
    await transaction(db, 'clean refresh after writer ownership switches', async () => {});
    await state(db, finalFacts, 'clean refresh after independent writers');
    await close(db); db = open(database, { expectedExtensionSHA256, busyTimeoutMs: 0 });
    await timeout(db, 0); await state(db, finalFacts, 'reopen retains independent expected state');
    await transaction(db, 'clean reopened refresh', async () => {});
    await state(db, finalFacts, 'reopened refresh has no abandoned pending contributions');
    assert.deepEqual(await db.raw('PRAGMA journal_mode'), [{ journal_mode: 'delete' }]);
    checks.push('child event loop timed release permits synchronous waiting writer; same-key corrections, clean refresh and reopen preserve independent base/projection/group facts');

    // Native red/green controls use external copies of the real initializer.
    // Removing the identity gate must let the prohibited artifact open; removing
    // timeout setup/verification must expose better-sqlite3\'s default 5000ms.
    const runtimeSource = readFileSync(new URL('./runtime.mjs', import.meta.url), 'utf8');
    function replaceOnce(source, original, replacement) {
      assert.equal(source.split(original).length - 1, 1, `Mutation source site must occur exactly once: ${original}`);
      return source.replace(original, replacement);
    }
    for (const mutation of ['omit-artifact-pin-check', 'omit-native-timeout-configuration']) {
      let source = replaceOnce(runtimeSource, "from './prepare.mjs'", `from ${JSON.stringify(new URL('./prepare.mjs', import.meta.url).href)}`);
      if (mutation === 'omit-artifact-pin-check') {
        source = replaceOnce(source, 'if (extensionSHA256 !== expectedExtensionSHA256) {', 'if (false) {');
      } else {
        source = replaceOnce(source, 'connection.pragma(`busy_timeout = ${busyTimeoutMs}`);', '// mutant: omitted native configuration');
        source = replaceOnce(source, "assert.equal(nativeBusyTimeoutMs, busyTimeoutMs, 'Native SQLite busy_timeout differs from connection policy');", '// mutant: omitted verification');
      }
      const mutantFile = path.join(scratch, `${mutation}.mjs`); writeFileSync(mutantFile, source);
      const { loadRuntime: loadMutant } = await import(pathToFileURL(mutantFile).href);
      const mutantRuntime = await loadMutant({ ...args, database });
      let mutantDb, redFailure;
      try {
        mutantDb = mutantRuntime.createDatabase(path.join(scratch, `${mutation}.sqlite`), extension, [], {
          expectedExtensionSHA256: mutation === 'omit-artifact-pin-check' ? wrongHash : expectedExtensionSHA256, busyTimeoutMs: 0,
        });
        try {
          if (mutation === 'omit-artifact-pin-check') await assert.rejects(() => mutantDb.raw('SELECT 1'), /Extension artifact identity mismatch/);
          else assert.deepEqual(await mutantDb.raw('PRAGMA busy_timeout'), [{ timeout: 0 }], 'native timeout omission control');
        } catch (error) {
          assert(error instanceof assert.AssertionError, `${mutation}: red must be the intended acceptance assertion`);
          if (mutation === 'omit-artifact-pin-check') assert.match(error.message, /^Missing expected rejection/);
          else {
            assert.match(error.message, /native timeout omission control/);
            assert.deepEqual(error.actual, [{ timeout: 5000 }]);
            assert.deepEqual(error.expected, [{ timeout: 0 }]);
          }
          redFailure = error.message;
        }
        assert(redFailure, `${mutation}: native regression must be detected`);
        mutations.push({ mutation, status: 'detected', redFailure, intact: 'passed' });
      } finally {
        try { if (mutantDb) await mutantDb.destroy(); }
        finally { mutantRuntime.close(); }
      }
    }
    for (const event of lifecycle.filter((item) => item.event === 'extension-loaded')) {
      assert.equal(event.nativeBusyTimeoutMs, event.policy.busyTimeoutMs);
      if (event.policy.expectedExtensionSHA256 !== undefined) assert.equal(event.extensionSHA256, expectedExtensionSHA256);
    }
    validateUpstream(args.upstream);
    return {
      status: 'passed', scope: 'Exact artifact identity and per-native-handle busy timeout; bounded one-file independent writer acceptance, with serialized SQLite commits and explicit caller retry',
      ...runtime.identity(extension), scratch, policy, assertions, checks, lifecycle, missingPinnedArtifact: missingEvents,
      replacedArtifact: replacedEvents, transactions, emittedQueries: queries, expectedFacts: { seed, afterChild, afterRetry, final: finalFacts },
      contention: { busyFailure, failedElapsedMs, waitingElapsedMs, shortBusyTimeoutMs: SHORT_BUSY_MS, waitingBusyTimeoutMs: WAIT_BUSY_MS, childTimedReleaseMs: TIMED_RELEASE_MS,
        heldChild: { ready, ...heldChild }, timedChild: { ready: timedReady, ...timedChild } },
      mutations, journalMode: 'delete', upstreamClean: true,
      extensionLoadFailureCoverage: 'driverSmoke in the same serial acceptance command',
    };
  } finally {
    const cleanupErrors = [];
    for (const child of children) { try { await child.close(); } catch (error) { cleanupErrors.push(error); } }
    for (const connection of opened) { try { await close(connection); } catch (error) { cleanupErrors.push(error); } }
    try { runtime.close(); } catch (error) { cleanupErrors.push(error); }
    if (cleanupErrors.length) throw new AggregateError(cleanupErrors, 'Connection acceptance cleanup failed');
  }
}
