//! New source-shaped rollups, in the implementation order documented under
//! `docs/demos/`. Every generator is synthetic; upstream timings are not ours.

use super::{DemandCase, Handwritten};
use rusqlite::{params, Connection, Statement};

const BATCH_KINDS: usize = 6;
const SPANS_PER_TRACE: usize = 81;
const MIN_TRACES: usize = 128;
const BOOK_AUTHORS: usize = 5_000;

fn validate(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    if batch < BATCH_KINDS || rounds == 0 {
        return Err("batch must be at least 6 and rounds must be positive".into());
    }
    let touched = batch.checked_mul(rounds).ok_or("batch range overflows")?;
    let end = rows.checked_add(touched).ok_or("insert range overflows")?;
    if touched > rows || end > (i64::MAX as usize) / 2 {
        return Err(
            "base rows must cover all mixed batches and ids must fit SQLite INTEGER".into(),
        );
    }
    Ok(())
}

pub static ZENOD: DemandCase = DemandCase {
    indexed_query: None,
    name: "zenod_usage_totals",
    ddl: "CREATE TABLE tenant_usage(id INTEGER PRIMARY KEY, tenant_id INTEGER NOT NULL,
          kind TEXT NOT NULL, units INTEGER NOT NULL) STRICT",
    view_name: "zenod_totals",
    view_sql: "SELECT tenant_id, kind, COUNT(*), SUM(units) FROM tenant_usage \
               GROUP BY tenant_id, kind",
    covering_index_sql: "CREATE INDEX usage_covering ON tenant_usage(tenant_id, kind, units)",
    default_rows: 1_760_000,
    default_batch: 200,
    seed_fn: seed_zenod,
    validate_fn: validate,
    batch_sql: &[
        "INSERT INTO tenant_usage VALUES (?1, ?2, ?3, ?4)",
        "DELETE FROM tenant_usage WHERE id=?1",
        "UPDATE tenant_usage SET units=units+17 WHERE id=?1",
        "UPDATE tenant_usage SET tenant_id=tenant_id+100 WHERE id=?1",
        "UPDATE tenant_usage SET kind='corrected' WHERE id=?1",
        "UPDATE tenant_usage SET units=0 WHERE id=?1",
    ],
    apply_fn: apply_zenod,
    handwritten: Some(Handwritten {
        install: install_zenod,
        read_sql: "SELECT * FROM hand_rollup",
    }),
};

fn seed_zenod(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare(ZENOD.batch_sql[0])?;
        for id in 0..rows {
            let tenant = if id % 10 < 7 { 0 } else { 1 + id % 31 };
            insert.execute(params![
                id as i64,
                tenant as i64,
                format!("kind-{}", id % 4),
                (1 + id % 500) as i64
            ])?;
        }
    }
    tx.commit()
}

fn apply_zenod(
    s: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    for i in 0..batch {
        let id = round * batch + i;
        let kind = i % BATCH_KINDS;
        if kind == 0 {
            s[0].execute(params![
                (rows + id) as i64,
                (id % 32) as i64,
                "new",
                (id % 500 + 1) as i64
            ])?;
        } else {
            s[kind].execute([id as i64])?;
        }
    }
    Ok(())
}

pub static NICTI: DemandCase = DemandCase {
    indexed_query: None,
    name: "nicti_facet_counts",
    ddl: "CREATE TABLE assets(id INTEGER PRIMARY KEY, model TEXT NOT NULL, rating INTEGER NOT NULL) STRICT;
          CREATE TABLE asset_keywords(id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, keyword TEXT NOT NULL) STRICT;
          CREATE INDEX keywords_asset ON asset_keywords(asset_id)",
    view_name: "nicti_facets",
    view_sql: "SELECT assets.model, assets.rating, asset_keywords.keyword, COUNT(*) \
               FROM assets JOIN asset_keywords ON assets.id=asset_keywords.asset_id \
               GROUP BY assets.model, assets.rating, asset_keywords.keyword",
    covering_index_sql: "CREATE INDEX assets_covering ON assets(model, rating, id);
                         CREATE INDEX keywords_covering ON asset_keywords(asset_id, keyword)",
    default_rows: 2_000_000,
    default_batch: 200,
    seed_fn: seed_nicti,
    validate_fn: validate,
    batch_sql: &[
        "INSERT INTO assets VALUES (?1, ?2, ?3)",
        "INSERT INTO asset_keywords VALUES (?1, ?2, ?3)",
        "DELETE FROM asset_keywords WHERE id=?1",
        "UPDATE assets SET model='corrected', rating=(rating+1)%6 WHERE id=?1",
        "UPDATE asset_keywords SET keyword='retagged' WHERE id=?1",
        "UPDATE asset_keywords SET asset_id=?2, keyword=?3 WHERE id=?1",
        "DELETE FROM assets WHERE id=?1",
    ],
    apply_fn: apply_nicti,
    handwritten: Some(Handwritten { install: install_nicti, read_sql: "SELECT * FROM hand_rollup" }),
};

fn seed_nicti(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut asset = tx.prepare(NICTI.batch_sql[0])?;
        let mut keyword = tx.prepare(NICTI.batch_sql[1])?;
        for id in 0..rows {
            asset.execute(params![
                id as i64,
                format!("model-{}", id % 8),
                (id % 6) as i64
            ])?;
            for k in 0..2 {
                keyword.execute(params![
                    (id * 2 + k) as i64,
                    id as i64,
                    format!("keyword-{}", (id + k) % 64)
                ])?;
            }
        }
    }
    tx.commit()
}

fn apply_nicti(
    s: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    for i in 0..batch {
        let id = round * batch + i;
        match i % BATCH_KINDS {
            0 => {
                let new_id = rows + id;
                s[0].execute(params![new_id as i64, "new-model", (id % 6) as i64])?;
                for k in 0..2 {
                    s[1].execute(params![
                        (new_id * 2 + k) as i64,
                        new_id as i64,
                        format!("new-keyword-{k}")
                    ])?;
                }
            }
            1 => {
                s[2].execute([(id * 2) as i64])?;
            }
            2 => {
                s[3].execute([id as i64])?;
            }
            3 => {
                s[4].execute([(id * 2) as i64])?;
            }
            4 => {
                s[5].execute(params![(id * 2) as i64, 0, format!("moved-{id}")])?;
            }
            _ => {
                s[6].execute([id as i64])?;
            }
        }
    }
    Ok(())
}

pub static POSTIO: DemandCase = DemandCase {
    indexed_query: None,
    name: "postio_mailbox_footprint",
    ddl: "CREATE TABLE messages(id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL,
          size INTEGER NOT NULL, deleted_locally INTEGER NOT NULL) STRICT",
    view_name: "postio_footprint",
    view_sql: "SELECT account_id, COUNT(*), SUM(size) FROM messages \
               WHERE deleted_locally=0 GROUP BY account_id",
    covering_index_sql:
        "CREATE INDEX messages_covering ON messages(deleted_locally, account_id, size)",
    default_rows: 81_000,
    default_batch: 200,
    seed_fn: seed_postio,
    validate_fn: validate,
    batch_sql: &[
        "INSERT INTO messages VALUES (?1, ?2, ?3, 0)",
        "UPDATE messages SET deleted_locally=1 WHERE id=?1",
        "UPDATE messages SET deleted_locally=0 WHERE id=?1",
        "UPDATE messages SET size=size+1024 WHERE id=?1",
        "UPDATE messages SET account_id=account_id+32 WHERE id=?1",
        "DELETE FROM messages WHERE id=?1",
    ],
    apply_fn: apply_postio,
    handwritten: Some(Handwritten {
        install: install_postio,
        read_sql: "SELECT * FROM hand_rollup",
    }),
};

fn seed_postio(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO messages VALUES (?1, ?2, ?3, ?4)")?;
        for id in 0..rows {
            insert.execute(params![
                id as i64,
                (id % 8) as i64,
                (1024 + id % 1_000_000) as i64,
                i64::from(id % 2 == 0)
            ])?;
        }
    }
    tx.commit()
}

fn apply_postio(
    s: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    for i in 0..batch {
        let id = round * batch + i;
        let kind = i % BATCH_KINDS;
        if kind == 0 {
            s[0].execute(params![
                (rows + id) as i64,
                (id % 8) as i64,
                (1024 + id) as i64
            ])?;
        } else {
            s[kind].execute([id as i64])?;
        }
    }
    Ok(())
}

pub static PHOENIX: DemandCase = DemandCase {
    indexed_query: Some("SELECT traces.project_id, spans.day_bucket, SUM(spans.prompt_tokens), \
        SUM(spans.completion_tokens), COUNT(*) FROM traces INDEXED BY traces_covering \
        JOIN spans INDEXED BY spans_covering ON traces.id=spans.trace_id \
        GROUP BY traces.project_id, spans.day_bucket"),
    name: "phoenix_token_rollup",
    ddl: "CREATE TABLE traces(id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL) STRICT;
          CREATE TABLE spans(id INTEGER PRIMARY KEY, trace_id INTEGER NOT NULL, day_bucket INTEGER NOT NULL,
          prompt_tokens INTEGER NOT NULL, completion_tokens INTEGER NOT NULL) STRICT;
          CREATE INDEX spans_trace ON spans(trace_id)",
    view_name: "phoenix_tokens",
    view_sql: "SELECT traces.project_id, spans.day_bucket, SUM(spans.prompt_tokens), \
               SUM(spans.completion_tokens), COUNT(*) FROM traces JOIN spans \
               ON traces.id=spans.trace_id GROUP BY traces.project_id, spans.day_bucket",
    covering_index_sql: "CREATE INDEX traces_covering ON traces(project_id, id);
                         CREATE INDEX spans_covering ON spans(trace_id, day_bucket, prompt_tokens, completion_tokens)",
    default_rows: 4_952_148,
    default_batch: 200,
    seed_fn: seed_phoenix,
    validate_fn: validate_phoenix,
    batch_sql: &[
        "INSERT INTO spans VALUES (?1, ?2, ?3, ?4, ?5)",
        "DELETE FROM spans WHERE id=?1",
        "UPDATE spans SET prompt_tokens=prompt_tokens+17, completion_tokens=completion_tokens+3 WHERE id=?1",
        "UPDATE spans SET trace_id=?2 WHERE id=?1",
        "UPDATE traces SET project_id=project_id+16 WHERE id=?1",
        "UPDATE spans SET day_bucket=day_bucket+30 WHERE id=?1",
    ],
    apply_fn: apply_phoenix,
    handwritten: Some(Handwritten { install: install_phoenix, read_sql: "SELECT * FROM hand_rollup" }),
};

fn trace_count(rows: usize) -> usize {
    rows.div_ceil(SPANS_PER_TRACE).max(MIN_TRACES).min(rows)
}

fn trace_for_span(id: usize, rows: usize) -> usize {
    if rows < MIN_TRACES * SPANS_PER_TRACE {
        id % trace_count(rows)
    } else {
        id / SPANS_PER_TRACE
    }
}

fn validate_phoenix(rows: usize, batch: usize, rounds: usize) -> Result<(), String> {
    validate(rows, batch, rounds)?;
    if rounds
        .checked_mul(batch.div_ceil(BATCH_KINDS))
        .is_none_or(|needed| needed > trace_count(rows))
    {
        return Err("not enough traces for disjoint parent updates across all rounds".into());
    }
    Ok(())
}

fn seed_phoenix(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let count = trace_count(rows);
        let mut trace = tx.prepare("INSERT INTO traces VALUES (?1, ?2)")?;
        let mut span = tx.prepare(PHOENIX.batch_sql[0])?;
        for id in 0..count {
            trace.execute(params![id as i64, (id % 16) as i64])?;
        }
        for id in 0..rows {
            let trace = trace_for_span(id, rows);
            span.execute(params![
                id as i64,
                trace as i64,
                (trace % 30) as i64,
                (id % 1000) as i64,
                (id % 200) as i64
            ])?;
        }
    }
    tx.commit()
}

fn apply_phoenix(
    s: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    let count = trace_count(rows);
    for i in 0..batch {
        let id = round * batch + i;
        match i % BATCH_KINDS {
            0 => {
                s[0].execute(params![
                    (rows + id) as i64,
                    (id % count) as i64,
                    30,
                    (id % 1000) as i64,
                    (id % 200) as i64
                ])?;
            }
            3 => {
                s[3].execute(params![
                    id as i64,
                    ((trace_for_span(id, rows) + 1) % count) as i64
                ])?;
            }
            4 => {
                s[4].execute([(round * batch.div_ceil(BATCH_KINDS) + i / BATCH_KINDS) as i64])?;
            }
            kind => {
                s[kind].execute([id as i64])?;
            }
        }
    }
    Ok(())
}

pub static POCKETBASE: DemandCase = DemandCase {
    indexed_query: None,
    name: "pocketbase_author_counts",
    ddl: "CREATE TABLE books(id INTEGER PRIMARY KEY, author TEXT NOT NULL) STRICT",
    view_name: "pocketbase_counts",
    view_sql: "SELECT author, COUNT(*) FROM books GROUP BY author",
    covering_index_sql: "CREATE INDEX books_covering ON books(author)",
    default_rows: 18_000,
    default_batch: 40,
    seed_fn: seed_pocketbase,
    validate_fn: validate,
    batch_sql: &[
        "INSERT INTO books VALUES (?1, ?2)",
        "DELETE FROM books WHERE id=?1",
        "UPDATE books SET author=?2 WHERE id=?1",
    ],
    apply_fn: apply_pocketbase,
    handwritten: Some(Handwritten {
        install: install_pocketbase,
        read_sql: "SELECT * FROM hand_rollup",
    }),
};

fn seed_pocketbase(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare(POCKETBASE.batch_sql[0])?;
        for id in 0..rows {
            insert.execute(params![id as i64, format!("author-{}", id % BOOK_AUTHORS)])?;
        }
    }
    tx.commit()
}

fn apply_pocketbase(
    s: &mut [Statement<'_>],
    rows: usize,
    batch: usize,
    round: usize,
) -> rusqlite::Result<()> {
    for i in 0..batch {
        let id = round * batch + i;
        match i % 3 {
            0 => {
                s[0].execute(params![(rows + id) as i64, format!("new-author-{id}")])?;
            }
            1 => {
                s[1].execute([id as i64])?;
            }
            _ => {
                s[2].execute(params![id as i64, format!("corrected-author-{id}")])?;
            }
        }
    }
    Ok(())
}

// The handwritten baseline is generated only from these fixed, trusted case
// definitions. It is deliberately independent SQL, not ivmlite's delta engine.
// All keys and sum inputs in these fixtures are NOT NULL; a signed count lets
// a retraction remove the last row of a group, including zero-valued sums.
struct Rollup {
    columns: &'static str,
    keys: &'static str,
    sums: &'static [&'static str],
    tables: &'static [&'static str],
    projection: &'static str,
    from: &'static str,
    predicate: &'static str,
}

fn install(c: &Connection, case: &DemandCase, spec: &Rollup) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE TABLE hand_rollup({}); INSERT INTO hand_rollup {}",
        spec.columns, case.view_sql
    ))?;
    let sum_names: Vec<_> = (0..spec.sums.len()).map(|i| format!("s{i}")).collect();
    let mut assignments = vec!["n=n+excluded.n".to_string()];
    assignments.extend(sum_names.iter().map(|s| format!("{s}={s}+excluded.{s}")));
    let targets = std::iter::once(spec.keys.to_string())
        .chain(std::iter::once("n".to_string()))
        .chain(sum_names.iter().cloned())
        .collect::<Vec<_>>()
        .join(",");
    for table in spec.tables {
        let mut stmt = c.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let delta = |image: &str, sign: i32| {
            // A one-row relation exposes OLD even after DELETE/UPDATE, and
            // avoids reading the entire base table for a single-row delta.
            let image_columns = columns
                .iter()
                .map(|col| format!("{image}.{col} AS {col}"))
                .collect::<Vec<_>>()
                .join(",");
            let source = format!("(SELECT {image_columns}) AS {table}");
            let from = format!("FROM {}", spec.from)
                .replace(&format!("FROM {table} "), &format!("FROM {source} "))
                .replace(&format!("JOIN {table} "), &format!("JOIN {source} "));
            let sums = spec
                .sums
                .iter()
                .map(|s| format!(", {sign}*SUM({s})"))
                .collect::<String>();
            format!("INSERT INTO hand_rollup({targets}) SELECT {}, {sign}*COUNT(*){sums} {from} WHERE {} GROUP BY {} ON CONFLICT({}) DO UPDATE SET {}; DELETE FROM hand_rollup WHERE ({}) IN (SELECT {} {from} WHERE {}) AND n=0;",
                spec.projection, spec.predicate, spec.projection, spec.keys, assignments.join(","), spec.keys, spec.projection, spec.predicate)
        };
        let remove = delta("OLD", -1);
        let add = delta("NEW", 1);
        c.execute_batch(&format!(
            "CREATE TRIGGER hand_{table}_insert AFTER INSERT ON {table} BEGIN {add} END;
             CREATE TRIGGER hand_{table}_delete AFTER DELETE ON {table} BEGIN {remove} END;
             CREATE TRIGGER hand_{table}_update AFTER UPDATE ON {table} BEGIN {remove} {add} END;"
        ))?;
    }
    Ok(())
}

fn install_zenod(c: &Connection) -> rusqlite::Result<()> {
    install(c, &ZENOD, &Rollup {
        columns: "tenant_id INTEGER NOT NULL, kind TEXT NOT NULL, n INTEGER NOT NULL, s0 INTEGER NOT NULL, PRIMARY KEY(tenant_id,kind)",
        keys: "tenant_id,kind", sums: &["tenant_usage.units"], tables: &["tenant_usage"],
        projection: "tenant_usage.tenant_id,tenant_usage.kind", from: "tenant_usage ", predicate: "1",
    })
}
fn install_nicti(c: &Connection) -> rusqlite::Result<()> {
    install(c, &NICTI, &Rollup {
        columns: "model TEXT NOT NULL, rating INTEGER NOT NULL, keyword TEXT NOT NULL, n INTEGER NOT NULL, PRIMARY KEY(model,rating,keyword)",
        keys: "model,rating,keyword", sums: &[], tables: &["assets","asset_keywords"],
        projection: "assets.model,assets.rating,asset_keywords.keyword",
        from: "assets JOIN asset_keywords ON assets.id=asset_keywords.asset_id ", predicate: "1",
    })
}
fn install_postio(c: &Connection) -> rusqlite::Result<()> {
    install(
        c,
        &POSTIO,
        &Rollup {
            columns: "account_id INTEGER PRIMARY KEY, n INTEGER NOT NULL, s0 INTEGER NOT NULL",
            keys: "account_id",
            sums: &["messages.size"],
            tables: &["messages"],
            projection: "messages.account_id",
            from: "messages ",
            predicate: "messages.deleted_locally=0",
        },
    )
}
fn install_phoenix(c: &Connection) -> rusqlite::Result<()> {
    // Explicit delta target columns preserve the public sums-before-count order.
    install(c, &PHOENIX, &Rollup {
        columns: "project_id INTEGER NOT NULL, day_bucket INTEGER NOT NULL, s0 INTEGER NOT NULL, s1 INTEGER NOT NULL, n INTEGER NOT NULL, PRIMARY KEY(project_id,day_bucket)",
        keys: "project_id,day_bucket", sums: &["spans.prompt_tokens","spans.completion_tokens"], tables: &["traces","spans"],
        projection: "traces.project_id,spans.day_bucket",
        from: "traces JOIN spans ON traces.id=spans.trace_id ", predicate: "1",
    })
}
fn install_pocketbase(c: &Connection) -> rusqlite::Result<()> {
    install(
        c,
        &POCKETBASE,
        &Rollup {
            columns: "author TEXT PRIMARY KEY, n INTEGER NOT NULL",
            keys: "author",
            sums: &[],
            tables: &["books"],
            projection: "books.author",
            from: "books ",
            predicate: "1",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_bench::{check_same_multiset, Workload, ROUNDS};

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn new_generators_keep_their_documented_shapes() {
        for case in [&ZENOD, &NICTI, &POSTIO, &PHOENIX, &POCKETBASE] {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch(case.ddl).unwrap();
            case.seed(&c, 18_000).unwrap();
            c.execute_batch(case.covering_index_sql).unwrap();
            check_same_multiset(&c, case.indexed_recompute_sql(), case.view_sql).unwrap();
            match case.name {
                "zenod_usage_totals" => assert_eq!(
                    count(&c, "SELECT COUNT(*) FROM tenant_usage WHERE tenant_id=0"),
                    12_600
                ),
                "nicti_facet_counts" => {
                    assert_eq!(count(&c, "SELECT COUNT(*) FROM asset_keywords"), 36_000);
                    assert_eq!(count(&c, "SELECT COUNT(*) FROM (SELECT asset_id FROM asset_keywords GROUP BY asset_id HAVING COUNT(*)!=2 OR COUNT(DISTINCT keyword)!=2)"), 0);
                }
                "postio_mailbox_footprint" => assert_eq!(
                    count(&c, "SELECT COUNT(*) FROM messages WHERE deleted_locally=0"),
                    9_000
                ),
                "phoenix_token_rollup" => assert_eq!(count(&c, "SELECT COUNT(*) FROM traces"), 223),
                "pocketbase_author_counts" => {
                    assert_eq!(count(&c, "SELECT COUNT(DISTINCT author) FROM books"), 5_000)
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn new_case_validation_rejects_incomplete_or_overflowing_batches() {
        for case in [&ZENOD, &NICTI, &POSTIO, &PHOENIX, &POCKETBASE] {
            assert!(case.validate(1_200, 5, ROUNDS).is_err());
            assert!(case.validate(1_200, 40, 0).is_err());
            assert!(case.validate(usize::MAX, 40, ROUNDS).is_err());
            assert!(case.validate(1_200, usize::MAX, ROUNDS).is_err());
        }
        assert!(PHOENIX.validate(1_200, 100, ROUNDS).is_err());
    }
}
