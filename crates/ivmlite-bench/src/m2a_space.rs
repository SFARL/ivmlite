//! Separate, untimed replay for allocations at matching maintenance checkpoints.
//! No state scans, oracle queries or page probes occur in performance batches.

use std::path::Path;

use ivmlite_test::demo_bench::check_same_multiset;
use ivmlite_workload::m2a::Fixture;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::m2a_runner::{bootstrap, open, seed, Mode, Prepared, Result, MODES, VIEW};
use crate::m2a_state::{capture_pages, PageMetrics};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpaceCheckpoint {
    pub mode: Mode,
    pub round: Option<usize>,
    pub phase: String,
    pub pages: PageMetrics,
}

fn capture(
    checkpoints: &mut Vec<SpaceCheckpoint>,
    conn: &Connection,
    mode: Mode,
    round: Option<usize>,
    phase: &str,
) -> Result<()> {
    checkpoints.push(SpaceCheckpoint {
        mode,
        round,
        phase: phase.to_owned(),
        pages: capture_pages(conn)?,
    });
    Ok(())
}

/// Each mode starts from a fresh database and replays exactly the exported
/// fixture and ordered batches. All modes receive every checkpoint label,
/// including no-maintenance's unchanged refresh/idle allocations. Its absence
/// of a maintained query result remains explicit in the timing results.
pub(crate) fn replay_space(fixture: &Fixture, library: &Path) -> Result<Vec<SpaceCheckpoint>> {
    fixture.validate()?;
    let mut checkpoints = Vec::new();
    for mode in MODES {
        let conn = open(mode, library)?;
        seed(&conn, fixture)?;
        capture(&mut checkpoints, &conn, mode, None, "base")?;
        bootstrap(&conn, fixture, mode)?;
        capture(&mut checkpoints, &conn, mode, None, "bootstrap")?;
        let mut prepared = Prepared::new(&conn, fixture, mode)?;
        for (round, operations) in fixture.batches.iter().enumerate() {
            prepared.apply(operations)?;
            capture(&mut checkpoints, &conn, mode, Some(round), "written")?;
            prepared.maintain()?;
            capture(&mut checkpoints, &conn, mode, Some(round), "refreshed")?;
            prepared.maintain()?;
            capture(&mut checkpoints, &conn, mode, Some(round), "idle")?;
        }
        // Verify only at the end; correctness and decoded-state evidence come
        // from the dedicated correctness replay. These queries are untimed.
        let actual = match mode {
            Mode::NoMaintenance => None,
            Mode::Recompute | Mode::CoveringRecompute => Some("SELECT * FROM m2a_result".into()),
            Mode::Ivmlite => Some(format!("SELECT * FROM {VIEW}")),
        };
        if let Some(actual) = actual {
            check_same_multiset(&conn, &actual, fixture.cell.view_sql())?;
        }
    }
    Ok(checkpoints)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_workload::m2a::{M2aConfig, Profile, Shape};

    #[test]
    fn space_replay_matches_checkpoint_keys_across_all_modes_and_both_shapes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m2a.toml");
        let mut config = M2aConfig::load(&path).unwrap();
        config.single_rows = 800;
        config.single_groups = 20;
        config.join_rows = 800;
        config.join_groups = 20;
        config.fanouts = vec![8];
        config.batch_sizes = vec![10];
        let library = ivmlite_test::extension_library();
        for shape in [Shape::Single, Shape::Join] {
            let cell = config
                .cells()
                .unwrap()
                .into_iter()
                .find(|cell| cell.shape == shape && cell.profile == Profile::ZipfHot)
                .unwrap();
            let fixture = config.generate(&cell, 42).unwrap();
            let checkpoints = replay_space(&fixture, &library).unwrap();
            assert_eq!(
                checkpoints.len(),
                MODES.len() * (2 + 3 * fixture.batches.len())
            );
            let mut expected_keys = vec![(None, "base"), (None, "bootstrap")];
            for round in 0..fixture.batches.len() {
                for phase in ["written", "refreshed", "idle"] {
                    expected_keys.push((Some(round), phase));
                }
            }
            let mut base_pages = Vec::new();
            for mode in MODES {
                let points: Vec<_> = checkpoints
                    .iter()
                    .filter(|point| point.mode == mode)
                    .collect();
                let keys: Vec<_> = points
                    .iter()
                    .map(|point| (point.round, point.phase.as_str()))
                    .collect();
                assert_eq!(keys, expected_keys);
                base_pages.push(points[0].pages.clone());
                for point in points {
                    assert_eq!(
                        point.pages.allocated_bytes,
                        point.pages.page_size * point.pages.page_count
                    );
                    assert!(point.pages.live_page_bytes <= point.pages.allocated_bytes);
                }
            }
            assert!(base_pages.windows(2).all(|pair| pair[0] == pair[1]));
        }
    }
}
