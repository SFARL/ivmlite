//! Executable slices of the source-backed cases under `docs/cases`.
//!
//! The tests deliberately distinguish exact source SQL from an adapted slice.
//! An adapted slice is allowed only when its semantic differences are stated in
//! `docs/cases/evaluation/README.md`; it must still run through the loaded
//! SQLite extension and agree with SQLite recomputing that adapted SQL.

mod common;
use common::*;

use ivmlite_test::demand_cases::{DemandCase, KENER, NOOP, ZCASH};
use ivmlite_test::fluxflow::{
    apply_mixed_batch as apply_fluxflow_batch, seed as seed_fluxflow, FLOW_TABLE_DDL,
    VIEW_SQL as FLUXFLOW_VIEW,
};
use ivmlite_test::open_with_extension;

const DATASETTE_FACET: &str = "SELECT county, COUNT(*) AS count \
    FROM ny_times_us_counties WHERE state = 'Kentucky' GROUP BY county";

const ORG_ROAM_TAG_COUNTS: &str = "SELECT n.id, COUNT(*) AS tag_count \
    FROM nodes AS n JOIN tags AS t ON n.id = t.node_id GROUP BY n.id";

const TAPROOT_SYNCS: &str = "SELECT roots.asset_id, roots.group_key, roots.proof_type, \
    COUNT(*) AS total_asset_syncs \
    FROM universe_events AS u JOIN universe_roots AS roots \
    ON u.universe_root_id = roots.id WHERE u.event_type = 'SYNC' \
    GROUP BY roots.asset_id, roots.group_key, roots.proof_type";

const TAPROOT_PROOFS: &str = "SELECT roots.asset_id, roots.group_key, roots.proof_type, \
    COUNT(*) AS total_asset_proofs \
    FROM universe_events AS u JOIN universe_roots AS roots \
    ON u.universe_root_id = roots.id WHERE u.event_type = 'NEW_PROOF' \
    GROUP BY roots.asset_id, roots.group_key, roots.proof_type";

fn setup_datasette(c: &rusqlite::Connection) {
    c.execute_batch(
        "CREATE TABLE ny_times_us_counties(
             id INTEGER PRIMARY KEY,
             date TEXT NOT NULL,
             county TEXT NOT NULL,
             state TEXT NOT NULL,
             fips TEXT NOT NULL,
             cases INTEGER NOT NULL,
             deaths INTEGER NOT NULL
         ) STRICT;
         INSERT INTO ny_times_us_counties VALUES
             (1, '2021-11-15', 'Jefferson', 'Kentucky', '21111', 100, 2),
             (2, '2021-11-15', 'Fayette',   'Kentucky', '21067',  80, 1),
             (3, '2021-11-16', 'Jefferson', 'Kentucky', '21111', 105, 2),
             (4, '2021-11-16', 'Hamilton',  'Ohio',     '39061', 200, 3);",
    )
    .unwrap();
}

fn setup_org_roam(c: &rusqlite::Connection) {
    c.execute_batch(
        "CREATE TABLE nodes(id TEXT PRIMARY KEY, file TEXT NOT NULL) STRICT;
         CREATE TABLE tags(node_id TEXT NOT NULL, tag TEXT NOT NULL) STRICT;
         INSERT INTO nodes VALUES ('n1', 'one.org'), ('n2', 'two.org'), ('n3', 'three.org');
         INSERT INTO tags VALUES ('n1', 'rust'), ('n1', 'sqlite'), ('n2', 'rust');",
    )
    .unwrap();
}

fn setup_taproot(c: &rusqlite::Connection) {
    c.execute_batch(
        "CREATE TABLE universe_roots(
             id INTEGER PRIMARY KEY,
             asset_id TEXT NOT NULL,
             group_key TEXT NOT NULL,
             proof_type TEXT NOT NULL
         ) STRICT;
         CREATE TABLE universe_events(
             id INTEGER PRIMARY KEY,
             event_type TEXT NOT NULL,
             universe_root_id INTEGER NOT NULL
         ) STRICT;
         INSERT INTO universe_roots VALUES
             (1, 'asset-a', 'group-a', 'issuance'),
             (2, 'asset-b', 'group-b', 'issuance');
         INSERT INTO universe_events VALUES
             (1, 'SYNC', 1), (2, 'NEW_PROOF', 1), (3, 'SYNC', 2);",
    )
    .unwrap();
}

#[test]
fn datasette_fixed_filter_facet_is_maintained_after_insert_update_and_delete() {
    let c = open_with_extension(None).unwrap();
    setup_datasette(&c);
    create(&c, "county_counts", DATASETTE_FACET).unwrap();
    assert_matches_oracle(&c, "county_counts", DATASETTE_FACET);

    c.execute_batch(
        "INSERT INTO ny_times_us_counties VALUES
             (5, '2021-11-17', 'Fayette', 'Kentucky', '21067', 90, 1);
         UPDATE ny_times_us_counties SET state = 'Kentucky' WHERE id = 4;
         UPDATE ny_times_us_counties SET county = 'Fayette' WHERE id = 1;
         DELETE FROM ny_times_us_counties WHERE id = 3;",
    )
    .unwrap();
    refresh(&c, "county_counts").unwrap();
    assert_matches_oracle(&c, "county_counts", DATASETTE_FACET);
}

#[test]
fn org_roam_two_table_tag_count_slice_is_maintained_on_both_sides() {
    let c = open_with_extension(None).unwrap();
    setup_org_roam(&c);
    create(&c, "node_tag_counts", ORG_ROAM_TAG_COUNTS).unwrap();
    assert_matches_oracle(&c, "node_tag_counts", ORG_ROAM_TAG_COUNTS);

    c.execute_batch(
        "INSERT INTO tags VALUES ('n2', 'emacs'), ('n3', 'notes');
         UPDATE tags SET node_id = 'n3' WHERE node_id = 'n1' AND tag = 'sqlite';
         DELETE FROM tags WHERE node_id = 'n1' AND tag = 'rust';
         UPDATE nodes SET id = 'n4' WHERE id = 'n2';",
    )
    .unwrap();
    refresh(&c, "node_tag_counts").unwrap();
    assert_matches_oracle(&c, "node_tag_counts", ORG_ROAM_TAG_COUNTS);
}

#[test]
fn taproot_split_event_counters_are_maintained_on_events_and_roots() {
    let c = open_with_extension(None).unwrap();
    setup_taproot(&c);
    create(&c, "sync_counts", TAPROOT_SYNCS).unwrap();
    create(&c, "proof_counts", TAPROOT_PROOFS).unwrap();
    assert_matches_oracle(&c, "sync_counts", TAPROOT_SYNCS);
    assert_matches_oracle(&c, "proof_counts", TAPROOT_PROOFS);

    c.execute_batch(
        "INSERT INTO universe_events VALUES (4, 'SYNC', 1), (5, 'NEW_PROOF', 2);
         UPDATE universe_events SET event_type = 'NEW_PROOF' WHERE id = 3;
         DELETE FROM universe_events WHERE id = 2;
         UPDATE universe_roots SET group_key = 'group-c' WHERE id = 1;",
    )
    .unwrap();
    refresh(&c, "sync_counts").unwrap();
    refresh(&c, "proof_counts").unwrap();
    assert_matches_oracle(&c, "sync_counts", TAPROOT_SYNCS);
    assert_matches_oracle(&c, "proof_counts", TAPROOT_PROOFS);
}

#[test]
fn fluxflow_grouped_rollup_tracks_insert_update_and_reorg_delete() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(FLOW_TABLE_DDL).unwrap();
    seed_fluxflow(&c, 200).unwrap();
    create(&c, "fluxflow_stats", FLUXFLOW_VIEW).unwrap();
    assert_matches_oracle(&c, "fluxflow_stats", FLUXFLOW_VIEW);

    apply_fluxflow_batch(&c, 200, 50).unwrap();
    assert_ne!(
        rows(&c, "SELECT * FROM fluxflow_stats"),
        rows(&c, FLUXFLOW_VIEW),
        "captured changes must wait for explicit refresh"
    );
    refresh(&c, "fluxflow_stats").unwrap();
    assert_matches_oracle(&c, "fluxflow_stats", FLUXFLOW_VIEW);

    let after_first_refresh = rows(&c, "SELECT * FROM fluxflow_stats");
    refresh(&c, "fluxflow_stats").unwrap();
    assert_eq!(
        rows(&c, "SELECT * FROM fluxflow_stats"),
        after_first_refresh,
        "a second refresh with no new writes must be a no-op"
    );
}

fn assert_demand_case_tracks_mixed_changes(case: &DemandCase) {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(case.ddl).unwrap();
    case.seed(&c, 600).unwrap();
    create(&c, case.view_name, case.view_sql).unwrap();
    assert_matches_oracle(&c, case.view_name, case.view_sql);

    case.apply_mixed_batch(&c, 600, 40).unwrap();
    assert_ne!(
        rows(&c, &format!("SELECT * FROM {}", case.view_name)),
        rows(&c, case.view_sql),
        "captured changes must wait for explicit refresh"
    );
    refresh(&c, case.view_name).unwrap();
    assert_matches_oracle(&c, case.view_name, case.view_sql);

    let after_first_refresh = rows(&c, &format!("SELECT * FROM {}", case.view_name));
    refresh(&c, case.view_name).unwrap();
    assert_eq!(
        rows(&c, &format!("SELECT * FROM {}", case.view_name)),
        after_first_refresh,
        "a second refresh with no new writes must be a no-op"
    );
}

#[test]
fn noop_day_counts_track_append_backfill_delete_and_correction() {
    assert_demand_case_tracks_mixed_changes(&NOOP);
}

#[test]
fn zcash_balances_track_receive_spend_rewind_and_correction() {
    assert_demand_case_tracks_mixed_changes(&ZCASH);
}

#[test]
fn kener_rollup_tracks_insert_status_rewrite_latency_update_and_delete() {
    assert_demand_case_tracks_mixed_changes(&KENER);
}

#[test]
fn exact_datasette_compound_query_reports_the_current_boundary() {
    let c = open_with_extension(None).unwrap();
    setup_datasette(&c);
    let source = include_str!("../../../docs/cases/datasette/extracted/kentucky-combined.sql");
    let err = create(&c, "datasette_original", source).expect_err("WITH is outside v0");
    assert!(err.to_string().contains("WITH"), "{err}");
}

#[test]
fn exact_taproot_view_reports_the_current_boundary() {
    let c = open_with_extension(None).unwrap();
    setup_taproot(&c);
    let migration =
        include_str!("../../../docs/cases/taproot-assets/upstream/000010_universe_stats.up.sql");
    let source = migration
        .split_once("CREATE VIEW universe_stats AS")
        .expect("the pinned migration still defines universe_stats")
        .1
        .trim()
        .trim_end_matches(';');
    let err = create(&c, "taproot_original", source).expect_err("COUNT(CASE ...) is outside v0");
    assert!(
        err.to_string().contains("COUNT") || err.to_string().contains("CASE"),
        "{err}"
    );
}

#[test]
fn source_shaped_org_roam_query_reports_the_current_boundary() {
    let c = open_with_extension(None).unwrap();
    setup_org_roam(&c);
    c.execute_batch(
        "CREATE TABLE aliases(node_id TEXT NOT NULL, alias TEXT NOT NULL) STRICT;
         CREATE TABLE refs(node_id TEXT NOT NULL, ref TEXT NOT NULL) STRICT;",
    )
    .unwrap();
    let source_shape = "SELECT nodes.id, group_concat(tags.tag), \
        group_concat(aliases.alias), group_concat(refs.ref) FROM nodes \
        LEFT JOIN tags ON tags.node_id = nodes.id \
        LEFT JOIN aliases ON aliases.node_id = nodes.id \
        LEFT JOIN refs ON refs.node_id = nodes.id GROUP BY nodes.id";
    let err = create(&c, "org_roam_original_shape", source_shape)
        .expect_err("outer and multi-table joins are outside v0");
    assert!(
        err.to_string().contains("outer") || err.to_string().contains("more than two"),
        "{err}"
    );
}
