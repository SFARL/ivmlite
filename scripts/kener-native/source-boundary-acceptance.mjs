import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { installAdapter, VIEW, createReader } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';
import { loadRuntime } from './runtime.mjs';
import { loadSourceBoundary } from './source-boundary.mjs';
import { validateUpstream } from './prepare.mjs';

const fields = ['monitor_tag', 'timestamp', 'status', 'latency', 'type', 'error_message', 'raw_status'];
const sample = (tag = 'api', ts = 901, latency = 1.25, status = 'UP') => ({ monitor_tag: tag, timestamp: ts, status, latency, type: 'REALTIME', error_message: null, raw_status: status });
const result = ({ status, latency, type, error_message, raw_status }) => ({ status, latency, type, error_message, raw_status });
const canonical = (rows) => rows.map((row) => JSON.stringify(row)).sort();
function expectedView(rows) {
  const groups = new Map();
  for (const row of rows) {
    const bucket_ts = Math.floor(row.timestamp / 900) * 900;
    const key = JSON.stringify([bucket_ts, row.monitor_tag, row.status]);
    if (!groups.has(key)) groups.set(key, { bucket_ts, monitor_tag: row.monitor_tag, status: row.status, sample_count: 0, latency_sum: null, latency_count: 0, min_latency: null, max_latency: null });
    const group = groups.get(key); group.sample_count++;
    if (row.latency !== null) { group.latency_sum = (group.latency_sum ?? 0) + row.latency; group.latency_count++; group.min_latency = group.min_latency === null ? row.latency : Math.min(group.min_latency, row.latency); group.max_latency = group.max_latency === null ? row.latency : Math.max(group.max_latency, row.latency); }
  }
  return [...groups.values()];
}
function expectedGrouped(rows, tags, start = 900, interval = 900, points = 1) {
  const groups = new Map();
  for (const row of rows) {
    if (!tags.includes(row.monitor_tag) || row.timestamp < start || row.timestamp >= start + interval * points) continue;
    const ts = Math.floor((row.timestamp - start) / interval) * interval + start;
    const key = JSON.stringify([row.monitor_tag, ts]);
    if (!groups.has(key)) groups.set(key, { monitor_tag: row.monitor_tag, ts, countOfUp: 0, countOfDown: 0, countOfDegraded: 0, countOfMaintenance: 0, latencies: [] });
    const group = groups.get(key), count = { UP: 'countOfUp', DOWN: 'countOfDown', DEGRADED: 'countOfDegraded', MAINTENANCE: 'countOfMaintenance' }[row.status];
    if (count) group[count]++; if (row.latency !== null) group.latencies.push(row.latency);
  }
  return [...groups.values()].sort((a, b) => a.monitor_tag.localeCompare(b.monitor_tag) || a.ts - b.ts).map(({ latencies, ...row }) => ({ ...row, avgLatency: latencies.length ? latencies.reduce((a, b) => a + b, 0) / latencies.length : 0, maxLatency: latencies.length ? Math.max(...latencies) : 0, minLatency: latencies.length ? Math.min(...latencies) : 0 }));
}

// Exported runner only: acceptance.mjs owns serial execution and evidence output.
export async function sourceBoundaryAcceptance(args) {
  mkdirSync(args.scratch ?? os.tmpdir(), { recursive: true });
  const scratch = mkdtempSync(path.join(realpathSync(args.scratch ?? os.tmpdir()), 'kener-source-boundary-'));
  const database = path.join(scratch, 'source.sqlite');
  const previousDatabaseURL = process.env.DATABASE_URL;
  const runtime = await loadRuntime({ ...args, database });
  const lifecycle = [], queries = [], checks = [], publications = [], profiles = [], mutations = [];
  const assertions = { total: 0, independentBase: 0, independentView: 0, publicationSnapshots: 0, groupedComparisons: 0, loaderIsolation: 0, cleanup: 0 };
  const equal = (actual, expected, message) => { assert.deepEqual(actual, expected, message); assertions.total++; };
  const truth = (value, message) => { assert(value, message); assertions.total++; };
  const rejects = async (work, pattern) => { await assert.rejects(work, pattern); assertions.total++; };
  const throws = (work, pattern) => { assert.throws(work, pattern); assertions.total++; };
  const db = runtime.createDatabase(database, realpathSync(args.extension), lifecycle);
  const oracle = runtime.createDatabase(database, realpathSync(args.extension), lifecycle);
  const fixtures = [], committed = [];
  let intended = committed, current;
  const previousRedisURL = process.env.REDIS_URL;
  const remember = (row) => { const index = committed.findIndex((item) => item.monitor_tag === row.monitor_tag && item.timestamp === row.timestamp); if (index < 0) committed.push(row); else committed[index] = row; };
  db.on('query', ({ sql, bindings }) => { queries.push({ sql, bindings: bindings ?? [] }); if (current) current.events.push({ event: 'native-sql', sql }); });
  async function state(expected, label) {
    const base = await oracle('monitoring_data').select(fields);
    equal(canonical(base), canonical(expected), `${label}: independent committed base facts`); assertions.independentBase++;
    const view = await oracle(VIEW).select('*');
    equal(canonical(view), canonical(expectedView(expected)), `${label}: independent committed public view`); assertions.independentView++;
    return { label, base, expectedBase: structuredClone(expected), view, expectedView: expectedView(expected) };
  }
  async function fixture(options = {}) {
    const boundary = await loadSourceBoundary({ upstream: args.upstream, runtime, db, scratch,
      onPublication: async ({ key, payload, ttl }) => {
        if (!key.endsWith(':last_status')) return;
        boundary.events.push({ event: 'cache-publication-observed', key });
        const facts = await state(intended, 'publication after native outer COMMIT');
        const events = boundary.events.map((event) => ({ ...event }));
        truth(events.some(({ event, sql }) => event === 'native-sql' && /^COMMIT/.test(sql)), 'Outer COMMIT emitted before cache publication');
        truth(events.findLastIndex(({ event }) => event === 'insert-resolved-after-commit') > events.findLastIndex(({ event, sql }) => event === 'native-sql' && /^COMMIT/.test(sql)), 'DB promise resolves after outer COMMIT');
        const insertStart = events.findLastIndex(({ event }) => event === 'insert-start');
        const refresh = events.findLastIndex(({ event, sql }) => event === 'native-sql' && sql === 'SELECT ivm_refresh_all()');
        const commit = events.findLastIndex(({ event, sql }) => event === 'native-sql' && /^COMMIT/.test(sql));
        truth(insertStart < refresh && refresh < commit, 'Actual insert, native refresh and outer COMMIT order before publication');
        publications.push({ key, payload, ttl, ...facts, events }); assertions.publicationSnapshots++;
      }, ...options });
    equal(process.env.REDIS_URL, previousRedisURL, 'Factory restores scoped Redis environment before return');
    fixtures.push(boundary); current = boundary; return boundary;
  }
  async function pushProcess(boundary, row, options) { await boundary.response.push(row.monitor_tag, row.timestamp, result(row), options); return boundary.process(); }
  try {
    await runtime.migrate(db); await installAdapter(db);
    const initial = await state([], 'initial independent baseline');
    const strictReads = [];
    const boundary = await fixture({ onRead: (entry) => strictReads.push(entry) });
    const first = sample(); intended = [first]; equal(await pushProcess(boundary, first), first, 'Actual controller + repository return inserted native row'); remember(first);
    truth(boundary.events.findLastIndex(({ event }) => event === 'alert-invoked') > boundary.events.findLastIndex(({ event }) => event === 'cache-publication-observed'), 'Actual alert invocation follows cache publication');
    equal(boundary.alerts, [['api', 901, 'UP']], 'Actual processor invokes alert after cache await');
    equal(runtime.getWorkerKnex(), undefined, 'Actual q wrapper restores canonical context');
    const queue = boundary.queues[0], worker = boundary.workers[0];
    equal(queue.name, 'monitorResponseQueue');
    equal({ ...queue.options, connection: 'controlled' }, { connection: 'controlled', prefix: 'kener', defaultJobOptions: { attempts: 3, backoff: { type: 'exponential', delay: 5000 }, removeOnComplete: true, removeOnFail: false } }, 'Actual q queue defaults');
    equal({ ...worker.options, connection: 'controlled' }, { connection: 'controlled', prefix: 'kener', concurrency: 5 }, 'Actual q worker options');
    equal(boundary.jobs[0], { name: 'monitorResponseJob_api', data: { monitorTag: 'api', ts: 901, ...result(first) }, options: { deduplication: { id: 'api-901' }, removeOnComplete: { age: 300, count: 100 }, removeOnFail: { age: 86400 } } }, 'Actual response enqueue options');
    equal(boundary.commands[0], { command: 'SET', key: 'kener:cache:api:last_status', payload: JSON.stringify({ monitor_tag: 'api', timestamp: 901, status: 'UP', latency: 1.25, type: 'REALTIME' }), ex: 'EX', ttl: 86400, logicalTime: 0 }, 'Actual prefix, JSON, EX and one-day latest-status TTL');
    checks.push('actual q wrapper, response/controller insert-only transaction, refresh and outer COMMIT precede independently observed latest-status publication and alert');

    const writesBefore = boundary.commands.filter(({ command }) => command === 'SET').length, alertsBefore = boundary.alerts.length;
    await rejects(() => pushProcess(boundary, { ...sample('invalid'), timestamp: 0 }), /Invalid data/);
    await state(committed, 'controller input failure leaves committed state intact');
    equal(boundary.commands.filter(({ command }) => command === 'SET').length, writesBefore); equal(boundary.alerts.length, alertsBefore);
    // Trigger changes a separate INTEGER input within the real repository insert.
    // The INSERT succeeds; the actual native precommit refresh overflows SUM.
    await db.raw('CREATE TABLE source_failure_guard (group_id INTEGER, value INTEGER)');
    await db.raw('INSERT INTO source_failure_guard VALUES (1,9223372036854775807)');
    await db.raw("CREATE VIRTUAL TABLE zz_source_failure_view USING ivm('SELECT group_id,SUM(value) AS total FROM source_failure_guard GROUP BY group_id')");
    await db.raw("CREATE TRIGGER source_failure_insert AFTER INSERT ON monitoring_data WHEN NEW.monitor_tag = 'refresh-failure' BEGIN INSERT INTO source_failure_guard VALUES (1,1); END");
    const beforeOverflow = queries.length;
    await rejects(() => pushProcess(boundary, sample('refresh-failure')), /ivm_refresh_all: view zz_source_failure_view: integer overflow in SUM; no view was refreshed/);
    truth(queries.slice(beforeOverflow).some(({ sql }) => /^insert into ["`]monitoring_data/.test(sql)), 'Actual repository INSERT executed before overflow');
    truth(queries.slice(beforeOverflow).some(({ sql }) => sql === 'SELECT ivm_refresh_all()'), 'Overflow arises in actual refresh');
    truth(queries.slice(beforeOverflow).some(({ sql }) => /^ROLLBACK/.test(sql)), 'Native overflow rolls back');
    await state(committed, 'native refresh overflow rollback');
    equal(await db.raw('SELECT CAST(value AS TEXT) AS value FROM source_failure_guard'), [{ value: '9223372036854775807' }]);
    equal(boundary.commands.filter(({ command }) => command === 'SET').length, writesBefore); equal(boundary.alerts.length, alertsBefore);
    await db.raw('DROP TRIGGER source_failure_insert'); await db.raw('DROP TABLE zz_source_failure_view'); await db.raw('DROP TABLE source_failure_guard');
    checks.push('invalid controller input and real native precommit SUM overflow publish no cache or alerts; all native rows/view/guard roll back');

    const retry = sample('retry', 901, 2.5); intended = [...committed, retry]; boundary.failNextSet();
    await rejects(() => pushProcess(boundary, retry), /controlled Redis SET failure/); remember(retry);
    await state(committed, 'postcommit SET failure retains committed facts'); equal(boundary.alerts.length, alertsBefore);
    equal(await boundary.process(), retry, 'Explicit manual replay result'); await state(committed, 'idempotent replay');
    equal((await oracle('monitoring_data').where({ monitor_tag: 'retry' })).length, 1, 'Manual replay is a single-row upsert');
    checks.push('postcommit SET rejection leaves correct committed native state; explicit manual replay upserts one base row without simulated broker retry/dedup');

    const tags = ['retry', 'api', 'api']; const originalTags = [...tags];
    const warm = await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1);
    equal(warm, expectedGrouped(committed, tags)); assertions.groupedComparisons++; equal(tags, originalTags, 'Actual grouped key sorts a copy'); equal(strictReads.at(-1), { mode: 'strict', source: 'original-repository', fallbackReason: 'strict-json-number-parity', refresh: 'explicit-host-contract' }, 'Controller DB facade defaults to strict original reads');
    const correction = sample('api', 901, 4.5, 'DOWN'); intended = committed.map((row) => row.monitor_tag === 'api' ? correction : row);
    await pushProcess(boundary, correction); remember(correction);
    equal(await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(['api', 'retry', 'api'], 900, 900, 1), warm, 'Write does not invalidate actual grouped cached result'); assertions.groupedComparisons++;
    boundary.advance(59); equal(await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1), warm, 'Grouped stale result remains before logical 60-second expiry');
    boundary.advance(1);
    const fresh = await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1);
    equal(fresh, expectedGrouped(committed, tags)); assertions.groupedComparisons++;
    equal(fresh, await new runtime.MonitoringRepository(oracle).getStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1), 'Fresh miss equals independent native repository');
    equal(fresh, (await createReader(oracle, new runtime.MonitoringRepository(oracle), { mode: 'experimental-numeric' }).read(true, tags, 900, 900, 1)).rows, 'Fresh miss equals maintained reader for exact dyadic fixture');
    for (const dimensions of [[tags, 901, 900, 1], [tags, 900, 1800, 1], [tags, 900, 900, 2], [['api', 'retry'], 900, 900, 1]]) await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(...dimensions);
    const groupedKeys = boundary.commands.filter(({ command, key }) => command === 'SET' && key.includes('status_counts_grouped:'));
    for (const key of ['api,api,retry:900:900:1', 'api,api,retry:901:900:1', 'api,api,retry:900:1800:1', 'api,api,retry:900:900:2', 'api,retry:900:900:1']) truth(groupedKeys.some((entry) => entry.key === `kener:cache:status_counts_grouped:${key}`), `Actual grouped key dimensions ${key}`);
    truth(groupedKeys.every(({ ttl, ex, payload }) => ttl === 60 && ex === 'EX' && Array.isArray(JSON.parse(payload))), 'Actual grouped wire JSON and 60-second TTL');
    const groupedKey = 'kener:cache:status_counts_grouped:api,api,retry:900:900:1'; boundary.injectRaw(groupedKey, '{broken');
    equal(await boundary.cache.getCache('status_counts_grouped:api,api,retry:900:900:1'), null, 'Malformed JSON cache read is miss');
    equal(await boundary.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1), fresh, 'Controller refetches and repairs malformed JSON');
    await boundary.cache.setCache('default-ttl', { test: true }); equal(boundary.commands.at(-1).ttl, 300);
    await boundary.cache.deleteCache('default-ttl'); equal(await boundary.cache.getCache('default-ttl'), null, 'Actual cache DEL wire route');
    checks.push('actual grouped source cache retains correction until logical 60-second expiry; copied/sorted/duplicate/window dimensions, malformed JSON and actual wire TTLs verified');

    let releaseAlert; const pendingAlert = new Promise((resolve) => { releaseAlert = resolve; }); boundary.holdAlerts(pendingAlert);
    const newer = sample('order', 1801, 0.125); intended = [...committed, newer];
    let alertTimeout;
    try {
      await Promise.race([pushProcess(boundary, newer), new Promise((_resolve, reject) => { alertTimeout = setTimeout(() => reject(new Error('Response incorrectly awaited pending alert')), 2000); })]);
      remember(newer); equal(boundary.alerts.at(-1), ['order', 1801, 'UP'], 'Unresolved alert enqueue does not block response resolution');
    } finally { clearTimeout(alertTimeout); releaseAlert(); }
    const older = sample('order', 901, 0.5, 'DOWN'); intended = [...committed, older]; await pushProcess(boundary, older); remember(older);
    equal((await boundary.setGet.GetLastMonitoringValue('order')).timestamp, 901, 'Older processed timestamp unconditionally overwrites last-status cache');
    equal((await new runtime.MonitoringRepository(oracle).getLatestMonitoringData('order')).timestamp, 1801, 'Native latest timestamp remains newer');
    const customOptions = { deduplication: { id: 'caller-dedup' }, removeOnComplete: false, removeOnFail: false };
    await boundary.response.push('options-only', 901, result(sample()), customOptions);
    equal(boundary.jobs.at(-1).options, { deduplication: { id: 'caller-dedup' }, removeOnComplete: { age: 300, count: 100 }, removeOnFail: { age: 86400 } }, 'Actual response preserves custom dedup but overrides retention');
    await boundary.response.shutdown(); truth(worker.closed); truth(!queue.closed, 'Actual upstream shutdown leaves queue open'); truth(boundary.clients.every((client) => !client.closed), 'Actual upstream shutdown leaves Redis open');
    await boundary.response.push('restart-only', 901, result(sample())); equal(boundary.workers.length, 2); equal(boundary.queues.length, 1, 'Actual shutdown resets worker only');
    throws(() => boundary.probeImport('q', '../unreviewed.ts'), /Unreviewed source-boundary import/);
    throws(() => boundary.probeImport('not-reviewed', 'node:fs'), /Unreviewed source-boundary import/);
    await rejects(() => boundary.probeHelper('tool.GetNowTimestampUTC'), /Unreviewed source-boundary helper/);
    throws(() => boundary.probeDb('deleteMonitorDataByTag'), /Unreviewed source-boundary DB method/);
    throws(() => boundary.controller.DeleteMonitorCompletelyUsingTag, /Unreviewed source-boundary controller export/);
    throws(() => boundary.redis.redisIOConnection().flushall(), /Unreviewed source-boundary Redis command/);
    const redisOptions = boundary.clients[0].options;
    equal({ ...redisOptions, reconnectOnError: 'function' }, { maxRetriesPerRequest: null, keepAlive: 30000, maxLoadingRetryTime: 120000, reconnectOnError: 'function' }, 'Actual Redis connector options');
    equal(['READONLY', 'LOADING', 'MASTERDOWN', 'OTHER'].map((message) => redisOptions.reconnectOnError(new Error(message))), [2, 2, 2, false]);
    profiles.push({ label: 'strict', identity: boundary.identity(), commands: structuredClone(boundary.commands), reads: strictReads, alerts: structuredClone(boundary.alerts), jobs: structuredClone(boundary.jobs), queueOptions: { ...queue.options, connection: 'controlled' }, workerOptions: { ...worker.options, connection: 'controlled' }, redisOptions: { ...redisOptions, reconnectOnError: 'observed function; READONLY/LOADING/MASTERDOWN=>2, other=>false' }, events: structuredClone(boundary.events) });
    await boundary.close(); truth(boundary.queues.every(({ closed }) => closed)); truth(boundary.workers.every(({ closed }) => closed)); truth(boundary.clients.every(({ closed }) => closed)); assertions.cleanup += 3;
    equal(boundary.cleanupState(), { closed: true, fixtureBindings: [], cachedKeys: 0 }, 'Close deregisters fixture references and clears cache');
    equal(process.env.REDIS_URL, previousRedisURL, 'Close preserves caller Redis environment');
    throws(() => boundary.advance(1), /Source boundary closed/);
    checks.push('source preserves unawaited pending alert and unconditional older last-status overwrite; queue/worker/retry/dedup/retention configuration observed only; actual shutdown worker-only, fixture closes remaining resources');

    const reads = [], numeric = await fixture({ mode: 'experimental-numeric', onRead: (entry) => reads.push(entry) });
    equal(numeric.jobs, []); equal(numeric.queues, []); equal(numeric.commands, []); truth(numeric.namespace !== boundary.namespace); assertions.loaderIsolation += 4;
    equal(await numeric.setGet.GetLastMonitoringValue('api'), null, 'Successive source singleton/cache facade is isolated'); assertions.loaderIsolation++;
    equal(await numeric.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 900, 900, 1), expectedGrouped(committed, tags), 'Fresh isolated numeric cache profile');
    equal(reads.at(-1).source, 'public-ivm-view');
    await numeric.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, 901, 900, 1); equal(reads.at(-1).fallbackReason, 'unaligned-window');
    await numeric.controller.GetStatusCountsByIntervalGroupedByMonitor(tags, Number.MAX_SAFE_INTEGER, 900, 1); equal(reads.at(-1).fallbackReason, 'unsafe-or-invalid-window');
    profiles.push({ label: 'isolated-experimental-numeric', identity: numeric.identity(), commands: structuredClone(numeric.commands), reads }); await numeric.close();
    checks.push('successive namespaced source/cache/queue/facade instances isolated; strict default preserved and fresh numeric opt-in uses existing alignment/safety fallback limits');

    const mutant = await fixture({ negativeControl: 'skip-precommit-refresh' });
    const mutated = sample('mutation', 2701, 8.5); intended = [...committed, mutated];
    let redFailure;
    await rejects(async () => { try { await pushProcess(mutant, mutated); } catch (error) { redFailure = error.message; throw error; } }, /independent committed public view/);
    equal(canonical(await oracle('monitoring_data').select(fields)), canonical(intended), 'Focused mutant commits independently expected base but leaves view stale');
    equal(mutant.alerts, [], 'Focused mutant detected at cache publication before alert');
    profiles.push({ label: 'negative-control', identity: mutant.identity(), commands: structuredClone(mutant.commands), events: structuredClone(mutant.events) }); await mutant.close();
    const intact = await fixture(); intended = [...committed, mutated]; equal(await pushProcess(intact, mutated), mutated); remember(mutated); await state(committed, 'intact counterpart refreshes and commits before publication');
    profiles.push({ label: 'intact-counterpart', identity: intact.identity(), commands: structuredClone(intact.commands), events: structuredClone(intact.events) }); await intact.close();
    mutations.push({ mutation: 'skip-precommit-refresh-only', status: 'detected', redFailure, independentAssertion: 'publication after native outer COMMIT: independent committed public view', intact: 'passed' });
    checks.push('focused skipped-refresh mutant fails exact independent publication-view assertion while same source intact counterpart passes');
    await rejects(() => loadSourceBoundary({ upstream: args.upstream, runtime, db, scratch, negativeControl: 'unknown-injection' }), /Unknown source-boundary negative control/);
    const final = await state(committed, 'final independent committed facts');
    validateUpstream(args.upstream); equal(runtime.getWorkerKnex(), undefined);
    return { status: 'passed', scope: 'Pinned unchanged q/response/controller/cache/Redis connector source boundary with controlled BullMQ/Redis/alert transports and manual replay; no real broker/server/retry/dedup/wall-clock expiry/alert delivery/app-wide startup or performance acceptance', scratch, ...runtime.identity(realpathSync(args.extension)), assertions, checks, mutations, initial, final, publications, profiles, emittedQueries: queries, lifecycle, upstreamClean: true };
  } finally {
    current = undefined;
    const failures = [];
    try {
      for (const entry of await Promise.allSettled(fixtures.map((boundary) => boundary.close()))) if (entry.status === 'rejected') failures.push(entry.reason);
      for (const entry of await Promise.allSettled([oracle.destroy(), db.destroy()])) if (entry.status === 'rejected') failures.push(entry.reason);
      try { runtime.close(); } catch (error) { failures.push(error); }
    } finally {
      if (previousDatabaseURL === undefined) delete process.env.DATABASE_URL; else process.env.DATABASE_URL = previousDatabaseURL;
    }
    if (failures.length) throw new AggregateError(failures, 'Source-boundary acceptance cleanup failed');
  }
}
