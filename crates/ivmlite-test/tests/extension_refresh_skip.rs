//! M1b Phase 7 spec §4: a refresh with no delta past any watermark returns
//! after its catalog checks, without computing, staging or applying.

mod common;

use common::*;
use ivmlite_test::open_with_extension;
use rusqlite::Connection;

fn total_changes(c: &Connection) -> i64 {
    count(c, "SELECT total_changes()")
}

/// Every stage row, each column rendered with `quote()`, in a stable order.
fn stage_dump(c: &Connection, view: &str) -> Vec<String> {
    let columns: Vec<String> = c
        .prepare(&format!(
            "SELECT name FROM pragma_table_info('__ivm_stage_{view}')"
        ))
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let quoted: Vec<String> = columns.iter().map(|n| format!("quote(\"{n}\")")).collect();
    let sql = format!(
        "SELECT {} FROM __ivm_stage_{view}",
        quoted.join(" || ',' || ")
    );
    let mut dump: Vec<String> = c
        .prepare(&sql)
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    dump.sort();
    dump
}

fn applied_seq(c: &Connection, view: &str) -> i64 {
    count(
        c,
        &format!("SELECT applied_seq FROM __ivm_progress WHERE view = '{view}' AND tbl = 'orders'"),
    )
}

#[test]
fn a_refresh_with_nothing_new_changes_nothing() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    c.execute_batch("INSERT INTO orders VALUES ('c', 9)")
        .unwrap();
    refresh(&c, "sums").unwrap();
    let changes = total_changes(&c);
    let stage = stage_dump(&c, "sums");
    assert!(!stage.is_empty(), "the previous apply left its rows staged");
    refresh(&c, "sums").unwrap();
    // SQLite counts the `INSERT INTO sums(sums)` statement itself as one
    // change; a skipped refresh writes no row of its own.
    assert_eq!(total_changes(&c) - changes, 1);
    assert_eq!(stage_dump(&c, "sums"), stage);
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_skipped_refresh_still_runs_its_checks() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    c.execute_batch(
        "DROP TABLE orders;
         CREATE TABLE orders(region TEXT, amount INTEGER) STRICT;",
    )
    .unwrap();
    let err = refresh(&c, "sums").expect_err("the capture triggers are gone");
    assert!(
        err.to_string().contains("cannot be maintained")
            && err
                .to_string()
                .contains("its capture trigger __ivm_trig_orders_ins is missing"),
        "{err}"
    );
}

#[test]
fn the_refresh_after_a_skip_applies_correctly() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    refresh(&c, "sums").unwrap(); // skipped
    c.execute_batch(
        "INSERT INTO orders VALUES ('a', 10), ('z', 3); DELETE FROM orders WHERE amount = 5;",
    )
    .unwrap();
    refresh(&c, "sums").unwrap();
    assert_matches_oracle(&c, "sums", SUMS);
}

#[test]
fn a_consolidated_empty_delta_still_moves_the_watermark() {
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    refresh(&c, "sums").unwrap();
    let before = applied_seq(&c, "sums");
    c.execute_batch("INSERT INTO orders VALUES ('q', 77); DELETE FROM orders WHERE amount = 77;")
        .unwrap();
    assert!(count(&c, "SELECT count(*) FROM __ivm_delta_orders") > 0);
    refresh(&c, "sums").unwrap();
    assert!(applied_seq(&c, "sums") > before);
    assert_eq!(count(&c, "SELECT count(*) FROM __ivm_delta_orders"), 0);
    assert_matches_oracle(&c, "sums", SUMS);
}
