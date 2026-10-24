import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, realpathSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inside, options, readJson, sha256, validateUpstream, writeJson } from './prepare.mjs';
import { COVERING_INDEX, loadRuntime } from './runtime.mjs';
import { createReader, installAdapter, refreshAdapter, VIEW, PROJECTION } from './adapter.mjs';

const START = 1767225600; // 2026-01-01 UTC; aligned to day and 900 seconds.
const DAY = 86400;
const SEED = 20261022;
const TAGS = ['api/east', '002', '2', '监控/é', ...Array.from({ length: 28 }, (_, i) => `service/${String(i).padStart(2, '0')}`)];
const FIELDS = 'monitor_tag,timestamp,status,latency,type,error_message,raw_status';
const METHOD = ['getStatusCountsByInterval', 'getStatusCountsByIntervalGroupedByMonitor'];
const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
async function timed(work) { const start = performance.now(); const value = await work(); return { ms: performance.now() - start, value }; }

// Stateless seeded integer mixer: changing loop order cannot change the data.
function sample(tagIndex, minute) {
  let mixed = (Math.imul(minute + 1, 1664525) + Math.imul(tagIndex + SEED, 1013904223)) >>> 0;
  mixed = Math.imul(mixed ^ (mixed >>> 16), 2246822519) >>> 0;
  const statusCode = mixed % 1000;
  const status = statusCode < 985 ? 'UP' : ['DOWN', 'DEGRADED', 'MAINTENANCE', 'UNKNOWN', null][statusCode % 5];
  return { monitor_tag: TAGS[tagIndex], timestamp: START + minute * 60, status,
    latency: mixed % 97 === 0 ? null : (mixed % 1600) / 8, type: 'REALTIME', error_message: null, raw_status: status };
}

async function native(db, work) {
  const connection = await db.client.acquireConnection();
  try { return work(connection); } finally { await db.client.releaseConnection(connection); }
}
async function baseIdentity(db) {
  return native(db, (connection) => {
    const hash = createHash('sha256'); let count = 0;
    for (const row of connection.prepare(`SELECT ${FIELDS} FROM monitoring_data ORDER BY monitor_tag,timestamp`).iterate()) {
      hash.update(JSON.stringify(row)); hash.update('\n'); count++;
    }
    return { rows: count, sha256: hash.digest('hex') };
  });
}
async function storage(db, filename) {
  const tables = await db.raw('SELECT name,tbl_name,type FROM sqlite_schema');
  const owners = new Map(tables.map((row) => [row.name, row.tbl_name]));
  // dbstat is allocation diagnostics only. No shadow contents are queried.
  const allocations = await db.raw('SELECT name,COUNT(*) AS pages,SUM(pgsize) AS bytes FROM dbstat GROUP BY name ORDER BY name');
  const categories = { base: 0, projection: 0, ivmDiagnosticState: 0, other: 0 };
  for (const row of allocations) {
    const owner = owners.get(row.name) ?? row.name;
    const category = owner === 'monitoring_data' ? 'base' : owner === PROJECTION ? 'projection' : owner.startsWith('__ivm_') ? 'ivmDiagnosticState' : 'other';
    categories[category] += row.bytes;
  }
  assert.equal(Object.values(categories).reduce((sum, value) => sum + value, 0), allocations.reduce((sum, row) => sum + row.bytes, 0));
  return { fileBytes: statSync(filename).size, pageCount: await db.raw('PRAGMA page_count'), freelist: await db.raw('PRAGMA freelist_count'), categories, allocations };
}
async function pragmas(db) {
  return Object.fromEntries(await Promise.all(['journal_mode', 'synchronous', 'cache_size', 'page_size', 'temp_store', 'mmap_size'].map(async (name) => [name, await db.raw(`PRAGMA ${name}`)])));
}
function cases(days) {
  return [
    { name: 'one-full-history', grouped: false, tags: TAGS[0], window: [START, DAY, days] },
    { name: 'one-recent-day', grouped: false, tags: TAGS[0], window: [START + (days - 1) * DAY, 900, 96] },
    { name: 'batch-merged-full-history', grouped: false, tags: TAGS, window: [START, DAY, days] },
    { name: 'batch-grouped-full-history', grouped: true, tags: TAGS, window: [START, DAY, days] },
    { name: 'batch-grouped-recent-day', grouped: true, tags: TAGS, window: [START + (days - 1) * DAY, 900, 96] },
  ];
}

export async function benchmark(args) {
  const config = args.config ? readJson(args.config) : {};
  assert(Object.keys(config).every((key) => ['days', 'repeats', 'rounds', 'compareTagPushdown'].includes(key)), 'Unknown benchmark setting');
  const { days = 90, repeats = 3, rounds = 3, compareTagPushdown = false } = config;
  assert(Number.isInteger(days) && days >= 2 && days <= 90);
  assert(Number.isInteger(repeats) && repeats >= 1 && repeats <= 3);
  assert(Number.isInteger(rounds) && rounds >= 3 && rounds <= 10);
  assert(typeof compareTagPushdown === 'boolean');
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  mkdirSync(args.scratch, { recursive: true });
  const parent = realpathSync(args.scratch);
  assert(!inside(repositoryRoot, parent) && !inside(realpathSync(args.upstream), parent), 'Scratch must be outside source checkouts');
  const scratch = mkdtempSync(path.join(parent, 'kener-benchmark-'));
  const template = path.join(scratch, 'template.sqlite');
  const runtime = await loadRuntime({ ...args, database: template });
  const extension = realpathSync(args.extension);
  const opened = new Set();
  const open = (filename) => { const db = runtime.createDatabase(filename, extension); opened.add(db); return db; };
  const close = async (db) => { await db.destroy(); opened.delete(db); };
  const evidence = { status: 'running', scratch, config: { days, repeats, rounds, compareTagPushdown },
    host: { platform: os.platform(), release: os.release(), arch: os.arch(), cpu: os.cpus()[0]?.model, logicalCPUs: os.cpus().length, totalMemory: os.totalmem() },
    scope: 'Warm native repository methods; exact dyadic fixture only; no endpoint/cache/UI/adoption claim',
    generator: { seed: SEED, tags: TAGS, start: START, minuteRows: TAGS.length * days * 1440, signalRows: 0,
      note: 'One REALTIME row per minute per tag; no SIGNAL rows in performance fixture (SIGNAL covered by acceptance). Null and dyadic REAL latency; status mixer includes unknown and null.' },
    cacheBoundary: 'Seeded migration-created template closed before independent baseline/adapter copies. Setup and copying excluded from read/write timers. No OS cache eviction. Every case/path warmed once before alternating timed rounds. Fresh database copies per repeat, shared deterministic template and OS cache.',
    setup: {}, repeats: [] };
  const checkpoint = () => writeJson(args.output, evidence);
  try {
    let db = open(template);
    evidence.setup.migrationMs = (await timed(() => runtime.migrate(db))).ms;
    const counts = {};
    const seeded = await timed(() => native(db, (connection) => {
      const insert = connection.prepare(`INSERT INTO monitoring_data (${FIELDS}) VALUES (@monitor_tag,@timestamp,@status,@latency,@type,@error_message,@raw_status)`);
      connection.transaction(() => {
        for (let tag = 0; tag < TAGS.length; tag++) for (let minute = 0; minute < days * 1440; minute++) {
          const row = sample(tag, minute); insert.run(row);
          counts[String(row.status)] = (counts[String(row.status)] ?? 0) + 1;
        }
      })();
    }));
    evidence.setup.seedMs = seeded.ms; evidence.generator.statusCounts = counts;
    evidence.setup.baseIdentity = await baseIdentity(db);
    assert.equal(evidence.setup.baseIdentity.rows, evidence.generator.minuteRows);
    evidence.setup.pragmas = await pragmas(db);
    evidence.sqlite = (await db.raw('SELECT sqlite_version() AS version,sqlite_source_id() AS sourceID'))[0];
    evidence.compileOptions = await db.raw('PRAGMA compile_options');
    evidence.setup.schema = await db.raw("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type,name");
    assert(evidence.setup.schema.some((row) => row.name === COVERING_INDEX));
    evidence.setup.storage = await storage(db, template);
    await close(db); checkpoint();
    console.log(JSON.stringify({ phase: 'template-ready', days, rows: evidence.generator.minuteRows, seedMs: seeded.ms }));

    for (let repeat = 0; repeat < repeats; repeat++) {
      const baselineFile = path.join(scratch, `baseline-${repeat}.sqlite`);
      const adapterFile = path.join(scratch, `adapter-${repeat}.sqlite`);
      const copy = await timed(() => { copyFileSync(template, baselineFile); copyFileSync(template, adapterFile); });
      const baseline = open(baselineFile), adapter = open(adapterFile);
      const result = { repeat, copyMs: copy.ms, reads: [], writes: [] };
      evidence.repeats.push(result);
      result.beforeSetupStorage = await storage(adapter, adapterFile);
      result.installMs = (await timed(() => installAdapter(adapter))).ms;
      result.afterSetupStorage = await storage(adapter, adapterFile);
      result.groups = (await adapter.raw(`SELECT COUNT(*) AS count FROM ${VIEW}`))[0].count;
      result.pragmas = { baseline: await pragmas(baseline), adapter: await pragmas(adapter) };
      const original = new runtime.MonitoringRepository(baseline);
      const strict = createReader(adapter, new runtime.MonitoringRepository(adapter));
      const numeric = createReader(adapter, new runtime.MonitoringRepository(adapter), { mode: 'experimental-numeric' });
      for (const testCase of cases(days)) {
        const method = METHOD[Number(testCase.grouped)];
        const request = [testCase.tags, ...testCase.window];
        const expected = await original[method](...request);
        const paths = {
          original: () => original[method](...request), strict: () => strict[method](...request),
          experimental: () => numeric[method](...request),
        };
        const queries = {};
        async function capture(name, target, work) {
          const listener = ({ sql, bindings }) => { if (/SUM\(CASE WHEN status/.test(sql)) queries[name] = { sql, bindings }; };
          target.on('query', listener); try { assert.deepEqual(await work(), expected); } finally { target.removeListener('query', listener); }
        }
        await capture('original', baseline, paths.original);
        await capture('experimental', adapter, paths.experimental);
        for (const [name, query] of Object.entries(queries)) {
          query.plan = await (name === 'original' ? baseline : adapter).raw(`EXPLAIN QUERY PLAN ${query.sql}`, query.bindings);
          query.vdbe = await (name === 'original' ? baseline : adapter).raw(`EXPLAIN ${query.sql}`, query.bindings);
        }
        assert(queries.original.plan.some((row) => row.detail.includes(`COVERING INDEX ${COVERING_INDEX}`)));
        if (compareTagPushdown) {
          const query = queries.experimental;
          const plus = query.sql.replace(/AND monitor_tag IN/, 'AND +monitor_tag IN');
          const plain = query.sql.replace(/AND \+monitor_tag IN/, 'AND monitor_tag IN');
          queries.plusTag = { sql: plus, bindings: query.bindings, plan: await adapter.raw(`EXPLAIN QUERY PLAN ${plus}`, query.bindings), vdbe: await adapter.raw(`EXPLAIN ${plus}`, query.bindings) };
          queries.plainTag = { sql: plain, bindings: query.bindings, plan: await adapter.raw(`EXPLAIN QUERY PLAN ${plain}`, query.bindings), vdbe: await adapter.raw(`EXPLAIN ${plain}`, query.bindings) };
          // Public view raw SQL comparison removes identical row-mapping overhead.
          paths.plusTag = () => adapter.raw(plus, query.bindings);
          paths.plainTag = () => adapter.raw(plain, query.bindings);
          assert.deepEqual(await paths.plusTag(), await paths.plainTag());
        }
        const raw = Object.fromEntries(Object.keys(paths).map((name) => [name, []]));
        for (const [name, work] of Object.entries(paths)) {
          const warm = await work(); if (!name.endsWith('Tag')) assert.deepEqual(warm, expected);
        }
        for (let round = 0; round < rounds; round++) {
          const names = Object.keys(paths); if ((round + repeat) % 2) names.reverse();
          for (const name of names) {
            const measurement = await timed(paths[name]); raw[name].push(measurement.ms);
            if (!name.endsWith('Tag')) assert.deepEqual(measurement.value, expected, `${testCase.name}/${name}`);
          }
        }
        const noopRefresh = [];
        for (let round = 0; round < rounds; round++) noopRefresh.push((await timed(() => refreshAdapter(adapter))).ms);
        result.reads.push({ ...testCase, resultRows: expected.length, exact: true, queries: repeat === 0 ? queries : undefined,
          roundsMs: raw, medianMs: Object.fromEntries(Object.entries(raw).map(([name, values]) => [name, median(values)])), noopRefreshMs: noopRefresh });
      }

      const newRow = { ...sample(0, days * 1440), status: 'DOWN', latency: 5.125, raw_status: 'DOWN' };
      const corrected = { ...sample(0, 60), status: 'DOWN', latency: 9.5, raw_status: 'UP', type: 'REALTIME', error_message: 'late correction; held pending recovery' };
      const rangeStart = START + (days - 1) * DAY;
      const batches = [
        { name: 'late-upsert-and-new-row', expected: [corrected, newRow], work: async (repo) => { await repo.insertMonitoringData(corrected); await repo.insertMonitoringData(newRow); } },
        { name: 'inclusive-range-update', expected: Array.from({ length: 15 }, (_, i) => ({ ...sample(1, (days - 1) * 1440 + i), status: 'MAINTENANCE', type: 'MAINTENANCE', latency: 0.5 })),
          work: (repo) => repo.updateMonitoringData(TAGS[1], rangeStart, rangeStart + 14 * 60, 'MAINTENANCE', 'MAINTENANCE', 0.5, 0) },
        { name: 'confirmation-recovery', expected: [{ ...corrected, status: corrected.raw_status, error_message: null }],
          work: (repo) => repo.backfillConfirmedStatus(TAGS[0], [corrected.timestamp], null) },
      ];
      const readCase = cases(days).at(-1);
      const readRequest = [readCase.tags, ...readCase.window];
      // The timed recent-day request intentionally has a bounded window. A
      // separate untimed audit must also observe the old correction and the
      // new row beyond that window, rather than accepting two unchanged reads.
      const auditRequest = [TAGS, START, DAY, days + 1];
      for (const batch of batches) {
        const expectedRows = batch.expected;
        const perPath = {};
        const order = repeat % 2 ? ['adapter', 'baseline'] : ['baseline', 'adapter'];
        for (const name of order) {
          const target = name === 'baseline' ? baseline : adapter;
          let writeMs, refreshMs = 0, readMs;
          const endToEnd = await timed(async () => {
            await target.transaction(async (trx) => {
              writeMs = (await timed(() => batch.work(new runtime.MonitoringRepository(trx)))).ms;
              if (name === 'adapter') refreshMs = (await timed(() => refreshAdapter(trx))).ms;
            });
            const reader = name === 'baseline' ? original : numeric;
            const read = await timed(() => reader[METHOD[1]](...readRequest)); readMs = read.ms; return read.value;
          });
          perPath[name] = { writeMs, refreshMs, readMs, endToEndMs: endToEnd.ms, rows: endToEnd.value };
          for (const row of expectedRows) assert.deepEqual(await target('monitoring_data').where({ monitor_tag: row.monitor_tag, timestamp: row.timestamp }).first(), row, `${batch.name}/${name}: independent expected changed row`);
        }
        assert.deepEqual(perPath.adapter.rows, perPath.baseline.rows, `${batch.name}: result parity`);
        const auditRows = await original[METHOD[1]](...auditRequest);
        assert.deepEqual(await numeric[METHOD[1]](...auditRequest), auditRows, `${batch.name}: changed aggregate windows`);
        delete perPath.adapter.rows; delete perPath.baseline.rows;
        const identity = { baseline: await baseIdentity(baseline), adapter: await baseIdentity(adapter) };
        assert.deepEqual(identity.adapter, identity.baseline);
        result.writes.push({ name: batch.name, independentExpectedRows: expectedRows, identity,
          aggregateAudit: { request: auditRequest, resultRows: auditRows.length, exact: true }, paths: perPath });
      }

      const OriginalDate = globalThis.Date;
      const frozenNow = (START + (days + 1) * DAY) * 1000;
      const cutoff = START + DAY;
      globalThis.Date = class extends OriginalDate {
        constructor(...values) { super(...(values.length ? values : [frozenNow])); }
        static now() { return frozenNow; }
      };
      const retention = { name: 'actual-background-one-day-expiry', frozenNow, retentionDays: days, cutoff, expectedDeleted: TAGS.length * 1440, paths: {} };
      try {
        for (const name of repeat % 2 ? ['adapter', 'baseline'] : ['baseline', 'adapter']) {
          const target = name === 'baseline' ? baseline : adapter;
          let deleted, writeMs, refreshMs = 0;
          const total = await timed(() => target.transaction(async (trx) => {
            const write = await timed(() => new runtime.MonitoringRepository(trx).background(days)); deleted = write.value; writeMs = write.ms;
            if (name === 'adapter') refreshMs = (await timed(() => refreshAdapter(trx))).ms;
          }));
          assert.equal(deleted, retention.expectedDeleted);
          assert.equal((await target.raw('SELECT COUNT(*) AS n FROM monitoring_data WHERE timestamp < ?', [cutoff]))[0].n, 0);
          assert.equal((await target.raw('SELECT COUNT(*) AS n FROM monitoring_data WHERE timestamp = ?', [cutoff]))[0].n, TAGS.length);
          retention.paths[name] = { deleted, writeMs, refreshMs, transactionMs: total.ms };
        }
      } finally { globalThis.Date = OriginalDate; }
      assert.deepEqual(await numeric[METHOD[1]](...readRequest), await original[METHOD[1]](...readRequest));
      const auditRows = await original[METHOD[1]](...auditRequest);
      assert.deepEqual(await numeric[METHOD[1]](...auditRequest), auditRows, 'retention: changed aggregate windows');
      retention.aggregateAudit = { request: auditRequest, resultRows: auditRows.length, exact: true };
      result.retention = retention;
      result.finalIdentity = { baseline: await baseIdentity(baseline), adapter: await baseIdentity(adapter) };
      assert.deepEqual(result.finalIdentity.adapter, result.finalIdentity.baseline);
      assert.equal(result.finalIdentity.baseline.rows, evidence.generator.minuteRows + 1 - retention.expectedDeleted);
      result.afterWritesStorage = { baseline: await storage(baseline, baselineFile), adapter: await storage(adapter, adapterFile) };
      await close(baseline); await close(adapter); checkpoint();
      console.log(JSON.stringify({ phase: 'repeat-complete', repeat, days, installMs: result.installMs, groups: result.groups,
        reads: result.reads.map(({ name, medianMs }) => ({ name, medianMs })), retention: retention.paths }));
    }
    evidence.identity = runtime.identity(extension);
    evidence.scriptHashes = Object.fromEntries(['benchmark.mjs', 'adapter.mjs', 'runtime.mjs', 'prepare.mjs'].map((file) => [file, sha256(path.join(repositoryRoot, 'scripts/kener-native', file))]));
    validateUpstream(args.upstream); evidence.upstreamClean = true; evidence.status = 'passed'; checkpoint();
    return evidence;
  } catch (error) { evidence.status = 'failed'; evidence.error = error.stack; checkpoint(); throw error; }
  finally { for (const db of opened) await close(db); runtime.close(); }
}

async function main() {
  const args = options(['upstream', 'runtime', 'extension', 'scratch', 'output'], ['config']);
  args.upstream = validateUpstream(args.upstream);
  mkdirSync(path.dirname(args.output), { recursive: true });
  const output = existsSync(args.output) ? realpathSync(args.output) : path.join(realpathSync(path.dirname(args.output)), path.basename(args.output));
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(root, output) && !inside(args.upstream, output), 'Evidence must be external to source checkouts');
  const result = await benchmark(args);
  console.log(JSON.stringify({ status: result.status, output: args.output, repeats: result.repeats.length }));
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) await main();
