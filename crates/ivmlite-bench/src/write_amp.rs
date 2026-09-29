//! M1b Phase 4 Task 5 (spec §7): the write-amplification workload's own
//! runner. `write-amp.toml`'s `accounts` table has two ordinary UNIQUE keys
//! besides its `INTEGER PRIMARY KEY`, so it can measure what §3's matrix
//! cannot: the cost of REPLACE's UNIQUE-candidate lookup, and of an UPDATE
//! trigger, under both `recursive_triggers` modes.
//!
//! The three engines here are a different, fixed list from `engine::ENGINES`
//! (spec §7: "no_maintenance, hand_written_trigger ... and ivmlite" —
//! `naive_recompute` plays no part in a write-amplification measurement,
//! since nothing here re-runs a view's SQL during the timed apply). `Space`
//! and its two `PRAGMA` samplers are re-declared here rather than imported
//! from `engine.rs`: that module belongs to Task 1, and Task 5's brief does
//! not list it among the files this task may touch.

use std::path::Path;
use std::time::Instant;

use ivmlite_workload::{ViewSpec, WriteAmpWorkload, WriteOp, WriteOpKind};
use rusqlite::{Connection, Statement};

use crate::engine::{rotate_left, verify_view, Engine};

/// The three engines a write-amp cell measures (spec §7).
const WRITE_AMP_ENGINES: [Engine; 3] = [
    Engine::NoMaintenance,
    Engine::HandWrittenTrigger,
    Engine::Ivmlite,
];

/// `WRITE_AMP_ENGINES` rotated left by the combination index — the index of
/// the (`views`, op, `recursive_triggers`) combination, not of the engine
/// itself (controller ruling 1: reuse the existing generic `rotate_left`,
/// rather than a second rotation rule).
fn write_amp_order(combo_index: usize) -> [Engine; 3] {
    let v = rotate_left(&WRITE_AMP_ENGINES, combo_index);
    [v[0], v[1], v[2]]
}

/// One space sample: `PRAGMA page_count` and `PRAGMA freelist_count` (spec
/// §3.4's technique, reused for write-amp's own four sample points).
#[derive(Debug, Clone, Copy, Default)]
struct Space {
    page_count: i64,
    freelist_count: i64,
}

fn space(conn: &Connection) -> rusqlite::Result<Space> {
    Ok(Space {
        page_count: conn.query_row("PRAGMA page_count", [], |r| r.get(0))?,
        freelist_count: conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?,
    })
}

fn page_size(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA page_size", [], |r| r.get(0))
}

/// One row this module's runner produces; `main.rs` formats and prints it.
pub struct WriteAmpRow {
    pub engine: Engine,
    pub views: usize,
    pub op: WriteOpKind,
    pub recursive_triggers: bool,
    pub rows: usize,
    pub apply_ms: f64,
    pub page_size: i64,
    pub base_pages: i64,
    pub base_free: i64,
    pub bootstrapped_pages: i64,
    pub bootstrapped_free: i64,
    pub written_pages: i64,
    pub written_free: i64,
    pub maintained_pages: i64,
    pub maintained_free: i64,
}

/// The statements `apply_trace` runs, prepared before the timer starts (the
/// same reasoning as `baseline::ApplyStatements`: compiling a statement
/// under the timer would measure compilation, not maintenance).
struct WriteStatements<'c> {
    insert: Statement<'c>,
    delete: Statement<'c>,
    update: Statement<'c>,
    replace: Statement<'c>,
}

impl<'c> WriteStatements<'c> {
    fn prepare(conn: &'c Connection, table: &str) -> rusqlite::Result<Self> {
        Ok(Self {
            insert: conn.prepare(&format!(
                "INSERT INTO \"{table}\"(id, email, handle, region, amount) VALUES (?1, ?2, ?3, ?4, ?5)"
            ))?,
            delete: conn.prepare(&format!("DELETE FROM \"{table}\" WHERE id = ?1"))?,
            update: conn.prepare(&format!(
                "UPDATE \"{table}\" SET region = ?2, amount = ?3 WHERE id = ?1"
            ))?,
            replace: conn.prepare(&format!(
                "INSERT OR REPLACE INTO \"{table}\"(id, email, handle, region, amount) VALUES (?1, ?2, ?3, ?4, ?5)"
            ))?,
        })
    }
}

/// Execute one op and check its per-row effect (spec §7's "verified per row"
/// column): `Insert`, `Delete` and `Update` must each change exactly one
/// row. A `Replace` is not checked here — its effect is the trace-wide net
/// row count `apply_trace` checks once, after the timer, since `changes()`
/// after `INSERT OR REPLACE` does not report the conflicting deletes as
/// rows this statement itself changed.
///
/// A miss aborts the whole run (ruling 3): the caller (`apply_trace`, then
/// `run_one`, then `run_write_amp`) propagates this `Err` all the way out,
/// so a wrong effect never reaches the CSV.
fn execute_op(stmts: &mut WriteStatements<'_>, op: &WriteOp) -> Result<(), String> {
    let changed = match op {
        WriteOp::Insert {
            id,
            email,
            handle,
            region,
            amount,
        } => stmts
            .insert
            .execute((id, email, handle, region, amount))
            .map_err(|e| e.to_string())?,
        WriteOp::Delete { id } => stmts.delete.execute((id,)).map_err(|e| e.to_string())?,
        WriteOp::Update { id, region, amount } => stmts
            .update
            .execute((id, region, amount))
            .map_err(|e| e.to_string())?,
        WriteOp::Replace {
            id,
            email,
            handle,
            region,
            amount,
            ..
        } => {
            stmts
                .replace
                .execute((id, email, handle, region, amount))
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    };
    if changed != 1 {
        return Err(format!("{op:?} changed {changed} rows, expected exactly 1"));
    }
    Ok(())
}

/// Create the `accounts` table and load its base rows (untimed).
fn seed_base(conn: &Connection, w: &WriteAmpWorkload) -> rusqlite::Result<()> {
    conn.execute_batch(&w.schema.ddl)?;
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{}\"(id, email, handle, region, amount) VALUES (?1, ?2, ?3, ?4, ?5)",
            w.schema.table
        ))?;
        for (id, email, handle, region, amount) in w.rows() {
            ins.execute((id, &email, &handle, &region, amount))?;
        }
    }
    tx.commit()
}

/// The net row count a replace trace must leave behind: `base − Σ
/// expect_removed + M` (spec §7). Summed from the trace's own claimed
/// `expect_removed` per op, rather than hard-coded per kind, so the check
/// stays correct even if a kind's `expect_removed` ever changed.
fn expected_replace_total(base_rows: usize, ops: &[WriteOp]) -> i64 {
    let removed: usize = ops
        .iter()
        .map(|op| match op {
            WriteOp::Replace { expect_removed, .. } => *expect_removed,
            _ => 0,
        })
        .sum();
    base_rows as i64 - removed as i64 + ops.len() as i64
}

/// Check the net row count for a replace trace (ruling 3): a miss aborts the
/// run, exactly like a per-op effect miss.
fn check_replace_net_count(conn: &Connection, table: &str, expected: i64) -> Result<(), String> {
    let got: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if got != expected {
        return Err(format!(
            "replace trace left {got} rows in \"{table}\", expected {expected}"
        ));
    }
    Ok(())
}

/// Run the trace in transactions of `tx_rows`, timed as one region (spec
/// §7's `apply_ms`). Every `Insert`/`Delete`/`Update` op is checked as it
/// runs, inside the timed region; a replace trace's net count is checked
/// once, right after the timer stops.
#[allow(clippy::too_many_arguments)]
fn apply_trace(
    conn: &Connection,
    stmts: &mut WriteStatements<'_>,
    table: &str,
    ops: &[WriteOp],
    tx_rows: usize,
    base_rows: usize,
    is_replace: bool,
) -> Result<f64, String> {
    let start = Instant::now();
    for chunk in ops.chunks(tx_rows.max(1)) {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        for op in chunk {
            execute_op(stmts, op)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    let apply_ms = start.elapsed().as_secs_f64() * 1000.0;

    if is_replace {
        check_replace_net_count(conn, table, expected_replace_total(base_rows, ops))?;
    }
    Ok(apply_ms)
}

/// Set the connection's `recursive_triggers` pragma to `mode` (spec §7 step
/// 4). Factored out of `run_one` so the wiring — "the workload's
/// `recursive_triggers` value actually reaches the connection" — is directly
/// testable on its own, rather than only observable through a whole cell's
/// behavior (and for a replace op, `hand_written_trigger`'s view is never
/// even checked, so a bug here could otherwise go unnoticed).
fn set_recursive_triggers(conn: &Connection, mode: bool) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA recursive_triggers = {}",
        if mode { 1 } else { 0 }
    ))
}

/// Run one (`views`, op, `recursive_triggers`) cell for one engine: open a
/// fresh database, seed it, bootstrap the views, run the timed apply, then
/// refresh and verify (spec §7). `lib` is the extension library, used only
/// for `Engine::Ivmlite`.
#[allow(clippy::too_many_arguments)]
fn run_one(
    w: &WriteAmpWorkload,
    views: &[ViewSpec],
    kind: WriteOpKind,
    trace: &[WriteOp],
    recursive_triggers: bool,
    engine: Engine,
    table: &str,
    lib: &Path,
) -> Result<WriteAmpRow, String> {
    let is_replace = matches!(
        kind,
        WriteOpKind::ReplaceRowid | WriteOpKind::ReplaceUnique | WriteOpKind::ReplaceTwo
    );

    // ---- open + seed (untimed) ----
    let conn = if engine == Engine::Ivmlite {
        ivmlite_test::open_with_extension_at(None, lib).map_err(|e| e.to_string())?
    } else {
        Connection::open_in_memory().map_err(|e| e.to_string())?
    };
    seed_base(&conn, w).map_err(|e| e.to_string())?;
    let page_size_val = page_size(&conn).map_err(|e| e.to_string())?;
    let base = space(&conn).map_err(|e| e.to_string())?;

    // ---- bootstrap the views ----
    match engine {
        Engine::HandWrittenTrigger => {
            for v in views {
                crate::baseline::install_trigger_view_with_update(&conn, table, v)
                    .map_err(|e| e.to_string())?;
            }
        }
        Engine::Ivmlite => {
            for v in views {
                let sql = v.sql(table).replace('\'', "''");
                let name = v.table();
                conn.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE \"{name}\" USING ivm('{sql}')"
                ))
                .map_err(|e| e.to_string())?;
            }
        }
        Engine::NoMaintenance => {}
        Engine::NaiveRecompute => {
            unreachable!("write-amp never selects naive_recompute (WRITE_AMP_ENGINES)")
        }
    }
    let bootstrapped = space(&conn).map_err(|e| e.to_string())?;

    // ---- prepare every statement, then set the mode (spec §7 step 4) ----
    let mut stmts = WriteStatements::prepare(&conn, table).map_err(|e| e.to_string())?;
    set_recursive_triggers(&conn, recursive_triggers).map_err(|e| e.to_string())?;
    let mut refresh_stmts = Vec::new();
    if engine == Engine::Ivmlite {
        for v in views {
            let name = v.table();
            refresh_stmts.push(
                conn.prepare(&format!(
                    "INSERT INTO \"{name}\"(\"{name}\") VALUES ('refresh')"
                ))
                .map_err(|e| e.to_string())?,
            );
        }
    }

    // ---- timed apply ----
    let apply_ms = apply_trace(
        &conn,
        &mut stmts,
        table,
        trace,
        w.tx_rows,
        w.base_rows,
        is_replace,
    )?;
    let written = space(&conn).map_err(|e| e.to_string())?;

    // ---- untimed: refresh, then verify against the oracle ----
    if engine == Engine::Ivmlite {
        for stmt in &mut refresh_stmts {
            stmt.execute([]).map_err(|e| e.to_string())?;
        }
    }
    match engine {
        Engine::Ivmlite => {
            for v in views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        Engine::HandWrittenTrigger if !is_replace => {
            for v in views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        // A replace op's implicit conflict-resolution deletes are only ever
        // visible to a hand-written AFTER DELETE trigger when
        // recursive_triggers is ON (spec §7); measured as cost-only here
        // regardless of the mode, so hand_written_trigger's view is never
        // compared with the oracle for a replace op.
        Engine::HandWrittenTrigger => {}
        Engine::NoMaintenance => {}
        Engine::NaiveRecompute => {
            unreachable!("write-amp never selects naive_recompute (WRITE_AMP_ENGINES)")
        }
    }
    let maintained = space(&conn).map_err(|e| e.to_string())?;

    Ok(WriteAmpRow {
        engine,
        views: views.len(),
        op: kind,
        recursive_triggers,
        rows: trace.len(),
        apply_ms,
        page_size: page_size_val,
        base_pages: base.page_count,
        base_free: base.freelist_count,
        bootstrapped_pages: bootstrapped.page_count,
        bootstrapped_free: bootstrapped.freelist_count,
        written_pages: written.page_count,
        written_free: written.freelist_count,
        maintained_pages: maintained.page_count,
        maintained_free: maintained.freelist_count,
    })
}

/// Run every (`views`, op, `recursive_triggers`) combination of `w`, over
/// the three write-amp engines, engine order rotating with the combination
/// index (spec §7). `lib` is the extension library, used only for
/// `Engine::Ivmlite`.
pub fn run_write_amp(w: &WriteAmpWorkload, lib: &Path) -> Result<Vec<WriteAmpRow>, String> {
    let table = w.schema.table.clone();
    let mut rows = Vec::new();
    let mut combo_index = 0usize;

    for &n_views in &w.view_counts {
        let views = w.views(n_views);
        for &kind in &w.ops {
            let trace = w.trace(kind);
            for &mode in &w.recursive_triggers {
                for engine in write_amp_order(combo_index) {
                    let row = run_one(w, &views, kind, &trace, mode, engine, &table, lib)
                        .map_err(|e| {
                            format!(
                                "write-amp views={n_views} op={} recursive_triggers={mode} engine={}: {e}",
                                kind.label(),
                                engine.label(),
                            )
                        })?;
                    rows.push(row);
                }
                combo_index += 1;
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_workload::ViewSpec;
    use std::collections::BTreeSet;

    fn repo_path(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    #[test]
    fn write_amp_order_rotates_by_combination_index() {
        assert_eq!(write_amp_order(0), WRITE_AMP_ENGINES);
        assert_eq!(
            write_amp_order(1),
            [
                Engine::HandWrittenTrigger,
                Engine::Ivmlite,
                Engine::NoMaintenance
            ]
        );
        assert_eq!(write_amp_order(3), WRITE_AMP_ENGINES);
        assert_eq!(write_amp_order(7), write_amp_order(1));
    }

    #[test]
    fn execute_op_aborts_when_a_delete_or_update_misses() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;",
        )
        .unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();

        let err = execute_op(&mut stmts, &WriteOp::Delete { id: 42 }).unwrap_err();
        assert!(err.contains("changed 0 rows"), "{err}");

        let err = execute_op(
            &mut stmts,
            &WriteOp::Update {
                id: 42,
                region: "r0".into(),
                amount: 1,
            },
        )
        .unwrap_err();
        assert!(err.contains("changed 0 rows"), "{err}");
    }

    #[test]
    fn execute_op_accepts_a_matching_insert_delete_and_update() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;",
        )
        .unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();

        execute_op(
            &mut stmts,
            &WriteOp::Insert {
                id: 1,
                email: "e1@x".into(),
                handle: "h1".into(),
                region: "r0".into(),
                amount: 5,
            },
        )
        .unwrap();
        execute_op(
            &mut stmts,
            &WriteOp::Update {
                id: 1,
                region: "r1".into(),
                amount: 6,
            },
        )
        .unwrap();
        execute_op(&mut stmts, &WriteOp::Delete { id: 1 }).unwrap();
    }

    /// `apply_trace` must actually call `check_replace_net_count` for a
    /// replace trace, not merely offer a function that would catch a
    /// mismatch if called: this test runs a real `Replace` op through
    /// `apply_trace` whose claimed `expect_removed` is wrong (the fresh id
    /// and fresh email/handle create no conflict at all, so nothing is
    /// actually removed, but the op claims one row was), and asserts the
    /// wiring — not just the isolated check function — surfaces the error.
    #[test]
    fn apply_trace_aborts_when_a_replace_traces_net_row_count_is_wrong() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO accounts VALUES (1, 'e1@x', 'h1', 'r0', 1);
             INSERT INTO accounts VALUES (2, 'e2@x', 'h2', 'r0', 1);",
        )
        .unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();
        let ops = vec![WriteOp::Replace {
            id: 3,
            email: "n3@x".into(),
            handle: "m3".into(),
            region: "r0".into(),
            amount: 1,
            // Wrong on purpose: id 3, email "n3@x" and handle "m3" conflict
            // with nothing, so this REPLACE actually removes 0 rows.
            expect_removed: 1,
        }];
        let err = apply_trace(&conn, &mut stmts, "accounts", &ops, 10, 2, true).unwrap_err();
        assert!(err.contains("expected 2"), "{err}");
    }

    #[test]
    fn check_replace_net_count_aborts_on_mismatch() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO accounts VALUES (1, 'e1@x', 'h1', 'r0', 1);",
        )
        .unwrap();
        assert!(check_replace_net_count(&conn, "accounts", 1).is_ok());
        let err = check_replace_net_count(&conn, "accounts", 2).unwrap_err();
        assert!(err.contains("expected 2"), "{err}");
    }

    #[test]
    fn expected_replace_total_sums_each_ops_own_expect_removed() {
        let ops = vec![
            WriteOp::Replace {
                id: 10,
                email: "e0@x".into(),
                handle: "m10".into(),
                region: "r0".into(),
                amount: 1,
                expect_removed: 1,
            },
            WriteOp::Replace {
                id: 11,
                email: "e2@x".into(),
                handle: "h3".into(),
                region: "r0".into(),
                amount: 1,
                expect_removed: 2,
            },
        ];
        // base=100, removed=1+2=3, M=2 ops -> 100-3+2=99.
        assert_eq!(expected_replace_total(100, &ops), 99);
    }

    /// The direct wiring test for "the recursive_triggers mode actually
    /// being applied" (ruling 3): `set_recursive_triggers` must set the
    /// connection's pragma to exactly the requested mode, read back through
    /// `PRAGMA recursive_triggers` itself rather than inferred from a
    /// trigger's behavior.
    #[test]
    fn set_recursive_triggers_sets_the_pragma_to_the_requested_mode() {
        let conn = Connection::open_in_memory().unwrap();
        let read_back = |c: &Connection| -> i64 {
            c.query_row("PRAGMA recursive_triggers", [], |r| r.get(0))
                .unwrap()
        };

        set_recursive_triggers(&conn, true).unwrap();
        assert_eq!(read_back(&conn), 1);

        set_recursive_triggers(&conn, false).unwrap();
        assert_eq!(read_back(&conn), 0);
    }

    /// The behavior the brief's comment describes: with `recursive_triggers`
    /// OFF, a hand-written `AFTER DELETE` trigger does not fire for a row
    /// removed by REPLACE's own conflict resolution, so the summary table
    /// disagrees with the oracle after a REPLACE that reuses an existing
    /// UNIQUE key; with the pragma ON, it fires and the two agree. This is
    /// the direct test for "the recursive_triggers mode actually being
    /// applied" (ruling 3): if the runner forgot to set the pragma (or set
    /// it backwards), one half of this test would fail.
    #[test]
    fn recursive_triggers_pragma_controls_whether_the_hand_written_trigger_sees_a_replace_deletion()
    {
        const SETUP: &str =
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO accounts VALUES (1, 'e1@x', 'h1', 'r0', 50);
             INSERT INTO accounts VALUES (2, 'e2@x', 'h2', 'r0', 50);";
        let view = ViewSpec {
            id: 0,
            threshold: 0,
        };
        const REPLACE_SQL: &str =
            "INSERT OR REPLACE INTO accounts(id, email, handle, region, amount) \
             VALUES (3, 'e1@x', 'h3', 'r0', 999)";

        let off = Connection::open_in_memory().unwrap();
        off.execute_batch(SETUP).unwrap();
        crate::baseline::install_trigger_view_with_update(&off, "accounts", &view).unwrap();
        off.execute_batch("PRAGMA recursive_triggers = 0").unwrap();
        off.execute(REPLACE_SQL, []).unwrap();
        assert!(
            verify_view(&off, "accounts", &view, &view.table()).is_err(),
            "with recursive_triggers OFF, the hand-written trigger must miss the REPLACE deletion"
        );

        let on = Connection::open_in_memory().unwrap();
        on.execute_batch(SETUP).unwrap();
        crate::baseline::install_trigger_view_with_update(&on, "accounts", &view).unwrap();
        on.execute_batch("PRAGMA recursive_triggers = 1").unwrap();
        on.execute(REPLACE_SQL, []).unwrap();
        verify_view(&on, "accounts", &view, &view.table()).expect(
            "with recursive_triggers ON, the hand-written trigger must see the REPLACE deletion",
        );
    }

    /// Spec §9-style coverage for Task 5's own brief: a tiny workload (300
    /// base rows, 20 rows per op, views [0, 2], both `recursive_triggers`
    /// modes) run for every op and every engine against the debug
    /// extension. Every per-op effect check and every `ivmlite` view
    /// verification happens inside `run_write_amp` itself; if any of them
    /// failed, this `unwrap_or_else` would panic the test.
    #[test]
    fn write_amp_runs_a_tiny_workload_for_every_engine_op_and_mode() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let mut w = WriteAmpWorkload::load(&repo_path("workloads/write-amp.toml"))
            .unwrap()
            .with_sizes(300, 20);
        w.view_counts = vec![0, 2];

        let rows = run_write_amp(&w, &lib).unwrap_or_else(|e| panic!("{e}"));

        let want_len = w.view_counts.len() * w.ops.len() * w.recursive_triggers.len() * 3;
        assert_eq!(rows.len(), want_len);
        for r in &rows {
            assert!(r.apply_ms >= 0.0);
            assert_eq!(r.rows, 20);
        }

        // Every (engine, views, op, recursive_triggers) combination is
        // represented exactly once — the rotation reorders engines within a
        // combination, it must not drop or duplicate one.
        let keys: BTreeSet<(&'static str, usize, &'static str, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.engine.label(),
                    r.views,
                    r.op.label(),
                    r.recursive_triggers,
                )
            })
            .collect();
        assert_eq!(keys.len(), want_len);
    }
}
