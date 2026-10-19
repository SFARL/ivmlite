//! M1b Phase 3a spec §6, scenarios 1 and 2: the differential harness against
//! the real loaded extension, with and without reopening the database before
//! every refresh — and, Phase 3b, with sibling views sharing the capture.
//!
//! Every sweep runs over the databases of every M2c type (M2c spec §6):
//! single-table sweeps over `gen_database(2)` and
//! `gen_real_numeric_database()`, join sweeps over those two and
//! `gen_database_with_swapped_right_table()`. The spaces are 510 single-table
//! and 1,197 join queries per database, so each sweep runs every `stride`-th
//! query. Cases per sweep (single over both databases + join over all three):
//! - in memory: stride 4 and 7, 2 × 128 + 3 × 171 = 769 cases;
//! - reopening before every refresh: stride 12 and 35, 2 × 43 + 3 × 35 = 191;
//! - with siblings: stride 8 and 15, 2 × 64 + 3 × 80 = 368;
//! - reopening with siblings: stride 24 and 70 (twice the reopening strides,
//!   as before M2c), 2 × 22 + 3 × 18 = 98.
//!
//! Before M2c (153 single-table and 900 join queries per database, over two
//! join databases) the
//! strides were 1/5, 3/25, 2/10 and 6/50, set from measured cost (debug
//! build, 2026-09-26): about 9 ms a single-table case and 14 ms a join case
//! in memory, about 60 ms a case when reopening a database file before every
//! refresh.

use ivmlite_test::{
    enumerate, enumerate_join, gen_case_with_query, gen_database,
    gen_database_with_swapped_right_table, gen_real_numeric_database, run, seed_range, Batching,
    Database, Domain, SqliteExtensionEngine, ViewQuery,
};

/// Run every `stride`-th query of `queries`, one case each, seeded by its
/// index. Under `IVMLITE_SEED=<n>` only query `n` runs, as in the harness's
/// other sweeps.
fn sweep(db: &Database, queries: &[ViewQuery], stride: usize, make: fn() -> SqliteExtensionEngine) {
    let domain = Domain::default();
    let selected: Vec<u64> = if std::env::var("IVMLITE_SEED").is_ok() {
        seed_range()
    } else {
        (0..queries.len() as u64).step_by(stride).collect()
    };
    assert!(!selected.is_empty());
    for seed in selected {
        let query = queries[seed as usize % queries.len()].clone();
        let case = gen_case_with_query(seed, db, &domain, query, 20, 60, Batching::Chunks(4));
        let mut engine = make();
        if let Err(f) = run(&mut engine, &case) {
            panic!("the SQLite extension disagrees with the oracle on query {seed}: {f}");
        }
    }
}

/// The single-table sweeps' databases: INTEGER, TEXT and no type; REAL,
/// NUMERIC and TEXT.
fn single_table_databases() -> [Database; 2] {
    [gen_database(2), gen_real_numeric_database()]
}

/// The join sweeps' databases: their same-affinity key pairs cover TEXT,
/// INTEGER, no type, REAL and NUMERIC keys, at equal and at different
/// positions.
fn join_databases() -> [Database; 3] {
    [
        gen_database(2),
        gen_database_with_swapped_right_table(),
        gen_real_numeric_database(),
    ]
}

/// The single-table sweep, then the join sweep, with `make`'s engine.
fn sweep_both(single_stride: usize, join_stride: usize, make: fn() -> SqliteExtensionEngine) {
    for db in single_table_databases() {
        sweep(&db, &enumerate(&db.tables()[0]), single_stride, make);
    }
    for db in join_databases() {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, join_stride, make);
    }
}

#[test]
fn the_extension_is_green_across_the_single_table_space() {
    for db in single_table_databases() {
        sweep(
            &db,
            &enumerate(&db.tables()[0]),
            4,
            SqliteExtensionEngine::new,
        );
    }
}

#[test]
fn the_extension_is_green_across_the_join_space() {
    for db in join_databases() {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 7, SqliteExtensionEngine::new);
    }
}

/// Every refresh starts from a freshly opened database file: what the view
/// knows it knows from its shadow tables alone.
#[test]
fn the_extension_resumes_from_persisted_state_on_every_refresh() {
    sweep_both(12, 35, SqliteExtensionEngine::reopening);
}

/// Phase 3b spec §8 scenario 1: sibling views share each base table's
/// capture, lag behind the view under test, and one is dropped mid-case.
#[test]
fn sibling_views_share_capture_across_the_single_table_space() {
    for db in single_table_databases() {
        sweep(
            &db,
            &enumerate(&db.tables()[0]),
            8,
            SqliteExtensionEngine::with_siblings,
        );
    }
}

#[test]
fn sibling_views_share_capture_across_the_join_space() {
    for db in join_databases() {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 15, SqliteExtensionEngine::with_siblings);
    }
}

#[test]
fn sibling_views_resume_from_persisted_state() {
    sweep_both(24, 70, SqliteExtensionEngine::reopening_with_siblings);
}
