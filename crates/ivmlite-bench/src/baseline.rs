use std::time::Instant;

use ivmlite_workload::{TraceOp, ViewSpec, Workload};
use rusqlite::{Connection, Statement};

/// Create the base table and load its initial data. Untimed.
///
/// The table definition comes from the workload, and `id INTEGER PRIMARY KEY`
/// is a hard requirement (spec §10.3 item 5): locating a row by the values of
/// all its columns has no usable index, `EXPLAIN QUERY PLAN` shows
/// `SCAN orders`, and deletes then cost time linear in the base table — while
/// "incremental cost does not grow with the base table" is the one thing this
/// benchmark exists to show.
pub fn seed_base(conn: &Connection, w: &Workload) -> rusqlite::Result<()> {
    conn.execute_batch(&w.schema.ddl)?;
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{}\"(id, region, amount) VALUES (?1, ?2, ?3)",
            w.schema.table
        ))?;
        for (id, region, amount) in w.rows() {
            ins.execute((id, &region, amount))?;
        }
    }
    tx.commit()
}

/// Create the summary table, **bootstrap it in full first**, then create the
/// triggers. The order cannot be reversed.
///
/// Spec §10.3 item 4: creating an empty summary table after the base table
/// already has data yields a view that is permanently incomplete, and whose
/// maintenance cost is not representative. The triggers must be created
/// **after** the bootstrap, or they would count the bootstrap's
/// `INSERT ... SELECT` a second time.
///
/// Note the summary table's `k` column is declared `TEXT`, not `ANY`: a STRICT
/// table lets an `ANY` column mix types row by row, and `1` and `'1'` would
/// split into two groups (spec §7.1).
pub fn install_trigger_view(conn: &Connection, table: &str, v: &ViewSpec) -> rusqlite::Result<()> {
    let t = v.table();
    let k = v.threshold;
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE "{t}" (
            k TEXT    NOT NULL PRIMARY KEY,
            s INTEGER NOT NULL,
            c INTEGER NOT NULL
        ) STRICT;

        INSERT INTO "{t}"(k, s, c)
            SELECT region, SUM(amount), COUNT(*)
            FROM "{table}" WHERE amount > {k} GROUP BY region;

        CREATE TRIGGER "{t}_ins" AFTER INSERT ON "{table}"
        WHEN NEW.amount > {k} BEGIN
            INSERT INTO "{t}"(k, s, c) VALUES (NEW.region, NEW.amount, 1)
            ON CONFLICT(k) DO UPDATE SET s = s + NEW.amount, c = c + 1;
        END;

        CREATE TRIGGER "{t}_del" AFTER DELETE ON "{table}"
        WHEN OLD.amount > {k} BEGIN
            UPDATE "{t}" SET s = s - OLD.amount, c = c - 1 WHERE k = OLD.region;
            DELETE FROM "{t}" WHERE k = OLD.region AND c = 0;
        END;
        "#
    ))
}

/// The statements `apply` executes, compiled before the timer starts.
///
/// They must be prepared **after every trigger exists**. Creating a trigger
/// changes the schema, which invalidates statements compiled before it, and
/// SQLite compiles a trigger's body into the statement that fires it. Timing a
/// `prepare` therefore measures compilation, not maintenance — and each matrix
/// cell runs exactly once, so that first-call cost is all a cell ever records.
/// Measured with 10 trigger views and a one-row batch: the first call cost
/// 0.129 ms against a 0.009 ms steady state (external review P2-3).
pub struct ApplyStatements<'c> {
    insert: Statement<'c>,
    delete: Statement<'c>,
}

impl<'c> ApplyStatements<'c> {
    pub fn prepare(conn: &'c Connection, table: &str) -> rusqlite::Result<Self> {
        Ok(Self {
            insert: conn.prepare(&format!(
                "INSERT INTO \"{table}\"(id, region, amount) VALUES (?1, ?2, ?3)"
            ))?,
            delete: conn.prepare(&format!("DELETE FROM \"{table}\" WHERE id = ?1"))?,
        })
    }
}

/// Apply one batch of changes and return the elapsed milliseconds. The timed
/// region covers execution and the commit — committing is a real cost — but
/// not statement compilation, which `ApplyStatements::prepare` does up front.
///
/// For the HandWrittenTrigger baseline the trigger work lands here, so
/// `apply_ms(trigger) - apply_ms(no_maintenance)` is the write amplification
/// spec §10.5 asks for.
pub fn apply(
    conn: &Connection,
    stmts: &mut ApplyStatements<'_>,
    ops: &[TraceOp],
) -> rusqlite::Result<f64> {
    let start = Instant::now();
    let tx = conn.unchecked_transaction()?;
    for op in ops {
        match op {
            TraceOp::Insert { id, region, amount } => {
                stmts.insert.execute((id, region, amount))?;
            }
            TraceOp::Delete { id } => {
                stmts.delete.execute((id,))?;
            }
        }
    }
    tx.commit()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// One compiled statement per view, prepared before the timer starts — the
/// same reasoning as `ApplyStatements`, applied to the naive baseline so both
/// sides of the comparison exclude compilation. `prepare_cached` would not
/// have been enough: rusqlite's statement cache holds 16 entries by default
/// and a cell can have 200 views.
pub struct RecomputeStatements<'c>(Vec<Statement<'c>>);

impl<'c> RecomputeStatements<'c> {
    pub fn prepare(conn: &'c Connection, w: &Workload) -> rusqlite::Result<Self> {
        w.views
            .iter()
            .map(|v| conn.prepare(&v.sql(&w.schema.table)))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map(Self)
    }
}

/// Naive recompute: run every view's SQL once and drain its result set.
///
/// The results are deliberately **not** written back to a table. That biases
/// the comparison toward naive recompute on purpose: if incremental maintenance
/// cannot beat a recompute that only reads, the conclusion is beyond dispute.
pub fn recompute_all(stmts: &mut RecomputeStatements<'_>) -> rusqlite::Result<f64> {
    let start = Instant::now();
    for stmt in &mut stmts.0 {
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDL: &str = "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT";

    fn trigger_rows(conn: &Connection, v: &ViewSpec) -> Vec<(String, i64, i64)> {
        let mut stmt = conn
            .prepare(&format!("SELECT k, s, c FROM \"{}\" ORDER BY k", v.table()))
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Evaluate the view's SQL directly, reading `region, SUM(amount), COUNT(*)`
    /// in the same (k, s, c) encoding the trigger table uses, sorted, so the two
    /// can be compared row by row.
    fn direct_query_rows(conn: &Connection, v: &ViewSpec, table: &str) -> Vec<(String, i64, i64)> {
        let sql = format!("{} ORDER BY region", v.sql(table));
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// I6: `ivmlite-bench` used to have no `#[test]` at all, so nothing checked
    /// that the summary table maintained by the hand-written triggers equals,
    /// row for row, a direct evaluation of the view's SQL after the trace is
    /// replayed. Every headline ratio in `docs/bench/README.md` has this
    /// baseline's time as its denominator; if the triggers computed the wrong
    /// thing (say the `WHEN OLD.amount > k` guard, or the `c = 0` cleanup, were
    /// wrong), those numbers would be measuring the wrong work.
    ///
    /// The base table deliberately has data before `install_trigger_view` runs,
    /// mirroring the real order in `main.rs::run_one`, where `seed_base` comes
    /// first. That also exercises the bootstrap statement itself — spec §10.3
    /// item 4: the triggers must be created after the bootstrap, and the
    /// bootstrap must actually count the existing data, or the summary table is
    /// permanently missing history (contrast
    /// `install_trigger_view_without_bootstrap_would_be_incomplete` below).
    #[test]
    fn trigger_maintained_table_matches_direct_query_after_seed_and_updates() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DDL).unwrap();

        // Load the "existing data" first, then install the triggers — the case
        // install_trigger_view's bootstrap statement has to get right.
        let seed_rows = [
            (0i64, "a", 10i64),
            (1, "a", 20),
            (2, "b", 5),
            (3, "b", 50),
            (4, "c", 1),
        ];
        {
            let tx = conn.unchecked_transaction().unwrap();
            {
                let mut ins = tx
                    .prepare("INSERT INTO orders(id, region, amount) VALUES (?1, ?2, ?3)")
                    .unwrap();
                for (id, region, amount) in seed_rows {
                    ins.execute((id, region, amount)).unwrap();
                }
            }
            tx.commit().unwrap();
        }

        let view = ViewSpec {
            id: 0,
            threshold: 8,
        };
        install_trigger_view(&conn, "orders", &view).unwrap();

        // Replay a batch mixing inserts and deletes through baseline::apply —
        // exactly the path the benchmark itself uses.
        let ops = vec![
            TraceOp::Insert {
                id: 5,
                region: "a".into(),
                amount: 30,
            },
            TraceOp::Delete { id: 2 }, // amount=5, below the threshold to begin with
            TraceOp::Insert {
                id: 6,
                region: "c".into(),
                amount: 100,
            },
        ];
        let mut stmts = ApplyStatements::prepare(&conn, "orders").unwrap();
        apply(&conn, &mut stmts, &ops).unwrap();
        drop(stmts);

        let got = trigger_rows(&conn, &view);
        let want = direct_query_rows(&conn, &view, "orders");
        assert_eq!(
            got, want,
            "the trigger-maintained summary table must equal a direct evaluation of the view SQL, row for row"
        );
        // Also pin the concrete numbers, so the two sides cannot "confirm" each
        // other by sharing the same (possibly wrong) SQL.
        assert_eq!(
            got,
            vec![
                ("a".to_string(), 60, 3),
                ("b".to_string(), 50, 1),
                ("c".to_string(), 100, 1),
            ]
        );
    }

    /// The second half of I6: the bootstrap must happen **before** the triggers
    /// are created (spec §10.3 item 4). Hand-assembled SQL simulates the mutation
    /// "the bootstrap statement is missing", showing that if that bug appeared,
    /// the first test in this group would see a difference. The assertion here
    /// is direct — a trigger table without the bootstrap differs from one with
    /// it — so the methodological constraint is pinned by a test that can go
    /// red, not merely asserted by a comment.
    #[test]
    fn install_trigger_view_without_bootstrap_would_be_incomplete() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DDL).unwrap();
        for (id, region, amount) in [(0i64, "a", 10i64), (1, "a", 20), (2, "b", 50)] {
            conn.execute(
                "INSERT INTO orders(id, region, amount) VALUES (?1, ?2, ?3)",
                (id, region, amount),
            )
            .unwrap();
        }

        let view = ViewSpec {
            id: 0,
            threshold: 0,
        };
        // Hand-assemble the version missing the bootstrap INSERT ... SELECT:
        // create the table and the triggers, nothing else.
        let t = view.table();
        conn.execute_batch(&format!(
            r#"
            CREATE TABLE "{t}" (k TEXT NOT NULL PRIMARY KEY, s INTEGER NOT NULL, c INTEGER NOT NULL) STRICT;
            CREATE TRIGGER "{t}_ins" AFTER INSERT ON "orders" WHEN NEW.amount > 0 BEGIN
                INSERT INTO "{t}"(k, s, c) VALUES (NEW.region, NEW.amount, 1)
                ON CONFLICT(k) DO UPDATE SET s = s + NEW.amount, c = c + 1;
            END;
            "#
        ))
        .unwrap();

        let without_bootstrap = trigger_rows(&conn, &view);
        let want = direct_query_rows(&conn, &view, "orders");
        assert_ne!(
            without_bootstrap, want,
            "skipping the bootstrap must leave the summary table missing history — \
             if these are equal, the scenario is not exercising a missing bootstrap at all"
        );
        assert!(
            without_bootstrap.is_empty(),
            "without the bootstrap statement, the three rows that existed before the triggers are never counted"
        );
    }
}
