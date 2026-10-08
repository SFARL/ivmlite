//! The shared runner of the demand-backed demo benchmarks
//! (`docs/demos/README.md`): FluxFlow (`examples/fluxflow_demo.rs`) and the
//! noop, Zcash and Kener cases (`examples/demand_case_bench.rs`).
//!
//! It follows the Phase 4 benchmark protocol
//! (`docs/superpowers/specs/2026-09-28-m1b-phase4-benchmark-design.md` §3):
//! the release extension is refused when missing or stale unless
//! `--extension <path>` names one (§3.1); every mode gets a fresh in-memory
//! database, its initial and final states are verified outside the timers,
//! and every statement a timer covers is prepared before it (§3.2); the mode
//! order rotates across repeats (§3.3); and every median is reported with its
//! min–max (§3.5).
//!
//! One addition to §3.2: between the initial verification and the timed
//! batch, an untimed warm-up batch is applied and maintained. Its maintenance
//! is reported on its own as `first_refresh_ms`, because ivmlite's first
//! refresh after `CREATE` also drains the stage its bootstrap left behind, a
//! one-time cost that is not steady-state refresh (`docs/demos/README.md`).

use std::cmp::Ordering;
use std::error::Error;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{Connection, Statement};

use crate::{extension_library_for, open_with_extension_at, Profile};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The batch that warms each mode up; its maintenance is `first_refresh_ms`.
pub const WARM_UP_ROUND: usize = 0;
/// The batch whose apply, maintenance and read are the steady-state timings.
pub const MEASURED_ROUND: usize = 1;
/// How many mixed batches one run applies; workloads validate against it.
pub const ROUNDS: usize = 2;

/// The `ivmlite-sqlite` manifest, whose version names the extension's sources.
const EXTENSION_MANIFEST: &str = include_str!("../../ivmlite-sqlite/Cargo.toml");

// ---------------------------------------------------------------------------
// Loading the extension (Phase 4 spec §3.1)
// ---------------------------------------------------------------------------

/// The extension library a run loads, and whether it was checked against
/// its sources.
#[derive(Debug, Clone)]
pub struct Extension {
    path: PathBuf,
    checked: bool,
}

impl Extension {
    /// `explicit` (from `--extension`) is used as given, skipping the
    /// staleness check so one binary can time different builds (Phase 4
    /// spec §3.1). Otherwise the release library is used, and refused when it
    /// is missing or older than any of its sources.
    pub fn resolve(explicit: Option<PathBuf>) -> std::result::Result<Self, String> {
        match explicit {
            Some(path) => Ok(Self {
                path,
                checked: false,
            }),
            None => extension_library_for(Profile::Release)
                .map(|path| Self {
                    path,
                    checked: true,
                })
                .map_err(|why| {
                    format!(
                        "{why}; for the demo benchmarks, `cargo build --release --locked \
                         --manifest-path crates/ivmlite-sqlite/Cargo.toml` builds only the \
                         extension, and `--extension <path>` loads another build unchecked"
                    )
                }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A fresh in-memory database with this extension loaded.
    pub fn open(&self) -> rusqlite::Result<Connection> {
        open_with_extension_at(None, &self.path)
    }

    /// The library's path, relative to the repository when it lies inside it.
    fn display_path(&self) -> String {
        let canonical = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let root = std::fs::canonicalize(repo()).unwrap_or_else(|_| repo());
        canonical
            .strip_prefix(&root)
            .unwrap_or(&canonical)
            .display()
            .to_string()
    }

    fn describe(&self) -> String {
        if self.checked {
            format!(
                "{}; built from ivmlite-sqlite {}; staleness check passed",
                self.display_path(),
                extension_source_version()
            )
        } else {
            format!(
                "{}; given by --extension, staleness check skipped, so its version is \
                 not verified (this tree's ivmlite-sqlite is {})",
                self.display_path(),
                extension_source_version()
            )
        }
    }
}

/// The `version` of `crates/ivmlite-sqlite/Cargo.toml`.
fn extension_source_version() -> &'static str {
    EXTENSION_MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix("version = "))
        .map_or("unknown", |quoted| quoted.trim_matches('"'))
}

/// Removes `--extension <path>` from `args`, returning the path (if given)
/// and the remaining arguments in order.
pub fn take_extension_flag(
    args: Vec<String>,
) -> std::result::Result<(Option<PathBuf>, Vec<String>), String> {
    let mut explicit = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--extension" {
            let path = args.next().ok_or("--extension needs a path")?;
            if explicit.replace(PathBuf::from(path)).is_some() {
                return Err("--extension may be given only once".into());
            }
        } else {
            rest.push(arg);
        }
    }
    Ok((explicit, rest))
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

pub fn create_view(c: &Connection, name: &str, sql: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {name} USING ivm('{}')",
        sql.replace('\'', "''")
    ))
}

/// The view's refresh command, prepared so a timer never compiles it.
pub fn prepare_refresh<'c>(c: &'c Connection, name: &str) -> rusqlite::Result<Statement<'c>> {
    c.prepare(&format!("INSERT INTO {name}({name}) VALUES ('refresh')"))
}

/// Steps `statement` to the end, returning how many rows it produced.
pub fn drain(statement: &mut Statement<'_>) -> rusqlite::Result<usize> {
    let mut rows = statement.query([])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok(count)
}

/// SQLite's total in-memory page allocation, in KiB.
pub fn database_kib(c: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = c.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = c.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages * page_size / 1024)
}

// ---------------------------------------------------------------------------
// The multiset oracle
// ---------------------------------------------------------------------------

/// A total order on SQLite values that never equates different storage
/// classes: the integer 5 and the real 5.0 are different results here, as
/// they are to a reader of the view.
pub fn compare_values(left: &Value, right: &Value) -> Ordering {
    fn class(value: &Value) -> u8 {
        match value {
            Value::Null => 0,
            Value::Integer(_) => 1,
            Value::Real(_) => 2,
            Value::Text(_) => 3,
            Value::Blob(_) => 4,
        }
    }
    match (left, right) {
        (Value::Integer(l), Value::Integer(r)) => l.cmp(r),
        (Value::Real(l), Value::Real(r)) => l.total_cmp(r),
        (Value::Text(l), Value::Text(r)) => l.cmp(r),
        (Value::Blob(l), Value::Blob(r)) => l.cmp(r),
        _ => class(left).cmp(&class(right)),
    }
}

fn compare_rows(left: &[Value], right: &[Value]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(l, r)| compare_values(l, r))
        .find(|order| order.is_ne())
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

/// Every row `sql` returns, duplicates kept, sorted by `compare_values`, so
/// two results compare as multisets.
pub fn sorted_rows(c: &Connection, sql: &str) -> rusqlite::Result<Vec<Vec<Value>>> {
    let mut statement = c.prepare(sql)?;
    let columns = statement.column_count();
    let mut rows: Vec<Vec<Value>> = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, Value>(column))
                .collect()
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.sort_by(|l, r| compare_rows(l, r));
    Ok(rows)
}

/// `None` when two sorted multisets are equal; otherwise their first
/// difference.
pub fn multiset_difference(actual: &[Vec<Value>], expected: &[Vec<Value>]) -> Option<String> {
    let first = actual
        .iter()
        .zip(expected)
        .position(|(a, e)| compare_rows(a, e).is_ne());
    match first {
        None if actual.len() == expected.len() => None,
        None => Some(format!(
            "{} rows, expected {}",
            actual.len(),
            expected.len()
        )),
        Some(i) => Some(format!(
            "{} rows, expected {}; first difference at sorted row {i}: {:?}, expected {:?}",
            actual.len(),
            expected.len(),
            actual[i],
            expected[i]
        )),
    }
}

/// `Ok` when `actual_sql` returns the same multiset of rows as `oracle_sql`.
pub fn check_same_multiset(
    c: &Connection,
    actual_sql: &str,
    oracle_sql: &str,
) -> std::result::Result<(), String> {
    let actual = sorted_rows(c, actual_sql).map_err(|e| e.to_string())?;
    let expected = sorted_rows(c, oracle_sql).map_err(|e| e.to_string())?;
    match multiset_difference(&actual, &expected) {
        None => Ok(()),
        Some(why) => Err(format!(
            "`{actual_sql}` diverged from `{oracle_sql}`: {why}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// The median and range of one metric over the repeats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub median: f64,
    pub min: f64,
    pub max: f64,
}

impl Summary {
    /// `None` for no values. An even count's median is the mean of the two
    /// middle values.
    pub fn of(values: &[f64]) -> Option<Self> {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let (&min, &max) = (sorted.first()?, sorted.last()?);
        let middle = sorted.len() / 2;
        let median = if sorted.len() % 2 == 1 {
            sorted[middle]
        } else {
            (sorted[middle - 1] + sorted[middle]) / 2.0
        };
        Some(Self { median, min, max })
    }
}

// ---------------------------------------------------------------------------
// Workloads and modes
// ---------------------------------------------------------------------------

/// What a mode does at bootstrap and at maintenance (`docs/demos/README.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Base writes only: the floor.
    NoMaintenance,
    /// SQLite drains the view's query over the table with no extra index.
    UnindexedRecompute,
    /// As `UnindexedRecompute`, over a covering index on the grouping and
    /// aggregated columns; the index is built at bootstrap and maintained
    /// inside the timed writes.
    IndexedRecompute,
    /// A rollup table kept by hand-written triggers inside the writes.
    HandwrittenTrigger,
    /// ivmlite's capture triggers during the writes, then explicit refresh.
    Ivmlite,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::NoMaintenance => "no_maintenance",
            Mode::UnindexedRecompute => "unindexed_recompute",
            Mode::IndexedRecompute => "indexed_recompute",
            Mode::HandwrittenTrigger => "handwritten_trigger",
            Mode::Ivmlite => "ivmlite",
        }
    }
}

/// `modes` rotated left by `repeat`, the order of one repeat's runs
/// (Phase 4 spec §3.3, applied across repeats as in §3.5).
pub fn rotated(modes: &[Mode], repeat: usize) -> Vec<Mode> {
    let mut order = modes.to_vec();
    if !order.is_empty() {
        let shift = repeat % order.len();
        order.rotate_left(shift);
    }
    order
}

/// Applies one round of a workload's mixed batch with its prepared
/// statements: `(statements, base_rows, batch, round)`. Different rounds
/// touch different rows, so the warm-up never repeats the measured batch.
pub type ApplyFn = fn(&mut [Statement<'_>], usize, usize, usize) -> rusqlite::Result<()>;

/// A mixed batch's statements, prepared before any timer (Phase 4 spec §3.2
/// step 4), with its transaction's `BEGIN` and `COMMIT`.
pub struct PreparedBatch<'c> {
    begin: Statement<'c>,
    commit: Statement<'c>,
    steps: Vec<Statement<'c>>,
    apply: ApplyFn,
}

impl<'c> PreparedBatch<'c> {
    pub fn prepare(c: &'c Connection, sql: &[&str], apply: ApplyFn) -> rusqlite::Result<Self> {
        Ok(Self {
            begin: c.prepare("BEGIN")?,
            commit: c.prepare("COMMIT")?,
            steps: sql
                .iter()
                .map(|statement| c.prepare(statement))
                .collect::<rusqlite::Result<_>>()?,
            apply,
        })
    }

    /// Runs round `round` of the batch as one transaction.
    pub fn execute(
        &mut self,
        base_rows: usize,
        batch: usize,
        round: usize,
    ) -> rusqlite::Result<()> {
        self.begin.execute([])?;
        (self.apply)(&mut self.steps, base_rows, batch, round)?;
        self.commit.execute([])?;
        Ok(())
    }
}

/// A hand-written trigger baseline: installs its rollup (bootstrap) and
/// names the query that reads it back in the view's column order.
#[derive(Clone, Copy)]
pub struct Handwritten {
    pub install: fn(&Connection) -> rusqlite::Result<()>,
    pub read_sql: &'static str,
}

/// One demo: its schema, data, maintained view and mixed batch.
pub trait Workload {
    fn name(&self) -> &'static str;
    fn ddl(&self) -> &'static str;
    fn view_name(&self) -> &'static str;
    fn view_sql(&self) -> &'static str;
    /// A covering index on the view's grouping and aggregated columns, for
    /// `Mode::IndexedRecompute`.
    fn covering_index_sql(&self) -> &'static str;
    fn handwritten(&self) -> Option<Handwritten> {
        None
    }
    /// `Ok` when `rounds` batches of `batch` operations fit `rows` base rows
    /// without touching a row twice or violating a constraint.
    fn validate(&self, rows: usize, batch: usize, rounds: usize)
        -> std::result::Result<(), String>;
    fn seed(&self, c: &Connection, rows: usize) -> rusqlite::Result<()>;
    /// The statements of the mixed batch, in the order `apply_fn` indexes them.
    fn batch_sql(&self) -> &'static [&'static str];
    fn apply_fn(&self) -> ApplyFn;

    /// Prepares and applies one round of the mixed batch, for untimed use.
    fn apply_mixed_batch(
        &self,
        c: &Connection,
        rows: usize,
        batch: usize,
        round: usize,
    ) -> rusqlite::Result<()> {
        PreparedBatch::prepare(c, self.batch_sql(), self.apply_fn())?.execute(rows, batch, round)
    }

    /// The modes this workload is timed in, in output order.
    fn modes(&self) -> Vec<Mode> {
        let mut modes = vec![
            Mode::NoMaintenance,
            Mode::UnindexedRecompute,
            Mode::IndexedRecompute,
        ];
        if self.handwritten().is_some() {
            modes.push(Mode::HandwrittenTrigger);
        }
        modes.push(Mode::Ivmlite);
        modes
    }
}

// ---------------------------------------------------------------------------
// One run
// ---------------------------------------------------------------------------

/// One run's timings, in milliseconds, and its space.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub bootstrap_ms: f64,
    /// The maintenance after the warm-up batch: for ivmlite, the first
    /// refresh after `CREATE`, including the bootstrap's leftover stage.
    pub first_refresh_ms: f64,
    pub apply_ms: f64,
    pub maintain_ms: f64,
    pub read_ms: f64,
    pub database_kib: f64,
}

impl Timing {
    fn apply_plus_maintain_ms(&self) -> f64 {
        self.apply_ms + self.maintain_ms
    }

    fn end_to_end_ms(&self) -> f64 {
        self.apply_ms + self.maintain_ms + self.read_ms
    }
}

type Metric = (&'static str, fn(&Timing) -> f64, usize);

/// The CSV's metrics: name, value, decimals.
const METRICS: [Metric; 8] = [
    ("bootstrap_ms", |t| t.bootstrap_ms, 3),
    ("first_refresh_ms", |t| t.first_refresh_ms, 3),
    ("apply_ms", |t| t.apply_ms, 3),
    ("maintain_ms", |t| t.maintain_ms, 3),
    ("read_ms", |t| t.read_ms, 3),
    ("apply_plus_maintain_ms", Timing::apply_plus_maintain_ms, 3),
    ("end_to_end_ms", Timing::end_to_end_ms, 3),
    ("database_kib", |t| t.database_kib, 1),
];

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

/// The maintenance step of a mode, prepared before the timers.
enum Maintenance<'c> {
    Nothing,
    Refresh(Statement<'c>),
    Recompute(Statement<'c>),
}

impl Maintenance<'_> {
    fn run(&mut self) -> rusqlite::Result<()> {
        match self {
            Maintenance::Nothing => {}
            Maintenance::Refresh(refresh) => {
                refresh.execute([])?;
            }
            Maintenance::Recompute(query) => {
                black_box(drain(query)?);
            }
        }
        Ok(())
    }
}

/// The query that reads a mode's materialized result, if it has one.
fn materialized_sql(workload: &dyn Workload, mode: Mode) -> Option<String> {
    match mode {
        Mode::Ivmlite => Some(format!("SELECT * FROM {}", workload.view_name())),
        Mode::HandwrittenTrigger => workload.handwritten().map(|h| h.read_sql.to_string()),
        Mode::NoMaintenance | Mode::UnindexedRecompute | Mode::IndexedRecompute => None,
    }
}

/// Phase 4 spec §3.2 steps 3 and 7: the materialized result equals SQLite's
/// own evaluation of the view, as a multiset.
fn verify(c: &Connection, workload: &dyn Workload, mode: Mode, when: &str) -> Result<()> {
    if let Some(sql) = materialized_sql(workload, mode) {
        check_same_multiset(c, &sql, workload.view_sql())
            .map_err(|why| format!("{} {} {when}: {why}", workload.name(), mode.name()))?;
    }
    Ok(())
}

/// The indexed baseline must really read the covering index.
fn check_covering_plan(c: &Connection, workload: &dyn Workload) -> Result<()> {
    let mut plan = c.prepare(&format!("EXPLAIN QUERY PLAN {}", workload.view_sql()))?;
    let details: Vec<String> = plan
        .query_map([], |row| row.get(3))?
        .collect::<rusqlite::Result<_>>()?;
    if !details.iter().any(|d| d.contains("COVERING INDEX")) {
        return Err(format!(
            "{}: the indexed recompute does not use its covering index: {details:?}",
            workload.name()
        )
        .into());
    }
    Ok(())
}

fn bootstrap(c: &Connection, workload: &dyn Workload, mode: Mode) -> rusqlite::Result<()> {
    match mode {
        Mode::Ivmlite => create_view(c, workload.view_name(), workload.view_sql()),
        Mode::HandwrittenTrigger => match workload.handwritten() {
            Some(handwritten) => (handwritten.install)(c),
            None => Ok(()),
        },
        Mode::IndexedRecompute => c.execute_batch(workload.covering_index_sql()),
        Mode::NoMaintenance | Mode::UnindexedRecompute => Ok(()),
    }
}

/// One fresh database, one mode: seed, bootstrap (timed), verify, prepare,
/// warm-up batch (maintenance timed as `first_refresh_ms`), verify, then the
/// measured batch's apply, maintenance and read (each timed), and a final
/// verify.
pub fn run_once(
    workload: &dyn Workload,
    mode: Mode,
    rows: usize,
    batch: usize,
    extension: &Extension,
) -> Result<Timing> {
    let c = extension.open()?;
    c.execute_batch(workload.ddl())?;
    workload.seed(&c, rows)?;

    let start = Instant::now();
    bootstrap(&c, workload, mode)?;
    let bootstrap_ms = elapsed_ms(start);
    if mode == Mode::IndexedRecompute {
        check_covering_plan(&c, workload)?;
    }
    verify(&c, workload, mode, "after bootstrap")?;

    let mut writes = PreparedBatch::prepare(&c, workload.batch_sql(), workload.apply_fn())?;
    let mut maintenance = match mode {
        Mode::Ivmlite => Maintenance::Refresh(prepare_refresh(&c, workload.view_name())?),
        Mode::UnindexedRecompute | Mode::IndexedRecompute => {
            Maintenance::Recompute(c.prepare(workload.view_sql())?)
        }
        Mode::NoMaintenance | Mode::HandwrittenTrigger => Maintenance::Nothing,
    };
    let mut read = materialized_sql(workload, mode)
        .map(|sql| c.prepare(&sql))
        .transpose()?;

    writes.execute(rows, batch, WARM_UP_ROUND)?;
    let start = Instant::now();
    maintenance.run()?;
    let first_refresh_ms = elapsed_ms(start);
    if let Some(read) = read.as_mut() {
        black_box(drain(read)?);
    }
    verify(&c, workload, mode, "after the warm-up batch")?;

    let start = Instant::now();
    writes.execute(rows, batch, MEASURED_ROUND)?;
    let apply_ms = elapsed_ms(start);

    let start = Instant::now();
    maintenance.run()?;
    let maintain_ms = elapsed_ms(start);

    let start = Instant::now();
    if let Some(read) = read.as_mut() {
        black_box(drain(read)?);
    }
    let read_ms = elapsed_ms(start);

    verify(&c, workload, mode, "after the measured batch")?;
    Ok(Timing {
        bootstrap_ms,
        first_refresh_ms,
        apply_ms,
        maintain_ms,
        read_ms,
        database_kib: database_kib(&c)? as f64,
    })
}

// ---------------------------------------------------------------------------
// The whole benchmark
// ---------------------------------------------------------------------------

pub struct RunConfig {
    pub rows: usize,
    pub batch: usize,
    pub repeats: usize,
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo())
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The commit, and whether the code that builds the harness or the
/// extension has uncommitted changes.
fn commit_line() -> String {
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = match git(&[
        "status",
        "--porcelain",
        "--",
        "crates",
        "Cargo.toml",
        "Cargo.lock",
    ]) {
        Some(status) if status.is_empty() => "no",
        Some(_) => "yes",
        None => "unknown",
    };
    format!("{commit}; uncommitted changes under crates/, Cargo.toml or Cargo.lock: {dirty}")
}

/// `#`-prefixed lines naming what produced the CSV that follows
/// (`docs/demos/real-world-demo-roadmap.md`, claims policy).
fn print_provenance(workload: &dyn Workload, config: &RunConfig, extension: &Extension) {
    let command: Vec<String> = std::env::args().collect();
    println!("# command: {}", command.join(" "));
    println!("# commit: {}", commit_line());
    println!(
        "# sqlite: {} (bundled by rusqlite in this harness, which hosts the extension)",
        rusqlite::version()
    );
    println!("# ivmlite extension: {}", extension.describe());
    println!(
        "# workload: {}, {} base rows, {}-operation mixed batches, {} repeats; \
         rows report the median and min-max over the repeats",
        workload.name(),
        config.rows,
        config.batch,
        config.repeats
    );
}

fn print_csv(workload: &dyn Workload, config: &RunConfig, modes: &[Mode], timings: &[Vec<Timing>]) {
    let mut header = vec!["case", "mode", "base_rows", "batch_size", "repeats"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    for (name, _, _) in METRICS {
        let stem = name
            .strip_suffix("_ms")
            .map_or(name.to_string(), |s| s.to_string());
        let unit = if name.ends_with("_ms") { "_ms" } else { "" };
        header.push(name.to_string());
        header.push(format!("{stem}_min{unit}"));
        header.push(format!("{stem}_max{unit}"));
    }
    println!("{}", header.join(","));

    for (mode, runs) in modes.iter().zip(timings) {
        let mut fields = vec![
            workload.name().to_string(),
            mode.name().to_string(),
            config.rows.to_string(),
            config.batch.to_string(),
            config.repeats.to_string(),
        ];
        for (_, value, decimals) in METRICS {
            let values: Vec<f64> = runs.iter().map(value).collect();
            let summary = Summary::of(&values).expect("repeats is positive");
            for v in [summary.median, summary.min, summary.max] {
                fields.push(format!("{v:.decimals$}"));
            }
        }
        println!("{}", fields.join(","));
    }
}

/// Runs every mode `config.repeats` times, rotating the mode order across
/// repeats, and prints the provenance and the CSV to stdout.
pub fn run(workload: &dyn Workload, config: &RunConfig, extension: &Extension) -> Result<()> {
    if config.rows == 0 || config.batch == 0 || config.repeats == 0 {
        return Err("rows, batch and repeats must be positive".into());
    }
    workload.validate(config.rows, config.batch, ROUNDS)?;
    print_provenance(workload, config, extension);
    let modes = workload.modes();
    let mut timings: Vec<Vec<Timing>> = vec![Vec::with_capacity(config.repeats); modes.len()];
    for repeat in 0..config.repeats {
        for mode in rotated(&modes, repeat) {
            eprintln!(
                "{}: repeat {}/{}, {}",
                workload.name(),
                repeat + 1,
                config.repeats,
                mode.name()
            );
            let timing = run_once(workload, mode, config.rows, config.batch, extension)?;
            let slot = modes
                .iter()
                .position(|m| *m == mode)
                .expect("rotation keeps the modes");
            timings[slot].push(timing);
        }
    }
    print_csv(workload, config, &modes, &timings);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_counts() {
        let odd = Summary::of(&[3.0, 1.0, 2.0]).unwrap();
        assert_eq!(
            odd,
            Summary {
                median: 2.0,
                min: 1.0,
                max: 3.0
            }
        );
        let even = Summary::of(&[4.0, 1.0, 3.0, 2.0]).unwrap();
        assert_eq!(
            even,
            Summary {
                median: 2.5,
                min: 1.0,
                max: 4.0
            }
        );
        assert_eq!(Summary::of(&[7.0]).unwrap().median, 7.0);
        assert_eq!(Summary::of(&[]), None);
    }

    #[test]
    fn multiset_oracle_counts_duplicates_and_storage_classes() {
        let c = Connection::open_in_memory().unwrap();
        let same = "SELECT 1, 'a' UNION ALL SELECT 2, 'b'";
        let reordered = "SELECT 2, 'b' UNION ALL SELECT 1, 'a'";
        assert_eq!(check_same_multiset(&c, same, reordered), Ok(()));

        // `EXCEPT` would accept both of these; a multiset comparison must not.
        let duplicated = "SELECT 1, 'a' UNION ALL SELECT 1, 'a' UNION ALL SELECT 2, 'b'";
        assert!(check_same_multiset(&c, duplicated, same).is_err());
        let real = "SELECT 1.0, 'a' UNION ALL SELECT 2, 'b'";
        assert!(check_same_multiset(&c, real, same).is_err());
        let null = "SELECT NULL, 'a' UNION ALL SELECT 2, 'b'";
        assert!(check_same_multiset(&c, null, same).is_err());
    }

    #[test]
    fn mode_order_rotates_across_repeats() {
        let modes = [Mode::NoMaintenance, Mode::IndexedRecompute, Mode::Ivmlite];
        assert_eq!(rotated(&modes, 0), modes);
        assert_eq!(
            rotated(&modes, 1),
            [Mode::IndexedRecompute, Mode::Ivmlite, Mode::NoMaintenance]
        );
        assert_eq!(rotated(&modes, 4), rotated(&modes, 1));
    }

    #[test]
    fn extension_flag_is_taken_from_any_position() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (path, rest) =
            take_extension_flag(args(&["case", "--extension", "/tmp/lib.dylib", "10"])).unwrap();
        assert_eq!(path, Some(PathBuf::from("/tmp/lib.dylib")));
        assert_eq!(rest, args(&["case", "10"]));
        assert_eq!(
            take_extension_flag(args(&["10"])).unwrap(),
            (None, args(&["10"]))
        );
        assert!(take_extension_flag(args(&["--extension"])).is_err());
        assert!(take_extension_flag(args(&["--extension", "a", "--extension", "b"])).is_err());
    }

    fn workloads() -> Vec<&'static dyn Workload> {
        let mut all: Vec<&'static dyn Workload> = vec![&crate::fluxflow::FLUXFLOW];
        all.extend(
            crate::demand_cases::ALL
                .into_iter()
                .map(|case| case as &dyn Workload),
        );
        all
    }

    #[test]
    fn every_indexed_recompute_reads_only_its_covering_index() {
        for workload in workloads() {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch(workload.ddl()).unwrap();
            workload.seed(&c, 1_000).unwrap();
            assert!(
                check_covering_plan(&c, workload).is_err(),
                "{} is covered before its index exists",
                workload.name()
            );
            c.execute_batch(workload.covering_index_sql()).unwrap();
            check_covering_plan(&c, workload).unwrap();
        }
    }

    #[test]
    fn modes_include_the_trigger_baseline_only_where_one_exists() {
        assert!(crate::fluxflow::FLUXFLOW
            .modes()
            .contains(&Mode::HandwrittenTrigger));
        for case in crate::demand_cases::ALL {
            assert_eq!(
                case.modes(),
                [
                    Mode::NoMaintenance,
                    Mode::UnindexedRecompute,
                    Mode::IndexedRecompute,
                    Mode::Ivmlite
                ]
            );
        }
    }

    #[test]
    fn extension_version_comes_from_its_manifest() {
        assert_ne!(extension_source_version(), "unknown");
    }
}
