//! Targeted, deterministic correctness fixtures for the M2a query shapes.
//!
//! Every case spells out its seed rows, its writes and the exact rows the view
//! must hold after each refresh. After every refresh `Db::check` asserts that
//! the view equals those explicit rows, equals SQLite's full recomputation of
//! the view SQL as a multiset, and that the decoded state validates.

use crate::m2a_runner::VIEW;
use crate::m2a_state::{capture_snapshot, validate_idle, validate_snapshot, Snapshot};
use ivmlite_test::demo_bench::{check_same_multiset, sorted_rows};
use ivmlite_test::open_with_extension;
use rusqlite::types::Value;
use rusqlite::Connection;

const JOIN_VIEW_SQL: &str =
    "SELECT lhs.g, COUNT(*), SUM(rhs.amount) FROM lhs JOIN rhs ON lhs.k = rhs.k GROUP BY lhs.g";
const SINGLE_VIEW_SQL: &str = "SELECT g, COUNT(*), SUM(amount) FROM events GROUP BY g";
/// Spec §2 join schema.
const JOIN_SCHEMA: &str =
    "CREATE TABLE lhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL, g INTEGER NOT NULL) STRICT;
     CREATE TABLE rhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL, amount INTEGER NOT NULL) STRICT;";
/// The join schema with nullable join keys, for the NULL-key case.
const NULL_KEY_JOIN_SCHEMA: &str =
    "CREATE TABLE lhs(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER NOT NULL) STRICT;
     CREATE TABLE rhs(id INTEGER PRIMARY KEY, k INTEGER, amount INTEGER NOT NULL) STRICT;";
const JOIN_INDEXES: &str = "CREATE INDEX lhs_k ON lhs(k); CREATE INDEX rhs_k ON rhs(k);";
/// Spec §2 single-table schema.
const SINGLE_SCHEMA: &str = "CREATE TABLE events(id INTEGER PRIMARY KEY, g INTEGER NOT NULL, amount INTEGER NOT NULL) STRICT;";

type Expected = [(i64, i64, Option<i64>)];

/// A connection with the extension loaded, one of the two M2a schemas and the
/// view created through the extension API. Refresh is always explicit.
struct Db {
    conn: Connection,
    join: bool,
}

impl Db {
    fn open(schema: &str, indexes: &str, join: bool) -> Self {
        let conn = open_with_extension(None).expect("fresh debug extension required");
        conn.execute_batch(schema).unwrap();
        conn.execute_batch(indexes).unwrap();
        Self { conn, join }
    }

    fn join() -> Self {
        Self::open(JOIN_SCHEMA, JOIN_INDEXES, true)
    }

    fn null_key_join() -> Self {
        Self::open(NULL_KEY_JOIN_SCHEMA, JOIN_INDEXES, true)
    }

    fn single() -> Self {
        Self::open(SINGLE_SCHEMA, "", false)
    }

    fn view_sql(&self) -> &'static str {
        if self.join {
            JOIN_VIEW_SQL
        } else {
            SINGLE_VIEW_SQL
        }
    }

    fn exec(&self, sql: &str) {
        self.conn.execute_batch(sql).unwrap();
    }

    fn create_view(&self) {
        self.exec(&format!(
            "CREATE VIRTUAL TABLE {VIEW} USING ivm('{}')",
            self.view_sql()
        ));
    }

    fn refresh(&self) {
        self.exec(&format!("INSERT INTO {VIEW}({VIEW}) VALUES('refresh')"));
    }

    /// Run the writes, refresh, and check the result against `expected`.
    fn refresh_and_check(&self, expected: &Expected) -> Snapshot {
        self.refresh();
        self.check(expected)
    }

    /// (a) explicit rows, (b) SQLite recomputation, (c) validated state.
    fn check(&self, expected: &Expected) -> Snapshot {
        let mut expected_sorted = expected.to_vec();
        expected_sorted.sort();
        let expected_rows: Vec<Vec<Value>> = expected_sorted
            .iter()
            .map(|&(g, n, sum)| {
                vec![
                    Value::Integer(g),
                    Value::Integer(n),
                    sum.map_or(Value::Null, Value::Integer),
                ]
            })
            .collect();
        let actual = sorted_rows(&self.conn, &format!("SELECT * FROM {VIEW}")).unwrap();
        assert_eq!(actual, expected_rows, "view rows differ from expected rows");
        check_same_multiset(
            &self.conn,
            &format!("SELECT * FROM {VIEW}"),
            self.view_sql(),
        )
        .unwrap();
        validate_snapshot(&self.conn, self.join, VIEW).unwrap()
    }
}

fn join_with(lhs: &str, rhs: &str) -> Db {
    let db = Db::join();
    db.exec(&format!("INSERT INTO lhs VALUES {lhs};"));
    if !rhs.is_empty() {
        db.exec(&format!("INSERT INTO rhs VALUES {rhs};"));
    }
    db.create_view();
    db
}

#[test]
fn lhs_join_key_update_moves_matches() {
    let db = join_with("(1,10,1)", "(1,10,5),(2,20,7)");
    db.check(&[(1, 1, Some(5))]);
    db.exec("UPDATE lhs SET k=20 WHERE id=1");
    db.refresh_and_check(&[(1, 1, Some(7))]);
}

#[test]
fn rhs_join_key_update_moves_matches() {
    let db = join_with("(1,10,1),(2,20,2)", "(1,10,5)");
    db.check(&[(1, 1, Some(5))]);
    db.exec("UPDATE rhs SET k=20 WHERE id=1");
    db.refresh_and_check(&[(2, 1, Some(5))]);
}

#[test]
fn duplicate_join_keys_multiply() {
    let db = join_with("(1,10,1),(2,10,1)", "(1,10,3),(2,10,4)");
    db.check(&[(1, 4, Some(14))]);
    db.exec("DELETE FROM rhs WHERE id=2");
    db.refresh_and_check(&[(1, 2, Some(6))]);
}

#[test]
fn simultaneous_left_and_right_deltas_count_the_delta_product_once() {
    let db = join_with("(1,10,1)", "(1,10,2)");
    db.check(&[(1, 1, Some(2))]);
    // 2 lhs x 2 rhs rows; sums 2+5+2+5. A delta-by-delta double count would
    // give (1,5,19) and a missing term (1,3,9).
    db.exec("BEGIN; INSERT INTO lhs VALUES (2,10,1); INSERT INTO rhs VALUES (2,10,5); COMMIT;");
    db.refresh_and_check(&[(1, 4, Some(14))]);
}

#[test]
fn orphan_becomes_matched() {
    let db = join_with("(1,10,1)", "");
    db.check(&[]);
    db.exec("INSERT INTO rhs VALUES (1,10,4)");
    db.refresh_and_check(&[(1, 1, Some(4))]);
}

#[test]
fn matched_becomes_orphan() {
    let db = join_with("(1,10,1)", "(1,10,4)");
    db.check(&[(1, 1, Some(4))]);
    db.exec("UPDATE rhs SET k=99 WHERE id=1");
    let snapshot = db.refresh_and_check(&[]);
    assert!(snapshot.retained_signature().output.is_empty());
    let output_rows: i64 = db
        .conn
        .query_row(&format!("SELECT count(*) FROM __ivm_out_{VIEW}"), [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(output_rows, 0, "the output table keeps no row for g=1");
}

#[test]
fn null_join_keys_never_match() {
    let db = Db::null_key_join();
    db.exec("INSERT INTO lhs VALUES (1,NULL,1),(2,10,1)");
    db.exec("INSERT INTO rhs VALUES (1,NULL,3),(2,10,4)");
    db.create_view();
    db.check(&[(1, 1, Some(4))]);
    db.exec("UPDATE lhs SET k=NULL WHERE id=2");
    db.refresh_and_check(&[]);
}

#[test]
fn group_appears_and_vanishes() {
    let db = Db::single();
    db.exec("INSERT INTO events VALUES (1,1,5)");
    db.create_view();
    db.check(&[(1, 1, Some(5))]);
    db.exec("INSERT INTO events VALUES (2,2,3)");
    db.refresh_and_check(&[(1, 1, Some(5)), (2, 1, Some(3))]);
    db.exec("DELETE FROM events WHERE id=2");
    db.refresh_and_check(&[(1, 1, Some(5))]);
    db.exec("DELETE FROM events WHERE id=1");
    db.refresh_and_check(&[]);
}

#[test]
fn positive_count_zero_sum_stays_visible() {
    let db = Db::single();
    db.exec("INSERT INTO events VALUES (1,1,5),(2,1,-5)");
    db.create_view();
    db.check(&[(1, 2, Some(0))]);
}

#[test]
fn rollback_leaves_view_state_and_pending_unchanged() {
    let db = join_with("(1,10,1)", "(1,10,4)");
    let a = db.refresh_and_check(&[(1, 1, Some(4))]);
    db.exec("BEGIN; INSERT INTO rhs VALUES (2,10,6); ROLLBACK;");
    let b = capture_snapshot(&db.conn, true, VIEW).unwrap();
    assert_eq!(b.pending, a.pending);
    assert!(b
        .pending
        .iter()
        .all(|p| p.delta_rows == 0 && p.replace_candidate_rows == 0));
    assert_eq!(b.retained_signature(), a.retained_signature());
    assert_eq!(b.base_rows, a.base_rows);
    assert_eq!(b.arrangements, a.arrangements);
    assert_eq!(b.stage_rows, a.stage_rows);
    assert_eq!(b.stage_operation_counts, a.stage_operation_counts);
    db.refresh_and_check(&[(1, 1, Some(4))]);
}

#[test]
fn repeated_idle_refresh_changes_nothing() {
    let db = join_with("(1,10,1)", "(1,10,5),(2,20,7)");
    db.check(&[(1, 1, Some(5))]);
    db.exec("UPDATE lhs SET k=20 WHERE id=1");
    let settled = db.refresh_and_check(&[(1, 1, Some(7))]);
    for _ in 0..2 {
        let idle = db.refresh_and_check(&[(1, 1, Some(7))]);
        validate_idle(&settled, &idle).unwrap();
        assert_eq!(idle.base_rows, settled.base_rows);
        assert_eq!(idle.arrangements, settled.arrangements);
        assert_eq!(idle.aggregate_state_rows, settled.aggregate_state_rows);
        assert_eq!(idle.output_rows, settled.output_rows);
        assert_eq!(idle.output_weight_total, settled.output_weight_total);
        assert_eq!(idle.pending, settled.pending);
        assert_eq!(idle.stage_rows, settled.stage_rows);
        assert_eq!(idle.retained_stage_rows, settled.retained_stage_rows);
        assert_eq!(idle.stage_operation_counts, settled.stage_operation_counts);
        assert_eq!(idle.retained_signature(), settled.retained_signature());
    }
}
