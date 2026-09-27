//! M1b Phase 3b spec §5 and §8 scenario 3: delta rows every reader has
//! consumed are deleted, atomically with the watermark that consumed them.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rusqlite::Connection;

const COUNTS: &str = "SELECT region, COUNT(*) FROM orders GROUP BY region";

fn deltas(c: &Connection) -> i64 {
    count(c, "SELECT count(*) FROM __ivm_delta_orders")
}

fn watermark(c: &Connection, view: &str) -> i64 {
    count(
        c,
        &format!("SELECT applied_seq FROM __ivm_progress WHERE view = '{view}' AND tbl = 'orders'"),
    )
}

#[test]
fn deltas_stay_until_every_reader_has_consumed_them() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "counts", COUNTS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7); UPDATE orders SET amount = 3 WHERE amount = 1;")
        .unwrap();
    let written = deltas(&c);
    assert_eq!(written, 4);
    refresh(&c, "sums").unwrap();
    // counts has not read them yet.
    assert_eq!(deltas(&c), written);
    refresh(&c, "counts").unwrap();
    assert_eq!(deltas(&c), 0);
    // A lagging reader keeps exactly the rows after its watermark.
    c.execute_batch("INSERT INTO orders VALUES ('d', 1)")
        .unwrap();
    refresh(&c, "counts").unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('e', 2), ('f', 3)")
        .unwrap();
    refresh(&c, "counts").unwrap();
    let lag = watermark(&c, "sums");
    assert_eq!(
        count(
            &c,
            &format!("SELECT count(*) FROM __ivm_delta_orders WHERE __ivm_seq <= {lag}")
        ),
        0
    );
    assert_eq!(deltas(&c), 3);
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 0);
    assert_matches_oracle(&c, "sums", SUMS);
    assert_matches_oracle(&c, "counts", COUNTS);
}

#[test]
fn dropping_the_slowest_reader_collects_what_only_it_held() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    create(&c, "counts", COUNTS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 2);
    c.execute_batch("DROP TABLE counts").unwrap();
    assert_eq!(deltas(&c), 0);
    c.execute_batch("INSERT INTO orders VALUES ('a', 1)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

/// Spec §4: once GC has emptied the delta table, `MAX(__ivm_seq)` is NULL,
/// but a new view must still start from the sequence AUTOINCREMENT will
/// continue from.
#[test]
fn a_view_created_over_an_emptied_delta_table_starts_from_sqlite_sequence() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('a', 10), ('c', 7)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    assert_eq!(deltas(&c), 0);
    let seq = count(
        &c,
        "SELECT seq FROM sqlite_sequence WHERE name = '__ivm_delta_orders'",
    );
    assert_eq!(seq, 2);
    create(&c, "counts", COUNTS).unwrap();
    assert_eq!(watermark(&c, "counts"), seq);
    c.execute_batch("INSERT INTO orders VALUES ('c', 1)")
        .unwrap();
    refresh(&c, "counts").unwrap();
    assert_matches_oracle(&c, "counts", COUNTS);
}

/// Spec §5: GC runs inside the one arming statement, so a failed deletion
/// rolls back state, output, watermarks and the delta table together.
#[test]
fn a_failed_gc_changes_nothing_and_a_retry_succeeds() {
    for explicit_transaction in [false, true] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        create(&c, "counts", COUNTS).unwrap();
        c.execute_batch(
            "CREATE TRIGGER fault BEFORE DELETE ON __ivm_delta_orders \
             BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        )
        .unwrap();
        if explicit_transaction {
            c.execute_batch("BEGIN").unwrap();
        }
        c.execute_batch("INSERT INTO orders VALUES ('c', 3), ('a', 9)")
            .unwrap();
        // sums goes first: counts still holds the rows, so nothing is deleted.
        refresh(&c, "sums").unwrap();
        let before = (durable_state(&c, "counts"), deltas(&c));
        let err = refresh(&c, "counts").expect_err("GC deletes, and the fault aborts it");
        assert!(err.to_string().contains("injected fault"), "{err}");
        assert_eq!(
            (durable_state(&c, "counts"), deltas(&c)),
            before,
            "transaction: {explicit_transaction}"
        );
        if explicit_transaction {
            assert!(!c.is_autocommit(), "the transaction must still be open");
        }
        c.execute_batch("DROP TRIGGER fault").unwrap();
        refresh(&c, "counts").unwrap();
        if explicit_transaction {
            c.execute_batch("COMMIT").unwrap();
        }
        assert_eq!(deltas(&c), 0);
        assert_matches_oracle(&c, "counts", COUNTS);
        assert_matches_oracle(&c, "sums", SUMS);
    }
}
