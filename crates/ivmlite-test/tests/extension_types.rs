//! M2c spec §2–§5 against the real loaded extension: storage classes,
//! affinity, capture, the new aggregates and MIN/MAX.

mod common;
use common::*;

use ivmlite_test::open_with_extension;
use rusqlite::Connection;

/// `pragma_table_info(<table>)`'s `(name, type)` rows, in column order.
fn declared(c: &Connection, table: &str) -> Vec<(String, String)> {
    let mut s = c
        .prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap();
    s.query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn types_of(c: &Connection, table: &str, columns: &[&str]) -> Vec<String> {
    let all = declared(c, table);
    columns
        .iter()
        .map(|name| {
            all.iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{table} has no column {name}: {all:?}"))
                .1
                .clone()
        })
        .collect()
}

#[test]
fn shadow_value_columns_have_no_declared_type() {
    // Spec §3: SQLite applies no affinity to these columns, so it stores every
    // value with its storage class unchanged.
    let c = open_with_extension(None).unwrap();
    setup(&c);
    create(&c, "sums", SUMS).unwrap();
    assert_eq!(
        types_of(&c, "__ivm_delta_orders", &["region", "amount"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_delta_orders", &["__ivm_seq", "__ivm_w"]),
        ["INTEGER", "INTEGER"]
    );
    assert_eq!(
        types_of(&c, "__ivm_pend_orders", &["region", "amount"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_out_sums", &["region", "SUM(amount)", "COUNT(*)"]),
        ["", "", ""]
    );
    assert_eq!(types_of(&c, "__ivm_out_sums", &["__w"]), ["INTEGER"]);
    assert_eq!(
        types_of(&c, "__ivm_stage_sums", &["c0", "c1", "c2", "key", "val"]),
        ["", "", "", "", ""]
    );
    assert_eq!(
        types_of(&c, "__ivm_state_sums_0_agg_groups", &["key", "val"]),
        ["", ""]
    );
    assert_eq!(
        types_of(&c, "sums", &["region", "SUM(amount)", "COUNT(*)"]),
        ["", "", ""]
    );
}
