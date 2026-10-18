//! Derived M2a tables, computed only from the raw per-case artifacts.
//!
//! Definitions (also written to `summary.md`):
//!
//! * A run's total for a mode is the median over the five measured rounds of that round's
//!   apply+maintain+read sum, never a sum of component medians. Apply+maintain is the median of
//!   each round's apply+maintain sum.
//! * The fastest complete recompute of a run is the smaller of the `recompute` (ordinary) and
//!   `covering_recompute` totals of that same run. `no_maintenance` is never a numerator or a
//!   denominator; its apply time is only reported as the write lower bound.
//! * The ratio of a run is fastest-recompute total / ivmlite total. Per cell the minimum, median
//!   and maximum over the cell's complete runs are reported; a cell with fewer complete runs
//!   than planned is `partial`.
//! * A confirmation candidate is a cell whose [min, max] ratio range contains 1.0 or 2.0, or
//!   whose median is within 10% of 1.0 or 2.0.
//! * A case without `completed.json`, or whose completed artifacts cannot be read, is a loss in
//!   `losses.csv` and contributes to no table.
//! * Confirmation repeats (`confirm`) are timing-only paired runs of a candidate cell at seed 42.
//!   They get their own `confirmations.csv` and never enter the seed tables (`timing.csv`,
//!   `ratios.csv`).
//!
//! Smoke selections (`selection-*-smoke.json`) are never read into a table.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use ivmlite_test::demo_bench::Summary;
use ivmlite_workload::m2a::{Cell, M2aConfig, Shape};
use serde::{Deserialize, Serialize};

use crate::m2a_runner::{Checkpoint, Measurement, Mode, Result, RoundTiming, TargetStats, MODES};
use crate::m2a_space::SpaceCheckpoint;
use crate::m2a_state::{PageMetrics, Snapshot};
use crate::{
    case_path, read_json, selection_file, validate_completion, write_json, BuildIdentity,
    ExpectedCase, Manifest, Selection, SmokeGate, TimingFile, CONFIRM_REPEAT_ARTIFACTS,
    CONFIRM_SEED, INVERSE_ARTIFACTS, RUN_ARTIFACTS,
};

/// Which cases of each action a report covers, in the order they are listed.
const ACTIONS: [&str; 4] = ["run", "churn", "confirm", "verify"];
const COMPLETION_MARKER: &str = "completed.json";
/// Ratios whose range or median is near these values need confirmation (spec section 4).
const THRESHOLDS: [f64; 2] = [1.0, 2.0];
const NEAR_BAND: f64 = 0.10;
/// Absorbs floating-point error at the exact edge of the near band, so 1.1 and 0.9 (and 2.2 and
/// 1.8) are treated alike.
const BAND_TOLERANCE: f64 = 1e-9;

/// A cell and the fixture seed that a later confirmation run repeats.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CaseKey {
    pub cell: String,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportSummary {
    /// Cases of every non-smoke selection that completed and were read.
    pub complete: usize,
    /// Cases listed in `losses.csv`.
    pub incomplete: usize,
    pub candidates: Vec<CaseKey>,
    /// The highest per-cell median ratio, if any cell has a ratio.
    pub best_ratio: Option<f64>,
}

struct Loss {
    kind: String,
    key: ExpectedCase,
    phase: String,
    error: String,
}

#[derive(Debug)]
struct ModeTotals {
    bootstrap_ms: f64,
    apply_ms: f64,
    maintain_ms: Option<f64>,
    read_ms: Option<f64>,
    apply_plus_maintain_ms: Option<f64>,
    total_ms: Option<f64>,
}

struct RunRatio {
    fastest: Mode,
    fastest_ms: f64,
    ordinary_ms: f64,
    covering_ms: f64,
    ivmlite_ms: f64,
    no_maintenance_apply_ms: f64,
    ratio: f64,
}

struct RunCase {
    key: ExpectedCase,
    manifest: Manifest,
    totals: BTreeMap<&'static str, ModeTotals>,
    ratio: RunRatio,
    space: Vec<SpacePoint>,
    targets: Vec<TargetStats>,
    correctness: Vec<Checkpoint>,
    initial_hot_share: f64,
}

/// One timing-only repeat of a confirmed cell.
struct ConfirmCase {
    key: ExpectedCase,
    manifest: Manifest,
    ratio: RunRatio,
    load_before: f64,
    load_after: f64,
    max_load: f64,
    /// Names of the build or host fields in which the initial run differs from this repeat.
    build_differs: BTreeSet<&'static str>,
}
impl ConfirmCase {
    /// The host was busier after the repeat than the limit that admitted it (spec section 4).
    fn busy_after(&self) -> bool {
        self.load_after > self.max_load
    }
}

struct ChurnCase {
    manifest: Manifest,
    correctness: Vec<Checkpoint>,
}

struct SpacePoint {
    round: Option<usize>,
    phase: String,
    pages: BTreeMap<&'static str, PageMetrics>,
}
impl SpacePoint {
    fn over_base_only(&self, mode: Mode) -> Option<f64> {
        ratio(
            self.pages[mode.label()].allocated_bytes,
            self.pages[Mode::NoMaintenance.label()].allocated_bytes,
        )
    }
}

struct RatioStats {
    min: f64,
    median: f64,
    max: f64,
    candidate: bool,
    ivmlite_bootstrap_ms: f64,
}

/// One planned cell. A cell whose every run is lost has no `stats`.
struct CellRatios {
    id: String,
    /// Known from any manifest of the cell; absent if no case of it got that far.
    cell: Option<Cell>,
    runs: usize,
    expected_runs: usize,
    stats: Option<RatioStats>,
    final_space_ratio: Option<f64>,
}
impl CellRatios {
    fn partial(&self) -> bool {
        self.runs < self.expected_runs
    }
}

#[derive(Default)]
struct Derived {
    runs: Vec<RunCase>,
    churns: Vec<ChurnCase>,
    confirmations: Vec<ConfirmCase>,
    complete: usize,
    losses: Vec<Loss>,
    run_expected: BTreeMap<String, usize>,
    /// Planned confirmation repeats per cell.
    confirm_expected: BTreeMap<String, usize>,
    /// Cell dimensions of planned run cells, from whichever manifest could be read.
    run_cells: BTreeMap<String, Cell>,
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

fn median(values: &[f64]) -> Result<f64> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err("non-finite timing value".into());
    }
    Ok(Summary::of(values).ok_or("no measured rounds")?.median)
}
fn optional_median(values: &[Option<f64>]) -> Result<Option<f64>> {
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    if present.is_empty() {
        Ok(None)
    } else if present.len() != values.len() {
        Err("some measured rounds lack a metric".into())
    } else {
        median(&present).map(Some)
    }
}

/// Medians over the measured rounds of the per-round sums, recomputed from the raw rounds.
fn mode_totals(m: &Measurement) -> Result<ModeTotals> {
    let rounds: &[RoundTiming] = &m.measured;
    let of = |f: &dyn Fn(&RoundTiming) -> Option<f64>| -> Result<Option<f64>> {
        optional_median(&rounds.iter().map(f).collect::<Vec<_>>())
    };
    Ok(ModeTotals {
        bootstrap_ms: m.bootstrap_ms,
        apply_ms: median(&rounds.iter().map(|r| r.apply_ms).collect::<Vec<_>>())?,
        maintain_ms: of(&|r| r.maintain_ms)?,
        read_ms: of(&|r| r.read_ms)?,
        apply_plus_maintain_ms: of(&|r| r.maintain_ms.map(|m| r.apply_ms + m))?,
        total_ms: of(&|r| {
            r.maintain_ms
                .zip(r.read_ms)
                .map(|(m, read)| r.apply_ms + m + read)
        })?,
    })
}

/// Fixture files carry the whole trace; only their workload configuration is read.
#[derive(Deserialize)]
struct FixtureConfig {
    config: M2aConfig,
}

fn totals_by_mode(
    timing: &TimingFile,
    measured_rounds: usize,
) -> Result<BTreeMap<&'static str, ModeTotals>> {
    let mut totals = BTreeMap::new();
    for mode in MODES {
        let mut found = timing.measurements.iter().filter(|m| m.mode == mode);
        let measurement = found
            .next()
            .ok_or_else(|| format!("timing lacks mode {}", mode.label()))?;
        if found.next().is_some() {
            return Err(format!("timing repeats mode {}", mode.label()).into());
        }
        if measurement.measured.len() != measured_rounds {
            return Err(format!(
                "{} has {} measured rounds, the fixture configures {measured_rounds}",
                mode.label(),
                measurement.measured.len()
            )
            .into());
        }
        totals.insert(mode.label(), mode_totals(measurement)?);
    }
    Ok(totals)
}

fn run_ratio(totals: &BTreeMap<&'static str, ModeTotals>) -> Result<RunRatio> {
    let total = |mode: Mode| {
        totals[mode.label()]
            .total_ms
            .ok_or_else(|| format!("{} has no complete total", mode.label()))
    };
    let (ordinary_ms, covering_ms, ivmlite_ms) = (
        total(Mode::Recompute)?,
        total(Mode::CoveringRecompute)?,
        total(Mode::Ivmlite)?,
    );
    let (fastest, fastest_ms) = if covering_ms < ordinary_ms {
        (Mode::CoveringRecompute, covering_ms)
    } else {
        (Mode::Recompute, ordinary_ms)
    };
    if ivmlite_ms <= 0.0 || fastest_ms <= 0.0 {
        return Err("non-positive total".into());
    }
    Ok(RunRatio {
        fastest,
        fastest_ms,
        ordinary_ms,
        covering_ms,
        ivmlite_ms,
        no_maintenance_apply_ms: totals[Mode::NoMaintenance.label()].apply_ms,
        ratio: fastest_ms / ivmlite_ms,
    })
}

fn is_candidate(min: f64, median: f64, max: f64) -> bool {
    THRESHOLDS
        .iter()
        .any(|&t| (min <= t && t <= max) || (median - t).abs() <= NEAR_BAND * t + BAND_TOLERANCE)
}

fn space_points(space: &[SpaceCheckpoint]) -> Result<Vec<SpacePoint>> {
    let mut points: Vec<SpacePoint> = Vec::new();
    let mut index = BTreeMap::new();
    for checkpoint in space {
        let slot = *index
            .entry((checkpoint.round, checkpoint.phase.clone()))
            .or_insert_with(|| {
                points.push(SpacePoint {
                    round: checkpoint.round,
                    phase: checkpoint.phase.clone(),
                    pages: BTreeMap::new(),
                });
                points.len() - 1
            });
        if points[slot]
            .pages
            .insert(checkpoint.mode.label(), checkpoint.pages.clone())
            .is_some()
        {
            return Err("duplicate space checkpoint".into());
        }
    }
    if points.iter().any(|p| p.pages.len() != MODES.len()) {
        return Err("space checkpoints do not pair across all modes".into());
    }
    Ok(points)
}

/// Share of the initial fixture's grouped rows that lie in the hot partition.
fn initial_hot_share(cell: &Cell, config: &M2aConfig, correctness: &[Checkpoint]) -> Result<f64> {
    let bootstrap = correctness
        .iter()
        .find(|c| c.phase == "bootstrap" && c.round.is_none())
        .ok_or("correctness lacks the bootstrap checkpoint")?;
    let histogram = &bootstrap.snapshot.group_histogram;
    let hot = config.hot_group_count(cell.group_cardinality) as i64;
    let total: u64 = histogram.iter().map(|g| g.rows).sum();
    let hot_rows: u64 = histogram
        .iter()
        .filter(|g| g.group.is_some_and(|group| group < hot))
        .map(|g| g.rows)
        .sum();
    ratio(hot_rows, total).ok_or_else(|| "bootstrap histogram is empty".into())
}

fn read_manifest(dir: &Path, key: &ExpectedCase) -> Result<Manifest> {
    let manifest: Manifest = read_json(&dir.join("manifest.json"))?;
    if manifest.cell.id() != key.cell || manifest.seed != key.seed || manifest.repeat != key.repeat
    {
        return Err("manifest names a different case".into());
    }
    Ok(manifest)
}

fn load_run(dir: &Path, key: &ExpectedCase) -> Result<RunCase> {
    let manifest = read_manifest(dir, key)?;
    let fixture: FixtureConfig = read_json(&dir.join("fixture.json"))?;
    let timing: TimingFile = read_json(&dir.join("timing.json"))?;
    let totals = totals_by_mode(&timing, fixture.config.measured_rounds)?;
    let ratio = run_ratio(&totals)?;
    let space: Vec<SpaceCheckpoint> = read_json(&dir.join("space.json"))?;
    let correctness: Vec<Checkpoint> = read_json(&dir.join("correctness.json"))?;
    let initial_hot_share = initial_hot_share(&manifest.cell, &fixture.config, &correctness)?;
    Ok(RunCase {
        key: key.clone(),
        totals,
        ratio,
        space: space_points(&space)?,
        targets: read_json(&dir.join("targets.json"))?,
        correctness,
        initial_hot_share,
        manifest,
    })
}

/// `measured_rounds` of the workload that the initial run of `key`'s cell used. Confirmation
/// repeats keep no fixture of their own.
fn initial_measured_rounds(out: &Path, key: &ExpectedCase) -> Result<usize> {
    let initial = ExpectedCase {
        kind: "run".into(),
        repeat: 0,
        ..key.clone()
    };
    let fixture: FixtureConfig = read_json(&case_path(out, "run", &initial).join("fixture.json"))?;
    Ok(fixture.config.measured_rounds)
}

fn load_confirmation(
    out: &Path,
    dir: &Path,
    key: &ExpectedCase,
    measured_rounds: &mut BTreeMap<String, usize>,
) -> Result<ConfirmCase> {
    let manifest = read_manifest(dir, key)?;
    let (Some(load_before), Some(load_after), Some(max_load)) =
        (manifest.load_before, manifest.load_after, manifest.max_load)
    else {
        return Err("confirmation manifest lacks its load averages or its load limit".into());
    };
    let initial = manifest
        .initial_run
        .as_ref()
        .ok_or("confirmation manifest lacks the identity of its initial run")?;
    let build_differs = initial
        .differences(&BuildIdentity::of(&manifest))
        .into_iter()
        .collect();
    let rounds = match measured_rounds.get(&key.cell) {
        Some(&rounds) => rounds,
        None => {
            let rounds = initial_measured_rounds(out, key)?;
            measured_rounds.insert(key.cell.clone(), rounds);
            rounds
        }
    };
    let timing: TimingFile = read_json(&dir.join("timing.json"))?;
    Ok(ConfirmCase {
        key: key.clone(),
        ratio: run_ratio(&totals_by_mode(&timing, rounds)?)?,
        manifest,
        load_before,
        load_after,
        max_load,
        build_differs,
    })
}

fn load_churn(dir: &Path, key: &ExpectedCase) -> Result<ChurnCase> {
    Ok(ChurnCase {
        manifest: read_manifest(dir, key)?,
        correctness: read_json(&dir.join("correctness.json"))?,
    })
}

/// Why `key` has no completion marker: the selection's own record, the case's `failure.json`,
/// or never run.
fn missing_marker(selection: &Selection, dir: &Path, key: &ExpectedCase) -> (String, String) {
    if let Some(failed) = selection
        .summary
        .failed
        .iter()
        .find(|f| f.cell == key.cell && f.seed == key.seed && f.repeat == key.repeat)
    {
        return (failed.phase.clone(), failed.error.clone());
    }
    if let Ok(value) = read_json::<serde_json::Value>(&dir.join("failure.json")) {
        if let (Some(phase), Some(error)) = (value["phase"].as_str(), value["error"].as_str()) {
            return (phase.to_owned(), error.to_owned());
        }
    }
    // A case directory exists only once the runner started the case; a started case with neither
    // marker nor failure record was killed or interrupted, which is not the same as never run.
    if dir.is_dir() {
        return (
            "interrupted".into(),
            format!("started, but neither {COMPLETION_MARKER} nor failure.json was written"),
        );
    }
    ("not_run".into(), "case never started".into())
}

fn derive(out: &Path) -> Result<Derived> {
    if !selection_file(out, "run", false).exists() {
        return Err(format!(
            "no run selection at {}",
            selection_file(out, "run", false).display()
        )
        .into());
    }
    let mut derived = Derived::default();
    let mut measured_rounds = BTreeMap::new();
    for action in ACTIONS {
        let path = selection_file(out, action, false);
        if !path.exists() {
            continue;
        }
        let selection: Selection = read_json(&path)?;
        for key in &selection.expected {
            if action == "run" {
                *derived.run_expected.entry(key.cell.clone()).or_default() += 1;
            }
            if action == "confirm" {
                *derived
                    .confirm_expected
                    .entry(key.cell.clone())
                    .or_default() += 1;
            }
            let dir = case_path(out, action, key);
            let lost_cell = |derived: &mut Derived| {
                if action == "run" {
                    if let Ok(manifest) = read_manifest(&dir, key) {
                        derived.run_cells.insert(key.cell.clone(), manifest.cell);
                    }
                }
            };
            let loaded: Result<()> = if !dir.join(COMPLETION_MARKER).exists() {
                let (phase, error) = missing_marker(&selection, &dir, key);
                lost_cell(&mut derived);
                derived.losses.push(Loss {
                    kind: action.to_owned(),
                    key: key.clone(),
                    phase,
                    error,
                });
                continue;
            } else {
                // The tables are derived from hash-bound artifacts only: an artifact edited or
                // damaged after completion is a loss, never a silently used input.
                let artifacts: &[&str] = match action {
                    "run" => &RUN_ARTIFACTS,
                    "confirm" => &CONFIRM_REPEAT_ARTIFACTS,
                    _ => &INVERSE_ARTIFACTS,
                };
                validate_completion(&dir, "manifest.json", COMPLETION_MARKER, artifacts).and_then(
                    |()| match action {
                        "run" => load_run(&dir, key).map(|case| derived.runs.push(case)),
                        "churn" => load_churn(&dir, key).map(|case| derived.churns.push(case)),
                        "confirm" => load_confirmation(out, &dir, key, &mut measured_rounds)
                            .map(|case| derived.confirmations.push(case)),
                        _ => Ok(()),
                    },
                )
            };
            match loaded {
                Ok(()) => derived.complete += 1,
                Err(error) => {
                    lost_cell(&mut derived);
                    derived.losses.push(Loss {
                        kind: action.to_owned(),
                        key: key.clone(),
                        phase: "artifact_read".into(),
                        error: error.to_string(),
                    });
                }
            }
        }
    }
    Ok(derived)
}

fn cell_ratios(derived: &Derived) -> Result<Vec<CellRatios>> {
    let mut by_cell: BTreeMap<&str, Vec<&RunCase>> = BTreeMap::new();
    for case in &derived.runs {
        by_cell.entry(&case.key.cell).or_default().push(case);
    }
    let mut cells = Vec::new();
    for (id, &expected_runs) in &derived.run_expected {
        let cases = by_cell.remove(id.as_str()).unwrap_or_default();
        let ratios: Vec<f64> = cases.iter().map(|c| c.ratio.ratio).collect();
        let stats = match Summary::of(&ratios) {
            None => None,
            Some(summary) => {
                let bootstraps: Vec<f64> = cases
                    .iter()
                    .map(|c| c.totals[Mode::Ivmlite.label()].bootstrap_ms)
                    .collect();
                Some(RatioStats {
                    min: summary.min,
                    median: summary.median,
                    max: summary.max,
                    candidate: is_candidate(summary.min, summary.median, summary.max),
                    ivmlite_bootstrap_ms: median(&bootstraps)?,
                })
            }
        };
        let finals: Vec<f64> = cases.iter().filter_map(|c| final_space_ratio(c)).collect();
        cells.push(CellRatios {
            id: id.clone(),
            cell: cases
                .first()
                .map(|c| c.manifest.cell.clone())
                .or_else(|| derived.run_cells.get(id).cloned()),
            runs: cases.len(),
            expected_runs,
            stats,
            final_space_ratio: Summary::of(&finals).map(|s| s.median),
        });
    }
    cells.sort_by(|a, b| {
        (a.cell.as_ref().map(cell_order), &a.id).cmp(&(b.cell.as_ref().map(cell_order), &b.id))
    });
    Ok(cells)
}

fn cell_order(cell: &Cell) -> (Shape, usize, usize, &'static str) {
    (
        cell.shape,
        cell.fanout,
        cell.batch_size,
        cell.profile.label(),
    )
}

/// ivmlite / base-only allocated bytes at the run's last idle checkpoint.
fn final_space_ratio(case: &RunCase) -> Option<f64> {
    case.space
        .iter()
        .rfind(|p| p.phase == "idle")
        .and_then(|p| p.over_base_only(Mode::Ivmlite))
}

struct Csv {
    text: String,
    columns: usize,
}
impl Csv {
    fn new(header: &[&str]) -> Self {
        let mut csv = Self {
            text: String::new(),
            columns: header.len(),
        };
        csv.line(header.iter().map(|h| (*h).to_owned()));
        csv
    }
    fn line(&mut self, fields: impl IntoIterator<Item = String>) {
        let escaped: Vec<_> = fields.into_iter().map(|f| escape(&f)).collect();
        debug_assert_eq!(escaped.len(), self.columns);
        self.text.push_str(&escaped.join(","));
        self.text.push('\n');
    }
}
fn escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}
fn num(value: f64) -> String {
    value.to_string()
}
fn opt(value: Option<f64>) -> String {
    value.map_or_else(String::new, num)
}
fn optn(value: Option<usize>) -> String {
    value.map_or_else(String::new, |v| v.to_string())
}

const CELL_COLUMNS: [&str; 5] = ["cell", "shape", "profile", "fanout", "batch_size"];
fn shape_label(shape: Shape) -> &'static str {
    match shape {
        Shape::Single => "single",
        Shape::Join => "join",
    }
}
fn cell_fields(cell: &Cell) -> Vec<String> {
    vec![
        cell.id(),
        shape_label(cell.shape).to_owned(),
        cell.profile.label().to_owned(),
        cell.fanout.to_string(),
        cell.batch_size.to_string(),
    ]
}
/// Dimension columns of a cell known only by its id.
fn cell_id_fields(id: &str) -> Vec<String> {
    let mut fields = vec![id.to_owned()];
    fields.resize(CELL_COLUMNS.len(), String::new());
    fields
}
fn header(prefix: &[&str], rest: &[&str]) -> Vec<String> {
    prefix.iter().chain(rest).map(|c| (*c).to_owned()).collect()
}
fn with_header(columns: Vec<String>) -> Csv {
    Csv::new(&columns.iter().map(String::as_str).collect::<Vec<_>>())
}
fn joined(mut left: Vec<String>, right: impl IntoIterator<Item = String>) -> Vec<String> {
    left.extend(right);
    left
}

fn timing_csv(derived: &Derived) -> Csv {
    let mut csv = with_header(header(
        &CELL_COLUMNS,
        &[
            "seed",
            "repeat",
            "mode",
            "bootstrap_ms",
            "apply_ms",
            "maintain_ms",
            "read_ms",
            "apply_plus_maintain_ms",
            "total_ms",
        ],
    ));
    for case in &derived.runs {
        for mode in MODES {
            let t = &case.totals[mode.label()];
            csv.line(joined(
                cell_fields(&case.manifest.cell),
                [
                    case.key.seed.to_string(),
                    case.key.repeat.to_string(),
                    mode.label().to_owned(),
                    num(t.bootstrap_ms),
                    num(t.apply_ms),
                    opt(t.maintain_ms),
                    opt(t.read_ms),
                    opt(t.apply_plus_maintain_ms),
                    opt(t.total_ms),
                ],
            ));
        }
    }
    csv
}

fn ratios_csv(derived: &Derived, cells: &[CellRatios]) -> Csv {
    let mut csv = with_header(header(
        &["scope"],
        &[
            "cell",
            "shape",
            "profile",
            "fanout",
            "batch_size",
            "seed",
            "repeat",
            "fastest_recompute_mode",
            "fastest_recompute_ms",
            "ordinary_recompute_ms",
            "covering_recompute_ms",
            "ivmlite_ms",
            "no_maintenance_apply_ms",
            "ratio",
            "runs",
            "expected_runs",
            "partial",
            "ratio_min",
            "ratio_median",
            "ratio_max",
            "candidate",
        ],
    ));
    let blank = |n: usize| std::iter::repeat_n(String::new(), n);
    for case in &derived.runs {
        let r = &case.ratio;
        csv.line(
            joined(vec!["run".to_owned()], cell_fields(&case.manifest.cell))
                .into_iter()
                .chain([
                    case.key.seed.to_string(),
                    case.key.repeat.to_string(),
                    r.fastest.label().to_owned(),
                    num(r.fastest_ms),
                    num(r.ordinary_ms),
                    num(r.covering_ms),
                    num(r.ivmlite_ms),
                    num(r.no_maintenance_apply_ms),
                    num(r.ratio),
                ])
                .chain(blank(7)),
        );
    }
    for cell in cells {
        let stats = cell.stats.as_ref();
        csv.line(
            joined(
                vec!["cell".to_owned()],
                cell.cell
                    .as_ref()
                    .map_or_else(|| cell_id_fields(&cell.id), cell_fields),
            )
            .into_iter()
            .chain(blank(9))
            .chain([
                cell.runs.to_string(),
                cell.expected_runs.to_string(),
                cell.partial().to_string(),
                opt(stats.map(|s| s.min)),
                opt(stats.map(|s| s.median)),
                opt(stats.map(|s| s.max)),
                stats.is_some_and(|s| s.candidate).to_string(),
            ]),
        );
    }
    csv
}

/// Union of the build or host fields in which the initial run differs from these repeats.
fn build_differs<'a>(cases: impl Iterator<Item = &'a ConfirmCase>) -> String {
    let names: BTreeSet<_> = cases
        .flat_map(|c| c.build_differs.iter().copied())
        .collect();
    names.into_iter().collect::<Vec<_>>().join(";")
}

/// Repeat rows and one range row per confirmed cell. The seed tables stay as they are; the initial
/// range is repeated here only to read the two side by side.
fn confirmations_csv(derived: &Derived, cells: &[CellRatios]) -> Csv {
    let mut csv = with_header(header(
        &["scope"],
        &[
            "cell",
            "shape",
            "profile",
            "fanout",
            "batch_size",
            "repeat",
            "fastest_recompute_mode",
            "fastest_recompute_ms",
            "ordinary_recompute_ms",
            "covering_recompute_ms",
            "ivmlite_ms",
            "ratio",
            "load_before",
            "load_after",
            "max_load",
            "busy_after",
            "initial_build_differs",
            "repeats",
            "expected_repeats",
            "partial",
            "busy_after_repeats",
            "ratio_min",
            "ratio_median",
            "ratio_max",
            "straddles_1x",
            "straddles_2x",
            "initial_ratio_min",
            "initial_ratio_median",
            "initial_ratio_max",
        ],
    ));
    let blank = |n: usize| std::iter::repeat_n(String::new(), n);
    for (id, &expected) in &derived.confirm_expected {
        let mut repeats: Vec<&ConfirmCase> = derived
            .confirmations
            .iter()
            .filter(|c| &c.key.cell == id)
            .collect();
        repeats.sort_by_key(|c| c.key.repeat);
        let initial = cells.iter().find(|c| &c.id == id);
        let dims = initial
            .and_then(|c| c.cell.as_ref())
            .or_else(|| repeats.first().map(|c| &c.manifest.cell))
            .map_or_else(|| cell_id_fields(id), cell_fields);
        for case in &repeats {
            let r = &case.ratio;
            csv.line(
                joined(vec!["repeat".to_owned()], dims.clone())
                    .into_iter()
                    .chain([
                        case.key.repeat.to_string(),
                        r.fastest.label().to_owned(),
                        num(r.fastest_ms),
                        num(r.ordinary_ms),
                        num(r.covering_ms),
                        num(r.ivmlite_ms),
                        num(r.ratio),
                        num(case.load_before),
                        num(case.load_after),
                        num(case.max_load),
                        case.busy_after().to_string(),
                        build_differs(std::iter::once(*case)),
                    ])
                    .chain(blank(12)),
            );
        }
        let ratios: Vec<f64> = repeats.iter().map(|c| c.ratio.ratio).collect();
        let stats = Summary::of(&ratios);
        let straddles = |threshold: f64| {
            stats
                .as_ref()
                .map(|s| (s.min <= threshold && threshold <= s.max).to_string())
                .unwrap_or_default()
        };
        let initial = initial.and_then(|c| c.stats.as_ref());
        csv.line(
            joined(vec!["cell".to_owned()], dims)
                .into_iter()
                .chain(blank(11))
                .chain([
                    build_differs(repeats.iter().copied()),
                    repeats.len().to_string(),
                    expected.to_string(),
                    (repeats.len() < expected).to_string(),
                    repeats
                        .iter()
                        .filter(|c| c.busy_after())
                        .count()
                        .to_string(),
                    opt(stats.as_ref().map(|s| s.min)),
                    opt(stats.as_ref().map(|s| s.median)),
                    opt(stats.as_ref().map(|s| s.max)),
                    straddles(1.0),
                    straddles(2.0),
                    opt(initial.map(|s| s.min)),
                    opt(initial.map(|s| s.median)),
                    opt(initial.map(|s| s.max)),
                ]),
        );
    }
    csv
}

fn state_header() -> [&'static str; 6] {
    [
        "joined_rows",
        "arrangement_entries",
        "aggregate_state_rows",
        "output_rows",
        "stage_rows",
        "retained_stage_rows",
    ]
}
/// Logical state rows of a checkpoint; blank where the runner records no snapshot.
fn state_fields(snapshot: Option<&Snapshot>) -> Vec<String> {
    match snapshot {
        None => vec![String::new(); state_header().len()],
        Some(s) => vec![
            s.joined_rows.to_string(),
            s.arrangements
                .iter()
                .map(|a| a.entries)
                .sum::<u64>()
                .to_string(),
            s.aggregate_state_rows.to_string(),
            s.output_rows.to_string(),
            s.stage_rows.to_string(),
            s.retained_stage_rows.to_string(),
        ],
    }
}

fn space_csv(derived: &Derived) -> Csv {
    let mut columns = header(&CELL_COLUMNS, &["seed", "repeat", "round", "phase"]);
    for name in [
        "base_only_allocated_bytes",
        "ordinary_recompute_allocated_bytes",
        "covering_allocated_bytes",
        "ivmlite_allocated_bytes",
        "ordinary_recompute_over_base_only",
        "covering_over_base_only",
        "ivmlite_over_base_only",
        "base_only_live_page_bytes",
        "ordinary_recompute_live_page_bytes",
        "covering_live_page_bytes",
        "ivmlite_live_page_bytes",
    ] {
        columns.push(name.to_owned());
    }
    columns.extend(state_header().map(str::to_owned));
    let mut csv = with_header(columns);
    let order = [
        Mode::NoMaintenance,
        Mode::Recompute,
        Mode::CoveringRecompute,
        Mode::Ivmlite,
    ];
    for case in &derived.runs {
        for point in &case.space {
            let snapshot = case
                .correctness
                .iter()
                .find(|c| c.phase == point.phase && c.round == point.round)
                .map(|c| &c.snapshot);
            let allocated = order.map(|m| point.pages[m.label()].allocated_bytes.to_string());
            let live = order.map(|m| point.pages[m.label()].live_page_bytes.to_string());
            csv.line(
                joined(
                    cell_fields(&case.manifest.cell),
                    [
                        case.key.seed.to_string(),
                        case.key.repeat.to_string(),
                        optn(point.round),
                        point.phase.clone(),
                    ],
                )
                .into_iter()
                .chain(allocated)
                .chain([
                    opt(point.over_base_only(Mode::Recompute)),
                    opt(point.over_base_only(Mode::CoveringRecompute)),
                    opt(point.over_base_only(Mode::Ivmlite)),
                ])
                .chain(live)
                .chain(state_fields(snapshot)),
            );
        }
    }
    csv
}

fn targets_csv(derived: &Derived) -> Csv {
    let mut csv = with_header(header(
        &CELL_COLUMNS,
        &[
            "seed",
            "repeat",
            "round",
            "target_selection",
            "operations",
            "target_selections",
            "hot_anchor_selections",
            "hot_target_share",
            "target_groups",
            "max_target_group_selections",
            "distinct_primary_keys",
            "repeated_primary_key_touches",
            "repeated_existing_targets",
            "matched_partners_min",
            "matched_partners_max",
            "matched_partners_total",
            "rhs_matched_lhs_rows",
            "rhs_matched_hot_lhs_rows",
            "matched_hot_row_share",
            "rhs_affected_group_visits",
            "rhs_hot_group_visits",
            "matched_hot_group_share",
            "rows_changed",
            "initial_hot_group_row_share",
        ],
    ));
    let share = |n: usize, d: usize| opt(ratio(n as u64, d as u64));
    for case in &derived.runs {
        for t in &case.targets {
            csv.line(joined(
                cell_fields(&case.manifest.cell),
                [
                    case.key.seed.to_string(),
                    case.key.repeat.to_string(),
                    t.round.to_string(),
                    case.manifest.target_selection.clone(),
                    t.operations.to_string(),
                    t.target_selections.to_string(),
                    t.hot_anchor_selections.to_string(),
                    share(t.hot_anchor_selections, t.target_selections),
                    t.target_groups.len().to_string(),
                    t.target_groups.values().max().unwrap_or(&0).to_string(),
                    t.distinct_primary_keys.to_string(),
                    t.repeated_primary_key_touches.to_string(),
                    t.repeated_existing_targets.to_string(),
                    t.matched_partners_min.to_string(),
                    t.matched_partners_max.to_string(),
                    t.matched_partners_total.to_string(),
                    t.rhs_matched_lhs_rows.to_string(),
                    t.rhs_matched_hot_lhs_rows.to_string(),
                    share(t.rhs_matched_hot_lhs_rows, t.rhs_matched_lhs_rows),
                    t.rhs_affected_group_visits.to_string(),
                    t.rhs_hot_group_visits.to_string(),
                    share(t.rhs_hot_group_visits, t.rhs_affected_group_visits),
                    t.per_operation
                        .iter()
                        .map(|o| o.rows_changed)
                        .sum::<u64>()
                        .to_string(),
                    num(case.initial_hot_share),
                ],
            ));
        }
    }
    csv
}

fn churn_csv(derived: &Derived) -> Csv {
    let mut columns = header(
        &CELL_COLUMNS,
        &[
            "seed",
            "rounds",
            "phase",
            "round",
            "page_count",
            "freelist_count",
            "allocated_bytes",
            "live_page_bytes",
        ],
    );
    columns.extend(state_header().map(str::to_owned));
    let mut csv = with_header(columns);
    for case in &derived.churns {
        for checkpoint in &case.correctness {
            let pages = &checkpoint.snapshot.pages;
            csv.line(
                joined(
                    cell_fields(&case.manifest.cell),
                    [
                        case.manifest.seed.to_string(),
                        case.manifest.rounds.to_string(),
                        checkpoint.phase.clone(),
                        optn(checkpoint.round),
                        pages.page_count.to_string(),
                        pages.freelist_count.to_string(),
                        pages.allocated_bytes.to_string(),
                        pages.live_page_bytes.to_string(),
                    ],
                )
                .into_iter()
                .chain(state_fields(Some(&checkpoint.snapshot))),
            );
        }
    }
    csv
}

fn losses_csv(derived: &Derived) -> Csv {
    let mut csv = Csv::new(&["kind", "cell", "seed", "repeat", "phase", "error"]);
    for loss in &derived.losses {
        csv.line([
            loss.kind.clone(),
            loss.key.cell.clone(),
            loss.key.seed.to_string(),
            loss.key.repeat.to_string(),
            loss.phase.clone(),
            loss.error.clone(),
        ]);
    }
    csv
}

const DEFINITIONS: &str = "\
- A run's total for a mode is the median over the five measured rounds of that round's \
apply+maintain+read sum, never a sum of component medians. Apply+maintain is the median of each \
round's apply+maintain sum.
- Fastest complete recompute of a run is the smaller of the `recompute` (ordinary) and \
`covering_recompute` totals of that same run. `no_maintenance` is never a numerator or a \
denominator of a speedup; its apply time is reported only as the write lower bound.
- Ratio = fastest-recompute total / ivmlite total, per run (seed). Per cell: min, median and max \
over the cell's complete runs; a cell with fewer complete runs than planned is `partial`. Seeds \
measure fixture sensitivity; they are not repeated trials or confidence bounds.
- Confirmation candidate: a cell whose [min, max] ratio range contains 1.0 or 2.0, or whose median \
is within 10% of 1.0 or 2.0.
- Space: per checkpoint, ivmlite / base-only and covering / base-only allocated-byte ratios at the \
same checkpoint key (base-only is the `no_maintenance` database), live-page proxies, logical state \
rows and `retained_stage_rows`. Page allocation is not RSS or peak memory.
- Targets: top-10% group row share of the initial fixture, realized hot target share, and the \
hot share of the groups matched by each batch, plus distinct primary keys, repeated touches, \
matched partners per batch and target-group counts.
- Target selection (`target_selection` = `group_then_row`): existing-row targets are chosen \
group-first (group by profile, then a uniform live row in it); uniform group selection is not \
uniform row selection.
- Churn: page_count, freelist_count and logical state rows per round, per profile.
- A case without `completed.json`, or whose completed artifacts cannot be read or no longer match \
their completion hashes, is listed in `losses.csv` with its phase and error and contributes to no \
ratio. Phase `interrupted` is a started case that left neither a completion marker nor a failure \
record; `not_run` is a planned case that never started.
- Confirmation (`confirmations.csv`): timing-only paired repeats of each candidate cell at seed 42 \
on an idle host (1-minute load average recorded before and after each repeat; the planned repeat \
count is `expected_repeats`). A repeat whose load after the run exceeds the `--max-load` that \
admitted it is `busy after`: it stays in the table, is flagged, and is counted per cell. \
`initial_build_differs` / `other build` names the fields (compiler, host) in which the initial \
run was made differently; a different source, runner, extension or SQLite is refused at run \
time. Ratios use the same definition as above. They are reported separately and never enter `timing.csv` or \
`ratios.csv`; the cell row gives the full range of the repeats and whether it still contains 1.0 or \
2.0, next to the initial seed range for comparison.
- Smoke selections are reported on one line and are never mixed into the matrix tables.
";

/// Distinct values of one provenance field over all cases, flagged when they differ and, when
/// `alarm` is `(value, label)`, when any case has that value.
fn provenance_line(name: &str, values: BTreeSet<String>, alarm: Option<(&str, &str)>) -> String {
    let mut flags = Vec::new();
    if values.len() > 1 {
        flags.push("MIXED");
    }
    if let Some((value, label)) = alarm {
        if values.contains(value) {
            flags.push(label);
        }
    }
    let flag = if flags.is_empty() {
        String::new()
    } else {
        format!(" ({})", flags.join(", "))
    };
    let list: Vec<_> = values.into_iter().collect();
    format!("- {name}{flag}: {}\n", list.join("; "))
}

fn smoke_gate_label(gate: SmokeGate) -> &'static str {
    match gate {
        SmokeGate::NotApplicable => "not_applicable",
        SmokeGate::Passed => "passed",
        SmokeGate::Skipped => "skipped",
    }
}

/// Whether `git status --porcelain` lists a change to a tracked file; untracked files (`??`) do
/// not change the source that ran.
fn has_tracked_changes(status: &str) -> bool {
    status
        .lines()
        .any(|line| !line.trim().is_empty() && !line.starts_with("??"))
}

fn smoke_line(out: &Path) -> Result<String> {
    let path = selection_file(out, "run", true);
    if !path.exists() {
        return Ok("Smoke selection: none.\n".into());
    }
    let selection: Selection = read_json(&path)?;
    let s = &selection.summary;
    Ok(format!(
        "Smoke selection (reported only here, never mixed into any table): {} planned, {} complete, {} failed.\n",
        s.planned,
        s.complete,
        s.failed.len()
    ))
}

fn summary_md(out: &Path, derived: &Derived, cells: &[CellRatios]) -> Result<String> {
    let mut md = String::from("# M2a report\n\n## Definitions\n\n");
    md.push_str(DEFINITIONS);
    let _ = write!(
        md,
        "\n## Completeness\n\n{} cases complete, {} listed in `losses.csv`.\n{}\n",
        derived.complete,
        derived.losses.len(),
        smoke_line(out)?
    );
    md.push_str("\n## Provenance of the run, churn and confirm cases\n\n");
    let manifests: Vec<&Manifest> = derived
        .runs
        .iter()
        .map(|c| &c.manifest)
        .chain(derived.churns.iter().map(|c| &c.manifest))
        .chain(derived.confirmations.iter().map(|c| &c.manifest))
        .collect();
    let field = |f: &dyn Fn(&Manifest) -> String| -> BTreeSet<String> {
        manifests.iter().map(|m| f(m)).collect()
    };
    for (name, values, alarm) in [
        (
            "source revision",
            field(&|m| m.source_revision.clone()),
            None,
        ),
        ("source sha256", field(&|m| m.source_sha256.clone()), None),
        ("runner sha256", field(&|m| m.runner_sha256.clone()), None),
        (
            "extension sha256",
            field(&|m| m.extension_sha256.clone().unwrap_or_default()),
            None,
        ),
        ("config sha256", field(&|m| m.config_sha256.clone()), None),
        ("SQLite", field(&|m| m.sqlite_version.clone()), None),
        ("rustc", field(&|m| m.runner_rustc.clone()), None),
        ("host", field(&|m| m.hardware.host.clone()), None),
        ("cpu", field(&|m| m.hardware.cpu.clone()), None),
        ("memory", field(&|m| m.hardware.memory.clone()), None),
        (
            "logical cpus",
            field(&|m| m.hardware.logical_cpus.to_string()),
            None,
        ),
        (
            "smoke gate",
            field(&|m| smoke_gate_label(m.provenance.smoke_gate).to_owned()),
            Some(("skipped", "SKIPPED")),
        ),
        (
            "tracked source changes",
            field(&|m| {
                if has_tracked_changes(&m.provenance.worktree_status) {
                    "yes"
                } else {
                    "no"
                }
                .to_owned()
            }),
            Some(("yes", "DIRTY")),
        ),
    ] {
        md.push_str(&provenance_line(name, values, alarm));
    }
    md.push_str(
        "\n## Ratio against the fastest complete recompute\n\n\
         | cell | runs | min | median | max | candidate | partial | ivmlite bootstrap ms | ivmlite/base-only bytes (final idle) |\n\
         |---|---|---|---|---|---|---|---|---|\n",
    );
    for c in cells {
        let n = |value: Option<f64>| value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.3}"));
        let stats = c.stats.as_ref();
        let flag = match (stats, c.partial()) {
            (None, _) => "no result",
            (_, true) => "partial",
            _ => "",
        };
        let _ = writeln!(
            md,
            "| {} | {}/{} | {} | {} | {} | {} | {} | {} | {} |",
            c.id,
            c.runs,
            c.expected_runs,
            n(stats.map(|s| s.min)),
            n(stats.map(|s| s.median)),
            n(stats.map(|s| s.max)),
            stats.map_or("n/a", |s| if s.candidate { "yes" } else { "no" }),
            flag,
            n(stats.map(|s| s.ivmlite_bootstrap_ms)),
            n(c.final_space_ratio),
        );
    }
    let with = |keep: &dyn Fn(&RatioStats) -> bool| {
        cells
            .iter()
            .filter_map(|c| c.stats.as_ref())
            .filter(|s| keep(s))
            .count()
    };
    let _ = write!(
        md,
        "\n## Does 2x survive\n\n{} of {} cells exceed 2x in every complete run; {} more exceed it in some run; {} never do; {} have no result (every run lost). Cells below 1.0 in every run: {}.\n",
        with(&|s| s.min > 2.0),
        cells.len(),
        with(&|s| s.max > 2.0 && s.min <= 2.0),
        with(&|s| s.max <= 2.0),
        cells.iter().filter(|c| c.stats.is_none()).count(),
        with(&|s| s.max < 1.0)
    );
    Ok(md)
}

/// Per confirmed cell: the range of its repeats against 1.0 and 2.0, beside the initial range.
fn confirmation_md(derived: &Derived, cells: &[CellRatios]) -> String {
    let mut md = String::from("\n## Confirmation of candidate cells\n\n");
    if derived.confirm_expected.is_empty() {
        md.push_str("No confirmation run.\n");
        return md;
    }
    md.push_str(
        "| cell | repeats | min | median | max | straddles 1x | straddles 2x | initial min | initial max | busy after | other build |\n\
         |---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let n = |value: Option<f64>| value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.3}"));
    for (id, &expected) in &derived.confirm_expected {
        let repeats: Vec<&ConfirmCase> = derived
            .confirmations
            .iter()
            .filter(|c| &c.key.cell == id)
            .collect();
        let ratios: Vec<f64> = repeats.iter().map(|c| c.ratio.ratio).collect();
        let stats = Summary::of(&ratios);
        let straddles = |threshold: f64| {
            stats.as_ref().map_or("n/a", |s| {
                if s.min <= threshold && threshold <= s.max {
                    "yes"
                } else {
                    "no"
                }
            })
        };
        let initial = cells
            .iter()
            .find(|c| &c.id == id)
            .and_then(|c| c.stats.as_ref());
        let _ = writeln!(
            md,
            "| {id} | {}/{expected} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            ratios.len(),
            n(stats.as_ref().map(|s| s.min)),
            n(stats.as_ref().map(|s| s.median)),
            n(stats.as_ref().map(|s| s.max)),
            straddles(1.0),
            straddles(2.0),
            n(initial.map(|s| s.min)),
            n(initial.map(|s| s.max)),
            repeats.iter().filter(|c| c.busy_after()).count(),
            build_differs(repeats.iter().copied()),
        );
    }
    md
}

/// Writes `timing.csv`, `ratios.csv`, `confirm-candidates.json`, `confirmations.csv`, `space.csv`,
/// `targets.csv`, `churn.csv`, `losses.csv` and `summary.md` to `dest`, derived from the case directories that the
/// non-smoke selections of `out` plan.
pub fn report(out: &Path, dest: &Path) -> Result<ReportSummary> {
    let derived = derive(out)?;
    let cells = cell_ratios(&derived)?;
    let candidates: Vec<CaseKey> = cells
        .iter()
        .filter(|c| c.stats.as_ref().is_some_and(|s| s.candidate))
        .map(|c| CaseKey {
            cell: c.id.clone(),
            seed: CONFIRM_SEED,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    fs::create_dir_all(dest)?;
    for (name, csv) in [
        ("timing.csv", timing_csv(&derived)),
        ("ratios.csv", ratios_csv(&derived, &cells)),
        ("confirmations.csv", confirmations_csv(&derived, &cells)),
        ("space.csv", space_csv(&derived)),
        ("targets.csv", targets_csv(&derived)),
        ("churn.csv", churn_csv(&derived)),
        ("losses.csv", losses_csv(&derived)),
    ] {
        fs::write(dest.join(name), csv.text)?;
    }
    write_json(&dest.join("confirm-candidates.json"), &candidates)?;
    let mut summary = summary_md(out, &derived, &cells)?;
    summary.push_str(&confirmation_md(&derived, &cells));
    fs::write(dest.join("summary.md"), summary)?;
    Ok(ReportSummary {
        complete: derived.complete,
        incomplete: derived.losses.len(),
        candidates,
        best_ratio: cells
            .iter()
            .filter_map(|c| c.stats.as_ref().map(|s| s.median))
            .max_by(f64::total_cmp),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use ivmlite_workload::m2a::{Cell, M2aConfig, Profile, Shape, Table};
    use serde_json::json;

    use super::*;
    use crate::m2a_runner::{
        steady_state, Checkpoint, Measurement, Mode, OperationStats, Plans, RoundTiming,
        TargetStats, MODES,
    };
    use crate::m2a_space::SpaceCheckpoint;
    use crate::m2a_state::PageMetrics;
    use crate::{
        case_path, complete, selection_file, write_json, BuildIdentity, ExpectedCase, FailedCase,
        Hardware, Manifest, ModeProvenance, Provenance, Selection, SelectionIdentity, SmokeGate,
        Summary, TimingFile, CONFIRM_REPEAT_ARTIFACTS, INVERSE_ARTIFACTS, RUN_ARTIFACTS,
    };

    const ROUNDS: usize = 5;
    type Triple = (f64, f64, f64);

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "m2a-report-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn cell(profile: Profile, fanout: usize, batch_size: usize) -> Cell {
        Cell {
            shape: Shape::Join,
            profile,
            base_rows: 64,
            group_cardinality: 20,
            fanout,
            batch_size,
        }
    }
    fn pages(page_count: u64, freelist_count: u64) -> PageMetrics {
        PageMetrics {
            page_size: 4096,
            page_count,
            freelist_count,
            allocated_bytes: 4096 * page_count,
            live_page_bytes: 4096 * (page_count - freelist_count),
        }
    }
    fn round(n: usize, (apply, maintain, read): Triple, maintained: bool) -> RoundTiming {
        RoundTiming {
            round: n,
            apply_ms: apply,
            maintain_ms: maintained.then_some(maintain),
            read_ms: maintained.then_some(read),
            apply_plus_maintain_ms: maintained.then_some(apply + maintain),
            total_ms: maintained.then_some(apply + maintain + read),
        }
    }
    fn measurement(mode: Mode, bootstrap_ms: f64, triples: [Triple; ROUNDS]) -> Measurement {
        let maintained = mode != Mode::NoMaintenance;
        let measured: Vec<_> = triples
            .iter()
            .enumerate()
            .map(|(n, &t)| round(n + 1, t, maintained))
            .collect();
        Measurement {
            mode,
            bootstrap_ms,
            warmup: round(0, triples[0], maintained),
            summary: steady_state(&measured).unwrap(),
            measured,
            base_pages: pages(10, 0),
            bootstrap_pages: pages(12, 0),
            final_pages: pages(14, 1),
            plan: Vec::new(),
        }
    }
    fn flat(total: f64) -> [Triple; ROUNDS] {
        [(total / 2.0, total / 4.0, total / 4.0); ROUNDS]
    }
    /// Per-mode round triples of one run.
    struct Run {
        no_maintenance_apply: f64,
        ordinary: [Triple; ROUNDS],
        covering: [Triple; ROUNDS],
        ivmlite: [Triple; ROUNDS],
    }
    fn run_of(ordinary: f64, covering: f64, ivmlite: f64) -> Run {
        Run {
            no_maintenance_apply: 0.5,
            ordinary: flat(ordinary),
            covering: flat(covering),
            ivmlite: flat(ivmlite),
        }
    }
    fn hardware() -> Hardware {
        Hardware {
            host: "host".into(),
            cpu: "cpu".into(),
            memory: "memory".into(),
            logical_cpus: 1,
        }
    }
    fn provenance() -> Provenance {
        Provenance {
            invocation_id: "test".into(),
            command: vec!["m2a".into()],
            started_unix_ms: 1,
            started_utc: "start".into(),
            start_load: "idle".into(),
            extension: None,
            worktree_status: String::new(),
            hardware: hardware(),
            smoke_gate: SmokeGate::NotApplicable,
        }
    }
    fn manifest(kind: &str, cell: &Cell, seed: u64, rounds: usize, order: &[Mode]) -> Manifest {
        Manifest {
            schema_version: 5,
            kind: kind.into(),
            cell: cell.clone(),
            seed,
            repeat: 0,
            rounds,
            smoke: false,
            fixture_sha256: "fixture".into(),
            initial_data_sha256: "data".into(),
            trace_sha256: "trace".into(),
            sql_sha256: "sql".into(),
            view_sql: "SELECT 1".into(),
            schema_ddl: String::new(),
            ordinary_index_ddl: String::new(),
            covering_index_ddl: String::new(),
            config_sha256: "config".into(),
            source_revision: "revision".into(),
            source_sha256: "source".into(),
            extension_sha256: Some("extension".into()),
            runner_sha256: "runner".into(),
            sqlite_version: "3.0".into(),
            runner_rustc: "rustc".into(),
            hardware: hardware(),
            pragmas: BTreeMap::new(),
            target_selection: "group_then_row".into(),
            plans: Plans {
                view_sql_reference: String::new(),
                ordinary_recompute: String::new(),
                covering_recompute: String::new(),
                dml: BTreeMap::new(),
            },
            mode_order: order.iter().map(|m| m.label().to_owned()).collect(),
            provenance: provenance(),
            load_before: None,
            load_after: None,
            max_load: None,
            initial_run: None,
        }
    }
    fn snapshot(joined_rows: u64, histogram: &[(i64, u64)], pages: &PageMetrics) -> Checkpoint {
        // Built through serde, as the runner's artifact is read back.
        let snapshot = json!({
            "base_rows": {"lhs": 64, "rhs": 64},
            "joined_rows": joined_rows,
            "live_output_groups": histogram.len(),
            "group_histogram": histogram.iter().map(|&(g, rows)| json!({"group": g, "rows": rows})).collect::<Vec<_>>(),
            "arrangements": [
                {"table": "a", "entries": 7, "weight_total": 7},
                {"table": "b", "entries": 5, "weight_total": 5}
            ],
            "aggregate_state_rows": 3,
            "output_rows": 3,
            "output_weight_total": 3,
            "pending": [],
            "stage_rows": 4,
            "retained_stage_rows": 4,
            "stage_operation_counts": {"apply": 1},
            "pages": pages,
        });
        Checkpoint {
            phase: String::new(),
            round: None,
            snapshot: serde_json::from_value(snapshot).unwrap(),
        }
    }
    fn keyed(mut checkpoint: Checkpoint, phase: &str, round: Option<usize>) -> Checkpoint {
        checkpoint.phase = phase.into();
        checkpoint.round = round;
        checkpoint
    }
    /// Bootstrap histogram: group 0 and 1 (the hot 10% of 20 groups) hold 40 of 64 rows.
    fn correctness(rounds: usize) -> Vec<Checkpoint> {
        let histogram = [(0, 24), (1, 16), (2, 24)];
        let mut all = vec![keyed(
            snapshot(100, &histogram, &pages(12, 0)),
            "bootstrap",
            None,
        )];
        for n in 0..rounds {
            for phase in ["written", "refreshed", "idle"] {
                all.push(keyed(
                    snapshot(100 + n as u64, &histogram, &pages(13 + n as u64, n as u64)),
                    phase,
                    Some(n),
                ));
            }
        }
        all
    }
    fn target_stats(round: usize) -> TargetStats {
        TargetStats {
            round,
            operations: 10,
            target_selections: 8,
            hot_anchor_selections: 6,
            distinct_primary_keys: 9,
            repeated_primary_key_touches: 2,
            repeated_existing_targets: 1,
            target_groups: BTreeMap::from([(0, 5), (3, 2)]),
            rhs_matched_lhs_rows: 40,
            rhs_matched_hot_lhs_rows: 30,
            rhs_affected_group_visits: 20,
            rhs_hot_group_visits: 5,
            matched_partners_min: 1,
            matched_partners_max: 8,
            matched_partners_total: 50,
            per_operation: vec![OperationStats {
                index: 0,
                table: Table::Rhs,
                primary_key: 1,
                kind: "update".into(),
                anchor_group: Some(0),
                matched_partners: 8,
                matched_hot_lhs_rows: 6,
                matched_distinct_groups: 4,
                matched_hot_groups: 1,
                rows_changed: 1,
            }],
        }
    }
    /// Allocated pages per mode at every checkpoint of one round: no maintenance 10, ordinary 14,
    /// covering 20, ivmlite 30 pages.
    fn space() -> Vec<SpaceCheckpoint> {
        let mut points = Vec::new();
        let keys = [
            (None, "base"),
            (None, "bootstrap"),
            (Some(0), "written"),
            (Some(0), "refreshed"),
            (Some(0), "idle"),
        ];
        for mode in MODES {
            let count = match mode {
                Mode::NoMaintenance => 10,
                Mode::Recompute => 14,
                Mode::CoveringRecompute => 20,
                Mode::Ivmlite => 30,
            };
            for (round, phase) in keys {
                points.push(SpaceCheckpoint {
                    mode,
                    round,
                    phase: phase.into(),
                    pages: pages(count, 1),
                });
            }
        }
        points
    }
    fn key(kind: &str, cell: &Cell, seed: u64) -> ExpectedCase {
        ExpectedCase {
            kind: kind.into(),
            cell: cell.id(),
            seed,
            repeat: 0,
            rounds: ROUNDS + 1,
        }
    }
    fn config() -> M2aConfig {
        M2aConfig::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m2a.toml"))
            .unwrap()
    }
    const ORDER: [Mode; 4] = [
        Mode::NoMaintenance,
        Mode::Recompute,
        Mode::CoveringRecompute,
        Mode::Ivmlite,
    ];
    fn timing_file(manifest: &Manifest, run: &Run) -> TimingFile {
        let measurements = vec![
            measurement(
                Mode::NoMaintenance,
                0.0,
                [(run.no_maintenance_apply, 0.0, 0.0); ROUNDS],
            ),
            measurement(Mode::Recompute, 1.5, run.ordinary),
            measurement(Mode::CoveringRecompute, 2.5, run.covering),
            measurement(Mode::Ivmlite, 9.0, run.ivmlite),
        ];
        TimingFile {
            manifest: manifest.clone(),
            finished_unix_ms: 2,
            finish_load: "idle".into(),
            mode_provenance: ORDER
                .iter()
                .map(|m| ModeProvenance {
                    mode: m.label().into(),
                    started_unix_ms: 1,
                    start_load: "idle".into(),
                    finished_unix_ms: 2,
                    finish_load: "idle".into(),
                })
                .collect(),
            measurements,
        }
    }
    /// Writes the artifacts of one `run` case; `finished` adds the completion marker.
    fn write_run_case(out: &Path, cell: &Cell, seed: u64, run: &Run, finished: bool) {
        write_run_case_with(out, cell, seed, run, finished, &config());
    }
    fn write_run_case_with(
        out: &Path,
        cell: &Cell,
        seed: u64,
        run: &Run,
        finished: bool,
        config: &M2aConfig,
    ) {
        let key = key("run", cell, seed);
        let dir = case_path(out, "run", &key);
        fs::create_dir_all(&dir).unwrap();
        let manifest = manifest("run", cell, seed, key.rounds, &ORDER);
        let timing = timing_file(&manifest, run);
        write_json(&dir.join("manifest.json"), &manifest).unwrap();
        // The real fixture file also holds the trace; the report reads only its configuration.
        write_json(&dir.join("fixture.json"), &json!({ "config": config })).unwrap();
        write_json(&dir.join("timing.json"), &timing).unwrap();
        write_json(&dir.join("space.json"), &space()).unwrap();
        write_json(
            &dir.join("targets.json"),
            &[target_stats(0), target_stats(1)],
        )
        .unwrap();
        write_json(&dir.join("correctness.json"), &correctness(1)).unwrap();
        if finished {
            complete(&dir, "manifest.json", "completed.json", &RUN_ARTIFACTS).unwrap();
        }
    }
    fn confirm_key(cell: &Cell, repeat: usize) -> ExpectedCase {
        ExpectedCase {
            repeat,
            ..key("confirm", cell, 42)
        }
    }
    /// A timing-only confirmation repeat: manifest and timing, no fixture, correctness or space.
    fn write_confirm_case(out: &Path, cell: &Cell, repeat: usize, run: &Run, finished: bool) {
        let key = confirm_key(cell, repeat);
        let dir = case_path(out, "confirm", &key);
        fs::create_dir_all(&dir).unwrap();
        let mut manifest = manifest("confirm", cell, 42, key.rounds, &ORDER);
        manifest.repeat = repeat;
        manifest.load_before = Some(0.25);
        manifest.load_after = Some(0.5);
        manifest.max_load = Some(2.0);
        manifest.initial_run = Some(BuildIdentity::of(&manifest));
        write_json(&dir.join("manifest.json"), &manifest).unwrap();
        write_json(&dir.join("timing.json"), &timing_file(&manifest, run)).unwrap();
        if finished {
            complete(
                &dir,
                "manifest.json",
                "completed.json",
                &CONFIRM_REPEAT_ARTIFACTS,
            )
            .unwrap();
        }
    }
    fn write_churn_case(out: &Path, cell: &Cell, rounds: usize) {
        let key = ExpectedCase {
            rounds,
            ..key("churn", cell, 42)
        };
        let dir = case_path(out, "churn", &key);
        fs::create_dir_all(&dir).unwrap();
        write_json(
            &dir.join("manifest.json"),
            &manifest("churn", cell, 42, rounds, &[]),
        )
        .unwrap();
        write_json(&dir.join("fixture.json"), &json!({ "config": config() })).unwrap();
        write_json(&dir.join("correctness.json"), &correctness(rounds)).unwrap();
        write_json(&dir.join("targets.json"), &[target_stats(0)]).unwrap();
        complete(&dir, "manifest.json", "completed.json", &INVERSE_ARTIFACTS).unwrap();
    }
    /// Edits a finished case's manifest and binds the completion marker to the edit.
    fn rewrite_manifest(
        out: &Path,
        action: &str,
        key: &ExpectedCase,
        artifacts: &[&str],
        edit: impl FnOnce(&mut Manifest),
    ) {
        let dir = case_path(out, action, key);
        let mut manifest: Manifest = crate::read_json(&dir.join("manifest.json")).unwrap();
        edit(&mut manifest);
        write_json(&dir.join("manifest.json"), &manifest).unwrap();
        complete(&dir, "manifest.json", "completed.json", artifacts).unwrap();
    }
    fn write_selection(
        out: &Path,
        action: &str,
        smoke: bool,
        expected: BTreeSet<ExpectedCase>,
        failed: Vec<FailedCase>,
    ) {
        let selection = Selection {
            identity: SelectionIdentity {
                schema_version: 1,
                kind: action.into(),
                config_sha256: "config".into(),
                source_sha256: "source".into(),
                runner_sha256: "runner".into(),
                extension_sha256: None,
                smoke,
                hardware: hardware(),
            },
            summary: Summary {
                planned: expected.len() as u64,
                complete: 0,
                failed,
            },
            expected,
            invocations: Vec::new(),
        };
        write_json(&selection_file(out, action, smoke), &selection).unwrap();
    }
    /// One run case per `(cell, seed, run, finished)`, with the matching run selection.
    fn matrix(out: &Path, cases: &[(&Cell, u64, Run, bool)]) {
        let mut expected = BTreeSet::new();
        for (cell, seed, run, finished) in cases {
            write_run_case(out, cell, *seed, run, *finished);
            expected.insert(key("run", cell, *seed));
        }
        write_selection(out, "run", false, expected, Vec::new());
    }
    fn table(path: &Path) -> Vec<BTreeMap<String, String>> {
        let text = fs::read_to_string(path).unwrap();
        let mut lines = text.lines();
        let header: Vec<_> = lines
            .next()
            .unwrap()
            .split(',')
            .map(str::to_owned)
            .collect();
        lines
            .map(|line| {
                let fields: Vec<_> = line.split(',').map(str::to_owned).collect();
                assert_eq!(fields.len(), header.len(), "{line}");
                header.iter().cloned().zip(fields).collect()
            })
            .collect()
    }
    fn number(row: &BTreeMap<String, String>, column: &str) -> f64 {
        row[column]
            .parse()
            .unwrap_or_else(|_| panic!("{column}: {row:?}"))
    }
    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
    fn run_rows(dest: &Path) -> Vec<BTreeMap<String, String>> {
        table(&dest.join("ratios.csv"))
            .into_iter()
            .filter(|row| row["scope"] == "run")
            .collect()
    }
    fn cell_rows(dest: &Path) -> Vec<BTreeMap<String, String>> {
        table(&dest.join("ratios.csv"))
            .into_iter()
            .filter(|row| row["scope"] == "cell")
            .collect()
    }

    #[test]
    fn ratio_uses_the_faster_recompute_mode_per_run() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        // Seed 1: ordinary 10 beats covering 12. Seed 42: covering 8 beats ordinary 20.
        matrix(
            &out,
            &[
                (&cell, 1, run_of(10.0, 12.0, 5.0), true),
                (&cell, 42, run_of(20.0, 8.0, 5.0), true),
            ],
        );
        report(&out, &dest).unwrap();
        let rows = run_rows(&dest);
        let by_seed = |seed: &str| rows.iter().find(|row| row["seed"] == seed).unwrap();
        let first = by_seed("1");
        assert_eq!(first["fastest_recompute_mode"], "recompute");
        near(number(first, "fastest_recompute_ms"), 10.0);
        near(number(first, "ratio"), 2.0);
        let second = by_seed("42");
        assert_eq!(second["fastest_recompute_mode"], "covering_recompute");
        near(number(second, "fastest_recompute_ms"), 8.0);
        near(number(second, "ratio"), 1.6);
        let cells = cell_rows(&dest);
        assert_eq!(cells.len(), 1);
        near(number(&cells[0], "ratio_min"), 1.6);
        near(number(&cells[0], "ratio_max"), 2.0);
        near(number(&cells[0], "ratio_median"), 1.8);
    }

    #[test]
    fn totals_are_median_of_round_sums() {
        let rounds = [
            (1.0, 1.0, 1.0),
            (1.0, 1.0, 10.0),
            (10.0, 1.0, 1.0),
            (1.0, 10.0, 1.0),
            (2.0, 2.0, 2.0),
        ];
        let totals = mode_totals(&measurement(Mode::Ivmlite, 3.0, rounds)).unwrap();
        // Round sums are 3, 12, 12, 12, 6: median 12. The median of each component is 1, 1, 1.
        near(totals.total_ms.unwrap(), 12.0);
        // Apply+maintain sums are 2, 2, 11, 11, 4: median 4.
        near(totals.apply_plus_maintain_ms.unwrap(), 4.0);
        near(totals.apply_ms, 1.0);
        near(totals.bootstrap_ms, 3.0);
        assert!((totals.total_ms.unwrap() - 3.0).abs() > 1.0);

        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::UniformUniform, 8, 10);
        let mut run = run_of(24.0, 30.0, 12.0);
        run.ivmlite = rounds;
        matrix(&out, &[(&cell, 1, run, true)]);
        report(&out, &dest).unwrap();
        near(number(&run_rows(&dest)[0], "ivmlite_ms"), 12.0);
        near(number(&run_rows(&dest)[0], "ratio"), 2.0);
        let timing = table(&dest.join("timing.csv"));
        let ivmlite = timing.iter().find(|row| row["mode"] == "ivmlite").unwrap();
        near(number(ivmlite, "total_ms"), 12.0);
        near(number(ivmlite, "apply_plus_maintain_ms"), 4.0);
        let none = timing
            .iter()
            .find(|row| row["mode"] == "no_maintenance")
            .unwrap();
        assert_eq!(none["total_ms"], "");
        assert_eq!(none["maintain_ms"], "");
    }

    #[test]
    fn no_maintenance_is_never_in_a_speedup() {
        let ratio_with = |no_maintenance_apply: f64| {
            let out = temp();
            let dest = out.join("report");
            let cell = cell(Profile::UniformUniform, 8, 10);
            let mut run = run_of(10.0, 12.0, 5.0);
            run.no_maintenance_apply = no_maintenance_apply;
            matrix(&out, &[(&cell, 1, run, true)]);
            report(&out, &dest).unwrap();
            let row = run_rows(&dest).remove(0);
            assert_eq!(row["fastest_recompute_mode"], "recompute");
            near(
                number(&row, "no_maintenance_apply_ms"),
                no_maintenance_apply,
            );
            number(&row, "ratio")
        };
        // Fastest of all modes (0.1 ms per round) and slowest of all (1000 ms).
        near(ratio_with(0.1), 2.0);
        near(ratio_with(1000.0), 2.0);
    }

    #[test]
    fn candidate_rule_selects_straddles_and_near_misses() {
        for (ratios, expected) in [
            ([0.9, 1.1, 1.0], true),
            ([1.95, 2.05, 2.0], true),
            ([3.0, 3.1, 3.2], false),
            ([1.08, 1.09, 1.09], true),
            ([1.5, 1.5, 1.6], false),
            // The median is far from both thresholds but the range straddles them.
            ([0.5, 1.5, 3.0], true),
        ] {
            let out = temp();
            let dest = out.join("report");
            let cell = cell(Profile::ZipfUniform, 8, 10);
            // The fastest recompute total is 10, so the ivmlite total is 10 / ratio.
            let cases: Vec<_> = [1, 42, 2026]
                .into_iter()
                .zip(ratios)
                .map(|(seed, ratio)| (&cell, seed, run_of(10.0, 20.0, 10.0 / ratio), true))
                .collect();
            matrix(&out, &cases);
            let summary = report(&out, &dest).unwrap();
            let cells = cell_rows(&dest);
            assert_eq!(cells[0]["candidate"], expected.to_string(), "{ratios:?}");
            assert_eq!(
                summary.candidates.len(),
                usize::from(expected),
                "{ratios:?}"
            );
            let listed: Vec<CaseKey> =
                serde_json::from_slice(&fs::read(dest.join("confirm-candidates.json")).unwrap())
                    .unwrap();
            assert_eq!(listed, summary.candidates);
            if expected {
                assert_eq!(
                    listed,
                    [CaseKey {
                        cell: cell.id(),
                        seed: 42
                    }]
                );
            }
        }
    }

    #[test]
    fn report_lists_confirmations_separately() {
        let out = temp();
        let dest = out.join("report");
        let straddling = cell(Profile::ZipfUniform, 8, 10);
        let clear = cell(Profile::ZipfHot, 8, 10);
        // Fastest recompute total is 10, so the ivmlite total is 10 / ratio.
        let at = |ratio: f64| run_of(10.0, 20.0, 10.0 / ratio);
        let mut cases: Vec<_> = [(1, 1.8), (42, 2.2), (2026, 2.0)]
            .into_iter()
            .map(|(seed, ratio)| (&straddling, seed, at(ratio), true))
            .collect();
        cases.extend([1, 42, 2026].map(|seed| (&clear, seed, at(5.0), true)));
        matrix(&out, &cases);
        assert_eq!(report(&out, &dest).unwrap().candidates.len(), 1);
        let seed_tables = ["timing.csv", "ratios.csv"].map(|n| fs::read(dest.join(n)).unwrap());
        assert!(fs::read_to_string(dest.join("summary.md"))
            .unwrap()
            .contains("No confirmation run."));
        assert!(table(&dest.join("confirmations.csv")).is_empty());

        // Four planned repeats of the candidate: three finish, one is lost.
        for (repeat, ratio) in [(0, 1.9), (1, 2.05), (2, 2.3)] {
            write_confirm_case(&out, &straddling, repeat, &at(ratio), true);
        }
        write_confirm_case(&out, &straddling, 3, &at(9.0), false);
        write_selection(
            &out,
            "confirm",
            false,
            (0..4).map(|r| confirm_key(&straddling, r)).collect(),
            vec![FailedCase {
                cell: straddling.id(),
                seed: 42,
                repeat: 3,
                phase: "host_not_idle".into(),
                error: "load".into(),
            }],
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!(summary.complete, 6 + 3);
        assert_eq!(summary.incomplete, 1);
        assert_eq!(
            summary.candidates.len(),
            1,
            "confirmations never add candidates"
        );

        let rows = table(&dest.join("confirmations.csv"));
        let repeats: Vec<_> = rows.iter().filter(|r| r["scope"] == "repeat").collect();
        assert_eq!(repeats.len(), 3);
        for (row, ratio) in repeats.iter().zip([1.9, 2.05, 2.3]) {
            assert_eq!(row["cell"], straddling.id());
            near(number(row, "ratio"), ratio);
            near(number(row, "ivmlite_ms"), 10.0 / ratio);
            assert_eq!(row["fastest_recompute_mode"], "recompute");
            assert_eq!(
                (row["load_before"].as_str(), row["load_after"].as_str()),
                ("0.25", "0.5")
            );
        }
        let range: Vec<_> = rows.iter().filter(|r| r["scope"] == "cell").collect();
        assert_eq!(range.len(), 1, "only the confirmed cell is listed");
        let range = range[0];
        assert_eq!(range["cell"], straddling.id());
        near(number(range, "ratio_min"), 1.9);
        near(number(range, "ratio_median"), 2.05);
        near(number(range, "ratio_max"), 2.3);
        assert_eq!(
            (
                range["straddles_1x"].as_str(),
                range["straddles_2x"].as_str()
            ),
            ("false", "true")
        );
        assert_eq!(
            (
                range["repeats"].as_str(),
                range["expected_repeats"].as_str()
            ),
            ("3", "4")
        );
        assert_eq!(range["partial"], "true");
        near(number(range, "initial_ratio_min"), 1.8);
        near(number(range, "initial_ratio_max"), 2.2);

        let seed_tables_after =
            ["timing.csv", "ratios.csv"].map(|n| fs::read(dest.join(n)).unwrap());
        assert_eq!(
            seed_tables, seed_tables_after,
            "confirmations stay out of the seed tables"
        );
        let losses = fs::read_to_string(dest.join("losses.csv")).unwrap();
        assert!(losses.contains("confirm,"), "{losses}");
        assert!(losses.contains("host_not_idle"), "{losses}");
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("## Confirmation of candidate cells"));
        assert!(text.contains(&format!("| {} | 3/4 |", straddling.id())));
    }
    #[test]
    fn incomplete_case_is_a_loss_not_a_ratio() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(
            &out,
            &[
                (&cell, 1, run_of(10.0, 12.0, 5.0), true),
                (&cell, 42, run_of(10.0, 12.0, 4.0), true),
                (&cell, 2026, run_of(10.0, 12.0, 1.0), false),
            ],
        );
        write_selection(
            &out,
            "run",
            false,
            [1, 42, 2026].map(|seed| key("run", &cell, seed)).into(),
            vec![FailedCase {
                cell: cell.id(),
                seed: 2026,
                repeat: 0,
                phase: "timing_ivmlite".into(),
                error: "boom, with a comma".into(),
            }],
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (2, 1));
        let cells = cell_rows(&dest);
        assert_eq!(cells[0]["runs"], "2");
        assert_eq!(cells[0]["expected_runs"], "3");
        assert_eq!(cells[0]["partial"], "true");
        near(number(&cells[0], "ratio_min"), 2.0);
        near(number(&cells[0], "ratio_max"), 2.5);
        assert!(run_rows(&dest).iter().all(|row| row["seed"] != "2026"));
        let losses = fs::read_to_string(dest.join("losses.csv")).unwrap();
        assert!(losses.contains("run,"));
        assert!(losses.contains(",2026,0,timing_ivmlite,\"boom, with a comma\""));
        assert_eq!(losses.lines().count(), 2);
        let timing = table(&dest.join("timing.csv"));
        assert!(timing.iter().all(|row| row["seed"] != "2026"));
    }

    /// A matrix cell with a finished run per seed, `repeats` finished confirmation repeats at
    /// seed 42, and one finished churn case, with their selections.
    fn confirmed_out(out: &Path, cell: &Cell, repeats: usize) {
        let at = |ratio: f64| run_of(10.0, 20.0, 10.0 / ratio);
        matrix(
            out,
            &[
                (cell, 1, at(1.8), true),
                (cell, 42, at(2.2), true),
                (cell, 2026, at(2.0), true),
            ],
        );
        for repeat in 0..repeats {
            write_confirm_case(out, cell, repeat, &at(2.0), true);
        }
        write_selection(
            out,
            "confirm",
            false,
            (0..repeats).map(|r| confirm_key(cell, r)).collect(),
            Vec::new(),
        );
        write_churn_case(out, cell, 3);
        write_selection(
            out,
            "churn",
            false,
            BTreeSet::from([ExpectedCase {
                rounds: 3,
                ..key("churn", cell, 42)
            }]),
            Vec::new(),
        );
    }
    fn summary_text(dest: &Path) -> String {
        fs::read_to_string(dest.join("summary.md")).unwrap()
    }
    fn appended_space(path: &Path) {
        fs::write(path, format!("{} ", fs::read_to_string(path).unwrap())).unwrap();
    }

    #[test]
    fn a_completed_artifact_edited_after_completion_is_a_loss() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        confirmed_out(&out, &cell, 2);
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (3 + 2 + 1, 0));
        // Each edit keeps the file valid JSON, so only the completion hash can notice it.
        let churn_key = ExpectedCase {
            rounds: 3,
            ..key("churn", &cell, 42)
        };
        appended_space(&case_path(&out, "run", &key("run", &cell, 1)).join("timing.json"));
        appended_space(&case_path(&out, "confirm", &confirm_key(&cell, 0)).join("timing.json"));
        appended_space(&case_path(&out, "churn", &churn_key).join("correctness.json"));
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (3, 3));
        let losses = fs::read_to_string(dest.join("losses.csv")).unwrap();
        for kind in ["run", "confirm", "churn"] {
            let line = losses
                .lines()
                .find(|l| l.starts_with(&format!("{kind},")))
                .unwrap_or_else(|| panic!("{kind}: {losses}"));
            assert!(line.contains("artifact_read"), "{line}");
            assert!(line.contains("hash differs"), "{line}");
        }
        assert!(run_rows(&dest).iter().all(|row| row["seed"] != "1"));
        let repeats = table(&dest.join("confirmations.csv"));
        assert_eq!(repeats.iter().filter(|r| r["scope"] == "repeat").count(), 1);
    }

    #[test]
    fn a_case_started_but_never_finished_is_interrupted_not_unstarted() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(
            &out,
            &[
                (&cell, 1, run_of(10.0, 12.0, 5.0), true),
                // Killed after its manifest and artifacts were written: no marker, no failure.
                (&cell, 42, run_of(10.0, 12.0, 5.0), false),
            ],
        );
        write_selection(
            &out,
            "run",
            false,
            [1, 42, 2026].map(|seed| key("run", &cell, seed)).into(),
            Vec::new(),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (1, 2));
        let phases: BTreeMap<String, String> = fs::read_to_string(dest.join("losses.csv"))
            .unwrap()
            .lines()
            .skip(1)
            .map(|line| {
                let f: Vec<_> = line.split(',').collect();
                (f[2].to_owned(), f[4].to_owned())
            })
            .collect();
        assert_eq!(phases["42"], "interrupted");
        assert_eq!(phases["2026"], "not_run");
    }

    #[test]
    fn a_repeat_on_a_busy_host_is_published_and_flagged() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfUniform, 8, 10);
        confirmed_out(&out, &cell, 3);
        // The host got busy after the second repeat (limit 2.0, closing load 3.5).
        rewrite_manifest(
            &out,
            "confirm",
            &confirm_key(&cell, 1),
            &CONFIRM_REPEAT_ARTIFACTS,
            |m| m.load_after = Some(3.5),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!(
            summary.incomplete, 0,
            "a busy repeat is flagged, not dropped"
        );
        let rows = table(&dest.join("confirmations.csv"));
        let repeats: Vec<_> = rows.iter().filter(|r| r["scope"] == "repeat").collect();
        assert_eq!(repeats.len(), 3);
        let busy: Vec<_> = repeats.iter().map(|r| r["busy_after"].as_str()).collect();
        assert_eq!(busy, ["false", "true", "false"]);
        assert!(repeats.iter().all(|r| r["max_load"] == "2"));
        let range = rows.iter().find(|r| r["scope"] == "cell").unwrap();
        assert_eq!(range["busy_after_repeats"], "1");
        assert_eq!(range["repeats"], "3");
        let text = summary_text(&dest);
        let confirmation = text
            .split("## Confirmation of candidate cells")
            .nth(1)
            .unwrap();
        let line = confirmation
            .lines()
            .find(|l| l.starts_with(&format!("| {} | 3/3 |", cell.id())))
            .unwrap();
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        assert_eq!(cells[10], "1", "busy-after count of the cell: {line}");
        assert!(text.contains("`busy after`"), "defined in the summary");
    }

    #[test]
    fn a_repeat_made_by_another_compiler_or_host_is_flagged() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfUniform, 8, 10);
        confirmed_out(&out, &cell, 2);
        rewrite_manifest(
            &out,
            "confirm",
            &confirm_key(&cell, 0),
            &CONFIRM_REPEAT_ARTIFACTS,
            |m| {
                let initial = m.initial_run.as_mut().unwrap();
                initial.runner_rustc = "other rustc".into();
                initial.hardware.cpu = "other cpu".into();
            },
        );
        report(&out, &dest).unwrap();
        let rows = table(&dest.join("confirmations.csv"));
        let repeats: Vec<_> = rows.iter().filter(|r| r["scope"] == "repeat").collect();
        assert_eq!(repeats[0]["initial_build_differs"], "hardware;runner_rustc");
        assert_eq!(repeats[1]["initial_build_differs"], "");
        let range = rows.iter().find(|r| r["scope"] == "cell").unwrap();
        assert_eq!(range["initial_build_differs"], "hardware;runner_rustc");
        let text = summary_text(&dest);
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("| {} | 2/2 |", cell.id())))
            .unwrap();
        assert!(line.contains("hardware;runner_rustc"), "{line}");
    }

    #[test]
    fn provenance_covers_churn_and_confirm_manifests() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfUniform, 8, 10);
        confirmed_out(&out, &cell, 1);
        assert!(summary_text(&{
            report(&out, &dest).unwrap();
            dest.clone()
        })
        .contains("- extension sha256: extension\n"));
        rewrite_manifest(
            &out,
            "confirm",
            &confirm_key(&cell, 0),
            &CONFIRM_REPEAT_ARTIFACTS,
            |m| m.extension_sha256 = Some("other extension".into()),
        );
        let churn_key = ExpectedCase {
            rounds: 3,
            ..key("churn", &cell, 42)
        };
        rewrite_manifest(&out, "churn", &churn_key, &INVERSE_ARTIFACTS, |m| {
            m.source_sha256 = "other source".into();
            m.hardware.host = "other host".into();
            m.hardware.memory = "other memory".into();
        });
        report(&out, &dest).unwrap();
        let text = summary_text(&dest);
        assert!(
            text.contains("- extension sha256 (MIXED): extension; other extension\n"),
            "{text}"
        );
        assert!(
            text.contains("- source sha256 (MIXED): other source; source\n"),
            "{text}"
        );
        assert!(
            text.contains("- host (MIXED): host; other host\n"),
            "{text}"
        );
        assert!(
            text.contains("- memory (MIXED): memory; other memory\n"),
            "{text}"
        );
        assert!(text.contains("- cpu: cpu\n"), "{text}");
    }

    #[test]
    fn provenance_reports_the_smoke_gate_and_a_dirty_worktree() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(
            &out,
            &[
                (&cell, 1, run_of(10.0, 12.0, 5.0), true),
                (&cell, 42, run_of(10.0, 12.0, 5.0), true),
            ],
        );
        report(&out, &dest).unwrap();
        let text = summary_text(&dest);
        assert!(text.contains("- smoke gate: not_applicable\n"), "{text}");
        assert!(text.contains("- tracked source changes: no\n"), "{text}");
        // Untracked files (for example this tooling's own notes) are not source changes.
        rewrite_manifest(&out, "run", &key("run", &cell, 1), &RUN_ARTIFACTS, |m| {
            m.provenance.worktree_status = "?? notes/\n".into();
        });
        report(&out, &dest).unwrap();
        assert!(summary_text(&dest).contains("- tracked source changes: no\n"));
        rewrite_manifest(&out, "run", &key("run", &cell, 42), &RUN_ARTIFACTS, |m| {
            m.provenance.smoke_gate = SmokeGate::Skipped;
            m.provenance.worktree_status = " M crates/x.rs\n?? notes/\n".into();
        });
        report(&out, &dest).unwrap();
        let text = summary_text(&dest);
        assert!(
            text.contains("- smoke gate (MIXED, SKIPPED): not_applicable; skipped\n"),
            "{text}"
        );
        assert!(
            text.contains("- tracked source changes (MIXED, DIRTY): no; yes\n"),
            "{text}"
        );
    }

    #[test]
    fn space_ratio_pairs_same_checkpoint_keys() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(10.0, 12.0, 5.0), true)]);
        report(&out, &dest).unwrap();
        let rows = table(&dest.join("space.csv"));
        let keys: Vec<_> = rows
            .iter()
            .map(|row| (row["round"].as_str(), row["phase"].as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("", "base"),
                ("", "bootstrap"),
                ("0", "written"),
                ("0", "refreshed"),
                ("0", "idle")
            ]
        );
        for row in &rows {
            near(number(row, "base_only_allocated_bytes"), 10.0 * 4096.0);
            near(number(row, "ivmlite_allocated_bytes"), 30.0 * 4096.0);
            near(number(row, "covering_allocated_bytes"), 20.0 * 4096.0);
            near(number(row, "ivmlite_over_base_only"), 3.0);
            near(number(row, "covering_over_base_only"), 2.0);
            near(number(row, "ivmlite_live_page_bytes"), 29.0 * 4096.0);
        }
        // The logical state comes from the correctness checkpoint with the same key.
        assert_eq!(rows[0]["arrangement_entries"], "");
        assert_eq!(rows[1]["arrangement_entries"], "12");
        assert_eq!(rows[4]["retained_stage_rows"], "4");
        assert_eq!(rows[4]["joined_rows"], "100");
    }

    #[test]
    fn targets_table_reports_hot_shares_of_the_initial_fixture_and_each_batch() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(10.0, 12.0, 5.0), true)]);
        report(&out, &dest).unwrap();
        let rows = table(&dest.join("targets.csv"));
        assert_eq!(rows.len(), 2);
        // Groups 0 and 1 are the hot 10% of 20 groups and hold 40 of 64 initial rows.
        near(number(&rows[0], "initial_hot_group_row_share"), 40.0 / 64.0);
        near(number(&rows[0], "hot_target_share"), 6.0 / 8.0);
        near(number(&rows[0], "matched_hot_group_share"), 5.0 / 20.0);
        near(number(&rows[0], "matched_hot_row_share"), 30.0 / 40.0);
        assert_eq!(rows[1]["round"], "1");
        assert_eq!(rows[0]["rows_changed"], "1");
        // The section 3 metrics and the selection label come with every row.
        assert_eq!(rows[0]["target_selection"], "group_then_row");
        for (column, expected) in [
            ("distinct_primary_keys", "9"),
            ("repeated_primary_key_touches", "2"),
            ("repeated_existing_targets", "1"),
            ("matched_partners_min", "1"),
            ("matched_partners_max", "8"),
            ("matched_partners_total", "50"),
            ("target_groups", "2"),
            ("max_target_group_selections", "5"),
        ] {
            assert_eq!(rows[0][column], expected, "{column}");
        }
    }

    #[test]
    fn churn_table_lists_page_and_state_trajectory_per_profile() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(10.0, 12.0, 5.0), true)]);
        write_churn_case(&out, &cell, 3);
        write_selection(
            &out,
            "churn",
            false,
            BTreeSet::from([ExpectedCase {
                rounds: 3,
                ..key("churn", &cell, 42)
            }]),
            Vec::new(),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (2, 0));
        let rows = table(&dest.join("churn.csv"));
        assert_eq!(rows.len(), 1 + 3 * 3);
        assert!(rows.iter().all(|row| row["profile"] == "zipf_hot"));
        let last = rows.last().unwrap();
        assert_eq!(
            (last["phase"].as_str(), last["round"].as_str()),
            ("idle", "2")
        );
        assert_eq!(last["page_count"], "15");
        assert_eq!(last["freelist_count"], "2");
        assert_eq!(last["joined_rows"], "102");
        assert_eq!(last["arrangement_entries"], "12");
    }

    #[test]
    fn smoke_selection_never_reaches_the_matrix_tables() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(10.0, 12.0, 5.0), true)]);
        let smoke_cell = Cell {
            base_rows: 8,
            ..cell.clone()
        };
        write_run_case(&out, &smoke_cell, 1, &run_of(10.0, 12.0, 1.0), true);
        write_selection(
            &out,
            "run",
            true,
            BTreeSet::from([key("run", &smoke_cell, 1)]),
            Vec::new(),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!(summary.complete, 1);
        assert_eq!(cell_rows(&dest).len(), 1);
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("Smoke"));
        assert!(text.contains("never mixed"));
        assert!(!fs::read_to_string(dest.join("ratios.csv"))
            .unwrap()
            .contains(&smoke_cell.id()));
    }

    #[test]
    fn unreadable_artifact_of_a_completed_case_is_a_loss() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(10.0, 12.0, 5.0), true)]);
        let dir = case_path(&out, "run", &key("run", &cell, 1));
        fs::write(dir.join("targets.json"), "{").unwrap();
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (0, 1));
        let losses = fs::read_to_string(dest.join("losses.csv")).unwrap();
        assert!(losses.contains("artifact_read"));
        // The cell stays in the tables as a cell without a result.
        let cells = cell_rows(&dest);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0]["cell"], cell.id());
        assert_eq!(cells[0]["profile"], "zipf_hot");
        assert_eq!(cells[0]["runs"], "0");
        assert_eq!(cells[0]["expected_runs"], "1");
        assert_eq!(cells[0]["partial"], "true");
        assert_eq!(cells[0]["ratio_min"], "");
        assert_eq!(cells[0]["ratio_median"], "");
        assert_eq!(cells[0]["candidate"], "false");
        assert!(run_rows(&dest).is_empty());
        assert!(summary.candidates.is_empty() && summary.best_ratio.is_none());
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("| no result |"));
        assert!(text.contains("0 of 1 cells exceed 2x"));
        assert!(text.contains("1 have no result"));
    }

    #[test]
    fn a_cell_with_every_run_unstarted_is_still_listed() {
        let out = temp();
        let dest = out.join("report");
        let finished = cell(Profile::ZipfHot, 8, 10);
        let unstarted = cell(Profile::UniformHot, 8, 10);
        matrix(&out, &[(&finished, 1, run_of(10.0, 12.0, 5.0), true)]);
        write_selection(
            &out,
            "run",
            false,
            [key("run", &finished, 1), key("run", &unstarted, 1)].into(),
            Vec::new(),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (1, 1));
        let cells = cell_rows(&dest);
        assert_eq!(cells.len(), 2);
        let lost = cells.iter().find(|r| r["cell"] == unstarted.id()).unwrap();
        // No manifest exists, so only the id is known.
        assert_eq!((lost["runs"].as_str(), lost["profile"].as_str()), ("0", ""));
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("of 2 cells"));
    }

    #[test]
    fn hot_share_follows_the_fixture_configuration() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        let mut wide = config();
        wide.hot_group_fraction = 0.25;
        let run = run_of(10.0, 12.0, 5.0);
        write_run_case_with(&out, &cell, 1, &run, true, &wide);
        write_selection(
            &out,
            "run",
            false,
            BTreeSet::from([key("run", &cell, 1)]),
            Vec::new(),
        );
        report(&out, &dest).unwrap();
        // 25% of 20 groups is 5 hot groups, which hold all three populated groups.
        near(
            number(
                &table(&dest.join("targets.csv"))[0],
                "initial_hot_group_row_share",
            ),
            1.0,
        );
    }

    #[test]
    fn measured_round_count_must_match_the_fixture_configuration() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        let mut short = config();
        short.measured_rounds = 4;
        write_run_case_with(&out, &cell, 1, &run_of(10.0, 12.0, 5.0), true, &short);
        write_selection(
            &out,
            "run",
            false,
            BTreeSet::from([key("run", &cell, 1)]),
            Vec::new(),
        );
        let summary = report(&out, &dest).unwrap();
        assert_eq!((summary.complete, summary.incomplete), (0, 1));
        let losses = fs::read_to_string(dest.join("losses.csv")).unwrap();
        assert!(losses.contains("artifact_read"));
        assert!(losses.contains("5 measured rounds, the fixture configures 4"));
    }

    #[test]
    fn the_near_band_is_symmetric_at_its_exact_edge() {
        for edge in [0.9, 1.1, 1.8, 2.2] {
            assert!(is_candidate(edge, edge, edge), "{edge}");
        }
        for outside in [0.89, 1.11, 1.79, 2.21, 3.0] {
            assert!(!is_candidate(outside, outside, outside), "{outside}");
        }
    }

    #[test]
    fn mixed_configurations_are_flagged() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(
            &out,
            &[
                (&cell, 1, run_of(10.0, 12.0, 5.0), true),
                (&cell, 42, run_of(10.0, 12.0, 5.0), true),
            ],
        );
        report(&out, &dest).unwrap();
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("- config sha256: config\n"));
        rewrite_manifest(&out, "run", &key("run", &cell, 42), &RUN_ARTIFACTS, |m| {
            m.config_sha256 = "other".into();
        });
        report(&out, &dest).unwrap();
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        assert!(text.contains("- config sha256 (MIXED): config; other\n"));
    }

    #[test]
    fn summary_states_the_definitions_and_the_best_ratio() {
        let out = temp();
        let dest = out.join("report");
        let cell = cell(Profile::ZipfHot, 8, 10);
        matrix(&out, &[(&cell, 1, run_of(30.0, 12.0, 4.0), true)]);
        let summary = report(&out, &dest).unwrap();
        near(summary.best_ratio.unwrap(), 3.0);
        let text = fs::read_to_string(dest.join("summary.md")).unwrap();
        for phrase in [
            "median over the five measured rounds",
            "Fastest complete recompute",
            "Confirmation candidate",
            "no_maintenance",
            "existing-row targets are chosen group-first (group by profile, then a uniform live row in it); uniform group selection is not uniform row selection",
        ] {
            assert!(text.contains(phrase), "{phrase}");
        }
    }
}
