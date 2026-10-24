import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createReader } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';
import { inside, sha256, validateUpstream } from './prepare.mjs';

const SOURCE = {
  q: 'src/lib/server/queues/q.ts', response: 'src/lib/server/queues/monitorResponseQueue.ts',
  controller: 'src/lib/server/controllers/monitorsController.ts', cache: 'src/lib/server/cache/cache.ts',
  setGet: 'src/lib/server/cache/setGet.ts', redis: 'src/lib/server/redisConnector.ts',
};
const rejectHelper = (name) => () => { throw new Error(`Unreviewed source-boundary helper: ${name}`); };

// A controlled source boundary, not a BullMQ/Redis server emulator. Every app
// module is unchanged and namespaced; only the reviewed edges below may resolve.
export async function loadSourceBoundary({ upstream, runtime, db, scratch, mode = 'strict', onRead,
  onPublication = async () => {}, negativeControl }) {
  upstream = validateUpstream(upstream);
  assert(['strict', 'experimental-numeric'].includes(mode), 'Unknown source-boundary read mode');
  assert(negativeControl === undefined || negativeControl === 'skip-precommit-refresh', 'Unknown source-boundary negative control');
  mkdirSync(scratch, { recursive: true });
  const parent = realpathSync(scratch);
  const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(upstream, parent) && !inside(repository, parent), 'Facade scratch must be external');
  const canonicalContext = await import(pathToFileURL(path.join(upstream, 'src/lib/server/db/poolContext.ts')).href);
  assert.equal(canonicalContext.runWithWorkerKnex, runtime.runWithWorkerKnex, 'Source boundary must share canonical worker context with runtime');
  assert.equal(canonicalContext.getWorkerKnex, runtime.getWorkerKnex, 'Source boundary must share canonical worker context getter');
  const directory = mkdtempSync(path.join(parent, 'source-facades-'));
  const namespace = randomUUID();
  const files = new Map(), sources = new Set(), resolutions = [], commands = [], alerts = [], events = [];
  const queues = [], workers = [], clients = [], jobs = [];
  let closed = false, logicalTime = 0, failSet, alertPromise;
  const store = new Map();
  function active() { assert(!closed, 'Source boundary closed'); }
  const failUnknown = (object, label) => new Proxy(object, { get(target, property, receiver) {
    if (typeof property === 'symbol' || property in target || property === 'then') return Reflect.get(target, property, receiver);
    throw new Error(`Unreviewed source-boundary ${label}: ${property}`);
  } });
  const reader = createReader(db, new runtime.MonitoringRepository(db), { mode, onRead });
  const database = failUnknown({
    runInWorkerContext: (work) => { active(); events.push({ event: 'worker-context' }); return runtime.runWithWorkerKnex(db, work); },
    async insertMonitoringData(data) {
      active();
      assert.equal(runtime.getWorkerKnex(), db, 'Actual q wrapper must bind canonical worker context');
      events.push({ event: 'insert-start', data });
      const insert = ({ repository, connection }) => {
        assert.equal(runtime.getWorkerKnex(), connection, 'Insert must use canonical transaction context');
        return repository.insertMonitoringData(data);
      };
      const result = negativeControl === 'skip-precommit-refresh'
        ? await db.transaction((trx) => runtime.runWithWorkerKnex(trx, () => insert({ repository: new runtime.MonitoringRepository(trx), connection: trx })))
        : await kenerTransaction(db, runtime, insert);
      events.push({ event: 'insert-resolved-after-commit' });
      return result;
    },
    getStatusCountsByInterval: (...args) => reader.getStatusCountsByInterval(...args),
    getStatusCountsByIntervalGroupedByMonitor: (...args) => reader.getStatusCountsByIntervalGroupedByMonitor(...args),
  }, 'DB method');
  class Queue {
    constructor(name, options) { active(); this.name = name; this.options = options; this.closed = false; queues.push(this); }
    async add(name, data, options) { active(); assert(!this.closed); const job = { name, data: structuredClone(data), options: structuredClone(options) }; jobs.push(job); return job; }
    async close() { this.closed = true; events.push({ event: 'controlled-queue-close', name: this.name }); }
  }
  class Worker {
    constructor(name, processor, options) { active(); this.name = name; this.processor = processor; this.options = options; this.closed = false; this.listeners = []; workers.push(this); }
    on(name, listener) { assert.equal(name, 'completed', 'Unreviewed controlled Worker event'); this.listeners.push(listener); return this; }
    async close() { this.closed = true; events.push({ event: 'controlled-worker-close', name: this.name }); }
  }
  class IORedis {
    constructor(url, options) { active(); this.url = url; this.options = options; this.closed = false; clients.push(this); return failUnknown(this, 'Redis command'); }
    async get(key) { active(); commands.push({ command: 'GET', key, logicalTime }); const entry = store.get(key); if (!entry || logicalTime >= entry.expires) { store.delete(key); return null; } return entry.payload; }
    async set(key, payload, ex, ttl) {
      active(); assert.equal(ex, 'EX'); assert.equal(typeof payload, 'string'); assert(Number.isFinite(ttl) && ttl > 0);
      commands.push({ command: 'SET', key, payload, ex, ttl, logicalTime });
      if (failSet) { const error = failSet; failSet = undefined; throw error; }
      await onPublication({ key, payload: JSON.parse(payload), ttl });
      store.set(key, { payload, expires: logicalTime + ttl }); return 'OK';
    }
    async del(key) { active(); commands.push({ command: 'DEL', key, logicalTime }); return Number(store.delete(key)); }
    async quit() { this.closed = true; events.push({ event: 'controlled-redis-quit' }); return 'OK'; }
  }
  const alerting = { push(...args) { active(); alerts.push(args); events.push({ event: 'alert-invoked', args }); return alertPromise ?? Promise.resolve(); } };
  function facade(name, body) {
    const filename = path.join(directory, `${name}.mjs`); writeFileSync(filename, `${body}\n`); files.set(name, filename); return pathToFileURL(filename).href;
  }
  const bridgeURL = facade('bridge', 'export const fixture = {};');
  const { fixture } = await import(bridgeURL);
  Object.assign(fixture, { database, Queue, Worker, IORedis, alerting, rejectHelper });
  const bridge = `import { fixture } from ${JSON.stringify(bridgeURL)};`;
  facade('db', `${bridge}\nexport default fixture.database;`);
  facade('bullmq', `${bridge}\nexport const Queue = fixture.Queue, Worker = fixture.Worker; export class Job {}`);
  facade('ioredis', `${bridge}\nexport default fixture.IORedis;`);
  // Deliberately avoid dotenv.config/process-global environment mutation.
  facade('dotenv', 'export default {config() {return {parsed:{}};}};');
  facade('alerting', `${bridge}\nexport default fixture.alerting;`);
  const helpers = {
    tool: ['GetMinuteStartNowTimestampUTC', 'GetMinuteStartTimestampUTC', 'GetNowTimestampUTC', 'GetNowTimestampUTCInMs', 'UnparsePercentage', 'UptimeCalculator'],
    constants: ['getBadgeStyle'], badge: ['makeBadge'], anywhere: ['ErrorSvg', 'IsValidProxyURL', 'IsValidHeartbeatSecret', 'HEARTBEAT_SECRET_RULE'],
    clientTools: ['CollapseStatusCounts'], i18n: ['translate', 'isLocaleAvailable'], randomName: [],
  };
  for (const [name, exports] of Object.entries(helpers)) facade(name, `${bridge}\n${exports.map((symbol) => `export const ${symbol} = fixture.rejectHelper(${JSON.stringify(`${name}.${symbol}`)});`).join('\n')}\nexport default new Proxy({}, {get(_target, key) {return fixture.rejectHelper(${JSON.stringify(name)} + '.' + String(key))();}});`);
  const sourceURL = (name) => {
    const filename = realpathSync(path.join(upstream, SOURCE[name])); sources.add(filename);
    return `${pathToFileURL(filename).href}?kener-source=${namespace}`;
  };
  const routes = {
    q: { '../redisConnector.js': 'redis', '../db/db.js': '@db', bullmq: '@bullmq' },
    response: { bullmq: '@bullmq', './q.js': 'q', '../controllers/controller.js': '@controller', '../cache/setGet.js': 'setGet', './alertingQueue.js': '@alerting' },
    controller: { 'node:util': 'node:util', '../tool.js': '@tool', '../cache/cache.js': 'cache', '../db/db.js': '@db', '../../global-constants.js': '@constants', 'badge-maker': '@badge', '@scaleway/random-name': '@randomName', '../../anywhere.js': '@anywhere', '../cache/setGet.js': 'setGet', '../queues/alertingQueue.js': '@alerting', '../../clientTools.js': '@clientTools', '../i18n.js': '@i18n' },
    cache: { '../redisConnector.js': 'redis' }, setGet: { './cache.js': 'cache' }, redis: { ioredis: '@ioredis', dotenv: '@dotenv' },
  };
  const controllerURL = sourceURL('controller');
  facade('controller', `export { InsertMonitoringData } from ${JSON.stringify(controllerURL)};`);
  function resolveReviewed(name, specifier) {
    const target = routes[name]?.[specifier];
    assert(target, `Unreviewed source-boundary import: ${name} -> ${specifier}`);
    resolutions.push({ from: SOURCE[name], specifier, target });
    if (target.startsWith('@')) return pathToFileURL(files.get(target.slice(1))).href;
    return target.startsWith('node:') ? target : sourceURL(target);
  }
  const priorRedisURL = process.env.REDIS_URL;
  // The real connector requires a URL; this scoped value reaches only controlled
  // IORedis. Restore on both factory failure and normal fixture close.
  process.env.REDIS_URL = `redis://source-boundary.invalid/${namespace}`;
  const hook = registerHooks({ resolve(specifier, context, nextResolve) {
    if (context.parentURL?.includes(`?kener-source=${namespace}`)) {
      const filename = fileURLToPath(context.parentURL);
      const name = Object.keys(SOURCE).find((key) => path.join(upstream, SOURCE[key]) === filename);
      return nextResolve(resolveReviewed(name, specifier), context);
    }
    return nextResolve(specifier, context);
  } });
  function restoreEnvironment() { if (priorRedisURL === undefined) delete process.env.REDIS_URL; else process.env.REDIS_URL = priorRedisURL; }
  try {
    const response = await import(sourceURL('response'));
    const controller = await import(controllerURL);
    const cache = await import(sourceURL('cache'));
    const setGet = await import(sourceURL('setGet'));
    const redis = await import(sourceURL('redis'));
    // Force lazy singleton construction while the fixture URL is installed.
    redis.redisIOConnection();
    redis.redisConnection();
    restoreEnvironment();
    return {
      namespace, directory, response, cache, setGet, redis,
      controller: failUnknown({ InsertMonitoringData: controller.InsertMonitoringData, GetStatusCountsByIntervalGroupedByMonitor: controller.GetStatusCountsByIntervalGroupedByMonitor }, 'controller export'),
      queues, workers, clients, jobs, commands, alerts, events,
      async process(index = jobs.length - 1) { active(); const worker = workers.findLast((entry) => !entry.closed); assert(worker, 'No live captured processor'); assert(jobs[index], 'No captured job'); return worker.processor(jobs[index], 'controlled-manual-token'); },
      advance(seconds) { active(); assert(Number.isFinite(seconds) && seconds >= 0); logicalTime += seconds; },
      failNextSet(error = new Error('controlled Redis SET failure')) { active(); failSet = error; },
      holdAlerts(promise) { active(); assert(promise instanceof Promise); alertPromise = promise; },
      injectRaw(key, payload, ttl = 60) { active(); assert.equal(typeof payload, 'string'); store.set(key, { payload, expires: logicalTime + ttl }); },
      probeImport: resolveReviewed,
      async probeHelper(name) {
        const [module, symbol] = name.split('.');
        assert(Object.hasOwn(helpers, module) && helpers[module].includes(symbol), 'Unknown facade helper probe');
        return (await import(pathToFileURL(files.get(module)).href))[symbol]();
      },
      probeDb: (name) => database[name],
      cleanupState: () => ({ closed, fixtureBindings: Object.keys(fixture), cachedKeys: store.size }),
      identity() { return {
        namespace, mode, negativeControl: negativeControl ?? null,
        sourceHashes: Object.fromEntries([...sources].sort().map((file) => [path.relative(upstream, file), sha256(file)])),
        canonicalPoolContext: { file: 'src/lib/server/db/poolContext.ts', url: pathToFileURL(path.join(upstream, 'src/lib/server/db/poolContext.ts')).href, sha256: sha256(path.join(upstream, 'src/lib/server/db/poolContext.ts')) },
        facadeHashes: Object.fromEntries([...files].map(([name, file]) => [name, { file, sha256: sha256(file), source: readFileSync(file, 'utf8') }])),
        harnessSHA256: sha256(fileURLToPath(import.meta.url)), resolutions,
        transport: 'Controlled BullMQ capture/manual invocation, controlled Redis wire/JSON/logical TTL, pending-capable alert sink; no real broker/server/alert delivery',
        dbBoundary: negativeControl === 'skip-precommit-refresh'
          ? 'Focused negative control only: insert + outer COMMIT without refresh; independent publication-view assertion must reject'
          : 'Insert only; kenerTransaction refresh + outer COMMIT before resolving; actual canonical worker context',
      }; },
      async close() {
        if (closed) return;
        const failures = [];
        try {
          try { await response.shutdown(); } catch (error) { failures.push(error); }
          const results = await Promise.allSettled([
            ...workers.filter((worker) => !worker.closed).map((worker) => worker.close()),
            ...queues.map((queue) => queue.close()), ...clients.map((client) => client.quit()),
          ]);
          for (const entry of results) if (entry.status === 'rejected') failures.push(entry.reason);
        } finally { hook.deregister(); closed = true; store.clear(); for (const key of Object.keys(fixture)) delete fixture[key]; }
        if (failures.length) throw new AggregateError(failures, 'Source-boundary resource cleanup failed');
      },
    };
  } catch (error) {
    try { await Promise.allSettled([...workers.map((worker) => worker.close()), ...queues.map((queue) => queue.close()), ...clients.map((client) => client.quit())]); }
    finally { hook.deregister(); restoreEnvironment(); closed = true; store.clear(); for (const key of Object.keys(fixture)) delete fixture[key]; }
    throw error;
  }
}
