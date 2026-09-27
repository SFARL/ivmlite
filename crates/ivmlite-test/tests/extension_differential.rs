//! M1b Phase 3a spec §6, scenarios 1 and 2: the differential harness against
//! the real loaded extension, with and without reopening the database before
//! every refresh.
//!
//! Strides are set from measured cost (debug build, 2026-09-26): about 9 ms a
//! single-table case and 14 ms a join case in memory, about 60 ms a case when
//! reopening a database file before every refresh. Every join query was run
//! in full once while the plan was prototyped (1800 cases, all green).

use ivmlite_test::{
    enumerate, enumerate_join, gen_case_with_query, gen_database,
    gen_database_with_swapped_right_table, run, seed_range, Batching, Database, Domain,
    SqliteExtensionEngine, ViewQuery,
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

#[test]
fn the_extension_is_green_across_the_single_table_space() {
    let db = gen_database(2);
    sweep(
        &db,
        &enumerate(&db.tables()[0]),
        1,
        SqliteExtensionEngine::new,
    );
}

#[test]
fn the_extension_is_green_across_the_join_space() {
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 5, SqliteExtensionEngine::new);
    }
}

/// Every refresh starts from a freshly opened database file: what the view
/// knows it knows from its shadow tables alone.
#[test]
fn the_extension_resumes_from_persisted_state_on_every_refresh() {
    let db = gen_database(2);
    sweep(
        &db,
        &enumerate(&db.tables()[0]),
        3,
        SqliteExtensionEngine::reopening,
    );
    for db in [gen_database(2), gen_database_with_swapped_right_table()] {
        let joins = enumerate_join(&db.tables()[0], &db.tables()[1]);
        sweep(&db, &joins, 25, SqliteExtensionEngine::reopening);
    }
}
