import { installAdapter, PROJECTION, VIEW, TRIGGERS } from './adapter.mjs';
import { kenerTransaction } from './host.mjs';

const normalize = (sql) => sql.replace(/\s+/g, ' ').trim();
const owned = [PROJECTION, VIEW, ...Object.values(TRIGGERS)];
async function assertMainScope(connection) {
  // The unchanged installer and upstream repository use unqualified names.
  // Refuse connection-local shadows before they can redirect those operations.
  const sensitive = ['monitoring_data', ...owned];
  const shadows = await connection.raw(`SELECT name FROM temp.sqlite_schema
    WHERE name COLLATE NOCASE IN (${sensitive.map(() => '?').join(',')})`, sensitive);
  if (shadows.length) throw new Error(`Adapter temporary name shadow: ${shadows[0].name}`);
}
// Derive exact ownership from the unchanged installer rather than a second SQL
// definition. The recorder executes no SQL and records only its CREATE objects.
async function definitions() {
  const expected = new Map();
  const recorder = {
    transaction: (work) => work(recorder),
    raw(sql) {
      const match = /^CREATE (?:VIRTUAL )?(TABLE|TRIGGER) (\w+)/.exec(sql);
      if (match) expected.set(match[2], { type: match[1].toLowerCase(), sql: normalize(sql) });
    },
  };
  await installAdapter(recorder);
  return expected;
}

// This is an exclusive, known-sidecar ownership contract, not a general SQL
// dependency parser. Text matches (including literals/comments) refuse safely.
// Foreign IVM objects are never exempted by a private-name prefix.
export async function uninstallAdapter(connection) {
  return connection.transaction(async (trx) => {
    await assertMainScope(trx);
    const expected = await definitions();
    const schema = await trx.raw('SELECT type,name,sql FROM main.sqlite_schema');
    for (const name of owned) {
      const actual = schema.find((row) => row.name === name);
      const definition = expected.get(name);
      if (!actual || actual.type !== definition.type || actual.sql === null || normalize(actual.sql) !== definition.sql) {
        throw new Error(`Adapter ownership mismatch: ${name}`);
      }
    }
    // DROP invokes the extension's own cleanup, including shared capture/catalog
    // handling. Never delete private metadata or guess private object ownership.
    await trx.raw(`DROP TABLE main.${VIEW}`);
    for (const name of Object.values(TRIGGERS)) await trx.raw(`DROP TRIGGER main.${name}`);
    const reference = new RegExp(`(?:^|[^A-Za-z0-9_])(?:${PROJECTION}|${VIEW})(?:$|[^A-Za-z0-9_])`, 'i');
    // Include temporary and attached schemas conservatively. Qualify every
    // inspected/dropped owned object so a temporary name cannot shadow it.
    const databases = await trx.raw('PRAGMA database_list');
    for (const database of databases) {
      const quoted = `"${database.name.replaceAll('"', '""')}"`;
      for (const row of await trx.raw(`SELECT name,sql FROM ${quoted}.sqlite_schema WHERE sql IS NOT NULL`)) {
        if (database.name === 'main' && row.name === PROJECTION) continue;
        if (reference.test(row.sql)) throw new Error(`Adapter dependency remains: ${database.name}.${row.name}`);
      }
    }
    await trx.raw(`DROP TABLE main.${PROJECTION}`);
  });
}

export function migrateAdapter(connection, runtime, work) {
  return kenerTransaction(connection, runtime, async ({ connection: trx, repository }) => {
    await uninstallAdapter(trx);
    const result = await work({ connection: trx, repository });
    await assertMainScope(trx);
    await installAdapter(trx);
    return result;
  });
}
