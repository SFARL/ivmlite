//! M1b Phase 3b spec §4 and §8 scenario 2: several views over one base
//! table share its capture.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rusqlite::Connection;

const COUNTS: &str = "SELECT region, COUNT(*) FROM orders GROUP BY region";

/// Capture triggers per base table (spec §3; Task 4 raises it to 5).
const CAPTURE_TRIGGERS: i64 = 3;

fn capture_objects(c: &Connection, table: &str) -> i64 {
    count(
        c,
        &format!(
            "SELECT count(*) FROM sqlite_schema WHERE name = '__ivm_delta_{table}' \
             OR (type = 'trigger' AND tbl_name = '{table}' AND name LIKE '\\_\\_ivm\\_trig\\_%' ESCAPE '\\')"
        ),
    )
}

#[test]
fn views_over_one_table_share_its_capture_and_each_matches_its_oracle() {
    let file = TempFile::new("sharing");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        create(&c, "counts", COUNTS).unwrap();
        create(&c, "managers", JOIN).unwrap();
        // One delta table and one trigger per capture event, however many views read it.
        assert_eq!(capture_objects(&c, "orders"), 1 + CAPTURE_TRIGGERS);
        assert_eq!(capture_objects(&c, "regions"), 1 + CAPTURE_TRIGGERS);
        c.execute_batch(
            "INSERT INTO orders VALUES ('a', 10), ('c', 7); DELETE FROM orders WHERE region = 'b';
             UPDATE regions SET manager = 'cy' WHERE name = 'c';",
        )
        .unwrap();
        refresh(&c, "counts").unwrap();
        assert_matches_oracle(&c, "counts", COUNTS);
        c.execute_batch("UPDATE orders SET amount = 100 WHERE amount = 1")
            .unwrap();
        refresh(&c, "sums").unwrap();
        refresh(&c, "managers").unwrap();
        refresh(&c, "counts").unwrap();
        for (view, sql) in [("sums", SUMS), ("counts", COUNTS), ("managers", JOIN)] {
            assert_matches_oracle(&c, view, sql);
        }
    }
    {
        let plain = Connection::open(file.path()).unwrap();
        plain
            .execute_batch(
                "INSERT INTO orders VALUES ('c', 1); DELETE FROM regions WHERE name = 'a';",
            )
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    for (view, sql) in [("managers", JOIN), ("sums", SUMS), ("counts", COUNTS)] {
        refresh(&c, view).unwrap();
        assert_matches_oracle(&c, view, sql);
    }
}

#[test]
fn dropping_views_in_any_order_leaves_the_others_correct_and_the_last_leaves_nothing() {
    let views = [("sums", SUMS), ("counts", COUNTS), ("managers", JOIN)];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let c = open_with_extension(None).unwrap();
        setup(&c);
        let user_objects = objects(&c);
        for (view, sql) in views {
            create(&c, view, sql).unwrap();
        }
        let mut alive: Vec<usize> = vec![0, 1, 2];
        for (step, &gone) in order.iter().enumerate() {
            c.execute_batch(&format!("DROP TABLE {}", views[gone].0))
                .unwrap();
            alive.retain(|&i| i != gone);
            c.execute_batch(&format!(
                "INSERT INTO orders VALUES ('a', {step}), ('z', 1); UPDATE regions SET manager = 'm{step}' WHERE name = 'b';"
            ))
            .unwrap();
            for &i in &alive {
                refresh(&c, views[i].0).unwrap();
                assert_matches_oracle(&c, views[i].0, views[i].1);
            }
        }
        assert_eq!(objects(&c), user_objects, "drop order {order:?}");
        c.execute_batch(
            "INSERT INTO orders VALUES ('a', 1); INSERT INTO regions VALUES ('q', 'q');",
        )
        .unwrap();
    }
}

/// Spec §4: a view created while another view's deltas are unconsumed starts
/// from the table's current sequence number; replaying the older rows would
/// count them twice (they are already in the bootstrap's scan).
#[test]
fn a_view_created_later_starts_from_the_current_sequence() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch(
        "INSERT INTO orders VALUES ('a', 10), ('c', 7); DELETE FROM orders WHERE region = 'b';",
    )
    .unwrap();
    create(&c, "counts", COUNTS).unwrap();
    let seq = count(
        &c,
        "SELECT seq FROM sqlite_sequence WHERE name = '__ivm_delta_orders'",
    );
    assert!(seq > 0);
    assert_eq!(
        count(
            &c,
            "SELECT applied_seq FROM __ivm_progress WHERE view = 'counts' AND tbl = 'orders'"
        ),
        seq
    );
    refresh(&c, "counts").unwrap();
    assert_matches_oracle(&c, "counts", COUNTS);
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_tracked_table_whose_capture_is_broken_refuses_a_new_view() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("DROP TRIGGER __ivm_trig_orders_ins")
        .unwrap();
    let err = create(&c, "counts", COUNTS).expect_err("orders is no longer captured");
    let err = err.to_string();
    assert!(
        err.contains("__ivm_trig_orders_ins is missing") && err.contains("sums"),
        "{err}"
    );
}

#[test]
fn a_database_in_format_1_is_refused_by_create_and_by_connect() {
    let file = TempFile::new("format1");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        setup(&c);
        create(&c, "sums", SUMS).unwrap();
        c.execute_batch("UPDATE __ivm_meta SET value = 1; UPDATE __ivm_view SET format = 1;")
            .unwrap();
        let err = create(&c, "counts", COUNTS).expect_err("format 1");
        assert!(err.to_string().contains("format 1"), "{err}");
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    let err = c.execute_batch("SELECT * FROM sums").expect_err("format 1");
    assert!(err.to_string().contains("format 1"), "{err}");
    c.execute_batch("DROP TABLE sums").unwrap();
}
