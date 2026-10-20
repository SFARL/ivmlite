//! Source-shaped capability acceptance tests. The Jot MIN/MAX/COUNT projection
//! runs in ordinary CI; the three remaining unsupported queries are ignored.
//! Run a red gate with
//! `cargo test -p ivmlite-test --test capability_red <name> -- --ignored`.
//! A green result must include parity after changes, not merely CREATE success.

mod common;
use common::{assert_matches_oracle, create, refresh, rows};
use ivmlite_test::demand_cases::JOT_SESSION_COUNTS;
use ivmlite_test::demo_bench::Workload;
use ivmlite_test::open_with_extension;

#[test]
#[ignore = "red: exact P95 needs ordered rank/select maintenance and global query support"]
fn cpa_exact_p95_matches_ranked_sqlite_after_append() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE usage_events(
            id INTEGER PRIMARY KEY, timestamp_ms INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL, ttft_ms INTEGER NOT NULL
        ) STRICT;
        INSERT INTO usage_events VALUES
            (1, 100, 10, 5), (2, 200, 30, 15), (3, 300, 20, 0);",
    )
    .unwrap();
    // Adapted only to fixed boundaries; the upstream uses the same nearest-rank
    // ROW_NUMBER/COUNT calculation with separately filtered latency/TTFT rows.
    let sql = "WITH samples(kind, value) AS (
        SELECT 'latency', latency_ms FROM usage_events
          WHERE timestamp_ms >= 0 AND timestamp_ms < 1000 AND latency_ms > 0
        UNION ALL
        SELECT 'ttft', ttft_ms FROM usage_events
          WHERE timestamp_ms >= 0 AND timestamp_ms < 1000 AND ttft_ms > 0
    ), ranked AS (
        SELECT kind, value,
          ROW_NUMBER() OVER (PARTITION BY kind ORDER BY value) AS sample_number,
          COUNT(*) OVER (PARTITION BY kind) AS sample_count
        FROM samples
    )
    SELECT kind, value FROM ranked
    WHERE sample_number = ((sample_count * 95) + 99) / 100";
    assert!(
        !rows(&c, sql).is_empty(),
        "SQLite oracle must produce P95 rows"
    );
    create(&c, "cpa_p95", sql).unwrap();
    assert_matches_oracle(&c, "cpa_p95", sql);
    c.execute("INSERT INTO usage_events VALUES (4, 400, 100, 80)", [])
        .unwrap();
    refresh(&c, "cpa_p95").unwrap();
    assert_matches_oracle(&c, "cpa_p95", sql);
}

#[test]
#[ignore = "red: task buckets need CASE/NULLIF expressions, DISTINCT GROUP_CONCAT, and ORDER BY/LIMIT"]
fn cpa_task_buckets_match_sqlite_after_new_task() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(
        "CREATE TABLE usage_events(
            id INTEGER PRIMARY KEY, bucket_key TEXT NOT NULL,
            model TEXT NOT NULL, endpoint TEXT NOT NULL,
            timestamp_ms INTEGER NOT NULL, failed INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL, latency_ms INTEGER NOT NULL
        ) STRICT;
        INSERT INTO usage_events VALUES
            (1, 'task-a', 'm1', '/a', 100, 0, 10, 20),
            (2, 'task-a', 'm2', '/b', 150, 1, 20, 30),
            (3, 'task-b', 'm1', '/a', 200, 0, 5, 10);",
    )
    .unwrap();
    let sql = "SELECT bucket_key, COUNT(*),
        SUM(CASE WHEN failed=1 THEN 1 ELSE 0 END),
        MIN(timestamp_ms), MAX(timestamp_ms),
        GROUP_CONCAT(DISTINCT model), GROUP_CONCAT(DISTINCT endpoint),
        SUM(input_tokens), AVG(NULLIF(latency_ms, 0)), MAX(latency_ms)
        FROM usage_events GROUP BY bucket_key
        ORDER BY MAX(timestamp_ms) DESC LIMIT 500";
    assert!(
        !rows(&c, sql).is_empty(),
        "SQLite oracle must produce task rows"
    );
    create(&c, "cpa_task_buckets", sql).unwrap();
    assert_matches_oracle(&c, "cpa_task_buckets", sql);
    c.execute(
        "INSERT INTO usage_events VALUES (4, 'task-c', 'm3', '/c', 300, 0, 8, 40)",
        [],
    )
    .unwrap();
    refresh(&c, "cpa_task_buckets").unwrap();
    assert_matches_oracle(&c, "cpa_task_buckets", sql);
}

#[test]
fn jot_min_max_count_matches_sqlite_after_mixed_changes() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(JOT_SESSION_COUNTS.ddl()).unwrap();
    JOT_SESSION_COUNTS.seed(&c, 1_200).unwrap();
    let sql = "SELECT session_id, MIN(started_ms), MAX(ended_ms), COUNT(*)
        FROM transcripts WHERE mode='ambient' GROUP BY session_id";
    assert!(
        !rows(&c, sql).is_empty(),
        "SQLite oracle must produce sessions"
    );
    create(&c, "jot_session_bounds", sql).unwrap();
    assert_matches_oracle(&c, "jot_session_bounds", sql);
    for round in 0..3 {
        JOT_SESSION_COUNTS
            .apply_mixed_batch(&c, 1_200, 40, round)
            .unwrap();
        refresh(&c, "jot_session_bounds").unwrap();
        assert_matches_oracle(&c, "jot_session_bounds", sql);
    }
}

#[test]
#[ignore = "red: full sessions query needs LEFT JOIN, an ungrouped title, and ORDER BY/LIMIT"]
fn jot_full_session_list_matches_sqlite_after_title_change() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch(JOT_SESSION_COUNTS.ddl()).unwrap();
    JOT_SESSION_COUNTS.seed(&c, 1_200).unwrap();
    let sql = "SELECT t.session_id, MIN(t.started_ms), MAX(t.ended_ms),
        COUNT(*), s.title
        FROM transcripts t LEFT JOIN session_titles s ON s.session_id=t.session_id
        WHERE t.mode='ambient' GROUP BY t.session_id
        ORDER BY MAX(t.ended_ms) DESC, t.session_id LIMIT 100";
    assert!(
        !rows(&c, sql).is_empty(),
        "SQLite oracle must produce sessions"
    );
    create(&c, "jot_full_sessions", sql).unwrap();
    assert_matches_oracle(&c, "jot_full_sessions", sql);
    c.execute(
        "UPDATE session_titles SET title='Renamed' WHERE session_id='session-00'",
        [],
    )
    .unwrap();
    refresh(&c, "jot_full_sessions").unwrap();
    assert_matches_oracle(&c, "jot_full_sessions", sql);
}
