# ivmlite application census

Snapshot 2026-10-17. Candidates are deduplicated from `docs/demos/real-world-demo-roadmap.md`, `docs/demos/*.md`, `docs/cases/evaluation/README.md` and `docs/community/2026-10-16-demand-and-reply-queue.md`. Upstream code was read read-only at the pinned revisions linked below. Codes follow `taxonomy.md`. This is demand evidence, not adoption.

28 candidates were classified and 2 were excluded because they contain no workload.

## Conventions

- `features` lists every code the upstream query uses beyond v0, including reader-side normalizations.
- `minimal_unlock` leaves out the following reader-side normalizations, which do not change meaning:
  - **N1:** a bound-parameter equality or IN becomes a GROUP BY key plus a reader lookup.
  - **N2:** ORDER BY without LIMIT, and zero-fill through `COALESCE(SUM(x), 0)` or an absent group, are done by the reader.
  - **N3:** select-list order is cosmetic.
  - **N4:** arithmetic over a group's own aggregates is done by the reader.
- `adapted_unlock` also allows the documented rewrites, whose costs are counted in the per-item details:
  - an application-maintained STRICT projection;
  - stored derived columns;
  - pivoting a conditional aggregate into a group key;
  - AVG as SUM/COUNT;
  - top-N in the reader;
  - several views combined by the reader.
- `in_place` is `adapted_unlock` plus F10a, and F10b where the base table has other column types, for an app that keeps its own tables instead of adding a projection.
- An F14 candidate is `out_of_scope` per the taxonomy. Candidates that are not_ivm_shaped, or that need F15 semantics after adaptation, keep their feature lists for reference but are excluded from the unlock counts.

## Per-candidate table

| # | id | fit | conf | runtime (loadable) | features | minimal_unlock | adapted_unlock | in_place |
|---|---|---|---|---|---|---|---|---|
| 1 | fluxflow | fits_with | high | better-sqlite3 (yes) | F2, F3e, F10a, F10b | F2, F3e, F10a, F10b | - | F10a, F10b |
| 2 | taproot | out_of_scope | high | modernc.org/sqlite v1.25.0 (no) | F4a, F10a, F10b, F14 | F4a, F10a, F10b, F14 | F14 | F10a, F10b, F14 |
| 3 | noop | out_of_scope | high | Android Room (no) | F3c, F4b, F5, F6, F10a, F10b, F14 | F3c, F5, F10a, F10b, F14 | F14 | F10a, F10b, F14 |
| 4 | zcash | fits_with | low | rusqlite (yes) | F6, F6t, F7b, F8s, F10a, F10b | F6, F6t, F7b, F8s, F10a, F10b | - | F10a, F10b |
| 5 | kener | fits_with | high | knex + better-sqlite3 (yes) | F3b, F3c, F3e, F4a, F5, F6, F6t, F9, F10a, F10b | F3b, F3c, F3e, F4a, F5, F6, F6t, F10a, F10b | F3c | F3c, F10a, F10b |
| 6 | bifrost | fits_with | high | gorm.io/driver/sqlite over mattn/go-sqlite3 (yes) | F2, F3b, F3e, F4a, F6, F6t, F10a, F10b, F13 | F2, F3b, F3e, F4a, F6, F6t, F10a, F10b, F13 | F13 | F10a, F10b, F13 |
| 7 | claude_monitor | fits_with | low | better-sqlite3 (yes) | F3d, F3e, F3f, F5, F7b, F10a, F10b, F13 | F3d, F3e, F3f, F5, F7b, F10a, F10b, F13 | F13 | F10a, F10b, F13 |
| 8 | zenod | fits_with | high | node:sqlite DatabaseSync (yes) | F4b, F6, F6t, F9, F10a | F10a | - | F10a |
| 9 | nicti | fits_with | high | rusqlite 0.40 bundled (yes) | F5, F10a | F5, F10a | - | F10a |
| 10 | postio | out_of_scope | high | Turso (no) | F4a, F4b, F8s, F10a, F14 | F4a, F8s, F10a, F14 | F14 | F10a, F14 |
| 11 | phoenix | fits_with | low | SQLAlchemy + aiosqlite (yes) | F4b, F6, F6t, F10a, F10b | F6t, F10a, F10b | - | F10a, F10b |
| 12 | pocketbase | not_ivm_shaped | high | modernc.org/sqlite (no) | F4b, F10a, F14 | F10a, F14 | F14 | F10a, F14 |
| 13 | datasette | fits_with | low | Python sqlite3 (yes) | F6, F8, F9, F10a | F6, F8, F9, F10a | - | F10a |
| 14 | org_roam | fits_with | low | emacsql over Emacs built-in sqlite or emacsql-sqlite (uncertain) | F3f, F4a, F4b, F7a, F7b, F8, F10a, F10b | F3f, F4a, F4b, F7a, F7b, F8, F10a, F10b | F3f | F3f, F10a, F10b |
| 15 | better_ccflare | fits_with | low | bun:sqlite (yes) | F3a, F3c, F4a, F4b, F6, F7a, F8, F9, F10a, F10b | F3a, F3c, F4a, F4b, F6, F7a, F8, F9, F10a, F10b | - | F10a, F10b |
| 16 | context_guru | out_of_scope | high | modernc.org/sqlite (no) | F7a, F9, F10a, F10b, F14, F15 | F7a, F9, F10a, F10b, F14, F15 | F14, F15 | F10a, F10b, F14, F15 |
| 17 | openwhispr | fits_with | low | better-sqlite3 in Electron main process (yes) | F4a, F6, F6t, F9, F10a, F10b | F4a, F6, F9, F10a, F10b | - | F10a, F10b |
| 18 | garcon | not_ivm_shaped | high | modernc.org/sqlite v1.59.0 (no) | F2, F10a, F14 | F2, F10a, F14 | F14 | F10a, F14 |
| 19 | openvibely | not_ivm_shaped | high | modernc.org/sqlite v1.60.1 (no) | F4a, F5, F6, F6t, F9, F10a, F10b, F14, F15 | F4a, F5, F6, F6t, F10a, F10b, F14, F15 | F14 | F10a, F10b, F14 |
| 20 | setu | not_ivm_shaped | high | better-sqlite3 via drizzle; browser db-idb adapter for parity (yes) | F5, F10a, F15 | F5, F10a, F15 | - | F10a |
| 21 | screenpipe | not_ivm_shaped | high | sqlx 0.9 + bundled libsqlite3-sys (yes) | F6, F6t, F8, F8s, F9, F10a | F6, F6t, F8, F8s, F9, F10a | F6t, F8s | F6t, F8s, F10a |
| 22 | pulse | not_ivm_shaped | high | modernc.org/sqlite v1.59.0 (no) | F9, F10a, F14 | F9, F10a, F14 | F14 | F10a, F14 |
| 23 | stash | not_ivm_shaped | high | mattn/go-sqlite3 (yes) | F3d, F7a, F7b, F8, F10a, F15 | F3d, F7a, F7b, F8, F10a, F15 | F3d, F7b, F15 | F3d, F7b, F10a, F15 |
| 24 | photo_triage | not_ivm_shaped | high | SQLAlchemy + Python sqlite3 (yes) | F10a, F10b | F10a, F10b | - | F10a, F10b |
| 25 | subwave | not_ivm_shaped | high | better-sqlite3 (yes) | F2, F6, F10a, F10b | F2, F6, F10a, F10b | - | F10a, F10b |
| 26 | tautulli | not_ivm_shaped | low | Python sqlite3 (yes) | F3c, F3e, F3f, F4a, F4b, F6t, F7a, F7b, F8, F9, F10a, F10b, F15 | F3c, F3e, F3f, F4a, F4b, F6t, F7a, F7b, F8, F9, F10a, F10b, F15 | F3c, F3f, F7a, F9 | F3c, F3f, F7a, F9, F10a, F10b |
| 27 | unscreen | fits_with | low | Python sqlite3 inside a Flet Android/Windows app (uncertain) | F3e, F4a, F6, F6t, F9, F10a, F10b | F3e, F4a, F6, F6t, F10a, F10b | - | F10a, F10b |
| 28 | orpheus | not_ivm_shaped | high | Python (uncertain) | F15 | F15 | F15 | F10a, F15 |

Fit totals: fits_now 0; fits_with 13; out_of_scope 4; not_ivm_shaped 11.

## Aggregates

### Per feature code

The last three columns count only eligible candidates. A candidate is eligible unless it is not_ivm_shaped or its adapted form still needs an F15 semantic, as context_guru's ordered state machine does. Each of these columns lists the candidates that become fits_now when that code alone is added. F10a and F10b have 0 in adapted because the STRICT projection removes them, at the cost recorded for each item.

| code | need | in minimal | in adapted | alone as-is | alone adapted | alone in-place |
|---|---:|---:|---:|---|---|---|
| F2 | 4 | 4 | 0 | - | - | - |
| F3a | 1 | 1 | 0 | - | - | - |
| F3b | 2 | 2 | 0 | - | - | - |
| F3c | 4 | 4 | 2 | - | kener | - |
| F3d | 2 | 2 | 1 | - | - | - |
| F3e | 6 | 6 | 0 | - | - | - |
| F3f | 3 | 3 | 2 | - | org_roam | - |
| F4a | 10 | 10 | 0 | - | - | - |
| F4b | 8 | 3 | 0 | - | - | - |
| F5 | 6 | 6 | 0 | - | - | - |
| F6 | 13 | 10 | 0 | - | - | - |
| F6t | 10 | 8 | 1 | - | - | - |
| F7a | 5 | 5 | 1 | - | - | - |
| F7b | 5 | 5 | 1 | - | - | - |
| F8 | 6 | 6 | 0 | - | - | - |
| F8s | 3 | 3 | 1 | - | - | - |
| F9 | 11 | 7 | 1 | - | - | - |
| F10a | 27 | 27 | 0 | zenod | - | datasette, nicti, zenod |
| F10b | 17 | 17 | 0 | - | - | - |
| F13 | 2 | 2 | 2 | - | bifrost, claude_monitor | - |
| F14 | 8 | 8 | 8 | - | noop, postio, taproot | - |
| F15 | 6 | 6 | 3 | - | - | - |

Eligible candidates that already fit with no unlock:

- **As-is:** -.
- **Adapted, with a STRICT projection:** better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod (9).
- **In place:** -.

With only F10a and F10b, which accept existing non-STRICT schemas in place, 9 candidates fit: better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod. Each single code added on top unlocks:

- **F3c:** +1 (kener)
- **F3f:** +1 (org_roam)
- **F13:** +2 (bifrost, claude_monitor)
- **F14:** +3 (noop, postio, taproot)

### Smallest bundles (2-3 codes)

A bundle unlocks a candidate when that candidate's unlock set is non-empty and is a subset of the bundle. Candidates that already fit with no unlock are counted in the baseline above, not here. A 3-code bundle is shown only when it unlocks more than every 2-code bundle it contains. Only bundles that tie for the most unlocks at each size are listed. Bundles marked sql_only exclude F13, F14 and F15.

**as_is_sql_only**:

- {F5, F10a}: 2 (nicti, zenod)
- {F6t, F10a, F10b}: 2 (phoenix, zenod)

**as_is_any**:

- {F5, F10a}: 2 (nicti, zenod)
- {F6t, F10a, F10b}: 2 (phoenix, zenod)

**adapted_sql_only**:

- {F3c, F3f}: 2 (kener, org_roam)

**adapted_any**:

- {F13, F14}: 5 (bifrost, claude_monitor, noop, postio, taproot)
- {F3c, F13, F14}: 6 (bifrost, claude_monitor, kener, noop, postio, taproot)
- {F3f, F13, F14}: 6 (bifrost, claude_monitor, noop, org_roam, postio, taproot)

**in_place_sql_only**:

- {F10a, F10b}: 9 (better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod)
- {F3c, F10a, F10b}: 10 (better_ccflare, datasette, fluxflow, kener, nicti, openwhispr, phoenix, unscreen, zcash, zenod)
- {F3f, F10a, F10b}: 10 (better_ccflare, datasette, fluxflow, nicti, openwhispr, org_roam, phoenix, unscreen, zcash, zenod)

**in_place_any**:

- {F10a, F10b}: 9 (better_ccflare, datasette, fluxflow, nicti, openwhispr, phoenix, unscreen, zcash, zenod)
- {F10a, F10b, F14}: 12 (better_ccflare, datasette, fluxflow, nicti, noop, openwhispr, phoenix, postio, taproot, unscreen, zcash, zenod)

### Blocked regardless of SQL

- **F14, runtime cannot load the extension:** 8 (context_guru, garcon, noop, openvibely, pocketbase, postio, pulse, taproot).
  - Uncertain: org_roam, orpheus, unscreen.
- **F13, aggregates must survive raw-row deletion:** 2 (bifrost, claude_monitor).
- **not_ivm_shaped:** 11 (garcon, openvibely, orpheus, photo_triage, pocketbase, pulse, screenpipe, setu, stash, subwave, tautulli).
- **F15 semantics that remain after adaptation:** context_guru, orpheus, stash.
- **Union, blocked by any of these:** 17.
- **Unlockable by SQL and schema work alone:** 11 (better_ccflare, datasette, fluxflow, kener, nicti, openwhispr, org_roam, phoenix, unscreen, zcash, zenod).

## Per-candidate details

### fluxflow: FluxFlow getStats()

- **Fit:** fits_with, with high confidence. The complaint is repeated full-scan counts/sums over a growing table, which upstream itself solved with maintained counters.
- **Evidence:** public generator (PR #45 bench-api.ts); issue-reported 700 ms at 1.5M flow_events.
- **Revision:** 635fa43f8cff44e2265af3a78a70c69d5544db1f (issue permalink); ed26a17 (PR #45 rollups).
- **Runtime:** better-sqlite3 (Node). Loadable: yes. better-sqlite3 exposes loadExtension.
- **Query:** `SELECT COUNT(*) FROM blocks; SELECT COUNT(*) FROM transactions; SELECT COUNT(*) FROM flow_events; SELECT flow_type, COUNT(*) AS count, SUM(amount) AS total_amount FROM flow_events GROUP BY flow_type; SELECT classification_level, data_source, COUNT(*) FROM flow_events WHERE classification_level > 0 GROUP BY classification_level, data_source; SELECT MIN(height), MAX(height) FROM blocks`
- **Schema:** flow_events(id INTEGER PK AUTOINCREMENT, txid TEXT, vout INTEGER, block_height INTEGER, block_time INTEGER, from_*/to_* TEXT, flow_type TEXT NOT NULL, amount REAL NOT NULL, classification_level INTEGER DEFAULT 0, data_source TEXT DEFAULT 'sync', ...) non-STRICT; transactions has REAL value columns; blocks INTEGER/TEXT.
- **Adapted query:** `SELECT flow_type, COUNT(*), SUM(amount_base_units) FROM flow_proj GROUP BY flow_type; SELECT classification_level, data_source, COUNT(*) FROM flow_proj WHERE classification_level > 0 GROUP BY 1, 2; SELECT k, COUNT(*) FROM blocks_proj GROUP BY k (k constant 0); same for transactions`
- **Adaptation:** STRICT projections of flow_events, blocks and transactions; amount stored as integer base units (x1e8, exact only if amounts have at most 8 decimals); flow_events total = reader sum of flow_type groups; blocks/transactions global COUNT(*) via a constant-key column; MIN/MAX(height) stays an index seek.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); three projected tables, two of them only to count rows.
- **Note:** Doc slice targets the PR #45 period summary (flow_type, day, counterparty, exchange; SUM sat, COUNT). PR #45 keeps rollups after retention deletes raw rows, so replacing it with retention enabled needs F13; it also needs F5 (day bucket) unless stored.
- **Sources:**
  - <https://github.com/2ndtlmining/fluxflow/issues/3>
  - <https://github.com/2ndtlmining/fluxflow/blob/635fa43f8cff44e2265af3a78a70c69d5544db1f/src/lib/services/databaseService.js#L422-L473>
  - <https://github.com/2ndtlmining/fluxflow/blob/635fa43f8cff44e2265af3a78a70c69d5544db1f/src/lib/services/databaseService.js#L64-L127>
  - <https://github.com/2ndtlmining/fluxflow/pull/45>
  - <https://github.com/2ndtlmining/fluxflow/blob/ed26a17f5dbf3fcc0a5e2f1f7b9536877198ff3c/src/lib/server/db/migrations.ts>

### taproot: Taproot Assets universe_stats

- **Fit:** out_of_scope, with high confidence. The SQL is tractable, but tapd opens SQLite through modernc.org/sqlite, so a native extension cannot load without changing the driver.
- **Evidence:** public source/query; issue reports trouble above ~10M events; no timings.
- **Revision:** e569cf553d324297c303d190678320fcc50b9c10.
- **Runtime:** modernc.org/sqlite v1.25.0 (pure Go); also PostgreSQL. Loadable: no. pure-Go driver cannot load a native extension.
- **Query:** `CREATE VIEW universe_stats AS SELECT COUNT(CASE WHEN u.event_type = 'SYNC' THEN 1 ELSE NULL END) AS total_asset_syncs, COUNT(CASE WHEN u.event_type = 'NEW_PROOF' THEN 1 ELSE NULL END) AS total_asset_proofs, roots.asset_id, roots.group_key, roots.proof_type FROM universe_events u JOIN universe_roots roots ON u.universe_root_id = roots.id GROUP BY roots.asset_id, roots.group_key, roots.proof_type`
- **Schema:** universe_events(event_id BIGINT PK, event_type VARCHAR CHECK IN ('SYNC','NEW_PROOF','NEW_ROOT'), universe_root_id BIGINT, event_time TIMESTAMP); universe_roots(id BIGINT PK, namespace_root VARCHAR, asset_id BLOB, group_key BLOB, proof_type TEXT); non-STRICT.
- **Adapted query:** `SELECT r.asset_id_hex, r.group_key_hex, r.proof_type, e.event_type, COUNT(*) FROM events_proj e JOIN roots_proj r ON e.universe_root_id = r.id GROUP BY 1, 2, 3, 4`
- **Adaptation:** Pivot: event_type becomes a group key and the reader pivots SYNC/NEW_PROOF into columns, emitting 0/0 for groups that only have NEW_ROOT (this keeps those groups, unlike the docs' two filtered views, which drop them). BLOB keys projected to hex TEXT; both tables projected STRICT.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) for both join inputs; reader pivot.
- **Note:** docs/cases/evaluation instead uses two filtered views (event_type = 'SYNC' / 'NEW_PROOF'); that loses groups with neither type.
- **Sources:**
  - <https://github.com/lightninglabs/taproot-assets/issues/642>
  - <https://github.com/lightninglabs/taproot-assets/blob/e569cf553d324297c303d190678320fcc50b9c10/tapdb/sqlc/migrations/000010_universe_stats.up.sql>
  - <https://github.com/lightninglabs/taproot-assets/blob/e569cf553d324297c303d190678320fcc50b9c10/tapdb/sqlc/migrations/000007_universe.up.sql#L1-L86>
  - <https://github.com/lightninglabs/taproot-assets/blob/e569cf553d324297c303d190678320fcc50b9c10/go.mod>

### noop: noop gravity witness

- **Fit:** out_of_scope, with high confidence. The COUNT half is IVM-shaped, but both mobile runtimes block native extension loading.
- **Evidence:** device log 60 reads = 2,251 ms; Python in-memory comparisons.
- **Revision:** 8e4b0c16e7640607600fa6bde5424095eba6034e (current main; issue cites build 519).
- **Runtime:** Android Room (framework SQLite) and iOS GRDB 6.29.3 over system SQLite. Loadable: no. neither stock Android framework SQLite nor iOS system SQLite loads extensions; a bundled SQLite with static registration would be required.
- **Query:** `SELECT COUNT(*) AS c, COALESCE(MAX(ts), 0) AS m FROM gravitySample WHERE deviceId = :deviceId AND ts >= :from AND ts <= :to  -- run once per local day for 60 days`
- **Schema:** gravitySample(deviceId TEXT, ts INTEGER, x REAL, y REAL, z REAL, synced INTEGER, dynAccel REAL, PK(deviceId, ts)); Room/GRDB, non-STRICT.
- **Adapted query:** `SELECT device_id, day_bucket, COUNT(*) FROM gravity_witness GROUP BY device_id, day_bucket`
- **Adaptation:** Stored local-day bucket computed by the app with real timezone/DST rules; MAX(ts) stays an indexed seek read in the same snapshot; narrow STRICT projection (device_id, ts, day_bucket).
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); one projection row per 10 s sample (~518k rows/device for 60 days).
- **Sources:**
  - <https://github.com/ryanbr/noop/issues/2314>
  - <https://github.com/ryanbr/noop/blob/8e4b0c16e7640607600fa6bde5424095eba6034e/android/app/src/main/java/com/noop/data/WhoopDao.kt#L1369-L1373>
  - <https://github.com/ryanbr/noop/blob/8e4b0c16e7640607600fa6bde5424095eba6034e/Packages/WhoopStore/Tests/WhoopStoreTests/Resources/schema_oracle.json>
  - <https://github.com/ryanbr/noop/blob/8e4b0c16e7640607600fa6bde5424095eba6034e/Packages/WhoopStore/Package.swift>

### zcash: Zcash transparent balance

- **Fit:** fits_with, with low confidence. The original needs a four-table join, NOT IN and a tip-relative predicate, and the faithful rewrite moves most of the logic into app-maintained flags.
- **Evidence:** issue-reported ~197 ms warm at 500k UTXOs.
- **Revision:** 894922072fdf37b4d0eb1a3b6f0d876a7c51bc6e (last main commit before the issue).
- **Runtime:** rusqlite (bundled SQLite). Loadable: yes. rusqlite can load extensions or register one statically; mobile wallets embed the crate.
- **Query:** `SELECT accounts.uuid, SUM(u.value_zat) FROM transparent_received_outputs u JOIN accounts ON accounts.id = u.account_id JOIN transactions t ON t.id_tx = u.transaction_id JOIN addresses ON addresses.id = u.address_id WHERE (<mined or unexpired at :target_height>) AND u.id NOT IN (<spent utxos>) AND (<exclude ephemeral>) GROUP BY accounts.uuid; second query with mined_height/target_height/min_confirmations OR-predicates when min_confirmations > 0`
- **Schema:** rusqlite wallet schema; accounts.uuid BLOB; non-STRICT tables; value_zat INTEGER.
- **Adapted query:** `SELECT account_id, SUM(value_zat) FROM transparent_outputs WHERE spendable = 1 GROUP BY account_id (and a second view on pending = 1)`
- **Adaptation:** App-maintained eligibility flags (spendable, pending) on a STRICT projection, flipped on receive, spend, rewind and at every new block for unmined/under-confirmed outputs (target_height moves); reader maps account_id to uuid.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); per-block re-evaluation of unmined/unconfirmed outputs; duplicates the spentness/confirmation logic in app code.
- **Sources:**
  - <https://github.com/zcash/librustzcash/issues/2476>
  - <https://github.com/zcash/librustzcash/blob/894922072fdf37b4d0eb1a3b6f0d876a7c51bc6e/zcash_client_sqlite/src/wallet/transparent.rs#L1442-L1545>

### kener: Kener monitor bars

- **Fit:** fits_with, with high confidence. Upstream is building exactly this rollup, so the workload is IVM-shaped; MIN/MAX under deletes is the remaining gap.
- **Evidence:** issue-reported 5,631 ms over 4.4M rows; upstream rollup PR open.
- **Revision:** 737899bf36ed62bc774027f6d14e93028f120bca (PR #842 base).
- **Runtime:** knex + better-sqlite3 (also pg/mysql). Loadable: yes. better-sqlite3 loadExtension.
- **Query:** `SELECT monitor_tag, CAST((timestamp - ?) / ? AS INT) * ? + ? AS ts, SUM(CASE WHEN status = 'UP' THEN 1 ELSE 0 END), ... DOWN/DEGRADED/MAINTENANCE ..., AVG(latency), MAX(latency), MIN(latency) FROM monitoring_data WHERE monitor_tag IN (...) AND timestamp >= ? AND timestamp < ? GROUP BY monitor_tag, ts ORDER BY monitor_tag, ts`
- **Schema:** monitoring_data(monitor_tag VARCHAR(255) NOT NULL, timestamp INTEGER NOT NULL, status TEXT, latency FLOAT(8,2), type TEXT, PK(monitor_tag, timestamp)); knex, non-STRICT.
- **Adapted query:** `SELECT monitor_id, bucket_ts, status, SUM(latency_x100), COUNT(*), MIN(latency_x100), MAX(latency_x100) FROM monitoring_facts GROUP BY monitor_id, bucket_ts, status`
- **Adaptation:** Stored 15-minute bucket (every IANA offset is a multiple of 15 min), status pivoted to a group key, AVG = SUM/COUNT, latency scaled to integer hundredths, reader filters monitors/time window and composes viewer-timezone days. The docs' slice drops MIN/MAX; keeping them needs retraction-safe F3c (history is rewritten and deleted).
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); 550k groups for 4.1M rows; reader composes 96 buckets per day.
- **Sources:**
  - <https://github.com/rajnandan1/kener/issues/840>
  - <https://github.com/rajnandan1/kener/pull/842>
  - <https://github.com/rajnandan1/kener/blob/737899bf36ed62bc774027f6d14e93028f120bca/src/lib/server/db/repositories/monitoring.ts#L600-L636>
  - <https://github.com/rajnandan1/kener/blob/737899bf36ed62bc774027f6d14e93028f120bca/migrations/20250111153517_init.ts#L4-L13>

### bifrost: Bifrost SQLite log stats

- **Fit:** fits_with, with high confidence. The issue asks for aggregates that outlive deleted raw rows, which ivmlite's retraction semantics cannot provide without F13.
- **Evidence:** issue-reported 17.7 GB / 26.5k rows; 200 s before ANALYZE, 0.4-15 s after.
- **Revision:** a30e57dd9339f0d0efabdfdb1c96e63c462c41ae (main at census; issue reports v2.2.1).
- **Runtime:** gorm.io/driver/sqlite over mattn/go-sqlite3 (cgo). Loadable: yes. mattn exposes LoadExtension unless built with sqlite_omit_load_extension.
- **Query:** `SELECT COUNT(*) AS completed_count, SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), AVG(latency), SUM(total_tokens), SUM(prompt_tokens), SUM(completion_tokens), SUM(cost) FROM logs WHERE <applyFilters: time window, provider, model, ...> AND status IN (terminal statuses)`
- **Schema:** logs(id VARCHAR(255) PK, timestamp DATETIME, provider/model/status VARCHAR, latency REAL, cost REAL, *_tokens INTEGER, input_history TEXT ...); gorm, non-STRICT.
- **Adapted query:** `SELECT hour_bucket, provider, model, status, COUNT(*), SUM(latency_us), SUM(total_tokens), SUM(prompt_tokens), SUM(completion_tokens), SUM(cost_micro_usd) FROM logs_proj GROUP BY 1, 2, 3, 4`
- **Adaptation:** Hourly bucket stored, REAL cost/latency scaled to integers, status as a group key, AVG = SUM/COUNT, reader filters group dimensions; filters on non-grouped dimensions are not answerable. The issue's requirement is that the hourly totals survive raw-log retention, which is F13.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); only dimensions chosen up front can be filtered.
- **Note:** The 200 s query was fixed by ANALYZE (a planner problem, not IVM).
- **Sources:**
  - <https://github.com/maximhq/bifrost/issues/7460>
  - <https://github.com/maximhq/bifrost/blob/a30e57dd9339f0d0efabdfdb1c96e63c462c41ae/framework/logstore/rdb.go#L1799-L1846>
  - <https://github.com/maximhq/bifrost/blob/a30e57dd9339f0d0efabdfdb1c96e63c462c41ae/framework/logstore/tables.go#L304-L402>
  - <https://github.com/maximhq/bifrost/blob/a30e57dd9339f0d0efabdfdb1c96e63c462c41ae/framework/go.mod>

### claude_monitor: Claude Code Agent Monitor tiered retention

- **Fit:** fits_with, with low confidence. This is a design proposal with no measured query, but rollup-before-delete is plainly F13.
- **Evidence:** proposal only; no row counts or timings.
- **Revision:** a06db038a93b4d7316492dc55758f597e104894c.
- **Runtime:** better-sqlite3, fallback node:sqlite. Loadable: yes.
- **Query:** `Proposed rollup activity_rollups(day, project, provider, model, event_count, session_count, tool_counts JSON, input/output/cache tokens INTEGER, cost_usd REAL): i.e. GROUP BY date(created_at), project, provider, model with COUNT(*), COUNT(DISTINCT session_id), a per-tool JSON count map, SUM(tokens), SUM(cost) over events JOIN sessions JOIN token_usage`
- **Schema:** events(id INTEGER PK, session_id TEXT, agent_id TEXT, event_type TEXT, tool_name TEXT, summary TEXT, data TEXT, created_at TEXT); token_usage(session_id, model, ..., *_tokens INTEGER); non-STRICT.
- **Adapted query:** `SELECT day, project, tool_name, session_id, COUNT(*) FROM events_proj GROUP BY 1, 2, 3, 4 (reader derives event, session and per-tool counts); separate view summing token_usage tokens per day/project/model`
- **Adaptation:** Stored day and project on an events projection; COUNT(DISTINCT session) = number of finer groups; tool JSON map built by the reader; cost as integer micro-dollars. Rollups must survive deletion of cold raw rows (F13).
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); the finer session-level view grows with sessions.
- **Sources:**
  - <https://github.com/hoangsonww/Claude-Code-Agent-Monitor/issues/305>
  - <https://github.com/hoangsonww/Claude-Code-Agent-Monitor/blob/a06db038a93b4d7316492dc55758f597e104894c/server/db.js#L195-L224>

### zenod: Zenod tenant usage quota

- **Fit:** fits_with, with high confidence. Upstream fixed it with exactly this maintained (tenant, kind) total.
- **Evidence:** production 1.76M-row ledger, ~2.9 s blocking per request (author report).
- **Revision:** 44cee5384ae9061a92520565b70135d8aa3c2ea4 (PR #1380 base); fixed in 9b036915.
- **Runtime:** node:sqlite DatabaseSync. Loadable: yes. node:sqlite supports loadExtension with allowExtension: true (recent Node).
- **Query:** `checkQuota -> summary(tenant, since=0): SELECT COUNT(*) AS events, COALESCE(SUM(units), 0) AS units FROM tenant_usage WHERE tenant_id = ? AND ts >= ?; SELECT kind, COUNT(*), COALESCE(SUM(units), 0) FROM tenant_usage WHERE tenant_id = ? AND ts >= ? GROUP BY kind ORDER BY kind`
- **Schema:** tenant_usage(id INTEGER PK AUTOINCREMENT, tenant_id TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, units INTEGER NOT NULL DEFAULT 1, metadata_json TEXT); non-STRICT.
- **Adapted query:** `SELECT tenant_id, kind, COUNT(*), SUM(units) FROM tenant_usage GROUP BY tenant_id, kind`
- **Adaptation:** STRICT projection; tenant lifted to a group key (N1); hot path has since = 0. The since-filtered summary still needs F6t or stored buckets.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); explicit refresh vs transactional quota semantics must be decided.
- **Sources:**
  - <https://github.com/zenod-ai/zenod/pull/1380>
  - <https://github.com/zenod-ai/zenod/issues/1391>
  - <https://github.com/zenod-ai/zenod/blob/44cee5384ae9061a92520565b70135d8aa3c2ea4/packages/mcp-chassis/src/usage.ts#L79-L88>
  - <https://github.com/zenod-ai/zenod/blob/44cee5384ae9061a92520565b70135d8aa3c2ea4/packages/mcp-chassis/src/usage.ts#L115-L136>
  - <https://github.com/zenod-ai/zenod/blob/44cee5384ae9061a92520565b70135d8aa3c2ea4/packages/mcp-chassis/src/usage.ts#L183-L198>
  - <https://github.com/zenod-ai/zenod/blob/9b036915af84f0f7e6e34cd49e2d6879158fd16c/packages/mcp-chassis/src/usage.ts>

### nicti: Nicti facet counts

- **Fit:** fits_with, with high confidence. Upstream built trigger-maintained counts at exactly this grain.
- **Evidence:** upstream synthetic 2M-asset experiment (151 ms p95 vs 100 ms budget).
- **Revision:** c3ec874a3b5827c89326890dc0323817774e013c.
- **Runtime:** rusqlite 0.40 bundled. Loadable: yes.
- **Query:** `Maintained grain (backfill of facet_counts): SELECT r.volume_id, COALESCE(a.model, ''), COALESCE(a.rating, -128), COUNT(*) FROM asset a JOIN root r ON r.id = a.root_id GROUP BY r.volume_id, COALESCE(a.model, ''), COALESCE(a.rating, -128); ADR's original hot query: faceted filter model = ? AND rating >= ? with keyword counts`
- **Schema:** asset(id INTEGER PK, root_id INTEGER NOT NULL, model TEXT, rating INTEGER NOT NULL DEFAULT 0, ... all INTEGER/TEXT); root(id INTEGER PK, volume_id INTEGER NOT NULL, ...); non-STRICT.
- **Adapted query:** `SELECT r.volume_id, a.model, a.rating, COUNT(*) FROM asset a JOIN root r ON r.id = a.root_id GROUP BY 1, 2, 3`
- **Adaptation:** Drop COALESCE: NULL groups are native and the reader merges the NULL and '' model groups; STRICT projections of asset and root.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) for both join inputs; ADR-0107 moved the primary store toward DuckDB, so the SQLite path may not persist.
- **Note:** The docs' demo uses the ADR-0103 (model, rating, keyword) grain; current main keeps (volume_id, model, rating).
- **Sources:**
  - <https://github.com/jordanfelle/nicti/issues/103>
  - <https://github.com/jordanfelle/nicti/blob/c3ec874a3b5827c89326890dc0323817774e013c/docs/adr/0103-facet-count-cache.md>
  - <https://github.com/jordanfelle/nicti/blob/c3ec874a3b5827c89326890dc0323817774e013c/crates/nicti-lair/src/schema.rs#L250-L262>
  - <https://github.com/jordanfelle/nicti/blob/c3ec874a3b5827c89326890dc0323817774e013c/crates/nicti-lair/src/sqlite.rs#L332-L360>

### postio: Postio mailbox footprint

- **Fit:** out_of_scope, with high confidence. Postio runs on Turso's engine, and the issue is now resolved upstream by maintained counters.
- **Evidence:** 1.48 s on a real store (~81k messages); issue closed 2026-10-16 after maintained counters merged.
- **Revision:** c32a6f4af191fb07c48176c7b77a1e502817c600 (research snapshot); closed by PR #1871.
- **Runtime:** Turso (Rust rewrite of SQLite). Loadable: no. Turso's engine cannot load a C SQLite extension.
- **Query:** `SELECT count(*), coalesce(sum(size), 0) FROM messages WHERE account_id = ?1 AND deleted_locally = 0; SELECT coalesce(sum(a.size), 0) FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.account_id = ?1 AND m.deleted_locally = 0; SELECT coalesce(sum(CASE m.body_state WHEN 'full' THEN m.size WHEN 'partial' THEN m.size - coalesce((SELECT sum(a.size) FROM attachments a WHERE a.message_id = m.id AND a.blob_id IS NULL), 0) ELSE 0 END), 0) FROM messages m WHERE m.account_id = ?1 AND m.deleted_locally = 0`
- **Schema:** messages(id INTEGER PK, account_id INTEGER, ..., size INTEGER NOT NULL DEFAULT 0, deleted_locally INTEGER NOT NULL DEFAULT 0, body_state TEXT ...); attachments(id, message_id INTEGER, size INTEGER, blob_id ...); non-STRICT.
- **Adapted query:** `V1: SELECT account_id, body_state, COUNT(*), SUM(size) FROM messages_proj WHERE deleted_locally = 0 GROUP BY 1, 2; V2: SELECT m.account_id, m.body_state, a.has_blob, SUM(a.size) FROM attachments_proj a JOIN messages_proj m ON m.id = a.message_id WHERE m.deleted_locally = 0 GROUP BY 1, 2, 3`
- **Adaptation:** local_bytes = V1[full] + V1[partial] - V2[partial, has_blob = 0]; stored has_blob flag on an attachments projection; reader combines two views.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) for messages and attachments; upstream already ships trigger counters.
- **Sources:**
  - <https://github.com/dlapiduz/postio/issues/1508>
  - <https://github.com/dlapiduz/postio/blob/c32a6f4af191fb07c48176c7b77a1e502817c600/crates/postio-storage/src/repository/messages.rs#L2088-L2126>
  - <https://github.com/dlapiduz/postio/blob/c32a6f4af191fb07c48176c7b77a1e502817c600/crates/postio-storage/src/schema.rs#L717-L806>
  - <https://github.com/dlapiduz/postio/blob/5e6e34f478f53fb8c7f53b779655df390d4c86d9/crates/postio-storage/Cargo.toml#L41-L55>

### phoenix: Phoenix project token counts

- **Fit:** fits_with, with low confidence. The query was IVM-shaped, but upstream removed it instead, and exactness depends on how windows align.
- **Evidence:** reporter log: ~169 s over 4.95M spans; fix removed the feature.
- **Revision:** ca6a77924675faac05d29c5810a15b25d73be578 (PR #8357 base).
- **Runtime:** SQLAlchemy + aiosqlite (Python sqlite3); also PostgreSQL. Loadable: yes. needs a Python build with enable_load_extension (official Docker Python has it; macOS system Python does not).
- **Query:** `SELECT traces.project_rowid, coalesce(sum(spans.llm_token_count_prompt), ?) AS prompt, coalesce(sum(spans.llm_token_count_completion), ?) AS completion, coalesce(sum(...prompt), ?) + coalesce(sum(...completion), ?) AS total FROM traces JOIN spans ON traces.id = spans.trace_rowid WHERE spans.start_time >= ? AND traces.project_rowid IN (?, ?, ?) GROUP BY traces.project_rowid`
- **Schema:** spans(id, trace_rowid INTEGER, start_time TIMESTAMP, attributes JSON, llm_token_count_prompt INTEGER NULL, ...); traces(id, project_rowid INTEGER, start_time TIMESTAMP); SQLAlchemy, non-STRICT.
- **Adapted query:** `SELECT t.project_id, s.minute_bucket, SUM(s.prompt_tokens), SUM(s.completion_tokens) FROM traces_proj t JOIN spans_proj s ON t.id = s.trace_id GROUP BY 1, 2`
- **Adaptation:** Stored minute bucket (UI windows start on a minute), reader sums buckets >= start; project IN lifted (N1); totals and coalesce in reader (N2/N4); STRICT projections of traces and spans.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) of 5M spans; reader sums O(minutes in window) rows; exact only for minute-aligned starts.
- **Sources:**
  - <https://github.com/Arize-ai/phoenix/issues/8068>
  - <https://github.com/Arize-ai/phoenix/pull/8357>
  - <https://github.com/Arize-ai/phoenix/blob/ca6a77924675faac05d29c5810a15b25d73be578/src/phoenix/db/models.py#L532-L612>

### pocketbase: PocketBase author book counts

- **Fit:** not_ivm_shaped, with high confidence. The direct count view takes 75 ms; the cost is back-relation expand scheduling, i.e. per-record relation queries.
- **Evidence:** reporter 1,500 ms with expand; maintainer 425 ms; direct count view 75 ms.
- **Revision:** 6f370ea741f33adf889eef02e3c1b884a47aa522 (go.mod); view SQL from the discussion.
- **Runtime:** modernc.org/sqlite (default; replaceable via DBConnect with a cgo driver). Loadable: no.
- **Query:** `SELECT author AS id, author, COUNT(*) AS cnt FROM books GROUP BY author  -- view collection, expanded as a back-relation`
- **Schema:** books(id TEXT PK, author TEXT relation, ...); PocketBase collection tables, non-STRICT.
- **Adapted query:** `SELECT author, COUNT(*) FROM books_proj GROUP BY author`
- **Adaptation:** STRICT projection; duplicated output column done by reader.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently).
- **Sources:**
  - <https://github.com/pocketbase/pocketbase/discussions/7592>
  - <https://github.com/pocketbase/pocketbase/blob/6f370ea741f33adf889eef02e3c1b884a47aa522/go.mod>

### datasette: Datasette facet counts

- **Fit:** fits_with, with low confidence. Facet filters are user-chosen per request, so only a predeclared facet set is maintainable, and the reported remedies were limiting or disabling facets.
- **Evidence:** reporter timings only; 805 MB covid.db not downloaded.
- **Revision:** SQLite forum first revision as archived in docs/cases/datasette.
- **Runtime:** Python sqlite3 (datasette --load-extension). Loadable: yes.
- **Query:** `select county as value, count(*) as count from (select rowid, date, county, state, fips, cases, deaths from ny_times_us_counties where state = 'Kentucky') where county is not null group by county order by count desc, value limit 31`
- **Schema:** ny_times_us_counties(date TEXT, county TEXT, state TEXT, fips INTEGER, cases INTEGER, deaths INTEGER); csvs-to-sqlite, non-STRICT (full DDL not archived).
- **Adapted query:** `SELECT county, COUNT(*) FROM counties_proj WHERE state = 'Kentucky' GROUP BY county`
- **Adaptation:** Flatten the projection subquery; drop county IS NOT NULL (reader drops the NULL group); top-31 by the reader over all groups; STRICT projection.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) of an 805 MB table; only predeclared facet/filter pairs benefit, while Datasette facets are ad hoc.
- **Sources:**
  - <https://github.com/simonw/datasette/issues/1684>
  - <https://github.com/simonw/datasette/issues/1513>
  - <https://sqlite.org/forum/forumpost/c0e0fcbe36>
  - docs/cases/datasette/extracted/kentucky-facet-county.sql (local archive)

### org_roam: Org-roam denormalized node records

- **Fit:** fits_with, with low confidence. This is record assembly rather than aggregation, and both the equivalence of the rewrite and the Emacs runtime are unverified.
- **Evidence:** source reports on 9,554 notes; no public dataset.
- **Revision:** archived extracts (gist 192bafa002578f7691dda884f7bcc5f044dc721c).
- **Runtime:** emacsql over Emacs built-in sqlite or emacsql-sqlite. Loadable: uncertain. Emacs 29+ has sqlite-load-extension, but newer Emacs restricts which extension names load; not verified.
- **Query:** `SELECT id, ..., '(' || group_concat(tags, ' ') || ')' AS tags, aliases, refs FROM (SELECT ..., '(' || group_concat(aliases, ' ') || ')' AS aliases ... FROM (SELECT nodes.*, files.atime, files.mtime, files.title, tags.tag, aliases.alias, '(' || group_concat(RTRIM(refs.type, '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM nodes LEFT JOIN files ... LEFT JOIN tags ... LEFT JOIN aliases ... LEFT JOIN refs ... GROUP BY nodes.id, tags.tag, aliases.alias) GROUP BY id, tags) GROUP BY id`
- **Schema:** nodes/files/tags/aliases/refs with untyped columns (e.g. tags(node_id NOT NULL, tag)); non-STRICT.
- **Adapted query:** `SELECT node_id, group_concat(tag, ' ') FROM tags_proj GROUP BY node_id; same for aliases and formatted refs; reader joins nodes and files`
- **Adaptation:** Three single-table group_concat views (formatted ref stored on a refs projection), assembled with nodes/files by the reader; outer-join NULL lists handled by the reader. The docs' tag-count slice does not preserve the answer.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) for tags, aliases and refs; the nested grouping's de-duplication must be re-verified.
- **Sources:**
  - <https://github.com/org-roam/org-roam/issues/1997>
  - docs/cases/org-roam/extracted/original-query.el.txt and schema.sql (local archive)
  - <https://org-roam.discourse.group/t/3483>

### better_ccflare: better-ccflare dashboard stats

- **Fit:** fits_with, with low confidence. The stall is not yet attributed to any one of the five statistics paths, so the aggregate cost may not be the bottleneck.
- **Evidence:** live uncached handler 2.7-2.9 s; per-query attribution missing.
- **Revision:** 386c67c02136b7ba9b41fad3a9f690cae51c8742.
- **Runtime:** bun:sqlite. Loadable: yes. bun:sqlite loadExtension; on macOS needs Database.setCustomSQLite.
- **Query:** `Lifetime account stats: WITH raw AS (SELECT a.id, COUNT(r.id), SUM(CASE WHEN a.id IS NOT NULL OR r.account_used IS NULL OR r.account_used = ? THEN 1 ELSE 0 END), SUM(CASE WHEN (...) AND r.success = TRUE THEN 1 ELSE 0 END) FROM requests r LEFT JOIN accounts a ON a.id = r.account_used GROUP BY a.id), grouped AS (... COALESCE, MAX(a.total_requests) ... LEFT JOIN accounts ... GROUP BY 1, 2), rated AS (... SUM() OVER (PARTITION BY id)) SELECT ... WHERE request_count > 0 ORDER BY request_count DESC LIMIT ?; model counts: WITH model_counts AS (SELECT model, COUNT(*) FROM requests WHERE model IS NOT NULL GROUP BY model) ... ORDER BY count DESC LIMIT ?`
- **Schema:** requests(id TEXT PK, timestamp INTEGER, account_used TEXT, success BOOLEAN, model TEXT, cost_usd REAL, output_tokens_per_second REAL, account_generation BIGINT, ...); non-STRICT.
- **Adapted query:** `SELECT account_used, COUNT(*), SUM(success) FROM requests_proj GROUP BY account_used; SELECT model, COUNT(*) FROM requests_proj GROUP BY model`
- **Adaptation:** The CASE conditions depend only on the group key and on whether that account exists, so the reader evaluates them per group after joining the small accounts table; COUNT(r.id) = COUNT(*) because id is the PK; success projected to 0/1; the reader computes window sums, percentages, ordering and LIMIT; the NULL model group is dropped.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) of requests (account_used, success, model); the reader handles fallback/no_account semantics.
- **Sources:**
  - <https://github.com/StartupBros-com/better-ccflare/issues/480>
  - <https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/repositories/stats.repository.ts#L234-L285>
  - <https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/repositories/stats.repository.ts#L440-L457>
  - <https://github.com/StartupBros-com/better-ccflare/blob/386c67c02136b7ba9b41fad3a9f690cae51c8742/packages/database/src/migrations.ts#L464-L509>

### context_guru: context-guru replay ledger

- **Fit:** out_of_scope, with high confidence. Ordered ledger semantics cannot be expressed as grouped COUNT/SUM, and the driver is pure Go.
- **Evidence:** relative timings only.
- **Revision:** 1151c36eca75e1fdf5530293b64f93d573728bd2.
- **Runtime:** modernc.org/sqlite. Loadable: no.
- **Query:** `SELECT r.id, r.tenant_id, r.session_id, r.model, r.ts, r.bypassed, r.tokens_before, r.tokens_after, ..., c.component, c.saved_gross, c.saved_unique, c.saved_usd FROM requests r LEFT JOIN request_components c ON c.request_id = r.id AND ... AND c.saved_usd <> 0 AND c.saved_gross > 0 WHERE %s ORDER BY r.tenant_id, r.session_id, r.model, r.ts, r.id  -- then an ordered Go walk with 3-turn lookahead and max-credit de-duplication`
- **Schema:** requests/request_components; non-STRICT; REAL saved_usd.
- **Adaptation:** None faithful: the result is an ordered state machine (compaction detection with lookahead, max-credit de-dup); F15 = ordered state machine.
- **Sources:**
  - <https://github.com/rossoctl/context-guru/issues/588>
  - <https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L298-L307>
  - <https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/dash/replaycap.go#L213-L290>
  - <https://github.com/rossoctl/context-guru/blob/1151c36eca75e1fdf5530293b64f93d573728bd2/go.mod#L33>

### openwhispr: OpenWhispr get_stats by speaker

- **Fit:** fits_with, with low confidence. The issue frames this as main-process availability, and its first fixes are a default range and moving the read off the main process.
- **Evidence:** reporter: 1,200 ms isolated on 20k notes / 2M segments.
- **Revision:** d5e50fed042598c43152a20b0bf62c0cd1baa251.
- **Runtime:** better-sqlite3 in Electron main process. Loadable: yes. Electron packaging of a native extension unverified.
- **Query:** `SELECT s.speaker_name AS speaker, COUNT(*) AS segments, SUM(CASE WHEN trim(s.text) = '' THEN 0 ELSE length(trim(s.text)) - length(replace(trim(s.text), ' ', '')) + 1 END) AS words FROM transcript_segments s JOIN notes n ON n.id = s.note_id WHERE s.speaker_name IS NOT NULL AND n.deleted_at IS NULL [AND datetime(n.created_at) >= datetime(?) AND < datetime(?)] GROUP BY s.speaker_name ORDER BY segments DESC LIMIT ?`
- **Schema:** transcript_segments(note_id INTEGER, seq INTEGER, speaker_id TEXT, speaker_name TEXT, text TEXT NOT NULL, ..., PK(note_id, seq)); notes(id INTEGER PK, ..., audio_duration_seconds REAL, created_at DATETIME, deleted_at ...); non-STRICT.
- **Adapted query:** `SELECT s.speaker_name, COUNT(*), SUM(s.word_count) FROM segments_proj s JOIN notes_proj n ON n.id = s.note_id WHERE n.deleted = 0 GROUP BY s.speaker_name`
- **Adaptation:** Stored word_count per segment (same approximate rule), notes projection with a deleted flag, reader drops the NULL speaker group and applies top-N; the all-time call is the hot path. Date-bounded calls need a stored note-day bucket in the group key.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently) of 2M segments plus notes; word_count recomputed on each text edit.
- **Sources:**
  - <https://github.com/futuregerald/openwhispr/issues/84>
  - <https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L2861-L2886>
  - <https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L840-L852>
  - <https://github.com/futuregerald/openwhispr/blob/d5e50fed042598c43152a20b0bf62c0cd1baa251/src/helpers/database.js#L233-L242>

### garcon: Garcon dashboard polling

- **Fit:** not_ivm_shaped, with high confidence. The issue is about connection and mutex serialization; the proposed fixes are a read pool, a writer goroutine and a cached count.
- **Evidence:** no cardinality or latency numbers.
- **Revision:** 5a0ec29c4b521531deea704c9cf5cfb13253a03e.
- **Runtime:** modernc.org/sqlite v1.59.0, SetMaxOpenConns(1). Loadable: no.
- **Query:** `SELECT count(*) FROM requests  -- under s.mu every ~2.5 s; plus sessions(): window/JSON/DISTINCT/UNION ALL CTE ending in LIMIT 1000`
- **Schema:** requests(sequence, session_id, kind, state, time, record JSON TEXT, ...); non-STRICT.
- **Adaptation:** The cached count is an application counter; sessions() needs F3c/F3d/F5/F8/F8s/F9 and JSON functions.
- **Sources:**
  - <https://github.com/asieke/garcon/issues/38>
  - <https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/store.go#L99-L139>
  - <https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/internal/store/sessions.go#L17-L57>
  - <https://github.com/asieke/garcon/blob/5a0ec29c4b521531deea704c9cf5cfb13253a03e/go.mod>

### openvibely: OpenVibely skills analytics

- **Fit:** not_ivm_shaped, with high confidence. The issue and its acceptance criteria ask for index-ordered plans without temp B-trees at 50k rows, not maintained aggregates.
- **Evidence:** reporter benchmark ~102 ms/op at 50k rows.
- **Revision:** fd9b3f08274e9b7dc1fa43f9801a04c69007aae9 (current main; files moved under internal/persistence/sqlite).
- **Runtime:** modernc.org/sqlite v1.60.1. Loadable: no.
- **Query:** `SELECT date(e.created_at, 'localtime') AS period, SUM(CASE WHEN event_type = 'selected' THEN 1 ELSE 0 END), ... (loaded/viewed/created/edited), COUNT(*) FROM skill_analytics_events e WHERE <project_id, date range, ...> GROUP BY period ORDER BY period`
- **Schema:** skill_analytics_events(id TEXT PK, created_at DATETIME, project_id TEXT, ..., skill_scope TEXT, skill_handle TEXT, event_type TEXT CHECK(...), source TEXT, surface TEXT); non-STRICT.
- **Adapted query:** `SELECT project_id, local_day, event_type, COUNT(*) FROM events_proj GROUP BY 1, 2, 3`
- **Adaptation:** Pivot event_type; a stored local_day requires an explicit timezone policy (localtime is non-deterministic: F15 non-deterministic bucket).
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently).
- **Sources:**
  - <https://github.com/openvibely/openvibely/issues/345>
  - <https://github.com/openvibely/openvibely/blob/fd9b3f08274e9b7dc1fa43f9801a04c69007aae9/internal/persistence/sqlite/skill_analytics_repo.go#L63-L76>
  - <https://github.com/openvibely/openvibely/blob/fd9b3f08274e9b7dc1fa43f9801a04c69007aae9/internal/persistence/sqlite/database/migrations/093_skill_analytics_events.sql>
  - <https://github.com/openvibely/openvibely/blob/fd9b3f08274e9b7dc1fa43f9801a04c69007aae9/go.mod>

### setu: setu index facets

- **Fit:** not_ivm_shaped, with high confidence. The fix the issue names is to run a single scan or a plain SQL COUNT/GROUP BY instead of four JSON loads.
- **Evidence:** owner UAT; 4x full scans; no numbers.
- **Revision:** 8fe1dd0ef05a5e01e70cc40c94e645423cea74b7.
- **Runtime:** better-sqlite3 via drizzle; browser db-idb adapter for parity. Loadable: yes. the browser IndexedDB adapter could never load an extension (F14 for that adapter only).
- **Query:** `SELECT * FROM entry_index (loadAll) + JSON.parse + JS reduce, four times per /api/index/facets request`
- **Schema:** entry_index(key TEXT PK, row TEXT JSON); drizzle, non-STRICT.
- **Adapted query:** `SELECT category, COUNT(*) FROM entry_proj GROUP BY category; SELECT tag, COUNT(*) FROM entry_tags_proj GROUP BY tag`
- **Adaptation:** Facets need json_extract / json_each unnesting (F15: table-valued unnest); a normalized tags projection avoids it.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently).
- **Sources:**
  - <https://github.com/withsetu/setu/issues/588>
  - <https://github.com/withsetu/setu/blob/8fe1dd0ef05a5e01e70cc40c94e645423cea74b7/packages/db-sqlite/src/index-port.ts#L47-L52>
  - <https://github.com/withsetu/setu/blob/8fe1dd0ef05a5e01e70cc40c94e645423cea74b7/packages/db-sqlite/package.json>

### screenpipe: screenpipe activity summary

- **Fit:** not_ivm_shaped, with high confidence. The reporter confirmed the fix was a runaway polling loop plus correlated subqueries, which an index and write-time resolution address.
- **Evidence:** reporter log: 20-63 s per request every ~4 s.
- **Revision:** 5ce7edfcda145fa1bb074574925b290a1356bf2c (driver); query excerpt from the issue.
- **Runtime:** sqlx 0.9 + bundled libsqlite3-sys (Rust). Loadable: yes.
- **Query:** `WITH frame_fallback AS MATERIALIZED (... two correlated scalar subqueries per frame into ui_events ...), _bulk_logical_ui_events AS (...) ... WHERE timestamp BETWEEN ? AND ?`
- **Schema:** frames / ui_events / elements (~19M rows) hybrid Parquet index.
- **Adaptation:** Resolving app_name at write time would remove the correlated subqueries; that is a write-time fix, not IVM.
- **Sources:**
  - <https://github.com/screenpipe/screenpipe/issues/7364>
  - <https://github.com/screenpipe/screenpipe/blob/5ce7edfcda145fa1bb074574925b290a1356bf2c/Cargo.toml>

### pulse: Pulse protection observations

- **Fit:** not_ivm_shaped, with high confidence. The released fix was an indexed latest-row read, and the issue says the rollup query itself was fast.
- **Evidence:** reporter: 0.76 s over 70,949 rows returning 2 rows.
- **Revision:** 0728bda7acb9f4bb8de39bf1be2e5dc410f03c90 (go.mod); query described in the issue (store_posture.go L455 at v6.4.5).
- **Runtime:** modernc.org/sqlite v1.59.0. Loadable: no.
- **Query:** `SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY provider, scope ORDER BY observed_at_ms DESC) ...) WHERE rn = 1  -- latest observation per (provider, scope)`
- **Schema:** protection_provider_observations(..., provider, scope, observed_at_ms, payload); index (provider, scope, observed_at_ms DESC).
- **Adaptation:** Latest-per-key is an index seek per (provider, scope); top-1 semantics are F9.
- **Sources:**
  - <https://github.com/rcourtman/Pulse/issues/2465>
  - <https://github.com/rcourtman/Pulse/blob/0728bda7acb9f4bb8de39bf1be2e5dc410f03c90/go.mod>

### stash: Stash studio performer counts

- **Fit:** not_ivm_shaped, with high confidence. The root cause is a partial index the planner cannot use across a join; a full index or an IN rewrite fixes it.
- **Evidence:** reporter: 4.2-9 s per card x ~25 cards; 5.6M images.
- **Revision:** bdee3c83d33d802765332967f09db756e2877bcc (go.mod); query from the issue (v0.31.1).
- **Runtime:** mattn/go-sqlite3. Loadable: yes.
- **Query:** `WITH studio(root_id, item_id) AS (VALUES(?, ?)), performer_studio AS (SELECT performer_id FROM scenes JOIN performers_scenes ... UNION SELECT performer_id FROM images JOIN performers_images ... UNION SELECT ... galleries ...) SELECT COUNT(*) FROM (SELECT DISTINCT performers.id FROM performers LEFT JOIN performer_studio ON ... WHERE performer_studio.performer_id IS NOT NULL)`
- **Schema:** scenes/images/galleries + performers_* link tables; images.studio_id has a partial index (WHERE studio_id IS NOT NULL).
- **Adaptation:** F15 = UNION of three join branches; distinct performers per studio.
- **Sources:**
  - <https://github.com/stashapp/stash/issues/7222>
  - <https://github.com/stashapp/stash/blob/bdee3c83d33d802765332967f09db756e2877bcc/go.mod>

### photo_triage: photo-triage job counts

- **Fit:** not_ivm_shaped, with high confidence. An index range count plus a data_version gate met the 50 ms target, and the issue explicitly rejects running totals.
- **Evidence:** PR #60: 1M-row synthetic 493.6 ms -> 46-48 ms.
- **Revision:** 3e446dcb2129e3a7d0041eede865c9f9f68702d9 (after PR #60 merged 2026-10-16).
- **Runtime:** SQLAlchemy + Python sqlite3. Loadable: yes. depends on Python build.
- **Query:** `SELECT stage, status, COUNT(*) FROM jobs GROUP BY stage, status  (replaced by per-(stage, status) index range counts gated on PRAGMA data_version)`
- **Schema:** jobs(id INTEGER PK, item_id INTEGER, stage VARCHAR(32), priority INTEGER, status VARCHAR enum, enqueued_at DATETIME, ...); SQLAlchemy, non-STRICT.
- **Adapted query:** `SELECT stage, status, COUNT(*) FROM jobs_proj GROUP BY stage, status`
- **Adaptation:** STRICT projection.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently).
- **Sources:**
  - <https://github.com/MusikPolice/photo-triage/issues/24>
  - <https://github.com/MusikPolice/photo-triage/pull/60>
  - <https://github.com/MusikPolice/photo-triage/blob/3e446dcb2129e3a7d0041eede865c9f9f68702d9/backend/src/photo_triage/api/activity.py#L80-L93>
  - <https://github.com/MusikPolice/photo-triage/blob/3e446dcb2129e3a7d0041eede865c9f9f68702d9/backend/src/photo_triage/db/migrations/versions/0001_phase_1_tables_items_jobs_job_stats.py#L81-L115>

### subwave: Sub/Wave admin library stats

- **Fit:** not_ivm_shaped, with high confidence. The owner attributes the cost to repeated wide-row scans fixed by indexes and a change-invalidated cache.
- **Evidence:** 150,291 tracks / 600,987 facet rows; owner reproduced.
- **Revision:** d75122f007c6eb4d244228ed677d15eac270eb12.
- **Runtime:** better-sqlite3. Loadable: yes.
- **Query:** `SELECT COUNT(*) AS n FROM tracks WHERE <SQL_HAS_MOODS expression>; similar coverage/stats() counts over wide tracks rows`
- **Schema:** tracks (wide rows), track_facet_status, track_moods; non-STRICT.
- **Adaptation:** Not pursued: the owner's fix is indexes plus caching the counts until the library changes.
- **Sources:**
  - <https://github.com/perminder-klair/subwave/issues/1937>
  - <https://github.com/perminder-klair/subwave/issues/1937#issuecomment-6225655379>
  - <https://github.com/perminder-klair/subwave/blob/d75122f007c6eb4d244228ed677d15eac270eb12/controller/src/music/library-db/tracks.ts#L103>

### tautulli: Tautulli history page

- **Fit:** not_ivm_shaped, with low confidence. The reporter blames pagination and join strategy, and current nightly already aggregates before the wide joins; re-measurement is needed.
- **Evidence:** reporter ~20 s at 350k rows (2023); recent report 536k rows on v2.18.2.
- **Revision:** query from the 2023 issue; nightly 668ef00718faac90dcd18e68fa965bf38f3d9444 already aggregates before joins.
- **Runtime:** Python sqlite3. Loadable: yes.
- **Query:** `SELECT * FROM (SELECT reference_id, MAX(started), MIN(started), MAX(stopped), SUM(CASE WHEN stopped > 0 THEN stopped - started ELSE 0 END) - SUM(...), ..., MAX(view_offset/duration*100), COUNT(*), GROUP_CONCAT(session_history.id) FROM session_history LEFT OUTER JOIN users ... JOIN session_history_metadata ... JOIN session_history_media_info ... GROUP BY reference_id UNION SELECT ... strftime('%s','now') ... ) ORDER BY ... LIMIT/OFFSET`
- **Schema:** session_history + metadata + media_info + users; non-STRICT, REAL arithmetic.
- **Adaptation:** F15 = UNION with live sessions; paginated history rows rather than a summary.
- **Sources:**
  - <https://github.com/Tautulli/Tautulli/issues/1962>
  - <https://github.com/Tautulli/Tautulli/tree/668ef00718faac90dcd18e68fa965bf38f3d9444>

### unscreen: Unscreen per-app usage totals

- **Fit:** fits_with, with low confidence. No slow query has been reported, open sessions are time-dependent, and the Android runtime is unverified.
- **Evidence:** feature idea; no measured slow query; ADR-0004 says current queries suffice.
- **Revision:** 7e7885875beb7f52e31b5a4e733315ad6c343fc5.
- **Runtime:** Python sqlite3 inside a Flet Android/Windows app. Loadable: uncertain. extension loading in the Android Python build is unverified.
- **Query:** `get_app_session_totals: SUM(CASE WHEN s.duration_s IS NOT NULL THEN s.duration_s ELSE (min(now, until) - start_ts)/1000 END) FROM app_sessions s WHERE start_ts >= ? AND start_ts < ? [AND device_id = ?] [AND app_key NOT IN (...)] GROUP BY app_key, payload`
- **Schema:** app_sessions(device_id, app_key, payload, start_ts INTEGER, end_ts INTEGER NULL, duration_s REAL?); non-STRICT.
- **Adapted query:** `SELECT device_id, local_day, app_key, payload, SUM(duration_ms), COUNT(*) FROM closed_sessions_proj GROUP BY 1, 2, 3, 4`
- **Adaptation:** Closed sessions only, with stored local day and integer ms; open sessions (time-dependent) summed live by the reader; system-app exclusion and top-N by the reader.
- **Adaptation cost:** application-maintained STRICT projection (extra write per base-row change, extra storage, must be updated atomically with the base row and bootstrapped consistently); a reader merge of live open sessions.
- **Sources:**
  - <https://github.com/Sakth1/Unscreen/issues/30>
  - <https://github.com/Sakth1/Unscreen/blob/7e7885875beb7f52e31b5a4e733315ad6c343fc5/src/core/analytics/analytics_store.py#L1-L40>
  - <https://github.com/Sakth1/Unscreen/blob/7e7885875beb7f52e31b5a4e733315ad6c343fc5/src/core/storage/__init__.py#L724-L747>

### orpheus: Orpheus year-range history aggregates

- **Fit:** not_ivm_shaped, with high confidence. The measured problem was fixed with SQL and index changes, and the escalation path rules out capture triggers.
- **Evidence:** 31.6 s page already fixed by SQL/index changes.
- **Revision:** issue text only; no SQL in the issue.
- **Runtime:** Python (sqlite3). Loadable: uncertain.
- **Query:** `History/stats endpoints aggregate raw rows over covering indexes (exact SQL not in the issue)`
- **Schema:** not captured.
- **Adaptation:** F15 = owner forbids triggers or any write-path change, while ivmlite captures changes with triggers; summaries may refresh only on a maintenance tick.
- **Sources:**
  - <https://github.com/scottchronicity/orpheus/issues/127>

## Excluded (no workload)

- **zero:** Transaction/concurrency requirement (begin concurrent); no schema or query; libSQL runtime (would be F14). Source: <https://github.com/tursodatabase/libsql/issues/1631>.
- **sqlite_forum_ivm:** Generic capability request; no schema, query or data. Source: <https://sqlite.org/forum/forumpost/c7437e2f43>.

