//! Portable M2a export, correctness/state replay, timing and bounded churn.
#[cfg(test)]
#[path = "../m2a_fixtures.rs"]
mod m2a_fixtures;
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
    resume: bool,
    allow_dirty: bool,
    rounds: usize,
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
        resume: false,
        allow_dirty: false,
        rounds: 100,
        repeats: 5,
    };
    if !["export", "run", "verify", "churn", "confirm"].contains(&a.action.as_str()) {
        return Err("usage: m2a {export|run|verify|churn|confirm} [--workload FILE] [--out DIR] [--extension LIB] [--fixture FILE] [--cells IDS] [--profiles LABELS] [--shape single|join] [--fanout N] [--batch N] [--seeds N,...] [--smoke] [--resume] [--allow-dirty] [--rounds N] [--repeats N]".into());
    }
    let mut seen = BTreeSet::new();
    while let Some(flag) = it.next() {
        if !seen.insert(flag.clone()) {
            return Err(format!("duplicate option {flag}").into());
        }
        match flag.as_str() {
            "--smoke" => a.smoke = true,
            "--resume" => a.resume = true,
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
                    "--rounds" => a.rounds = value.parse()?,
                    "--repeats" => a.repeats = value.parse()?,
                    _ => return Err(format!("unknown option {flag}").into()),
                }
            }
        }
    }
    if a.rounds == 0 || a.repeats == 0 || a.fanout == Some(0) || a.batch == Some(0) {
        return Err("rounds/repeats/fanout/batch must be positive".into());
    }
    let only = |flag: &str, actions: &[&str]| -> Result<()> {
        if seen.contains(flag) && !actions.contains(&a.action.as_str()) {
            Err(format!("{flag} is not applicable to {}", a.action).into())
        } else {
            Ok(())
        }
    };
    only("--fixture", &["verify"])?;
    only("--rounds", &["churn"])?;
    only("--repeats", &["confirm"])?;
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
    rustc: String,
    hardware: Hardware,
    pragmas: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Selection {
    identity: SelectionIdentity,
    expected: BTreeSet<ExpectedCase>,
    invocations: Vec<Provenance>,
}
fn merge_selection(path: &Path, mut wanted: Selection) -> Result<()> {
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
    }
    write_json(path, &wanted)
}
fn load_config(a: &Args) -> Result<M2aConfig> {
    let mut cfg = M2aConfig::load(&a.workload)?;
    if a.smoke {
        cfg.single_rows = 4096;
        cfg.single_groups = 64;
        cfg.join_rows = 4096;
        cfg.join_groups = 64;
        cfg.batch_sizes = vec![10, 100];
    }
    cfg.validate()?;
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
                    && c.fanout == 64
                    && (a.smoke || c.batch_size == 1000)
                    && ["uniform_uniform", "zipf_hot"].contains(&c.profile.label()))
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
    let mut started = ctx.provenance.clone();
    started.started_unix_ms = now_ms();
    started.started_utc = command("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]);
    started.start_load = command("uptime", &[]);
    Ok(Manifest{schema_version:2,kind:a.action.clone(),cell:f.cell.clone(),seed:f.seed,repeat,rounds:f.rounds,smoke:a.smoke,fixture_sha256:json_hash(f)?,initial_data_sha256:json_hash(&f.tables)?,trace_sha256:json_hash(&f.batches)?,
        sql_sha256:hash(f.cell.view_sql().as_bytes()),view_sql:f.cell.view_sql().into(),schema_ddl:f.cell.schema_ddl().into(),ordinary_index_ddl:f.cell.ordinary_indexes().into(),covering_index_ddl:f.cell.covering_indexes().into(),config_sha256:json_hash(&f.config)?,
        source_revision:ctx.source_revision.clone(),source_sha256:ctx.source_sha256.clone(),extension_sha256:ctx.extension_sha256.clone(),runner_sha256:ctx.runner_sha256.clone(),sqlite_version:rusqlite::version().into(),rustc:command("rustc",&["--version"]),hardware:ctx.provenance.hardware.clone(),
        pragmas:"page_size=4096;cache_size=-65536;temp_store=MEMORY;journal_mode=MEMORY;synchronous=OFF;foreign_keys=ON;recursive_triggers=ON".into(),mode_order:(0..4).map(|i|MODES[(i+offset)%4].label().to_owned()).collect(),provenance:started})
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
        {
            return Err("missing/duplicate modes or incomplete measured rounds".into());
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
    match a.action.as_str() {
        "churn" => a.out.join(format!("churn_{}", key.cell)),
        "verify" => a
            .out
            .join(format!("verify_{}", key.cell))
            .join(format!("seed-{}", key.seed)),
        _ => a
            .out
            .join(&key.cell)
            .join(format!("seed-{}", key.seed))
            .join(if a.action == "confirm" {
                format!("confirm-repeat-{}", key.repeat)
            } else {
                format!("repeat-{}", key.repeat)
            }),
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
    let dir = case_dir(a, key);
    fs::create_dir_all(&dir)?;
    let export = a.action == "export";
    let manifest_name = if export {
        "export-manifest.json"
    } else {
        "manifest.json"
    };
    let marker = if export {
        "export-completed.json"
    } else {
        "completed.json"
    };
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
        *phase = "correctness".into();
        write_json(
            &dir.join("correctness.json"),
            &correctness(f, library, inverse)?,
        )?;
        *phase = "targets".into();
        write_json(&dir.join("targets.json"), &target_statistics(f)?)?;
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
    error: &dyn std::fmt::Display,
) -> Result<()> {
    let value = serde_json::json!({"schema_version":1,"invocation_id":provenance.invocation_id,"action":a.action,"cell":key.map(|k|&k.cell),"seed":key.map(|k|k.seed),"repeat":key.map(|k|k.repeat),"phase":phase,"error":error.to_string(),"source_sha256":ctx.map(|c|&c.source_sha256),"extension_sha256":ctx.and_then(|c|c.extension_sha256.as_ref()),"runner_sha256":ctx.map(|c|&c.runner_sha256),"provenance":provenance});
    write_json(&a.out.join("failure.json"), &value)?;
    if let Some(key) = key {
        let dir = case_dir(a, key);
        fs::create_dir_all(&dir)?;
        write_json(&dir.join("failure.json"), &value)?;
    }
    Ok(())
}
fn register_selection(
    a: &Args,
    cfg_hash: String,
    ctx: &Context,
    expected: BTreeSet<ExpectedCase>,
) -> Result<()> {
    merge_selection(
        &a.out.join(format!("selection-{}.json", a.action)),
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
        },
    )
}
fn run_inner(a: &Args, ctx: &Context) -> Result<()> {
    if a.action == "verify" {
        let mut phase = "fixture_load".to_owned();
        let f: Fixture = read_json(a.fixture.as_ref().unwrap())?;
        f.validate()?;
        let key = ExpectedCase {
            kind: a.action.clone(),
            cell: f.cell.id(),
            seed: f.seed,
            repeat: 0,
            rounds: f.rounds,
        };
        register_selection(a, json_hash(&f.config)?, ctx, BTreeSet::from([key.clone()]))?;
        if let Err(error) = execute_case(a, &f, &key, ctx, &mut phase) {
            record_failure(a, &ctx.provenance, Some(ctx), Some(&key), &phase, &error)?;
            return Err(error);
        }
        return Ok(());
    }
    let cfg = load_config(a)?;
    let cells = select_cells(&cfg, a)?;
    let seeds = if matches!(a.action.as_str(), "churn" | "confirm") {
        if !cfg.seeds.contains(&42) {
            return Err("seed 42 required for confirmation/churn".into());
        }
        vec![42]
    } else {
        a.seeds.clone().unwrap_or_else(|| cfg.seeds.clone())
    };
    let repeats = if a.action == "confirm" { a.repeats } else { 1 };
    let rounds = if a.action == "churn" {
        a.rounds
    } else {
        cfg.total_rounds()?
    };
    let mut expected = BTreeSet::new();
    for c in &cells {
        for &seed in &seeds {
            for repeat in 0..repeats {
                expected.insert(ExpectedCase {
                    kind: a.action.clone(),
                    cell: c.id(),
                    seed,
                    repeat,
                    rounds,
                });
            }
        }
    }
    register_selection(a, json_hash(&cfg)?, ctx, expected.clone())?;
    for key in expected {
        let mut phase = "generate".to_owned();
        let result = (|| -> Result<()> {
            let cell = cells
                .iter()
                .find(|c| c.id() == key.cell)
                .ok_or("planned cell disappeared")?;
            let f = if a.action == "churn" {
                cfg.generate_rounds(cell, key.seed, key.rounds)?
            } else {
                cfg.generate(cell, key.seed)?
            };
            execute_case(a, &f, &key, ctx, &mut phase)
        })();
        if let Err(error) = result {
            record_failure(a, &ctx.provenance, Some(ctx), Some(&key), &phase, &error)?;
            return Err(error);
        }
    }
    Ok(())
}
fn run(a: Args) -> Result<()> {
    fs::create_dir_all(&a.out)?;
    let provenance = provenance();
    let ctx = match context(&a, provenance.clone()) {
        Ok(ctx) => ctx,
        Err(error) => {
            record_failure(&a, &provenance, None, None, "setup", &error)?;
            return Err(error);
        }
    };
    if let Err(error) = run_inner(&a, &ctx) {
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
                &error,
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
            },
        )
        .unwrap();
        merge_selection(
            &path,
            Selection {
                identity: identity.clone(),
                expected: BTreeSet::from([second]),
                invocations: vec![],
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
                invocations: vec![]
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
    #[test]
    fn json_roundtrip_is_content_stable() {
        let a = serde_json::json!({"a":1,"b":["x",2]});
        let serialized = serde_json::to_vec(&a).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(json_hash(&a).unwrap(), json_hash(&decoded).unwrap());
    }
}
