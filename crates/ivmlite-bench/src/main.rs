mod ablation;
mod baseline;
mod confirm;
mod engine;
// M1b Phase 4 task 1 stops calling `plot::write_svg` here (the M0 chart
// writing moves to Task 6, spec §3), so the module is temporarily unreachable
// outside its own tests; Task 6 restores the call and this `allow` with it.
#[allow(dead_code)]
mod plot;

use std::path::{Path, PathBuf};

use engine::{engine_order, run_cell, Engine, Measurement};
use ivmlite_test::{extension_library_for, Profile};
use ivmlite_workload::Workload;

/// One row of an M0 chart's data (Task 6 draws the M1b chart from `Row`
/// instead). Kept here because `plot.rs` still reads `Record`s in its own
/// tests; `main.rs` no longer constructs any.
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

const CSV_HEADER: &str = "engine,views,base_rows,batch_size,group_cardinality,bootstrap_ms,apply_ms,maintain_ms,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free";

/// One matrix row: the cell's four dimensions (spec §3.4's "The cell (as M0)"
/// group) plus the `Measurement` `run_cell` produced for it.
struct Row {
    engine: Engine,
    views: usize,
    base_rows: usize,
    batch_size: usize,
    group_cardinality: usize,
    m: Measurement,
}

impl Row {
    fn sort_key(&self) -> (&'static str, usize, usize, usize, usize) {
        (
            self.engine.label(),
            self.views,
            self.base_rows,
            self.batch_size,
            self.group_cardinality,
        )
    }
}

/// One data row's fields after `engine`, formatted exactly as `CSV_HEADER`
/// orders them. Shared by every mode's printer (`matrix`, `confirm`,
/// `ablation`) so the 17 Task 1 columns are written in one place, with each
/// mode only adding its own leading columns (`repeat`, or `label,repeat`).
fn measurement_csv(
    engine: &str,
    views: usize,
    base_rows: usize,
    batch_size: usize,
    group_cardinality: usize,
    m: &Measurement,
) -> String {
    format!(
        "{},{},{},{},{},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{},{}",
        engine,
        views,
        base_rows,
        batch_size,
        group_cardinality,
        m.bootstrap_ms,
        m.apply_ms,
        m.maintain_ms,
        m.page_size,
        m.base.page_count,
        m.base.freelist_count,
        m.bootstrapped.page_count,
        m.bootstrapped.freelist_count,
        m.written.page_count,
        m.written.freelist_count,
        m.maintained.page_count,
        m.maintained.freelist_count,
    )
}

fn print_csv(mut rows: Vec<Row>) {
    // Sorted by (engine label, views, base_rows, batch_size, group_cardinality)
    // so the file diffs stably across runs (spec §3.4).
    rows.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    println!("{CSV_HEADER}");
    for r in &rows {
        println!(
            "{}",
            measurement_csv(
                r.engine.label(),
                r.views,
                r.base_rows,
                r.batch_size,
                r.group_cardinality,
                &r.m,
            )
        );
    }
}

/// `confirm` mode's rows: each is one repetition of a selected cell (spec
/// §3.5). Printed with `repeat` as an extra leading column before Task 1's
/// own header.
fn print_confirm_csv(mut rows: Vec<(usize, Row)>) {
    rows.sort_by(|a, b| (a.0, a.1.sort_key()).cmp(&(b.0, b.1.sort_key())));
    println!("repeat,{CSV_HEADER}");
    for (repeat, r) in &rows {
        println!(
            "{repeat},{}",
            measurement_csv(
                r.engine.label(),
                r.views,
                r.base_rows,
                r.batch_size,
                r.group_cardinality,
                &r.m,
            )
        );
    }
}

/// `ablation` mode's rows: printed with `label,repeat` as two extra leading
/// columns before Task 1's own header (spec §6). `label` names which build of
/// the extension produced these rows — the same for every row of one run, so
/// `scripts/bench-ablation.sh` can append several runs' output into one file.
fn print_ablation_csv(label: &str, mut rows: Vec<ablation::AblationRow>) {
    rows.sort_by(|a, b| {
        (
            a.repeat,
            a.engine.label(),
            a.views,
            a.base_rows,
            a.batch_size,
            a.group_cardinality,
        )
            .cmp(&(
                b.repeat,
                b.engine.label(),
                b.views,
                b.base_rows,
                b.batch_size,
                b.group_cardinality,
            ))
    });
    println!("label,repeat,{CSV_HEADER}");
    for r in &rows {
        println!(
            "{label},{},{}",
            r.repeat,
            measurement_csv(
                r.engine.label(),
                r.views,
                r.base_rows,
                r.batch_size,
                r.group_cardinality,
                &r.m,
            )
        );
    }
}

/// The full matrix: cells in the outer loop, engines in the inner loop,
/// engine order rotating with the cell index (spec §3.3). `Workload::cells()`
/// is the same derivation M0 used, unchanged (spec §3.5).
fn run_matrix(workload_path: &Path, lib: &Path) -> Result<(), String> {
    let base = Workload::load(workload_path).map_err(|e| e.to_string())?;
    let cells = base.cells();
    let mut rows = Vec::with_capacity(cells.len() * engine::ENGINES.len());

    for (i, cell) in cells.iter().enumerate() {
        for engine in engine_order(i) {
            let m = run_cell(engine, cell, lib).map_err(|e| {
                format!(
                    "cell {i} (views={} base_rows={} batch_size={} group_cardinality={}) engine {}: {e}",
                    cell.views.len(),
                    cell.data.base_rows,
                    cell.updates.batch_size,
                    cell.data.group_cardinality,
                    engine.label(),
                )
            })?;
            assert_eq!(
                m.engine, engine,
                "run_cell returned a Measurement for a different engine than it was asked to run"
            );
            rows.push(Row {
                engine,
                views: cell.views.len(),
                base_rows: cell.data.base_rows,
                batch_size: cell.updates.batch_size,
                group_cardinality: cell.data.group_cardinality,
                m,
            });
        }
    }

    print_csv(rows);
    Ok(())
}

/// `confirm` mode (spec §3.5): read the exploration CSV at `from`, select the
/// cells worth confirming, and re-run each `confirm::REPEATS` times — a fresh
/// database and a rotated engine order each time, exactly like `run_matrix`'s
/// inner loop, just over the selected cells instead of the whole matrix, with
/// the rotation offset by the cell's position among the selected cells (so
/// two different selections do not happen to share the same rotation).
fn run_confirm(from: &Path, workload_path: &Path, lib: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let exploration = confirm::parse_csv(&text)?;
    let keys = confirm::select(&exploration);
    let base = Workload::load(workload_path).map_err(|e| e.to_string())?;

    let mut rows: Vec<(usize, Row)> = Vec::new();
    for (i, &(views, base_rows, batch_size, group_cardinality)) in keys.iter().enumerate() {
        let cell = base.with_cell(base_rows, group_cardinality, views, batch_size);
        for r in 0..confirm::REPEATS {
            for engine in engine_order(r + i) {
                let m = run_cell(engine, &cell, lib).map_err(|e| {
                    format!(
                        "confirm cell {i} (views={views} base_rows={base_rows} batch_size={batch_size} group_cardinality={group_cardinality}) repeat {r} engine {}: {e}",
                        engine.label(),
                    )
                })?;
                rows.push((
                    r,
                    Row {
                        engine,
                        views,
                        base_rows,
                        batch_size,
                        group_cardinality,
                        m,
                    },
                ));
            }
        }
    }

    print_confirm_csv(rows);
    Ok(())
}

/// `ablation` mode (spec §6): run `workload_path`'s `[ablation]` cells against
/// `lib`, and print the rows labelled `label` so several runs (one per build
/// under test) can be told apart once appended into one file.
fn run_ablation_mode(workload_path: &Path, lib: &Path, label: &str) -> Result<(), String> {
    let base = Workload::load(workload_path).map_err(|e| e.to_string())?;
    let spec = base.ablation.clone().ok_or_else(|| {
        format!(
            "{} has no [ablation] section (spec §6, §8)",
            workload_path.display()
        )
    })?;
    let rows = ablation::run_ablation(&base, &spec, lib)?;
    print_ablation_csv(label, rows);
    Ok(())
}

/// Parsed command line: the mode (Task 5 adds `write-amp`; `matrix`,
/// `confirm` and `ablation` are today's modes, with `matrix` the default),
/// plus the shared overrides.
struct Args {
    mode: String,
    extension: Option<PathBuf>,
    workload: PathBuf,
    /// `confirm`'s exploration CSV (`--from`).
    from: Option<PathBuf>,
    /// `ablation`'s build label (`--label`).
    label: Option<String>,
}

fn parse_args(raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut mode: Option<String> = None;
    let mut extension: Option<PathBuf> = None;
    let mut workload = PathBuf::from("workloads/m0-baseline.toml");
    let mut from: Option<PathBuf> = None;
    let mut label: Option<String> = None;

    let mut it = raw;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--extension" => {
                let v = it.next().ok_or("--extension needs a path")?;
                extension = Some(PathBuf::from(v));
            }
            "--workload" => {
                let v = it.next().ok_or("--workload needs a path")?;
                workload = PathBuf::from(v);
            }
            "--from" => {
                let v = it.next().ok_or("--from needs a path")?;
                from = Some(PathBuf::from(v));
            }
            "--label" => {
                let v = it.next().ok_or("--label needs a name")?;
                label = Some(v);
            }
            other if mode.is_none() => mode = Some(other.to_string()),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }

    Ok(Args {
        mode: mode.unwrap_or_else(|| "matrix".to_string()),
        extension,
        workload,
        from,
        label,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args().skip(1))?;

    let lib = match args.extension {
        // The staleness check is skipped for an explicit path (spec §3.1):
        // the ablation (Task 5) points this at a library built without a fix
        // under test, which is older than the fixed sources on purpose.
        Some(p) => p,
        None => extension_library_for(Profile::Release).unwrap_or_else(|why| {
            eprintln!("{why}");
            std::process::exit(1);
        }),
    };

    match args.mode.as_str() {
        "matrix" => run_matrix(&args.workload, &lib)?,
        "confirm" => {
            let from = args
                .from
                .ok_or("confirm mode needs --from <exploration CSV path>")?;
            run_confirm(&from, &args.workload, &lib)?;
        }
        "ablation" => {
            let label = args.label.ok_or("ablation mode needs --label <name>")?;
            run_ablation_mode(&args.workload, &lib, &label)?;
        }
        other => return Err(format!("unknown mode {other:?} (Task 5 adds write-amp)").into()),
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

    #[test]
    fn parse_args_defaults_to_matrix_and_the_m0_workload() {
        let a = parse_args(std::iter::empty()).unwrap();
        assert_eq!(a.mode, "matrix");
        assert_eq!(a.extension, None);
        assert_eq!(a.workload, PathBuf::from("workloads/m0-baseline.toml"));
    }

    #[test]
    fn parse_args_reads_mode_and_overrides() {
        let a = parse_args(
            [
                "confirm",
                "--extension",
                "/tmp/lib.dylib",
                "--workload",
                "/tmp/w.toml",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.mode, "confirm");
        assert_eq!(a.extension, Some(PathBuf::from("/tmp/lib.dylib")));
        assert_eq!(a.workload, PathBuf::from("/tmp/w.toml"));
    }
}
