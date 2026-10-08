//! Deterministic synthetic workloads adapted from public SQLite performance reports.
//!
//! The public reports establish workload shape and scale, not production data.
//! Each case exposes one query inside ivmlite's current SQL subset and a mixed
//! mutation batch that exercises more than append-only maintenance.

use rusqlite::{params, Connection};

pub struct DemandCase {
    pub name: &'static str,
    pub ddl: &'static str,
    pub view_name: &'static str,
    pub view_sql: &'static str,
    pub default_rows: usize,
    pub default_batch: usize,
    seed_fn: fn(&Connection, usize) -> rusqlite::Result<()>,
    apply_fn: fn(&Connection, usize, usize) -> rusqlite::Result<()>,
}

impl DemandCase {
    pub fn seed(&self, connection: &Connection, rows: usize) -> rusqlite::Result<()> {
        (self.seed_fn)(connection, rows)
    }

    pub fn apply_mixed_batch(
        &self,
        connection: &Connection,
        rows: usize,
        batch: usize,
    ) -> rusqlite::Result<()> {
        (self.apply_fn)(connection, rows, batch)
    }
}

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
    default_rows: 518_400,
    default_batch: 200,
    seed_fn: seed_noop,
    apply_fn: apply_noop,
};

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
    default_rows: 500_000,
    default_batch: 200,
    seed_fn: seed_zcash,
    apply_fn: apply_zcash,
};

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
    default_rows: 550_000,
    default_batch: 500,
    seed_fn: seed_kener,
    apply_fn: apply_kener,
};

pub const ALL: [&DemandCase; 3] = [&NOOP, &ZCASH, &KENER];

pub fn by_name(name: &str) -> Option<&'static DemandCase> {
    ALL.into_iter().find(|case| case.name == name)
}

fn seed_noop(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO gravity_witness(id, device_id, day_bucket, ts) VALUES (?1, 1, ?2, ?3)",
        )?;
        for id in 0..rows {
            let day = id % 60;
            let sample_in_day = id / 60;
            insert.execute(params![
                id as i64,
                day as i64,
                (day * 86_400 + sample_in_day * 10) as i64
            ])?;
        }
    }
    transaction.commit()
}

fn apply_noop(connection: &Connection, rows: usize, batch: usize) -> rusqlite::Result<()> {
    let appends = batch / 2;
    let backfills = batch / 5;
    let deletes = batch / 6;
    let corrections = batch - appends - backfills - deletes;
    let transaction = connection.unchecked_transaction()?;

    {
        let mut insert = transaction.prepare(
            "INSERT INTO gravity_witness(id, device_id, day_bucket, ts) VALUES (?1, 1, ?2, ?3)",
        )?;
        for offset in 0..appends {
            let id = rows + offset;
            insert.execute(params![
                id as i64,
                60_i64,
                (60 * 86_400 + offset * 10) as i64
            ])?;
        }
        for offset in 0..backfills {
            let id = rows + appends + offset;
            let day = offset % 30;
            let sample_in_day = offset / 30;
            insert.execute(params![
                id as i64,
                day as i64,
                (day * 86_400 + sample_in_day * 10 + 5) as i64
            ])?;
        }
    }
    {
        let mut delete = transaction.prepare("DELETE FROM gravity_witness WHERE id = ?1")?;
        for offset in 0..deletes {
            delete.execute([offset as i64])?;
        }
    }
    {
        let mut update = transaction
            .prepare("UPDATE gravity_witness SET day_bucket = ?2, ts = ?3 WHERE id = ?1")?;
        for offset in 0..corrections {
            let id = rows / 2 + offset;
            let day = (offset + 31) % 60;
            let occurrence = offset / 60;
            update.execute(params![
                id as i64,
                day as i64,
                (day * 86_400 + 43_205 + occurrence * 10) as i64
            ])?;
        }
    }
    transaction.commit()
}

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

fn apply_zcash(connection: &Connection, rows: usize, batch: usize) -> rusqlite::Result<()> {
    let receives = batch * 2 / 5;
    let spends = batch / 4;
    let rewinds = batch / 5;
    let corrections = batch - receives - spends - rewinds;
    let account_count = zcash_account_count(rows);
    let transaction = connection.unchecked_transaction()?;

    {
        let mut insert = transaction.prepare(
            "INSERT INTO transparent_outputs(id, account_uuid, value_zat, eligible) \
             VALUES (?1, ?2, ?3, 1)",
        )?;
        for offset in 0..receives {
            let id = rows + offset;
            insert.execute(params![
                id as i64,
                format!("account-{:03}", id % account_count),
                (50_000 + offset) as i64
            ])?;
        }
    }
    {
        let mut spend =
            transaction.prepare("UPDATE transparent_outputs SET eligible = 0 WHERE id = ?1")?;
        for offset in 0..spends {
            spend.execute([(offset * 5 + 1) as i64])?;
        }
    }
    {
        let mut rewind =
            transaction.prepare("UPDATE transparent_outputs SET eligible = 1 WHERE id = ?1")?;
        for offset in 0..rewinds {
            rewind.execute([(offset * 10) as i64])?;
        }
    }
    {
        let mut correct = transaction.prepare(
            "UPDATE transparent_outputs SET account_uuid = ?2, value_zat = value_zat + 17 \
             WHERE id = ?1",
        )?;
        for offset in 0..corrections {
            let id = (offset * 10 + 3) % rows;
            correct.execute(params![
                id as i64,
                format!("account-{:03}", (id + 1) % account_count)
            ])?;
        }
    }
    transaction.commit()
}

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
        let mut insert = transaction.prepare(
            "INSERT INTO monitoring_facts(id, monitor_id, bucket_ts, status, latency_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
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

fn apply_kener(connection: &Connection, rows: usize, batch: usize) -> rusqlite::Result<()> {
    let inserts = batch * 2 / 5;
    let status_changes = batch * 3 / 10;
    let latency_changes = batch * 3 / 20;
    let deletes = batch - inserts - status_changes - latency_changes;
    let group_count = kener_group_count(rows);
    let transaction = connection.unchecked_transaction()?;

    {
        let mut insert = transaction.prepare(
            "INSERT INTO monitoring_facts(id, monitor_id, bucket_ts, status, latency_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for offset in 0..inserts {
            let id = rows + offset;
            let group = group_count + offset / 8;
            insert.execute(params![
                id as i64,
                (group % 215) as i64,
                ((group / 215) * 900) as i64,
                kener_status(group),
                (30 + offset % 700) as i64
            ])?;
        }
    }
    {
        let mut update = transaction.prepare(
            "UPDATE monitoring_facts SET status = ?2, monitor_id = (monitor_id + 1) % 215 \
             WHERE id = ?1",
        )?;
        for offset in 0..status_changes {
            update.execute(params![offset as i64, kener_status(offset + 1)])?;
        }
    }
    {
        let mut update = transaction
            .prepare("UPDATE monitoring_facts SET latency_ms = latency_ms + 25 WHERE id = ?1")?;
        for offset in 0..latency_changes {
            update.execute([(rows / 2 + offset) as i64])?;
        }
    }
    {
        let mut delete = transaction.prepare("DELETE FROM monitoring_facts WHERE id = ?1")?;
        for offset in 0..deletes {
            delete.execute([(rows - 1 - offset) as i64])?;
        }
    }
    transaction.commit()
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
    fn mixed_batches_change_each_oracle_result() {
        for case in ALL {
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(case.ddl).unwrap();
            case.seed(&connection, 600).unwrap();
            let before = query_rows(&connection, case.view_sql);
            case.apply_mixed_batch(&connection, 600, 40).unwrap();
            let after = query_rows(&connection, case.view_sql);
            assert_ne!(before, after, "{}", case.name);
        }
    }
}
