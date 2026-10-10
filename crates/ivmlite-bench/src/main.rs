mod ablation;
mod baseline;
mod confirm;
mod engine;
mod plot;
mod write_amp;

use std::path::{Path, PathBuf};

use engine::{engine_order, run_cell, Engine, Measurement};
use ivmlite_test::{extension_library_for, Profile};
use ivmlite_workload::{Workload, WriteAmpWorkload};

/// The Task 1 CSV header (spec §3.4). `pub(crate)` so `confirm.rs`'s
/// `parse_csv` reads exploration CSVs against this exact string too, rather
/// than keeping its own copy that could quietly drift from this one.
pub(crate) const CSV_HEADER: &str = "engine,views,base_rows,batch_size,group_cardinality,bootstrap_ms,apply_ms,maintain_ms,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free";

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

/// The write-amp CSV header (spec §7). A completely different shape from
/// `CSV_HEADER` (no `base_rows`/`batch_size`/`group_cardinality` — those are
/// fixed by the workload file, not swept — and `op`/`recursive_triggers`/
/// `rows`/`us_per_row`/`extra_us_per_row` in their place), so it gets its
/// own constant rather than sharing `measurement_csv`'s formatter.
/// `extra_us_per_row` is spec §7's "extra µs per row over `no_maintenance`"
/// (§3.4's "derived in the README" note is about the matrix CSV only —
/// write-amp computes and publishes this one directly, since it is exactly
/// what spec §7 asks the CSV for).
const WRITE_AMP_CSV_HEADER: &str = "engine,views,op,recursive_triggers,rows,apply_ms,us_per_row,extra_us_per_row,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free";

/// The extra µs per row `r`'s engine cost over `no_maintenance`, for the
/// same (`views`, op, `recursive_triggers`) combination (spec §7).
/// `no_maintenance` itself always gets exactly `0.0`: looking itself up in
/// `rows` finds its own `apply_ms`, so the subtraction cancels — no special
/// case needed. `run_write_amp` runs all three engines together for each
/// combination, so a missing `no_maintenance` row is a runner bug: it is an
/// error, never a silent `0.0`.
fn extra_us_per_row(
    rows: &[write_amp::WriteAmpRow],
    r: &write_amp::WriteAmpRow,
) -> Result<f64, String> {
    let no_maintenance_apply_ms = rows
        .iter()
        .find(|o| {
            o.engine == Engine::NoMaintenance
                && o.views == r.views
                && o.op == r.op
                && o.recursive_triggers == r.recursive_triggers
        })
        .map(|o| o.apply_ms)
        .ok_or_else(|| {
            format!(
                "no no_maintenance row for views={} op={} recursive_triggers={}",
                r.views,
                r.op.label(),
                r.recursive_triggers
            )
        })?;
    Ok((r.apply_ms - no_maintenance_apply_ms) * 1000.0 / r.rows as f64)
}

fn write_amp_row_csv(
    rows: &[write_amp::WriteAmpRow],
    r: &write_amp::WriteAmpRow,
) -> Result<String, String> {
    let us_per_row = r.apply_ms * 1000.0 / r.rows as f64;
    let extra = extra_us_per_row(rows, r)?;
    Ok(format!(
        "{},{},{},{},{},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{},{}",
        r.engine.label(),
        r.views,
        r.op.label(),
        r.recursive_triggers,
        r.rows,
        r.apply_ms,
        us_per_row,
        extra,
        r.page_size,
        r.base_pages,
        r.base_free,
        r.bootstrapped_pages,
        r.bootstrapped_free,
        r.written_pages,
        r.written_free,
        r.maintained_pages,
        r.maintained_free,
    ))
}

/// Sorted by (op, views, recursive_triggers, engine label) so the file diffs
/// stably across runs, the same rationale as `print_csv`. Every line is
/// formatted before any is printed, so an error leaves no partial CSV.
fn print_write_amp_csv(mut rows: Vec<write_amp::WriteAmpRow>) -> Result<(), String> {
    rows.sort_by(|a, b| {
        (
            a.op.label(),
            a.views,
            a.recursive_triggers,
            a.engine.label(),
        )
            .cmp(&(
                b.op.label(),
                b.views,
                b.recursive_triggers,
                b.engine.label(),
            ))
    });
    let lines = rows
        .iter()
        .map(|r| write_amp_row_csv(&rows, r))
        .collect::<Result<Vec<String>, String>>()?;
    println!("{WRITE_AMP_CSV_HEADER}");
    for line in &lines {
        println!("{line}");
    }
    Ok(())
}

/// Which view counts `write-amp` mode runs: the workload file's full
/// `view_counts` sweep, its `ablation_view_counts` (`--ablation-views`, the
/// set spec §6's ablation runs for each build), or an explicit `--views`
/// list.
#[derive(Debug, Clone, PartialEq)]
enum ViewSelection {
    Sweep,
    Ablation,
    Explicit(Vec<usize>),
}

/// `write-amp` mode (spec §7): load `workload_path` (default
/// `workloads/write-amp.toml`), select its view counts, run every
/// combination over the three write-amp engines, and print the CSV.
fn run_write_amp_mode(
    workload_path: &Path,
    lib: &Path,
    selection: ViewSelection,
) -> Result<(), String> {
    let mut w = WriteAmpWorkload::load(workload_path).map_err(|e| e.to_string())?;
    match selection {
        ViewSelection::Sweep => {}
        ViewSelection::Ablation => w.view_counts = w.ablation_view_counts.clone(),
        ViewSelection::Explicit(views) => w.view_counts = views,
    }
    // `load` validated the file as written; replacing `view_counts` can make
    // it invalid again (e.g. an empty list, though `parse_args` cannot
    // produce one today) — re-validate rather than trust the selection.
    w.validate().map_err(|e| e.to_string())?;
    let rows = write_amp::run_write_amp(&w, lib)?;
    print_write_amp_csv(rows)
}

/// `ablation` mode's rows: printed with `label,repeat` as two extra leading
/// columns before Task 1's own header (spec §6). `label` names which build of
/// the extension produced these rows — the same for every row of one run, so
/// `scripts/bench-ablation.sh` can combine several builds' rows in one file.
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
/// database and a rotated engine order each time (`confirm::order`), exactly
/// like `run_matrix`'s inner loop, just over the selected cells instead of
/// the whole matrix.
fn run_confirm(from: &Path, workload_path: &Path, lib: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let exploration = confirm::parse_csv(&text)?;
    let keys = confirm::select(&exploration);
    let base = Workload::load(workload_path).map_err(|e| e.to_string())?;

    let mut rows: Vec<(usize, Row)> = Vec::new();
    for (i, &(views, base_rows, batch_size, group_cardinality)) in keys.iter().enumerate() {
        let cell = base.with_cell(base_rows, group_cardinality, views, batch_size);
        for r in 0..confirm::REPEATS {
            for engine in confirm::order(i, r) {
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

/// Parsed command line: the mode (`matrix`, `confirm`, `ablation`,
/// `write-amp`, or `plot`, with `matrix` the default), plus shared overrides.
#[derive(Debug)]
struct Args {
    mode: String,
    extension: Option<PathBuf>,
    workload: PathBuf,
    /// `confirm`'s exploration CSV (`--from`).
    from: Option<PathBuf>,
    /// `ablation`'s build label (`--label`).
    label: Option<String>,
    /// `write-amp`'s view counts: the file's sweep by default,
    /// `--ablation-views` for the file's `ablation_view_counts`, or `--views`
    /// for an explicit comma-separated list.
    views: ViewSelection,
}

fn parse_args(raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut mode: Option<String> = None;
    let mut extension: Option<PathBuf> = None;
    let mut workload: Option<PathBuf> = None;
    let mut from: Option<PathBuf> = None;
    let mut label: Option<String> = None;
    let mut views: Option<Vec<usize>> = None;
    let mut ablation_views = false;

    let mut it = raw;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--extension" => {
                let v = it.next().ok_or("--extension needs a path")?;
                extension = Some(PathBuf::from(v));
            }
            "--workload" => {
                let v = it.next().ok_or("--workload needs a path")?;
                workload = Some(PathBuf::from(v));
            }
            "--from" => {
                let v = it.next().ok_or("--from needs a path")?;
                from = Some(PathBuf::from(v));
            }
            "--label" => {
                let v = it.next().ok_or("--label needs a name")?;
                label = Some(v);
            }
            "--views" => {
                let v = it.next().ok_or("--views needs a comma-separated list")?;
                let parsed = v
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<usize>()
                            .map_err(|e| format!("--views: invalid view count {s:?}: {e}"))
                    })
                    .collect::<Result<Vec<usize>, String>>()?;
                let mut seen = std::collections::BTreeSet::new();
                for &n in &parsed {
                    if !seen.insert(n) {
                        return Err(format!("--views: duplicate view count {n}"));
                    }
                }
                views = Some(parsed);
            }
            "--ablation-views" => ablation_views = true,
            other if mode.is_none() => mode = Some(other.to_string()),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }

    let mode = mode.unwrap_or_else(|| "matrix".to_string());

    // `--views` and `--ablation-views` (spec §7): only `write-amp` mode
    // ever reads them (main's `write-amp` arm is the only caller of
    // `Args::views`), so any other mode rejects them outright rather than
    // silently ignoring them, and the two cannot be combined.
    for (flag, given) in [
        ("--views", views.is_some()),
        ("--ablation-views", ablation_views),
    ] {
        if given && mode != "write-amp" {
            return Err(format!(
                "{flag} is only valid with write-amp mode, not {mode:?}"
            ));
        }
    }
    let views = match (views, ablation_views) {
        (Some(_), true) => {
            return Err("--views and --ablation-views cannot be combined".to_string())
        }
        (Some(list), false) => ViewSelection::Explicit(list),
        (None, true) => ViewSelection::Ablation,
        (None, false) => ViewSelection::Sweep,
    };

    // `write-amp`'s own default workload file differs from the other three
    // modes' (spec §7): only applied when `--workload` was not given
    // explicitly, so an explicit override always wins regardless of mode.
    let workload = workload.unwrap_or_else(|| {
        if mode == "write-amp" {
            PathBuf::from("workloads/write-amp.toml")
        } else {
            PathBuf::from("workloads/m0-baseline.toml")
        }
    });

    Ok(Args {
        mode,
        extension,
        workload,
        from,
        label,
        views,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args().skip(1))?;

    if args.mode == "plot" {
        let from = args
            .from
            .ok_or("plot mode needs --from <exploration CSV path>")?;
        for path in plot::write_charts(&from)? {
            eprintln!("wrote {}", path.display());
        }
        return Ok(());
    }

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
        "write-amp" => run_write_amp_mode(&args.workload, &lib, args.views)?,
        other => {
            return Err(format!(
                "unknown mode {other:?} (expected matrix, confirm, ablation, write-amp or plot)"
            )
            .into())
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `WriteAmpRow` fixture with every space/timing field but the ones
    /// the test cares about held at an arbitrary fixed value, for
    /// `extra_us_per_row`'s tests (spec §7).
    fn fixture_row(
        engine: Engine,
        views: usize,
        op: ivmlite_workload::WriteOpKind,
        recursive_triggers: bool,
        rows: usize,
        apply_ms: f64,
    ) -> write_amp::WriteAmpRow {
        write_amp::WriteAmpRow {
            engine,
            views,
            op,
            recursive_triggers,
            rows,
            apply_ms,
            page_size: 4096,
            base_pages: 1,
            base_free: 0,
            bootstrapped_pages: 1,
            bootstrapped_free: 0,
            written_pages: 1,
            written_free: 0,
            maintained_pages: 1,
            maintained_free: 0,
        }
    }

    /// `extra_us_per_row` (spec §7): `no_maintenance` itself gets exactly
    /// `0.0`, and another engine gets `(its apply_ms − no_maintenance's
    /// apply_ms) * 1000 / rows`, looked up from the same (`views`, op,
    /// `recursive_triggers`) combination. Three decoy `no_maintenance` rows
    /// come first, each differing from the right one in exactly one key
    /// field, so a lookup that ignored any one of the three fields would
    /// find its decoy before the right row.
    #[test]
    fn extra_us_per_row_is_zero_for_no_maintenance_and_the_difference_for_others() {
        use ivmlite_workload::WriteOpKind;

        let rows = vec![
            fixture_row(
                Engine::NoMaintenance,
                2,
                WriteOpKind::Insert,
                false,
                100,
                900.0,
            ),
            fixture_row(
                Engine::NoMaintenance,
                1,
                WriteOpKind::Delete,
                false,
                100,
                800.0,
            ),
            fixture_row(
                Engine::NoMaintenance,
                1,
                WriteOpKind::Insert,
                true,
                100,
                700.0,
            ),
            fixture_row(
                Engine::NoMaintenance,
                1,
                WriteOpKind::Insert,
                false,
                100,
                10.0,
            ),
            fixture_row(
                Engine::HandWrittenTrigger,
                1,
                WriteOpKind::Insert,
                false,
                100,
                12.0,
            ),
            fixture_row(Engine::Ivmlite, 1, WriteOpKind::Insert, false, 100, 15.0),
        ];

        assert_eq!(
            extra_us_per_row(&rows, &rows[3]),
            Ok(0.0),
            "no_maintenance itself"
        );
        assert_eq!(
            extra_us_per_row(&rows, &rows[4]),
            Ok((12.0 - 10.0) * 1000.0 / 100.0)
        );
        assert_eq!(
            extra_us_per_row(&rows, &rows[5]),
            Ok((15.0 - 10.0) * 1000.0 / 100.0)
        );
    }

    /// A combination without a `no_maintenance` row is a runner bug (spec
    /// §7): `extra_us_per_row` reports it, and the CSV printer refuses to
    /// print anything, instead of publishing a made-up `0.0`.
    #[test]
    fn extra_us_per_row_is_an_error_without_a_matching_no_maintenance_row() {
        use ivmlite_workload::WriteOpKind;

        let rows = vec![fixture_row(
            Engine::Ivmlite,
            1,
            WriteOpKind::Insert,
            false,
            100,
            15.0,
        )];
        let err = extra_us_per_row(&rows, &rows[0]).unwrap_err();
        assert!(err.contains("no no_maintenance row"), "{err}");
        assert!(err.contains("views=1 op=insert"), "{err}");
        assert!(write_amp_row_csv(&rows, &rows[0]).is_err());
    }

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

    /// `write-amp` mode's own default workload file (spec §7): applied only
    /// when `--workload` was not given explicitly.
    #[test]
    fn parse_args_defaults_write_amp_to_its_own_workload_file() {
        let a = parse_args(["write-amp"].into_iter().map(String::from)).unwrap();
        assert_eq!(a.mode, "write-amp");
        assert_eq!(a.workload, PathBuf::from("workloads/write-amp.toml"));
        assert_eq!(a.views, ViewSelection::Sweep);
    }

    #[test]
    fn parse_args_accepts_plot_with_an_input_csv() {
        let args = parse_args(
            ["plot", "--from", "docs/bench/m1b-phase4.csv"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.mode, "plot");
        assert_eq!(args.from, Some(PathBuf::from("docs/bench/m1b-phase4.csv")));
    }

    /// An explicit `--workload` always wins, regardless of mode.
    #[test]
    fn parse_args_lets_an_explicit_workload_override_write_amps_default() {
        let a = parse_args(
            ["write-amp", "--workload", "/tmp/w.toml"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(a.workload, PathBuf::from("/tmp/w.toml"));
    }

    #[test]
    fn parse_args_reads_the_views_override_as_a_comma_separated_list() {
        let a = parse_args(
            ["write-amp", "--views", "0, 1, 10"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(a.views, ViewSelection::Explicit(vec![0, 1, 10]));
    }

    #[test]
    fn parse_args_rejects_a_non_numeric_views_entry() {
        let err = parse_args(
            ["write-amp", "--views", "0,x"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err();
        assert!(err.contains("--views"), "{err}");
    }

    /// Spec §7: `--views` must reject a repeated entry rather than silently
    /// running that view count twice.
    #[test]
    fn parse_args_rejects_a_duplicate_views_entry() {
        let err = parse_args(
            ["write-amp", "--views", "0,1,0"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err();
        assert!(err.contains("--views"), "{err}");
        assert!(err.contains("duplicate"), "{err}");
    }

    /// Spec §7: `--views` is a `write-amp`-only flag; every other mode
    /// rejects it outright instead of silently ignoring it.
    #[test]
    fn parse_args_rejects_views_outside_write_amp_mode() {
        let err =
            parse_args(["matrix", "--views", "0,1"].into_iter().map(String::from)).unwrap_err();
        assert!(err.contains("--views"), "{err}");
        assert!(err.contains("matrix"), "{err}");
    }

    /// `--ablation-views` selects the workload file's
    /// `ablation_view_counts` (spec §6, §8: the ablation's view counts live
    /// in the workload file, not in the script), is `write-amp`-only like
    /// `--views`, and cannot be combined with it.
    #[test]
    fn parse_args_reads_ablation_views_and_rejects_misuse() {
        let a = parse_args(
            ["write-amp", "--ablation-views"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(a.views, ViewSelection::Ablation);

        let err = parse_args(
            ["ablation", "--ablation-views"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err();
        assert!(err.contains("--ablation-views"), "{err}");
        assert!(err.contains("ablation"), "{err}");

        let err = parse_args(
            ["write-amp", "--ablation-views", "--views", "10"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err();
        assert!(err.contains("cannot be combined"), "{err}");
    }

    /// The shipped write-amp file names the ablation's view counts, and they
    /// are the ones the published ablation write-amp CSV was run at.
    #[test]
    fn the_shipped_write_amp_file_names_the_ablation_view_counts() {
        let workload_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/write-amp.toml");
        let w = WriteAmpWorkload::load(&workload_path).unwrap();
        assert_eq!(w.ablation_view_counts, vec![10, 200]);
    }

    /// Spec §8: `run_write_amp_mode` must re-validate after selecting the
    /// view counts, not just trust the selection — an empty explicit list
    /// (which `parse_args` itself can never produce, but nothing stops a
    /// future caller from) must be rejected before anything tries to run it.
    #[test]
    fn run_write_amp_mode_rejects_an_override_that_empties_view_counts() {
        let workload_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/write-amp.toml");
        let err = run_write_amp_mode(
            &workload_path,
            Path::new("/nonexistent"),
            ViewSelection::Explicit(vec![]),
        )
        .unwrap_err();
        assert!(err.contains("view_counts"), "{err}");
    }
}
