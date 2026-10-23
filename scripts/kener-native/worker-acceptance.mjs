import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadRuntime } from './runtime.mjs';
import { installAdapter, PROJECTION, VIEW } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';
import { validateUpstream } from './prepare.mjs';

const sample = (tag, timestamp = 901, latency = 1.25, status = 'UP') => ({
  monitor_tag: tag, timestamp, latency, status, type: 'REALTIME', raw_status: status, error_message: null,
});
const baseFields = ['monitor_tag', 'timestamp', 'latency', 'status', 'type', 'raw_status', 'error_message'];
const ordered = (rows) => [...rows].sort((a, b) => a.monitor_tag.localeCompare(b.monitor_tag, 'en') || a.timestamp - b.timestamp);
const baseRows = async (connection) => ordered(await connection('monitoring_data').select(baseFields));
const snapshot = async (connection) => ({
  base: await connection.raw('SELECT rowid AS source_rowid,* FROM monitoring_data ORDER BY rowid'),
  projection: await connection(PROJECTION).select('*').orderBy('source_rowid'),
  view: await connection(VIEW).select('*').orderBy(['bucket_ts', 'monitor_tag', 'status']),
});

function expectedView(rows) {
  const groups = new Map();
  for (const row of rows) {
    const bucket = Math.floor(row.timestamp / 900) * 900;
    const key = JSON.stringify([bucket, row.monitor_tag, row.status]);
    if (!groups.has(key)) groups.set(key, {
      bucket_ts: bucket, monitor_tag: row.monitor_tag, status: row.status,
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
const canonical = (rows) => rows.map((row) => JSON.stringify(row)).sort();

// Real upstream context, native repository methods and native SQLite. Expected
// committed facts are maintained independently rather than copied from the base.
export async function workerAcceptance(args) {
  mkdirSync(args.scratch ?? os.tmpdir(), { recursive: true });
  const scratch = mkdtempSync(path.join(realpathSync(args.scratch ?? os.tmpdir()), 'kener-worker-'));
  const runtime = await loadRuntime({ ...args, database: path.join(scratch, 'caller.sqlite') });
  const extension = realpathSync(args.extension);
  const lifecycle = [], opened = new Set(), emittedQueries = [], checks = [], transactions = [], mutations = [];
  const assertions = { baseComparisons: 0, projectionComparisons: 0, viewComparisons: 0, contextComparisons: 0, nativeQueryIdentities: 0, transactionIdentities: 0 };
  const open = (name) => {
    const connection = runtime.createDatabase(path.join(scratch, name), extension, lifecycle);
    connection.on('query', ({ sql, bindings, __knexUid, __knexTxId }) => emittedQueries.push({ database: name, sql, bindings: bindings ?? [], nativeConnection: __knexUid, transaction: __knexTxId }));
    opened.add(connection);
    return connection;
  };
  const close = async (connection) => { await connection.destroy(); opened.delete(connection); };
  let db = open('caller.sqlite');
  const ambient = open('ambient.sqlite');
  const committed = [];
  const ambientRows = [sample('ambient-sentinel', 900, 9.5)];
  function context(expected, label) {
    assert.equal(runtime.getWorkerKnex(), expected, label);
    assertions.contextComparisons++;
  }
  async function state(connection, expected, label) {
    assert.deepEqual(await baseRows(connection), ordered(expected), `${label}: independent base facts`);
    assertions.baseComparisons++;
    const native = await connection.raw('SELECT rowid AS source_rowid,monitor_tag,timestamp FROM monitoring_data ORDER BY rowid');
    const facts = native.map(({ source_rowid, monitor_tag, timestamp }) => {
      const row = expected.find((fact) => fact.monitor_tag === monitor_tag && fact.timestamp === timestamp);
      assert(row, `${label}: unexpected native key`);
      return { source_rowid, bucket_ts: Math.floor(timestamp / 900) * 900, monitor_tag, status: row.status, latency: row.latency };
    });
    assert.deepEqual(await connection(PROJECTION).select('*').orderBy('source_rowid'), facts, `${label}: independent projection facts`);
    assertions.projectionComparisons++;
    const fields = ['bucket_ts', 'monitor_tag', 'status', 'sample_count', 'latency_sum', 'latency_count', 'min_latency', 'max_latency'];
    assert.deepEqual(canonical(await connection(VIEW).select(fields)), canonical(expectedView(expected)), `${label}: independent public view`);
    assertions.viewComparisons++;
  }
  // Inspect the actual better-sqlite3 handle retained by Knex's transaction
  // client. Its __knexUid must match every emitted query, including refresh,
  // savepoints and final COMMIT/ROLLBACK, until the returned promise settles.
  async function transaction(connection, work, options) {
    const records = [];
    const listen = (query) => records.push({ sql: query.sql, nativeConnection: query.__knexUid, transaction: query.__knexTxId });
    let nativeConnection;
    connection.on('query', listen);
    try {
      return await kenerTransaction(connection, runtime, async (scope) => {
        context(scope.connection, 'host binds actual upstream context');
        assert.equal(scope.repository.knex, scope.connection, 'actual BaseRepository getter uses host transaction');
        const native = await scope.connection.client.acquireConnection();
        assert.equal(native.open, true);
        assert.equal(typeof native.__knexUid, 'string');
        nativeConnection = native.__knexUid;
        assertions.transactionIdentities++;
        return work(scope);
      }, options);
    } finally {
      connection.removeListener('query', listen);
      if (nativeConnection) {
        assert(records.length > 0, 'host must emit transaction SQL');
        for (const record of records) {
          assert.equal(record.nativeConnection, nativeConnection, `SQL escaped native transaction connection: ${record.sql}`);
          assert.equal(typeof record.transaction, 'string', `SQL lacks Knex transaction identity: ${record.sql}`);
          assertions.nativeQueryIdentities++;
        }
        transactions.push({ nativeConnection, queries: records });
      }
    }
  }
  try {
    context(undefined, 'no initial ambient context');
    for (const connection of [db, ambient]) { await runtime.migrate(connection); await installAdapter(connection); }
    await transaction(ambient, ({ repository }) => repository.insertMonitoringData(ambientRows[0]));
    context(undefined, 'host restores initially absent context');

    // Constructor-only binding demonstrably loses to ambient context. The query
    // uses another free pool, so this negative probe cannot self-deadlock.
    await runtime.runWithWorkerKnex(ambient, () => db.transaction(async (trx) => {
      const repository = new runtime.MonitoringRepository(trx);
      assert.equal(repository.knex, ambient);
      assert.deepEqual(await repository.getLatestMonitoringData('ambient-sentinel'), ambientRows[0]);
      assert.deepEqual(await trx('monitoring_data').select(baseFields), []);
    }));
    context(undefined, 'constructor precedence probe restores context');
    checks.push('actual constructor transaction loses to ambient worker pool');

    const readEvents = [];
    const first = sample('committed', 901, 2.5);
    const result = await runtime.runWithWorkerKnex(ambient, async () => {
      context(ambient, 'ambient pool before host');
      const value = await transaction(db, async ({ repository, connection, reader, refresh }) => {
        await repository.insertMonitoringData(first);
        const strict = await reader.read(false, first.monitor_tag, 900, 900, 1);
        assert.equal(strict.provenance.mode, 'strict');
        assert.equal(strict.provenance.source, 'original-repository');
        assert.equal(strict.rows[0].avgLatency, first.latency);
        assert.equal((await connection(PROJECTION).where('monitor_tag', first.monitor_tag)).length, 1);
        assert.deepEqual(await connection(VIEW).select('*'), [], 'public view stale before explicit refresh');
        await refresh();
        await state(connection, [first], 'explicit read-own-write refresh');
        return 'caller result';
      }, { onRead: (provenance) => readEvents.push(provenance) });
      context(ambient, 'ambient pool restored after host success');
      return value;
    });
    assert.equal(result, 'caller result');
    assert.equal(readEvents.length, 1);
    committed.push(first);
    await state(db, committed, 'atomic success');
    await state(ambient, ambientRows, 'caller explicitly selects different pool');
    context(undefined, 'ambient wrapper restores absent context');
    checks.push('explicit caller connection overrides different ambient pool; strict default and caller result');

    const numeric = sample('numeric', 1801, 0.125);
    await runtime.runWithWorkerKnex(ambient, async () => {
      await transaction(db, async ({ repository, reader, refresh }) => {
        await repository.insertMonitoringData(numeric);
        const before = await reader.read(true, [numeric.monitor_tag], 1800, 900, 1);
        assert.equal(before.provenance.source, 'public-ivm-view');
        assert.deepEqual(before.rows, []);
        await refresh();
        const after = await reader.read(true, [numeric.monitor_tag], 1800, 900, 1);
        assert.deepEqual(after.rows, [{ monitor_tag: numeric.monitor_tag, ts: 1800, countOfUp: 1, countOfDown: 0, countOfDegraded: 0, countOfMaintenance: 0, avgLatency: 0.125, maxLatency: 0.125, minLatency: 0.125 }]);
      }, { mode: 'experimental-numeric' });
      context(ambient, 'ambient restored after numeric host');
    });
    committed.push(numeric);
    await state(db, committed, 'numeric explicit refresh');
    checks.push('experimental numeric opt-in stays stale until explicit refresh');

    const beforeFailure = await snapshot(db);
    const callerFailure = new Error('worker caller failure');
    await runtime.runWithWorkerKnex(ambient, async () => {
      await assert.rejects(() => transaction(db, async ({ repository, refresh }) => {
        await repository.insertMonitoringData(sample('caller-rollback'));
        await refresh();
        throw callerFailure;
      }), (error) => error === callerFailure);
      context(ambient, 'ambient restored after caller failure');
    });
    assert.deepEqual(await snapshot(db), beforeFailure, 'caller failure restores all three layers');
    await state(db, committed, 'caller rollback');
    context(undefined, 'no leaked context after caller failure');
    checks.push('caller failure propagates same error and rolls back base/projection/public view');

    await db.raw('CREATE TABLE kener_worker_failure_guard (group_id INTEGER, value INTEGER)');
    await db.raw('INSERT INTO kener_worker_failure_guard VALUES (1,9223372036854775807)');
    await db.raw("CREATE VIRTUAL TABLE zz_kener_worker_failure_view USING ivm('SELECT group_id,SUM(value) AS total FROM kener_worker_failure_guard GROUP BY group_id')");
    let refreshFailure;
    await runtime.runWithWorkerKnex(ambient, async () => {
      await assert.rejects(() => transaction(db, async ({ repository, connection }) => {
        await repository.insertMonitoringData(sample('refresh-rollback'));
        await connection.raw('INSERT INTO kener_worker_failure_guard VALUES (1,1)');
        // Deliberately leave refresh to the host's precommit path.
      }), (error) => {
        refreshFailure = error.message;
        return /ivm_refresh_all: view zz_kener_worker_failure_view: integer overflow in SUM; no view was refreshed/.test(error.message);
      });
      context(ambient, 'ambient restored after automatic refresh failure');
    });
    assert.deepEqual(await snapshot(db), beforeFailure, 'automatic refresh failure restores all three layers');
    assert.deepEqual(await db.raw('SELECT CAST(value AS TEXT) AS value FROM kener_worker_failure_guard'), [{ value: '9223372036854775807' }]);
    await transaction(db, async () => {});
    await state(db, committed, 'clean refresh after overflow rollback');
    await db.raw('DROP TABLE zz_kener_worker_failure_view');
    await db.raw('DROP TABLE kener_worker_failure_guard');
    checks.push('real automatic precommit SUM overflow rolls back and leaves clean pending state');

    const outerKeep = sample('outer-keep', 2701, 4.5);
    await runtime.runWithWorkerKnex(ambient, async () => {
      await transaction(db, async ({ repository, connection }) => {
        await repository.insertMonitoringData(outerKeep);
        await assert.rejects(() => transaction(connection, async ({ repository: nested, connection: savepoint, refresh }) => {
          assert.notEqual(savepoint, connection);
          await nested.insertMonitoringData(sample('inner-drop'));
          await refresh();
          throw new Error('inner savepoint rollback');
        }), /inner savepoint rollback/);
        context(connection, 'nested failure restores outer trx');
        assert.equal(repository.knex, connection);
        // Exercise a real repository method that itself opens a savepoint.
        await repository.updateMonitoringData(outerKeep.monitor_tag, outerKeep.timestamp, outerKeep.timestamp, 'DOWN', 'REALTIME', outerKeep.latency, 0);
      });
      context(ambient, 'nested failure outer success restores ambient pool');
    });
    committed.push({ ...outerKeep, status: 'DOWN' });
    await state(db, committed, 'inner rollback outer commit including original method savepoint');

    const beforeOuterRollback = await snapshot(db);
    await runtime.runWithWorkerKnex(ambient, async () => {
      await assert.rejects(() => transaction(db, async ({ repository, connection }) => {
        await repository.insertMonitoringData(sample('outer-drop'));
        await transaction(connection, async ({ repository: nested }) => nested.insertMonitoringData(sample('inner-commit-drop')));
        context(connection, 'nested success restores outer trx');
        throw new Error('outer rollback after inner commit');
      }), /outer rollback after inner commit/);
      context(ambient, 'outer rollback restores ambient pool');
    });
    assert.deepEqual(await snapshot(db), beforeOuterRollback);
    await state(db, committed, 'inner commit outer rollback');
    context(undefined, 'nested operations leave no ambient leak');
    checks.push('inner refreshed rollback then outer commit; inner commit then outer rollback; actual repository nested savepoint');

    // Both contexts overlap across awaited barriers, but only one connection
    // writes at a time. Separate database files avoid SQLite writer contention.
    let releaseFirst, releaseSecond, firstReady, secondReady;
    const firstGate = new Promise((resolve) => { releaseFirst = resolve; });
    const secondGate = new Promise((resolve) => { releaseSecond = resolve; });
    const firstStarted = new Promise((resolve) => { firstReady = resolve; });
    const secondStarted = new Promise((resolve) => { secondReady = resolve; });
    const concurrentFirst = sample('async-first', 3601, 5.5);
    const concurrentSecond = sample('async-second', 3601, 6.5);
    const firstTask = runtime.runWithWorkerKnex(ambient, () => transaction(db, async ({ repository, connection }) => {
      firstReady();
      await firstGate;
      context(connection, 'first context survives second context entry');
      await repository.insertMonitoringData(concurrentFirst);
      releaseSecond();
      await secondStarted;
      context(connection, 'first context remains isolated');
    })).finally(() => { firstReady(); releaseSecond(); });
    const secondTask = runtime.runWithWorkerKnex(db, () => transaction(ambient, async ({ repository, connection }) => {
      secondReady();
      await secondGate;
      context(connection, 'second context survives first task continuation');
      await repository.insertMonitoringData(concurrentSecond);
    })).finally(() => { secondReady(); });
    const settled = Promise.allSettled([firstTask, secondTask]);
    await Promise.all([firstStarted, secondStarted]);
    context(undefined, 'parent context unaffected by overlapping tasks');
    releaseFirst();
    // allSettled ensures neither transaction remains active after a failure.
    const asyncResults = await settled;
    for (const item of asyncResults) { if (item.status === 'rejected') throw item.reason; }
    committed.push(concurrentFirst); ambientRows.push(concurrentSecond);
    context(undefined, 'async tasks restore absent parent context');
    await state(db, committed, 'first async task independent facts');
    await state(ambient, ambientRows, 'second async task independent facts');
    checks.push('controlled overlapping AsyncLocalStorage contexts isolate native repositories and restore parent');

    // Focused mutant runs the real host helper with only its scope binding
    // removed. Native repository writes escape to ambient and autocommit there;
    // an independent expected-base assertion must detect the exact regression.
    const escaped = sample('mutant-escaped');
    const mutantRuntime = { ...runtime, runWithWorkerKnex: (_connection, work) => work() };
    let redMessage;
    await runtime.runWithWorkerKnex(ambient, async () => {
      await kenerTransaction(db, mutantRuntime, ({ repository }) => repository.insertMonitoringData(escaped));
      try { assert.deepEqual(await baseRows(db), ordered([...committed, escaped]), 'missing worker scope binding: independent base facts'); }
      catch (error) { redMessage = error.message; }
      assert(redMessage?.includes('missing worker scope binding'), 'scope mutant must be detected by independent state assertion');
      assert.deepEqual(await new runtime.MonitoringRepository(db).getLatestMonitoringData(escaped.monitor_tag), escaped);
      context(ambient, 'mutant leaves real ambient context intact');
    });
    await transaction(ambient, ({ repository }) => repository.deleteMonitorDataByTag(escaped.monitor_tag));
    await runtime.runWithWorkerKnex(ambient, () => transaction(db, ({ repository }) => repository.insertMonitoringData(escaped)));
    committed.push(escaped);
    await state(db, committed, 'intact counterpart commits to explicit caller');
    await state(ambient, ambientRows, 'intact counterpart leaves ambient database unchanged');
    mutations.push({ mutation: 'omit-actual-worker-scope-binding', status: 'detected', redFailure: redMessage, intact: 'passed' });

    await close(db); db = open('caller.sqlite');
    await runtime.runWithWorkerKnex(ambient, () => transaction(db, async ({ reader }) => {
      const result = await reader.read(false, 'committed', 900, 900, 1);
      assert.equal(result.provenance.source, 'original-repository');
      assert.equal(result.rows[0].avgLatency, 2.5);
    }));
    await state(db, committed, 'worker helper reopen without bootstrap');
    assert.equal(lifecycle.filter(({ event, filename }) => event === 'extension-loaded' && filename.endsWith('/caller.sqlite')).length, 2);
    context(undefined, 'final worker context absent');
    checks.push('extension reload on native reopen; persisted state reused without bootstrap');
    const identity = runtime.identity(extension);
    assert(identity.sourceHashes['src/lib/server/db/poolContext.ts']);
    assert(!Object.keys(identity.sourceHashes).some((file) => file.endsWith('/dbimpl.ts')));
    validateUpstream(args.upstream);
    return {
      status: 'passed', scope: 'Explicit worker transaction host integration using actual pinned AsyncLocalStorage; no queue, singleton, endpoint or Redis boot',
      ...identity, scratch, assertions, checks, transactions, emittedQueries, mutations, lifecycle, refreshFailure,
      readEvents, expectedFacts: { caller: committed, ambient: ambientRows }, upstreamClean: true,
      extensionLoadFailureCoverage: 'driverSmoke in the same serial acceptance command',
    };
  } finally {
    for (const connection of opened) await close(connection);
    runtime.close();
  }
}
