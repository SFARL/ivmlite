//! `ivmlite` as a fourth engine under Phase 4's measurement protocol
//! (spec §3): the same `Workload` cell the M0 baselines run, driven through
//! the real SQLite extension instead of hand-written SQL.

use std::path::Path;
use std::time::Instant;

use ivmlite_workload::{ViewSpec, Workload};
use rusqlite::Connection;

use crate::baseline::{
    apply, install_trigger_view, recompute_all, seed_base, ApplyStatements, RecomputeStatements,
};

/// The four engines under measurement (spec §3.2): the three M0 baselines,
/// plus the real extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    NoMaintenance,
    HandWrittenTrigger,
    NaiveRecompute,
    Ivmlite,
}

pub const ENGINES: [Engine; 4] = [
    Engine::NoMaintenance,
    Engine::HandWrittenTrigger,
    Engine::NaiveRecompute,
    Engine::Ivmlite,
];

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::NoMaintenance => "no_maintenance",
            Engine::HandWrittenTrigger => "hand_written_trigger",
            Engine::NaiveRecompute => "naive_recompute",
            Engine::Ivmlite => "ivmlite",
        }
    }
}

/// Rotate `items` left by `i % items.len()`. Not specific to `Engine`: Task 5
/// reuses this for a three-engine ablation list (controller ruling 1).
pub(crate) fn rotate_left<T: Copy>(items: &[T], i: usize) -> Vec<T> {
    if items.is_empty() {
        return Vec::new();
    }
    let k = i % items.len();
    items[k..]
        .iter()
        .chain(items[..k].iter())
        .copied()
        .collect()
}

/// `ENGINES` rotated left by `cell_index % 4` (spec §3.3).
pub fn engine_order(cell_index: usize) -> [Engine; 4] {
    let v = rotate_left(&ENGINES, cell_index);
    [v[0], v[1], v[2], v[3]]
}

/// One space sample: `PRAGMA page_count` and `PRAGMA freelist_count` (spec §3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Space {
    pub page_count: i64,
    pub freelist_count: i64,
}

/// Everything one engine's run of one cell measures (spec §3.2, §3.4).
#[derive(Debug, Clone)]
pub struct Measurement {
    pub engine: Engine,
    pub bootstrap_ms: f64,
    pub apply_ms: f64,
    pub maintain_ms: f64,
    pub page_size: i64,
    pub base: Space,
    pub bootstrapped: Space,
    pub written: Space,
    pub maintained: Space,
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

/// One `(region, sum, count)` row, read positionally: the hand-written
/// summary table's columns are `(k, s, c)`, and the view's SQL yields
/// `(region, SUM, COUNT)` — different names for the same three positions
/// (spec §3.2 step 3).
type Triple = (String, i64, i64);

fn read_triples(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<Triple>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    rows.collect()
}

/// Compare `SELECT * FROM relation` with SQLite's own evaluation of
/// `view.sql(table)`, both read as multisets (spec §3.2 steps 3 and 7).
/// `relation` is the maintained view: the ivmlite virtual table, or the
/// hand-written trigger's summary table.
pub(crate) fn verify_view(
    conn: &Connection,
    table: &str,
    view: &ViewSpec,
    relation: &str,
) -> Result<(), String> {
    let mut got = read_triples(conn, &format!("SELECT * FROM \"{relation}\""))
        .map_err(|e| format!("{relation}: reading it failed: {e}"))?;
    let mut want = read_triples(conn, &view.sql(table))
        .map_err(|e| format!("{relation}: evaluating the oracle failed: {e}"))?;
    got.sort();
    want.sort();
    if got != want {
        return Err(format!(
            "{relation} disagrees with the oracle\n  got:  {got:?}\n  want: {want:?}"
        ));
    }
    Ok(())
}

/// Run spec §3.2's sequence for one engine on one cell, in a fresh in-memory
/// database. `lib` is the extension library, used only for `Engine::Ivmlite`.
pub fn run_cell(engine: Engine, cell: &Workload, lib: &Path) -> Result<Measurement, String> {
    let table = &cell.schema.table;

    // ---- step 1: seed (untimed) ----
    let conn = if engine == Engine::Ivmlite {
        ivmlite_test::open_with_extension_at(None, lib).map_err(|e| e.to_string())?
    } else {
        Connection::open_in_memory().map_err(|e| e.to_string())?
    };
    seed_base(&conn, cell).map_err(|e| e.to_string())?;
    let page_size_val = page_size(&conn).map_err(|e| e.to_string())?;
    let base = space(&conn).map_err(|e| e.to_string())?;

    // ---- step 2: bootstrap (timed) ----
    let bootstrap_start = Instant::now();
    match engine {
        Engine::HandWrittenTrigger => {
            for v in &cell.views {
                install_trigger_view(&conn, table, v).map_err(|e| e.to_string())?;
            }
        }
        Engine::Ivmlite => {
            for v in &cell.views {
                let sql = v.sql(table).replace('\'', "''");
                let name = v.table();
                conn.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE \"{name}\" USING ivm('{sql}')"
                ))
                .map_err(|e| e.to_string())?;
            }
        }
        Engine::NoMaintenance | Engine::NaiveRecompute => {}
    }
    let bootstrap_ms = bootstrap_start.elapsed().as_secs_f64() * 1000.0;
    let bootstrapped = space(&conn).map_err(|e| e.to_string())?;

    // ---- step 3: verify the initial state (untimed) ----
    if matches!(engine, Engine::HandWrittenTrigger | Engine::Ivmlite) {
        for v in &cell.views {
            verify_view(&conn, table, v, &v.table())?;
        }
    }

    // ---- step 4: prepare every timed statement ----
    let ops = cell.update_trace();
    let mut apply_stmts = ApplyStatements::prepare(&conn, table).map_err(|e| e.to_string())?;
    let mut recompute_stmts = match engine {
        Engine::NaiveRecompute => {
            Some(RecomputeStatements::prepare(&conn, cell).map_err(|e| e.to_string())?)
        }
        Engine::NoMaintenance | Engine::HandWrittenTrigger | Engine::Ivmlite => None,
    };
    let mut refresh_stmts = Vec::new();
    if engine == Engine::Ivmlite {
        for v in &cell.views {
            let name = v.table();
            refresh_stmts.push(
                conn.prepare(&format!(
                    "INSERT INTO \"{name}\"(\"{name}\") VALUES ('refresh')"
                ))
                .map_err(|e| e.to_string())?,
            );
        }
    }

    // ---- step 5: timed apply ----
    let apply_ms = apply(&conn, &mut apply_stmts, &ops).map_err(|e| e.to_string())?;
    let written = space(&conn).map_err(|e| e.to_string())?;

    // ---- step 6: timed maintain ----
    let maintain_start = Instant::now();
    match engine {
        Engine::NaiveRecompute => {
            let stmts = recompute_stmts
                .as_mut()
                .expect("prepared above for NaiveRecompute");
            recompute_all(stmts).map_err(|e| e.to_string())?;
        }
        Engine::Ivmlite => {
            for stmt in &mut refresh_stmts {
                stmt.execute([]).map_err(|e| e.to_string())?;
            }
        }
        Engine::NoMaintenance | Engine::HandWrittenTrigger => {}
    }
    let maintain_ms = maintain_start.elapsed().as_secs_f64() * 1000.0;
    let maintained = space(&conn).map_err(|e| e.to_string())?;

    // ---- step 7: verify the final state (untimed) ----
    match engine {
        Engine::HandWrittenTrigger | Engine::Ivmlite => {
            for v in &cell.views {
                verify_view(&conn, table, v, &v.table())?;
            }
        }
        Engine::NaiveRecompute => {
            // Sanity check only (spec §3.2 step 7): naive recompute's
            // "maintenance" already ran the oracle query itself in step 6, so
            // there is no separate materialized relation to check it against.
            // Re-running the same query and comparing it with itself catches a
            // query that errors or a nondeterministic result set, not a wrong
            // answer — the naive baseline cannot compute a wrong answer by
            // construction.
            for v in &cell.views {
                let sql = v.sql(table);
                let mut a = read_triples(&conn, &sql).map_err(|e| e.to_string())?;
                let mut b = read_triples(&conn, &sql).map_err(|e| e.to_string())?;
                a.sort();
                b.sort();
                if a != b {
                    return Err(format!(
                        "naive_recompute: view {} is nondeterministic",
                        v.table()
                    ));
                }
            }
        }
        Engine::NoMaintenance => {}
    }

    Ok(Measurement {
        engine,
        bootstrap_ms,
        apply_ms,
        maintain_ms,
        page_size: page_size_val,
        base,
        bootstrapped,
        written,
        maintained,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Join `rel` onto `crates/ivmlite-bench/../..` — the repository root —
    /// so tests can load committed files (workload TOMLs, published CSVs)
    /// without depending on the process's current directory.
    fn repo_path(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    #[test]
    fn engine_order_rotates_by_cell_index() {
        assert_eq!(engine_order(0), ENGINES);
        assert_eq!(
            engine_order(1),
            [
                Engine::HandWrittenTrigger,
                Engine::NaiveRecompute,
                Engine::Ivmlite,
                Engine::NoMaintenance
            ]
        );
        assert_eq!(engine_order(4), ENGINES);
        assert_eq!(engine_order(7), engine_order(3));
    }

    /// Spec §9: a tiny cell per engine against the debug extension, with both
    /// verifications, so the workspace suite exercises the whole protocol.
    #[test]
    fn every_engine_runs_a_tiny_cell_and_verifies_its_views() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let mut w = Workload::load(&repo_path("workloads/m0-baseline.toml")).unwrap();
        w.data.base_rows = 500;
        w.data.group_cardinality = 20;
        w.updates.batch_size = 30;
        w.views.truncate(2);
        for engine in ENGINES {
            let m =
                run_cell(engine, &w, &lib).unwrap_or_else(|e| panic!("{}: {e}", engine.label()));
            assert!(m.apply_ms >= 0.0 && m.maintain_ms >= 0.0 && m.bootstrap_ms >= 0.0);
            assert!(m.page_size > 0 && m.base.page_count > 0);
            if engine == Engine::Ivmlite {
                assert!(
                    m.bootstrapped.page_count > m.base.page_count,
                    "views add pages"
                );
                assert!(m.maintain_ms > 0.0);
            }
        }
    }

    /// A wrong engine must never produce numbers (spec §3.2 step 7).
    #[test]
    fn a_view_that_disagrees_with_the_oracle_aborts_the_cell() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             INSERT INTO orders VALUES (1, 'a', 5);
             CREATE TABLE mv_0(k TEXT NOT NULL PRIMARY KEY, s INTEGER NOT NULL, c INTEGER NOT NULL) STRICT;
             INSERT INTO mv_0 VALUES ('a', 6, 1);",
        )
        .unwrap();
        let view = ivmlite_workload::ViewSpec {
            id: 0,
            threshold: 0,
        };
        let err =
            verify_view(&conn, "orders", &view, "mv_0").expect_err("6 is not SUM(amount) = 5");
        assert!(err.contains("mv_0"), "{err}");
    }
}
