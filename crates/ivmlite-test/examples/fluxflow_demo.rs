//! Source-shaped FluxFlow aggregate demo and same-host benchmark.
//!
//! Usage:
//! `cargo run --release --locked -p ivmlite-test --example fluxflow_demo -- [rows] [batch] [repeats]`
//!
//! See `docs/demos/fluxflow.md` for provenance, adaptations and timing rules.

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use ivmlite_test::fluxflow::{
    apply_mixed_batch, install_handwritten_rollup, seed, FLOW_TABLE_DDL, ROLLUP_TABLE, VIEW_SQL,
};
use rusqlite::types::Value;
use rusqlite::Connection;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const IVM_VIEW: &str = "fluxflow_stats";

#[derive(Clone, Copy)]
enum Mode {
    NoMaintenance,
    FullRecompute,
    HandwrittenTrigger,
    Ivmlite,
}

impl Mode {
    const ALL: [Self; 4] = [
        Self::NoMaintenance,
        Self::FullRecompute,
        Self::HandwrittenTrigger,
        Self::Ivmlite,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::NoMaintenance => "no_maintenance",
            Self::FullRecompute => "full_recompute",
            Self::HandwrittenTrigger => "handwritten_trigger",
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

    let c = Connection::open_in_memory()?;
    // SAFETY: load this repository's release library through its explicit
    // entry point, then disable extension loading immediately.
    unsafe {
        c.load_extension_enable()?;
        c.load_extension(&library, Some("sqlite3_ivmlite_init"))?;
        c.load_extension_disable()?;
    }
    Ok(c)
}

fn sorted_rows(c: &Connection, sql: &str) -> rusqlite::Result<Vec<Vec<Value>>> {
    let mut statement = c.prepare(sql)?;
    let columns = statement.column_count();
    let mut rows: Vec<Vec<Value>> = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, Value>(column))
                .collect()
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
    Ok(rows)
}

fn create_ivm_view(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {IVM_VIEW} USING ivm('{}')",
        VIEW_SQL.replace('\'', "''")
    ))
}

fn refresh_ivm(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "INSERT INTO {IVM_VIEW}({IVM_VIEW}) VALUES ('refresh')"
    ))
}

fn drain(c: &Connection, sql: &str) -> rusqlite::Result<usize> {
    let mut statement = c.prepare(sql)?;
    let mut result = statement.query([])?;
    let mut rows = 0;
    while result.next()?.is_some() {
        rows += 1;
    }
    Ok(rows)
}

fn database_kib(c: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = c.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = c.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages * page_size / 1024)
}

fn assert_materialized_matches(c: &Connection, mode: Mode) -> rusqlite::Result<()> {
    let actual = match mode {
        Mode::Ivmlite => sorted_rows(c, &format!("SELECT * FROM {IVM_VIEW}"))?,
        Mode::HandwrittenTrigger => sorted_rows(
            c,
            &format!(
                "SELECT flow_type, day_bucket, counterparty_kind, exchange_key, sat, count \
                 FROM {ROLLUP_TABLE}"
            ),
        )?,
        Mode::NoMaintenance | Mode::FullRecompute => return Ok(()),
    };
    assert_eq!(actual, sorted_rows(c, VIEW_SQL)?, "{}", mode.name());
    Ok(())
}

fn run_once(mode: Mode, rows: usize, batch: usize) -> Result<Timing> {
    let c = open_release_extension()?;
    c.execute_batch(FLOW_TABLE_DDL)?;
    seed(&c, rows)?;

    let start = Instant::now();
    match mode {
        Mode::HandwrittenTrigger => install_handwritten_rollup(&c)?,
        Mode::Ivmlite => create_ivm_view(&c)?,
        Mode::NoMaintenance | Mode::FullRecompute => {}
    }
    let bootstrap_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    apply_mixed_batch(&c, rows, batch)?;
    let apply_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    match mode {
        Mode::FullRecompute => {
            black_box(drain(&c, VIEW_SQL)?);
        }
        Mode::Ivmlite => refresh_ivm(&c)?,
        Mode::NoMaintenance | Mode::HandwrittenTrigger => {}
    }
    let maintain_ms = start.elapsed().as_secs_f64() * 1_000.0;

    let start = Instant::now();
    match mode {
        Mode::HandwrittenTrigger => {
            black_box(drain(&c, &format!("SELECT * FROM {ROLLUP_TABLE}"))?);
        }
        Mode::Ivmlite => {
            black_box(drain(&c, &format!("SELECT * FROM {IVM_VIEW}"))?);
        }
        Mode::NoMaintenance | Mode::FullRecompute => {}
    }
    let read_ms = start.elapsed().as_secs_f64() * 1_000.0;

    assert_materialized_matches(&c, mode)?;
    Ok(Timing {
        bootstrap_ms,
        apply_ms,
        maintain_ms,
        read_ms,
        database_kib: database_kib(&c)?,
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

fn arguments() -> Result<(usize, usize, usize)> {
    let mut args = std::env::args().skip(1);
    let rows = args.next().map_or(Ok(250_000), |value| value.parse())?;
    let batch = args.next().map_or(Ok(100), |value| value.parse())?;
    let repeats = args.next().map_or(Ok(5), |value| value.parse())?;
    if args.next().is_some() || rows == 0 || batch == 0 || repeats == 0 || rows < batch {
        return Err(
            "usage: fluxflow_demo [positive rows >= batch] [positive batch] [positive repeats]"
                .into(),
        );
    }
    Ok((rows, batch, repeats))
}

fn main() -> Result<()> {
    let (rows, batch, repeats) = arguments()?;
    eprintln!(
        "FluxFlow-shaped slice: rows={rows}, mixed_batch={batch}, repeats={repeats}; median reported"
    );
    println!(
        "mode,base_rows,batch_size,repeats,bootstrap_ms,apply_ms,maintain_ms,read_ms,\
         apply_plus_maintain_ms,database_kib"
    );

    for mode in Mode::ALL {
        let timings: Vec<_> = (0..repeats)
            .map(|_| run_once(mode, rows, batch))
            .collect::<Result<_>>()?;
        let bootstrap_ms = median(timings.iter().map(|timing| timing.bootstrap_ms).collect());
        let apply_ms = median(timings.iter().map(|timing| timing.apply_ms).collect());
        let maintain_ms = median(timings.iter().map(|timing| timing.maintain_ms).collect());
        let read_ms = median(timings.iter().map(|timing| timing.read_ms).collect());
        let database_kib = median_i64(timings.iter().map(|timing| timing.database_kib).collect());
        println!(
            "{},{rows},{batch},{repeats},{bootstrap_ms:.3},{apply_ms:.3},{maintain_ms:.3},\
             {read_ms:.3},{:.3},{database_kib}",
            mode.name(),
            apply_ms + maintain_ms,
        );
    }
    Ok(())
}
