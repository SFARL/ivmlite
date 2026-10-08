//! Task 2: ablation mode (spec §6). Runs a fixed set of representative cells
//! (`Workload::ablation`, not the derived matrix) for the `naive_recompute`
//! and `ivmlite` engines only, each repeated `spec.repeats` times with a
//! rotated two-engine order — the same protocol `run_cell` already enforces
//! per cell (spec §3.2), just over a fixed cell list instead of the matrix.
//!
//! `scripts/bench-ablation.sh` runs this mode against a separate build of the
//! extension at each of several commits, so the README's before/after table
//! can compare medians and min-max per build (spec §6).

use std::path::Path;

use ivmlite_workload::{AblationSpec, Workload};

use crate::engine::{rotate_left, run_cell, Engine, Measurement};

/// The two engines the ablation compares (spec §6: "the `ivmlite` and
/// `naive_recompute` engines"). Spec §3.3: `rotate_left` (Task 1)
/// is generic and reused here for this two-engine list, exactly as
/// `engine_order` reuses it for the four-engine matrix list.
const ABLATION_ENGINES: [Engine; 2] = [Engine::NaiveRecompute, Engine::Ivmlite];

/// `ABLATION_ENGINES` rotated left by `repeat % 2`, so which engine runs
/// first alternates across repetitions instead of always warming the same
/// one first.
fn ablation_order(repeat: usize) -> [Engine; 2] {
    let v = rotate_left(&ABLATION_ENGINES, repeat);
    [v[0], v[1]]
}

/// One ablation row: the cell's four dimensions, which repetition it was, and
/// the `Measurement` `run_cell` produced for it.
pub struct AblationRow {
    pub repeat: usize,
    pub engine: Engine,
    pub views: usize,
    pub base_rows: usize,
    pub batch_size: usize,
    pub group_cardinality: usize,
    pub m: Measurement,
}

/// Run every cell of `spec.cells`, each `spec.repeats` times, over
/// `naive_recompute` and `ivmlite` only (spec §6). `base` supplies the schema
/// and update-trace parameters an ablation cell does not repeat itself
/// (`Workload::with_cell` fills in the rest, exactly as `cells()` does for
/// the matrix — spec §8: the two can never disagree).
pub fn run_ablation(
    base: &Workload,
    spec: &AblationSpec,
    lib: &Path,
) -> Result<Vec<AblationRow>, String> {
    let mut rows = Vec::new();
    for cell in &spec.cells {
        let w = base.with_cell(
            cell.base_rows,
            cell.group_cardinality,
            cell.views,
            cell.batch_size,
        );
        for r in 0..spec.repeats {
            for engine in ablation_order(r) {
                let m = run_cell(engine, &w, lib).map_err(|e| {
                    format!(
                        "ablation cell (views={} base_rows={} batch_size={} group_cardinality={}) repeat {r} engine {}: {e}",
                        cell.views, cell.base_rows, cell.batch_size, cell.group_cardinality, engine.label(),
                    )
                })?;
                rows.push(AblationRow {
                    repeat: r,
                    engine,
                    views: cell.views,
                    base_rows: cell.base_rows,
                    batch_size: cell.batch_size,
                    group_cardinality: cell.group_cardinality,
                    m,
                });
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ablation_order_rotates_the_two_engines_by_repeat() {
        assert_eq!(ablation_order(0), [Engine::NaiveRecompute, Engine::Ivmlite]);
        assert_eq!(ablation_order(1), [Engine::Ivmlite, Engine::NaiveRecompute]);
        assert_eq!(ablation_order(2), [Engine::NaiveRecompute, Engine::Ivmlite]);
        assert_eq!(ablation_order(7), ablation_order(1));
    }
}
