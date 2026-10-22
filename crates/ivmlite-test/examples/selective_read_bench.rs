//! Identical native host for explicitly selected old/new extension libraries.
//! Latency and nested-work instrumentation run in separate processes.

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use ivmlite_test::demand_cases::KENER;
use ivmlite_test::demo_bench::{
    compare_values, create_view, multiset_difference, prepare_refresh, sorted_rows, Summary,
    Workload,
};
use ivmlite_test::open_with_extension_at;
use ivmlite_test::read_work::{ReadTrace, ReadWork};
use rusqlite::{Connection, Statement, StatementStatus};
use serde::Serialize;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const BATCH: usize = 500;
const WARMUPS: usize = 1;
const MEASURED: usize = 10;
const ROUNDS: usize = WARMUPS + MEASURED;
const DEFAULT_ROWS: usize = 100_000;
const MONITOR_ID: i64 = 123;
const BUCKET_START: i64 = 0;
const BUCKET_END: i64 = 9_000;
const COLUMNS: usize = 7;
const PROGRESS_INTERVAL: i32 = 100;
const CONFIG: &str = "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; \
    PRAGMA temp_store=MEMORY; PRAGMA cache_size=-2000; \
    PRAGMA foreign_keys=OFF; PRAGMA recursive_triggers=OFF;";
const USAGE: &str =
    "usage: selective_read_bench /absolute/path/to/LIBRARY latency|work [BASE_ROWS] [REPEATS]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Latency,
    Work,
}

#[derive(Debug)]
struct Arguments {
    library: PathBuf,
    mode: Mode,
    base_rows: usize,
    repeats: usize,
}

fn positive_count(value: OsString, name: &str) -> std::result::Result<usize, String> {
    let text = value
        .to_str()
        .ok_or_else(|| format!("{name} is not UTF-8"))?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{name} must be a positive decimal integer"));
    }
    text.parse::<usize>()
        .ok()
        .filter(|&count| count > 0)
        .ok_or_else(|| format!("{name} is zero or overflows usize"))
}

fn arguments(mut args: impl Iterator<Item = OsString>) -> std::result::Result<Arguments, String> {
    let library = PathBuf::from(args.next().ok_or(USAGE)?);
    if !library.is_absolute() {
        return Err(format!("LIBRARY must be absolute; {USAGE}"));
    }
    let mode = match args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .as_deref()
    {
        Some("latency") => Mode::Latency,
        Some("work") => Mode::Work,
        _ => return Err(USAGE.into()),
    };
    let base_rows = args
        .next()
        .map(|value| positive_count(value, "BASE_ROWS"))
        .transpose()?
        .unwrap_or(DEFAULT_ROWS);
    let repeats = args
        .next()
        .map(|value| positive_count(value, "REPEATS"))
        .transpose()?
        .unwrap_or(1);
    if args.next().is_some() {
        return Err(format!("extra arguments; {USAGE}"));
    }
    // KENER's public generator binds SQLite INTEGER ids through i64 casts.
    // Reserve even a whole batch of new ids for every round before those casts.
    let largest_id = base_rows
        .checked_add(BATCH * ROUNDS)
        .ok_or("BASE_ROWS plus mutation ids overflows usize")?;
    i64::try_from(largest_id)
        .map_err(|_| "BASE_ROWS plus mutation ids overflows SQLite INTEGER")?;
    repeats
        .checked_mul(ROUNDS)
        .ok_or("REPEATS times rounds overflows usize")?;
    KENER.validate(base_rows, BATCH, ROUNDS)?;
    Ok(Arguments {
        library,
        mode,
        base_rows,
        repeats,
    })
}

#[derive(Clone, Copy)]
struct Shape {
    name: &'static str,
    sql: &'static str,
    predicate: &'static str,
    binds: &'static [i64],
}

const SHAPES: [Shape; 4] = [
    Shape {
        name: "full",
        sql: "SELECT * FROM kener_bucket_rollup",
        predicate: "",
        binds: &[],
    },
    Shape {
        name: "monitor",
        sql: "SELECT * FROM kener_bucket_rollup WHERE monitor_id=?1",
        predicate: "WHERE monitor_id=123",
        binds: &[MONITOR_ID],
    },
    Shape {
        name: "window",
        sql: "SELECT * FROM kener_bucket_rollup WHERE monitor_id=?1 AND bucket_ts>=?2 AND bucket_ts<?3",
        predicate: "WHERE monitor_id=123 AND bucket_ts>=0 AND bucket_ts<9000",
        binds: &[MONITOR_ID, BUCKET_START, BUCKET_END],
    },
    Shape {
        name: "nonleading",
        sql: "SELECT * FROM kener_bucket_rollup WHERE bucket_ts>=?1 AND bucket_ts<?2",
        predicate: "WHERE bucket_ts>=0 AND bucket_ts<9000",
        binds: &[BUCKET_START, BUCKET_END],
    },
];

fn prepare_read(connection: &Connection, shape: Shape) -> rusqlite::Result<Statement<'_>> {
    let mut statement = connection.prepare(shape.sql)?;
    for (index, value) in shape.binds.iter().enumerate() {
        statement.raw_bind_parameter(index + 1, value)?;
    }
    Ok(statement)
}

/// Stream all rows and touch every public column without retaining host rows.
/// All parameters have already been bound; raw_query preserves those bindings.
fn consume(statement: &mut Statement<'_>) -> rusqlite::Result<usize> {
    let mut rows = statement.raw_query();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        for column in 0..COLUMNS {
            black_box(row.get_ref(column)?);
        }
        count += 1;
    }
    Ok(black_box(count))
}

fn check_returned_count(shape: Shape, returned: usize, expected: usize) -> Result<()> {
    if returned != expected {
        return Err(format!(
            "{} timed returned row count {returned}, reference expected {expected}",
            shape.name
        )
        .into());
    }
    Ok(())
}

/// Reexecute the actual prepared statement with its actual retained bindings.
/// Only the reference side uses the independently written expected predicate.
/// Collection/sorting and count checks happen after the round's timers end.
fn verify_read(
    statement: &mut Statement<'_>,
    reference: &Connection,
    shape: Shape,
    returned: Option<usize>,
) -> Result<usize> {
    let columns = statement.column_count();
    let mut actual = Vec::new();
    {
        let mut rows = statement.raw_query();
        while let Some(row) = rows.next()? {
            actual.push(
                (0..columns)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
    }
    actual.sort_by(|left, right| {
        left.iter()
            .zip(right)
            .map(|(left, right)| compare_values(left, right))
            .find(|order| order.is_ne())
            .unwrap_or_else(|| left.len().cmp(&right.len()))
    });
    let expected = sorted_rows(
        reference,
        &format!("SELECT * FROM ({}) {}", KENER.view_sql(), shape.predicate),
    )?;
    if let Some(difference) = multiset_difference(&actual, &expected) {
        return Err(format!("{} reference GROUP BY mismatch: {difference}", shape.name).into());
    }
    if let Some(returned) = returned {
        check_returned_count(shape, returned, expected.len())?;
    }
    Ok(expected.len())
}

/// Ordered streaming comparison keeps the source-scale base oracle bounded in
/// host memory and distinguishes every storage class and every REAL bit.
fn same_base_rows(connection: &Connection, reference: &Connection) -> Result<()> {
    let sql = "SELECT id,monitor_id,bucket_ts,status,latency_ms FROM monitoring_facts ORDER BY id";
    let mut actual_statement = connection.prepare(sql)?;
    let mut expected_statement = reference.prepare(sql)?;
    let mut actual_rows = actual_statement.query([])?;
    let mut expected_rows = expected_statement.query([])?;
    let mut index = 0;
    loop {
        match (actual_rows.next()?, expected_rows.next()?) {
            (None, None) => return Ok(()),
            (Some(actual), Some(expected)) => {
                for column in 0..5 {
                    let actual = actual.get_ref(column)?;
                    let expected = expected.get_ref(column)?;
                    let equal = match (actual, expected) {
                        (
                            rusqlite::types::ValueRef::Real(left),
                            rusqlite::types::ValueRef::Real(right),
                        ) => left.to_bits() == right.to_bits(),
                        _ => actual == expected,
                    };
                    if !equal {
                        return Err(format!("base replay mismatch at ordered row {index}, column {column}: {actual:?} != {expected:?}").into());
                    }
                }
                index += 1;
            }
            _ => {
                return Err(
                    format!("base replay row-count mismatch after {index} ordered rows").into(),
                )
            }
        }
    }
}

fn allocation(connection: &Connection) -> Result<Value> {
    let pages: i64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let size: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let bytes = pages.checked_mul(size).ok_or("page allocation overflow")?;
    Ok(json!({"page_count": pages, "page_size": size, "allocated_page_bytes": bytes}))
}

fn command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "{program} {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn metadata(args: &Arguments, library: &Path) -> Result<Value> {
    let connection = open_with_extension_at(None, library)?;
    let (sqlite_version, sqlite_source_id): (String, String) =
        connection.query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let mut options = connection.prepare("PRAGMA compile_options")?;
    let sqlite_compile_options = options
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({
        "library_argument": args.library,
        "library_canonical_path": library,
        "executable": std::env::current_exe()?,
        "command_arguments": std::env::args_os().map(|argument| argument.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "command_arguments_os": std::env::args_os().collect::<Vec<_>>(),
        "working_directory": std::env::current_dir()?,
        "host_source_head_observed_at_run": command("git", &["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])?,
        "rustc_observed_at_run": command("rustc", &["--version", "--verbose"])?,
        "platform": command("uname", &["-a"])?,
        "sqlite_version": sqlite_version,
        "sqlite_source_id": sqlite_source_id,
        "sqlite_compile_options": sqlite_compile_options,
        "configuration_sql": CONFIG,
        "base_rows": args.base_rows, "repeats": args.repeats,
        "batch": BATCH, "warmup_rounds": WARMUPS, "measured_rounds": MEASURED,
        "round_index_origin": 0, "repeat_index_origin": 0,
        "monitor_id": MONITOR_ID, "bucket_start_inclusive": BUCKET_START,
        "bucket_end_exclusive": BUCKET_END,
        "read_consumption": "fully drain; get_ref and black_box all seven columns; no retained timed rows",
        "oracle": "independent ordinary connection seeded freshly per repeat; replay same workload schedule after actual COMMIT outside timers; ordered streaming base storage-class/REAL-bit equality and reference GROUP BY full multisets initial/after warmup/final; actual prepared/bound selected statements reexecuted and compared with independent reference predicates after every round; all timed read counts compared with reference GROUP BY counts each round",
        "source_identity_note": "observed HEAD/compiler are run metadata; externally record binary, harness source and each library SHA256 and actual build command",
    }))
}

#[derive(Serialize)]
struct WorkMeasurement {
    outer_fullscan_steps: i32,
    outer_vm_steps: i32,
    nested: Vec<ReadWork>,
    nested_eqp: Vec<Vec<String>>,
    progress_interval_vm_instructions: i32,
    whole_query_progress_callbacks_separate_pass: usize,
    separate_pass_outer_rows: usize,
}

fn measure_work(
    connection: &Connection,
    statement: &mut Statement<'_>,
) -> Result<(usize, WorkMeasurement)> {
    statement.reset_status(StatementStatus::FullscanStep);
    statement.reset_status(StatementStatus::VmStep);
    let trace = ReadTrace::start(connection)?;
    let outer_rows = consume(statement)?;
    let nested = trace.finish()?;
    if nested.is_empty() {
        return Err("trace captured no nested output SELECT".into());
    }
    let outer_fullscan_steps = statement.get_status(StatementStatus::FullscanStep);
    let outer_vm_steps = statement.get_status(StatementStatus::VmStep);
    // EXPLAIN happens only after the trace has unregistered and uses the exact
    // expanded binds observed at PROFILE, never a synthesized plan label.
    let nested_eqp = nested
        .iter()
        .map(|read| {
            let mut explain =
                connection.prepare(&format!("EXPLAIN QUERY PLAN {}", read.expanded_sql))?;
            let rows = explain
                .query_map([], |row| row.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let callbacks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&callbacks);
    connection.progress_handler(
        PROGRESS_INTERVAL,
        Some(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )?;
    let separate_result = consume(statement);
    connection.progress_handler(0, None::<fn() -> bool>)?;
    let separate_pass_outer_rows = separate_result?;
    if separate_pass_outer_rows != outer_rows {
        return Err("progress-only read changed the returned row count".into());
    }
    Ok((
        outer_rows,
        WorkMeasurement {
            outer_fullscan_steps,
            outer_vm_steps,
            nested,
            nested_eqp,
            progress_interval_vm_instructions: PROGRESS_INTERVAL,
            whole_query_progress_callbacks_separate_pass: callbacks.load(Ordering::Relaxed),
            separate_pass_outer_rows,
        },
    ))
}

#[derive(Serialize)]
struct ReadMeasurement {
    shape: &'static str,
    outer_rows: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    work: Option<WorkMeasurement>,
}

#[derive(Serialize)]
struct Round {
    repeat: usize,
    round: usize,
    warmup: bool,
    read_order: Vec<&'static str>,
    reads: Vec<ReadMeasurement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings_ms: Option<BTreeMap<String, f64>>,
}

fn step<T>(
    timings: &mut Option<BTreeMap<String, f64>>,
    name: &str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match timings {
        Some(timings) => {
            let start = Instant::now();
            let value = action()?;
            let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
            timings.insert(name.to_owned(), elapsed_ms);
            Ok(value)
        }
        None => action(),
    }
}

fn summarize(rounds: &[Round]) -> Value {
    let measured: Vec<_> = rounds
        .iter()
        .filter(|round| !round.warmup)
        .filter_map(|round| round.timings_ms.as_ref())
        .collect();
    let Some(first) = measured.first() else {
        return Value::Null;
    };
    let mut summary = serde_json::Map::new();
    for key in first.keys() {
        let values = measured
            .iter()
            .map(|timings| timings[key])
            .collect::<Vec<_>>();
        if let Some(value) = Summary::of(&values) {
            summary.insert(
                key.clone(),
                json!({"median": value.median, "min": value.min, "max": value.max}),
            );
        }
    }
    Value::Object(summary)
}

fn fixture(args: &Arguments, library: &Path, repeat: usize) -> Result<Value> {
    let connection = open_with_extension_at(None, library)?;
    connection.execute_batch(CONFIG)?;
    connection.execute_batch(KENER.ddl())?;
    KENER.seed(&connection, args.base_rows)?;
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(CONFIG)?;
    reference.execute_batch(KENER.ddl())?;
    KENER.seed(&reference, args.base_rows)?;
    let before_bootstrap = allocation(&connection)?;
    let mut bootstrap = (args.mode == Mode::Latency).then(BTreeMap::new);
    step(&mut bootstrap, "create_view", || {
        Ok(create_view(
            &connection,
            KENER.view_name(),
            KENER.view_sql(),
        )?)
    })?;
    let after_bootstrap = allocation(&connection)?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let temp_store: i64 = connection.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    let mut begin = connection.prepare("BEGIN")?;
    let mut commit = connection.prepare("COMMIT")?;
    let mut mutations = KENER
        .batch_sql()
        .iter()
        .map(|sql| connection.prepare(sql))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut reference_begin = reference.prepare("BEGIN")?;
    let mut reference_commit = reference.prepare("COMMIT")?;
    let mut reference_mutations = KENER
        .batch_sql()
        .iter()
        .map(|sql| reference.prepare(sql))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut refresh = prepare_refresh(&connection, KENER.view_name())?;
    let shape_count = if args.mode == Mode::Latency {
        3
    } else {
        SHAPES.len()
    };
    let mut statements = SHAPES[..shape_count]
        .iter()
        .map(|&shape| prepare_read(&connection, shape))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    same_base_rows(&connection, &reference)?;
    let initial_groups = verify_read(&mut statements[0], &reference, SHAPES[0], None)?;
    let mut rounds = Vec::new();
    for round in 0..ROUNDS {
        let mut timings = (args.mode == Mode::Latency).then(BTreeMap::new);
        step(&mut timings, "begin", || Ok(begin.execute([])?))?;
        // PreparedBatch::execute owns another transaction. Call the public
        // workload function with preprepared steps inside this single BEGIN.
        step(&mut timings, "write", || {
            Ok((KENER.apply_fn())(
                &mut mutations,
                args.base_rows,
                BATCH,
                round,
            )?)
        })?;
        step(&mut timings, "refresh", || Ok(refresh.execute([])?))?;
        let mut reads = Vec::new();
        let mut read_order = Vec::new();
        for offset in 0..shape_count {
            let index = (round + offset) % shape_count;
            let shape = SHAPES[index];
            read_order.push(shape.name);
            let (outer_rows, work) = match args.mode {
                Mode::Latency => (
                    step(&mut timings, shape.name, || {
                        Ok(consume(&mut statements[index])?)
                    })?,
                    None,
                ),
                Mode::Work => {
                    let (rows, work) = measure_work(&connection, &mut statements[index])?;
                    (rows, Some(work))
                }
            };
            reads.push(ReadMeasurement {
                shape: shape.name,
                outer_rows,
                work,
            });
        }
        step(&mut timings, "commit", || Ok(commit.execute([])?))?;
        reference_begin.execute([])?;
        (KENER.apply_fn())(&mut reference_mutations, args.base_rows, BATCH, round)?;
        reference_commit.execute([])?;
        if let Some(timings) = &mut timings {
            let write_refresh = timings["write"] + timings["refresh"];
            let transaction = timings["begin"] + write_refresh + timings["commit"];
            timings.insert("write_plus_refresh".into(), write_refresh);
            for shape in &SHAPES[..3] {
                // Each total models one selected read from this same round.
                // Other reads executed in the transaction are excluded.
                timings.insert(
                    format!("end_to_end_{}", shape.name),
                    transaction + timings[shape.name],
                );
                timings.insert(
                    format!("write_refresh_read_commit_{}", shape.name),
                    write_refresh + timings[shape.name] + timings["commit"],
                );
            }
        }
        // Oracle queries and their allocations are never inside any timer.
        let full_checkpoint = round + 1 == WARMUPS || round + 1 == ROUNDS;
        for (index, &shape) in SHAPES[..shape_count].iter().enumerate() {
            let returned = reads
                .iter()
                .find(|read| read.shape == shape.name)
                .ok_or("missing timed read measurement")?
                .outer_rows;
            if index == 0 && !full_checkpoint {
                let expected: i64 = reference.query_row(
                    &format!("SELECT COUNT(*) FROM ({})", KENER.view_sql()),
                    [],
                    |row| row.get(0),
                )?;
                check_returned_count(shape, returned, usize::try_from(expected)?)?;
            } else {
                verify_read(&mut statements[index], &reference, shape, Some(returned))?;
            }
        }
        if full_checkpoint {
            same_base_rows(&connection, &reference)?;
        }
        rounds.push(Round {
            repeat,
            round,
            warmup: round < WARMUPS,
            read_order,
            reads,
            timings_ms: timings,
        });
    }
    let mut result = json!({
        "repeat": repeat, "initial_groups": initial_groups,
        "configuration_observed": {"journal_mode": journal_mode, "synchronous": synchronous, "temp_store": temp_store},
        "allocation_before_bootstrap": before_bootstrap,
        "allocation_after_bootstrap": after_bootstrap,
        "allocation_final": allocation(&connection)?,
        "rounds": rounds,
        "oracle_checks_passed": true,
    });
    if let Some(bootstrap) = bootstrap {
        result["bootstrap_ms"] = json!(bootstrap["create_view"]);
        result["measured_round_summary_ms"] = summarize(&rounds);
    }
    Ok(result)
}

fn run(args: &Arguments) -> Result<Value> {
    if !args.library.is_file() {
        return Err(format!("LIBRARY is not a file: {}", args.library.display()).into());
    }
    let library = args.library.canonicalize()?;
    let metadata = metadata(args, &library)?;
    let mut repeats = Vec::new();
    for repeat in 0..args.repeats {
        eprintln!(
            "selective_read_bench: repeat {}/{}; {:?}; {} base rows",
            repeat + 1,
            args.repeats,
            args.mode,
            args.base_rows
        );
        repeats.push(fixture(args, &library, repeat)?);
    }
    Ok(
        json!({"schema_version": 1, "mode": if args.mode == Mode::Latency { "latency" } else { "work" }, "metadata": metadata, "repeats": repeats}),
    )
}

fn main() -> Result<()> {
    let args = arguments(std::env::args_os().skip(1))?;
    let output = run(&args)?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &output)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> std::result::Result<Arguments, String> {
        arguments(values.iter().map(OsString::from))
    }

    #[test]
    fn cli_accepts_default_quick_and_source_scale_contract() {
        let quick = parse(&["/absolute/library", "latency"]).unwrap();
        assert_eq!(
            (quick.base_rows, quick.repeats, quick.mode),
            (100_000, 1, Mode::Latency)
        );
        let scale = parse(&["/absolute/library", "work", "4100000", "5"]).unwrap();
        assert_eq!(
            (scale.base_rows, scale.repeats, scale.mode),
            (4_100_000, 5, Mode::Work)
        );
    }

    #[test]
    fn cli_refuses_ambiguous_invalid_and_unrepresentable_workloads() {
        for values in [
            vec![],
            vec!["/library"],
            vec!["relative", "latency"],
            vec!["/library", "profile"],
            vec!["/library", "latency", "0"],
            vec!["/library", "work", "100000", "0"],
            vec!["/library", "work", "100000", "1", "extra"],
            vec!["/library", "work", "-1"],
            vec!["/library", "work", "1.5"],
            vec!["/library", "work", "10"], // overlapping KENER mutation ranges
            vec!["/library", "work", "18446744073709551616"],
            vec!["/library", "work", "9223372036854775807"],
            vec!["/library", "work", "100000", "18446744073709551615"],
        ] {
            assert!(parse(&values).is_err(), "accepted {values:?}");
        }
    }

    #[test]
    fn streaming_reads_consume_all_columns_and_bound_filters_without_retaining_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE kener_bucket_rollup(monitor_id,bucket_ts,status,a,b,c,n); \
            INSERT INTO kener_bucket_rollup VALUES(123,0,'UP',1.0,1.0,1.0,1), \
            (123,9000,'UP',2.0,2.0,2.0,1), (9,0,'UP',3.0,3.0,3.0,1)",
            )
            .unwrap();
        for (shape, count) in SHAPES.into_iter().zip([3, 2, 1, 2]) {
            let mut statement = prepare_read(&connection, shape).unwrap();
            assert_eq!(consume(&mut statement).unwrap(), count, "{}", shape.name);
            // A reset preserves all bindings, as required for every later round.
            assert_eq!(consume(&mut statement).unwrap(), count, "{}", shape.name);
        }
        let mut short = connection.prepare("SELECT 1,2,3,4,5,6").unwrap();
        assert!(
            consume(&mut short).is_err(),
            "must access the seventh column"
        );
    }

    #[test]
    fn prepared_kener_schedule_preserves_single_transaction_and_replays_identically() {
        let mut snapshots = Vec::new();
        for _ in 0..2 {
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(KENER.ddl()).unwrap();
            KENER.seed(&connection, 10_000).unwrap();
            let before =
                ivmlite_test::demo_bench::sorted_rows(&connection, KENER.view_sql()).unwrap();
            let mut statements = KENER
                .batch_sql()
                .iter()
                .map(|sql| connection.prepare(sql).unwrap())
                .collect::<Vec<_>>();
            let mut begin = connection.prepare("BEGIN").unwrap();
            let mut commit = connection.prepare("COMMIT").unwrap();
            for round in 0..ROUNDS {
                begin.execute([]).unwrap();
                (KENER.apply_fn())(&mut statements, 10_000, BATCH, round).unwrap();
                assert!(
                    !connection.is_autocommit(),
                    "workload must leave our transaction open"
                );
                commit.execute([]).unwrap();
            }
            let after =
                ivmlite_test::demo_bench::sorted_rows(&connection, KENER.view_sql()).unwrap();
            assert_ne!(before, after);
            let base_rows: i64 = connection
                .query_row("SELECT count(*) FROM monitoring_facts", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                base_rows, 11_375,
                "11 batches each insert 200 and delete 75"
            );
            snapshots.push(after);
        }
        assert_eq!(
            snapshots[0], snapshots[1],
            "fresh repeats must replay the same trace"
        );
    }

    #[test]
    fn independent_base_oracle_detects_a_skipped_actual_batch() {
        let actual = Connection::open_in_memory().unwrap();
        let reference = Connection::open_in_memory().unwrap();
        for connection in [&actual, &reference] {
            connection.execute_batch(KENER.ddl()).unwrap();
            KENER.seed(connection, 10_000).unwrap();
        }
        same_base_rows(&actual, &reference).unwrap();
        KENER
            .apply_mixed_batch(&reference, 10_000, BATCH, 0)
            .unwrap();
        assert!(same_base_rows(&actual, &reference).is_err());
        KENER.apply_mixed_batch(&actual, 10_000, BATCH, 0).unwrap();
        same_base_rows(&actual, &reference).unwrap();
        actual
            .execute(
                "UPDATE monitoring_facts SET latency_ms=latency_ms+0.25 WHERE id=5000",
                [],
            )
            .unwrap();
        assert!(same_base_rows(&actual, &reference).is_err());
    }

    #[test]
    fn read_oracle_rejects_wrong_actual_sql_binds_and_substituted_full_read_counts() {
        let actual = Connection::open_in_memory().unwrap();
        let reference = Connection::open_in_memory().unwrap();
        for connection in [&actual, &reference] {
            connection.execute_batch(KENER.ddl()).unwrap();
            KENER.seed(connection, 20_000).unwrap();
        }
        actual
            .execute_batch(&format!(
                "CREATE VIEW kener_bucket_rollup AS {}",
                KENER.view_sql()
            ))
            .unwrap();
        let mut correct = prepare_read(&actual, SHAPES[1]).unwrap();
        assert_eq!(
            verify_read(&mut correct, &reference, SHAPES[1], Some(12)).unwrap(),
            12
        );

        let wrong_sql = Shape {
            sql: "SELECT * FROM kener_bucket_rollup WHERE monitor_id<>?1",
            ..SHAPES[1]
        };
        let mut statement = prepare_read(&actual, wrong_sql).unwrap();
        assert!(verify_read(&mut statement, &reference, wrong_sql, None).is_err());

        // Neighboring monitors have the same number of groups here. Only
        // comparing row counts would miss the wrong binding's different keys.
        let wrong_bind = Shape {
            binds: &[122],
            ..SHAPES[1]
        };
        let mut statement = prepare_read(&actual, wrong_bind).unwrap();
        assert_eq!(consume(&mut statement).unwrap(), 12);
        assert!(verify_read(&mut statement, &reference, wrong_bind, Some(12)).is_err());

        let mut full = prepare_read(&actual, SHAPES[0]).unwrap();
        let full_rows = consume(&mut full).unwrap();
        assert_eq!(full_rows, 2_682);
        let error = verify_read(&mut correct, &reference, SHAPES[1], Some(full_rows))
            .unwrap_err()
            .to_string();
        assert!(error.contains("timed returned row count"), "{error}");
        // Also reject using that full statement as the actual selected SQL.
        assert!(verify_read(&mut full, &reference, SHAPES[1], Some(full_rows)).is_err());
    }

    #[test]
    fn native_fixture_run_checks_real_mutation_and_read_calls_in_both_modes() {
        let library = ivmlite_test::extension_library();
        for mode in [Mode::Latency, Mode::Work] {
            let args = Arguments {
                library: library.clone(),
                mode,
                base_rows: 20_000,
                repeats: 1,
            };
            // This invokes the real fixture's timed apply/refresh/read callsites,
            // independent replay and all post-timer assertions. Skipping its
            // actual apply or reading statements[0] for each shape must fail.
            let output = run(&args).unwrap();
            let repeat = &output["repeats"][0];
            assert_eq!(repeat["initial_groups"], 2_682);
            assert_eq!(repeat["oracle_checks_passed"], true);
            let rounds = repeat["rounds"].as_array().unwrap();
            assert_eq!(rounds.len(), 11);
            assert_eq!(
                rounds
                    .iter()
                    .filter(|round| round["warmup"] == true)
                    .count(),
                1
            );
            for round in rounds {
                let rows = round["reads"].as_array().unwrap();
                let count = |shape| {
                    rows.iter().find(|row| row["shape"] == shape).unwrap()["outer_rows"]
                        .as_u64()
                        .unwrap()
                };
                assert!(count("full") > count("monitor"));
                assert!(count("monitor") >= count("window"));
                if mode == Mode::Work {
                    assert!(round.get("timings_ms").is_none());
                    assert!(rows
                        .iter()
                        .all(|row| !row["work"]["nested"].as_array().unwrap().is_empty()));
                }
            }
            if mode == Mode::Work {
                assert!(repeat.get("bootstrap_ms").is_none());
                assert!(repeat.get("measured_round_summary_ms").is_none());
            }
        }
    }
}
