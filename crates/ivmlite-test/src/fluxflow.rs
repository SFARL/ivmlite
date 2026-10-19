//! Deterministic, FluxFlow-shaped fixture shared by its correctness demo and
//! benchmark. The adaptation and its source evidence are documented in
//! `docs/demos/fluxflow.md`.

use rusqlite::{params, Connection, Statement};

use crate::demo_bench::{ApplyFn, Handwritten, Workload};

pub const FLOW_TABLE_DDL: &str = "CREATE TABLE flow_facts(
    id INTEGER PRIMARY KEY,
    flow_type TEXT NOT NULL,
    day_bucket INTEGER NOT NULL,
    counterparty_kind TEXT NOT NULL,
    exchange_key TEXT NOT NULL,
    amount REAL NOT NULL,
    confidence REAL
)";

pub const VIEW_SQL: &str = "SELECT flow_type, day_bucket, counterparty_kind, exchange_key, \
    SUM(amount) AS amount, COUNT(*) AS count FROM flow_facts \
    GROUP BY flow_type, day_bucket, counterparty_kind, exchange_key";

pub const ROLLUP_TABLE: &str = "fluxflow_rollup";

const EXCHANGES: [&str; 7] = [
    "Kucoin", "Coinex", "GateIO", "NonKYC", "Binance", "MEXC", "HTX",
];
const KINDS: [&str; 3] = ["unknown", "node_operator", "foundation"];
const FLOWS_PER_DAY: i64 = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub id: i64,
    pub flow_type: &'static str,
    pub day_bucket: i64,
    pub counterparty_kind: &'static str,
    pub exchange_key: &'static str,
    pub amount: f64,
    pub confidence: f64,
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
/// 45% buying, 44% selling and 11% p2p, seven exchanges and three kinds, with
/// 10,000 flows per day bucket (150 buckets at 1.5M rows). A p2p flow has no
/// exchange; the empty `exchange_key` stands in for upstream's NULL, because
/// the grouping key is `NOT NULL` here (`docs/demos/fluxflow.md`).
/// Amounts are dyadic fractions below 2^24. Even the source-scale run's total
/// in units of 1/256 stays below 2^53, so every group's sum is exact in any
/// order and the demo can retain its exact SQLite multiset oracle.
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
        amount: ((hash >> 24) % (1_u64 << 32)) as f64 / 256.0,
        confidence: (hash % 1000) as f64 / 1000.0,
    }
}

fn insert_flow(statement: &mut Statement<'_>, flow: &Flow) -> rusqlite::Result<()> {
    statement.execute(params![
        flow.id,
        flow.flow_type,
        flow.day_bucket,
        flow.counterparty_kind,
        flow.exchange_key,
        flow.amount,
        flow.confidence,
    ])?;
    Ok(())
}

pub fn seed(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare(INSERT_SQL)?;
        for id in 0..rows {
            insert_flow(&mut insert, &generated_flow(id))?;
        }
    }
    tx.commit()
}

const INSERT_SQL: &str = "INSERT INTO flow_facts VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// The mixed batch's statements, indexed by `INSERT`, `UPDATE` and `DELETE`.
const BATCH_SQL: &[&str] = &[
    INSERT_SQL,
    "UPDATE flow_facts SET flow_type = ?2, day_bucket = ?3, \
     counterparty_kind = ?4, exchange_key = ?5, amount = ?6 WHERE id = ?1",
    "DELETE FROM flow_facts WHERE id = ?1",
];
const INSERT: usize = 0;
const UPDATE: usize = 1;
const DELETE: usize = 2;

/// `Ok` when `rounds` batches fit: each round updates or deletes its own
/// newest base rows, so the rounds together need `rounds * batch` of them.
pub fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows < rounds * batch {
        return Err(format!(
            "fluxflow: {rounds} batches of {batch} need at least {} base rows, not {rows}",
            rounds * batch
        ));
    }
    Ok(())
}

/// Round `round` of the deterministic mixed batch. Three fifths are new
/// flows, one fifth changes an existing flow's grouping keys and amount, and
/// one fifth removes an existing flow as a reorg would. The updates and
/// deletes hit the newest base rows, as a reorg does, so they concentrate on
/// the last day bucket and the next (`docs/demos/fluxflow.md`).
fn apply_round(
    statements: &mut [Statement<'_>],
    base_rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    assert!(
        base_rows >= (round + 1) * batch,
        "base_rows must cover every round's updates and deletes"
    );
    for offset in 0..batch {
        let k = round * batch + offset;
        match offset % 5 {
            0..=2 => insert_flow(&mut statements[INSERT], &generated_flow(base_rows + k))?,
            3 => {
                let id = base_rows - 1 - k;
                let source = generated_flow(id);
                let flow_type = match source.flow_type {
                    "buying" => "selling",
                    "selling" => "p2p",
                    _ => "buying",
                };
                let exchange_key = if flow_type == "p2p" { "" } else { "Kucoin" };
                statements[UPDATE].execute(params![
                    id as i64,
                    flow_type,
                    source.day_bucket + 1,
                    "foundation",
                    exchange_key,
                    source.amount + 1.0,
                ])?;
            }
            4 => {
                statements[DELETE].execute([(base_rows - 1 - k) as i64])?;
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

/// Prepares and applies round `round` of the mixed batch, for untimed use.
pub fn apply_mixed_batch(
    c: &Connection,
    base_rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    FLUXFLOW.apply_mixed_batch(c, base_rows, batch, round)
}

pub fn install_handwritten_rollup(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE TABLE {ROLLUP_TABLE}(
             flow_type TEXT NOT NULL,
             day_bucket INTEGER NOT NULL,
             counterparty_kind TEXT NOT NULL,
             exchange_key TEXT NOT NULL,
             amount REAL NOT NULL,
             count INTEGER NOT NULL,
             PRIMARY KEY(flow_type, day_bucket, counterparty_kind, exchange_key)
         ) WITHOUT ROWID;
         INSERT INTO {ROLLUP_TABLE}
         {VIEW_SQL};

         CREATE TRIGGER flow_facts_rollup_insert AFTER INSERT ON flow_facts BEGIN
           INSERT INTO {ROLLUP_TABLE} VALUES(
             NEW.flow_type, NEW.day_bucket, NEW.counterparty_kind, NEW.exchange_key, NEW.amount, 1
           ) ON CONFLICT(flow_type, day_bucket, counterparty_kind, exchange_key)
             DO UPDATE SET amount = {ROLLUP_TABLE}.amount + excluded.amount,
                           count = {ROLLUP_TABLE}.count + 1;
         END;

         CREATE TRIGGER flow_facts_rollup_delete AFTER DELETE ON flow_facts BEGIN
           UPDATE {ROLLUP_TABLE}
             SET amount = amount - OLD.amount, count = count - 1
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
             SET amount = amount - OLD.amount, count = count - 1
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key;
           DELETE FROM {ROLLUP_TABLE}
             WHERE flow_type = OLD.flow_type AND day_bucket = OLD.day_bucket
               AND counterparty_kind = OLD.counterparty_kind
               AND exchange_key = OLD.exchange_key AND count <= 0;
           INSERT INTO {ROLLUP_TABLE} VALUES(
             NEW.flow_type, NEW.day_bucket, NEW.counterparty_kind, NEW.exchange_key, NEW.amount, 1
           ) ON CONFLICT(flow_type, day_bucket, counterparty_kind, exchange_key)
             DO UPDATE SET amount = {ROLLUP_TABLE}.amount + excluded.amount,
                           count = {ROLLUP_TABLE}.count + 1;
         END;"
    ))
}

/// Reads the hand-written rollup in the view's column order.
const ROLLUP_READ_SQL: &str = "SELECT flow_type, day_bucket, counterparty_kind, exchange_key, \
    amount, count FROM fluxflow_rollup";

/// The FluxFlow demo as a `demo_bench` workload.
pub struct FluxFlow;

pub static FLUXFLOW: FluxFlow = FluxFlow;

impl Workload for FluxFlow {
    fn name(&self) -> &'static str {
        "fluxflow_grouped_flow_rollup"
    }
    fn ddl(&self) -> &'static str {
        FLOW_TABLE_DDL
    }
    fn view_name(&self) -> &'static str {
        "fluxflow_stats"
    }
    fn view_sql(&self) -> &'static str {
        VIEW_SQL
    }
    fn covering_index_sql(&self) -> &'static str {
        "CREATE INDEX flow_facts_covering ON flow_facts(\
         flow_type, day_bucket, counterparty_kind, exchange_key, amount)"
    }
    fn handwritten(&self) -> Option<Handwritten> {
        Some(Handwritten {
            install: install_handwritten_rollup,
            read_sql: ROLLUP_READ_SQL,
        })
    }
    fn validate(&self, rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
        validate(rows, batch, rounds)
    }
    fn seed(&self, c: &Connection, rows: usize) -> rusqlite::Result<()> {
        seed(c, rows)
    }
    fn batch_sql(&self) -> &'static [&'static str] {
        BATCH_SQL
    }
    fn apply_fn(&self) -> ApplyFn {
        apply_round
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_bench::check_same_multiset;

    #[test]
    fn generator_is_deterministic_and_has_all_flow_types() {
        assert_eq!(generated_flow(123), generated_flow(123));
        let sample = generated_flow(123);
        assert_eq!(sample.amount.to_bits(), 14_988_326.183_593_75_f64.to_bits());
        assert_eq!(sample.confidence.to_bits(), 0.535_f64.to_bits());
        let types: std::collections::BTreeSet<_> =
            (0..100).map(|id| generated_flow(id).flow_type).collect();
        assert_eq!(types, ["buying", "p2p", "selling"].into_iter().collect());
    }

    #[test]
    fn every_generated_amount_is_a_dyadic_rational_below_two_pow_24() {
        let mut fractional = 0;
        for id in 0..10_000 {
            let flow = generated_flow(id);
            assert!((0.0..16_777_216.0).contains(&flow.amount), "id {id}");
            assert_eq!((flow.amount * 256.0).fract().to_bits(), 0.0_f64.to_bits());
            assert!((0.0..1.0).contains(&flow.confidence), "id {id}");
            fractional += usize::from(flow.amount.fract() != 0.0);
        }
        assert!(
            fractional > 9_000,
            "the fixture must exercise fractional REALs"
        );
    }

    #[test]
    fn handwritten_rollup_tracks_both_rounds_of_mixed_changes() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(FLOW_TABLE_DDL).unwrap();
        let strict: i64 = c
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE name = 'flow_facts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(strict, 0);
        seed(&c, 100).unwrap();
        install_handwritten_rollup(&c).unwrap();
        let non_real: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM flow_facts \
                 WHERE typeof(amount) <> 'real' OR typeof(confidence) <> 'real'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(non_real, 0);
        assert_eq!(check_same_multiset(&c, ROLLUP_READ_SQL, VIEW_SQL), Ok(()));
        validate(100, 25, 2).unwrap();
        for round in 0..2 {
            apply_mixed_batch(&c, 100, 25, round).unwrap();
            let updated_id = 100 - 1 - (round * 25 + 3);
            let (amount, confidence): (f64, f64) = c
                .query_row(
                    "SELECT amount, confidence FROM flow_facts WHERE id = ?1",
                    [updated_id as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            let source = generated_flow(updated_id);
            assert_eq!(amount.to_bits(), (source.amount + 1.0).to_bits());
            assert_eq!(confidence.to_bits(), source.confidence.to_bits());
            assert_eq!(
                check_same_multiset(&c, ROLLUP_READ_SQL, VIEW_SQL),
                Ok(()),
                "round {round}"
            );
            let non_real: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM fluxflow_rollup WHERE typeof(amount) <> 'real'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(non_real, 0, "round {round}");
        }
        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM flow_facts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            rows,
            100 + 2 * 15 - 2 * 5,
            "each round inserts 15 and deletes 5"
        );
    }

    #[test]
    fn validate_requires_a_base_row_per_round_operation() {
        assert!(validate(50, 25, 2).is_ok());
        assert!(validate(49, 25, 2).is_err());
    }
}
