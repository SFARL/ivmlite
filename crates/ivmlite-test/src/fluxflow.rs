//! Deterministic, FluxFlow-shaped fixture shared by its correctness demo and
//! benchmark. The adaptation and its source evidence are documented in
//! `docs/demos/fluxflow.md`.

use rusqlite::{params, Connection};

pub const FLOW_TABLE_DDL: &str = "CREATE TABLE flow_facts(
    id INTEGER PRIMARY KEY,
    flow_type TEXT NOT NULL,
    day_bucket INTEGER NOT NULL,
    counterparty_kind TEXT NOT NULL,
    exchange_key TEXT NOT NULL,
    sat INTEGER NOT NULL
) STRICT";

pub const VIEW_SQL: &str = "SELECT flow_type, day_bucket, counterparty_kind, exchange_key, \
    SUM(sat) AS sat, COUNT(*) AS count FROM flow_facts \
    GROUP BY flow_type, day_bucket, counterparty_kind, exchange_key";

pub const ROLLUP_TABLE: &str = "fluxflow_rollup";

const EXCHANGES: [&str; 7] = [
    "Kucoin", "Coinex", "GateIO", "NonKYC", "Binance", "MEXC", "HTX",
];
const KINDS: [&str; 3] = ["unknown", "node_operator", "foundation"];
const FLOWS_PER_DAY: i64 = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Flow {
    pub id: i64,
    pub flow_type: &'static str,
    pub day_bucket: i64,
    pub counterparty_kind: &'static str,
    pub exchange_key: &'static str,
    pub sat: i64,
}

/// A stable integer mixer keeps benchmark databases identical between modes
/// and runs without depending on a particular `rand` release.
fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Preserve the upstream generator's important categorical shape: roughly
/// 45% buying, 44% selling and 11% p2p, seven exchanges and three kinds over
/// about 180 day buckets at the reported 1.5--1.7M-row scale.
pub fn generated_flow(id: usize) -> Flow {
    let hash = mix(id as u64 + 42);
    let percentile = hash % 100;
    let flow_type = if percentile < 45 {
        "buying"
    } else if percentile < 89 {
        "selling"
    } else {
        "p2p"
    };
    Flow {
        id: id as i64,
        flow_type,
        day_bucket: id as i64 / FLOWS_PER_DAY,
        counterparty_kind: KINDS[((hash >> 8) as usize) % KINDS.len()],
        exchange_key: if flow_type == "p2p" {
            ""
        } else {
            EXCHANGES[((hash >> 16) as usize) % EXCHANGES.len()]
        },
        sat: ((hash >> 24) % 500_000_000_000) as i64,
    }
}

fn insert_flow(statement: &mut rusqlite::Statement<'_>, flow: &Flow) -> rusqlite::Result<()> {
    statement.execute(params![
        flow.id,
        flow.flow_type,
        flow.day_bucket,
        flow.counterparty_kind,
        flow.exchange_key,
        flow.sat,
    ])?;
    Ok(())
}

pub fn seed(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO flow_facts VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for id in 0..rows {
            insert_flow(&mut insert, &generated_flow(id))?;
        }
    }
    tx.commit()
}

/// Apply one deterministic mixed batch. Three fifths are new flows, one fifth
/// changes an existing flow's grouping keys and amount, and one fifth removes
/// an existing flow as a reorg would.
pub fn apply_mixed_batch(c: &Connection, base_rows: usize, batch: usize) -> rusqlite::Result<()> {
    assert!(base_rows >= batch, "base_rows must be at least batch size");
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO flow_facts VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        let mut update = tx.prepare(
            "UPDATE flow_facts SET flow_type = ?2, day_bucket = ?3, \
             counterparty_kind = ?4, exchange_key = ?5, sat = ?6 WHERE id = ?1",
        )?;
        let mut delete = tx.prepare("DELETE FROM flow_facts WHERE id = ?1")?;

        for offset in 0..batch {
            match offset % 5 {
                0..=2 => insert_flow(&mut insert, &generated_flow(base_rows + offset))?,
                3 => {
                    let id = offset;
                    let source = generated_flow(id);
                    let flow_type = match source.flow_type {
                        "buying" => "selling",
                        "selling" => "p2p",
                        _ => "buying",
                    };
                    let exchange_key = if flow_type == "p2p" { "" } else { "Kucoin" };
                    update.execute(params![
                        id as i64,
                        flow_type,
                        source.day_bucket + 1,
                        "foundation",
                        exchange_key,
                        source.sat + 7,
                    ])?;
                }
                4 => {
                    delete.execute([offset as i64])?;
                }
                _ => unreachable!(),
            }
        }
    }
    tx.commit()
}

pub fn install_handwritten_rollup(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE TABLE {ROLLUP_TABLE}(
             flow_type TEXT NOT NULL,
             day_bucket INTEGER NOT NULL,
             counterparty_kind TEXT NOT NULL,
             exchange_key TEXT NOT NULL,
             sat INTEGER NOT NULL,
             count INTEGER NOT NULL,
             PRIMARY KEY(flow_type, day_bucket, counterparty_kind, exchange_key)
         ) WITHOUT ROWID;
         INSERT INTO {ROLLUP_TABLE}
         {VIEW_SQL};

         CREATE TRIGGER flow_facts_rollup_insert AFTER INSERT ON flow_facts BEGIN
           INSERT INTO {ROLLUP_TABLE} VALUES(
             NEW.flow_type, NEW.day_bucket, NEW.counterparty_kind, NEW.exchange_key, NEW.sat, 1
           ) ON CONFLICT(flow_type, day_bucket, counterparty_kind, exchange_key)
             DO UPDATE SET sat = {ROLLUP_TABLE}.sat + excluded.sat,
                           count = {ROLLUP_TABLE}.count + 1;
         END;

         CREATE TRIGGER flow_facts_rollup_delete AFTER DELETE ON flow_facts BEGIN
           UPDATE {ROLLUP_TABLE}
             SET sat = sat - OLD.sat, count = count - 1
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key;
           DELETE FROM {ROLLUP_TABLE}
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key AND count <= 0;
         END;

         CREATE TRIGGER flow_facts_rollup_update AFTER UPDATE ON flow_facts BEGIN
           UPDATE {ROLLUP_TABLE}
             SET sat = sat - OLD.sat, count = count - 1
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key;
           DELETE FROM {ROLLUP_TABLE}
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key AND count <= 0;
           INSERT INTO {ROLLUP_TABLE} VALUES(
             NEW.flow_type, NEW.day_bucket, NEW.counterparty_kind, NEW.exchange_key, NEW.sat, 1
           ) ON CONFLICT(flow_type, day_bucket, counterparty_kind, exchange_key)
             DO UPDATE SET sat = {ROLLUP_TABLE}.sat + excluded.sat,
                           count = {ROLLUP_TABLE}.count + 1;
         END;"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic_and_has_all_flow_types() {
        assert_eq!(generated_flow(123), generated_flow(123));
        let types: std::collections::BTreeSet<_> =
            (0..100).map(|id| generated_flow(id).flow_type).collect();
        assert_eq!(types, ["buying", "p2p", "selling"].into_iter().collect());
    }

    #[test]
    fn handwritten_rollup_tracks_mixed_changes() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(FLOW_TABLE_DDL).unwrap();
        seed(&c, 100).unwrap();
        install_handwritten_rollup(&c).unwrap();
        apply_mixed_batch(&c, 100, 25).unwrap();

        let oracle: Vec<(String, i64, String, String, i64, i64)> = c
            .prepare(&format!("{VIEW_SQL} ORDER BY 1, 2, 3, 4"))
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let actual: Vec<(String, i64, String, String, i64, i64)> = c
            .prepare(&format!(
                "SELECT flow_type, day_bucket, counterparty_kind, exchange_key, sat, count \
                 FROM {ROLLUP_TABLE} ORDER BY 1, 2, 3, 4"
            ))
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(actual, oracle);
    }
}
