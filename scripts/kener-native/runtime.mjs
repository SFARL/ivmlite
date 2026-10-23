import assert from 'node:assert/strict';
import { existsSync, realpathSync } from 'node:fs';
import { createRequire, isBuiltin, registerHooks } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inside, readJson, sha256, SOURCE_SHA, validateRuntime, validateUpstream } from './prepare.mjs';

export const MIGRATIONS = [
  '20250111153517_init.ts',
  '20260202090915_add_new_columns_data_monitors.ts',
  '20260216095021_index_monitoring_data.ts',
  '20260613120000_add_confirmation_threshold_and_raw_status.ts',
  '20260617120000_add_monitoring_data_covering_index.ts',
];
export const COVERING_INDEX = 'idx_monitoring_data_monitor_tag_timestamp_status_latency';

// Hooks only rewrite imports originating in the pinned source tree. Node strips
// TypeScript natively; neither upstream source nor its dependency tree is edited.
export async function loadRuntime({ upstream, runtime, database }) {
  upstream = validateUpstream(upstream);
  runtime = realpathSync(runtime);
  const runtimeIdentity = validateRuntime(upstream, runtime);
  process.env.DATABASE_URL = `sqlite://${path.resolve(database)}`;
  const require = createRequire(path.join(runtime, 'package.json'));
  const sourceFiles = new Set(['package.json', 'package-lock.json'].map((file) => path.join(upstream, file)));
  function sourceURL(filename) {
    const absolute = realpathSync(filename);
    assert(inside(upstream, absolute), `Source escaped pinned checkout: ${filename}`);
    sourceFiles.add(absolute);
    return pathToFileURL(absolute).href;
  }
  const hook = registerHooks({
    resolve(specifier, context, nextResolve) {
      const parent = context.parentURL?.startsWith('file:') ? fileURLToPath(context.parentURL) : '';
      if (!inside(upstream, parent) || isBuiltin(specifier)) return nextResolve(specifier, context);
      let candidate;
      if (specifier.startsWith('$lib/')) candidate = path.join(upstream, 'src/lib', specifier.slice(5));
      else if (specifier.startsWith('.')) candidate = path.resolve(path.dirname(parent), specifier);
      else if (!specifier.startsWith('file:') && !path.isAbsolute(specifier)) {
        return nextResolve(pathToFileURL(require.resolve(specifier)).href, context);
      } else return nextResolve(specifier, context);
      if (!existsSync(candidate)) {
        if (candidate.endsWith('.js')) candidate = candidate.slice(0, -3) + '.ts';
        else if (!path.extname(candidate)) candidate += '.ts';
      }
      return nextResolve(sourceURL(candidate), context);
    },
  });
  try {
    const { MonitoringRepository } = await import(sourceURL(path.join(upstream, 'src/lib/server/db/repositories/monitoring.ts')));
    const { runWithWorkerKnex, getWorkerKnex } = await import(sourceURL(path.join(upstream, 'src/lib/server/db/poolContext.ts')));
    const migrations = [];
    for (const name of MIGRATIONS) migrations.push(await import(sourceURL(path.join(upstream, 'migrations', name))));
    const knex = require('knex');
    return {
      MonitoringRepository,
      runWithWorkerKnex,
      getWorkerKnex,
      migrate: async (db) => { for (const migration of migrations) await migration.up(db); },
      createDatabase(filename, extension, lifecycle = []) {
        return knex({
          client: 'better-sqlite3', connection: { filename }, useNullAsDefault: true,
          acquireConnectionTimeout: 2000,
          pool: {
            min: 0, max: 1, propagateCreateError: true,
            afterCreate(connection, done) {
              try {
                connection.loadExtension(extension, 'sqlite3_ivmlite_init');
                lifecycle.push({ event: 'extension-loaded', filename });
                done(null, connection);
              } catch (error) {
                // Knex has not handed this connection to Tarn yet, so pool
                // destruction cannot release a failed afterCreate connection.
                let closeError;
                try { connection.close(); }
                catch (failure) { closeError = failure.message; }
                lifecycle.push({ event: 'extension-load-failed', filename, message: error.message, open: connection.open, ...(closeError ? { closeError } : {}) });
                done(error, connection);
              }
            },
          },
        });
      },
      identity(extension) {
        const addon = require.resolve('better-sqlite3/build/Release/better_sqlite3.node');
        return {
          sourceSHA: SOURCE_SHA,
          sourceHashes: Object.fromEntries([...sourceFiles].sort().map((file) => [path.relative(upstream, file), sha256(file)])),
          runtime: runtimeIdentity,
          node: { version: process.version, modules: process.versions.modules, platform: process.platform, arch: process.arch },
          dependencies: Object.fromEntries(['knex', 'better-sqlite3', 'dotenv'].map((name) => [name, readJson(path.join(runtime, 'node_modules', name, 'package.json')).version])),
          addon: { filename: path.basename(addon), sha256: sha256(addon) },
          extension: { filename: path.basename(extension), sha256: sha256(extension) },
        };
      },
      close: () => hook.deregister(),
    };
  } catch (error) {
    hook.deregister();
    throw error;
  }
}
