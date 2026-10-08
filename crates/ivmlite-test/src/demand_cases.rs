//! Deterministic synthetic workloads adapted from public SQLite performance reports.
//!
//! The public reports establish workload shape and scale, not production data.
//! Each case exposes one query inside ivmlite's current SQL subset and a mixed
//! mutation batch that exercises more than append-only maintenance.

use rusqlite::{params, Connection, Statement};

use crate::demo_bench::{ApplyFn, Workload};

pub struct DemandCase {
    pub name: &'static str,
    pub ddl: &'static str,
    pub view_name: &'static str,
    pub view_sql: &'static str,
    /// A covering index on the grouping and aggregated columns, for the
    /// indexed recompute baseline (`docs/demos/README.md`).
    pub covering_index_sql: &'static str,
    pub default_rows: usize,
    pub default_batch: usize,
    seed_fn: fn(&Connection, usize) -> rusqlite::Result<()>,
    validate_fn: fn(usize, usize, usize) -> Result<(), String>,
    batch_sql: &'static [&'static str],
    apply_fn: ApplyFn,
}

impl Workload for DemandCase {
    fn name(&self) -> &'static str {
        self.name
    }
    fn ddl(&self) -> &'static str {
        self.ddl
    }
    fn view_name(&self) -> &'static str {
        self.view_name
    }
    fn view_sql(&self) -> &'static str {
        self.view_sql
    }
    fn covering_index_sql(&self) -> &'static str {
        self.covering_index_sql
    }
    fn validate(&self, rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
        (self.validate_fn)(rows, batch, rounds).map_err(|why| format!("{}: {why}", self.name))
    }
    fn seed(&self, c: &Connection, rows: usize) -> rusqlite::Result<()> {
        (self.seed_fn)(c, rows)
    }
    fn batch_sql(&self) -> &'static [&'static str] {
        self.batch_sql
    }
    fn apply_fn(&self) -> ApplyFn {
        self.apply_fn
    }
}

/// `Ok` when every id range a batch touches lies inside the base table and
/// no two of them overlap, so no round rewrites or deletes a row twice.
fn check_ranges(rows: usize, ranges: &[(&str, std::ops::Range<usize>)]) -> Result<(), String> {
    for (name, range) in ranges {
        if range.end > rows {
            return Err(format!(
                "{rows} base rows are too few: the {name} need ids up to {}",
                range.end - 1
            ));
        }
    }
    for (i, (left, l)) in ranges.iter().enumerate() {
        for (right, r) in &ranges[i + 1..] {
            if l.start < r.end && r.start < l.end {
                return Err(format!(
                    "{rows} base rows are too few: the {left} and the {right} would touch the same rows"
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// noop gravity witness (`docs/demos/noop.md`)
// ---------------------------------------------------------------------------

const NOOP_DAYS: usize = 60;
const SECONDS_PER_DAY: usize = 86_400;
const NOOP_SAMPLE_SECONDS: usize = 10;
/// One device's sixty days of ten-second samples. A larger seed would repeat
/// a `(device_id, ts)` pair and violate the table's `UNIQUE` constraint.
pub const NOOP_MAX_ROWS: usize = NOOP_DAYS * SECONDS_PER_DAY / NOOP_SAMPLE_SECONDS;
/// Corrections land at this second of the day, an odd offset no seed or
/// backfill timestamp reaches.
const NOOP_CORRECTION_SECOND: usize = 43_205;

pub static NOOP: DemandCase = DemandCase {
    name: "noop_gravity_witness",
    ddl: "CREATE TABLE gravity_witness(
              id INTEGER PRIMARY KEY,
              device_id INTEGER NOT NULL,
              day_bucket INTEGER NOT NULL,
              ts INTEGER NOT NULL,
              UNIQUE(device_id, ts)
          ) STRICT",
    view_name: "noop_day_counts",
    view_sql: "SELECT device_id, day_bucket, COUNT(*) FROM gravity_witness \
               GROUP BY device_id, day_bucket",
    covering_index_sql: "CREATE INDEX gravity_witness_covering \
                         ON gravity_witness(device_id, day_bucket)",
    default_rows: NOOP_MAX_ROWS,
    default_batch: 200,
    seed_fn: seed_noop,
    validate_fn: validate_noop,
    batch_sql: &[
        "INSERT INTO gravity_witness(id, device_id, day_bucket, ts) VALUES (?1, 1, ?2, ?3)",
        "DELETE FROM gravity_witness WHERE id = ?1",
        "UPDATE gravity_witness SET day_bucket = ?2, ts = ?3 WHERE id = ?1",
    ],
    apply_fn: apply_noop,
};
const NOOP_INSERT: usize = 0;
const NOOP_DELETE: usize = 1;
const NOOP_UPDATE: usize = 2;

pub static ZCASH: DemandCase = DemandCase {
    name: "zcash_transparent_balance",
    ddl: "CREATE TABLE transparent_outputs(
              id INTEGER PRIMARY KEY,
              account_uuid TEXT NOT NULL,
              value_zat INTEGER NOT NULL,
              eligible INTEGER NOT NULL
          ) STRICT",
    view_name: "zcash_account_balances",
    view_sql: "SELECT account_uuid, SUM(value_zat) FROM transparent_outputs \
               WHERE eligible = 1 GROUP BY account_uuid",
    covering_index_sql: "CREATE INDEX transparent_outputs_covering \
                         ON transparent_outputs(eligible, account_uuid, value_zat)",
    default_rows: 500_000,
    default_batch: 200,
    seed_fn: seed_zcash,
    validate_fn: validate_zcash,
    batch_sql: &[
        "INSERT INTO transparent_outputs(id, account_uuid, value_zat, eligible) \
         VALUES (?1, ?2, ?3, 1)",
        "UPDATE transparent_outputs SET eligible = 0 WHERE id = ?1",
        "UPDATE transparent_outputs SET eligible = 1 WHERE id = ?1",
        "UPDATE transparent_outputs SET account_uuid = ?2, value_zat = value_zat + 17 \
         WHERE id = ?1",
    ],
    apply_fn: apply_zcash,
};
const ZCASH_RECEIVE: usize = 0;
const ZCASH_SPEND: usize = 1;
const ZCASH_REWIND: usize = 2;
const ZCASH_CORRECT: usize = 3;

pub static KENER: DemandCase = DemandCase {
    name: "kener_quarter_hour_rollup",
    ddl: "CREATE TABLE monitoring_facts(
              id INTEGER PRIMARY KEY,
              monitor_id INTEGER NOT NULL,
              bucket_ts INTEGER NOT NULL,
              status TEXT NOT NULL,
              latency_ms INTEGER NOT NULL
          ) STRICT",
    view_name: "kener_bucket_rollup",
    view_sql: "SELECT monitor_id, bucket_ts, status, SUM(latency_ms), COUNT(*) \
               FROM monitoring_facts GROUP BY monitor_id, bucket_ts, status",
    covering_index_sql: "CREATE INDEX monitoring_facts_covering \
                         ON monitoring_facts(monitor_id, bucket_ts, status, latency_ms)",
    default_rows: 550_000,
    default_batch: 500,
    seed_fn: seed_kener,
    validate_fn: validate_kener,
    batch_sql: &[
        "INSERT INTO monitoring_facts(id, monitor_id, bucket_ts, status, latency_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        "UPDATE monitoring_facts SET status = ?2, monitor_id = (monitor_id + 1) % 215 \
         WHERE id = ?1",
        "UPDATE monitoring_facts SET latency_ms = latency_ms + 25 WHERE id = ?1",
        "DELETE FROM monitoring_facts WHERE id = ?1",
    ],
    apply_fn: apply_kener,
};
const KENER_INSERT: usize = 0;
const KENER_STATUS: usize = 1;
const KENER_LATENCY: usize = 2;
const KENER_DELETE: usize = 3;

pub const ALL: [&DemandCase; 3] = [&NOOP, &ZCASH, &KENER];

pub fn by_name(name: &str) -> Option<&'static DemandCase> {
    ALL.into_iter().find(|case| case.name == name)
}

fn seed_noop(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    assert!(rows <= NOOP_MAX_ROWS, "{}", noop_rows_message(rows));
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(NOOP.batch_sql[NOOP_INSERT])?;
        for id in 0..rows {
            let day = id % NOOP_DAYS;
            let sample_in_day = id / NOOP_DAYS;
            insert.execute(params![
                id as i64,
                day as i64,
                (day * SECONDS_PER_DAY + sample_in_day * NOOP_SAMPLE_SECONDS) as i64
            ])?;
        }
    }
    transaction.commit()
}

fn noop_rows_message(rows: usize) -> String {
    format!(
        "at most {NOOP_MAX_ROWS} base rows fit one device's sixty days of ten-second samples; \
         {rows} would repeat a (device_id, ts) pair"
    )
}

/// The batch's appends, backfills, deletes and corrections.
fn noop_mix(batch: usize) -> (usize, usize, usize, usize) {
    let appends = batch / 2;
    let backfills = batch / 5;
    let deletes = batch / 6;
    (
        appends,
        backfills,
        deletes,
        batch - appends - backfills - deletes,
    )
}

fn validate_noop(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows > NOOP_MAX_ROWS {
        return Err(noop_rows_message(rows));
    }
    let (appends, backfills, deletes, corrections) = noop_mix(batch);
    // Appends fill day 60 at ten-second steps; backfills take the odd
    // seconds below the corrections' second; corrections take the odd
    // seconds from it to the end of the day.
    let day_steps = SECONDS_PER_DAY / NOOP_SAMPLE_SECONDS;
    let fits = rounds * appends <= day_steps
        && (rounds * backfills).div_ceil(30) <= NOOP_CORRECTION_SECOND / NOOP_SAMPLE_SECONDS
        && (rounds * corrections).div_ceil(NOOP_DAYS)
            <= (SECONDS_PER_DAY - NOOP_CORRECTION_SECOND) / NOOP_SAMPLE_SECONDS;
    if !fits {
        return Err(format!(
            "a {batch}-operation batch has too many timestamps for one day"
        ));
    }
    check_ranges(
        rows,
        &[
            ("deletes", 0..rounds * deletes),
            ("corrections", rows / 2..rows / 2 + rounds * corrections),
        ],
    )
}

fn apply_noop(
    statements: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    let (appends, backfills, deletes, corrections) = noop_mix(batch);
    let first_new_id = rows + round * (appends + backfills);

    for offset in 0..appends {
        let k = round * appends + offset;
        statements[NOOP_INSERT].execute(params![
            (first_new_id + offset) as i64,
            NOOP_DAYS as i64,
            (NOOP_DAYS * SECONDS_PER_DAY + k * NOOP_SAMPLE_SECONDS) as i64
        ])?;
    }
    for offset in 0..backfills {
        let k = round * backfills + offset;
        let day = k % 30;
        let sample_in_day = k / 30;
        statements[NOOP_INSERT].execute(params![
            (first_new_id + appends + offset) as i64,
            day as i64,
            (day * SECONDS_PER_DAY + sample_in_day * NOOP_SAMPLE_SECONDS + 5) as i64
        ])?;
    }
    for offset in 0..deletes {
        statements[NOOP_DELETE].execute([(round * deletes + offset) as i64])?;
    }
    for offset in 0..corrections {
        let k = round * corrections + offset;
        let id = rows / 2 + k;
        let day = (k + 31) % NOOP_DAYS;
        let occurrence = k / NOOP_DAYS;
        statements[NOOP_UPDATE].execute(params![
            id as i64,
            day as i64,
            (day * SECONDS_PER_DAY + NOOP_CORRECTION_SECOND + occurrence * NOOP_SAMPLE_SECONDS)
                as i64
        ])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Zcash transparent balance (`docs/demos/zcash.md`)
// ---------------------------------------------------------------------------

fn zcash_account_count(rows: usize) -> usize {
    rows.clamp(8, 256)
}

fn seed_zcash(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let account_count = zcash_account_count(rows);
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO transparent_outputs(id, account_uuid, value_zat, eligible) \
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for id in 0..rows {
            insert.execute(params![
                id as i64,
                format!("account-{:03}", id % account_count),
                (10_000 + id % 1_000_000) as i64,
                i64::from(!id.is_multiple_of(10))
            ])?;
        }
    }
    transaction.commit()
}

/// The batch's receives, spends, rewinds and corrections.
fn zcash_mix(batch: usize) -> (usize, usize, usize, usize) {
    let receives = batch * 2 / 5;
    let spends = batch / 4;
    let rewinds = batch / 5;
    (
        receives,
        spends,
        rewinds,
        batch - receives - spends - rewinds,
    )
}

fn validate_zcash(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    let (_, spends, rewinds, corrections) = zcash_mix(batch);
    // Round after round, spends take ids 5k + 1, rewinds 10k (the initially
    // ineligible outputs) and corrections 10k + 3, so they never share a
    // row; the highest id each touches must exist.
    let highest = [
        (
            "spends",
            (rounds * spends).checked_sub(1).map(|k| k * 5 + 1),
        ),
        ("rewinds", (rounds * rewinds).checked_sub(1).map(|k| k * 10)),
        (
            "corrections",
            (rounds * corrections).checked_sub(1).map(|k| k * 10 + 3),
        ),
    ];
    for (name, id) in highest {
        if let Some(id) = id.filter(|&id| id >= rows) {
            return Err(format!(
                "{rows} base rows are too few: the {name} need ids up to {id}"
            ));
        }
    }
    Ok(())
}

fn apply_zcash(
    statements: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    let (receives, spends, rewinds, corrections) = zcash_mix(batch);
    let account_count = zcash_account_count(rows);

    for offset in 0..receives {
        let k = round * receives + offset;
        let id = rows + k;
        statements[ZCASH_RECEIVE].execute(params![
            id as i64,
            format!("account-{:03}", id % account_count),
            (50_000 + k) as i64
        ])?;
    }
    for offset in 0..spends {
        let k = round * spends + offset;
        statements[ZCASH_SPEND].execute([(k * 5 + 1) as i64])?;
    }
    for offset in 0..rewinds {
        let k = round * rewinds + offset;
        statements[ZCASH_REWIND].execute([(k * 10) as i64])?;
    }
    for offset in 0..corrections {
        let k = round * corrections + offset;
        let id = k * 10 + 3;
        statements[ZCASH_CORRECT].execute(params![
            id as i64,
            format!("account-{:03}", (id + 1) % account_count)
        ])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Kener quarter-hour rollup (`docs/demos/kener.md`)
// ---------------------------------------------------------------------------

fn kener_group_count(rows: usize) -> usize {
    (((rows as u128 * 550_000) / 4_100_000) as usize).max(1)
}

fn kener_status(group: usize) -> &'static str {
    match group % 4 {
        0 => "UP",
        1 => "DOWN",
        2 => "DEGRADED",
        _ => "MAINTENANCE",
    }
}

fn seed_kener(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let group_count = kener_group_count(rows);
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(KENER.batch_sql[KENER_INSERT])?;
        for id in 0..rows {
            let group = (id as u128 * group_count as u128 / rows as u128) as usize;
            insert.execute(params![
                id as i64,
                (group % 215) as i64,
                ((group / 215) * 900) as i64,
                kener_status(group),
                (20 + id % 980) as i64
            ])?;
        }
    }
    transaction.commit()
}

/// The batch's inserts, status rewrites, latency corrections and deletes.
fn kener_mix(batch: usize) -> (usize, usize, usize, usize) {
    let inserts = batch * 2 / 5;
    let status_changes = batch * 3 / 10;
    let latency_changes = batch * 3 / 20;
    (
        inserts,
        status_changes,
        latency_changes,
        batch - inserts - status_changes - latency_changes,
    )
}

fn validate_kener(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    let (_, status_changes, latency_changes, deletes) = kener_mix(batch);
    let deleted = rounds * deletes;
    if deleted > rows {
        return Err(format!(
            "{rows} base rows are too few for {deleted} deletes"
        ));
    }
    check_ranges(
        rows,
        &[
            ("status rewrites", 0..rounds * status_changes),
            (
                "latency corrections",
                rows / 2..rows / 2 + rounds * latency_changes,
            ),
            ("deletes", rows - deleted..rows),
        ],
    )
}

fn apply_kener(
    statements: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    let (inserts, status_changes, latency_changes, deletes) = kener_mix(batch);
    let group_count = kener_group_count(rows);

    for offset in 0..inserts {
        let k = round * inserts + offset;
        let group = group_count + k / 8;
        statements[KENER_INSERT].execute(params![
            (rows + k) as i64,
            (group % 215) as i64,
            ((group / 215) * 900) as i64,
            kener_status(group),
            (30 + k % 700) as i64
        ])?;
    }
    for offset in 0..status_changes {
        let k = round * status_changes + offset;
        statements[KENER_STATUS].execute(params![k as i64, kener_status(k + 1)])?;
    }
    for offset in 0..latency_changes {
        let k = round * latency_changes + offset;
        statements[KENER_LATENCY].execute([(rows / 2 + k) as i64])?;
    }
    for offset in 0..deletes {
        let k = round * deletes + offset;
        statements[KENER_DELETE].execute([(rows - 1 - k) as i64])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_rows(connection: &Connection, sql: &str) -> Vec<Vec<String>> {
        let mut statement = connection.prepare(sql).unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| Ok(format!("{:?}", row.get_ref(column)?)))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn case_names_are_unique_and_resolvable() {
        let names: std::collections::BTreeSet<_> = ALL.iter().map(|case| case.name).collect();
        assert_eq!(names.len(), ALL.len());
        for case in ALL {
            assert!(std::ptr::eq(by_name(case.name).unwrap(), case));
        }
    }

    #[test]
    fn generators_match_published_scale_shapes() {
        assert_eq!(kener_group_count(4_100_000), 550_000);

        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(ZCASH.ddl).unwrap();
        ZCASH.seed(&connection, 100).unwrap();
        let eligible: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM transparent_outputs WHERE eligible = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(eligible, 90);
    }

    #[test]
    fn both_rounds_of_each_mixed_batch_apply_and_change_the_result() {
        for case in ALL {
            case.validate(600, 40, 2).unwrap();
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(case.ddl).unwrap();
            case.seed(&connection, 600).unwrap();
            let mut before = query_rows(&connection, case.view_sql);
            for round in 0..2 {
                case.apply_mixed_batch(&connection, 600, 40, round).unwrap();
                let after = query_rows(&connection, case.view_sql);
                assert_ne!(before, after, "{} round {round}", case.name);
                before = after;
            }
        }
    }

    #[test]
    fn source_scale_defaults_validate() {
        for case in ALL {
            case.validate(case.default_rows, case.default_batch, 2)
                .unwrap();
        }
        // The documented runs (`docs/demos/*.md`).
        NOOP.validate(518_400, 200, 2).unwrap();
        ZCASH.validate(500_000, 500, 2).unwrap();
        KENER.validate(4_100_000, 500, 2).unwrap();
    }

    #[test]
    fn noop_refuses_more_rows_than_one_device_has_samples() {
        assert_eq!(NOOP_MAX_ROWS, 518_400);
        let error = NOOP.validate(NOOP_MAX_ROWS + 1, 200, 2).unwrap_err();
        assert!(error.contains("518400"), "{error}");
    }

    #[test]
    fn validation_refuses_batches_that_overrun_the_table() {
        for case in ALL {
            assert!(case.validate(20, 40, 2).is_err(), "{}", case.name);
        }
    }
}
