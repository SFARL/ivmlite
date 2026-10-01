//! Task 2: confirmation mode (spec §3.5). The exploration matrix (`matrix`
//! mode) runs once and shows the shape of the surface; on its own it proves
//! no single point. `confirm` reads that CSV, selects the cells worth
//! re-running, and re-runs each `REPEATS` times independently — a fresh
//! database and a rotated engine order each time — so a claim about a
//! crossover, the 2x bar, or beating hand-written triggers can cite a median
//! and a min-max over repeats, not a single sample.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::{engine_order, Engine};

/// K = 5 (spec §3.5): each selected cell is re-run this many times. A
/// property of the measurement protocol, not of any workload file (spec §8).
pub const REPEATS: usize = 5;

/// The engine order for one repetition of one selected cell: `engine_order`
/// (spec §3.3's rotation), offset by both the cell's position among the
/// selected cells (so two different selections do not happen to share a
/// rotation) and the repetition `repeat` (spec §3.5: "a rotated engine order"
/// each time — five repeats of the same cell must not all warm the same
/// engine first). Factored out of `main::run_confirm` so the rotation rule
/// itself — not just its effect once mixed into a whole confirmation run —
/// is directly testable.
pub fn order(cell_index: usize, repeat: usize) -> [Engine; 4] {
    engine_order(cell_index + repeat)
}

/// One exploration row, parsed back from the Task 1 CSV (`main::CSV_HEADER`).
/// Only the fields `select` needs: the cell's four dimensions, the engine
/// label, and the two timed columns. The space columns are discarded.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub engine: String,
    pub views: usize,
    pub base_rows: usize,
    pub batch_size: usize,
    pub group_cardinality: usize,
    pub apply_ms: f64,
    pub maintain_ms: f64,
}

/// The number of comma-separated fields `crate::CSV_HEADER` has.
const FIELD_COUNT: usize = 17;

/// Parse the Task 1 exploration CSV. A minimal hand-written parse, like
/// `main.rs`'s own `read_csv_cell_keys`: the file has no quoting and no
/// embedded commas, so it does not justify a csv dependency. `parse_csv`
/// refuses any header other than `crate::CSV_HEADER` (the same constant
/// `main.rs` prints), since the field positions below are read by index and
/// a changed header would silently misread the columns.
pub fn parse_csv(text: &str) -> Result<Vec<Row>, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty CSV: no header row")?;
    if header != crate::CSV_HEADER {
        return Err(format!(
            "unexpected CSV header (the exploration CSV must be Task 1's own): {header}"
        ));
    }

    lines
        .filter(|l| !l.is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() != FIELD_COUNT {
                return Err(format!(
                    "wrong number of CSV fields ({}, want {FIELD_COUNT}): {line}",
                    f.len()
                ));
            }
            let field = |i: usize, name: &str| -> Result<&str, String> {
                Ok(*f.get(i).ok_or_else(|| format!("missing {name}: {line}"))?)
            };
            let parse = |i: usize, name: &str| -> Result<usize, String> {
                field(i, name)?
                    .parse()
                    .map_err(|e| format!("{name}: {e}: {line}"))
            };
            let parse_f64 = |i: usize, name: &str| -> Result<f64, String> {
                field(i, name)?
                    .parse()
                    .map_err(|e| format!("{name}: {e}: {line}"))
            };
            Ok(Row {
                engine: field(0, "engine")?.to_string(),
                views: parse(1, "views")?,
                base_rows: parse(2, "base_rows")?,
                batch_size: parse(3, "batch_size")?,
                group_cardinality: parse(4, "group_cardinality")?,
                apply_ms: parse_f64(6, "apply_ms")?,
                maintain_ms: parse_f64(7, "maintain_ms")?,
            })
        })
        .collect()
}

/// A cell's four dimensions: `(views, base_rows, batch_size, group_cardinality)`.
pub type CellKey = (usize, usize, usize, usize);

fn key(r: &Row) -> CellKey {
    (r.views, r.base_rows, r.batch_size, r.group_cardinality)
}

/// The timed total a cell's speedup and trigger comparison are computed from:
/// apply plus maintain (spec §3.5 says "ivmlite's apply_ms + maintain_ms").
fn total_ms(r: &Row) -> f64 {
    r.apply_ms + r.maintain_ms
}

/// Spec §3.5: select every cell where
/// - `naive_recompute`'s total over `ivmlite`'s total ("speedup") lies in
///   [0.7, 1.4] (near the 1x crossover) or [1.4, 2.8] (near the 2x
///   falsification bar) — the two ranges share 1.4, so together they are
///   simply [0.7, 2.8];
/// - or `ivmlite`'s total is strictly below `hand_written_trigger`'s (a claim
///   of beating hand-written triggers).
///
/// Returns the cell keys, sorted and deduplicated (a `BTreeSet` gives both for
/// free).
pub fn select(rows: &[Row]) -> Vec<CellKey> {
    let mut naive: BTreeMap<CellKey, f64> = BTreeMap::new();
    let mut ivmlite: BTreeMap<CellKey, f64> = BTreeMap::new();
    let mut triggers: BTreeMap<CellKey, f64> = BTreeMap::new();
    for r in rows {
        let k = key(r);
        let total = total_ms(r);
        match r.engine.as_str() {
            "naive_recompute" => {
                naive.insert(k, total);
            }
            "ivmlite" => {
                ivmlite.insert(k, total);
            }
            "hand_written_trigger" => {
                triggers.insert(k, total);
            }
            _ => {}
        }
    }

    let mut selected: BTreeSet<CellKey> = BTreeSet::new();
    for (&k, &iv) in &ivmlite {
        if let Some(&nv) = naive.get(&k) {
            if iv > 0.0 && (0.7..=2.8).contains(&(nv / iv)) {
                selected.insert(k);
            }
        }
        if let Some(&t) = triggers.get(&k) {
            if iv < t {
                selected.insert(k);
            }
        }
    }
    selected.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(engine: &str, key: CellKey, apply: f64, maintain: f64) -> Row {
        Row {
            engine: engine.into(),
            views: key.0,
            base_rows: key.1,
            batch_size: key.2,
            group_cardinality: key.3,
            apply_ms: apply,
            maintain_ms: maintain,
        }
    }

    #[test]
    fn selection_takes_the_near_1x_and_near_2x_cells_and_every_claim_of_beating_triggers() {
        let near_one = (10, 100_000, 100, 1_000);
        let near_two = (10, 100_000, 1_000, 1_000);
        let far = (10, 1_000_000, 1_000, 10);
        let beats_triggers = (200, 100_000, 1_000, 10);
        let rows = vec![
            row("naive_recompute", near_one, 1.0, 99.0),
            row("ivmlite", near_one, 10.0, 90.0), // 1.0x
            row("naive_recompute", near_two, 1.0, 199.0),
            row("ivmlite", near_two, 10.0, 90.0), // 2.0x
            row("naive_recompute", far, 1.0, 999.0),
            row("ivmlite", far, 1.0, 9.0),               // 100x
            row("hand_written_trigger", far, 50.0, 0.0), // ivmlite 10 < 50
            row("naive_recompute", beats_triggers, 1.0, 999.0),
            row("ivmlite", beats_triggers, 5.0, 5.0),
            row("hand_written_trigger", beats_triggers, 9.0, 0.0), // 10 > 9: no claim
        ];
        assert_eq!(select(&rows), vec![near_one, near_two, far]);
    }

    #[test]
    fn parse_csv_round_trips_the_task_1_header() {
        let text = format!(
            "{}\nivmlite,10,1000,10,10,1.0,2.5,3.5,4096,1,0,2,0,3,0,3,0\n",
            crate::CSV_HEADER
        );
        let rows = parse_csv(&text).unwrap();
        assert_eq!(
            (rows[0].views, rows[0].apply_ms, rows[0].maintain_ms),
            (10, 2.5, 3.5)
        );
        assert!(parse_csv("wrong,header\n").is_err());
    }

    /// A header alone (no data rows) is a legal, empty exploration CSV.
    #[test]
    fn parse_csv_accepts_a_header_with_no_rows() {
        assert_eq!(parse_csv(crate::CSV_HEADER).unwrap(), Vec::new());
    }

    /// `select` must not crash or double-count when a cell has no
    /// `hand_written_trigger` row at all (the CSV's usual shape: three of the
    /// four `select` cases only need `naive_recompute` and `ivmlite`).
    #[test]
    fn select_tolerates_a_cell_with_no_trigger_row() {
        let k = (10, 100_000, 100, 1_000);
        let rows = vec![
            row("naive_recompute", k, 1.0, 99.0),
            row("ivmlite", k, 10.0, 90.0),
        ];
        assert_eq!(select(&rows), vec![k]);
    }

    /// Spec §3.5's speedup range is inclusive at both ends: 0.7, 1.4 and 2.8
    /// must select; just outside them (0.69, 2.81) must not. Every pair below
    /// uses integer millisecond totals whose ratio is *exactly* the boundary
    /// in question (e.g. 7.0/10.0 for 0.7) — correctly-rounded `f64` division
    /// of exact integers and correctly-rounded parsing of the equal decimal
    /// literal round the same mathematical value to the same bit pattern, so
    /// this lands on the boundary itself rather than merely close to it.
    #[test]
    fn select_includes_the_range_boundaries_and_excludes_just_outside_them() {
        let at_0_7 = (1, 10, 1, 1);
        let at_1_4 = (2, 10, 1, 1);
        let at_2_8 = (3, 10, 1, 1);
        let below_0_7 = (4, 10, 1, 1);
        let above_2_8 = (5, 10, 1, 1);
        let rows = vec![
            row("naive_recompute", at_0_7, 7.0, 0.0),
            row("ivmlite", at_0_7, 10.0, 0.0), // 0.7
            row("naive_recompute", at_1_4, 14.0, 0.0),
            row("ivmlite", at_1_4, 10.0, 0.0), // 1.4
            row("naive_recompute", at_2_8, 28.0, 0.0),
            row("ivmlite", at_2_8, 10.0, 0.0), // 2.8
            row("naive_recompute", below_0_7, 69.0, 0.0),
            row("ivmlite", below_0_7, 100.0, 0.0), // 0.69, just under
            row("naive_recompute", above_2_8, 281.0, 0.0),
            row("ivmlite", above_2_8, 100.0, 0.0), // 2.81, just over
        ];
        assert_eq!(select(&rows), vec![at_0_7, at_1_4, at_2_8]);
    }

    /// The rotation `main::run_confirm` actually uses, pinned directly: it
    /// must vary across repeats for a fixed cell (spec §3.5), not just
    /// happen to vary once mixed into a whole run.
    #[test]
    fn order_rotates_across_repeats_for_a_fixed_cell() {
        assert_eq!(order(0, 0), engine_order(0));
        assert_eq!(order(0, 1), engine_order(1));
        assert_eq!(order(2, 3), engine_order(5));
        assert_ne!(order(0, 0), order(0, 1));
    }
}
