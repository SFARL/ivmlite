//! Same-host benchmark for the noop, Zcash and Kener demand-shaped demos.
//!
//! Usage:
//! `cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
//!  <case> [rows] [batch] [repeats]`

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use ivmlite_test::demand_cases::{by_name, DemandCase, ALL};
use rusqlite::Connection;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
enum Mode {
    NoMaintenance,
    FullRecompute,
    Ivmlite,
}

impl Mode {
    const ALL: [Self; 3] = [Self::NoMaintenance, Self::FullRecompute, Self::Ivmlite];

    fn name(self) -> &'static str {
        match self {
            Self::NoMaintenance => "no_maintenance",
            Self::FullRecompute => "full_recompute",
            Self::Ivmlite => "ivmlite",
        }
    }
}

#[derive(Clone, Copy)]
struct Timing {
    bootstrap_ms: f64,
    apply_ms: f64,
    maintain_ms: f64,
    read_ms: f64,
    database_kib: i64,
}

fn open_release_extension() -> Result<Connection> {
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ivmlite-sqlite/target/release")
        .join(if cfg!(target_os = "macos") {
            "libivmlite_sqlite.dylib"
        } else {
            "libivmlite_sqlite.so"
        });
    if !library.is_file() {
        return Err(format!(
            "the release extension is missing at {}; run `cargo build --release --locked \
             --manifest-path crates/ivmlite-sqlite/Cargo.toml`",
            library.display()
        )
        .into());
    }

    let connection = Connection::open_in_memory()?;
    // SAFETY: load this repository's release library through its explicit
    // entry point, then disable extension loading immediately.
    unsafe {
        connection.load_extension_enable()?;
        connection.load_extension(&library, Some("sqlite3_ivmlite_init"))?;
        connection.load_extension_disable()?;
    }
    Ok(connection)
}

fn create_view(connection: &Connection, case: &DemandCase) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {} USING ivm('{}')",
        case.view_name,
        case.view_sql.replace('\'', "''")
    ))
}

fn refresh(connection: &Connection, view_name: &str) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "INSERT INTO {view_name}({view_name}) VALUES ('refresh')"
    ))
}

fn drain(connection: &Connection, sql: &str) -> rusqlite::Result<usize> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok(count)
}

fn materialized_matches_oracle(
    connection: &Connection,
    case: &DemandCase,
) -> rusqlite::Result<bool> {
    let mismatch: i64 = connection.query_row(
        &format!(
            "SELECT EXISTS(SELECT * FROM ({}) EXCEPT SELECT * FROM {}) \
             OR EXISTS(SELECT * FROM {} EXCEPT SELECT * FROM ({}))",
            case.view_sql, case.view_name, case.view_name, case.view_sql
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(mismatch == 0)
}

fn database_kib(connection: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages * page_size / 1024)
}

fn run_once(case: &DemandCase, mode: Mode, rows: usize, batch: usize) -> Result<Timing> {
    let connection = open_release_extension()?;
    connection.execute_batch(case.ddl)?;
    case.seed(&connection, rows)?;

    let start = Instant::now();
    if matches!(mode, Mode::Ivmlite) {
        create_view(&connection, case)?;
    }
    let bootstrap_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    case.apply_mixed_batch(&connection, rows, batch)?;
    let apply_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    match mode {
        Mode::NoMaintenance => {}
        Mode::FullRecompute => {
            black_box(drain(&connection, case.view_sql)?);
        }
        Mode::Ivmlite => refresh(&connection, case.view_name)?,
    }
    let maintain_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    if matches!(mode, Mode::Ivmlite) {
        black_box(drain(
            &connection,
            &format!("SELECT * FROM {}", case.view_name),
        )?);
    }
    let read_ms = start.elapsed().as_secs_f64() * 1_000.0;

    if matches!(mode, Mode::Ivmlite) && !materialized_matches_oracle(&connection, case)? {
        return Err(format!("{} diverged from SQLite recomputation", case.name).into());
    }

    Ok(Timing {
        bootstrap_ms,
        apply_ms,
        maintain_ms,
        read_ms,
        database_kib: database_kib(&connection)?,
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn median_i64(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn usage() -> String {
    let names = ALL
        .iter()
        .map(|case| case.name)
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "usage: demand_case_bench <{names}> [positive rows >= batch] \
         [positive batch] [positive repeats]"
    )
}

fn arguments() -> Result<(&'static DemandCase, usize, usize, usize)> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or_else(usage)?;
    let case = by_name(&name).ok_or_else(usage)?;
    let rows = args
        .next()
        .map_or(Ok(case.default_rows), |value| value.parse())?;
    let batch = args
        .next()
        .map_or(Ok(case.default_batch), |value| value.parse())?;
    let repeats = args.next().map_or(Ok(5), |value| value.parse())?;
    if args.next().is_some() || rows == 0 || batch == 0 || repeats == 0 || rows < batch {
        return Err(usage().into());
    }
    Ok((case, rows, batch, repeats))
}

fn main() -> Result<()> {
    let (case, rows, batch, repeats) = arguments()?;
    eprintln!(
        "{}: rows={rows}, mixed_batch={batch}, repeats={repeats}; median reported",
        case.name
    );
    println!(
        "case,mode,base_rows,batch_size,repeats,bootstrap_ms,apply_ms,maintain_ms,read_ms,\
         apply_plus_maintain_ms,end_to_end_ms,database_kib"
    );

    for mode in Mode::ALL {
        let timings: Vec<_> = (0..repeats)
            .map(|_| run_once(case, mode, rows, batch))
            .collect::<Result<_>>()?;
        let bootstrap_ms = median(timings.iter().map(|timing| timing.bootstrap_ms).collect());
        let apply_ms = median(timings.iter().map(|timing| timing.apply_ms).collect());
        let maintain_ms = median(timings.iter().map(|timing| timing.maintain_ms).collect());
        let read_ms = median(timings.iter().map(|timing| timing.read_ms).collect());
        let database_kib = median_i64(timings.iter().map(|timing| timing.database_kib).collect());
        println!(
            "{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{}",
            case.name,
            mode.name(),
            rows,
            batch,
            repeats,
            bootstrap_ms,
            apply_ms,
            maintain_ms,
            read_ms,
            apply_ms + maintain_ms,
            apply_ms + maintain_ms + read_ms,
            database_kib
        );
    }
    Ok(())
}
