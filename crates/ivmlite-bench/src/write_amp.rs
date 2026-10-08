//! M1b Phase 4 Task 5 (spec §7): the write-amplification workload's own
//! runner. `write-amp.toml`'s `accounts` table has two ordinary UNIQUE keys
//! besides its `INTEGER PRIMARY KEY`, so it can measure what §3's matrix
//! cannot: the cost of REPLACE's UNIQUE-candidate lookup, and of an UPDATE
//! trigger, under both `recursive_triggers` modes.
//!
//! The three engines here are a different, fixed list from `engine::ENGINES`
//! (spec §7: "no_maintenance, hand_written_trigger ... and ivmlite" —
//! `naive_recompute` plays no part in a write-amplification measurement,
//! since nothing here re-runs a view's SQL during the timed apply). The space
//! samplers, the `ivmlite` bootstrap and the refresh statements are shared
//! with the matrix runner in `engine.rs`.

use std::path::Path;
use std::time::Instant;

use ivmlite_workload::{ViewSpec, WriteAmpWorkload, WriteOp, WriteOpKind};
use rusqlite::{Connection, Statement};

use crate::baseline::assert_not_reprepared;
use crate::engine::{
    create_ivmlite_views, page_size, prepare_refresh_statements, rotate_left, space, verify_view,
    Engine,
};

/// The three engines a write-amp cell measures (spec §7).
const WRITE_AMP_ENGINES: [Engine; 3] = [
    Engine::NoMaintenance,
    Engine::HandWrittenTrigger,
    Engine::Ivmlite,
];

/// `WRITE_AMP_ENGINES` rotated left by the combination index — the index of
/// the (`views`, op, `recursive_triggers`) combination, not of the engine
/// itself (spec §3.3's rotation rule, through the shared `rotate_left`).
fn write_amp_order(combo_index: usize) -> [Engine; 3] {
    let v = rotate_left(&WRITE_AMP_ENGINES, combo_index);
    [v[0], v[1], v[2]]
}

/// One row this module's runner produces; `main.rs` formats and prints it.
pub struct WriteAmpRow {
    pub engine: Engine,
    pub views: usize,
    pub op: WriteOpKind,
    /// The connection's `PRAGMA recursive_triggers`, read back after the
    /// timed apply (spec §7's modes), not merely the mode that was asked for:
    /// `run_one` refuses to produce a row whose read-back differs from the
    /// requested mode.
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

    /// None of the four timed statements may have been recompiled during
    /// the timed apply (spec §3.2 step 4) — see
    /// `baseline::assert_not_reprepared` for why a flag `PRAGMA` set after
    /// `prepare` would cause exactly that.
    fn assert_not_reprepared(&self) -> Result<(), String> {
        for (label, s) in [
            ("insert", &self.insert),
            ("delete", &self.delete),
            ("update", &self.update),
            ("replace", &self.replace),
        ] {
            assert_not_reprepared(label, s)?;
        }
        Ok(())
    }
}

/// Execute one op and check its per-row effect (spec §7's "verified per row"
/// column): `Insert`, `Delete` and `Update` must each change exactly one
/// row. A `Replace` is not checked here — `changes()` after `INSERT OR
/// REPLACE` does not report the conflicting deletes as rows this statement
/// itself changed — but by `check_replace_effects` and
/// `check_replace_net_count`, after the timer.
///
/// A miss aborts the whole run (spec §3.2 step 7): the caller (`apply_trace`,
/// then `run_one`, then `run_write_amp`) propagates this `Err` all the way
/// out, so a wrong effect never reaches the CSV.
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

/// The net row count a replace trace must leave behind (spec §7): the base
/// rows, minus every op's `targets`, plus one new row per op. Summed from each op's own
/// claimed `targets`, rather than hard-coded per kind, so the check stays
/// correct even if a kind's targets ever changed.
fn expected_replace_total(base_rows: usize, ops: &[WriteOp]) -> i64 {
    let removed: usize = ops
        .iter()
        .map(|op| match op {
            WriteOp::Replace { targets, .. } => targets.len(),
            _ => 0,
        })
        .sum();
    base_rows as i64 - removed as i64 + ops.len() as i64
}

/// Check the net row count for a replace trace (spec §7): a miss aborts the
/// run, exactly like a per-op effect miss. On its own this is a whole-trace
/// check — opposing per-op errors could cancel in the total — so
/// `check_replace_effects` checks every op as well.
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

/// Check every REPLACE of a trace on its own (spec §7), untimed, after the
/// trace: the row it wrote is present with exactly its values, and every
/// existing row it claims to conflict with (`targets`, other than its own
/// id) is gone.
///
/// Together with `check_replace_net_count` this makes the check per-op
/// exact: `trace()` keeps every op's targets and new ids disjoint, so if
/// every claimed target is gone and every new row is present, a net count
/// equal to `base − Σ |targets| + M` leaves no room for an op that removed
/// a row it did not claim.
fn check_replace_effects(conn: &Connection, table: &str, ops: &[WriteOp]) -> Result<(), String> {
    let mut read_row = conn
        .prepare(&format!(
            "SELECT email, handle, region, amount FROM \"{table}\" WHERE id = ?1"
        ))
        .map_err(|e| e.to_string())?;
    for op in ops {
        let WriteOp::Replace {
            id,
            email,
            handle,
            region,
            amount,
            targets,
        } = op
        else {
            continue;
        };
        let want = (email.clone(), handle.clone(), region.clone(), *amount);
        let got: Vec<(String, String, String, i64)> = read_row
            .query_map((id,), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .and_then(|rows| rows.collect())
            .map_err(|e| e.to_string())?;
        if got != [want] {
            return Err(format!(
                "{op:?}: the row it wrote is not in \"{table}\" as written (found {got:?})"
            ));
        }
        for target in targets.iter().filter(|t| *t != id) {
            let left: Vec<(String, String, String, i64)> = read_row
                .query_map((target,), |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .and_then(|rows| rows.collect())
                .map_err(|e| e.to_string())?;
            if !left.is_empty() {
                return Err(format!(
                    "{op:?}: its conflicting row {target} is still in \"{table}\""
                ));
            }
        }
    }
    Ok(())
}

/// Run the trace in transactions of `tx_rows`, timed as one region (spec
/// §7's `apply_ms`). Every `Insert`/`Delete`/`Update` op is checked as it
/// runs, inside the timed region; right after the timer stops, the four
/// statements are checked for an unwanted recompilation (spec §3.2 step 4),
/// and a replace trace is checked per op and by its net count (spec §7).
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
    for chunk in ops.chunks(tx_rows) {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        for op in chunk {
            execute_op(stmts, op)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    let apply_ms = start.elapsed().as_secs_f64() * 1000.0;

    stmts.assert_not_reprepared()?;
    if is_replace {
        check_replace_net_count(conn, table, expected_replace_total(base_rows, ops))?;
        check_replace_effects(conn, table, ops)?;
    }
    Ok(apply_ms)
}

/// Set the connection's `recursive_triggers` pragma to `mode` (spec §7's
/// modes). Factored out of `run_one` so the helper's own contract is
/// directly testable; `run_one` separately reads the pragma back after the
/// timed apply (`read_recursive_triggers`), so a call site that passed the
/// wrong mode, or none, cannot produce a row.
fn set_recursive_triggers(conn: &Connection, mode: bool) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA recursive_triggers = {}",
        if mode { 1 } else { 0 }
    ))
}

/// The connection's current `PRAGMA recursive_triggers`. Reading a flag
/// pragma does not expire prepared statements; only setting one does.
fn read_recursive_triggers(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("PRAGMA recursive_triggers", [], |r| r.get::<_, i64>(0))
        .map(|v| v != 0)
}

/// The mode a cell actually ran under, read back after its timed apply, must
/// be the mode the workload asked for (spec §7's modes): otherwise every
/// number in that row belongs to the other mode.
fn check_recursive_triggers(conn: &Connection, requested: bool) -> Result<bool, String> {
    let observed = read_recursive_triggers(conn).map_err(|e| e.to_string())?;
    if observed != requested {
        return Err(format!(
            "the connection ran with recursive_triggers = {observed}, but the workload asked for {requested}"
        ));
    }
    Ok(observed)
}

/// Whether `hand_written_trigger`'s view should be checked against the
/// oracle for this cell, after the trace has run (spec §7):
/// insert/delete/update are always checked; a replace op is checked only
/// when `recursive_triggers` is ON, since only then does the engine's
/// `AFTER DELETE` trigger see a REPLACE conflict's implicit delete —
/// confirmed exact in that mode by
/// `recursive_triggers_pragma_controls_whether_the_hand_written_trigger_sees_a_replace_deletion`.
/// With the pragma OFF, a replace op stays cost-only.
fn should_verify_hand_written_trigger(is_replace: bool, recursive_triggers: bool) -> bool {
    !is_replace || recursive_triggers
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

    // ---- open, then set the mode immediately (spec §3.2 step 4) ----
    let conn = if engine == Engine::Ivmlite {
        ivmlite_test::open_with_extension_at(None, lib).map_err(|e| e.to_string())?
    } else {
        Connection::open_in_memory().map_err(|e| e.to_string())?
    };
    // `PRAGMA recursive_triggers` is a flag pragma: SQLite responds to it
    // with `OP_Expire`, which marks every statement currently prepared on
    // this connection as expired, and an expired statement is silently
    // recompiled the next time it runs. Setting the mode here — immediately
    // after opening the connection, before `seed_base`'s own prepared
    // insert, before bootstrap's DDL, and long before `WriteStatements`
    // exists — means nothing prepared later on this connection can ever be
    // expired by it. (It used to be set right before the timed apply,
    // after `WriteStatements::prepare`, which recompiled all four
    // statements on their first use inside the timed region: measured at
    // 3.44 ms for that first execute against a 0.29 ms steady state at 200
    // views.)
    set_recursive_triggers(&conn, recursive_triggers).map_err(|e| e.to_string())?;

    // ---- seed (untimed) ----
    seed_base(&conn, w).map_err(|e| e.to_string())?;
    let page_size_val = page_size(&conn).map_err(|e| e.to_string())?;
    let base = space(&conn).map_err(|e| e.to_string())?;

    // ---- bootstrap the views (untimed: unlike the matrix's spec §3.4,
    // write-amp's CSV has no bootstrap_ms column, so there is nothing to
    // time here and no reason to guard against timing compilation) ----
    match engine {
        Engine::HandWrittenTrigger => {
            for v in views {
                crate::baseline::install_trigger_view_with_update(&conn, table, v)
                    .map_err(|e| e.to_string())?;
            }
        }
        Engine::Ivmlite => create_ivmlite_views(&conn, table, views)?,
        Engine::NoMaintenance => {}
        Engine::NaiveRecompute => {
            unreachable!("write-amp never selects naive_recompute (WRITE_AMP_ENGINES)")
        }
    }
    let bootstrapped = space(&conn).map_err(|e| e.to_string())?;

    // ---- verify the initial state (untimed, spec §3.2 step 3's
    // technique): right after bootstrap, before any op has run, every view
    // must already equal the oracle. Unconditional for both engines that
    // have anything to check — no op has touched the table yet, so `kind`
    // and `recursive_triggers` cannot matter here the way they do for the
    // final verification below.
    match engine {
        Engine::Ivmlite | Engine::HandWrittenTrigger => {
            for v in views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        Engine::NoMaintenance => {}
        Engine::NaiveRecompute => {
            unreachable!("write-amp never selects naive_recompute (WRITE_AMP_ENGINES)")
        }
    }

    // ---- prepare every statement (the mode was already set, above) ----
    let mut stmts = WriteStatements::prepare(&conn, table).map_err(|e| e.to_string())?;
    let mut refresh_stmts = if engine == Engine::Ivmlite {
        prepare_refresh_statements(&conn, views)?
    } else {
        Vec::new()
    };

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
    let observed_recursive_triggers = check_recursive_triggers(&conn, recursive_triggers)?;

    // ---- untimed: refresh, sample space, then verify against the oracle
    // (space is sampled before verification, as engine::run_cell does) ----
    for stmt in &mut refresh_stmts {
        stmt.execute([]).map_err(|e| e.to_string())?;
    }
    let maintained = space(&conn).map_err(|e| e.to_string())?;
    match engine {
        Engine::Ivmlite => {
            for v in views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        Engine::HandWrittenTrigger
            if should_verify_hand_written_trigger(is_replace, recursive_triggers) =>
        {
            for v in views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        // A replace op's implicit conflict-resolution deletes are only ever
        // visible to a hand-written AFTER DELETE trigger when
        // recursive_triggers is ON (see `should_verify_hand_written_trigger`);
        // with the pragma OFF, a replace op's numbers stay cost-only, so its
        // view is never compared with the oracle in that mode.
        Engine::HandWrittenTrigger => {}
        Engine::NoMaintenance => {}
        Engine::NaiveRecompute => {
            unreachable!("write-amp never selects naive_recompute (WRITE_AMP_ENGINES)")
        }
    }

    Ok(WriteAmpRow {
        engine,
        views: views.len(),
        op: kind,
        recursive_triggers: observed_recursive_triggers,
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

    const ACCOUNTS_DDL: &str =
        "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
         handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;";

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

    /// Spec §3.2 step 4: the four `WriteStatements` must not be recompiled
    /// during `apply_trace`'s timed region. This reproduces
    /// `run_one`'s real ordering for `hand_written_trigger` — set the
    /// pragma, *then* create the table and its trigger (a schema change),
    /// *then* prepare — since that bootstrap is the one schema change in
    /// the whole sequence and the case the reviewer's measurement was about.
    /// If the pragma were set after `prepare` instead (as it originally
    /// was), every statement's first `execute` after that would recompile
    /// it, and `StatementStatus::RePrepare` would read nonzero.
    #[test]
    fn apply_trace_does_not_reprepare_the_timed_statements() {
        let conn = Connection::open_in_memory().unwrap();
        set_recursive_triggers(&conn, true).unwrap();
        conn.execute_batch(ACCOUNTS_DDL).unwrap();
        let view = ViewSpec {
            id: 0,
            threshold: 0,
        };
        crate::baseline::install_trigger_view_with_update(&conn, "accounts", &view).unwrap();

        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();
        let ops = vec![
            WriteOp::Insert {
                id: 1,
                email: "e1@x".into(),
                handle: "h1".into(),
                region: "r0".into(),
                amount: 1,
            },
            WriteOp::Update {
                id: 1,
                region: "r1".into(),
                amount: 2,
            },
            WriteOp::Delete { id: 1 },
            WriteOp::Replace {
                id: 2,
                email: "n2@x".into(),
                handle: "m2".into(),
                region: "r0".into(),
                amount: 1,
                targets: vec![],
            },
        ];
        apply_trace(&conn, &mut stmts, "accounts", &ops, 10, 0, false)
            .expect("no statement should be recompiled during the timed apply");
    }

    /// The negative half: `WriteStatements::assert_not_reprepared` must
    /// actually detect a recompilation when one happens, not just report
    /// zero because nothing ever recompiles in practice. A flag pragma
    /// issued *after* `prepare` (the bug the call order in `run_one`
    /// prevents, reproduced directly here rather than by reverting
    /// `run_one`) expires the statement, and its first `execute` recompiles
    /// it.
    #[test]
    fn assert_not_reprepared_catches_a_statement_expired_after_prepare() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ACCOUNTS_DDL).unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();
        // A flag pragma set only now, after prepare: exactly the bug the
        // call order in `run_one` prevents (the pragma used to run here).
        set_recursive_triggers(&conn, true).unwrap();
        execute_op(
            &mut stmts,
            &WriteOp::Insert {
                id: 1,
                email: "e1@x".into(),
                handle: "h1".into(),
                region: "r0".into(),
                amount: 1,
            },
        )
        .unwrap();

        let err = stmts.assert_not_reprepared().unwrap_err();
        assert!(err.contains("insert"), "{err}");
        assert!(err.contains("recompiled"), "{err}");
    }

    /// Spec §7: `hand_written_trigger`'s view is checked for every op except
    /// a replace op with `recursive_triggers` OFF.
    #[test]
    fn should_verify_hand_written_trigger_only_skips_a_replace_op_with_the_pragma_off() {
        assert!(should_verify_hand_written_trigger(false, false));
        assert!(should_verify_hand_written_trigger(false, true));
        assert!(!should_verify_hand_written_trigger(true, false));
        assert!(should_verify_hand_written_trigger(true, true));
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
    /// `apply_trace` whose claimed `targets` are wrong (the fresh id
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
            targets: vec![1],
        }];
        let err = apply_trace(&conn, &mut stmts, "accounts", &ops, 10, 2, true).unwrap_err();
        assert!(err.contains("expected 2"), "{err}");
    }

    /// Spec §7, per op: two REPLACEs whose count errors cancel. The first
    /// claims to replace row 1 but conflicts with nothing (0 removed, 1
    /// claimed); the second claims only row 2 but its handle also hits row 3
    /// (2 removed, 1 claimed). The net count balances (4 − 2 + 2 = 4), so
    /// only the per-op check can see that row 1 was never removed.
    #[test]
    fn apply_trace_aborts_when_a_replace_misses_its_target_even_if_the_net_count_balances() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO accounts VALUES (1, 'e1@x', 'h1', 'r0', 1);
             INSERT INTO accounts VALUES (2, 'e2@x', 'h2', 'r0', 1);
             INSERT INTO accounts VALUES (3, 'e3@x', 'h3', 'r0', 1);
             INSERT INTO accounts VALUES (4, 'e4@x', 'h4', 'r0', 1);",
        )
        .unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();
        let ops = vec![
            WriteOp::Replace {
                id: 10,
                email: "n10@x".into(),
                handle: "m10".into(),
                region: "r0".into(),
                amount: 1,
                targets: vec![1],
            },
            WriteOp::Replace {
                id: 11,
                email: "e2@x".into(),
                handle: "h3".into(),
                region: "r0".into(),
                amount: 1,
                targets: vec![2],
            },
        ];
        assert_eq!(expected_replace_total(4, &ops), 4, "the net count balances");
        let err = apply_trace(&conn, &mut stmts, "accounts", &ops, 10, 4, true).unwrap_err();
        assert!(
            err.contains("conflicting row 1 is still in"),
            "the per-op check must name the target that was not removed: {err}"
        );
    }

    /// The per-op check accepts a trace whose every op did exactly what it
    /// claims, including a `replace_rowid`-shaped op whose target is its
    /// own id.
    #[test]
    fn check_replace_effects_accepts_ops_that_did_what_they_claim() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, \
             handle TEXT NOT NULL UNIQUE, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO accounts VALUES (1, 'e1@x', 'h1', 'r0', 1);
             INSERT INTO accounts VALUES (2, 'e2@x', 'h2', 'r0', 1);
             INSERT INTO accounts VALUES (3, 'e3@x', 'h3', 'r0', 1);",
        )
        .unwrap();
        let mut stmts = WriteStatements::prepare(&conn, "accounts").unwrap();
        let ops = vec![
            WriteOp::Replace {
                id: 1,
                email: "n1@x".into(),
                handle: "m1".into(),
                region: "r1".into(),
                amount: 7,
                targets: vec![1],
            },
            WriteOp::Replace {
                id: 10,
                email: "e2@x".into(),
                handle: "h3".into(),
                region: "r2".into(),
                amount: 8,
                targets: vec![2, 3],
            },
        ];
        apply_trace(&conn, &mut stmts, "accounts", &ops, 10, 3, true)
            .expect("every op removed exactly its targets and wrote its row");
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
    fn expected_replace_total_sums_each_ops_own_targets() {
        let ops = vec![
            WriteOp::Replace {
                id: 10,
                email: "e0@x".into(),
                handle: "m10".into(),
                region: "r0".into(),
                amount: 1,
                targets: vec![0],
            },
            WriteOp::Replace {
                id: 11,
                email: "e2@x".into(),
                handle: "h3".into(),
                region: "r0".into(),
                amount: 1,
                targets: vec![2, 3],
            },
        ];
        // base=100, removed=1+2=3, M=2 ops -> 100-3+2=99.
        assert_eq!(expected_replace_total(100, &ops), 99);
    }

    /// The helper's own contract (spec §7's modes): `set_recursive_triggers`
    /// must set the connection's pragma to exactly the requested mode, read
    /// back through `PRAGMA recursive_triggers` itself rather than inferred
    /// from a trigger's behavior. Whether `run_one` passes the right mode is
    /// a separate question, answered by `check_recursive_triggers`.
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

    /// SQLite's own behavior, which spec §7's "cost only" rule rests on:
    /// with `recursive_triggers` OFF, a hand-written `AFTER DELETE` trigger
    /// does not fire for a row removed by REPLACE's own conflict
    /// resolution, so the summary table disagrees with the oracle after a
    /// REPLACE that reuses an existing UNIQUE key; with the pragma ON, it
    /// fires and the two agree. This test sets the pragma itself, so it says
    /// nothing about whether the runner does: `run_one`'s read-back
    /// (`check_recursive_triggers`) covers that.
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

    /// The read-back `run_one` makes after the timed apply: a connection
    /// whose pragma differs from the requested mode is refused, naming both
    /// values, and a matching one reports the observed mode.
    #[test]
    fn check_recursive_triggers_refuses_a_connection_in_the_other_mode() {
        let conn = Connection::open_in_memory().unwrap();
        set_recursive_triggers(&conn, true).unwrap();
        assert_eq!(check_recursive_triggers(&conn, true), Ok(true));
        let err = check_recursive_triggers(&conn, false).unwrap_err();
        assert!(err.contains("recursive_triggers = true"), "{err}");
        assert!(err.contains("asked for false"), "{err}");

        set_recursive_triggers(&conn, false).unwrap();
        assert_eq!(check_recursive_triggers(&conn, false), Ok(false));
        assert!(check_recursive_triggers(&conn, true).is_err());
    }

    /// Spec §9-style coverage of the write-amp runner: a tiny workload (300
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
