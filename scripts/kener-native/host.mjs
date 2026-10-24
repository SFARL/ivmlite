import { adapterTransaction, createReader, refreshAdapter } from './adapter.mjs';

// The caller selects its pool or transaction explicitly. Upstream repositories
// prefer worker context over constructor arguments, so bind the actual context
// to this transaction, including when an outer worker pool is already active.
// Passing a transaction as `connection` creates a Knex savepoint. Reads of pending
// experimental state require `refresh()`; the adapter also refreshes before commit.
export function kenerTransaction(connection, runtime, work, { mode = 'strict', onRead } = {}) {
  return adapterTransaction(connection, runtime.MonitoringRepository, (repository, trx) =>
    runtime.runWithWorkerKnex(trx, () => work({
      repository,
      connection: trx,
      reader: createReader(trx, repository, { mode, onRead }),
      refresh: () => refreshAdapter(trx),
    })));
}
