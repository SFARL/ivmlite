import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadRuntime } from './runtime.mjs';
import { installAdapter, refreshAdapter, adapterTransaction, createReader, PROJECTION, VIEW, TRIGGERS } from './adapter.mjs';

const TYPES = ['REALTIME', 'TIMEOUT', 'ERROR', 'MANUAL', 'DEFAULT', 'SIGNAL', 'INCIDENT', 'MAINTENANCE'];
const TAGS = ['', '002', '2', 'api/east', '监控/é', 'null-only'];
const STATUSES = ['UP', 'DOWN', 'DEGRADED', 'MAINTENANCE', 'NO_DATA', 'UNKNOWN', null];
const baseRow = (tag, timestamp, latency = 1.25, status = 'UP', type = 'REALTIME') => ({ monitor_tag: tag, timestamp, latency, status, type, raw_status: status, error_message: null });
const orderedBase = (db) => db.raw('SELECT rowid AS source_rowid, * FROM monitoring_data ORDER BY monitor_tag,timestamp');
const orderedProjection = (db) => db.raw(`SELECT * FROM ${PROJECTION} ORDER BY source_rowid`);
const orderedView = (db) => db.raw(`SELECT bucket_ts,monitor_tag,status,sample_count,latency_sum,latency_count,min_latency,max_latency FROM ${VIEW} ORDER BY bucket_ts,monitor_tag,status`);

// Only finite unequal sums/averages receive tolerance. NaN/Infinity cannot disappear in
// subtraction or JSON serialization; identical exceptional values require Object.is.
export function compareNumeric(actual, expected, label, differences = [], location = '') {
  if (typeof actual === 'number' || typeof expected === 'number') {
    assert.equal(typeof actual, typeof expected, `${label}${location}: numeric type`);
    if (Object.is(actual, expected)) return;
    assert(Number.isFinite(actual) && Number.isFinite(expected), `${label}${location}: nonfinite mismatch`);
    assert(/\.(latency_sum|avgLatency)$/.test(location), `${label}${location}: exact numeric field mismatch`);
    const error = Math.abs(actual - expected);
    const bound = 1e-12 * Math.max(1, Math.abs(expected));
    assert(error <= bound, `${label}${location}: numeric error ${error} > ${bound}`);
    differences.push({ location, actual, expected, absoluteError: error, bound });
    return;
  }
  if (actual && expected && typeof actual === 'object' && typeof expected === 'object') {
    assert.deepEqual(Object.keys(actual), Object.keys(expected), `${label}${location}: shape/order`);
    for (const key of Object.keys(expected)) compareNumeric(actual[key], expected[key], label, differences, `${location}.${key}`);
    return;
  }
  assert.deepEqual(actual, expected, `${label}${location}`);
}

function requests() {
  const selections = ['api/east', '002', '', 'missing', [], ['api/east', '002', 'api/east', 'missing'], TAGS];
  const windows = [[-1800, 900, 14], [0, 1800, 6], [0, 900, 0], [1800, 86400, 1], [2700, 86400, 1],
    [37, 900, 8], [0, 300, 12], [0, 899, 4], [-901, 900, 4], [0, 900, -1], [0, 900, 0.5],
    [Number.MAX_SAFE_INTEGER, 900, 1]];
  return selections.flatMap((tags) => windows.map((window) => ({ tags, window })));
}

export async function adapterAcceptance(args) {
  mkdirSync(args.scratch ?? os.tmpdir(), { recursive: true });
  const scratch = mkdtempSync(path.join(realpathSync(args.scratch ?? os.tmpdir()), 'kener-adapter-'));
  const runtime = await loadRuntime({ ...args, database: path.join(scratch, 'adapter.sqlite') });
  const extension = realpathSync(args.extension);
  const Repository = runtime.MonitoringRepository;
  const lifecycle = [];
  const opened = new Set();
  function open(name) { const db = runtime.createDatabase(path.join(scratch, name), extension, lifecycle); opened.add(db); return db; }
  async function close(db) { await db.destroy(); opened.delete(db); }
  const baseline = open('baseline.sqlite');
  let db = open('adapter.sqlite');
  const batches = [], reads = { strict: 0, experimental: 0, fallbackReasons: {}, accelerated: 0 };
  const assertions = { baseComparisons: 0, projectionComparisons: 0, viewComparisons: 0, exactResultComparisons: 0 };
  const lifecycleChecks = [];
  const numericalDifferences = [];
  const emittedAdapterQueries = [];
  try {
    // A deliberately independent guard on the comparator, including both exceptional directions.
    assert.throws(() => compareNumeric(NaN, 0, 'guard'), /nonfinite/);
    assert.throws(() => compareNumeric(0, Infinity, 'guard'), /nonfinite/);
    compareNumeric(Infinity, Infinity, 'Object.is guard');
    for (const field of ['ts', 'countOfUp', 'sample_count', 'latency_count', 'minLatency', 'maxLatency', 'min_latency', 'max_latency']) {
      assert.throws(() => compareNumeric({ [field]: 1 + Number.EPSILON }, { [field]: 1 }, 'exact field guard'), /exact numeric field mismatch/);
    }
    await runtime.migrate(baseline); await runtime.migrate(db);
    // Bootstrap has real preexisting facts, including negative timestamps.
    for (const target of [baseline, db]) await new Repository(target).insertMonitoringData(baseRow('api/east', -901, 0.125));
    // Fail at the final DDL statement to prove setup/backfill/triggers roll back.
    await db.raw(`CREATE TABLE ${VIEW} (sentinel INTEGER)`);
    await assert.rejects(() => installAdapter(db), /already exists/);
    assert.deepEqual(await db.raw("SELECT name FROM sqlite_schema WHERE name = ? OR name IN (?,?,?)", [PROJECTION, ...Object.values(TRIGGERS)]), []);
    assert.deepEqual(await orderedBase(db), await orderedBase(baseline));
    await db.raw(`DROP TABLE ${VIEW}`);
    await installAdapter(db);
    lifecycleChecks.push('failed final bootstrap DDL rolls back sidecar/backfill/triggers');
    db.on('query', ({ sql, bindings }) => { if (sql.includes(`FROM ${VIEW} WHERE`)) emittedAdapterQueries.push({ sql, bindings }); });

    async function state(label, target = db, reference = baseline, exact = true) {
      assert.deepEqual(await orderedBase(target), await orderedBase(reference), `${label}: native base replay mismatch`);
      assertions.baseComparisons++;
      const expectedProjection = await target.raw(`SELECT rowid AS source_rowid,
        timestamp - ((timestamp % 900 + 900) % 900) AS bucket_ts, monitor_tag,status,latency
        FROM monitoring_data ORDER BY rowid`);
      assert.deepEqual(await orderedProjection(target), expectedProjection, `${label}: projection mismatch`);
      assertions.projectionComparisons++;
      const expectedView = await reference.raw(`SELECT timestamp - ((timestamp % 900 + 900) % 900) AS bucket_ts,
        monitor_tag,status,COUNT(*) AS sample_count,SUM(latency) AS latency_sum,COUNT(latency) AS latency_count,
        MIN(latency) AS min_latency,MAX(latency) AS max_latency FROM monitoring_data
        GROUP BY bucket_ts,monitor_tag,status ORDER BY bucket_ts,monitor_tag,status`);
      if (exact) assert.deepEqual(await orderedView(target), expectedView, `${label}: refreshed view mismatch`);
      else compareNumeric(await orderedView(target), expectedView, `${label}: refreshed view mismatch`, numericalDifferences);
      assertions.viewComparisons++;
    }
    async function resultChecks(label, exact = true) {
      const original = new Repository(baseline);
      const strict = createReader(db, new Repository(db));
      const numeric = createReader(db, new Repository(db), { mode: 'experimental-numeric' });
      for (const { tags, window } of requests()) for (const grouped of [false, true]) {
        const selected = grouped && typeof tags === 'string' ? [tags] : tags;
        const method = grouped ? 'getStatusCountsByIntervalGroupedByMonitor' : 'getStatusCountsByInterval';
        const expected = await original[method](selected, ...window);
        const strictResult = await strict.read(grouped, selected, ...window);
        assert.deepEqual(strictResult.rows, expected, `${label}: strict original result mismatch`);
        assert.equal(strictResult.provenance.source, 'original-repository'); reads.strict++;
        const actual = await numeric.read(grouped, selected, ...window);
        reads.experimental++;
        const reason = actual.provenance.fallbackReason;
        if (reason) { reads.fallbackReasons[reason] = (reads.fallbackReasons[reason] ?? 0) + 1; assert.deepEqual(actual.rows, expected, `${label}: fallback mismatch`); }
        else {
          reads.accelerated++;
          if (exact) assert.deepEqual(actual.rows, expected, `${label}: experimental dyadic result mismatch`);
          else compareNumeric(actual.rows, expected, `${label}: experimental decimal result mismatch`, numericalDifferences);
        }
        assertions.exactResultComparisons += exact || reason ? 2 : 1;
      }
    }
    async function batch(label, work, exact = true) {
      await baseline.transaction(async (trx) => work(new Repository(trx), trx));
      await adapterTransaction(db, Repository, work);
      await state(label, db, baseline, exact); await resultChecks(label, exact);
      batches.push({ label, rows: (await orderedBase(db)).length, passed: true });
    }
    await state('atomic bootstrap');
    // Roll back failed second installation without disturbing existing state.
    await assert.rejects(() => installAdapter(db), /already exists/);
    await state('duplicate bootstrap rollback');
    await batch('all native types, statuses, nulls, boundaries, sparse buckets', async (repo) => {
      const times = [-900, -1, 0, 1, 59, 60, 899, 900, 901, 1799, 1800, 2701, 7200];
      for (let tagIndex = 0; tagIndex < TAGS.length - 1; tagIndex++) for (let i = 0; i < times.length; i++) {
        await repo.insertMonitoringData(baseRow(TAGS[tagIndex], times[i], i % 5 === 0 ? null : (i - 5) * 0.125,
          STATUSES[(i + tagIndex) % STATUSES.length], TYPES[i % TYPES.length]));
      }
      await repo.insertMonitoringData(baseRow('null-only', 9001, null, null, 'SIGNAL'));
      await repo.insertMonitoringData(baseRow('null-only', 9002, null, 'UNKNOWN', 'MANUAL'));
    });
    const storageClasses = await db.raw(`SELECT DISTINCT typeof(monitor_tag) AS tag,typeof(status) AS status,typeof(latency) AS latency,typeof(bucket_ts) AS bucket FROM ${PROJECTION}`);
    assert(storageClasses.some((row) => row.latency === 'real') && storageClasses.some((row) => row.latency === 'null'));
    assert(storageClasses.every((row) => row.tag === 'text' && row.bucket === 'integer'));
    await batch('upsert late correction and extreme insertion', async (repo) => {
      await repo.insertMonitoringData(baseRow('api/east', 1, 8192, 'DOWN', 'INCIDENT'));
      await repo.insertMonitoringData(baseRow('api/east', 2, -8192, 'DEGRADED', 'TIMEOUT'));
      await repo.insertMonitoringData({ ...baseRow('api/east', 899, 0.5, 'UP', 'ERROR'), raw_status: 'DOWN', error_message: 'failure' });
      await repo.insertMonitoringData({ ...baseRow('api/east', 900, 0.25, 'UP', 'REALTIME'), raw_status: 'DEGRADED' });
    });
    await batch('inclusive range update with deviation zero', (repo) => repo.updateMonitoringData('api/east', 840, 960, 'MAINTENANCE', 'MAINTENANCE', 2.25, 0));
    assert.deepEqual((await db('monitoring_data').where('monitor_tag', 'api/east').whereBetween('timestamp', [840, 960]).orderBy('timestamp')).map((row) => row.timestamp), [840, 899, 900, 901, 960]);
    await batch('confirmed unhealthy backfill', (repo) => repo.backfillConfirmedStatus('api/east', [899, 900], 2));
    await batch('prepare recovery', async (repo) => {
      await repo.insertMonitoringData({ ...baseRow('api/east', 899, 0.5, 'DOWN', 'ERROR'), raw_status: 'UP', error_message: 'held error' });
    });
    await batch('confirmed recovery backfill', (repo) => repo.backfillConfirmedStatus('api/east', [899], null));
    // The original repository has no key-change method; this boundary probe is
    // additional SQL after the complete original-method replay, not a substitute.
    await batch('native rowid and composite key change trigger probe', (_repo, trx) => trx('monitoring_data').where({ monitor_tag: 'api/east', timestamp: 2 }).update({ rowid: 20000, monitor_tag: '监控/é', timestamp: 3599 }));
    await batch('filtered inclusive extreme removal', (repo) => repo.deleteMonitorDataByTag('api/east', 1, 1, 'DOWN'));
    await batch('open ended lower deletion', (repo) => repo.deleteMonitorDataByTag('002', 1800));
    await batch('open ended upper deletion', (repo) => repo.deleteMonitorDataByTag('2', undefined, 60));

    // Reads inside a transaction remain stale until the explicit refresh. Strict
    // mode always observes the original fresh base even during that interval.
    await baseline.transaction(async (trx) => new Repository(trx).insertMonitoringData(baseRow('tx', 3601, 3.5)));
    await db.transaction(async (trx) => {
      const repo = new Repository(trx); const numeric = createReader(trx, repo, { mode: 'experimental-numeric' });
      await repo.insertMonitoringData(baseRow('tx', 3601, 3.5));
      assert.deepEqual(await numeric.getStatusCountsByInterval('tx', 3600, 900, 1), []);
      assert.equal((await createReader(trx, repo).getStatusCountsByInterval('tx', 3600, 900, 1))[0].avgLatency, 3.5);
      await refreshAdapter(trx);
      assert.deepEqual(await numeric.getStatusCountsByInterval('tx', 3600, 900, 1), await repo.getStatusCountsByInterval('tx', 3600, 900, 1));
      const after = await orderedView(trx); await refreshAdapter(trx); assert.deepEqual(await orderedView(trx), after);
    });
    await state('transaction before/after and repeated refresh'); lifecycleChecks.push('stale-before-refresh, strict-fresh-base, same-transaction visibility, repeated refresh');
    const beforeRollback = { base: await orderedBase(db), projection: await orderedProjection(db), view: await orderedView(db) };
    await assert.rejects(() => adapterTransaction(db, Repository, async (repo, trx) => {
      await repo.insertMonitoringData(baseRow('rollback', 100)); await refreshAdapter(trx); throw new Error('caller rollback');
    }), /caller rollback/);
    assert.deepEqual({ base: await orderedBase(db), projection: await orderedProjection(db), view: await orderedView(db) }, beforeRollback);
    await state('caller throw rollback'); lifecycleChecks.push('caller exception after refresh rolls back base/projection/IVM');
    await baseline.transaction(async (trx) => new Repository(trx).insertMonitoringData(baseRow('savepoint-keep', 4500, 4.5)));
    await adapterTransaction(db, Repository, async (repo, trx) => {
      await assert.rejects(() => trx.transaction(async (nested) => {
        await new Repository(nested).insertMonitoringData(baseRow('savepoint-drop', 4500));
        await refreshAdapter(nested); throw new Error('savepoint rollback');
      }), /savepoint rollback/);
      await repo.insertMonitoringData(baseRow('savepoint-keep', 4500, 4.5));
    });
    await state('nested savepoint rollback refresh then continued commit'); lifecycleChecks.push('nested savepoint refresh rollback, continued write, refresh and outer commit');

    await db.raw('CREATE TABLE kener_failure_guard (group_id INTEGER, value INTEGER)');
    await db.raw('INSERT INTO kener_failure_guard VALUES (1,9223372036854775807)');
    await db.raw("CREATE VIRTUAL TABLE zz_kener_failure_view USING ivm('SELECT group_id,SUM(value) AS total FROM kener_failure_guard GROUP BY group_id')");
    const beforeFailure = { base: await orderedBase(db), projection: await orderedProjection(db), view: await orderedView(db) };
    let refreshFailure;
    let forcedRefreshError;
    await assert.rejects(() => adapterTransaction(db, Repository, async (repo, trx) => {
      await repo.insertMonitoringData(baseRow('failed-refresh', 4501));
      await trx.raw('INSERT INTO kener_failure_guard VALUES (1,1)');
      try { await refreshAdapter(trx); }
      catch (error) {
        forcedRefreshError = error;
        assert.match(error.message, /ivm_refresh_all: view zz_kener_failure_view: integer overflow in SUM; no view was refreshed/);
        assert.deepEqual(await orderedView(trx), beforeFailure.view, 'refresh_all atomicity: earlier adapter refresh must be undone');
        assert.equal((await trx('monitoring_data').where('monitor_tag', 'failed-refresh')).length, 1, 'Base write is still uncommitted before host rollback');
        assert.equal((await trx(PROJECTION).where('monitor_tag', 'failed-refresh')).length, 1, 'Projection write is still uncommitted before host rollback');
        throw error;
      }
      assert.fail('Overflow fixture must fail refresh');
    }), (error) => { refreshFailure = error.message; return forcedRefreshError !== undefined && error === forcedRefreshError; });
    assert.deepEqual({ base: await orderedBase(db), projection: await orderedProjection(db), view: await orderedView(db) }, beforeFailure);
    assert.equal((await db.raw('SELECT COUNT(*) AS n FROM kener_failure_guard'))[0].n, 1);
    // Demonstrate the rolled-back IVM pending state can refresh cleanly afterwards.
    await refreshAdapter(db); await state('forced atomic refresh_all failure rollback');
    await db.raw('DROP TABLE zz_kener_failure_view'); await db.raw('DROP TABLE kener_failure_guard');
    lifecycleChecks.push('real INTEGER SUM overflow aborts refresh_all, outer transaction rollback, clean subsequent refresh');

    await close(db); db = open('adapter.sqlite');
    await state('reopen without bootstrap'); await resultChecks('reopen');
    await refreshAdapter(db); await state('refresh after reopen');
    lifecycleChecks.push('extension reloaded on reopen, persisted projection/view, no rebootstrap');

    // Decimal fixtures intentionally demonstrate why strict raw mode is default.
    await batch('decimal and rounding boundary fixtures', async (repo) => {
      const fixtures = { decimal: [0.1, 0.2, 0.3], rounding: [0.1, 0.2, 0.3, 0.4, 0.5], boundary: [1.005, 1.015, 2.675] };
      for (const [tag, values] of Object.entries(fixtures)) for (let i = 0; i < values.length; i++) await repo.insertMonitoringData(baseRow(tag, i + 10, values[i], i < 2 ? 'UP' : 'DOWN'));
    }, false);
    const decimalReport = [];
    for (const tag of ['decimal', 'rounding', 'boundary']) {
      const expected = await new Repository(baseline).getStatusCountsByInterval(tag, 0, 900, 1);
      const strict = await createReader(db, new Repository(db)).getStatusCountsByInterval(tag, 0, 900, 1);
      const experimental = await createReader(db, new Repository(db), { mode: 'experimental-numeric' }).getStatusCountsByInterval(tag, 0, 900, 1);
      assert.deepEqual(strict, expected);
      const differences = []; compareNumeric(experimental, expected, `decimal ${tag}`, differences);
      decimalReport.push({ tag, original: expected, strict, experimental, exact: differences.length === 0, differences });
    }
    assert(decimalReport.some((item) => !item.exact), 'Decimal fixture must expose an actual exact JSON number difference');
    await batch('all tag status filtered deletion', (repo) => repo.deleteMonitorDataByTag(undefined, undefined, undefined, 'DOWN'), false);
    await batch('all monitor range deletion', (repo) => repo.deleteMonitorDataByTag(undefined, -900, -1), false);
    // Freeze a chosen clock away from a 900-second boundary for fixture placement;
    // original background() runs twice without minute-rollover races.
    const OriginalDate = globalThis.Date;
    const frozenNow = OriginalDate.UTC(2026, 9, 22, 12, 7, 23);
    const cutoff = Math.floor(frozenNow / 60000) * 60 - 86400;
    await batch('retention cutoff fixtures', async (repo) => {
      for (const timestamp of [cutoff - 1, cutoff, cutoff + 1]) await repo.insertMonitoringData(baseRow('retention', timestamp, 0.25));
    }, false);
    let retentionResult;
    globalThis.Date = class extends OriginalDate {
      constructor(...values) { super(...(values.length ? values : [frozenNow])); }
      static now() { return frozenNow; }
    };
    try { await batch('actual background retention less-than cutoff', async (repo) => { retentionResult = await repo.background(1); }, false); }
    finally { globalThis.Date = OriginalDate; }
    assert.deepEqual((await db('monitoring_data').where('monitor_tag', 'retention').orderBy('timestamp')).map((row) => row.timestamp), [cutoff, cutoff + 1]);
    await batch('unfiltered all data deletion', (repo) => repo.deleteMonitorDataByTag());

    // Focused mutation controls exist only in the harness. The exact production
    // adapter is installed; dropping its actual triggers/omitting its refresh is
    // sufficient to prove these assertions detect each semantic failure.
    const mutations = [];
    for (const kind of ['update', 'delete', 'refresh']) {
      const target = open(`mutation-${kind}.sqlite`); await runtime.migrate(target);
      await new Repository(target).insertMonitoringData(baseRow('mutation', 1)); await installAdapter(target);
      let failure;
      if (kind !== 'refresh') await target.raw(`DROP TRIGGER ${TRIGGERS[kind]}`);
      await target.transaction(async (trx) => {
        const repo = new Repository(trx);
        if (kind === 'delete') await repo.deleteMonitorDataByTag('mutation');
        else await repo.insertMonitoringData(baseRow('mutation', 1, 8.5, 'DOWN'));
        if (kind !== 'refresh') await refreshAdapter(trx);
      });
      try {
        const projection = await target.raw(`SELECT rowid AS source_rowid,timestamp - ((timestamp % 900 + 900) % 900) AS bucket_ts,monitor_tag,status,latency FROM monitoring_data ORDER BY rowid`);
        if (kind !== 'refresh') assert.deepEqual(await orderedProjection(target), projection, `${kind} mutation: projection mismatch`);
        const original = new Repository(target);
        assert.deepEqual(await createReader(target, original, { mode: 'experimental-numeric' }).getStatusCountsByInterval('mutation', 0, 900, 1), await original.getStatusCountsByInterval('mutation', 0, 900, 1), 'refresh mutation: refreshed result mismatch');
      } catch (error) { failure = error.message; }
      assert(failure?.includes(kind === 'refresh' ? 'refreshed result mismatch' : 'projection mismatch'), `${kind} mutation must fail the intended semantic assertion`);
      mutations.push({ mutation: kind === 'refresh' ? 'skip refresh' : `disable ${kind.toUpperCase()} trigger`, status: 'killed', failure });
      await close(target);
      // Green counterpart uses the same operation and checks with the adapter intact.
      const green = open(`green-${kind}.sqlite`); await runtime.migrate(green);
      await new Repository(green).insertMonitoringData(baseRow('mutation', 1)); await installAdapter(green);
      await adapterTransaction(green, Repository, (repo) => kind === 'delete' ? repo.deleteMonitorDataByTag('mutation') : repo.insertMonitoringData(baseRow('mutation', 1, 8.5, 'DOWN')));
      const expectedProjection = await green.raw(`SELECT rowid AS source_rowid,timestamp - ((timestamp % 900 + 900) % 900) AS bucket_ts,monitor_tag,status,latency FROM monitoring_data ORDER BY rowid`);
      assert.deepEqual(await orderedProjection(green), expectedProjection);
      const repo = new Repository(green);
      assert.deepEqual(await createReader(green, repo, { mode: 'experimental-numeric' }).getStatusCountsByInterval('mutation', 0, 900, 1), await repo.getStatusCountsByInterval('mutation', 0, 900, 1));
      mutations.at(-1).green = 'passed'; await close(green);
    }
    const sampleQuery = emittedAdapterQueries[0];
    const adapterPlan = sampleQuery ? await db.raw(`EXPLAIN QUERY PLAN ${sampleQuery.sql}`, sampleQuery.bindings) : [];
    // Keep tag membership in ordinary SQL: a pushed IN equality makes SQLite
    // invoke xFilter per TEXT tag and repeat its coercion-safe candidate scan.
    // Existing result checks exercise both methods and all native TEXT fixtures.
    const plannedQueries = [false, true].map((grouped) => emittedAdapterQueries.find(({ sql, bindings }) =>
      sql.includes(grouped ? 'SELECT monitor_tag,' : 'SELECT \n') && bindings.includes('002') && bindings.includes('api/east')));
    const tagMembershipPlans = [];
    for (const query of plannedQueries) {
      assert(query, 'Both methods must execute batched TEXT membership');
      assert(query.sql.includes('AND +monitor_tag IN ('), 'Tag membership must stay in outer SQL');
      const plan = await db.raw(`EXPLAIN QUERY PLAN ${query.sql}`, query.bindings);
      const virtual = plan.find((row) => row.detail.includes('VIRTUAL TABLE'));
      assert(virtual?.detail.includes('0=ge,0=lt') && !virtual.detail.includes('1=eq'), 'Only numeric window constraints may enter xFilter');
      tagMembershipPlans.push({ query, plan });
    }
    return { status: 'passed', scope: 'SQLite repository adapter only; no endpoint/UI/response-builder or performance acceptance',
      identity: runtime.identity(extension), scratch, batches, assertions, reads, storageClasses, lifecycleChecks, refreshFailure,
      decimalReport, numericalDifferences, numericContract: 'Finite fixtures only; only latency_sum and avgLatency allow 1e-12 * max(1,abs(expected)); timestamps/counts/extrema exact; no general REAL parity guarantee',
      retention: { frozenNow, cutoff, deleted: retentionResult }, mutations, adapterPlan, tagMembershipPlans, sampleAdapterQuery: sampleQuery, lifecycle };
  } finally { for (const connection of opened) await connection.destroy(); runtime.close(); }
}
