//! Fixed-hour core totals adapted from CPA Manager Plus #647. This is only
//! the mergeable COUNT/SUM portion of the compact/full summary, not P95 or
//! task buckets. The source already has its own hourly aggregate fast path.

use super::DemandCase;
use rusqlite::{params, Connection, Statement};

const EVENTS_DDL: &str = "CREATE TABLE usage_events(
    id INTEGER PRIMARY KEY,
    hour_bucket INTEGER NOT NULL,
    model TEXT NOT NULL,
    failed INTEGER NOT NULL CHECK(failed IN (0, 1)),
    success_count INTEGER NOT NULL CHECK(success_count = CASE WHEN failed=0 THEN 1 ELSE 0 END),
    failure_count INTEGER NOT NULL CHECK(failure_count = CASE WHEN failed=1 THEN 1 ELSE 0 END),
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL CHECK(total_tokens = input_tokens + output_tokens)
) STRICT";

const INSERT: &str = "INSERT INTO usage_events
    (id, hour_bucket, model, failed, success_count, failure_count,
     input_tokens, output_tokens, total_tokens)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

pub static CPA_HOURLY_CORE: DemandCase = DemandCase {
    name: "cpa_hourly_core",
    ddl: EVENTS_DDL,
    view_name: "cpa_hourly_core_totals",
    view_sql: "SELECT hour_bucket, model, COUNT(*), SUM(success_count),
                      SUM(failure_count), SUM(input_tokens), SUM(output_tokens),
                      SUM(total_tokens)
               FROM usage_events GROUP BY hour_bucket, model",
    covering_index_sql: "CREATE INDEX usage_events_covering ON usage_events(
        hour_bucket, model, success_count, failure_count,
        input_tokens, output_tokens, total_tokens)",
    indexed_query: Some(
        "SELECT hour_bucket, model, COUNT(*), SUM(success_count),
                SUM(failure_count), SUM(input_tokens), SUM(output_tokens),
                SUM(total_tokens)
         FROM usage_events INDEXED BY usage_events_covering
         GROUP BY hour_bucket, model",
    ),
    default_rows: 6_100_000,
    default_batch: 200,
    seed_fn: seed,
    validate_fn: validate,
    batch_sql: &[INSERT],
    apply_fn: apply,
    handwritten: None,
};

fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows == 0 || batch == 0 || rounds == 0 {
        return Err("rows, batch, and rounds must be positive".into());
    }
    let arrivals = batch.checked_mul(rounds).ok_or("batch range overflows")?;
    if rows < arrivals {
        return Err("base rows must cover a full run of append batches".into());
    }
    let end = rows
        .checked_add(arrivals)
        .ok_or("event id range overflows")?;
    if end > i64::MAX as usize {
        return Err("event ids must fit SQLite INTEGER".into());
    }
    Ok(())
}

fn event(id: usize) -> (i64, String, i64, i64, i64, i64, i64, i64) {
    let failed = i64::from(id.is_multiple_of(9));
    let input = 100 + (id % 251) as i64;
    let output = 50 + (id % 113) as i64;
    (
        (id % 24) as i64,
        format!("model-{:02}", id % 17),
        failed,
        1 - failed,
        failed,
        input,
        output,
        input + output,
    )
}

fn seed(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    {
        let mut insert = transaction.prepare(INSERT)?;
        for id in 0..rows {
            let (hour, model, failed, success, failure, input, output, total) = event(id);
            insert.execute(params![
                id as i64, hour, model, failed, success, failure, input, output, total
            ])?;
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
        let (hour, model, failed, success, failure, input, output, total) = event(id);
        statements[0].execute(params![
            id as i64, hour, model, failed, success, failure, input, output, total
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_bench::{check_same_multiset, Workload};

    const SOURCE_CORE: &str = "SELECT hour_bucket, model, COUNT(*),
        SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
        SUM(CASE WHEN failed=1 THEN 1 ELSE 0 END),
        SUM(input_tokens), SUM(output_tokens), SUM(total_tokens)
        FROM usage_events GROUP BY hour_bucket, model";

    #[test]
    fn stored_status_projection_matches_source_conditions_after_appends() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(EVENTS_DDL).unwrap();
        seed(&connection, 1_200).unwrap();
        for round in 0..=3 {
            check_same_multiset(&connection, CPA_HOURLY_CORE.view_sql, SOURCE_CORE).unwrap();
            if round < 3 {
                CPA_HOURLY_CORE
                    .apply_mixed_batch(&connection, 1_200, 40, round)
                    .unwrap();
            }
        }
        assert!(connection
            .execute(
                "INSERT INTO usage_events VALUES (9999, 1, 'bad', 1, 1, 0, 1, 2, 3)",
                []
            )
            .is_err());
    }
}
