//! Same-host probe for the executable slices in `tests/real_world_cases.rs`.
//!
//! Usage: `cargo run --release -p ivmlite-test --example real_case_bench -- [rows] [batch] [repeats]`
//! Setup, seeding, extension loading, view creation and correctness checks are
//! outside the timed regions. The full-recompute baseline drains each query but
//! does not persist its result, deliberately favouring the baseline.

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{params, Connection, Statement};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
struct View {
    name: &'static str,
    sql: &'static str,
}

struct Case {
    name: &'static str,
    ddl: &'static str,
    views: &'static [View],
    seed: fn(&Connection, usize) -> rusqlite::Result<()>,
    apply: fn(&Connection, usize, usize) -> rusqlite::Result<f64>,
}

#[derive(Clone, Copy)]
enum Mode {
    NoMaintenance,
    Ivm,
    FullRecompute,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::NoMaintenance, Mode::Ivm, Mode::FullRecompute];

    fn name(self) -> &'static str {
        match self {
            Mode::NoMaintenance => "no_maintenance",
            Mode::Ivm => "ivmlite",
            Mode::FullRecompute => "full_recompute",
        }
    }
}

#[derive(Clone, Copy)]
struct Timing {
    apply_ms: f64,
    maintain_ms: f64,
}

const DATASETTE_VIEWS: &[View] = &[View {
    name: "county_counts",
    sql: "SELECT county, COUNT(*) AS count FROM ny_times_us_counties \
          WHERE state = 'Kentucky' GROUP BY county",
}];

const ORG_ROAM_VIEWS: &[View] = &[View {
    name: "node_tag_counts",
    sql: "SELECT n.id, COUNT(*) AS tag_count FROM nodes AS n \
          JOIN tags AS t ON n.id = t.node_id GROUP BY n.id",
}];

const TAPROOT_VIEWS: &[View] = &[
    View {
        name: "sync_counts",
        sql: "SELECT roots.asset_id, roots.group_key, roots.proof_type, \
              COUNT(*) AS total_asset_syncs FROM universe_events AS u \
              JOIN universe_roots AS roots ON u.universe_root_id = roots.id \
              WHERE u.event_type = 'SYNC' \
              GROUP BY roots.asset_id, roots.group_key, roots.proof_type",
    },
    View {
        name: "proof_counts",
        sql: "SELECT roots.asset_id, roots.group_key, roots.proof_type, \
              COUNT(*) AS total_asset_proofs FROM universe_events AS u \
              JOIN universe_roots AS roots ON u.universe_root_id = roots.id \
              WHERE u.event_type = 'NEW_PROOF' \
              GROUP BY roots.asset_id, roots.group_key, roots.proof_type",
    },
];

const CASES: &[Case] = &[
    Case {
        name: "datasette_county_facet",
        ddl: "CREATE TABLE ny_times_us_counties(
                  id INTEGER PRIMARY KEY,
                  date TEXT NOT NULL,
                  county TEXT NOT NULL,
                  state TEXT NOT NULL,
                  fips TEXT NOT NULL,
                  cases INTEGER NOT NULL,
                  deaths INTEGER NOT NULL
              ) STRICT",
        views: DATASETTE_VIEWS,
        seed: seed_datasette,
        apply: apply_datasette,
    },
    Case {
        name: "org_roam_tag_counts",
        ddl: "CREATE TABLE nodes(id TEXT PRIMARY KEY, file TEXT NOT NULL) STRICT;
              CREATE TABLE tags(node_id TEXT NOT NULL, tag TEXT NOT NULL) STRICT",
        views: ORG_ROAM_VIEWS,
        seed: seed_org_roam,
        apply: apply_org_roam,
    },
    Case {
        name: "taproot_event_counts",
        ddl: "CREATE TABLE universe_roots(
                  id INTEGER PRIMARY KEY,
                  asset_id TEXT NOT NULL,
                  group_key TEXT NOT NULL,
                  proof_type TEXT NOT NULL
              ) STRICT;
              CREATE TABLE universe_events(
                  id INTEGER PRIMARY KEY,
                  event_type TEXT NOT NULL,
                  universe_root_id INTEGER NOT NULL
              ) STRICT",
        views: TAPROOT_VIEWS,
        seed: seed_taproot,
        apply: apply_taproot,
    },
];

fn seed_datasette(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut insert =
            tx.prepare("INSERT INTO ny_times_us_counties VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for id in 0..rows {
            let state = if id.is_multiple_of(8) {
                "Kentucky"
            } else {
                "Ohio"
            };
            insert.execute(params![
                id as i64,
                format!("2021-11-{:02}", id % 28 + 1),
                format!("county-{:03}", id % 120),
                state,
                format!("{:05}", id % 100_000),
                id as i64 * 3,
                id as i64 / 100
            ])?;
        }
    }
    tx.commit()
}

fn seed_org_roam(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let nodes = (rows / 5).max(1);
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO nodes VALUES (?1, ?2)")?;
        for id in 0..nodes {
            insert.execute(params![format!("node-{id}"), format!("note-{id}.org")])?;
        }
    }
    {
        let mut insert = tx.prepare("INSERT INTO tags VALUES (?1, ?2)")?;
        for id in 0..rows {
            insert.execute(params![
                format!("node-{}", id % nodes),
                format!("tag-{}", id % 40)
            ])?;
        }
    }
    tx.commit()
}

fn seed_taproot(c: &Connection, rows: usize) -> rusqlite::Result<()> {
    let roots = (rows / 20).max(1);
    let tx = c.unchecked_transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO universe_roots VALUES (?1, ?2, ?3, ?4)")?;
        for id in 0..roots {
            insert.execute(params![
                id as i64,
                format!("asset-{id}"),
                format!("group-{}", id % 100),
                "issuance"
            ])?;
        }
    }
    {
        let mut insert = tx.prepare("INSERT INTO universe_events VALUES (?1, ?2, ?3)")?;
        for id in 0..rows {
            insert.execute(params![
                id as i64,
                if id.is_multiple_of(2) {
                    "SYNC"
                } else {
                    "NEW_PROOF"
                },
                (id % roots) as i64
            ])?;
        }
    }
    tx.commit()
}

fn timed_inserts<F>(c: &Connection, sql: &str, batch: usize, mut bind: F) -> rusqlite::Result<f64>
where
    F: FnMut(&mut Statement<'_>, usize) -> rusqlite::Result<()>,
{
    let mut insert = c.prepare(sql)?;
    let start = Instant::now();
    c.execute_batch("BEGIN IMMEDIATE")?;
    for offset in 0..batch {
        bind(&mut insert, offset)?;
    }
    c.execute_batch("COMMIT")?;
    Ok(start.elapsed().as_secs_f64() * 1_000.0)
}

fn apply_datasette(c: &Connection, rows: usize, batch: usize) -> rusqlite::Result<f64> {
    timed_inserts(
        c,
        "INSERT INTO ny_times_us_counties VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        batch,
        |insert, offset| {
            let id = rows + offset;
            insert.execute(params![
                id as i64,
                "2021-12-01",
                format!("county-{:03}", id % 120),
                if id.is_multiple_of(8) {
                    "Kentucky"
                } else {
                    "Ohio"
                },
                format!("{:05}", id % 100_000),
                id as i64 * 3,
                id as i64 / 100
            ])?;
            Ok(())
        },
    )
}

fn apply_org_roam(c: &Connection, rows: usize, batch: usize) -> rusqlite::Result<f64> {
    let nodes = (rows / 5).max(1);
    timed_inserts(
        c,
        "INSERT INTO tags VALUES (?1, ?2)",
        batch,
        |insert, offset| {
            let id = rows + offset;
            insert.execute(params![
                format!("node-{}", id % nodes),
                format!("tag-{}", id % 40)
            ])?;
            Ok(())
        },
    )
}

fn apply_taproot(c: &Connection, rows: usize, batch: usize) -> rusqlite::Result<f64> {
    let roots = (rows / 20).max(1);
    timed_inserts(
        c,
        "INSERT INTO universe_events VALUES (?1, ?2, ?3)",
        batch,
        |insert, offset| {
            let id = rows + offset;
            insert.execute(params![
                id as i64,
                if id.is_multiple_of(2) {
                    "SYNC"
                } else {
                    "NEW_PROOF"
                },
                (id % roots) as i64
            ])?;
            Ok(())
        },
    )
}

fn create_view(c: &Connection, view: View) -> rusqlite::Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {} USING ivm('{}')",
        view.name,
        view.sql.replace('\'', "''")
    ))
}

fn open_benchmark_connection() -> Result<Connection> {
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ivmlite-sqlite/target/release")
        .join(if cfg!(target_os = "macos") {
            "libivmlite_sqlite.dylib"
        } else {
            "libivmlite_sqlite.so"
        });
    if !library.is_file() {
        return Err(format!(
            "the release extension is missing at {}; run \
             `cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml`",
            library.display()
        )
        .into());
    }
    let c = Connection::open_in_memory()?;
    // SAFETY: this loads this repository's release-built extension and names
    // its explicit entry point. Loading is disabled again immediately.
    unsafe {
        c.load_extension_enable()?;
        c.load_extension(&library, Some("sqlite3_ivmlite_init"))?;
        c.load_extension_disable()?;
    }
    Ok(c)
}

fn refresh(c: &Connection, name: &str) -> rusqlite::Result<()> {
    c.execute_batch(&format!("INSERT INTO {name}({name}) VALUES ('refresh')"))
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

fn check(c: &Connection, views: &[View]) -> rusqlite::Result<()> {
    for view in views {
        assert_eq!(
            sorted_rows(c, &format!("SELECT * FROM {}", view.name))?,
            sorted_rows(c, view.sql)?,
            "{} diverged from SQLite recomputation",
            view.name
        );
    }
    Ok(())
}

fn run_once(case: &Case, mode: Mode, rows: usize, batch: usize) -> Result<Timing> {
    let c = open_benchmark_connection()?;
    c.execute_batch(case.ddl)?;
    (case.seed)(&c, rows)?;
    if matches!(mode, Mode::Ivm) {
        for &view in case.views {
            create_view(&c, view)?;
        }
    }

    let mut recompute = if matches!(mode, Mode::FullRecompute) {
        Some(
            case.views
                .iter()
                .map(|view| c.prepare(view.sql))
                .collect::<rusqlite::Result<Vec<_>>>()?,
        )
    } else {
        None
    };
    let apply_ms = (case.apply)(&c, rows, batch)?;
    let start = Instant::now();
    match mode {
        Mode::NoMaintenance => {}
        Mode::Ivm => {
            for view in case.views {
                refresh(&c, view.name)?;
            }
        }
        Mode::FullRecompute => {
            for statement in recompute.as_mut().expect("statements were prepared") {
                let mut result = statement.query([])?;
                let mut count = 0usize;
                while result.next()?.is_some() {
                    count += 1;
                }
                black_box(count);
            }
        }
    }
    let maintain_ms = start.elapsed().as_secs_f64() * 1_000.0;
    if matches!(mode, Mode::Ivm) {
        check(&c, case.views)?;
    }
    Ok(Timing {
        apply_ms,
        maintain_ms,
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn arguments() -> Result<(usize, usize, usize)> {
    let mut args = std::env::args().skip(1);
    let rows = args.next().map_or(Ok(25_000), |s| s.parse())?;
    let batch = args.next().map_or(Ok(1_000), |s| s.parse())?;
    let repeats = args.next().map_or(Ok(5), |s| s.parse())?;
    if args.next().is_some() || rows == 0 || batch == 0 || repeats == 0 {
        return Err(
            "usage: real_case_bench [positive rows] [positive batch] [positive repeats]".into(),
        );
    }
    Ok((rows, batch, repeats))
}

fn main() -> Result<()> {
    let (rows, batch, repeats) = arguments()?;
    eprintln!(
        "Running source-backed slices: rows={rows}, batch={batch}, repeats={repeats}; median reported"
    );
    println!("case,mode,views,base_rows,batch_size,repeats,apply_ms,maintain_ms,total_ms");
    for case in CASES {
        for mode in Mode::ALL {
            let mut timings = Vec::with_capacity(repeats);
            for _ in 0..repeats {
                timings.push(run_once(case, mode, rows, batch)?);
            }
            let apply_ms = median(timings.iter().map(|t| t.apply_ms).collect());
            let maintain_ms = median(timings.iter().map(|t| t.maintain_ms).collect());
            println!(
                "{},{},{},{},{},{},{:.3},{:.3},{:.3}",
                case.name,
                mode.name(),
                case.views.len(),
                rows,
                batch,
                repeats,
                apply_ms,
                maintain_ms,
                apply_ms + maintain_ms
            );
        }
    }
    Ok(())
}
