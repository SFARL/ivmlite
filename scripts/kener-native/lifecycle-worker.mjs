import assert from 'node:assert/strict';
import { loadRuntime } from './runtime.mjs';
import { migrateAdapter } from './lifecycle.mjs';
import { VIEW } from './adapter.mjs';

// IPC barriers identify the exact commit boundary. Parent SIGKILL deliberately
// bypasses this fixture's finally and leaves the actual native handle open.
async function main() {
  assert(process.send, 'Lifecycle worker requires an IPC parent');
  const config = JSON.parse(process.argv[2]);
  assert(['before-commit', 'after-commit'].includes(config.phase));
  let runtime, db, rejectGate;
  const gate = new Promise((resolve, reject) => { rejectGate = reject; });
  gate.catch(() => {});
  const abort = () => rejectGate(new Error('Lifecycle worker parent disconnected or deadline expired'));
  process.on('disconnect', abort);
  const watchdog = setTimeout(abort, 20000);
  const queries = [], lifecycle = [];
  try {
    runtime = await loadRuntime(config);
    db = runtime.createDatabase(config.database, config.extension, lifecycle, config.policy);
    db.on('query', ({ sql, __knexUid, __knexTxId }) => queries.push({ sql, nativeConnection: __knexUid, transaction: __knexTxId }));
    assert.deepEqual(await db.raw('PRAGMA journal_mode'), [{ journal_mode: 'delete' }]);
    async function ready() {
      const native = await db.client.acquireConnection();
      try { assert(native.open, 'Readiness requires a still-open native handle'); }
      finally { await db.client.releaseConnection(native); }
      process.send({ type: 'ready', phase: config.phase, pid: process.pid, nativeOpen: true, lifecycle, queries });
      await gate;
    }
    await db.transaction(async (outer) => {
      const value = await migrateAdapter(outer, runtime, async ({ connection, repository }) => {
        assert.equal(runtime.getWorkerKnex(), connection);
        assert.equal(repository.knex, connection);
        await connection.raw("ALTER TABLE monitoring_data ADD COLUMN host_note TEXT NOT NULL DEFAULT 'worker-default'");
        await connection.raw('UPDATE monitoring_data SET rowid = rowid + 100, timestamp = timestamp + 900, monitor_tag = monitor_tag || ?', ['-moved']);
        await repository.insertMonitoringData(config.row);
        return 'worker-rebuilt';
      });
      assert.equal(value, 'worker-rebuilt');
      assert.equal(runtime.getWorkerKnex(), undefined);
      assert((await outer.raw(`SELECT * FROM ${VIEW}`)).length > 0);
      if (config.phase === 'before-commit') {
        // Acquiring from db here would deadlock its one-handle pool. Verify the
        // actual transaction native handle, then wait inside the outer BEGIN.
        const native = await outer.client.acquireConnection();
        assert(native.open);
        process.send({ type: 'ready', phase: config.phase, pid: process.pid, nativeOpen: native.open, lifecycle, queries });
        await gate;
      }
    });
    await ready();
  } finally {
    clearTimeout(watchdog); process.removeListener('disconnect', abort);
    try { if (db) await db.destroy(); }
    finally { runtime?.close(); }
  }
}
try { await main(); }
catch (error) {
  if (process.connected) process.send({ type: 'failed', error: error.stack });
  else console.error(error.stack);
  process.exitCode = 1;
} finally { if (process.connected) process.disconnect(); }
