import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inside, options, validateUpstream, writeJson } from './prepare.mjs';
import { COVERING_INDEX, loadRuntime, MIGRATIONS } from './runtime.mjs';
import { adapterAcceptance } from './adapter-acceptance.mjs';
import { workerAcceptance } from './worker-acceptance.mjs';
import { connectionAcceptance } from './connection-acceptance.mjs';
import { lifecycleAcceptance } from './lifecycle-acceptance.mjs';

export async function driverSmoke(args) {
  args = { ...args, upstream: validateUpstream(args.upstream) };
  const scratchParent = args.scratch ?? os.tmpdir();
  mkdirSync(scratchParent, { recursive: true });
  const parent = realpathSync(scratchParent);
  const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(args.upstream, parent) && !inside(repository, parent), 'Scratch storage must be external to source checkouts');
  const scratch = mkdtempSync(path.join(parent, 'kener-native-'));
  const database = path.join(scratch, 'driver.sqlite');
  const extension = realpathSync(args.extension);
  const runtime = await loadRuntime({ ...args, database });
  const lifecycle = [];
  const queries = [];
  let db;
  try {
    db = runtime.createDatabase(database, extension, lifecycle);
    db.on('query', ({ sql, bindings }) => { queries.push({ sql, bindings: bindings ?? [] }); });
    await runtime.migrate(db);
    const repository = new runtime.MonitoringRepository(db);
    const base = { monitor_tag: 'api/east', timestamp: 1000, status: 'UP', latency: 1.25, type: 'REALTIME', error_message: null, raw_status: 'UP' };
    assert.equal((await repository.insertMonitoringData(base)).latency, 1.25);
    await repository.insertMonitoringData({ ...base, status: 'DOWN', latency: 2.5, raw_status: 'DOWN', error_message: 'replayed correction' });
    await repository.insertMonitoringData({ ...base, timestamp: 1061, latency: null, status: null, type: 'SIGNAL', raw_status: null });
    await repository.insertMonitoringData({ ...base, monitor_tag: '002', status: 'UNKNOWN', latency: 0.5 });
    await repository.insertMonitoringData({ ...base, timestamp: 1900, status: 'MAINTENANCE', latency: 0 });
    const single = await repository.getStatusCountsByInterval('api/east', 1000, 900, 2);
    const expected = [
      { ts: 1000, countOfUp: 0, countOfDown: 1, countOfDegraded: 0, countOfMaintenance: 0, avgLatency: 2.5, maxLatency: 2.5, minLatency: 2.5 },
      { ts: 1900, countOfUp: 0, countOfDown: 0, countOfDegraded: 0, countOfMaintenance: 1, avgLatency: 0, maxLatency: 0, minLatency: 0 },
    ];
    assert.deepEqual(single, expected);
    const grouped = await repository.getStatusCountsByIntervalGroupedByMonitor(['api/east', '002', 'api/east', 'missing'], 1000, 900, 2);
    assert.deepEqual(grouped, [
      { monitor_tag: '002', ts: 1000, countOfUp: 0, countOfDown: 0, countOfDegraded: 0, countOfMaintenance: 0, avgLatency: 0.5, maxLatency: 0.5, minLatency: 0.5 },
      ...expected.map((row) => ({ monitor_tag: 'api/east', ...row })),
    ]);
    assert.deepEqual(await repository.getStatusCountsByIntervalGroupedByMonitor([], 1000, 900, 2), []);
    assert.deepEqual(await repository.getStatusCountsByInterval('missing', 1000, 900, 2), []);

    const nativeTypes = await db.raw(`SELECT monitor_tag, timestamp, typeof(monitor_tag) AS tag_type,
      typeof(timestamp) AS timestamp_type, typeof(status) AS status_type, typeof(latency) AS latency_type
      FROM monitoring_data ORDER BY monitor_tag, timestamp`);
    assert.equal(nativeTypes.length, 4, 'Upsert must replace the original composite-key row');
    assert(nativeTypes.every((row) => row.tag_type === 'text' && row.timestamp_type === 'integer'));
    assert.deepEqual(nativeTypes.find((row) => row.monitor_tag === 'api/east' && row.timestamp === 1000), {
      monitor_tag: 'api/east', timestamp: 1000, tag_type: 'text', timestamp_type: 'integer', status_type: 'text', latency_type: 'real',
    });
    assert.equal(nativeTypes.find((row) => row.timestamp === 1061).latency_type, 'null');
    assert.equal(nativeTypes.find((row) => row.timestamp === 1061).status_type, 'null');

    const schema = await db.raw("SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name");
    const tableInfo = await db.raw('PRAGMA table_info(monitoring_data)');
    const tableXInfo = await db.raw('PRAGMA table_xinfo(monitoring_data)');
    const indexInfo = await db.raw('PRAGMA index_list(monitoring_data)');
    assert(indexInfo.some((index) => index.name === COVERING_INDEX));
    assert.deepEqual(tableInfo.filter((column) => column.pk).map((column) => [column.name, column.pk]), [['monitor_tag', 1], ['timestamp', 2]]);
    assert.equal(tableInfo.find((column) => column.name === 'latency').type.toLowerCase(), 'float');
    assert(tableInfo.some((column) => column.name === 'error_message'));
    assert(tableInfo.some((column) => column.name === 'raw_status'));
    const intervalQueries = queries.filter(({ sql }) => /SUM\(CASE WHEN status/.test(sql));
    assert.equal(intervalQueries.length, 3);
    const plans = [];
    for (const query of intervalQueries) {
      const plan = await db.raw(`EXPLAIN QUERY PLAN ${query.sql}`, query.bindings);
      assert(plan.some((row) => row.detail.includes(`COVERING INDEX ${COVERING_INDEX}`)), JSON.stringify(plan));
      plans.push({ ...query, plan });
    }
    const sqlite = await db.raw('SELECT sqlite_version() AS version, sqlite_source_id() AS source_id');
    const pragmas = {
      compileOptions: await db.raw('PRAGMA compile_options'),
      journalMode: await db.raw('PRAGMA journal_mode'),
      synchronous: await db.raw('PRAGMA synchronous'),
      cacheSize: await db.raw('PRAGMA cache_size'),
    };
    assert.equal((await db.raw("SELECT count(*) AS count FROM pragma_function_list WHERE name = 'ivm_refresh_all'"))[0].count, 1);
    await db.raw('SELECT ivm_refresh_all()');
    const rowsBeforeClose = await db('monitoring_data').select('*').orderBy(['monitor_tag', 'timestamp']);
    await db.destroy();
    db = runtime.createDatabase(database, extension, lifecycle);
    assert.equal((await db.raw("SELECT count(*) AS count FROM pragma_function_list WHERE name = 'ivm_refresh_all'"))[0].count, 1);
    await db.raw('SELECT ivm_refresh_all()');
    assert.deepEqual(await db('monitoring_data').select('*').orderBy(['monitor_tag', 'timestamp']), rowsBeforeClose);
    assert.deepEqual(await new runtime.MonitoringRepository(db).getStatusCountsByInterval('api/east', 1000, 900, 2), expected);
    await db.destroy();
    db = undefined;
    const invalidExtension = path.join(scratch, 'missing-extension');
    const broken = runtime.createDatabase(path.join(scratch, 'load-failure.sqlite'), invalidExtension, lifecycle);
    let loadFailure;
    try {
      await assert.rejects(async () => {
        try { await broken.raw('SELECT 1'); }
        catch (error) { loadFailure = error.message; throw error; }
      }, /missing-extension/);
    } finally { await broken.destroy(); }
    assert.equal(lifecycle.filter((event) => event.event === 'extension-loaded').length, 2);
    assert.equal(lifecycle.filter((event) => event.event === 'extension-load-failed').length, 1);
    assert.equal(lifecycle.find((event) => event.event === 'extension-load-failed').open, false, 'Failed initialization must close the unowned native connection');
    const identity = runtime.identity(extension);
    assert(!Object.keys(identity.sourceHashes).some((file) => file.endsWith('/dbimpl.ts')), 'App singleton must not be evaluated');
    validateUpstream(args.upstream);
    return {
      status: 'passed', scope: 'Native repository driver smoke; no endpoint, response-builder, adapter or performance acceptance',
      ...identity, scratch, migrations: MIGRATIONS, sqlite: sqlite[0], pragmas,
      schema, tableInfo, tableXInfo, indexInfo, nativeTypes, plans, emittedQueries: queries,
      results: { single, grouped }, lifecycle, loadFailure, reopenedRows: rowsBeforeClose, upstreamClean: true,
    };
  } finally {
    if (db) await db.destroy();
    runtime.close();
  }
}

async function main() {
  const args = options(['upstream', 'runtime', 'extension', 'output'], ['scratch']);
  args.upstream = validateUpstream(args.upstream);
  const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(args.upstream, args.output) && !inside(repository, args.output), 'Evidence must be external to source checkouts');
  mkdirSync(path.dirname(args.output), { recursive: true });
  const output = existsSync(args.output) ? realpathSync(args.output) : path.join(realpathSync(path.dirname(args.output)), path.basename(args.output));
  assert(!inside(args.upstream, output) && !inside(repository, output), 'Evidence must be external to source checkouts');
  try {
    const smoke = await driverSmoke(args);
    const adapter = await adapterAcceptance(args);
    const worker = await workerAcceptance(args);
    const connection = await connectionAcceptance(args);
    const lifecycle = await lifecycleAcceptance(args);
    const evidence = { status: 'passed', driverSmoke: smoke, adapter, worker, connection, lifecycle };
    writeJson(args.output, evidence);
    console.log(JSON.stringify({ status: evidence.status, output: args.output, sqlite: smoke.sqlite.version, assertions: adapter.assertions, workerAssertions: worker.assertions, connectionAssertions: connection.assertions, lifecycleAssertions: lifecycle.assertions, mutations: [...adapter.mutations, ...worker.mutations, ...connection.mutations, ...lifecycle.mutations].map(({ mutation, status }) => ({ mutation, status })) }));
  } catch (error) {
    writeJson(args.output, { status: 'failed', error: error.stack });
    throw error;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) await main();
