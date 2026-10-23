import assert from 'node:assert/strict';
import { loadRuntime } from './runtime.mjs';
import { kenerTransaction } from './host.mjs';

// A separate event loop is essential: better-sqlite3's busy wait blocks its
// caller's timers. This fixture never changes journal mode or retries work.
const MAX_HOLD_MS = 15000;

async function main() {
  assert(process.send, 'Lock holder requires an IPC parent');
  const config = JSON.parse(process.argv[2]);
  const lifecycle = [], queries = [];
  let release, rejectRelease, timer, watchdog, runtime, db, callbackCount = 0, nativeConnection;
  const gate = new Promise((resolve, reject) => { release = resolve; rejectRelease = reject; });
  // A disconnect must also unwind an in-flight transaction.
  const abort = () => rejectRelease(new Error('Lock holder parent disconnected or aborted'));
  const onMessage = (message) => {
    if (message?.type === 'release') release('ipc');
    else if (message?.type === 'arm-timer') {
      assert(config.timedReleaseMs > 0 && config.timedReleaseMs < MAX_HOLD_MS);
      assert.equal(timer, undefined, 'Timed release can only be armed once');
      timer = setTimeout(() => release('child-timer'), config.timedReleaseMs);
    } else if (message?.type === 'abort') abort();
  };
  process.on('message', onMessage);
  process.on('disconnect', abort);
  // Attach a rejection handler before asynchronous setup can receive an abort.
  gate.catch(() => {});
  watchdog = setTimeout(() => rejectRelease(new Error('Lock holder exceeded bounded hold time')), MAX_HOLD_MS);
  try {
    runtime = await loadRuntime(config);
    db = runtime.createDatabase(config.database, config.extension, lifecycle, config.policy);
    db.on('query', (query) => queries.push({ sql: query.sql, nativeConnection: query.__knexUid, transaction: query.__knexTxId }));
    await db.raw('SELECT 1'); // Warm the independent pool before taking the lock.
    const warm = await db.client.acquireConnection();
    const nativeBusyTimeoutMs = warm.pragma('busy_timeout', { simple: true });
    await db.client.releaseConnection(warm);
    assert.equal(nativeBusyTimeoutMs, config.policy.busyTimeoutMs);
    const transactionStart = queries.length;
    const releaseReason = await kenerTransaction(db, runtime, async ({ repository, connection, refresh }) => {
      callbackCount++;
      assert.equal(runtime.getWorkerKnex(), connection);
      assert.equal(repository.knex, connection);
      const native = await connection.client.acquireConnection();
      nativeConnection = native.__knexUid;
      await repository.insertMonitoringData(config.row);
      await refresh();
      assert.deepEqual(await repository.getLatestMonitoringData(config.row.monitor_tag), config.row);
      process.send({ type: 'ready', pid: process.pid, nativeConnection, nativeBusyTimeoutMs, callbackCount, lifecycle });
      return gate;
    });
    assert.equal(runtime.getWorkerKnex(), undefined, 'Child host restores context');
    assert.equal(callbackCount, 1, 'Child work is never implicitly retried');
    const transactionQueries = queries.slice(transactionStart);
    assert(transactionQueries.length > 0);
    for (const query of transactionQueries) {
      assert.equal(query.nativeConnection, nativeConnection, `Child SQL escaped transaction: ${query.sql}`);
      assert.equal(typeof query.transaction, 'string');
    }
    await db.destroy(); db = undefined;
    process.send({ type: 'committed', pid: process.pid, releaseReason, callbackCount, nativeConnection, queries: transactionQueries, lifecycle });
  } finally {
    clearTimeout(timer); clearTimeout(watchdog);
    process.removeListener('message', onMessage);
    process.removeListener('disconnect', abort);
    try { if (db) await db.destroy(); }
    finally { runtime?.close(); }
  }
}

try { await main(); }
catch (error) {
  if (process.connected) process.send({ type: 'failed', error: error.stack });
  else console.error(error.stack);
  process.exitCode = 1;
} finally {
  if (process.connected) process.disconnect();
}
