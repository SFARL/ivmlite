//! Adapted append-only traffic ledger from dark-xray #159. The source uses
//! SUM(up_bytes + down_bytes) on a non-STRICT table; the demo stores that
//! expression in a checked INTEGER column in a narrow STRICT projection.

use super::DemandCase;
use crate::demo_bench::Handwritten;
use rusqlite::{params, Connection, Statement};

const LEDGER_DDL: &str = "CREATE TABLE traffic_ledger(
    event_id INTEGER PRIMARY KEY,
    owner TEXT NOT NULL,
    period INTEGER NOT NULL,
    up_bytes INTEGER NOT NULL,
    down_bytes INTEGER NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes = up_bytes + down_bytes)
) STRICT;
CREATE INDEX ledger_owner_period ON traffic_ledger(owner, period)";

const COVERING_INDEX: &str = "CREATE INDEX ledger_covering
    ON traffic_ledger(owner, period, total_bytes)";
const INSERT: &str = "INSERT INTO traffic_ledger
    (event_id, owner, period, up_bytes, down_bytes, total_bytes)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

pub static DARK_XRAY_PERIOD: DemandCase = DemandCase {
    name: "dark_xray_period_usage",
    ddl: LEDGER_DDL,
    view_name: "dark_xray_period_totals",
    view_sql: "SELECT owner, period, SUM(total_bytes) FROM traffic_ledger
               GROUP BY owner, period",
    covering_index_sql: COVERING_INDEX,
    indexed_query: Some(
        "SELECT owner, period, SUM(total_bytes)
        FROM traffic_ledger INDEXED BY ledger_covering GROUP BY owner, period",
    ),
    default_rows: 9_524_296,
    default_batch: 200,
    seed_fn: seed,
    validate_fn: validate,
    batch_sql: &[INSERT],
    apply_fn: apply,
    handwritten: Some(Handwritten {
        install: install_period,
        read_sql: "SELECT owner, period, total_bytes FROM hand_period_totals",
    }),
};

pub static DARK_XRAY_LIFETIME: DemandCase = DemandCase {
    name: "dark_xray_lifetime_usage",
    ddl: LEDGER_DDL,
    view_name: "dark_xray_lifetime_totals",
    view_sql: "SELECT owner, SUM(total_bytes) FROM traffic_ledger GROUP BY owner",
    covering_index_sql: COVERING_INDEX,
    indexed_query: Some(
        "SELECT owner, SUM(total_bytes)
        FROM traffic_ledger INDEXED BY ledger_covering GROUP BY owner",
    ),
    default_rows: 9_524_296,
    default_batch: 200,
    seed_fn: seed,
    validate_fn: validate,
    batch_sql: &[INSERT],
    apply_fn: apply,
    handwritten: Some(Handwritten {
        install: install_lifetime,
        read_sql: "SELECT owner, total_bytes FROM hand_lifetime_totals",
    }),
};

fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows == 0 || batch == 0 || rounds == 0 {
        return Err("rows, batch, and rounds must be positive".into());
    }
    let arrivals = batch.checked_mul(rounds).ok_or("batch range overflows")?;
    if rows < arrivals {
        return Err("base rows must cover at least one full run of append batches".into());
    }
    let last_id = rows
        .checked_add(arrivals)
        .ok_or("event id range overflows")?;
    if last_id > i64::MAX as usize {
        return Err("event ids must fit SQLite INTEGER".into());
    }
    Ok(())
}

fn owner(id: usize) -> String {
    // A few hot owners plus a longer tail; these are synthetic, not a trace.
    let slot = if id.is_multiple_of(2) { 0 } else { 1 + id % 63 };
    format!("owner-{slot:02}")
}

fn event(id: usize) -> (i64, i64, i64) {
    let period = 202601 + (id % 12) as i64;
    let up = 1_000 + (id % 307) as i64;
    let down = 500 + (id % 211) as i64;
    (period, up, down)
}

fn seed(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(INSERT)?;
        for id in 0..rows {
            let (period, up, down) = event(id);
            insert.execute(params![id as i64, owner(id), period, up, down, up + down])?;
        }
    }
    transaction.commit()
}

fn apply(
    statements: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    for offset in 0..batch {
        let id = rows + round * batch + offset;
        let (period, up, down) = event(id);
        statements[0].execute(params![id as i64, owner(id), period, up, down, up + down])?;
    }
    Ok(())
}

fn install_period(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE hand_period_totals(
        owner TEXT NOT NULL, period INTEGER NOT NULL, total_bytes INTEGER NOT NULL,
        PRIMARY KEY(owner, period)
    ) STRICT;
    INSERT INTO hand_period_totals
        SELECT owner, period, SUM(total_bytes) FROM traffic_ledger GROUP BY owner, period;
    CREATE TRIGGER hand_period_insert AFTER INSERT ON traffic_ledger BEGIN
        INSERT INTO hand_period_totals VALUES (NEW.owner, NEW.period, NEW.total_bytes)
        ON CONFLICT(owner, period) DO UPDATE SET
            total_bytes = total_bytes + excluded.total_bytes;
    END",
    )
}

fn install_lifetime(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE hand_lifetime_totals(
        owner TEXT PRIMARY KEY, total_bytes INTEGER NOT NULL
    ) STRICT;
    INSERT INTO hand_lifetime_totals
        SELECT owner, SUM(total_bytes) FROM traffic_ledger GROUP BY owner;
    CREATE TRIGGER hand_lifetime_insert AFTER INSERT ON traffic_ledger BEGIN
        INSERT INTO hand_lifetime_totals VALUES (NEW.owner, NEW.total_bytes)
        ON CONFLICT(owner) DO UPDATE SET
            total_bytes = total_bytes + excluded.total_bytes;
    END",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_bench::{check_same_multiset, Workload};

    #[test]
    fn projection_matches_original_expression_and_point_reads_after_every_batch() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(LEDGER_DDL).unwrap();
        seed(&connection, 1_200).unwrap();
        for round in 0..=3 {
            check_same_multiset(
                &connection,
                DARK_XRAY_PERIOD.view_sql,
                "SELECT owner, period, SUM(up_bytes + down_bytes)
                 FROM traffic_ledger GROUP BY owner, period",
            )
            .unwrap();
            check_same_multiset(
                &connection,
                DARK_XRAY_LIFETIME.view_sql,
                "SELECT owner, SUM(up_bytes + down_bytes)
                 FROM traffic_ledger GROUP BY owner",
            )
            .unwrap();
            for period in [202601, 202610] {
                let projected: i64 = connection
                    .query_row(
                        "SELECT COALESCE(SUM(total_bytes), 0) FROM traffic_ledger
                     WHERE owner='owner-00' AND period=?1",
                        [period],
                        |row| row.get(0),
                    )
                    .unwrap();
                let original: i64 = connection
                    .query_row(
                        "SELECT COALESCE(SUM(up_bytes + down_bytes), 0) FROM traffic_ledger
                     WHERE owner='owner-00' AND period=?1",
                        [period],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(projected, original);
            }
            if round < 3 {
                DARK_XRAY_PERIOD
                    .apply_mixed_batch(&connection, 1_200, 40, round)
                    .unwrap();
            }
        }
        assert!(connection
            .execute(
                "INSERT INTO traffic_ledger VALUES (9999, 'owner-00', 202601, 1, 2, 4)",
                []
            )
            .is_err());
    }
}
