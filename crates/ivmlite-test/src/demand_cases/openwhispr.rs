//! Synthetic by-speaker transcript rollup adapted from OpenWhispr #84.
//! The source query computes words from TEXT; this case stores the exact
//! SQLite expression's result on each segment before it reaches ivmlite.

use super::DemandCase;
use rusqlite::{params, Connection, Statement};

const SEGMENTS_PER_NOTE: usize = 100;
const BATCH_KINDS: usize = 6;
const TEXTS: [&str; 5] = ["", "  ", "hello", "two words", "  three  words  here "];

/// The upstream expression uses SQLite's default `trim`, which removes ASCII
/// spaces, then counts every remaining ASCII space (including repeats).
fn source_word_count(value: &str) -> i64 {
    let trimmed = value.trim_matches(' ');
    if trimmed.is_empty() {
        0
    } else {
        (trimmed.bytes().filter(|&byte| byte == b' ').count() + 1) as i64
    }
}

pub static OPENWHISPR: DemandCase = DemandCase {
    name: "openwhispr_speaker_words",
    ddl: "CREATE TABLE notes(
              id INTEGER PRIMARY KEY, deleted_at TEXT
          ) STRICT;
          CREATE TABLE transcript_segments(
              id INTEGER PRIMARY KEY, note_id INTEGER NOT NULL,
              speaker_name TEXT, text TEXT NOT NULL, word_count INTEGER NOT NULL,
              FOREIGN KEY (note_id) REFERENCES notes(id)
          ) STRICT;
          CREATE INDEX transcript_segments_note ON transcript_segments(note_id)",
    view_name: "openwhispr_speakers",
    // The original excludes NULL speaker names. One predicate is available
    // inside the maintained query, so the reader excludes the NULL group.
    view_sql: "SELECT s.speaker_name, COUNT(*), SUM(s.word_count)
               FROM transcript_segments s JOIN notes n ON n.id=s.note_id
               WHERE n.deleted_at IS NULL GROUP BY s.speaker_name",
    covering_index_sql: "CREATE INDEX notes_covering ON notes(id, deleted_at);
                         CREATE INDEX transcript_segments_covering
                         ON transcript_segments(note_id, speaker_name, word_count)",
    indexed_query: Some(
        "SELECT s.speaker_name, COUNT(*), SUM(s.word_count)
         FROM transcript_segments s INDEXED BY transcript_segments_covering
         JOIN notes n INDEXED BY notes_covering ON n.id=s.note_id
         WHERE n.deleted_at IS NULL GROUP BY s.speaker_name",
    ),
    default_rows: 2_000_000,
    default_batch: 200,
    seed_fn: seed,
    validate_fn: validate,
    batch_sql: &[
        "INSERT INTO transcript_segments VALUES (?1, ?2, ?3, ?4, ?5)",
        "DELETE FROM transcript_segments WHERE id=?1",
        "UPDATE transcript_segments SET text=?2, word_count=?3 WHERE id=?1",
        "UPDATE transcript_segments SET speaker_name=?2 WHERE id=?1",
        "UPDATE notes SET deleted_at='2026-10-16' WHERE id=?1",
        "UPDATE notes SET deleted_at=NULL WHERE id=?1",
    ],
    apply_fn: apply,
    handwritten: None,
};

fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if rows < 4 * SEGMENTS_PER_NOTE || batch < BATCH_KINDS || rounds == 0 {
        return Err("need at least 400 segments, a batch of six, and positive rounds".into());
    }
    let touched = batch.checked_mul(rounds).ok_or("batch range overflows")?;
    let last_id = rows.checked_add(touched).ok_or("insert range overflows")?;
    if touched > rows || last_id > i64::MAX as usize {
        return Err("base rows must cover all mixed batches and ids must fit INTEGER".into());
    }
    Ok(())
}

fn note_count(rows: usize) -> usize {
    rows.div_ceil(SEGMENTS_PER_NOTE)
}

fn speaker(id: usize) -> Option<String> {
    (!id.is_multiple_of(13)).then(|| format!("speaker-{}", id % 64))
}

fn seed(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert_note = tx.prepare("INSERT INTO notes VALUES (?1, ?2)")?;
        for id in 1..=note_count(rows) {
            let deleted = (id % 2 == 0).then_some("2026-10-01");
            insert_note.execute(params![id as i64, deleted])?;
        }
        let mut insert_segment = tx.prepare(OPENWHISPR.batch_sql[0])?;
        for id in 0..rows {
            let value = TEXTS[id % TEXTS.len()];
            insert_segment.execute(params![
                id as i64,
                (id / SEGMENTS_PER_NOTE + 1) as i64,
                speaker(id),
                value,
                source_word_count(value)
            ])?;
        }
    }
    tx.commit()
}

fn apply(
    statements: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    let note_pairs = note_count(rows) / 2;
    let mutable_active_notes = note_pairs - 1;
    for i in 0..batch {
        let id = round * batch + i;
        match i % BATCH_KINDS {
            0 => {
                let new_id = rows + id;
                let value = TEXTS[new_id % TEXTS.len()];
                statements[0].execute(params![
                    new_id as i64,
                    1_i64,
                    format!("arrival-{new_id}"),
                    value,
                    source_word_count(value)
                ])?;
            }
            1 => {
                statements[1].execute([id as i64])?;
            }
            2 => {
                let value = TEXTS[(id + 1) % TEXTS.len()];
                statements[2].execute(params![id as i64, value, source_word_count(value)])?;
            }
            3 => {
                statements[3].execute(params![id as i64, format!("renamed-{}", id % 8)])?;
            }
            4 => {
                let note_id = 3 + 2 * ((id / BATCH_KINDS) % mutable_active_notes);
                statements[4].execute([note_id as i64])?;
            }
            _ => {
                let note_id = 2 + 2 * ((id / BATCH_KINDS) % note_pairs);
                statements[5].execute([note_id as i64])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::source_word_count;

    #[test]
    fn projected_word_count_matches_source_spacing_rule() {
        for (text, expected) in [
            ("", 0),
            ("  ", 0),
            ("hello", 1),
            ("two words", 2),
            ("  three  words  here ", 5),
        ] {
            assert_eq!(source_word_count(text), expected);
        }
    }
}
