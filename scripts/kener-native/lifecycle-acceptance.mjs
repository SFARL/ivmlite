import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { loadRuntime } from './runtime.mjs';
import { installAdapter, refreshAdapter, PROJECTION, VIEW, TRIGGERS } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';
import { migrateAdapter, uninstallAdapter } from './lifecycle.mjs';
import { inside, sha256, validateUpstream } from './prepare.mjs';

const CHILD_DEADLINE_MS = 20000;
const viewFields = ['bucket_ts', 'monitor_tag', 'status', 'sample_count', 'latency_sum', 'latency_count', 'min_latency', 'max_latency'];
const owned = [PROJECTION, VIEW, ...Object.values(TRIGGERS)];
const sample = (source_rowid, tag, timestamp = 901, latency = 1.25, status = 'UP') => ({
  source_rowid, monitor_tag: tag, timestamp, latency, status, type: 'REALTIME', raw_status: status, error_message: null,
});
const payload = ({ source_rowid, host_note, ...row }) => row;
const canonical = (rows) => rows.map((row) => JSON.stringify(Object.fromEntries(Object.entries(row).sort(([a], [b]) => a.localeCompare(b))))).sort();
function grouped(rows) {
  const groups = new Map();
  for (const row of rows) {
    const bucket_ts = Math.floor(row.timestamp / 900) * 900;
    const key = JSON.stringify([bucket_ts, row.monitor_tag, row.status]);
    if (!groups.has(key)) groups.set(key, { bucket_ts, monitor_tag: row.monitor_tag, status: row.status,
      sample_count: 0, latency_sum: null, latency_count: 0, min_latency: null, max_latency: null });
    const group = groups.get(key); group.sample_count++;
    if (row.latency !== null) {
      group.latency_sum = (group.latency_sum ?? 0) + row.latency; group.latency_count++;
      group.min_latency = group.min_latency === null ? row.latency : Math.min(group.min_latency, row.latency);
      group.max_latency = group.max_latency === null ? row.latency : Math.max(group.max_latency, row.latency);
    }
  }
  return [...groups.values()];
}
const schema = (db) => db.raw("SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE name <> 'sqlite_sequence' ORDER BY type,name");

function crashWorker(config) {
  const child = fork(fileURLToPath(new URL('./lifecycle-worker.mjs', import.meta.url)), [JSON.stringify(config)], {
    execPath: process.execPath, stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
  });
  let failure, ready, exitStatus, stdout = '', stderr = '';
  const waiters = new Set();
  child.stdout.on('data', (data) => { stdout = (stdout + data).slice(-65536); });
  child.stderr.on('data', (data) => { stderr = (stderr + data).slice(-65536); });
  function flush() {
    for (const waiter of waiters) {
      if (ready) waiter.finish(null, ready);
      else if (failure || exitStatus) waiter.finish(failure ?? new Error(`Lifecycle child exited before readiness: ${JSON.stringify(exitStatus)} ${stderr}`));
    }
  }
  const exited = new Promise((resolve) => child.once('exit', (code, signal) => { exitStatus = { code, signal }; resolve(exitStatus); flush(); }));
  child.on('error', (error) => { failure = error; flush(); });
  child.on('message', (message) => {
    if (message.type === 'ready') ready = message;
    if (message.type === 'failed') failure = new Error(message.error);
    flush();
  });
  async function boundedExit(ms) {
    let timer;
    try { return await Promise.race([exited, new Promise((resolve) => { timer = setTimeout(() => resolve(null), ms); })]); }
    finally { clearTimeout(timer); }
  }
  return {
    waitReady() {
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => waiter.finish(new Error(`Lifecycle readiness deadline: ${stderr}`)), CHILD_DEADLINE_MS);
        const waiter = { finish(error, value) { clearTimeout(timer); waiters.delete(waiter); if (error) reject(error); else resolve(value); } };
        waiters.add(waiter); flush();
      });
    },
    async kill() {
      assert(ready && !exitStatus, 'Kill requires the live worker readiness barrier');
      assert.equal(ready.nativeOpen, true);
      assert(child.kill('SIGKILL'), 'SIGKILL delivered to live lifecycle worker');
      const exit = await boundedExit(3000);
      assert.deepEqual(exit, { code: null, signal: 'SIGKILL' }, 'Recovery must follow actual abrupt process death');
      return { ready, exit, stdout, stderr };
    },
    async close() {
      if (!exitStatus) { child.kill('SIGKILL'); assert(await boundedExit(3000), 'Lifecycle child survived cleanup SIGKILL'); }
      for (const waiter of waiters) waiter.finish(new Error('Lifecycle worker closed'));
    },
  };
}

// Export only: the guarded acceptance CLI owns path/upstream validation and JSON.
export async function lifecycleAcceptance(args) {
  args = { ...args, upstream: validateUpstream(args.upstream) };
  mkdirSync(args.scratch ?? os.tmpdir(), { recursive: true });
  const parent = realpathSync(args.scratch ?? os.tmpdir());
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(args.upstream, parent) && !inside(repositoryRoot, parent), 'Scratch storage must be external to source checkouts');
  const scratch = mkdtempSync(path.join(parent, 'kener-lifecycle-'));
  const extension = realpathSync(args.extension), policy = { expectedExtensionSHA256: sha256(extension), busyTimeoutMs: 100 };
  const runtime = await loadRuntime({ ...args, database: path.join(scratch, 'runtime.sqlite') });
  const opened = new Set(), children = new Set(), checks = [], mutations = [], crashRecovery = [], emittedQueries = [], lifecycle = [];
  const assertions = { baseComparisons: 0, projectionComparisons: 0, viewComparisons: 0, schemaComparisons: 0,
    completeStateComparisons: 0, contextComparisons: 0, dependencyRefusals: 0, ownershipRefusals: 0, temporaryShadowRefusals: 0, abruptDeaths: 0 };
  function open(filename) {
    const db = runtime.createDatabase(filename, extension, lifecycle, policy); opened.add(db);
    db.on('query', ({ sql, __knexUid, __knexTxId }) => emittedQueries.push({ filename, sql, nativeConnection: __knexUid, transaction: __knexTxId }));
    return db;
  }
  async function close(db) { await db.destroy(); opened.delete(db); }
  function context(expected, label) { assert.equal(runtime.getWorkerKnex(), expected, label); assertions.contextComparisons++; }
  function sameSchema(actual, expected, label) { assert.deepEqual(actual, expected, label); assertions.schemaComparisons++; }
  async function base(db, expected, label) {
    // Full columns, including rowid and host-added fields: never derive expected
    // source facts or source keys by reading the database under examination.
    const actual = await db.raw('SELECT rowid AS source_rowid,* FROM main.monitoring_data');
    assert.deepEqual(canonical(actual), canonical(expected), `${label}: independent complete base facts`); assertions.baseComparisons++;
  }
  async function state(db, expected, label) {
    await base(db, expected, label);
    const projection = expected.map(({ source_rowid, timestamp, monitor_tag, status, latency }) => ({ source_rowid,
      bucket_ts: Math.floor(timestamp / 900) * 900, monitor_tag, status, latency }));
    assert.deepEqual(canonical(await db.withSchema('main').table(PROJECTION).select('*')), canonical(projection), `${label}: independent complete projection`); assertions.projectionComparisons++;
    assert.deepEqual(canonical(await db.withSchema('main').table(VIEW).select(viewFields)), canonical(grouped(expected)), `${label}: independent complete grouped view`); assertions.viewComparisons++;
  }
  async function snapshot(db) {
    const objects = await schema(db), tables = {};
    const temporarySchema = await db.raw("SELECT type,name,tbl_name,sql FROM temp.sqlite_schema WHERE name <> 'sqlite_sequence' ORDER BY type,name");
    const temporaryTables = {};
    // Compare all main/temp data tables, including private IVM state/capture/
    // catalogs and foreign objects. Only SQLite sequence bookkeeping is excluded.
    for (const [namespace, definitions, contents] of [['main', objects, tables], ['temp', temporarySchema, temporaryTables]]) {
      for (const object of definitions.filter((row) => row.type === 'table')) {
        try { contents[object.name] = canonical(await db.raw(`SELECT * FROM ${namespace}."${object.name.replaceAll('"', '""')}"`)); }
        catch (error) {
          // Deliberate projection drift makes its public virtual table refuse
          // reads; physical backing data still participates in the comparison.
          assert.equal(namespace, 'main'); assert.equal(object.name, VIEW); assert.match(error.message, /different plan/);
          contents[object.name] = { readFailure: error.message, code: error.code };
        }
      }
    }
    return { schema: objects, tables, temporarySchema, temporaryTables };
  }
  async function unchanged(db, before, label) { assert.deepEqual(await snapshot(db), before, `${label}: exact schema and every data table`); assertions.completeStateComparisons++; }
  async function fixture(name) {
    const filename = path.join(scratch, `${name}.sqlite`), db = open(filename);
    await runtime.migrate(db);
    await db.raw('CREATE TABLE unrelated_payload (id INTEGER PRIMARY KEY, note TEXT NOT NULL)');
    await db.raw("INSERT INTO unrelated_payload VALUES (1,'must survive')");
    await db.raw('CREATE INDEX unrelated_note ON unrelated_payload(note)');
    await db.raw('CREATE VIEW unrelated_read AS SELECT note FROM unrelated_payload');
    await db.raw('CREATE TRIGGER unrelated_trigger AFTER INSERT ON unrelated_payload BEGIN SELECT NEW.note; END');
    const originalSchema = await schema(db);
    await installAdapter(db);
    const expected = [sample(1, 'seed'), sample(2, 'null-only', -1, null, null), sample(3, 'seed', 902, 0.5, 'DOWN')];
    await kenerTransaction(db, runtime, async ({ repository }) => { for (const row of expected) await repository.insertMonitoringData(payload(row)); });
    await state(db, expected, `${name}: fixture`);
    return { filename, db, originalSchema, expected };
  }
  async function cleanWrite(db, expected, label) {
    const correction = { ...expected[0], latency: 7.5, status: 'DEGRADED', raw_status: 'DEGRADED' };
    await kenerTransaction(db, runtime, async ({ connection, repository }) => {
      context(connection, `${label}: next write context`);
      await repository.insertMonitoringData(payload(correction));
    });
    const updated = [correction, ...expected.slice(1)];
    await state(db, updated, `${label}: next valid write`);
    await kenerTransaction(db, runtime, async () => {});
    await state(db, updated, `${label}: next clean refresh`);
    return updated;
  }
  try {
    context(undefined, 'initial context absent');
    let { db, filename, originalSchema, expected } = await fixture('uninstall');
    const pending = sample(4, 'pending', 1801, 2.5, 'MAINTENANCE');
    // Leave actual unrefreshed capture pending, then uninstall in that same
    // caller transaction. No read-side or manual private-metadata cleanup.
    await db.transaction(async (trx) => runtime.runWithWorkerKnex(trx, async () => {
      await new runtime.MonitoringRepository(trx).insertMonitoringData(payload(pending));
      await uninstallAdapter(trx);
    }));
    expected = [...expected, pending];
    await base(db, expected, 'uninstall retains pending base write');
    sameSchema(await schema(db), originalSchema, 'sole adapter cleanup restores entire original schema, indexes and unrelated objects');
    assert.deepEqual(await db('unrelated_read').select('*'), [{ note: 'must survive' }]);
    await assert.rejects(() => uninstallAdapter(db), /Adapter ownership mismatch/); assertions.ownershipRefusals++;
    await close(db);

    // Test-only bare pool, with the exact owned dependency installation and no
    // loadExtension hook. No production escape hatch is added to runtime.mjs.
    const require = createRequire(path.join(realpathSync(args.runtime), 'package.json'));
    const bare = require('knex')({ client: 'better-sqlite3', connection: { filename }, useNullAsDefault: true, pool: { min: 0, max: 1 } });
    opened.add(bare);
    assert.equal((await bare.raw("SELECT count(*) AS n FROM pragma_function_list WHERE name='ivm_refresh_all'"))[0].n, 0);
    const bareAdded = sample(5, 'bare-added', 2701, 3.5, 'DOWN');
    const bareCorrection = { ...expected[0], latency: 4.5, status: 'DOWN', raw_status: 'DOWN' };
    await runtime.runWithWorkerKnex(bare, async () => {
      const repository = new runtime.MonitoringRepository(bare);
      await repository.insertMonitoringData(payload(bareAdded));
      await repository.insertMonitoringData(payload(bareCorrection));
      assert.deepEqual(await repository.getLatestMonitoringData('bare-added'), payload(bareAdded));
      assert((await repository.getStatusCountsByInterval('bare-added', 2700, 900, 1)).length === 1);
      assert((await repository.getStatusCountsByIntervalGroupedByMonitor(['bare-added'], 2700, 900, 1)).length === 1);
    });
    expected = [bareCorrection, ...expected.slice(1), bareAdded];
    await base(bare, expected, 'actual upstream repository bare native writes and reads');
    sameSchema(await schema(bare), originalSchema, 'bare repository leaves original schema intact');
    await close(bare); db = open(filename); await installAdapter(db);
    await state(db, expected, 'reinstall backfills all bare source changes');
    await cleanWrite(db, expected, 'reinstall'); await close(db);
    checks.push('pending-write uninstall uses extension cleanup; complete original schema restored; fresh bare exact-dependency Knex pool writes/reads through pinned repository without extension, then reinstall/backfill and next write pass');

    ({ db, expected } = await fixture('migration'));
    const initialSchema = await schema(db);
    const migrated = expected.map((row) => ({ ...row, source_rowid: row.source_rowid + 100,
      monitor_tag: `${row.monitor_tag}-moved`, timestamp: row.timestamp + 900, host_note: 'host-default' }));
    const extra = { ...sample(104, 'migration-added', 3601, 6.5), host_note: 'host-default' };
    await runtime.runWithWorkerKnex(db, async () => {
      const result = await migrateAdapter(db, runtime, async ({ connection, repository }) => {
        context(connection, 'migration binds actual upstream worker context'); assert.equal(repository.knex, connection);
        await connection.raw("ALTER TABLE monitoring_data ADD COLUMN host_note TEXT NOT NULL DEFAULT 'host-default'");
        await connection.raw('UPDATE monitoring_data SET rowid = rowid + 100, timestamp = timestamp + 900, monitor_tag = monitor_tag || ?', ['-moved']);
        await repository.insertMonitoringData(payload(extra));
        return { result: 'host migration complete', count: 4 };
      });
      assert.deepEqual(result, { result: 'host migration complete', count: 4 });
      context(db, 'successful migration restores real ambient pool');
    });
    context(undefined, 'successful migration restores absent context');
    expected = [...migrated, extra];
    await state(db, expected, 'schema/source-key/rowid migration backfill');
    const expectedSchema = initialSchema.map((row) => row.name === 'monitoring_data'
      ? { ...row, sql: row.sql.replace(", primary key (", ", host_note TEXT NOT NULL DEFAULT 'host-default', primary key (") } : row);
    sameSchema(await schema(db), expectedSchema, 'migration changes only requested original table definition');
    const repeat = await migrateAdapter(db, runtime, async ({ connection }) => { context(connection, 'repeat rebuild context'); return 42; });
    assert.equal(repeat, 42); await state(db, expected, 'repeat rebuild');
    sameSchema(await schema(db), expectedSchema, 'repeat rebuild exact schema');
    const beforeOuter = await snapshot(db);
    await assert.rejects(() => db.transaction(async (outer) => runtime.runWithWorkerKnex(outer, async () => {
      await migrateAdapter(outer, runtime, async ({ connection, repository }) => {
        context(connection, 'nested migration binds savepoint');
        await connection.raw('ALTER TABLE monitoring_data ADD COLUMN outer_only TEXT');
        await repository.insertMonitoringData(payload(sample(105, 'outer-rollback')));
      });
      context(outer, 'nested helper restores outer transaction context');
      throw new Error('outer rollback requested');
    })), /outer rollback requested/);
    await unchanged(db, beforeOuter, 'explicit outer transaction rollback restores migration savepoint');
    await state(db, expected, 'outer rollback independent facts');
    checks.push('migration preserves callback value and ambient worker context; added unreferenced column, changed composite source keys/rowids, repeat rebuild and explicit outer rollback pass');

    for (const failure of ['callback-after-ddl', 'reinstall-collision']) {
      const before = await snapshot(db);
      await runtime.runWithWorkerKnex(db, async () => {
        await assert.rejects(() => migrateAdapter(db, runtime, async ({ connection, repository }) => {
          context(connection, `${failure}: migration context`);
          await connection.raw('ALTER TABLE monitoring_data ADD COLUMN failed_column TEXT');
          await connection.raw('UPDATE monitoring_data SET rowid = rowid + 1000, timestamp = timestamp + 900');
          await repository.insertMonitoringData(payload(sample(1000, 'must-rollback')));
          if (failure === 'callback-after-ddl') throw new Error('intentional callback failure after DDL');
          await connection.raw(`CREATE TABLE ${PROJECTION} (collision TEXT)`);
        }), failure === 'callback-after-ddl' ? /intentional callback failure/ : /already exists/);
        context(db, `${failure}: restores ambient context`);
      });
      context(undefined, `${failure}: restores absent context`);
      await unchanged(db, before, failure); await state(db, expected, failure);
      expected = await cleanWrite(db, expected, failure);
    }
    await close(db);
    checks.push('callback error after DDL and forced reinstall collision restore exact schema, every table and independent base/projection/view; next valid write and clean refresh pass');

    // Reproduce the SQLite name-resolution boundary with different temporary
    // facts. The unchanged repository/installer use unqualified source names.
    for (const shadow of ['preexisting-source', 'callback-source', 'callback-owned']) {
      const item = await fixture(`temporary-shadow-${shadow}`); db = item.db;
      if (shadow === 'preexisting-source') {
        await db.raw('CREATE TEMP TABLE monitoring_data (monitor_tag TEXT,timestamp INTEGER,status TEXT,latency REAL,type TEXT,raw_status TEXT,error_message TEXT)');
        await db.raw("INSERT INTO temp.monitoring_data VALUES ('foreign-temp-fact',9999,'DOWN',99,'SIGNAL','DOWN','preserve temporary source')");
      }
      const before = await snapshot(db);
      let callbacks = 0;
      await assert.rejects(() => migrateAdapter(db, runtime, async ({ connection, repository }) => {
        callbacks++; context(connection, `${shadow}: bound migration context`);
        await connection.raw('ALTER TABLE main.monitoring_data ADD COLUMN shadow_attempt TEXT');
        await repository.insertMonitoringData(payload(sample(4, 'shadow-attempt-main-write')));
        const name = shadow === 'callback-owned' ? PROJECTION : 'monitoring_data';
        await connection.raw(`CREATE TEMP TABLE ${name} (sentinel TEXT)`);
        await connection.raw(`INSERT INTO temp.${name} VALUES ('must rollback')`);
        return 'shadow-created';
      }), /Adapter temporary name shadow/);
      assertions.temporaryShadowRefusals++;
      assert.equal(callbacks, shadow === 'preexisting-source' ? 0 : 1, 'Existing shadow refuses before host work; callback-created shadow refuses before reinstall');
      context(undefined, `${shadow}: failed migration restores absent context`);
      await unchanged(db, before, `${shadow}: main and temporary schema/data restore exactly`);
      await state(db, item.expected, `${shadow}: independently verified original main state`);
      if (shadow === 'preexisting-source') {
        assert.deepEqual(await db.raw('SELECT * FROM temp.monitoring_data'), [{ monitor_tag: 'foreign-temp-fact', timestamp: 9999,
          status: 'DOWN', latency: 99, type: 'SIGNAL', raw_status: 'DOWN', error_message: 'preserve temporary source' }]);
        await db.raw('DROP TABLE temp.monitoring_data');
      }
      await cleanWrite(db, item.expected, `${shadow}: recovery`); await close(db);
    }
    checks.push('preexisting TEMP monitoring_data with distinct foreign facts refuses before callback; callback-created TEMP source/owned shadows refuse before reinstall and restore exact main+temp schema/data, then next write/refresh passes');

    for (const dependency of ['maintained-projection', 'ordinary-public', 'ordinary-projection', 'index', 'trigger', 'foreign-key', 'private-prefix', 'temporary-public']) {
      const item = await fixture(`dependent-${dependency}`); db = item.db;
      if (dependency === 'maintained-projection') await db.raw(`CREATE VIRTUAL TABLE foreign_projection USING ivm('SELECT monitor_tag, COUNT(*) AS n FROM ${PROJECTION} GROUP BY monitor_tag')`);
      else if (dependency === 'ordinary-public') await db.raw(`CREATE VIEW foreign_public AS SELECT * FROM ${VIEW}`);
      else if (dependency === 'ordinary-projection') await db.raw(`CREATE VIEW foreign_projection AS SELECT * FROM ${PROJECTION}`);
      else if (dependency === 'index') await db.raw(`CREATE INDEX foreign_projection_index ON ${PROJECTION}(monitor_tag)`);
      else if (dependency === 'trigger') await db.raw(`CREATE TRIGGER foreign_projection_trigger AFTER INSERT ON ${PROJECTION} BEGIN SELECT NEW.monitor_tag; END`);
      else if (dependency === 'foreign-key') await db.raw(`CREATE TABLE foreign_child (source_rowid INTEGER REFERENCES ${PROJECTION}(source_rowid))`);
      else if (dependency === 'private-prefix') await db.raw(`CREATE VIEW __ivm_foreign_owner AS SELECT * FROM ${VIEW}`);
      else await db.raw(`CREATE TEMP VIEW foreign_temporary AS SELECT * FROM main.${VIEW}`);
      const temporaryBefore = await db.raw('SELECT type,name,tbl_name,sql FROM temp.sqlite_schema ORDER BY type,name');
      const before = await snapshot(db);
      await assert.rejects(() => uninstallAdapter(db), /Adapter dependency remains/); assertions.dependencyRefusals++;
      await unchanged(db, before, `foreign ${dependency} refusal`);
      sameSchema(await db.raw('SELECT type,name,tbl_name,sql FROM temp.sqlite_schema ORDER BY type,name'), temporaryBefore, `foreign ${dependency} temporary schema intact`);
      await state(db, item.expected, `foreign ${dependency} no collateral`);
      // No later write is asserted for an external index: the extension itself
      // rejects changed capture shapes. Refusal must preserve exactly that state.
      await close(db);
    }
    checks.push('foreign maintained projection view, ordinary views over both owned tables, index, trigger, foreign key, __ivm_-prefixed foreign view and temporary view all fail atomically without collateral changes');

    ({ db, expected, originalSchema } = await fixture('unrelated-maintained'));
    await db.raw("CREATE VIRTUAL TABLE foreign_base USING ivm('SELECT monitor_tag, COUNT(*) AS n, SUM(latency) AS total FROM monitoring_data GROUP BY monitor_tag')");
    const foreignSQL = (await schema(db)).find((row) => row.name === 'foreign_base');
    await uninstallAdapter(db); await base(db, expected, 'foreign base view preserves full source');
    sameSchema((await schema(db)).filter((row) => originalSchema.some((old) => old.name === row.name)), originalSchema, 'unrelated maintained view retains every original object');
    assert.deepEqual((await schema(db)).find((row) => row.name === 'foreign_base'), foreignSQL);
    const correction = { ...expected[0], latency: 8.5, status: 'DOWN', raw_status: 'DOWN' };
    await db.transaction(async (trx) => runtime.runWithWorkerKnex(trx, async () => {
      await new runtime.MonitoringRepository(trx).insertMonitoringData(payload(correction)); await refreshAdapter(trx);
    }));
    expected = [correction, ...expected.slice(1)]; await base(db, expected, 'foreign maintained view continues receiving writes');
    assert.deepEqual(canonical(await db('foreign_base').select('*')), canonical([
      { monitor_tag: 'seed', n: 2, total: 9 }, { monitor_tag: 'null-only', n: 1, total: null },
    ]));
    assert(!(await schema(db)).some((row) => owned.includes(row.name) || row.name.includes(PROJECTION) || row.name.includes(VIEW)));
    await installAdapter(db); await state(db, expected, 'reinstall alongside unrelated maintained base view'); await close(db);
    checks.push('separate unrelated maintained view over original monitoring_data survives uninstall with shared extension metadata/capture, consumes subsequent source correction and coexists with reinstall');

    for (const footprint of ['absent', 'partial', 'trigger-body', 'projection-column', 'view-query']) {
      const item = await fixture(`ownership-${footprint}`); db = item.db;
      if (footprint === 'absent') await uninstallAdapter(db);
      else if (footprint === 'partial') await db.raw(`DROP TRIGGER ${TRIGGERS.delete}`);
      else if (footprint === 'trigger-body') {
        await db.raw(`DROP TRIGGER ${TRIGGERS.delete}`);
        await db.raw(`CREATE TRIGGER ${TRIGGERS.delete} AFTER DELETE ON monitoring_data BEGIN SELECT OLD.rowid; END`);
      } else if (footprint === 'projection-column') await db.raw(`ALTER TABLE ${PROJECTION} ADD COLUMN foreign_extra TEXT`);
      else {
        await db.raw(`DROP TABLE ${VIEW}`);
        await db.raw(`CREATE VIRTUAL TABLE ${VIEW} USING ivm('SELECT monitor_tag, COUNT(*) AS n FROM ${PROJECTION} GROUP BY monitor_tag')`);
      }
      const before = await snapshot(db);
      await assert.rejects(() => uninstallAdapter(db), /Adapter ownership mismatch/); assertions.ownershipRefusals++;
      await unchanged(db, before, `${footprint} footprint fail closed`); await base(db, item.expected, `${footprint} footprint preserves source`);
      await close(db);
    }
    checks.push('absent, partial and same-name modified trigger/projection/public-view definitions refuse without changes; uninstall is deliberately non-idempotent');

    // Real-source mutation: remove only reinstall from migrateAdapter, retain
    // successful callback/transaction completion, then require all owned objects.
    const source = readFileSync(new URL('./lifecycle.mjs', import.meta.url), 'utf8');
    const site = '    await installAdapter(trx);'; assert.equal(source.split(site).length - 1, 1);
    const mutant = source.replace("from './adapter.mjs'", `from ${JSON.stringify(new URL('./adapter.mjs', import.meta.url).href)}`)
      .replace("from './host.mjs'", `from ${JSON.stringify(new URL('./host.mjs', import.meta.url).href)}`).replace(site, '    // mutant: omitted reinstall');
    const mutantFile = path.join(scratch, 'omit-reinstall.mjs'); writeFileSync(mutantFile, mutant);
    const { migrateAdapter: brokenMigrate } = await import(pathToFileURL(mutantFile).href);
    ({ db, expected } = await fixture('mutation'));
    assert.equal(await brokenMigrate(db, runtime, async () => 'callback-done'), 'callback-done');
    let redFailure;
    try { assert.deepEqual((await schema(db)).filter((row) => owned.includes(row.name)).map((row) => row.name).sort(), [...owned].sort(), 'lifecycle reinstall must restore every owned object'); }
    catch (error) {
      assert(error instanceof assert.AssertionError); assert.match(error.message, /lifecycle reinstall must restore every owned object/);
      assert.deepEqual(error.actual, []); assert.deepEqual(error.expected, [...owned].sort()); redFailure = error.message;
    }
    assert(redFailure, 'Omitted reinstall must fail the intended ownership assertion');
    await base(db, expected, 'mutation reaches successful callback without base corruption');
    await installAdapter(db);
    assert.equal(await migrateAdapter(db, runtime, async () => 'callback-done'), 'callback-done');
    assert.deepEqual((await schema(db)).filter((row) => owned.includes(row.name)).map((row) => row.name).sort(), [...owned].sort());
    await state(db, expected, 'intact source restores all owned objects and exact state'); await close(db);
    mutations.push({ mutation: 'omit-lifecycle-reinstall', status: 'detected', redFailure, intact: 'passed' });

    for (const phase of ['before-commit', 'after-commit']) {
      const item = await fixture(`crash-${phase}`); db = item.db;
      const oldSchema = await schema(db), oldSnapshot = await snapshot(db);
      await close(db);
      const workerRow = sample(104, 'worker-added', 3601, 5.5, 'MAINTENANCE');
      const child = crashWorker({ ...args, database: item.filename, extension, policy, phase, row: payload(workerRow) }); children.add(child);
      const ready = await child.waitReady(); assert.notEqual(ready.pid, process.pid); assert.equal(ready.phase, phase);
      const death = await child.kill(); await child.close(); children.delete(child); assertions.abruptDeaths++;
      const commits = ready.queries.filter((query) => /^COMMIT;?$/.test(query.sql));
      assert.equal(commits.length, phase === 'before-commit' ? 0 : 1, 'IPC readiness is on specified side of the actual outer COMMIT');
      assert(ready.queries.some((query) => /^RELEASE SAVEPOINT/.test(query.sql)), 'Migration actually used nested savepoints');
      assert(ready.queries.some((query) => /SELECT ivm_refresh_all\(\)/.test(query.sql)), 'Migration refreshed before readiness');
      db = open(item.filename);
      assert.deepEqual(await db.raw('PRAGMA journal_mode'), [{ journal_mode: 'delete' }]);
      let recovered = item.expected;
      if (phase === 'before-commit') {
        sameSchema(await schema(db), oldSchema, 'abrupt uncommitted migration restores old complete schema');
        await unchanged(db, oldSnapshot, 'rollback journal recovery before COMMIT restores every original table');
      } else {
        recovered = [...item.expected.map((row) => ({ ...row, source_rowid: row.source_rowid + 100,
          timestamp: row.timestamp + 900, monitor_tag: `${row.monitor_tag}-moved`, host_note: 'worker-default' })),
        { ...workerRow, host_note: 'worker-default' }];
        const newSchema = oldSchema.map((row) => row.name === 'monitoring_data'
          ? { ...row, sql: row.sql.replace(", primary key (", ", host_note TEXT NOT NULL DEFAULT 'worker-default', primary key (") } : row);
        sameSchema(await schema(db), newSchema, 'abrupt committed migration retains independently constructed new schema');
      }
      await state(db, recovered, `SIGKILL ${phase} fresh reopen independent state`);
      recovered = await cleanWrite(db, recovered, `SIGKILL ${phase} recovery`);
      crashRecovery.push({ phase, ...death, expectedRecoveredFacts: phase === 'before-commit' ? item.expected : [...item.expected.map((row) => ({ ...row,
        source_rowid: row.source_rowid + 100, timestamp: row.timestamp + 900, monitor_tag: `${row.monitor_tag}-moved`, host_note: 'worker-default' })), { ...workerRow, host_note: 'worker-default' }],
      nextWriteFacts: recovered, recoveredSchema: await schema(db), journalMode: 'delete' });
      await close(db);
    }
    checks.push('two real child SIGKILLs after rebuild/refresh before outer COMMIT and after COMMIT with native handle open; fresh default rollback-journal recovery gives independent old/new schema+facts and clean next write/refresh');
    context(undefined, 'final absent worker context'); validateUpstream(args.upstream);
    return { status: 'passed', scope: 'Bounded explicit-pool/savepoint adapter uninstall and host schema migration; conservative exclusive-sidecar ownership; default rollback-journal abrupt process death only',
      ...runtime.identity(extension), scratch, policy, assertions, checks, mutations, crashRecovery, lifecycle, emittedQueries,
      independentOracle: 'Host-constructed full main base fields including explicit source rowids and migration host_note; JS projection/group ledger; exact main+temp schema and every data-table snapshots exclude only sqlite_sequence',
      upstreamClean: true };
  } finally {
    const errors = [];
    for (const child of children) { try { await child.close(); } catch (error) { errors.push(error); } }
    for (const db of opened) { try { await close(db); } catch (error) { errors.push(error); } }
    try { runtime.close(); } catch (error) { errors.push(error); }
    if (errors.length) throw new AggregateError(errors, 'Lifecycle acceptance cleanup failed');
  }
}
