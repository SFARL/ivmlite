//! `ivmlite` as a fourth engine under Phase 4's measurement protocol
//! (spec §3): the same `Workload` cell the M0 baselines run, driven through
//! the real SQLite extension instead of hand-written SQL.

use std::path::Path;
use std::time::Instant;

use ivmlite_test::demo_bench::{median_of_sums, Summary};
use ivmlite_workload::{ViewSpec, Workload, MEASURED_ROUNDS};
use rusqlite::{Connection, Statement};

use crate::baseline::{
    apply, assert_not_reprepared, install_trigger_view, recompute_all, seed_base, ApplyStatements,
    RecomputeStatements,
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

/// Rotate `items` left by `i % items.len()`. Not specific to `Engine`: the
/// ablation (spec §6) and write-amp (spec §7) runners reuse it for their own
/// engine lists, so there is one rotation rule (spec §3.3).
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

/// One measured round's timed steps, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RoundTiming {
    pub apply_ms: f64,
    pub maintain_ms: f64,
}

/// A cell's steady state over its measured rounds (M1b Phase 6 spec §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SteadyState {
    pub apply_ms: f64,
    pub maintain_ms: f64,
    pub apply_plus_maintain_ms: f64,
}

/// Spec §6's median rule, the demo runner's (`demo_bench::steady_state`):
/// each step's own median over the rounds, and for the total the median of
/// each round's `apply + maintain`, so the total is what one round cost and
/// not a sum of medians taken from different rounds. An even count's median
/// is the mean of the two middle values. `None` for no rounds.
pub(crate) fn steady_state(rounds: &[RoundTiming]) -> Option<SteadyState> {
    let median = |step: fn(&RoundTiming) -> f64| {
        let values: Vec<f64> = rounds.iter().map(step).collect();
        Summary::of(&values).map(|summary| summary.median)
    };
    let parts: Vec<(f64, f64)> = rounds.iter().map(|r| (r.apply_ms, r.maintain_ms)).collect();
    Some(SteadyState {
        apply_ms: median(|r| r.apply_ms)?,
        maintain_ms: median(|r| r.maintain_ms)?,
        apply_plus_maintain_ms: median_of_sums(&parts)?,
    })
}

/// One space sample: `PRAGMA page_count` and `PRAGMA freelist_count` (spec §3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Space {
    pub page_count: i64,
    pub freelist_count: i64,
}

/// Everything one engine's run of one cell measures (spec §3.2, §3.4). The
/// timings after `bootstrap_ms` follow M1b Phase 6 spec §6's steady state.
#[derive(Debug, Clone)]
pub struct Measurement {
    pub engine: Engine,
    pub bootstrap_ms: f64,
    /// The maintenance of the warm-up batch: for ivmlite, the first refresh
    /// after `CREATE`. Exactly 0 for `no_maintenance` and
    /// `hand_written_trigger`, which maintain nothing on refresh; a trigger's
    /// maintenance runs inside the writes, in `apply_ms`.
    pub first_refresh_ms: f64,
    /// The median apply over the measured rounds.
    pub apply_ms: f64,
    /// The median maintenance over the measured rounds; exactly 0 for the
    /// engines `first_refresh_ms` names.
    pub maintain_ms: f64,
    /// The median over the measured rounds of each round's own
    /// `apply + maintain`, not the sum of the two medians above.
    pub apply_plus_maintain_ms: f64,
    pub page_size: i64,
    pub base: Space,
    pub bootstrapped: Space,
    pub written: Space,
    pub maintained: Space,
}

pub(crate) fn space(conn: &Connection) -> rusqlite::Result<Space> {
    Ok(Space {
        page_count: conn.query_row("PRAGMA page_count", [], |r| r.get(0))?,
        freelist_count: conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?,
    })
}

pub(crate) fn page_size(conn: &Connection) -> rusqlite::Result<i64> {
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

/// Bootstrap `ivmlite`: create one `ivm` virtual table per view, named
/// `view.table()`, over `table` (spec §3.2 step 2). Shared by the matrix
/// (`run_cell`) and the write-amp runner (`write_amp::run_one`).
pub(crate) fn create_ivmlite_views(
    conn: &Connection,
    table: &str,
    views: &[ViewSpec],
) -> Result<(), String> {
    for v in views {
        let sql = v.sql(table).replace('\'', "''");
        let name = v.table();
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE \"{name}\" USING ivm('{sql}')"
        ))
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Prepare one `refresh` statement per view (spec §3.2 step 4: before any
/// timed region). Shared by `run_cell` and `write_amp::run_one`.
pub(crate) fn prepare_refresh_statements<'c>(
    conn: &'c Connection,
    views: &[ViewSpec],
) -> Result<Vec<Statement<'c>>, String> {
    views
        .iter()
        .map(|v| {
            let name = v.table();
            conn.prepare(&format!(
                "INSERT INTO \"{name}\"(\"{name}\") VALUES ('refresh')"
            ))
            .map_err(|e| e.to_string())
        })
        .collect()
}

/// The statements one engine's maintenance step runs, prepared before any
/// timer (spec §3.2 step 4).
enum Maintenance<'c> {
    /// `no_maintenance` and `hand_written_trigger`: nothing runs on refresh.
    Nothing,
    Recompute(RecomputeStatements<'c>),
    Refresh(Vec<Statement<'c>>),
}

impl<'c> Maintenance<'c> {
    fn prepare(engine: Engine, conn: &'c Connection, cell: &Workload) -> Result<Self, String> {
        Ok(match engine {
            Engine::NaiveRecompute => Maintenance::Recompute(
                RecomputeStatements::prepare(conn, cell).map_err(|e| e.to_string())?,
            ),
            Engine::Ivmlite => Maintenance::Refresh(prepare_refresh_statements(conn, &cell.views)?),
            Engine::NoMaintenance | Engine::HandWrittenTrigger => Maintenance::Nothing,
        })
    }

    /// Run the step and return its milliseconds; an engine with nothing to
    /// run reports `0.0` rather than the time of an empty timer. No
    /// statement may have been recompiled (spec §3.2 step 4).
    fn run(&mut self) -> Result<f64, String> {
        match self {
            Maintenance::Nothing => Ok(0.0),
            Maintenance::Recompute(stmts) => recompute_all(stmts),
            Maintenance::Refresh(stmts) => {
                let start = Instant::now();
                for stmt in stmts.iter_mut() {
                    stmt.execute([]).map_err(|e| e.to_string())?;
                }
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                for (i, stmt) in stmts.iter().enumerate() {
                    assert_not_reprepared(&format!("refresh view {i}"), stmt)?;
                }
                Ok(elapsed_ms)
            }
        }
    }
}

/// Check every maintained view against the oracle (spec §3.2 steps 3 and 7).
fn verify(engine: Engine, conn: &Connection, cell: &Workload) -> Result<(), String> {
    let table = &cell.schema.table;
    match engine {
        Engine::HandWrittenTrigger | Engine::Ivmlite => {
            for v in &cell.views {
                verify_view(conn, table, v, &v.table())?;
            }
        }
        Engine::NaiveRecompute => {
            // Sanity check only (spec §3.2 step 7): naive recompute's
            // "maintenance" already ran the oracle query itself, so there is
            // no separate materialized relation to check it against.
            // Re-running the same query and comparing it with itself catches a
            // query that errors or a nondeterministic result set, not a wrong
            // answer — the naive baseline cannot compute a wrong answer by
            // construction.
            for v in &cell.views {
                let sql = v.sql(table);
                let mut a = read_triples(conn, &sql).map_err(|e| e.to_string())?;
                let mut b = read_triples(conn, &sql).map_err(|e| e.to_string())?;
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
    Ok(())
}

/// The fresh in-memory database one engine's run of a cell uses. `lib` is
/// the extension library, loaded only for `Engine::Ivmlite`.
fn open(engine: Engine, lib: &Path) -> Result<Connection, String> {
    if engine == Engine::Ivmlite {
        ivmlite_test::open_with_extension_at(None, lib).map_err(|e| e.to_string())
    } else {
        Connection::open_in_memory().map_err(|e| e.to_string())
    }
}

/// Run spec §3.2's sequence for one engine on one cell, in a fresh in-memory
/// database, with M1b Phase 6 spec §6's steady state: a warm-up batch whose
/// maintenance alone is timed, then `MEASURED_ROUNDS` timed batches.
pub fn run_cell(engine: Engine, cell: &Workload, lib: &Path) -> Result<Measurement, String> {
    let conn = open(engine, lib)?;
    measure(engine, cell, &conn)
}

/// `run_cell` on a connection the caller opened (`open`), so a test can
/// inspect the database afterwards.
fn measure(engine: Engine, cell: &Workload, conn: &Connection) -> Result<Measurement, String> {
    let table = &cell.schema.table;

    // ---- seed (untimed) ----
    seed_base(conn, cell).map_err(|e| e.to_string())?;
    let page_size_val = page_size(conn).map_err(|e| e.to_string())?;
    let base = space(conn).map_err(|e| e.to_string())?;

    // ---- bootstrap (timed), then verify (untimed) ----
    let bootstrap_start = Instant::now();
    match engine {
        Engine::HandWrittenTrigger => {
            for v in &cell.views {
                install_trigger_view(conn, table, v).map_err(|e| e.to_string())?;
            }
        }
        Engine::Ivmlite => create_ivmlite_views(conn, table, &cell.views)?,
        Engine::NoMaintenance | Engine::NaiveRecompute => {}
    }
    let bootstrap_ms = bootstrap_start.elapsed().as_secs_f64() * 1000.0;
    let bootstrapped = space(conn).map_err(|e| e.to_string())?;
    // Spec §3.2 step 3 checks the materialized views; naive recompute's
    // self-comparison (`verify`) is not run at bootstrap, only after the
    // warm-up and after the last batch.
    if matches!(engine, Engine::HandWrittenTrigger | Engine::Ivmlite) {
        verify(engine, conn, cell)?;
    }

    // ---- prepare every timed statement ----
    let mut apply_stmts = ApplyStatements::prepare(conn, table).map_err(|e| e.to_string())?;
    let mut maintenance = Maintenance::prepare(engine, conn, cell)?;

    // ---- the batches: one warm-up, then the measured rounds (spec §6) ----
    let rounds = 1 + MEASURED_ROUNDS;
    cell.check_rounds(rounds).map_err(|e| e.to_string())?;
    let batches = cell.update_rounds(rounds);
    let (warm_up, measured) = batches.split_first().expect("rounds is at least 1");

    // Spec §3.2 step 4: no timed statement may have been recompiled — see
    // `baseline::assert_not_reprepared`.
    apply(conn, &mut apply_stmts, warm_up).map_err(|e| e.to_string())?;
    apply_stmts.assert_not_reprepared()?;
    let first_refresh_ms = maintenance.run()?;
    verify(engine, conn, cell)?;

    let mut timings = Vec::with_capacity(measured.len());
    let mut written = Space::default();
    let mut maintained = Space::default();
    for ops in measured {
        let apply_ms = apply(conn, &mut apply_stmts, ops).map_err(|e| e.to_string())?;
        apply_stmts.assert_not_reprepared()?;
        written = space(conn).map_err(|e| e.to_string())?;
        let maintain_ms = maintenance.run()?;
        maintained = space(conn).map_err(|e| e.to_string())?;
        timings.push(RoundTiming {
            apply_ms,
            maintain_ms,
        });
    }
    verify(engine, conn, cell)?;

    let steady = steady_state(&timings).expect("MEASURED_ROUNDS is positive");
    Ok(Measurement {
        engine,
        bootstrap_ms,
        first_refresh_ms,
        apply_ms: steady.apply_ms,
        maintain_ms: steady.maintain_ms,
        apply_plus_maintain_ms: steady.apply_plus_maintain_ms,
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
        let w = small_cell();
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

    /// A 1,000-row cell with two views, small enough for the workspace suite.
    fn small_cell() -> Workload {
        let mut w = Workload::load(&repo_path("workloads/m0-baseline.toml")).unwrap();
        w.data.base_rows = 1_000;
        w.data.group_cardinality = 20;
        w.updates.batch_size = 30;
        w.views.truncate(2);
        w
    }

    /// M1b Phase 6 spec §6: the warm-up batch's maintenance is reported as
    /// `first_refresh_ms` — real work for the engines that maintain on
    /// refresh, and exactly `0` for one that maintains nothing.
    #[test]
    fn the_warm_up_maintenance_is_reported_as_first_refresh_ms() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let w = small_cell();
        for engine in [Engine::Ivmlite, Engine::NaiveRecompute] {
            let m =
                run_cell(engine, &w, &lib).unwrap_or_else(|e| panic!("{}: {e}", engine.label()));
            assert!(m.first_refresh_ms > 0.0, "{}: {m:?}", engine.label());
        }
        let m = run_cell(Engine::NoMaintenance, &w, &lib).unwrap();
        assert_eq!(m.first_refresh_ms, 0.0, "{m:?}");
    }

    /// M1b Phase 6 spec §6: the cell runner applies the warm-up batch and
    /// every measured batch, each once, in order — the base table ends as
    /// the base rows plus the net effect of all `1 + MEASURED_ROUNDS`
    /// rounds — and verifies after the last.
    #[test]
    fn the_cell_runner_applies_the_warm_up_and_every_measured_batch() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let w = small_cell();
        let mut want: std::collections::BTreeSet<i64> = w.rows().map(|(id, _, _)| id).collect();
        for op in w.update_rounds(1 + MEASURED_ROUNDS).iter().flatten() {
            match op {
                ivmlite_workload::TraceOp::Insert { id, .. } => want.insert(*id),
                ivmlite_workload::TraceOp::Delete { id } => want.remove(id),
            };
        }
        for engine in ENGINES {
            let conn = open(engine, &lib).unwrap();
            measure(engine, &w, &conn).unwrap_or_else(|e| panic!("{}: {e}", engine.label()));
            let mut stmt = conn
                .prepare("SELECT id FROM \"orders\" ORDER BY id")
                .unwrap();
            let got: std::collections::BTreeSet<i64> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                got,
                want,
                "{}: the table's ids after the run",
                engine.label()
            );
        }
    }

    /// M1b Phase 6 spec §6: a cell that cannot fit the warm-up plus the
    /// measured batches is refused before anything is timed.
    #[test]
    fn the_cell_runner_refuses_a_cell_that_cannot_fit_its_batches() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let mut w = small_cell();
        w.data.base_rows = 10;
        w.data.group_cardinality = 5;
        w.updates.batch_size = 100;
        let err = run_cell(Engine::NoMaintenance, &w, &lib).unwrap_err();
        assert!(err.contains("base_rows = 10"), "{err}");
    }

    /// M1b Phase 6 spec §6's median rule: each step is its own median over
    /// the rounds, and the total is the median of each round's sum. Here the
    /// two differ: the step medians are 3 and 1, summing to 4, while the
    /// rounds' sums are 6, 3, 4, 5 and 10, whose median is 5.
    #[test]
    fn the_total_is_the_median_of_each_rounds_sum_not_the_sum_of_medians() {
        let rounds: Vec<RoundTiming> = [(1.0, 5.0), (2.0, 1.0), (3.0, 1.0), (4.0, 1.0), (5.0, 5.0)]
            .into_iter()
            .map(|(apply_ms, maintain_ms)| RoundTiming {
                apply_ms,
                maintain_ms,
            })
            .collect();
        assert_eq!(
            steady_state(&rounds),
            Some(SteadyState {
                apply_ms: 3.0,
                maintain_ms: 1.0,
                apply_plus_maintain_ms: 5.0,
            })
        );
        // An even count takes the mean of the two middle values: apply
        // 2, 3, 4, 5 gives 3.5; the sums 3, 4, 5, 10 give 4.5.
        let even = steady_state(&rounds[1..]).unwrap();
        assert_eq!((even.apply_ms, even.apply_plus_maintain_ms), (3.5, 4.5));
        assert_eq!(steady_state(&[]), None);
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

    /// Spec §3.2 step 4: `baseline::assert_not_reprepared` must actually
    /// detect a recompiled refresh statement, not just report zero because
    /// nothing in the matrix currently sets a flag pragma. A pragma issued
    /// after `prepare` expires the refresh statement, and its next
    /// `execute` recompiles it.
    #[test]
    fn refresh_statement_detects_an_intervening_pragma() {
        let lib = ivmlite_test::extension_library_for(ivmlite_test::Profile::Debug)
            .unwrap_or_else(|why| panic!("{why}"));
        let conn = ivmlite_test::open_with_extension_at(None, &lib).unwrap();
        conn.execute_batch(
            "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT;
             CREATE VIRTUAL TABLE mv_0 USING ivm('SELECT region, SUM(amount), COUNT(*) FROM \"orders\" GROUP BY region');",
        )
        .unwrap();
        let mut stmt = conn
            .prepare("INSERT INTO mv_0(mv_0) VALUES ('refresh')")
            .unwrap();
        // A flag pragma set only now, after prepare — exactly the class of
        // bug this check guards against.
        conn.execute_batch("PRAGMA recursive_triggers = 1").unwrap();
        stmt.execute([]).unwrap();

        let err = assert_not_reprepared("refresh", &stmt).unwrap_err();
        assert!(err.contains("refresh"), "{err}");
        assert!(err.contains("recompiled"), "{err}");
    }
}
