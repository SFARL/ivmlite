// Optional SQLite experiment. Reads never refresh: the host owns the transaction.
export const BUCKET_SECONDS = 900;
export const PROJECTION = 'kener_ivm_projection';
export const VIEW = 'kener_ivm_900';
export const TRIGGERS = Object.freeze({ insert: 'kener_ivm_insert', update: 'kener_ivm_update', delete: 'kener_ivm_delete' });
const bucket = (prefix) => `${prefix}.timestamp - ((${prefix}.timestamp % 900 + 900) % 900)`;
export const VIEW_QUERY = `SELECT bucket_ts, monitor_tag, status,
  COUNT(*) AS sample_count, SUM(latency) AS latency_sum,
  COUNT(latency) AS latency_count, MIN(latency) AS min_latency, MAX(latency) AS max_latency
  FROM ${PROJECTION} GROUP BY bucket_ts, monitor_tag, status`;

// Deliberately fails on a second installation; reopen existing state without bootstrap.
// DDL, initial backfill, trigger installation and IVM creation commit together.
export async function installAdapter(db) {
  await db.transaction(async (trx) => {
    await trx.raw(`CREATE TABLE ${PROJECTION} (
      source_rowid INTEGER PRIMARY KEY, bucket_ts INTEGER NOT NULL,
      monitor_tag TEXT NOT NULL, status TEXT, latency REAL)`);
    await trx.raw(`INSERT INTO ${PROJECTION} SELECT rowid, ${bucket('monitoring_data')}, monitor_tag, status, latency FROM monitoring_data`);
    const insert = `INSERT INTO ${PROJECTION} (source_rowid,bucket_ts,monitor_tag,status,latency)
      VALUES (NEW.rowid, ${bucket('NEW')}, NEW.monitor_tag, NEW.status, NEW.latency);`;
    await trx.raw(`CREATE TRIGGER ${TRIGGERS.insert} AFTER INSERT ON monitoring_data BEGIN ${insert} END`);
    await trx.raw(`CREATE TRIGGER ${TRIGGERS.update} AFTER UPDATE ON monitoring_data BEGIN
      DELETE FROM ${PROJECTION} WHERE source_rowid = OLD.rowid; ${insert} END`);
    await trx.raw(`CREATE TRIGGER ${TRIGGERS.delete} AFTER DELETE ON monitoring_data BEGIN
      DELETE FROM ${PROJECTION} WHERE source_rowid = OLD.rowid; END`);
    await trx.raw(`CREATE VIRTUAL TABLE ${VIEW} USING ivm('${VIEW_QUERY}')`);
  });
}

export async function refreshAdapter(connection) {
  return connection.raw('SELECT ivm_refresh_all()');
}

// Construct the original repository with trx, including for methods that create
// nested Knex savepoints. A failed write, refresh or caller exception rolls back.
export async function adapterTransaction(db, Repository, work) {
  return db.transaction(async (trx) => {
    const result = await work(new Repository(trx), trx);
    await refreshAdapter(trx);
    return result;
  });
}

export function fallbackReason(mode, tags, start, interval, points) {
  if (mode === 'strict') return 'strict-json-number-parity';
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(interval) || interval <= 0 ||
      !Number.isSafeInteger(points) || points < 0 || !Number.isSafeInteger(points * interval) ||
      !Number.isSafeInteger(start + points * interval)) return 'unsafe-or-invalid-window';
  if (start % BUCKET_SECONDS !== 0 || interval % BUCKET_SECONDS !== 0) return 'unaligned-window';
  if (!(typeof tags === 'string' || (Array.isArray(tags) && tags.every((tag) => typeof tag === 'string')))) return 'unsupported-tags';
  return null;
}

// `read` exposes provenance separately; the two repository-shaped methods return
// only the original row shape. Experimental mode promises no general REAL parity.
export function createReader(db, repository, { mode = 'strict', onRead = () => {} } = {}) {
  if (!['strict', 'experimental-numeric'].includes(mode)) throw new Error(`Unknown adapter mode: ${mode}`);
  async function read(grouped, tags, start, interval, points) {
    const reason = fallbackReason(mode, tags, start, interval, points);
    const method = grouped ? 'getStatusCountsByIntervalGroupedByMonitor' : 'getStatusCountsByInterval';
    const provenance = { mode, source: reason ? 'original-repository' : 'public-ivm-view', fallbackReason: reason, refresh: 'explicit-host-contract' };
    let rows;
    if (reason) rows = await repository[method](tags, start, interval, points);
    else {
      const selected = Array.isArray(tags) ? tags : [tags];
      if (selected.length === 0) rows = [];
      else {
        // Unary plus preserves this no-affinity view column's TEXT membership
        // semantics while keeping IN out of xFilter. Otherwise SQLite repeats
        // the coercion-safe candidate scan for every selected tag.
        const sql = `SELECT ${grouped ? 'monitor_tag,' : ''}
          CAST((bucket_ts - ?) / ? AS INT) * ? + ? AS ts,
          SUM(CASE WHEN status = 'UP' THEN sample_count ELSE 0 END) AS count_of_up,
          SUM(CASE WHEN status = 'DOWN' THEN sample_count ELSE 0 END) AS count_of_down,
          SUM(CASE WHEN status = 'DEGRADED' THEN sample_count ELSE 0 END) AS count_of_degraded,
          SUM(CASE WHEN status = 'MAINTENANCE' THEN sample_count ELSE 0 END) AS count_of_maintenance,
          SUM(latency_sum) * 1.0 / SUM(latency_count) AS avg_latency,
          MAX(max_latency) AS max_latency, MIN(min_latency) AS min_latency
          FROM ${VIEW} WHERE bucket_ts >= ? AND bucket_ts < ?
          AND +monitor_tag IN (${selected.map(() => '?').join(',')})
          GROUP BY ${grouped ? 'monitor_tag,' : ''} ts ORDER BY ${grouped ? 'monitor_tag ASC,' : ''} ts ASC`;
        const result = await db.raw(sql, [start, interval, interval, start, start, start + points * interval, ...selected]);
        rows = result.map((row) => ({
          ...(grouped ? { monitor_tag: row.monitor_tag } : {}), ts: Number(row.ts),
          countOfUp: Number(row.count_of_up) || 0, countOfDown: Number(row.count_of_down) || 0,
          countOfDegraded: Number(row.count_of_degraded) || 0, countOfMaintenance: Number(row.count_of_maintenance) || 0,
          avgLatency: Number(row.avg_latency) || 0, maxLatency: Number(row.max_latency) || 0, minLatency: Number(row.min_latency) || 0,
        }));
      }
    }
    onRead(provenance);
    return { rows, provenance };
  }
  return {
    read,
    getStatusCountsByInterval: async (...args) => (await read(false, ...args)).rows,
    getStatusCountsByIntervalGroupedByMonitor: async (...args) => (await read(true, ...args)).rows,
  };
}
