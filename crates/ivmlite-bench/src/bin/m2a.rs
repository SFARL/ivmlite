//! Portable M2a export, correctness/state replay, timing and bounded churn.
#[cfg(test)]
#[path = "../m2a_fixtures.rs"]
mod m2a_fixtures;
#[path = "../m2a_report.rs"]
mod m2a_report;
#[path = "../m2a_runner.rs"]
mod m2a_runner;
#[path = "../m2a_space.rs"]
mod m2a_space;
#[path = "../m2a_state.rs"]
mod m2a_state;
#[path = "../m2a_tracecheck.rs"]
mod m2a_tracecheck;

use ivmlite_workload::m2a::{Cell, Fixture, M2aConfig, Shape};
use m2a_runner::{
    correctness, target_statistics, time_fixture, Measurement, Result, TargetStats, MODES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct Args {
    action: String,
    workload: PathBuf,
    out: PathBuf,
    extension: Option<PathBuf>,
    fixture: Option<PathBuf>,
    cells: Vec<String>,
    profiles: Vec<String>,
    seeds: Option<Vec<u64>>,
    shape: Option<Shape>,
    fanout: Option<usize>,
    batch: Option<usize>,
    smoke: bool,
    skip_smoke_gate: bool,
    resume: bool,
    retry_failed: bool,
    allow_dirty: bool,
    /// Destination of the `report` tables; `<out>/report` when absent.
    report: Option<PathBuf>,
    /// Overrides `churn.rounds` of the workload file.
    rounds: Option<usize>,
    repeats: usize,
}
fn parse(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut it = args.into_iter();
    let mut a = Args {
        action: it.next().unwrap_or("run".into()),
        workload: "workloads/m2a.toml".into(),
        out: "target/m2a".into(),
        extension: None,
        fixture: None,
        cells: vec![],
        profiles: vec![],
        seeds: None,
        shape: None,
        fanout: None,
        batch: None,
        smoke: false,
        skip_smoke_gate: false,
        resume: false,
        retry_failed: false,
        allow_dirty: false,
        report: None,
        rounds: None,
        repeats: 5,
    };
    if !["export", "run", "verify", "churn", "confirm", "report"].contains(&a.action.as_str()) {
        return Err("usage: m2a {export|run|verify|churn|confirm|report} [--workload FILE] [--out DIR] [--extension LIB] [--fixture FILE] [--cells IDS] [--profiles LABELS] [--shape single|join] [--fanout N] [--batch N] [--seeds N,...] [--smoke] [--skip-smoke-gate] [--resume] [--retry-failed] [--allow-dirty] [--rounds N] [--repeats N] [--report DIR]".into());
    }
    let mut seen = BTreeSet::new();
    while let Some(flag) = it.next() {
        if !seen.insert(flag.clone()) {
            return Err(format!("duplicate option {flag}").into());
        }
        match flag.as_str() {
            "--smoke" => a.smoke = true,
            "--skip-smoke-gate" => a.skip_smoke_gate = true,
            "--resume" => a.resume = true,
            "--retry-failed" => a.retry_failed = true,
            "--allow-dirty" => a.allow_dirty = true,
            _ => {
                let value = it
                    .next()
                    .ok_or_else(|| format!("missing value after {flag}"))?;
                match flag.as_str() {
                    "--workload" => a.workload = value.into(),
                    "--out" => a.out = value.into(),
                    "--extension" => a.extension = Some(value.into()),
                    "--fixture" => a.fixture = Some(value.into()),
                    "--cells" => a.cells = list(&value)?,
                    "--profiles" => a.profiles = list(&value)?,
                    "--shape" => {
                        a.shape = Some(match value.as_str() {
                            "single" => Shape::Single,
                            "join" => Shape::Join,
                            _ => return Err("shape must be single or join".into()),
                        })
                    }
                    "--fanout" => a.fanout = Some(value.parse()?),
                    "--batch" => a.batch = Some(value.parse()?),
                    "--seeds" => {
                        let v: Vec<u64> = list(&value)?
                            .into_iter()
                            .map(|v| v.parse())
                            .collect::<std::result::Result<_, _>>()?;
                        if v.iter().collect::<BTreeSet<_>>().len() != v.len() {
                            return Err("duplicate numeric seed".into());
                        }
                        a.seeds = Some(v);
                    }
                    "--report" => a.report = Some(value.into()),
                    "--rounds" => a.rounds = Some(value.parse()?),
                    "--repeats" => a.repeats = value.parse()?,
                    _ => return Err(format!("unknown option {flag}").into()),
                }
            }
        }
    }
    if a.rounds == Some(0) || a.repeats == 0 || a.fanout == Some(0) || a.batch == Some(0) {
        return Err("rounds/repeats/fanout/batch must be positive".into());
    }
    let only = |flag: &str, actions: &[&str]| -> Result<()> {
        if seen.contains(flag) && !actions.contains(&a.action.as_str()) {
            Err(format!("{flag} is not applicable to {}", a.action).into())
        } else {
            Ok(())
        }
    };
    only("--report", &["report"])?;
    only("--fixture", &["verify"])?;
    only("--rounds", &["churn"])?;
    only("--repeats", &["confirm"])?;
    only("--skip-smoke-gate", &["run"])?;
    if a.smoke && a.skip_smoke_gate {
        return Err("--skip-smoke-gate is meaningless with --smoke".into());
    }
    only("--seeds", &["run", "export"])?;
    only("--extension", &["run", "verify", "churn", "confirm"])?;
    for flag in [
        "--workload",
        "--cells",
        "--profiles",
        "--shape",
        "--fanout",
        "--batch",
        "--smoke",
    ] {
        only(flag, &["run", "export", "churn", "confirm"])?;
    }
    for flag in ["--resume", "--retry-failed", "--allow-dirty"] {
        only(flag, &["export", "run", "verify", "churn", "confirm"])?;
    }
    if a.retry_failed && !a.resume {
        return Err("--retry-failed requires --resume".into());
    }
    if a.action == "verify" && a.fixture.is_none() {
        return Err("verify requires --fixture".into());
    }
    Ok(a)
}
fn list(value: &str) -> Result<Vec<String>> {
    let result: Vec<String> = value.split(',').map(str::to_owned).collect();
    if result.iter().any(|s| s.is_empty() || s.trim() != s)
        || result.iter().collect::<BTreeSet<_>>().len() != result.len()
    {
        return Err("empty, duplicate or whitespace-padded list entry".into());
    }
    Ok(result)
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn json_hash(value: &impl Serialize) -> Result<String> {
    Ok(hash(&serde_json::to_vec(value)?))
}
fn file_hash(path: &Path) -> Result<String> {
    Ok(hash(&fs::read(path)?))
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut out = BufWriter::new(File::create(&temporary)?);
    serde_json::to_writer(&mut out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    out.get_ref().sync_all()?;
    drop(out);
    fs::rename(temporary, path)?;
    Ok(())
}
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn command(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .current_dir(repo())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}
fn source_hash() -> Result<String> {
    let root = repo();
    let mut files = vec![
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("crates/ivmlite-bench/Cargo.toml"),
        root.join("crates/ivmlite-workload/Cargo.toml"),
        root.join("crates/ivmlite-test/Cargo.toml"),
    ];
    for name in [
        "crates/ivmlite-bench/src",
        "crates/ivmlite-workload/src",
        "crates/ivmlite-test/src",
    ] {
        collect_sources(&root.join(name), &mut files)?;
    }
    files.sort();
    let mut digest = Sha256::new();
    for path in files {
        digest.update(path.strip_prefix(&root)?.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sources(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Hardware {
    host: String,
    cpu: String,
    memory: String,
    logical_cpus: usize,
}
fn hardware() -> Hardware {
    let mut cpu = command("sysctl", &["-n", "machdep.cpu.brand_string"]);
    if cpu == "unavailable" {
        cpu = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|line| line.starts_with("model name"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| command("uname", &["-m"]));
    }
    let mut memory = command("sysctl", &["-n", "hw.memsize"]);
    if memory == "unavailable" {
        memory = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|line| line.starts_with("MemTotal:"))
                    .map(str::to_owned)
            })
            .unwrap_or(memory);
    }
    Hardware {
        host: command("uname", &["-a"]),
        cpu,
        memory,
        logical_cpus: std::thread::available_parallelism().map_or(0, usize::from),
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Provenance {
    invocation_id: String,
    command: Vec<String>,
    started_unix_ms: u64,
    started_utc: String,
    start_load: String,
    extension: Option<PathBuf>,
    worktree_status: String,
    hardware: Hardware,
    /// Whether a full run was admitted by a completed smoke run or by `--skip-smoke-gate`.
    #[serde(default)]
    smoke_gate: SmokeGate,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SmokeGate {
    #[default]
    NotApplicable,
    Passed,
    Skipped,
}
fn provenance() -> Provenance {
    let started_unix_ms = now_ms();
    Provenance {
        invocation_id: format!("{started_unix_ms}-{}", std::process::id()),
        command: std::env::args().collect(),
        started_unix_ms,
        started_utc: command("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]),
        start_load: command("uptime", &[]),
        extension: None,
        worktree_status: command("git", &["status", "--porcelain"]),
        hardware: hardware(),
        smoke_gate: SmokeGate::NotApplicable,
    }
}
struct Context {
    source_sha256: String,
    source_revision: String,
    runner_sha256: String,
    extension_sha256: Option<String>,
    library: Option<PathBuf>,
    provenance: Provenance,
}
fn context(a: &Args, mut provenance: Provenance) -> Result<Context> {
    let status = command("git", &["status", "--porcelain", "--untracked-files=no"]);
    if a.action != "export" && !a.allow_dirty && !status.is_empty() {
        return Err("tracked source changes exist; commit them or explicitly use --allow-dirty for development smoke".into());
    }
    let library = if a.action == "export" {
        None
    } else {
        Some(match &a.extension {
            Some(path) => path.canonicalize()?,
            None => ivmlite_test::extension_library_for(ivmlite_test::Profile::Release)
                .map_err(|s| format!("{s}; or pin --extension LIB"))?,
        })
    };
    provenance.extension = library.clone();
    let extension_sha256 = library.as_ref().map(|p| file_hash(p)).transpose()?;
    Ok(Context {
        source_sha256: source_hash()?,
        source_revision: command("git", &["rev-parse", "HEAD"]),
        runner_sha256: file_hash(&std::env::current_exe()?)?,
        extension_sha256,
        library,
        provenance,
    })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    kind: String,
    cell: Cell,
    seed: u64,
    repeat: usize,
    rounds: usize,
    smoke: bool,
    fixture_sha256: String,
    initial_data_sha256: String,
    trace_sha256: String,
    sql_sha256: String,
    view_sql: String,
    schema_ddl: String,
    ordinary_index_ddl: String,
    covering_index_ddl: String,
    config_sha256: String,
    source_revision: String,
    source_sha256: String,
    extension_sha256: Option<String>,
    runner_sha256: String,
    sqlite_version: String,
    /// `rustc -vV` of the compiler that built this runner, lines joined with "; ".
    runner_rustc: String,
    hardware: Hardware,
    /// Read back from a live connection after setup, never copied from the setup string.
    pragmas: BTreeMap<String, String>,
    target_selection: String,
    plans: m2a_runner::Plans,
    mode_order: Vec<String>,
    provenance: Provenance,
}
impl Manifest {
    fn compatible(&self, other: &Self) -> bool {
        let mut other = other.clone();
        other.provenance = self.provenance.clone();
        *self == other
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimingFile {
    manifest: Manifest,
    finished_unix_ms: u64,
    finish_load: String,
    mode_provenance: Vec<ModeProvenance>,
    measurements: Vec<Measurement>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModeProvenance {
    mode: String,
    started_unix_ms: u64,
    start_load: String,
    finished_unix_ms: u64,
    finish_load: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ExpectedCase {
    kind: String,
    cell: String,
    seed: u64,
    repeat: usize,
    rounds: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SelectionIdentity {
    schema_version: u32,
    kind: String,
    config_sha256: String,
    source_sha256: String,
    runner_sha256: String,
    extension_sha256: Option<String>,
    smoke: bool,
    hardware: Hardware,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FailedCase {
    cell: String,
    seed: u64,
    repeat: usize,
    phase: String,
    error: String,
}
/// Losses by cell: every planned case is complete, failed or not yet run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Summary {
    planned: u64,
    complete: u64,
    failed: Vec<FailedCase>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Selection {
    identity: SelectionIdentity,
    expected: BTreeSet<ExpectedCase>,
    invocations: Vec<Provenance>,
    #[serde(default)]
    summary: Summary,
}
/// Returned when one or more cases failed; the matrix still ran to the end.
struct CasesFailed(String);
impl std::fmt::Display for CasesFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
// `main` prints the Debug form, so keep it as plain as the message.
impl std::fmt::Debug for CasesFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for CasesFailed {}
#[cfg(test)]
thread_local! {
    /// Test-only: the cell whose cases `execute_case` must fail with "injected".
    static INJECTED_FAILURE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}
/// Smoke selections have their own file, so a smoke and a full run can share one output directory.
fn selection_file(out: &Path, action: &str, smoke: bool) -> PathBuf {
    let suffix = if smoke { "-smoke" } else { "" };
    out.join(format!("selection-{action}{suffix}.json"))
}
fn selection_path(a: &Args) -> PathBuf {
    selection_file(&a.out, &a.action, a.smoke)
}
fn merge_selection(path: &Path, mut wanted: Selection) -> Result<Selection> {
    if path.exists() {
        let old: Selection = read_json(path)?;
        if old.identity != wanted.identity {
            return Err("incompatible selection manifest; use a new output directory".into());
        }
        if wanted.expected.iter().any(|case| {
            old.expected.iter().any(|previous| {
                previous.kind == case.kind
                    && previous.cell == case.cell
                    && previous.seed == case.seed
                    && previous.repeat == case.repeat
                    && previous.rounds != case.rounds
            })
        }) {
            return Err("a planned case already exists with a different round count".into());
        }
        wanted.expected.extend(old.expected);
        let mut invocations = old.invocations;
        invocations.extend(wanted.invocations);
        wanted.invocations = invocations;
        wanted.summary = old.summary;
    }
    wanted.summary.planned = wanted.expected.len() as u64;
    write_json(path, &wanted)?;
    Ok(wanted)
}
fn load_config(a: &Args) -> Result<M2aConfig> {
    let mut cfg = M2aConfig::load(&a.workload)?;
    if a.smoke {
        cfg = cfg.smoke_variant();
    }
    if !a
        .profiles
        .iter()
        .all(|v| cfg.profiles.iter().any(|p| p.label() == v))
    {
        return Err("unknown profile selection".into());
    }
    if a.seeds
        .as_ref()
        .is_some_and(|seeds| seeds.iter().any(|s| !cfg.seeds.contains(s)))
    {
        return Err("seed selection is absent from workload".into());
    }
    Ok(cfg)
}
fn select_cells(cfg: &M2aConfig, a: &Args) -> Result<Vec<Cell>> {
    let all = cfg.cells()?;
    if !a.cells.iter().all(|id| all.iter().any(|c| &c.id() == id)) {
        return Err("unknown cell selection".into());
    }
    let selected: Vec<_> = all
        .into_iter()
        .filter(|c| {
            (a.cells.is_empty() || a.cells.contains(&c.id()))
                && (a.profiles.is_empty() || a.profiles.iter().any(|p| p == c.profile.label()))
                && a.shape.is_none_or(|s| s == c.shape)
                && a.fanout.is_none_or(|n| n == c.fanout)
                && a.batch.is_none_or(|n| n == c.batch_size)
        })
        .filter(|c| {
            a.action != "churn"
                || (c.shape == Shape::Join
                    && c.fanout == cfg.churn.fanout
                    && (a.smoke || c.batch_size == cfg.churn.batch)
                    && cfg.churn.profiles.iter().any(|p| p == c.profile.label()))
        })
        .collect();
    if selected.is_empty() {
        return Err("selection contains no applicable workload cells".into());
    }
    Ok(selected)
}
fn mode_offset(cfg: &M2aConfig, cell: &Cell, seed: u64, repeat: usize) -> Result<usize> {
    let ci = cfg
        .cells()?
        .iter()
        .position(|c| c == cell)
        .ok_or("cell absent from global matrix")?;
    let si = cfg
        .seeds
        .iter()
        .position(|&s| s == seed)
        .ok_or("seed absent from workload")?;
    Ok((ci % 4 + si % 4 + repeat % 4) % 4)
}
fn manifest(f: &Fixture, a: &Args, repeat: usize, ctx: &Context) -> Result<Manifest> {
    let offset = mode_offset(&f.config, &f.cell, f.seed, repeat)?;
    let probe = m2a_runner::probe_provenance(f)?;
    let mut started = ctx.provenance.clone();
    started.started_unix_ms = now_ms();
    started.started_utc = command("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]);
    started.start_load = command("uptime", &[]);
    Ok(Manifest {
        schema_version: 4,
        kind: a.action.clone(),
        cell: f.cell.clone(),
        seed: f.seed,
        repeat,
        rounds: f.rounds,
        smoke: a.smoke,
        fixture_sha256: json_hash(f)?,
        initial_data_sha256: json_hash(&f.tables)?,
        trace_sha256: json_hash(&f.batches)?,
        sql_sha256: hash(f.cell.view_sql().as_bytes()),
        view_sql: f.cell.view_sql().into(),
        schema_ddl: f.cell.schema_ddl().into(),
        ordinary_index_ddl: f.cell.ordinary_indexes().into(),
        covering_index_ddl: f.cell.covering_indexes().into(),
        config_sha256: json_hash(&f.config)?,
        source_revision: ctx.source_revision.clone(),
        source_sha256: ctx.source_sha256.clone(),
        extension_sha256: ctx.extension_sha256.clone(),
        runner_sha256: ctx.runner_sha256.clone(),
        sqlite_version: rusqlite::version().into(),
        runner_rustc: env!("M2A_RUSTC_VV").into(),
        hardware: ctx.provenance.hardware.clone(),
        pragmas: probe.pragmas,
        target_selection: "group_then_row".into(),
        plans: probe.plans,
        mode_order: (0..4)
            .map(|i| MODES[(i + offset) % 4].label().to_owned())
            .collect(),
        provenance: started,
    })
}
fn save_fixture(path: &Path, f: &Fixture) -> Result<()> {
    if path.exists() {
        let old: Fixture = read_json(path)?;
        if old != *f {
            return Err(format!("existing fixture differs: {}", path.display()).into());
        }
    } else {
        write_json(path, f)?;
    }
    let loaded: Fixture = read_json(path)?;
    loaded.validate()?;
    if loaded != *f {
        return Err("fixture export round trip changed workload or trace".into());
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Completion {
    schema_version: u32,
    manifest_sha256: String,
    artifacts: BTreeMap<String, String>,
    finished_unix_ms: u64,
}
fn complete(dir: &Path, manifest_name: &str, marker: &str, artifacts: &[&str]) -> Result<()> {
    let hashes = artifacts
        .iter()
        .map(|name| Ok(((*name).to_owned(), file_hash(&dir.join(name))?)))
        .collect::<Result<_>>()?;
    write_json(
        &dir.join(marker),
        &Completion {
            schema_version: 1,
            manifest_sha256: file_hash(&dir.join(manifest_name))?,
            artifacts: hashes,
            finished_unix_ms: now_ms(),
        },
    )
}
fn validate_completion(
    dir: &Path,
    manifest_name: &str,
    marker: &str,
    required: &[&str],
) -> Result<()> {
    let completed: Completion = read_json(&dir.join(marker))?;
    let expected: BTreeSet<_> = required.iter().map(|name| (*name).to_owned()).collect();
    if completed.schema_version != 1
        || completed.manifest_sha256 != file_hash(&dir.join(manifest_name))?
        || completed.artifacts.keys().cloned().collect::<BTreeSet<_>>() != expected
    {
        return Err("completion marker has incompatible manifest/artifact set".into());
    }
    for (name, wanted) in completed.artifacts {
        if file_hash(&dir.join(&name))? != wanted {
            return Err(format!("completed artifact hash differs: {name}").into());
        }
    }
    Ok(())
}
fn validate_pages(p: &m2a_state::PageMetrics) -> Result<()> {
    const PINNED_PAGE_SIZE: u64 = 4096;
    if p.page_size != PINNED_PAGE_SIZE
        || p.page_size.checked_mul(p.page_count) != Some(p.allocated_bytes)
        || p.page_count
            .checked_sub(p.freelist_count)
            .and_then(|n| n.checked_mul(p.page_size))
            != Some(p.live_page_bytes)
    {
        return Err("invalid page allocation metrics".into());
    }
    Ok(())
}
fn validate_round(r: &m2a_runner::RoundTiming, mode: m2a_runner::Mode, n: usize) -> Result<()> {
    let maintained = mode != m2a_runner::Mode::NoMaintenance;
    if r.round != n
        || !r.apply_ms.is_finite()
        || r.apply_ms < 0.0
        || r.maintain_ms.is_some() != maintained
        || r.read_ms.is_some() != maintained
        || !same_optional_number(
            r.apply_plus_maintain_ms,
            r.maintain_ms.map(|m| r.apply_ms + m),
        )
        || !same_optional_number(
            r.total_ms,
            r.maintain_ms
                .zip(r.read_ms)
                .map(|(m, read)| r.apply_ms + m + read),
        )
        || [
            r.maintain_ms,
            r.read_ms,
            r.apply_plus_maintain_ms,
            r.total_ms,
        ]
        .into_iter()
        .flatten()
        .any(|v| !v.is_finite() || v < 0.0)
    {
        return Err("incomplete, invalid or unpaired timing round".into());
    }
    Ok(())
}
// JSON decimal decoding may shift one ULP. This tolerance only admits normal
// floating-point round trips, far below the timing precision of this runner.
fn same_optional_number(actual: Option<f64>, expected: Option<f64>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.is_finite()
                && b.is_finite()
                && (a - b).abs()
                    <= 16.0 * f64::EPSILON * a.abs().max(b.abs()).max(f64::MIN_POSITIVE)
        }
        _ => false,
    }
}
fn validate_timing(file: &TimingFile, m: &Manifest, f: &Fixture) -> Result<()> {
    if file.manifest != *m
        || file.measurements.len() != MODES.len()
        || m.mode_order.len() != MODES.len()
        || file.mode_provenance.len() != MODES.len()
    {
        return Err("timing manifest or mode count differs".into());
    }
    let mut modes = BTreeSet::new();
    for (i, measurement) in file.measurements.iter().enumerate() {
        if measurement.mode.label() != m.mode_order[i]
            || file.mode_provenance[i].mode != m.mode_order[i]
            || !modes.insert(measurement.mode.label())
            || measurement.measured.len() != f.config.measured_rounds
            || !measurement.bootstrap_ms.is_finite()
            || measurement.bootstrap_ms < 0.0
            || measurement.plan.is_empty()
                != !matches!(
                    measurement.mode,
                    m2a_runner::Mode::Recompute | m2a_runner::Mode::CoveringRecompute
                )
        {
            return Err("missing/duplicate modes or incomplete measured rounds".into());
        }
        let manifest_plan = match measurement.mode {
            m2a_runner::Mode::Recompute => Some(&m.plans.ordinary_recompute),
            m2a_runner::Mode::CoveringRecompute => Some(&m.plans.covering_recompute),
            _ => None,
        };
        if let Some(expected) = manifest_plan {
            if m2a_runner::plan_text(&measurement.plan) != *expected {
                return Err(format!(
                    "{} timing plan differs from the manifest plan",
                    measurement.mode.label()
                )
                .into());
            }
        }
        validate_round(&measurement.warmup, measurement.mode, 0)?;
        for (n, r) in measurement.measured.iter().enumerate() {
            validate_round(r, measurement.mode, n + 1)?;
        }
        let computed = m2a_runner::steady_state(&measurement.measured)?;
        if !same_optional_number(Some(measurement.summary.apply_ms), Some(computed.apply_ms))
            || !same_optional_number(measurement.summary.maintain_ms, computed.maintain_ms)
            || !same_optional_number(measurement.summary.read_ms, computed.read_ms)
            || !same_optional_number(
                measurement.summary.apply_plus_maintain_ms,
                computed.apply_plus_maintain_ms,
            )
            || !same_optional_number(measurement.summary.total_ms, computed.total_ms)
        {
            return Err("timing summary differs from paired raw rounds".into());
        }
        for pages in [
            &measurement.base_pages,
            &measurement.bootstrap_pages,
            &measurement.final_pages,
        ] {
            validate_pages(pages)?;
        }
    }
    Ok(())
}
#[derive(Deserialize)]
struct ReadCheckpoint {
    phase: String,
    round: Option<usize>,
    snapshot: serde_json::Value,
}
fn validate_evidence(dir: &Path, f: &Fixture, inverse: bool, space: bool) -> Result<()> {
    let checkpoints: Vec<ReadCheckpoint> = read_json(&dir.join("correctness.json"))?;
    let mut expected = vec![("bootstrap", None)];
    for n in 0..f.rounds {
        for phase in ["written", "refreshed", "idle"] {
            expected.push((phase, Some(n)));
        }
    }
    if inverse {
        for n in 0..f.rounds {
            expected.push(("inverse", Some(n)));
        }
    }
    if checkpoints.len() != expected.len() {
        return Err("incomplete correctness checkpoint coverage".into());
    }
    for (checkpoint, (phase, round)) in checkpoints.iter().zip(expected) {
        if checkpoint.phase != phase
            || checkpoint.round != round
            || !checkpoint.snapshot["arrangements"].is_array()
            || !checkpoint.snapshot["pending"].is_array()
        {
            return Err("invalid correctness checkpoint".into());
        }
        let pages: m2a_state::PageMetrics =
            serde_json::from_value(checkpoint.snapshot["pages"].clone())?;
        validate_pages(&pages)?;
        if phase != "written" {
            for pending in checkpoint.snapshot["pending"].as_array().unwrap() {
                if pending["delta_rows"].as_u64() != Some(0)
                    || pending["replace_candidate_rows"].as_u64() != Some(0)
                {
                    return Err("completed correctness has pending deltas".into());
                }
            }
        }
    }
    let targets: Vec<TargetStats> = read_json(&dir.join("targets.json"))?;
    if targets.len() != f.rounds
        || targets.iter().enumerate().any(|(n, t)| {
            t.round != n
                || t.operations != f.cell.batch_size
                || t.per_operation.len() != f.cell.batch_size
        })
    {
        return Err("incomplete target statistics".into());
    }
    if space {
        let points: Vec<m2a_space::SpaceCheckpoint> = read_json(&dir.join("space.json"))?;
        let mut keys = Vec::new();
        for mode in MODES {
            keys.push((mode, None, "base"));
            keys.push((mode, None, "bootstrap"));
            for n in 0..f.rounds {
                for phase in ["written", "refreshed", "idle"] {
                    keys.push((mode, Some(n), phase));
                }
            }
        }
        if points.len() != keys.len() {
            return Err("incomplete space checkpoint coverage".into());
        }
        for (point, (mode, round, phase)) in points.iter().zip(keys) {
            if point.mode != mode || point.round != round || point.phase != phase {
                return Err("space checkpoint key differs".into());
            }
            validate_pages(&point.pages)?;
        }
    }
    Ok(())
}
fn case_dir(a: &Args, key: &ExpectedCase) -> PathBuf {
    case_path(&a.out, &a.action, key)
}
/// Where `action` stores the artifacts of `key` under `out`.
fn case_path(out: &Path, action: &str, key: &ExpectedCase) -> PathBuf {
    match action {
        "churn" => out.join(format!("churn_{}", key.cell)),
        "verify" => out
            .join(format!("verify_{}", key.cell))
            .join(format!("seed-{}", key.seed)),
        _ => out
            .join(&key.cell)
            .join(format!("seed-{}", key.seed))
            .join(if action == "confirm" {
                format!("confirm-repeat-{}", key.repeat)
            } else {
                format!("repeat-{}", key.repeat)
            }),
    }
}
fn marker_name(a: &Args) -> &'static str {
    if a.action == "export" {
        "export-completed.json"
    } else {
        "completed.json"
    }
}
fn install_manifest(path: &Path, wanted: &Manifest) -> Result<Manifest> {
    if path.exists() {
        let old: Manifest = read_json(path)?;
        if !old.compatible(wanted) {
            return Err(format!("refusing incompatible manifest at {}", path.display()).into());
        }
        Ok(old)
    } else {
        write_json(path, wanted)?;
        Ok(wanted.clone())
    }
}
fn execute_case(
    a: &Args,
    f: &Fixture,
    key: &ExpectedCase,
    ctx: &Context,
    phase: &mut String,
) -> Result<()> {
    #[cfg(test)]
    if INJECTED_FAILURE.with(|cell| cell.borrow().as_deref() == Some(key.cell.as_str())) {
        *phase = "injected".into();
        return Err("injected".into());
    }
    let dir = case_dir(a, key);
    fs::create_dir_all(&dir)?;
    let export = a.action == "export";
    let manifest_name = if export {
        "export-manifest.json"
    } else {
        "manifest.json"
    };
    let marker = marker_name(a);
    *phase = "manifest".into();
    let m = install_manifest(&dir.join(manifest_name), &manifest(f, a, key.repeat, ctx)?)?;
    save_fixture(&dir.join("fixture.json"), f)?;
    let timing = matches!(a.action.as_str(), "run" | "confirm");
    let inverse = matches!(a.action.as_str(), "churn" | "verify");
    let artifacts: &[&str] = if export {
        &["fixture.json"]
    } else if timing {
        &[
            "fixture.json",
            "correctness.json",
            "targets.json",
            "space.json",
            "timing.json",
        ]
    } else {
        &["fixture.json", "correctness.json", "targets.json"]
    };
    if dir.join(marker).exists() {
        if !a.resume {
            return Err(format!(
                "completed case exists at {}; use --resume or a new output directory",
                dir.display()
            )
            .into());
        }
        *phase = "resume_validation".into();
        validate_completion(&dir, manifest_name, marker, artifacts)?;
        if !export {
            validate_evidence(&dir, f, inverse, timing)?;
        }
        if timing {
            validate_timing(&read_json(&dir.join("timing.json"))?, &m, f)?;
        }
        eprintln!(
            "resumed {} {} seed {} repeat {}",
            a.action,
            f.cell.id(),
            f.seed,
            key.repeat
        );
        return Ok(());
    }
    if !export {
        let library = ctx
            .library
            .as_deref()
            .ok_or("execution requires an extension")?;
        *phase = "plan".into();
        m2a_runner::assert_plans(&m.plans, f.cell.shape)?;
        *phase = "correctness".into();
        let passed = correctness(f, library, inverse)?;
        write_json(&dir.join("correctness.json"), &passed.checkpoints)?;
        *phase = "targets".into();
        write_json(
            &dir.join("targets.json"),
            &target_statistics(f, &passed.rows_changed)?,
        )?;
        if timing {
            *phase = "space".into();
            write_json(
                &dir.join("space.json"),
                &m2a_space::replay_space(f, library)?,
            )?;
            let offset = mode_offset(&f.config, &f.cell, f.seed, key.repeat)?;
            let mut measurements = Vec::new();
            let mut mode_provenance = Vec::new();
            for i in 0..4 {
                let mode = MODES[(offset + i) % 4];
                *phase = format!("timing_{}", mode.label());
                eprintln!(
                    "timing {} seed {} repeat {} {}",
                    f.cell.id(),
                    f.seed,
                    key.repeat,
                    mode.label()
                );
                let started_unix_ms = now_ms();
                let start_load = command("uptime", &[]);
                measurements.push(time_fixture(f, mode, library)?);
                mode_provenance.push(ModeProvenance {
                    mode: mode.label().into(),
                    started_unix_ms,
                    start_load,
                    finished_unix_ms: now_ms(),
                    finish_load: command("uptime", &[]),
                });
            }
            let file = TimingFile {
                manifest: m.clone(),
                finished_unix_ms: now_ms(),
                finish_load: command("uptime", &[]),
                mode_provenance,
                measurements,
            };
            validate_timing(&file, &m, f)?;
            write_json(&dir.join("timing.json"), &file)?;
        }
        *phase = "evidence_validation".into();
        validate_evidence(&dir, f, inverse, timing)?;
    }
    *phase = "completion".into();
    complete(&dir, manifest_name, marker, artifacts)?;
    eprintln!(
        "completed {} {} seed {} repeat {}",
        a.action,
        f.cell.id(),
        f.seed,
        key.repeat
    );
    Ok(())
}
fn record_failure(
    a: &Args,
    provenance: &Provenance,
    ctx: Option<&Context>,
    key: Option<&ExpectedCase>,
    phase: &str,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let value = serde_json::json!({"schema_version":1,"invocation_id":provenance.invocation_id,"action":a.action,"cell":key.map(|k|&k.cell),"seed":key.map(|k|k.seed),"repeat":key.map(|k|k.repeat),"phase":phase,"error":error.to_string(),"source_sha256":ctx.map(|c|&c.source_sha256),"extension_sha256":ctx.and_then(|c|c.extension_sha256.as_ref()),"runner_sha256":ctx.map(|c|&c.runner_sha256),"provenance":provenance});
    write_json(&a.out.join("failure.json"), &value)?;
    if let Some(key) = key {
        let dir = case_dir(a, key);
        fs::create_dir_all(&dir)?;
        write_json(&dir.join("failure.json"), &value)?;
        if let Some(mismatch) = error.downcast_ref::<m2a_runner::MultisetMismatch>() {
            write_json(
                &dir.join("failure-rows.json"),
                &serde_json::json!({"expected_only":mismatch.expected_only,"actual_only":mismatch.actual_only}),
            )?;
        }
    }
    Ok(())
}
fn register_selection(
    a: &Args,
    cfg_hash: String,
    ctx: &Context,
    expected: BTreeSet<ExpectedCase>,
) -> Result<BTreeSet<ExpectedCase>> {
    let merged = merge_selection(
        &selection_path(a),
        Selection {
            identity: SelectionIdentity {
                schema_version: 1,
                kind: a.action.clone(),
                config_sha256: cfg_hash,
                source_sha256: ctx.source_sha256.clone(),
                runner_sha256: ctx.runner_sha256.clone(),
                extension_sha256: ctx.extension_sha256.clone(),
                smoke: a.smoke,
                hardware: ctx.provenance.hardware.clone(),
            },
            expected,
            invocations: vec![ctx.provenance.clone()],
            summary: Summary::default(),
        },
    )?;
    Ok(merged.expected)
}
fn is_failed(a: &Args, key: &ExpectedCase) -> bool {
    let dir = case_dir(a, key);
    dir.join("failure.json").exists() && !dir.join(marker_name(a)).exists()
}
fn failed_case(key: &ExpectedCase, phase: &str, error: &dyn std::fmt::Display) -> FailedCase {
    FailedCase {
        cell: key.cell.clone(),
        seed: key.seed,
        repeat: key.repeat,
        phase: phase.to_owned(),
        error: error.to_string(),
    }
}
fn read_failed_case(a: &Args, key: &ExpectedCase) -> Result<FailedCase> {
    let value: serde_json::Value = read_json(&case_dir(a, key).join("failure.json"))?;
    let field = |name: &str| -> Result<String> {
        Ok(value[name]
            .as_str()
            .ok_or_else(|| format!("failure.json lacks {name}"))?
            .to_owned())
    };
    Ok(failed_case(key, &field("phase")?, &field("error")?))
}
/// Counts the full plan from this run's failures and the markers on disk, so a
/// case that failed earlier and was skipped on resume still counts as lost.
/// A case whose marker passed validation is never in `failed_now`.
fn summarize(
    a: &Args,
    plan: &BTreeSet<ExpectedCase>,
    failed_now: &BTreeMap<ExpectedCase, FailedCase>,
) -> Result<Summary> {
    let mut summary = Summary {
        planned: plan.len() as u64,
        ..Summary::default()
    };
    for key in plan {
        if let Some(failure) = failed_now.get(key) {
            summary.failed.push(failure.clone());
        } else if case_dir(a, key).join(marker_name(a)).exists() {
            summary.complete += 1;
        } else if is_failed(a, key) {
            summary.failed.push(read_failed_case(a, key)?);
        }
    }
    Ok(summary)
}
/// Drops the failure records of a case that now completes. The top-level
/// record is removed only when it names this exact case.
fn clear_failure(a: &Args, key: &ExpectedCase) -> Result<()> {
    for name in ["failure.json", "failure-rows.json"] {
        let case_record = case_dir(a, key).join(name);
        if case_record.exists() {
            fs::remove_file(case_record)?;
        }
    }
    let top_level = a.out.join("failure.json");
    if top_level.exists() {
        let value: serde_json::Value = read_json(&top_level)?;
        if value["action"] == a.action.as_str()
            && value["cell"] == key.cell.as_str()
            && value["seed"] == key.seed
            && value["repeat"] == key.repeat
        {
            fs::remove_file(top_level)?;
        }
    }
    Ok(())
}
/// Without `--resume` an existing completed case is never touched: refuse the
/// whole invocation before anything is registered or written.
fn refuse_completed_cases(a: &Args, to_run: &BTreeSet<ExpectedCase>) -> Result<()> {
    if a.resume {
        return Ok(());
    }
    for key in to_run {
        let dir = case_dir(a, key);
        if dir.join(marker_name(a)).exists() {
            return Err(format!(
                "completed case exists at {}; use --resume or a new output directory",
                dir.display()
            )
            .into());
        }
    }
    Ok(())
}
/// Runs every case of this invocation (`to_run`), records each failure and keeps going, then writes
/// the loss summary for the whole merged `plan` into the selection file. Fails iff any case is lost.
fn run_cases(
    a: &Args,
    ctx: &Context,
    to_run: &BTreeSet<ExpectedCase>,
    plan: &BTreeSet<ExpectedCase>,
    mut run_one: impl FnMut(&ExpectedCase, &mut String) -> Result<()>,
) -> Result<()> {
    let mut failed_now = BTreeMap::new();
    for key in to_run {
        if a.resume && !a.retry_failed && is_failed(a, key) {
            eprintln!(
                "skipping failed {} {} seed {} repeat {}; use --retry-failed to rerun",
                a.action, key.cell, key.seed, key.repeat
            );
            continue;
        }
        let mut phase = "case_setup".to_owned();
        match run_one(key, &mut phase) {
            Ok(()) => clear_failure(a, key)?,
            Err(error) => {
                record_failure(a, &ctx.provenance, Some(ctx), Some(key), &phase, &*error)?;
                failed_now.insert(key.clone(), failed_case(key, &phase, &error));
            }
        }
    }
    let summary = summarize(a, plan, &failed_now)?;
    let path = selection_path(a);
    let mut selection: Selection = read_json(&path)?;
    selection.summary = summary.clone();
    write_json(&path, &selection)?;
    if summary.failed.is_empty() {
        Ok(())
    } else {
        Err(Box::new(CasesFailed(format!(
            "{} of {} cases failed; see {}",
            summary.failed.len(),
            summary.planned,
            path.display()
        ))))
    }
}
/// Confirmation cells repeat the seed-42 cells of the matrix.
const CONFIRM_SEED: u64 = 42;
fn case_set(
    kind: &str,
    cells: &[Cell],
    seeds: &[u64],
    repeats: usize,
    rounds: usize,
) -> BTreeSet<ExpectedCase> {
    let mut expected = BTreeSet::new();
    for c in cells {
        for &seed in seeds {
            for repeat in 0..repeats {
                expected.insert(ExpectedCase {
                    kind: kind.to_owned(),
                    cell: c.id(),
                    seed,
                    repeat,
                    rounds,
                });
            }
        }
    }
    expected
}
/// The cases this invocation runs. Churn seed and rounds come from the workload file;
/// `--rounds` overrides the rounds.
fn planned_cases(a: &Args, cfg: &M2aConfig, cells: &[Cell]) -> Result<BTreeSet<ExpectedCase>> {
    let seeds = match a.action.as_str() {
        "churn" => vec![cfg.churn.seed],
        "confirm" => {
            if !cfg.seeds.contains(&CONFIRM_SEED) {
                return Err(format!("seed {CONFIRM_SEED} required for confirmation").into());
            }
            vec![CONFIRM_SEED]
        }
        _ => a.seeds.clone().unwrap_or_else(|| cfg.seeds.clone()),
    };
    let repeats = if a.action == "confirm" { a.repeats } else { 1 };
    let rounds = if a.action == "churn" {
        a.rounds.unwrap_or(cfg.churn.rounds)
    } else {
        cfg.total_rounds()?
    };
    Ok(case_set(&a.action, cells, &seeds, repeats, rounds))
}
/// Every case of the unfiltered smoke matrix, which a full run requires to have completed.
fn smoke_plan(smoke: &M2aConfig) -> Result<BTreeSet<ExpectedCase>> {
    Ok(case_set(
        "run",
        &smoke.cells()?,
        &smoke.seeds,
        1,
        smoke.total_rounds()?,
    ))
}
fn check_smoke_completed(a: &Args, ctx: &Context) -> Result<()> {
    let smoke = M2aConfig::load(&a.workload)?.smoke_variant();
    let path = selection_file(&a.out, "run", true);
    if !path.exists() {
        return Err(format!("no smoke run found at {}", path.display()).into());
    }
    let selection: Selection = read_json(&path)?;
    let identity = &selection.identity;
    if identity.config_sha256 != json_hash(&smoke)?
        || identity.source_sha256 != ctx.source_sha256
        || identity.runner_sha256 != ctx.runner_sha256
        || identity.extension_sha256 != ctx.extension_sha256
    {
        return Err("the smoke run used a different workload, source, runner or extension".into());
    }
    let plan = smoke_plan(&smoke)?;
    if !plan.is_subset(&selection.expected) {
        return Err("the smoke run does not cover every smoke case".into());
    }
    if let Some(key) = plan
        .iter()
        .find(|key| !case_dir(a, key).join(marker_name(a)).exists())
    {
        return Err(format!("smoke case {} seed {} is not complete", key.cell, key.seed).into());
    }
    Ok(())
}
/// A full `run` starts only after the smoke matrix passed with this exact source, runner and
/// extension in the same output directory, unless `--skip-smoke-gate` is given. The outcome
/// is recorded in the provenance.
fn apply_smoke_gate(a: &Args, ctx: &mut Context) -> Result<()> {
    if a.action != "run" || a.smoke {
        return Ok(());
    }
    ctx.provenance.smoke_gate = if a.skip_smoke_gate {
        SmokeGate::Skipped
    } else {
        check_smoke_completed(a, ctx).map_err(|reason| {
            format!(
                "full run requires a completed smoke run: {reason}; run `m2a run --smoke --out {}` first or pass --skip-smoke-gate",
                a.out.display()
            )
        })?;
        SmokeGate::Passed
    };
    Ok(())
}
fn run_inner(a: &Args, ctx: &Context) -> Result<()> {
    if a.action == "verify" {
        let f: Fixture = read_json(a.fixture.as_ref().unwrap())?;
        f.validate()?;
        let key = ExpectedCase {
            kind: a.action.clone(),
            cell: f.cell.id(),
            seed: f.seed,
            repeat: 0,
            rounds: f.rounds,
        };
        let to_run = BTreeSet::from([key]);
        refuse_completed_cases(a, &to_run)?;
        let plan = register_selection(a, json_hash(&f.config)?, ctx, to_run.clone())?;
        return run_cases(a, ctx, &to_run, &plan, |key, phase| {
            execute_case(a, &f, key, ctx, phase)
        });
    }
    let cfg = load_config(a)?;
    let cells = select_cells(&cfg, a)?;
    let expected = planned_cases(a, &cfg, &cells)?;
    refuse_completed_cases(a, &expected)?;
    let plan = register_selection(a, json_hash(&cfg)?, ctx, expected.clone())?;
    run_cases(a, ctx, &expected, &plan, |key, phase| {
        *phase = "generate".into();
        let cell = cells
            .iter()
            .find(|c| c.id() == key.cell)
            .ok_or("planned cell disappeared")?;
        let f = if a.action == "churn" {
            cfg.generate_rounds(cell, key.seed, key.rounds)?
        } else {
            cfg.generate(cell, key.seed)?
        };
        execute_case(a, &f, key, ctx, phase)
    })
}
/// Derives the published tables from the raw artifacts; nothing is executed or timed.
fn report(a: &Args) -> Result<()> {
    let dest = a.report.clone().unwrap_or_else(|| a.out.join("report"));
    let summary = m2a_report::report(&a.out, &dest)?;
    println!(
        "report {}: {} complete, {} incomplete, {} confirmation candidates, best median ratio {}",
        dest.display(),
        summary.complete,
        summary.incomplete,
        summary.candidates.len(),
        summary
            .best_ratio
            .map_or("n/a".to_owned(), |ratio| format!("{ratio:.3}"))
    );
    Ok(())
}
fn run(a: Args) -> Result<()> {
    if a.action == "report" {
        return report(&a);
    }
    fs::create_dir_all(&a.out)?;
    let provenance = provenance();
    let mut ctx = match context(&a, provenance.clone()) {
        Ok(ctx) => ctx,
        Err(error) => {
            record_failure(&a, &provenance, None, None, "setup", &*error)?;
            return Err(error);
        }
    };
    if let Err(error) = apply_smoke_gate(&a, &mut ctx) {
        record_failure(&a, &ctx.provenance, Some(&ctx), None, "smoke_gate", &*error)?;
        return Err(error);
    }
    if let Err(error) = run_inner(&a, &ctx) {
        if error.is::<CasesFailed>() {
            return Err(error);
        }
        let recorded = read_json::<serde_json::Value>(&a.out.join("failure.json"))
            .ok()
            .is_some_and(|v| v["invocation_id"].as_str() == Some(&ctx.provenance.invocation_id));
        if !recorded {
            record_failure(
                &a,
                &ctx.provenance,
                Some(&ctx),
                None,
                "selection_or_fixture_preflight",
                &*error,
            )?;
        }
        return Err(error);
    }
    Ok(())
}
fn main() -> Result<()> {
    run(parse(std::env::args().skip(1))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_workload::m2a::Table;
    fn args(items: &[&str]) -> Args {
        parse(items.iter().map(|s| (*s).to_owned())).unwrap()
    }
    fn cfg() -> M2aConfig {
        M2aConfig::load(&repo().join("workloads/m2a.toml")).unwrap()
    }
    fn test_context() -> Context {
        let hardware = Hardware {
            host: "host".into(),
            cpu: "cpu".into(),
            memory: "memory".into(),
            logical_cpus: 1,
        };
        Context {
            source_sha256: "source".into(),
            source_revision: "revision".into(),
            runner_sha256: "runner".into(),
            extension_sha256: None,
            library: None,
            provenance: Provenance {
                invocation_id: "test".into(),
                command: vec!["m2a".into()],
                started_unix_ms: 1,
                started_utc: "start".into(),
                start_load: "idle".into(),
                extension: None,
                worktree_status: String::new(),
                hardware,
                smoke_gate: SmokeGate::NotApplicable,
            },
        }
    }
    fn tiny_fixture() -> Fixture {
        let mut config = cfg();
        config.single_rows = 800;
        config.single_groups = 20;
        config.join_rows = 800;
        config.join_groups = 20;
        config.fanouts = vec![8];
        config.batch_sizes = vec![10];
        let cell = config.cells().unwrap().remove(0);
        config.generate(&cell, 1).unwrap()
    }
    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "m2a-cli-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn report_action_takes_out_and_report_only() {
        let a = args(&["report", "--out", "x", "--report", "y"]);
        assert_eq!((a.out, a.report), ("x".into(), Some("y".into())));
        assert!(args(&["report"]).report.is_none());
        for bad in [
            vec!["report", "--extension", "x"],
            vec!["report", "--resume"],
            vec!["report", "--allow-dirty"],
            vec!["report", "--smoke"],
            vec!["report", "--seeds", "1"],
            vec!["run", "--report", "y"],
        ] {
            assert!(
                parse(bad.iter().map(|s| (*s).to_owned())).is_err(),
                "{bad:?}"
            );
        }
    }
    #[test]
    fn cli_rejects_unknown_duplicate_empty_and_inapplicable_options() {
        for bad in [
            vec!["bogus"],
            vec!["run", "--seeds", "1,1"],
            vec!["run", "--seeds", "01,1"],
            vec!["run", "--cells", ",x"],
            vec!["verify"],
            vec!["run", "--fixture", "x"],
            vec!["run", "--shape", "outer"],
            vec!["run", "--rounds", "10"],
            vec!["run", "--repeats", "2"],
            vec!["churn", "--rounds", "0"],
            vec!["churn", "--seeds", "42"],
            vec!["verify", "--fixture", "x", "--smoke"],
            vec!["export", "--extension", "x"],
            vec!["run", "--cells", "a", "--cells", "b"],
            vec!["run", "--smoke", "--smoke"],
        ] {
            assert!(
                parse(bad.iter().map(|s| (*s).to_owned())).is_err(),
                "{bad:?}"
            );
        }
    }
    #[test]
    fn filters_keep_global_rotation_and_empty_churn_is_rejected() {
        let cfg = cfg();
        let all = cfg.cells().unwrap();
        let chosen = all
            .iter()
            .find(|c| c.shape == Shape::Join && c.fanout == 64)
            .unwrap();
        let a = args(&["run", "--cells", &chosen.id(), "--seeds", "42"]);
        let selected = select_cells(&cfg, &a).unwrap();
        assert_eq!(selected, vec![chosen.clone()]);
        assert_eq!(
            mode_offset(&cfg, &selected[0], 42, 0).unwrap(),
            (all.iter().position(|c| c == chosen).unwrap() + 1) % 4
        );
        let a = args(&["churn", "--shape", "single"]);
        assert!(select_cells(&cfg, &a).is_err());
    }
    #[test]
    fn completion_hashes_reject_missing_corrupt_and_incomplete_artifact_sets() {
        let dir = temp();
        write_json(&dir.join("manifest.json"), &serde_json::json!({"x":1})).unwrap();
        write_json(&dir.join("data.json"), &vec![1]).unwrap();
        complete(&dir, "manifest.json", "completed.json", &["data.json"]).unwrap();
        validate_completion(&dir, "manifest.json", "completed.json", &["data.json"]).unwrap();
        assert!(validate_completion(
            &dir,
            "manifest.json",
            "completed.json",
            &["data.json", "missing.json"]
        )
        .is_err());
        write_json(&dir.join("data.json"), &vec![2]).unwrap();
        assert!(
            validate_completion(&dir, "manifest.json", "completed.json", &["data.json"]).is_err()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn confirmation_paths_are_independent_of_initial_seed42_runs() {
        let mut a = args(&["run"]);
        let key = ExpectedCase {
            kind: "run".into(),
            cell: "cell".into(),
            seed: 42,
            repeat: 0,
            rounds: 6,
        };
        let first = case_dir(&a, &key);
        a.action = "confirm".into();
        assert_ne!(case_dir(&a, &key), first);
    }
    #[test]
    fn selection_resume_keeps_full_plan_when_invocation_selects_subset() {
        let dir = temp();
        let identity = SelectionIdentity {
            schema_version: 1,
            kind: "run".into(),
            config_sha256: "cfg".into(),
            source_sha256: "source".into(),
            runner_sha256: "runner".into(),
            extension_sha256: Some("extension".into()),
            smoke: true,
            hardware: Hardware {
                host: "host".into(),
                cpu: "cpu".into(),
                memory: "memory".into(),
                logical_cpus: 1,
            },
        };
        let first = ExpectedCase {
            kind: "run".into(),
            cell: "a".into(),
            seed: 1,
            repeat: 0,
            rounds: 6,
        };
        let second = ExpectedCase {
            cell: "b".into(),
            ..first.clone()
        };
        let path = dir.join("selection.json");
        merge_selection(
            &path,
            Selection {
                identity: identity.clone(),
                expected: BTreeSet::from([first.clone(), second.clone()]),
                invocations: vec![],
                summary: Summary::default(),
            },
        )
        .unwrap();
        merge_selection(
            &path,
            Selection {
                identity: identity.clone(),
                expected: BTreeSet::from([second]),
                invocations: vec![],
                summary: Summary::default(),
            },
        )
        .unwrap();
        let loaded: Selection = read_json(&path).unwrap();
        assert_eq!(loaded.expected.len(), 2);
        assert!(loaded.expected.contains(&first));
        let before = file_hash(&path).unwrap();
        let mut changed = identity;
        changed.extension_sha256 = Some("changed".into());
        assert!(merge_selection(
            &path,
            Selection {
                identity: changed,
                expected: BTreeSet::from([first]),
                invocations: vec![],
                summary: Summary::default()
            }
        )
        .is_err());
        assert_eq!(file_hash(&path).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn raw_timing_validation_rejects_unpaired_totals_missing_metrics_and_wrong_round() {
        use m2a_runner::{Mode, RoundTiming};
        let round = RoundTiming {
            round: 1,
            apply_ms: 0.1,
            maintain_ms: Some(0.2),
            read_ms: Some(0.3),
            apply_plus_maintain_ms: Some(0.1 + 0.2),
            total_ms: Some(0.1 + 0.2 + 0.3),
        };
        let round: RoundTiming =
            serde_json::from_slice(&serde_json::to_vec(&round).unwrap()).unwrap();
        validate_round(&round, Mode::Recompute, 1).unwrap();
        let mut bad = round.clone();
        bad.total_ms = Some(10.0);
        assert!(validate_round(&bad, Mode::Recompute, 1).is_err());
        bad = round.clone();
        bad.read_ms = None;
        assert!(validate_round(&bad, Mode::Recompute, 1).is_err());
        assert!(validate_round(&round, Mode::NoMaintenance, 1).is_err());
        assert!(validate_round(&round, Mode::Recompute, 2).is_err());
        bad = round;
        bad.apply_ms = f64::NAN;
        assert!(validate_round(&bad, Mode::Recompute, 1).is_err());
    }
    #[test]
    fn timing_validation_compares_recompute_plans_with_the_manifest() {
        let f = tiny_fixture();
        let library = ivmlite_test::extension_library();
        let m = manifest(&f, &args(&["run", "--smoke"]), 0, &test_context()).unwrap();
        let measurements = m
            .mode_order
            .iter()
            .map(|label| {
                let mode = MODES
                    .into_iter()
                    .find(|mode| mode.label() == label)
                    .unwrap();
                time_fixture(&f, mode, &library).unwrap()
            })
            .collect();
        let mode_provenance = m
            .mode_order
            .iter()
            .map(|mode| ModeProvenance {
                mode: mode.clone(),
                started_unix_ms: 1,
                start_load: String::new(),
                finished_unix_ms: 2,
                finish_load: String::new(),
            })
            .collect();
        let mut file = TimingFile {
            manifest: m.clone(),
            finished_unix_ms: 2,
            finish_load: String::new(),
            mode_provenance,
            measurements,
        };
        validate_timing(&file, &m, &f).unwrap();
        let covering = file
            .measurements
            .iter_mut()
            .find(|x| x.mode == m2a_runner::Mode::CoveringRecompute)
            .unwrap();
        covering.plan.push("SCAN events".into());
        let error = validate_timing(&file, &m, &f).unwrap_err();
        assert!(error.to_string().contains("covering_recompute"), "{error}");
    }
    #[test]
    fn export_context_never_resolves_an_extension() {
        let mut a = args(&["export"]);
        a.extension = Some(PathBuf::from("/missing/extension.so"));
        let ctx = context(&a, provenance()).unwrap();
        assert!(ctx.library.is_none());
        assert!(ctx.extension_sha256.is_none());
    }
    #[test]
    fn export_resume_binds_fixture_and_manifest_without_loading_a_library() {
        let dir = temp();
        let fixture = tiny_fixture();
        let mut a = args(&["export"]);
        a.out = dir.clone();
        let key = ExpectedCase {
            kind: "export".into(),
            cell: fixture.cell.id(),
            seed: fixture.seed,
            repeat: 0,
            rounds: fixture.rounds,
        };
        let mut ctx = test_context();
        let mut phase = String::new();
        execute_case(&a, &fixture, &key, &ctx, &mut phase).unwrap();
        assert!(case_dir(&a, &key).join("export-completed.json").exists());
        a.resume = true;
        ctx.provenance.started_unix_ms = 2;
        execute_case(&a, &fixture, &key, &ctx, &mut phase).unwrap();
        ctx.source_sha256 = "different-source".into();
        assert!(execute_case(&a, &fixture, &key, &ctx, &mut phase)
            .unwrap_err()
            .to_string()
            .contains("incompatible manifest"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn execution_completion_resumes_only_with_complete_hash_bound_evidence() {
        let dir = temp();
        let fixture = tiny_fixture();
        let mut a = args(&["run"]);
        a.out = dir.clone();
        let key = ExpectedCase {
            kind: "run".into(),
            cell: fixture.cell.id(),
            seed: fixture.seed,
            repeat: 0,
            rounds: fixture.rounds,
        };
        let mut ctx = test_context();
        let library = ivmlite_test::extension_library();
        ctx.extension_sha256 = Some(file_hash(&library).unwrap());
        ctx.library = Some(library);
        let mut phase = String::new();
        execute_case(&a, &fixture, &key, &ctx, &mut phase).unwrap();
        let case = case_dir(&a, &key);
        assert!(case.join("completed.json").exists());
        a.resume = true;
        execute_case(&a, &fixture, &key, &ctx, &mut phase).unwrap();
        write_json(&case.join("targets.json"), &Vec::<serde_json::Value>::new()).unwrap();
        assert!(execute_case(&a, &fixture, &key, &ctx, &mut phase)
            .unwrap_err()
            .to_string()
            .contains("artifact hash differs"));
        assert_eq!(phase, "resume_validation");
        fs::remove_dir_all(dir).unwrap();
    }
    fn smoke_run_args(out: &Path) -> Args {
        let mut a = args(&["run", "--smoke", "--seeds", "1", "--shape", "single"]);
        a.workload = repo().join("workloads/m2a.toml");
        a.out = out.to_owned();
        a.batch = Some(10);
        a
    }
    fn extension_context() -> Context {
        let mut ctx = test_context();
        let library = ivmlite_test::extension_library();
        ctx.extension_sha256 = Some(file_hash(&library).unwrap());
        ctx.library = Some(library);
        ctx
    }
    /// Three planned smoke cells, ordered as the runner executes them.
    fn three_smoke_keys(a: &Args) -> Vec<ExpectedCase> {
        let cfg = load_config(a).unwrap();
        let mut cells: Vec<_> = select_cells(&cfg, a)
            .unwrap()
            .into_iter()
            .map(|c| c.id())
            .collect();
        cells.sort();
        cells.truncate(3);
        cells
            .into_iter()
            .map(|cell| ExpectedCase {
                kind: "run".into(),
                cell,
                seed: 1,
                repeat: 0,
                rounds: cfg.total_rounds().unwrap(),
            })
            .collect()
    }
    fn set_injected_failure(cell: Option<&str>) {
        INJECTED_FAILURE.with(|slot| *slot.borrow_mut() = cell.map(str::to_owned));
    }
    fn stored_summary(a: &Args) -> Summary {
        read_json::<Selection>(&selection_path(a)).unwrap().summary
    }
    #[test]
    fn a_failing_case_is_recorded_and_the_rest_still_run() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let keys = three_smoke_keys(&a);
        a.cells = keys.iter().map(|k| k.cell.clone()).collect();
        set_injected_failure(Some(&keys[1].cell));
        let error = run_inner(&a, &extension_context()).unwrap_err();
        set_injected_failure(None);
        assert!(error.is::<CasesFailed>());
        assert!(error.to_string().contains("1 of 3 cases failed"), "{error}");
        assert!(case_dir(&a, &keys[0]).join("completed.json").exists());
        assert!(case_dir(&a, &keys[2]).join("completed.json").exists());
        let failed = case_dir(&a, &keys[1]);
        assert!(!failed.join("completed.json").exists());
        let failure: serde_json::Value = read_json(&failed.join("failure.json")).unwrap();
        assert_eq!(failure["phase"], "injected");
        assert_eq!(failure["error"], "injected");
        let summary = stored_summary(&a);
        assert_eq!((summary.planned, summary.complete), (3, 2));
        assert_eq!(
            summary.failed,
            vec![FailedCase {
                cell: keys[1].cell.clone(),
                seed: 1,
                repeat: 0,
                phase: "injected".into(),
                error: "injected".into(),
            }]
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn resume_skips_failed_cases_unless_retry_failed_is_set() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let keys = three_smoke_keys(&a);
        a.cells = keys.iter().map(|k| k.cell.clone()).collect();
        let ctx = extension_context();
        set_injected_failure(Some(&keys[1].cell));
        run_inner(&a, &ctx).unwrap_err();
        set_injected_failure(None);
        let failed = case_dir(&a, &keys[1]);

        a.resume = true;
        let error = run_inner(&a, &ctx).unwrap_err();
        assert!(error.to_string().contains("1 of 3 cases failed"), "{error}");
        assert!(!failed.join("completed.json").exists());
        assert!(failed.join("failure.json").exists());
        let summary = stored_summary(&a);
        assert_eq!((summary.complete, summary.failed.len()), (2, 1));
        assert_eq!(summary.failed[0].phase, "injected");

        a.retry_failed = true;
        run_inner(&a, &ctx).unwrap();
        assert!(failed.join("completed.json").exists());
        assert!(!failed.join("failure.json").exists());
        let summary = stored_summary(&a);
        assert_eq!((summary.planned, summary.complete), (3, 3));
        assert!(summary.failed.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_second_invocation_with_other_cells_leaves_the_first_untouched() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let keys = three_smoke_keys(&a);
        let ctx = extension_context();
        a.cells = vec![keys[0].cell.clone()];
        run_inner(&a, &ctx).unwrap();
        let first = case_dir(&a, &keys[0]);
        let marker_before = file_hash(&first.join("completed.json")).unwrap();
        let first_files = fs::read_dir(&first).unwrap().count();

        a.cells = vec![keys[1].cell.clone()];
        a.resume = true;
        run_inner(&a, &ctx).unwrap();
        assert_eq!(
            file_hash(&first.join("completed.json")).unwrap(),
            marker_before
        );
        assert_eq!(fs::read_dir(&first).unwrap().count(), first_files);
        assert!(!first.join("failure.json").exists());
        assert!(!dir.join("failure.json").exists());
        let summary = stored_summary(&a);
        assert_eq!((summary.planned, summary.complete), (2, 2));
        assert!(summary.failed.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rerun_without_resume_refuses_a_completed_case_and_writes_nothing() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let keys = three_smoke_keys(&a);
        a.cells = vec![keys[0].cell.clone()];
        let ctx = extension_context();
        run_inner(&a, &ctx).unwrap();
        let case = case_dir(&a, &keys[0]);
        let marker_before = file_hash(&case.join("completed.json")).unwrap();
        let selection_before = file_hash(&selection_path(&a)).unwrap();

        let error = run_inner(&a, &ctx).unwrap_err();
        assert!(error.to_string().contains("use --resume"), "{error}");
        assert!(!error.is::<CasesFailed>());
        assert_eq!(
            file_hash(&case.join("completed.json")).unwrap(),
            marker_before
        );
        assert!(!case.join("failure.json").exists());
        assert_eq!(file_hash(&selection_path(&a)).unwrap(), selection_before);
        let summary = stored_summary(&a);
        assert_eq!((summary.planned, summary.complete), (1, 1));
        assert!(summary.failed.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_retried_case_clears_its_own_top_level_failure_record() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let keys = three_smoke_keys(&a);
        a.cells = vec![keys[0].cell.clone()];
        let ctx = extension_context();
        set_injected_failure(Some(&keys[0].cell));
        run_inner(&a, &ctx).unwrap_err();
        set_injected_failure(None);
        assert!(dir.join("failure.json").exists());
        a.resume = true;
        a.retry_failed = true;
        run_inner(&a, &ctx).unwrap();
        assert!(!dir.join("failure.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn manifest_records_live_pragmas_plans_toolchain_and_target_selection() {
        let a = args(&["run", "--smoke"]);
        let m = manifest(&tiny_fixture(), &a, 0, &test_context()).unwrap();
        assert_eq!(m.pragmas["cache_size"], "-65536");
        assert_eq!(m.pragmas.len(), 7);
        assert_eq!(m.target_selection, "group_then_row");
        assert!(m.runner_rustc.starts_with("rustc "), "{}", m.runner_rustc);
        assert!(m.runner_rustc.contains("release:"), "{}", m.runner_rustc);
        assert!(!m.runner_rustc.contains('\n'));
        assert!(m.plans.covering_recompute.contains("COVERING INDEX"));
        assert!(m
            .plans
            .dml
            .values()
            .all(|p| p.contains("INTEGER PRIMARY KEY")));
        m2a_runner::assert_plans(&m.plans, m.cell.shape).unwrap();
    }
    #[test]
    fn an_oracle_mismatch_writes_failure_rows_next_to_the_failure() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        let key = three_smoke_keys(&a).remove(0);
        a.cells = vec![key.cell.clone()];
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE a(x INTEGER, y INTEGER); CREATE TABLE b(x INTEGER, y INTEGER);
                 INSERT INTO a VALUES(1,5),(2,3); INSERT INTO b VALUES(1,5),(3,3),(3,3);",
            )
            .unwrap();
        let mismatch = m2a_runner::oracle_check(&connection, "SELECT * FROM a", "SELECT * FROM b")
            .unwrap_err();
        let ctx = test_context();
        record_failure(
            &a,
            &ctx.provenance,
            Some(&ctx),
            Some(&key),
            "timing_ivmlite",
            &mismatch,
        )
        .unwrap();
        let rows: serde_json::Value =
            read_json(&case_dir(&a, &key).join("failure-rows.json")).unwrap();
        assert_eq!(rows["expected_only"], serde_json::json!([[3, 3], [3, 3]]));
        assert_eq!(rows["actual_only"], serde_json::json!([[2, 3]]));
        clear_failure(&a, &key).unwrap();
        assert!(!case_dir(&a, &key).join("failure-rows.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_failing_plan_assertion_is_a_plan_phase_case_failure() {
        let dir = temp();
        let mut a = smoke_run_args(&dir);
        a.shape = Some(Shape::Join);
        let keys = three_smoke_keys(&a);
        a.cells = keys.iter().map(|k| k.cell.clone()).collect();
        m2a_runner::DROP_ORDINARY_INDEXES.with(|flag| flag.set(true));
        let error = run_inner(&a, &extension_context()).unwrap_err();
        m2a_runner::DROP_ORDINARY_INDEXES.with(|flag| flag.set(false));
        assert!(error.is::<CasesFailed>(), "{error}");
        let summary = stored_summary(&a);
        assert_eq!((summary.planned, summary.complete), (3, 0));
        assert!(summary.failed.iter().all(|f| f.phase == "plan"));
        assert!(summary.failed[0].error.contains("join key index"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn retry_failed_requires_resume() {
        assert!(parse(["run", "--retry-failed"].map(str::to_owned)).is_err());
        assert!(parse(["run", "--resume", "--retry-failed"].map(str::to_owned)).is_ok());
    }
    /// The shipped workload with the churn rounds and the smoke single-table rows replaced.
    fn edited_workload(dir: &Path) -> PathBuf {
        let text = fs::read_to_string(repo().join("workloads/m2a.toml"))
            .unwrap()
            .replace("rounds = 100", "rounds = 7")
            .replace("single_rows = 4096", "single_rows = 512");
        let path = dir.join("workload.toml");
        fs::write(&path, text).unwrap();
        path
    }
    fn args_for(items: &[&str], workload: &Path, out: &Path) -> Args {
        let mut a = args(items);
        a.workload = workload.to_owned();
        a.out = out.to_owned();
        a
    }
    fn plan_of(a: &Args) -> (M2aConfig, BTreeSet<ExpectedCase>) {
        let cfg = load_config(a).unwrap();
        let cells = select_cells(&cfg, a).unwrap();
        let plan = planned_cases(a, &cfg, &cells).unwrap();
        (cfg, plan)
    }
    #[test]
    fn churn_and_smoke_come_from_the_workload_file() {
        let dir = temp();
        let workload = edited_workload(&dir);

        let (cfg, plan) = plan_of(&args_for(&["churn"], &workload, &dir));
        assert_eq!(cfg.churn.rounds, 7);
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|c| c.rounds == 7 && c.seed == 42));
        assert!(plan.iter().all(|c| c.cell.contains("_f64_b1000")));

        let (_, plan) = plan_of(&args_for(&["churn", "--rounds", "3"], &workload, &dir));
        assert!(plan.iter().all(|c| c.rounds == 3), "--rounds overrides");

        let smoke = args_for(&["churn", "--smoke"], &workload, &dir);
        let (cfg, plan) = plan_of(&smoke);
        assert_eq!(plan.len(), 4, "two profiles at both smoke batch sizes");
        assert!(plan
            .iter()
            .all(|c| c.cell.contains("_n4096_g64_f64_b") && c.rounds == 7));

        assert_eq!(cfg.single_rows, 512);
        let cell = cfg
            .cells()
            .unwrap()
            .into_iter()
            .find(|c| c.shape == Shape::Single)
            .unwrap();
        let fixture = cfg.generate(&cell, 1).unwrap();
        assert_eq!(fixture.tables[&Table::Events].len(), 512);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn smoke_and_full_runs_keep_separate_selection_files() {
        let out = Path::new("out");
        let full = args_for(&["run"], Path::new("w.toml"), out);
        let smoke = args_for(&["run", "--smoke"], Path::new("w.toml"), out);
        assert_eq!(selection_path(&full), out.join("selection-run.json"));
        assert_eq!(selection_path(&smoke), out.join("selection-run-smoke.json"));
    }
    /// Writes the smoke selection a finished `run --smoke` leaves: every smoke case planned,
    /// complete and carrying a completion marker.
    fn finished_smoke_run(a: &Args, ctx: &Context) {
        let smoke = M2aConfig::load(&a.workload).unwrap().smoke_variant();
        let plan = smoke_plan(&smoke).unwrap();
        for key in &plan {
            let dir = case_dir(a, key);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("completed.json"), "{}").unwrap();
        }
        let summary = Summary {
            planned: plan.len() as u64,
            complete: plan.len() as u64,
            failed: vec![],
        };
        write_json(
            &selection_file(&a.out, "run", true),
            &Selection {
                identity: SelectionIdentity {
                    schema_version: 1,
                    kind: "run".into(),
                    config_sha256: json_hash(&smoke).unwrap(),
                    source_sha256: ctx.source_sha256.clone(),
                    runner_sha256: ctx.runner_sha256.clone(),
                    extension_sha256: ctx.extension_sha256.clone(),
                    smoke: true,
                    hardware: ctx.provenance.hardware.clone(),
                },
                expected: plan,
                invocations: vec![],
                summary,
            },
        )
        .unwrap();
    }
    #[test]
    fn full_run_refuses_without_a_matching_completed_smoke() {
        let dir = temp();
        let workload = repo().join("workloads/m2a.toml");
        let mut ctx = test_context();
        ctx.extension_sha256 = Some("extension".into());

        let full = args_for(&["run"], &workload, &dir);
        let error = apply_smoke_gate(&full, &mut ctx).unwrap_err().to_string();
        assert!(error.contains("smoke"), "{error}");
        assert_eq!(ctx.provenance.smoke_gate, SmokeGate::NotApplicable);

        finished_smoke_run(&full, &ctx);
        apply_smoke_gate(&full, &mut ctx).unwrap();
        assert_eq!(ctx.provenance.smoke_gate, SmokeGate::Passed);

        let mut other_runner = test_context();
        other_runner.extension_sha256 = Some("extension".into());
        other_runner.runner_sha256 = "another runner".into();
        let error = apply_smoke_gate(&full, &mut other_runner).unwrap_err();
        assert!(error.to_string().contains("smoke"), "{error}");

        let mut other_extension = test_context();
        other_extension.extension_sha256 = Some("another extension".into());
        assert!(apply_smoke_gate(&full, &mut other_extension).is_err());

        let key = smoke_plan(&M2aConfig::load(&workload).unwrap().smoke_variant())
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        fs::remove_file(case_dir(&full, &key).join("completed.json")).unwrap();
        let error = apply_smoke_gate(&full, &mut ctx).unwrap_err();
        assert!(error.to_string().contains("not complete"), "{error}");

        let empty = temp();
        let mut skipping = args_for(&["run", "--skip-smoke-gate"], &workload, &empty);
        let mut ctx = test_context();
        apply_smoke_gate(&skipping, &mut ctx).unwrap();
        assert_eq!(ctx.provenance.smoke_gate, SmokeGate::Skipped);
        let recorded = manifest(&tiny_fixture(), &skipping, 0, &ctx).unwrap();
        let json = serde_json::to_value(&recorded).unwrap();
        assert_eq!(json["provenance"]["smoke_gate"], "skipped");

        skipping.smoke = true;
        let mut ctx = test_context();
        apply_smoke_gate(&skipping, &mut ctx).unwrap();
        assert_eq!(ctx.provenance.smoke_gate, SmokeGate::NotApplicable);
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(empty).unwrap();
    }
    #[test]
    fn the_gate_applies_only_to_full_runs() {
        let dir = temp();
        let workload = repo().join("workloads/m2a.toml");
        for items in [
            &["run", "--smoke"][..],
            &["churn"],
            &["confirm"],
            &["export"],
        ] {
            let mut ctx = test_context();
            apply_smoke_gate(&args_for(items, &workload, &dir), &mut ctx).unwrap();
            assert_eq!(
                ctx.provenance.smoke_gate,
                SmokeGate::NotApplicable,
                "{items:?}"
            );
        }
        assert!(parse(["run", "--smoke", "--skip-smoke-gate"].map(str::to_owned)).is_err());
        assert!(parse(["churn", "--skip-smoke-gate"].map(str::to_owned)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    /// The first pair is the plan's example and happens to survive a default decode;
    /// the second pair is one that a default `serde_json` decode shifts by one ULP.
    #[test]
    fn fixture_floats_round_trip_exactly() {
        let dir = temp();
        for (zipf_exponent, hot_group_fraction) in [
            (1.170_000_000_000_000_2, 0.300_000_000_000_000_04),
            (1.004_721_517_217_110_7, 0.104_940_354_630_094_99),
        ] {
            let mut config = cfg();
            config.single_rows = 800;
            config.single_groups = 20;
            config.join_rows = 800;
            config.join_groups = 20;
            config.fanouts = vec![8];
            config.batch_sizes = vec![10];
            config.zipf_exponent = zipf_exponent;
            config.hot_group_fraction = hot_group_fraction;
            let cell = config.cells().unwrap().remove(0);
            let f = config.generate(&cell, 1).unwrap();
            let path = dir.join(format!("fixture-{zipf_exponent}.json"));
            save_fixture(&path, &f).unwrap();
            let loaded: Fixture = read_json(&path).unwrap();
            assert_eq!(
                loaded.config.zipf_exponent.to_bits(),
                zipf_exponent.to_bits()
            );
            assert_eq!(
                loaded.config.hot_group_fraction.to_bits(),
                hot_group_fraction.to_bits()
            );
            assert_eq!(json_hash(&loaded).unwrap(), json_hash(&f).unwrap());
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn json_roundtrip_is_content_stable() {
        let a = serde_json::json!({"a":1,"b":["x",2]});
        let serialized = serde_json::to_vec(&a).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(json_hash(&a).unwrap(), json_hash(&decoded).unwrap());
    }
}
