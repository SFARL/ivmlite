mod baseline;
mod plot;

use std::path::Path;

use baseline::{
    apply, install_trigger_view, recompute_all, seed_base, ApplyStatements, Baseline,
    RecomputeStatements,
};
use ivmlite_workload::Workload;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct Record {
    pub baseline: &'static str,
    pub views: usize,
    pub base_rows: usize,
    pub batch: usize,
    pub cardinality: usize,
    pub apply_ms: f64,
    pub maintain_ms: f64,
}

/// Run one fully materialized matrix cell. `cell` comes from `Workload::cells()`
/// and is not modified here: all four dimensions — `base_rows`,
/// `group_cardinality`, `batch_size`, `views` — are decided by
/// `ivmlite-workload` (spec §10.3 item 7). `main.rs` only runs the cell under
/// each baseline, times it, and records the result.
fn run_one(b: Baseline, cell: &Workload) -> rusqlite::Result<Record> {
    let conn = Connection::open_in_memory()?;

    // ---- untimed: build the initial state ----
    seed_base(&conn, cell)?;
    if b == Baseline::HandWrittenTrigger {
        for v in &cell.views {
            install_trigger_view(&conn, &cell.schema.table, v)?;
        }
    }
    let ops = cell.update_trace();

    // Compile every statement the timed region runs, now that all triggers
    // exist. See `ApplyStatements` for why this must not happen under the timer.
    let mut apply_stmts = ApplyStatements::prepare(&conn, &cell.schema.table)?;
    let mut recompute_stmts = match b {
        Baseline::NaiveRecompute => Some(RecomputeStatements::prepare(&conn, cell)?),
        Baseline::NoMaintenance | Baseline::HandWrittenTrigger => None,
    };

    // ---- timed region ----
    let apply_ms = apply(&conn, &mut apply_stmts, &ops)?;
    let maintain_ms = match recompute_stmts.as_mut() {
        Some(stmts) => recompute_all(stmts)?,
        // The trigger's cost is already in apply_ms: that is the write amplification.
        None => 0.0,
    };

    Ok(Record {
        baseline: b.label(),
        views: cell.views.len(),
        base_rows: cell.data.base_rows,
        batch: cell.updates.batch_size,
        cardinality: cell.data.group_cardinality,
        apply_ms,
        maintain_ms,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Workload::load(Path::new("workloads/m0-baseline.toml"))?;
    let cells = base.cells();
    let mut records: Vec<Record> = Vec::new();

    let baselines = [
        Baseline::NoMaintenance,
        Baseline::HandWrittenTrigger,
        Baseline::NaiveRecompute,
    ];

    // The matrix's structure — the two sweeps, the `card > base_rows` skip rule,
    // the view-threshold formula — lives entirely in `Workload::cells()`
    // (spec §10.3 item 7); this loop only walks the cells it produces under each
    // baseline. Skipped cells are not logged to stderr here: `cells()` simply
    // does not produce them, and `main.rs` has no `card > rows` check with which
    // to recognise a cell that "should have been there". Adding one would
    // re-implement, here, a rule that was deliberately moved out.
    // `docs/bench/README.md` already records the skip (`card=100000 >
    // base_rows=10000`).
    for b in baselines {
        for cell in &cells {
            records.push(run_one(b, cell)?);
        }
    }

    println!("baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms");
    for r in &records {
        println!(
            "{},{},{},{},{},{:.3},{:.3}",
            r.baseline, r.views, r.base_rows, r.batch, r.cardinality, r.apply_ms, r.maintain_ms
        );
    }

    // One chart per group cardinality: the crossover moves sharply with it, so
    // publishing a single chart would amount to picking a flattering point
    // (spec §10.1). Cardinalities and the fixed view count come from `[matrix]`,
    // not from constants here.
    let matrix = base
        .matrix
        .as_ref()
        .expect("workloads/m0-baseline.toml has no [matrix] section");
    for card in matrix.group_cardinalities.clone() {
        let path = format!("docs/bench/m0-baseline-card{card}.svg");
        match plot::write_svg(Path::new(&path), &records, matrix.fixed_views, 100, card) {
            Ok(()) => eprintln!("wrote chart {path}"),
            // A group cardinality skipped at every base-table size has no data
            // points. That is not an error: say so and carry on.
            Err(e) => eprintln!("skipping the chart for card={card}: {e}"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of `docs/bench/m0-baseline.csv`, keeping only the four matrix
    /// dimensions (the baseline name and the two timing columns are not
    /// something `cells()` produces).
    type CsvCellKey = (usize, usize, usize, usize); // (views, base_rows, batch_size, group_cardinality)

    /// A minimal hand-written CSV parse: the file has no quoting and no embedded
    /// commas — every field is a plain identifier or number — so it does not
    /// justify a csv dependency.
    fn read_csv_cell_keys(path: &Path) -> std::collections::BTreeSet<CsvCellKey> {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let mut lines = text.lines();
        let header = lines.next().expect("the CSV must have a header row");
        assert_eq!(
            header, "baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms",
            "the CSV header changed shape, so reading fields by position below no longer holds"
        );

        lines
            .filter(|l| !l.is_empty())
            .map(|line| {
                let fields: Vec<&str> = line.split(',').collect();
                assert_eq!(fields.len(), 7, "wrong number of CSV fields: {line}");
                let views: usize = fields[1].parse().unwrap_or_else(|_| panic!("{line}"));
                let base_rows: usize = fields[2].parse().unwrap_or_else(|_| panic!("{line}"));
                let batch_size: usize = fields[3].parse().unwrap_or_else(|_| panic!("{line}"));
                let group_cardinality: usize =
                    fields[4].parse().unwrap_or_else(|_| panic!("{line}"));
                (views, base_rows, batch_size, group_cardinality)
            })
            .collect()
    }

    /// M0 review finding I7: another engine's runner that loads
    /// `workloads/m0-baseline.toml` must be able to re-derive exactly the set of
    /// cells in the published `docs/bench/m0-baseline.csv` — which is the whole
    /// reason `ivmlite-workload` exists (spec §10.3 item 7). This test is the
    /// only proof of that claim: it dedups the `(views, base_rows, batch_size,
    /// group_cardinality)` tuples in the published CSV and compares them, as a
    /// set, with the tuples `base.cells()` produces — same count, same members,
    /// nothing missing and nothing extra.
    ///
    /// View thresholds are not a CSV column (the CSV records only how many views
    /// there are), so the test also independently recomputes every view's
    /// threshold from `[matrix]`'s `view_threshold_stride` /
    /// `view_threshold_modulus` and asserts it matches what `cells()` produced.
    /// Otherwise a regression like "one character of the threshold formula
    /// changed when it moved from main.rs into `MatrixSpec::views()`" would be
    /// invisible to the tuple comparison, since no CSV column carries it.
    ///
    /// It does not re-run the benchmark (that takes about 9 minutes): it only
    /// reads the committed CSV, byte for byte unchanged, as a fixture.
    #[test]
    fn cells_reproduce_exactly_the_published_csv_matrix() {
        let workload_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m0-baseline.toml");
        let csv_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bench/m0-baseline.csv");

        let base = Workload::load(&workload_path).expect("the published workload file must parse");
        let matrix = base
            .matrix
            .as_ref()
            .expect("workloads/m0-baseline.toml has no [matrix] section")
            .clone();

        let cells = base.cells();

        // Independent check of the threshold formula, separate from the tuple
        // comparison: it guards the invariant that the formula was not quietly
        // altered when it moved.
        for cell in &cells {
            for (i, v) in cell.views.iter().enumerate() {
                let expected =
                    (i as i64 * matrix.view_threshold_stride) % matrix.view_threshold_modulus;
                assert_eq!(
                    v.threshold, expected,
                    "view {i} should have threshold {expected} (stride={}, modulus={}), got {}",
                    matrix.view_threshold_stride, matrix.view_threshold_modulus, v.threshold
                );
            }
        }

        let derived: std::collections::BTreeSet<CsvCellKey> = cells
            .iter()
            .map(|c| {
                (
                    c.views.len(),
                    c.data.base_rows,
                    c.updates.batch_size,
                    c.data.group_cardinality,
                )
            })
            .collect();

        let published = read_csv_cell_keys(&csv_path);

        let missing: Vec<_> = published.difference(&derived).collect();
        let extra: Vec<_> = derived.difference(&published).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "cells() and the published CSV disagree on the set of cells:\nmissing (in the CSV, not from cells()): {missing:?}\nextra (from cells(), not in the CSV): {extra:?}"
        );
        assert_eq!(
            derived.len(),
            published.len(),
            "cells() produced {} distinct cells, the CSV has {}",
            derived.len(),
            published.len()
        );
    }
}
