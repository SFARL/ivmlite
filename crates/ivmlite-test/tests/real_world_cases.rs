//! Executable slices of the source-backed cases under `docs/cases`.
//!
//! The tests deliberately distinguish exact source SQL from an adapted slice.
//! An adapted slice is allowed only when its semantic differences are stated in
//! `docs/cases/evaluation/README.md`; it must still run through the loaded
//! SQLite extension and agree with SQLite recomputing that adapted SQL.

mod common;
use common::*;

use ivmlite_test::demand_cases::{
    DemandCase, CPA_HOURLY_CORE, DARK_XRAY_LIFETIME, DARK_XRAY_PERIOD, JOT_SESSION_COUNTS, KENER,
    NICTI, NOOP, OPENWHISPR, PHOENIX, POCKETBASE, POSTIO, ZCASH, ZENOD,
};
use ivmlite_test::demo_bench::{measured_rounds, Workload, ROUNDS};
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
    seed_fluxflow(&c, 600).unwrap();
    create(&c, "fluxflow_stats", FLUXFLOW_VIEW).unwrap();
    assert_matches_oracle(&c, "fluxflow_stats", FLUXFLOW_VIEW);

    apply_fluxflow_batch(&c, 600, 50, 0).unwrap();
    assert_ne!(
        rows(&c, "SELECT * FROM fluxflow_stats"),
        rows(&c, FLUXFLOW_VIEW),
        "captured changes must wait for explicit refresh"
    );
    refresh(&c, "fluxflow_stats").unwrap();
    assert_matches_oracle(&c, "fluxflow_stats", FLUXFLOW_VIEW);

    // The benchmark's measured rounds follow the warm-up round.
    for round in measured_rounds() {
        apply_fluxflow_batch(&c, 600, 50, round).unwrap();
        refresh(&c, "fluxflow_stats").unwrap();
        assert_matches_oracle(&c, "fluxflow_stats", FLUXFLOW_VIEW);
    }

    let after_last_refresh = rows(&c, "SELECT * FROM fluxflow_stats");
    refresh(&c, "fluxflow_stats").unwrap();
    assert_eq!(
        rows(&c, "SELECT * FROM fluxflow_stats"),
        after_last_refresh,
        "a second refresh with no new writes must be a no-op"
    );
}

/// Runs every round the benchmark applies of `case`'s mixed batch through
/// the extension, checking the view against SQLite after each refresh, and
/// returns the result groups (the leading `key_columns` of each row) that
/// existed after bootstrap but no longer exist at the end.
fn assert_demand_case_tracks_mixed_changes(
    case: &DemandCase,
    key_columns: usize,
) -> Vec<Vec<rusqlite::types::Value>> {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(case.ddl).unwrap();
    case.seed(&c, 1_200).unwrap();
    create(&c, case.view_name, case.view_sql).unwrap();
    assert_matches_oracle(&c, case.view_name, case.view_sql);
    let view = format!("SELECT * FROM {}", case.view_name);
    let keys = |rows: Vec<Vec<rusqlite::types::Value>>| -> Vec<_> {
        rows.into_iter()
            .map(|row| row[..key_columns].to_vec())
            .collect()
    };
    let initial_keys = keys(rows(&c, &view));
    let hand = case.handwritten().map(|baseline| {
        let h = rusqlite::Connection::open_in_memory().unwrap();
        h.execute_batch(case.ddl).unwrap();
        case.seed(&h, 1_200).unwrap();
        (baseline.install)(&h).unwrap();
        assert_eq!(rows(&h, baseline.read_sql), rows(&c, &view));
        (h, baseline)
    });

    for round in 0..ROUNDS {
        case.apply_mixed_batch(&c, 1_200, 40, round).unwrap();
        assert_ne!(
            rows(&c, &view),
            rows(&c, case.view_sql),
            "captured changes must wait for explicit refresh"
        );
        refresh(&c, case.view_name).unwrap();
        assert_matches_oracle(&c, case.view_name, case.view_sql);
        if let Some((h, baseline)) = &hand {
            case.apply_mixed_batch(h, 1_200, 40, round).unwrap();
            assert_eq!(
                rows(h, baseline.read_sql),
                rows(&c, &view),
                "{} handwritten round {round}",
                case.name
            );
        }
    }

    let after_last_refresh = rows(&c, &view);
    refresh(&c, case.view_name).unwrap();
    assert_eq!(
        rows(&c, &view),
        after_last_refresh,
        "a second refresh with no new writes must be a no-op"
    );

    let final_keys = keys(after_last_refresh);
    initial_keys
        .into_iter()
        .filter(|key| !final_keys.contains(key))
        .collect()
}

#[test]
fn noop_day_counts_track_append_backfill_delete_and_correction() {
    assert_demand_case_tracks_mixed_changes(&NOOP, 2);
}

#[test]
fn zcash_balances_track_receive_spend_rewind_and_correction() {
    assert_demand_case_tracks_mixed_changes(&ZCASH, 1);
}

#[test]
fn kener_rollup_tracks_insert_status_rewrite_latency_update_and_delete() {
    let vanished = assert_demand_case_tracks_mixed_changes(&KENER, 3);
    // The status rewrites move every row out of the first quarter-hour
    // group, so the view must retract that group to nothing, not to zero.
    assert!(
        !vanished.is_empty(),
        "some Kener group must disappear entirely"
    );
}

#[test]
fn zenod_totals_track_usage_corrections_and_retractions() {
    assert_demand_case_tracks_mixed_changes(&ZENOD, 2);
}

#[test]
fn nicti_facets_track_asset_and_keyword_mutations() {
    assert_demand_case_tracks_mixed_changes(&NICTI, 3);
}

#[test]
fn postio_footprint_tracks_soft_delete_restore_move_and_resize() {
    assert_demand_case_tracks_mixed_changes(&POSTIO, 1);
}

#[test]
fn phoenix_tokens_track_span_and_trace_changes() {
    assert_demand_case_tracks_mixed_changes(&PHOENIX, 2);
}

#[test]
fn pocketbase_counts_retract_the_last_book_of_an_author() {
    assert!(!assert_demand_case_tracks_mixed_changes(&POCKETBASE, 1).is_empty());
}

#[test]
fn openwhispr_speaker_rollup_tracks_segment_and_note_changes() {
    assert_demand_case_tracks_mixed_changes(&OPENWHISPR, 1);
}

#[test]
fn dark_xray_period_and_lifetime_totals_track_immutable_arrivals() {
    for (case, keys) in [(&DARK_XRAY_PERIOD, 2), (&DARK_XRAY_LIFETIME, 1)] {
        assert_demand_case_tracks_mixed_changes(case, keys);
    }
}

#[test]
fn dark_xray_views_match_the_original_byte_expression() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(DARK_XRAY_PERIOD.ddl).unwrap();
    DARK_XRAY_PERIOD.seed(&c, 1_200).unwrap();
    for case in [&DARK_XRAY_PERIOD, &DARK_XRAY_LIFETIME] {
        create(&c, case.view_name, case.view_sql).unwrap();
    }

    for round in 0..=ROUNDS {
        if round > 0 {
            DARK_XRAY_PERIOD
                .apply_mixed_batch(&c, 1_200, 40, round - 1)
                .unwrap();
            refresh(&c, DARK_XRAY_PERIOD.view_name).unwrap();
            refresh(&c, DARK_XRAY_LIFETIME.view_name).unwrap();
        }
        assert_eq!(
            rows(&c, "SELECT * FROM dark_xray_period_totals"),
            rows(
                &c,
                "SELECT owner, period, SUM(up_bytes + down_bytes)
                      FROM traffic_ledger GROUP BY owner, period"
            ),
            "period round {round}"
        );
        assert_eq!(
            rows(&c, "SELECT * FROM dark_xray_lifetime_totals"),
            rows(
                &c,
                "SELECT owner, SUM(up_bytes + down_bytes)
                      FROM traffic_ledger GROUP BY owner"
            ),
            "lifetime round {round}"
        );
    }
}

#[test]
fn cpa_hourly_core_tracks_arrivals_and_source_conditional_counts() {
    const SOURCE_CORE: &str = "SELECT hour_bucket, model, COUNT(*),
        SUM(CASE WHEN failed=0 THEN 1 ELSE 0 END),
        SUM(CASE WHEN failed=1 THEN 1 ELSE 0 END),
        SUM(input_tokens), SUM(output_tokens), SUM(total_tokens)
        FROM usage_events GROUP BY hour_bucket, model";
    assert_demand_case_tracks_mixed_changes(&CPA_HOURLY_CORE, 2);

    let c = open_with_extension(None).unwrap();
    c.execute_batch(CPA_HOURLY_CORE.ddl).unwrap();
    CPA_HOURLY_CORE.seed(&c, 1_200).unwrap();
    create(&c, CPA_HOURLY_CORE.view_name, CPA_HOURLY_CORE.view_sql).unwrap();
    for round in 0..=ROUNDS {
        if round > 0 {
            CPA_HOURLY_CORE
                .apply_mixed_batch(&c, 1_200, 40, round - 1)
                .unwrap();
            refresh(&c, CPA_HOURLY_CORE.view_name).unwrap();
        }
        assert_eq!(
            rows(&c, "SELECT * FROM cpa_hourly_core_totals"),
            rows(&c, SOURCE_CORE),
            "conditional counts at round {round}"
        );
    }
}

#[test]
fn jot_session_counts_track_mixed_changes_and_failed_replay() {
    assert_demand_case_tracks_mixed_changes(&JOT_SESSION_COUNTS, 1);

    let c = open_with_extension(None).unwrap();
    c.execute_batch(JOT_SESSION_COUNTS.ddl).unwrap();
    JOT_SESSION_COUNTS.seed(&c, 1_200).unwrap();
    create(
        &c,
        JOT_SESSION_COUNTS.view_name,
        JOT_SESSION_COUNTS.view_sql,
    )
    .unwrap();
    let initial = rows(&c, "SELECT * FROM jot_ambient_counts");

    c.execute_batch("BEGIN").unwrap();
    assert!(c
        .execute(
            "INSERT INTO transcripts VALUES (0, 'session-00', 'ambient', 0, 500)",
            []
        )
        .is_err());
    c.execute_batch("ROLLBACK").unwrap();
    refresh(&c, JOT_SESSION_COUNTS.view_name).unwrap();
    assert_eq!(rows(&c, "SELECT * FROM jot_ambient_counts"), initial);

    c.execute_batch(
        "BEGIN; INSERT INTO transcripts VALUES
        (5000, 'session-00', 'ambient', 5000000, 5000500); ROLLBACK",
    )
    .unwrap();
    refresh(&c, JOT_SESSION_COUNTS.view_name).unwrap();
    assert_eq!(rows(&c, "SELECT * FROM jot_ambient_counts"), initial);
}

#[test]
fn openwhispr_speaker_rollup_matches_the_source_word_expression() {
    const SOURCE_SQL: &str = "SELECT s.speaker_name, COUNT(*),
        SUM(CASE WHEN trim(s.text) = '' THEN 0
                 ELSE length(trim(s.text)) - length(replace(trim(s.text), ' ', '')) + 1 END)
        FROM transcript_segments s JOIN notes n ON n.id=s.note_id
        WHERE s.speaker_name IS NOT NULL AND n.deleted_at IS NULL
        GROUP BY s.speaker_name";
    let c = open_with_extension(None).unwrap();
    c.execute_batch(OPENWHISPR.ddl).unwrap();
    OPENWHISPR.seed(&c, 1_200).unwrap();
    create(&c, OPENWHISPR.view_name, OPENWHISPR.view_sql).unwrap();

    for round in 0..=ROUNDS {
        if round > 0 {
            OPENWHISPR
                .apply_mixed_batch(&c, 1_200, 40, round - 1)
                .unwrap();
            refresh(&c, OPENWHISPR.view_name).unwrap();
        }
        assert_matches_oracle(&c, OPENWHISPR.view_name, OPENWHISPR.view_sql);
        let missing_projection: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM transcript_segments
                 WHERE word_count != CASE WHEN trim(text) = '' THEN 0
                     ELSE length(trim(text)) - length(replace(trim(text), ' ', '')) + 1 END",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing_projection, 0, "round {round}");
        assert_eq!(
            rows(
                &c,
                "SELECT * FROM openwhispr_speakers WHERE speaker_name IS NOT NULL"
            ),
            rows(&c, SOURCE_SQL),
            "source query round {round}"
        );
    }
}

/// Both maintenance implementations must handle zero sums, vanished groups,
/// orphan joins and parent-key changes, beyond the large fixture's mix.
fn check_edge_transitions(case: &DemandCase, initial: &str, steps: &[&str]) {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(case.ddl).unwrap();
    c.execute_batch(initial).unwrap();
    create(&c, case.view_name, case.view_sql).unwrap();
    let baseline = case.handwritten().unwrap();
    (baseline.install)(&c).unwrap();
    for sql in std::iter::once(&"").chain(steps.iter()) {
        c.execute_batch(sql).unwrap();
        refresh(&c, case.view_name).unwrap();
        assert_matches_oracle(&c, case.view_name, case.view_sql);
        assert_eq!(
            rows(&c, baseline.read_sql),
            rows(&c, case.view_sql),
            "{}: {sql}",
            case.name
        );
    }
}

#[test]
fn new_rollups_handle_group_removal_zero_sums_and_join_rekeys() {
    check_edge_transitions(
        &ZENOD,
        "INSERT INTO tenant_usage VALUES (1,1,'api',0)",
        &[
            "UPDATE tenant_usage SET tenant_id=2,kind='mcp'",
            "DELETE FROM tenant_usage",
        ],
    );
    check_edge_transitions(
        &NICTI,
        "INSERT INTO asset_keywords VALUES (1,1,'rust'),(2,1,'sqlite')",
        &[
            "INSERT INTO assets VALUES (1,'model',0)",
            "UPDATE assets SET id=2",
            "UPDATE asset_keywords SET asset_id=2",
            "UPDATE assets SET model='new',rating=5",
            "DELETE FROM assets",
            "DELETE FROM asset_keywords",
        ],
    );
    check_edge_transitions(
        &POSTIO,
        "INSERT INTO messages VALUES (1,1,0,0)",
        &[
            "UPDATE messages SET deleted_locally=1",
            "UPDATE messages SET deleted_locally=0",
            "UPDATE messages SET account_id=2",
            "DELETE FROM messages",
        ],
    );
    check_edge_transitions(
        &PHOENIX,
        "INSERT INTO spans VALUES (1,1,0,0,0),(2,1,0,10,20)",
        &[
            "INSERT INTO traces VALUES (1,1)",
            "UPDATE traces SET id=2",
            "UPDATE spans SET trace_id=2",
            "UPDATE traces SET project_id=3",
            "DELETE FROM spans WHERE id=2",
            "DELETE FROM traces",
        ],
    );
    check_edge_transitions(
        &POCKETBASE,
        "INSERT INTO books VALUES (1,'one')",
        &["UPDATE books SET author='two'", "DELETE FROM books"],
    );
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
