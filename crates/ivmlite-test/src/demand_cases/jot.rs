//! Per-session COUNT slice of StoneHub/jot #209. The source summary also
//! needs MIN(start), MAX(start + duration), a title LEFT JOIN, and ordering.

use super::DemandCase;
use rusqlite::{params, Connection, Statement};

const TRANSCRIPTS_DDL: &str = "CREATE TABLE transcripts(
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    ended_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE session_titles(session_id TEXT PRIMARY KEY, title TEXT NOT NULL) STRICT";

const INSERT: &str = "INSERT INTO transcripts
    (id, session_id, mode, started_ms, ended_ms) VALUES (?1, ?2, ?3, ?4, ?5)";

pub static JOT_SESSION_COUNTS: DemandCase = DemandCase {
    name: "jot_session_counts",
    ddl: TRANSCRIPTS_DDL,
    view_name: "jot_ambient_counts",
    view_sql: "SELECT session_id, COUNT(*) FROM transcripts
               WHERE mode='ambient' GROUP BY session_id",
    covering_index_sql: "CREATE INDEX transcripts_covering
                         ON transcripts(mode, session_id)",
    indexed_query: Some(
        "SELECT session_id, COUNT(*) FROM transcripts INDEXED BY transcripts_covering
         WHERE mode='ambient' GROUP BY session_id",
    ),
    default_rows: 100_000,
    default_batch: 200,
    seed_fn: seed,
    validate_fn: validate,
    batch_sql: &[
        INSERT,
        "DELETE FROM transcripts WHERE id=?1",
        "UPDATE transcripts SET session_id=?2 WHERE id=?1",
        "UPDATE transcripts SET mode='dictation' WHERE id=?1",
    ],
    apply_fn: apply,
    handwritten: None,
};

fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows == 0 || batch < 4 || rounds == 0 {
        return Err("need base rows, at least four batch operations, and positive rounds".into());
    }
    let touched = batch.checked_mul(rounds).ok_or("batch range overflows")?;
    if rows < touched {
        return Err("base rows must cover disjoint mixed batches".into());
    }
    let end = rows.checked_add(touched).ok_or("id range overflows")?;
    if end > i64::MAX as usize {
        return Err("transcript ids must fit SQLite INTEGER".into());
    }
    Ok(())
}

fn session(id: usize) -> String {
    format!("session-{:02}", id % 64)
}

fn mode(id: usize) -> &'static str {
    if id.is_multiple_of(5) {
        "dictation"
    } else {
        "ambient"
    }
}

fn seed(connection: &Connection, rows: usize) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    {
        let mut title = transaction.prepare("INSERT INTO session_titles VALUES (?1, ?2)")?;
        for index in 0..64 {
            title.execute(params![
                format!("session-{index:02}"),
                format!("Meeting {index}")
            ])?;
        }
        let mut insert = transaction.prepare(INSERT)?;
        for id in 0..rows {
            let start = id as i64 * 1000;
            insert.execute(params![
                id as i64,
                session(id),
                mode(id),
                start,
                start + 500
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
        let id = round * batch + offset;
        match offset % 4 {
            0 => {
                let new_id = rows + id;
                let start = new_id as i64 * 1000;
                statements[0].execute(params![
                    new_id as i64,
                    format!("session-arrival-{}", id % 8),
                    "ambient",
                    start,
                    start + 500
                ])?;
            }
            1 => {
                statements[1].execute([id as i64])?;
            }
            2 => {
                statements[2].execute(params![id as i64, format!("session-split-{}", id % 8)])?;
            }
            _ => {
                statements[3].execute([id as i64])?;
            }
        }
    }
    Ok(())
}
